//! Progressive picture display: decode the part of a picture that has arrived so far.
//!
//! The engine hands over the leading bytes of a file up to its first missing
//! segment. JPEG 2000 and JPEG decoders can both make a picture of a cut-off file:
//! JPEG 2000 (what EasyPal and our transcoder send) shows the whole frame and sharpens
//! as more arrives, baseline JPEG fills in from the top. A worker thread does the
//! decoding so the window never waits for it.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

/// A decoded partial picture.
pub struct Partial {
    pub tid: u16,
    /// Bytes of the file this was decoded from.
    pub bytes: usize,
    pub rgba: Vec<u8>,
    pub size: [usize; 2],
}

struct Job {
    tid: u16,
    data: Arc<Vec<u8>>,
}

/// Decode the start of a picture file. `None` when too little has arrived or the
/// format cannot be shown part-way.
pub fn decode_partial(data: &[u8]) -> Option<image::RgbaImage> {
    let jp2 = data.starts_with(&[0, 0, 0, 0x0C, b'j', b'P', b' ', b' '])
        || data.starts_with(&[0xFF, 0x4F, 0xFF, 0x51]);
    if jp2 {
        return decode_jp2(data);
    }
    if data.starts_with(&[0xFF, 0xD8]) {
        return decode_jpeg(data);
    }
    None
}

/// A cut-off JPEG 2000 file. With several quality layers (our transcoder) the plain
/// decode is already a full-size picture that sharpens. A single-layer file arrives
/// resolution by resolution, and OpenJPEG then returns a small picture in the corner
/// of a black frame, or mixes components of different sizes. So: decode at every
/// resolution from the coarsest up and keep the finest one that still agrees with
/// the one below it.
fn decode_jp2(data: &[u8]) -> Option<image::RgbaImage> {
    let at = |reduce: u32| -> Option<image::RgbaImage> {
        let params = jpeg2k::DecodeParameters::new().strict(false).reduce(reduce);
        let img = jpeg2k::Image::from_bytes_with(data, params).ok()?;
        Some(image::DynamicImage::try_from(&img).ok()?.to_rgba8())
    };
    let mut best: Option<image::RgbaImage> = None;
    let mut level = 0;
    for reduce in (0..=5).rev() {
        let Some(img) = at(reduce) else { continue };
        if let Some(prev) = &best {
            let small = image::imageops::resize(
                &img,
                prev.width(),
                prev.height(),
                image::imageops::FilterType::Triangle,
            );
            // Measured: 2 to 10 between good levels, 27 and up for a broken one.
            if mean_diff(&small, prev) > 18.0 {
                break;
            }
        }
        best = Some(img);
        level = reduce;
    }
    // Always hand back the full frame size, so the picture does not jump as the
    // finer resolutions come in.
    let img = best?;
    if level == 0 {
        return Some(img);
    }
    Some(image::imageops::resize(
        &img,
        img.width() << level,
        img.height() << level,
        image::imageops::FilterType::Triangle,
    ))
}

/// Mean absolute difference of two pictures of the same size (0..255).
fn mean_diff(a: &image::RgbaImage, b: &image::RgbaImage) -> f64 {
    let s: u64 = a
        .as_raw()
        .iter()
        .zip(b.as_raw())
        .map(|(x, y)| u64::from(x.abs_diff(*y)))
        .sum();
    s as f64 / a.as_raw().len().max(1) as f64
}

/// A cut-off JPEG: the rows not yet received stay grey.
fn decode_jpeg(data: &[u8]) -> Option<image::RgbaImage> {
    use image::ImageDecoder;
    let dec = image::codecs::jpeg::JpegDecoder::new(std::io::Cursor::new(data)).ok()?;
    let (w, h) = dec.dimensions();
    let color = dec.color_type();
    let mut buf = vec![0x30u8; dec.total_bytes() as usize];
    // The decoder reports the missing end as an error but has filled what it could.
    let _ = dec.read_image(&mut buf);
    let img = match color {
        image::ColorType::Rgb8 => {
            image::DynamicImage::ImageRgb8(image::RgbImage::from_raw(w, h, buf)?)
        }
        image::ColorType::L8 => {
            image::DynamicImage::ImageLuma8(image::GrayImage::from_raw(w, h, buf)?)
        }
        _ => return None,
    };
    Some(img.to_rgba8())
}

/// The worker thread. Requests made while it is busy are skipped except the newest.
pub struct Previewer {
    tx: Sender<Job>,
    rx: Receiver<Partial>,
    /// (transport id, bytes) of the newest request.
    asked: Option<(u16, usize)>,
    pending: usize,
}

