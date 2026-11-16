//! Picture → JPEG 2000 transcoder with a live preview: a worker thread re-encodes
//! the picture whenever the quality or size setting changes and decodes the result
//! again, so the window shows exactly what the other station will see, with its size
//! in bytes. JPEG 2000 (what EasyPal sends) degrades gracefully at the tiny sizes that
//! fit a 2.5 kHz channel. Codec: `jpeg2k` / `openjp2` (pure-Rust OpenJPEG port).

use image::DynamicImage;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

/// Open any picture this build reads, JPEG 2000 included.
pub fn open_picture(path: &Path) -> Option<DynamicImage> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if matches!(ext.as_str(), "jp2" | "j2k" | "j2c" | "jpc") {
        let img = jpeg2k::Image::from_file(path).ok()?;
        return DynamicImage::try_from(&img).ok();
    }
    image::open(path).ok().or_else(|| {
        // Wrong or missing extension: try JPEG 2000 by content.
        let data = std::fs::read(path).ok()?;
        let img = jpeg2k::Image::from_bytes(&data).ok()?;
        DynamicImage::try_from(&img).ok()
    })
}

/// Settings of one encode.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    /// 1 (smallest file) ..= 100 (near lossless).
    pub quality: u32,
    /// Longest side in pixels after scaling.
    pub max_side: u32,
}

impl Settings {
    /// Compression ratio (raw RGB bytes : file bytes) for the quality setting:
    /// 2:1 at 100 down to 400:1 at 1, on a logarithmic scale.
    pub fn ratio(&self) -> f32 {
        let q = self.quality.clamp(1, 100) as f32;
        2.0 * 200f32.powf((100.0 - q) / 99.0)
    }
}

struct Job {
    source: Arc<DynamicImage>,
    settings: Settings,
    seq: u64,
}

/// A finished encode.
pub struct Encoded {
    pub seq: u64,
    pub settings: Settings,
    pub jp2: Vec<u8>,
    /// The JPEG 2000 file decoded again (RGBA) for the preview.
    pub rgba: Vec<u8>,
    pub size: [usize; 2],
    pub millis: u128,
}

/// The worker thread. Jobs queued while it is busy are skipped except the newest.
pub struct Transcoder {
    tx: Sender<Job>,
    rx: Receiver<Result<Encoded, String>>,
    seq: u64,
    /// Sequence number of the newest result received.
    pub done_seq: u64,
}

fn encode(job: &Job) -> Result<Encoded, String> {
    let t0 = std::time::Instant::now();
    let src = &job.source;
    let side = src.width().max(src.height());
    let scaled = if side > job.settings.max_side {
        src.resize(
            job.settings.max_side,
            job.settings.max_side,
            image::imageops::FilterType::Lanczos3,
        )
    } else {
        (**src).clone()
    };
    let rgb = DynamicImage::ImageRgb8(scaled.to_rgb8());
    let params = jpeg2k::EncodeParameters::new()
        .irreversible(true)
        .layer_rate(job.settings.ratio());
    let jp2 = jpeg2k::Image::from_dynamic_image(&rgb)
        .encode_jp2(params)
        .map_err(|e| e.to_string())?;
    let back = jpeg2k::Image::from_bytes(&jp2).map_err(|e| e.to_string())?;
    let back = DynamicImage::try_from(&back)
        .map_err(|e| e.to_string())?
        .to_rgba8();
    Ok(Encoded {
        seq: job.seq,
        settings: job.settings,
        size: [back.width() as usize, back.height() as usize],
        rgba: back.into_raw(),
        jp2,
        millis: t0.elapsed().as_millis(),
    })
}

impl Transcoder {
    pub fn new() -> Self {
        let (tx, jobs) = channel::<Job>();
        let (out, rx) = channel();
        std::thread::Builder::new()
            .name("jp2-transcoder".into())
            .spawn(move || {
                while let Ok(mut job) = jobs.recv() {
                    // Only the newest request matters while the slider moves.
                    while let Ok(newer) = jobs.try_recv() {
                        job = newer;
                    }
                    if out.send(encode(&job)).is_err() {
                        break;
                    }
                }
            })
            .expect("spawn transcoder");
        Self {
            tx,
            rx,
            seq: 0,
            done_seq: 0,
        }
    }

    /// Ask for an encode; returns its sequence number.
    pub fn request(&mut self, source: Arc<DynamicImage>, settings: Settings) -> u64 {
        self.seq += 1;
        let _ = self.tx.send(Job {
            source,
            settings,
            seq: self.seq,
        });
        self.seq
    }

    pub fn busy(&self) -> bool {
        self.done_seq < self.seq
    }

    /// The newest finished encode, if any arrived since the last call.
    pub fn poll(&mut self) -> Option<Result<Encoded, String>> {
        let mut last = None;
        while let Ok(r) = self.rx.try_recv() {
            if let Ok(e) = &r {
                self.done_seq = e.seq;
            } else {
                self.done_seq = self.seq;
            }
            last = Some(r);
        }
        last
    }
}

/// Where transcoded pictures are written before they are sent.
pub fn tx_dir() -> PathBuf {
    std::env::var_os("APPDATA")
        .or_else(|| std::env::var_os("HOME"))
        .map(|d| PathBuf::from(d).join("DecHAMDRM").join("Tx Files"))
        .unwrap_or_else(|| PathBuf::from("Tx Files"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lower_quality_gives_a_smaller_file_that_still_decodes() {
        let img = DynamicImage::ImageRgb8(image::RgbImage::from_fn(160, 120, |x, y| {
            image::Rgb([
                (x * 3 % 256) as u8,
                (y * 5 % 256) as u8,
                ((x + y) % 256) as u8,
            ])
        }));
        let src = Arc::new(img);
        let enc = |q| {
            encode(&Job {
                source: src.clone(),
                settings: Settings {
                    quality: q,
                    max_side: 160,
                },
                seq: 1,
            })
            .unwrap()
        };
        let (hi, lo) = (enc(90), enc(20));
        assert!(
            lo.jp2.len() < hi.jp2.len(),
            "{} vs {}",
            lo.jp2.len(),
            hi.jp2.len()
        );
        assert_eq!(lo.size, [160, 120]);
        assert_eq!(lo.rgba.len(), 160 * 120 * 4);
        // Scaling down.
        let small = encode(&Job {
            source: src,
            settings: Settings {
                quality: 50,
                max_side: 80,
            },
            seq: 2,
        })
        .unwrap();
        assert_eq!(small.size, [80, 60]);
    }
}