impl Previewer {
    pub fn new() -> Self {
        let (tx, jobs) = channel::<Job>();
        let (out, rx) = channel();
        std::thread::Builder::new()
            .name("picture-preview".into())
            .spawn(move || {
                while let Ok(mut job) = jobs.recv() {
                    let mut skipped = 0;
                    while let Ok(newer) = jobs.try_recv() {
                        job = newer;
                        skipped += 1;
                    }
                    let img = decode_partial(&job.data);
                    let (rgba, size) = match img {
                        Some(i) => {
                            let size = [i.width() as usize, i.height() as usize];
                            (i.into_raw(), size)
                        }
                        None => (Vec::new(), [0, 0]),
                    };
                    for _ in 0..skipped {
                        // Skipped jobs still count as answered.
                        let _ = out.send(Partial {
                            tid: job.tid,
                            bytes: 0,
                            rgba: Vec::new(),
                            size: [0, 0],
                        });
                    }
                    if out
                        .send(Partial {
                            tid: job.tid,
                            bytes: job.data.len(),
                            rgba,
                            size,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .expect("spawn previewer");
        Self {
            tx,
            rx,
            asked: None,
            pending: 0,
        }
    }

    /// Ask for a decode of the first bytes of object `tid` unless this very prefix
    /// was already asked for or the worker is still busy.
    pub fn request(&mut self, tid: u16, data: &Arc<Vec<u8>>) {
        if self.pending > 0 || self.asked == Some((tid, data.len())) {
            return;
        }
        self.asked = Some((tid, data.len()));
        self.pending += 1;
        let _ = self.tx.send(Job {
            tid,
            data: data.clone(),
        });
    }

    /// The newest finished decode that produced a picture.
    pub fn poll(&mut self) -> Option<Partial> {
        let mut last = None;
        while let Ok(p) = self.rx.try_recv() {
            self.pending = self.pending.saturating_sub(1);
            if p.size[0] > 0 {
                last = Some(p);
            }
        }
        last
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture() -> image::DynamicImage {
        let img = image::RgbImage::from_fn(320, 240, |x, y| {
            let c = ((x / 40 + y / 40) % 2) as u8 * 60;
            image::Rgb([(x * 255 / 320) as u8, (y * 255 / 240) as u8, 90 + c])
        });
        image::DynamicImage::ImageRgb8(img)
    }

    fn dump(name: &str, img: &image::RgbaImage) {
        if let Ok(dir) = std::env::var("PROGRESSIVE_DUMP") {
            let _ = img.save(std::path::Path::new(&dir).join(name));
        }
    }

    #[test]
    fn cut_off_jpeg2000_gets_better_with_more_bytes() {
        let src = picture();
        let params = jpeg2k::EncodeParameters::new()
            .irreversible(true)
            .layer_rate(20.0);
        let jp2 = jpeg2k::Image::from_dynamic_image(&src)
            .encode_jp2(params)
            .unwrap();
        let full = decode_partial(&jp2).unwrap();
        let mut last = f64::MAX;
        for pct in [25, 50, 75] {
            let part = decode_partial(&jp2[..jp2.len() * pct / 100])
                .unwrap_or_else(|| panic!("{pct}% of {} bytes", jp2.len()));
            dump(&format!("jp2-{pct}.png"), &part);
            let part = image::imageops::resize(
                &part,
                full.width(),
                full.height(),
                image::imageops::FilterType::Triangle,
            );
            let e = mean_diff(&part, &full);
            assert!(e < 20.0, "{pct}%: {e}");
            eprintln!("jp2 {pct}%: error {e:.2}");
            assert!(e <= last, "{pct}%: {e} after {last}");
            last = e;
        }
    }

    /// Contact sheet of cut-off decodes (only with PROGRESSIVE_DUMP and a picture in
    /// PROGRESSIVE_SRC): rows = 1 layer, the transcoder's layers; columns = 10 % to 90 %.
    #[test]
    fn contact_sheet() {
        let (Ok(_), Ok(srcp)) = (
            std::env::var("PROGRESSIVE_DUMP"),
            std::env::var("PROGRESSIVE_SRC"),
        ) else {
            return;
        };
        let src = image::open(srcp).unwrap().thumbnail(480, 480);
        let src = image::DynamicImage::ImageRgb8(src.to_rgb8());
        let (w, h) = (src.width(), src.height());
        let mut sheet = image::RgbaImage::new(w * 9, h * 2);
        for (row, layers) in [1usize, 8].into_iter().enumerate() {
            let params = if layers == 1 {
                jpeg2k::EncodeParameters::new()
                    .irreversible(true)
                    .layer_rate(30.0)
            } else {
                crate::transcode::encode_params(30.0)
            };
            let jp2 = jpeg2k::Image::from_dynamic_image(&src)
                .encode_jp2(params)
                .unwrap();
            eprintln!("{layers} layers: {} bytes", jp2.len());
            if let Ok(dir) = std::env::var("PROGRESSIVE_DUMP") {
                let _ = std::fs::write(
                    std::path::Path::new(&dir).join(format!("layers{layers}.jp2")),
                    &jp2,
                );
            }
            for c in 0..9u32 {
                let n = jp2.len() * (c as usize + 1) / 10;
                if let Some(img) = decode_partial(&jp2[..n]) {
                    let img =
                        image::imageops::resize(&img, w, h, image::imageops::FilterType::Triangle);
                    image::imageops::replace(
                        &mut sheet,
                        &img,
                        i64::from(c * w),
                        i64::from(row as u32 * h),
                    );
                } else {
                    eprintln!("{layers} layers {}%: nothing", (c + 1) * 10);
                }
            }
        }
        dump("sheet.png", &sheet);
    }

    #[test]
    fn cut_off_jpeg_shows_the_top() {
        let src = picture();
        let mut jpg = Vec::new();
        src.write_to(
            &mut std::io::Cursor::new(&mut jpg),
            image::ImageFormat::Jpeg,
        )
        .unwrap();
        let full = decode_partial(&jpg).unwrap();
        let part = decode_partial(&jpg[..jpg.len() / 2]).expect("half a JPEG");
        dump("jpg-50.png", &part);
        assert_eq!(part.dimensions(), full.dimensions());
        // The top quarter is already right.
        let rows = 60 * 320 * 4;
        assert_eq!(part.as_raw()[..rows], full.as_raw()[..rows]);
    }

    #[test]
    fn other_data_is_refused() {
        assert!(decode_partial(b"hello, this is not a picture").is_none());
        assert!(decode_partial(&[0xFF, 0xD8]).is_none());
    }
}
