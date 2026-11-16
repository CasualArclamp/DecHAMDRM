//! Waterfall text ("waterfall ID"): the callsign or a short message drawn on the
//! receiver's waterfall, as EasyDRF's G / B / ID buttons do (`docs/spec/app.md` §6.1;
//! EasyDRF `WFText.cpp`, 8×8 font from ChirpHel by DF6NM via `WFText.h`).
//!
//! Each of the 8 glyph rows (bottom row first, so the text reads upright on a
//! waterfall scrolling downwards) is one IFFT block of N bins at 12000/N Hz spacing;
//! a lit pixel column becomes a run of bins with a quadratic (chirp) phase that spreads
//! the block's energy over time, which keeps the peak-to-average ratio low. The block
//! is repeated `rep` times. EasyDRF computes at 12 kHz; we compute the same spectrum
//! on a 4N-point IFFT at 48 kHz and shift it with a continuous mixer.

use crate::params::SAMPLE_RATE;
use crate::{Cplx, Real};
use decdrm_core::dsp::fft::Fft;
use std::f64::consts::PI;

/// Longest text (EasyDRF `WFText.cpp:82-117`).
pub const MAX_CHARS: usize = 20;

/// ASCII 32..=127 glyphs: pixel (x, y) (x = column from the left, y = row from the
/// top) is bit `(7 − x)·8 + y`.
const FONT: [u64; 96] = [
    0x0000000000000000,
    0x0000065f5f060000,
    0x0003070000070300,
    0x147f7f147f7f1400,
    0x00242e6b6b3a1200,
    0x466630180c666200,
    0x307a4f5d377a4800,
    0x0000040703000000,
    0x00001c3e63410000,
    0x000041633e1c0000,
    0x082a3e1c1c3e2a08,
    0x0008083e3e080800,
    0x000080e060000000,
    0x0008080808080800,
    0x0000006060000000,
    0x6030180c06030100,
    0x1c3e6349633e1c00,
    0x0040427f7f404000,
    0x42637159496f6600,
    0x22634949497f3600,
    0x181c16537f7f5000,
    0x2f6f494949793100,
    0x3c7e4b4949783000,
    0x030371790d070300,
    0x367f4949497f3600,
    0x064f4949693f1e00,
    0x0000006666000000,
    0x000080e666000000,
    0x0000081c36634100,
    0x0024242424242400,
    0x004163361c080000,
    0x020301595d070200,
    0x3e7f415d5d1f1e00,
    0x7c7e0b090b7e7c00,
    0x417f7f49497f3600,
    0x1c3e634141632200,
    0x417f7f41633e1c00,
    0x417f7f495d416300,
    0x417f7f491d010300,
    0x1c3e634151337200,
    0x7f7f0808087f7f00,
    0x0000417f7f410000,
    0x307040417f3f0100,
    0x417f7f081c776300,
    0x417f7f4140607000,
    0x7f7f0e1c0e7f7f00,
    0x7f7f060c187f7f00,
    0x3e7f4141417f3e00,
    0x417f7f49090f0600,
    0x3e7f4141e1ffbe00,
    0x417f7f09197f6600,
    0x0022674d59732200,
    0x0007437f7f430700,
    0x3f7f4040407f3f00,
    0x1f3f6040603f1f00,
    0x3f7f6038607f3f00,
    0x63771c081c776300,
    0x00074f78784f0700,
    0x476371594d677300,
    0x00007f7f41410000,
    0x0103060c18306000,
    0x000041417f7f0000,
    0x080c0603060c0800,
    0x8080808080808080,
    0x0000010306040000,
    0x207454543c784000,
    0x417f3f44447c3800,
    0x387c4444446c2800,
    0x387c44453f7f4000,
    0x387c5454545c1800,
    0x487e7f4909030200,
    0x98bca4a4f87c0400,
    0x417f7f08047c7800,
    0x0000447d7d400000,
    0x0060e08080fd7d00,
    0x417f7f10386c4400,
    0x0000417f7f400000,
    0x7c7c0c780c7c7800,
    0x047c7804047c7800,
    0x387c4444447c3800,
    0x84fcf8a4243c1800,
    0x183c24a4f8fc8400,
    0x447c784c040c0800,
    0x485c545454742400,
    0x04043f7f44642000,
    0x3c7c40403c7c4000,
    0x1c3c6040603c1c00,
    0x3c7c6038607c3c00,
    0x446c3810386c4400,
    0x9cbca0a0a0fc7c00,
    0x004c64745c4c6400,
    0x0008083e77414100,
    0x0000007f7f000000,
    0x004141773e080800,
    0x0203010302030100,
    0x70784c464c787000,
];

/// Pixel of character `c` at column `x`, row `y` (row 0 = top).
fn pixel(c: u8, x: usize, y: usize) -> bool {
    let i = (c.clamp(32, 127) - 32) as usize;
    FONT[i] >> ((7 - x) * 8 + y) & 1 == 1
}

/// Render `text` (ASCII, at most 20 characters) as real 48 kHz audio for a carrier-0
/// frequency `dc_freq` (the text then spans about dc+77 .. dc+2540 Hz), RMS `rms`.
pub fn render(text: &str, dc_freq: Real, rms: Real) -> Vec<f32> {
    let chars: Vec<u8> = text.bytes().take(MAX_CHARS).collect();
    let r = chars.len().max(1);
    let n = 512 + r.saturating_sub(9) * 8;
    let out_len = (1.65 * n as f64) as usize;
    let inp = 64 * r;
    let start = 3 * inp / n + 18;
    let rep = (100 / r).clamp(5, 50) + 1;
    let n48 = 4 * n;
    let mut fft = Fft::new(n48);
    // Bin k (12 kHz grid) sits at 12000·k/N − 1640 Hz after EasyDRF's shift, then at
    // +1225 + DC in its up-converter: net shift DC − 415 Hz.
    let w = 2.0 * PI * (dc_freq - 415.0) / f64::from(SAMPLE_RATE);
    let mut phase: Real = 0.0;
    let mut out = Vec::with_capacity(8 * rep * n48);
    let mut block = vec![Cplx::new(0.0, 0.0); n48];
    for row in (0..8).rev() {
        let pix: Vec<f64> = (0..8 * r)
            .map(|i| {
                if chars.get(i / 8).is_some_and(|&c| pixel(c, i % 8, row)) {
                    1.0
                } else {
                    0.0
                }
            })
            .collect();
        block.iter_mut().for_each(|v| *v = Cplx::new(0.0, 0.0));
        for k in start..n - start {
            let m = k - start;
            if m >= out_len {
                break;
            }
            let p = pix.get(m * inp / out_len).copied().unwrap_or(0.0);
            if p > 0.0 {
                let ph = PI * 8.0 * (k * k) as f64 / n as f64;
                block[k] = Cplx::from_polar(p, ph);
            }
        }
        fft.inverse(&mut block);
        for _ in 0..rep {
            for v in &block {
                out.push((v * Cplx::from_polar(1.0, phase)).re);
                phase = (phase + w) % (2.0 * PI);
            }
        }
    }
    let p = (out.iter().map(|v| v * v).sum::<f64>() / out.len().max(1) as f64).sqrt();
    let g = if p > 0.0 { rms / p } else { 0.0 };
    out.iter().map(|v| (v * g) as f32).collect()
}

/// Duration of `render(text, …)` in seconds.
pub fn duration(text: &str) -> Real {
    let r = text.len().clamp(1, MAX_CHARS);
    let n = 512 + r.saturating_sub(9) * 8;
    let rep = (100 / r).clamp(5, 50) + 1;
    (8 * rep * n) as f64 / 12000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn length_and_band() {
        let a = render("TEST", 350.0, 0.2);
        assert_eq!(a.len(), (duration("TEST") * 48000.0).round() as usize);
        // Energy must sit inside the SSB passband (300..3000 Hz).
        let n = 4096;
        let mut fft = Fft::new(n);
        let bin = 48000.0 / n as f64;
        let (mut inb, mut tot) = (0.0, 0.0);
        for chunk in a.chunks_exact(n) {
            let mut buf: Vec<Cplx> = chunk
                .iter()
                .map(|&v| Cplx::new(f64::from(v), 0.0))
                .collect();
            fft.forward(&mut buf);
            for (i, v) in buf[..n / 2].iter().enumerate() {
                let e = v.norm_sqr();
                tot += e;
                if (300.0..3000.0).contains(&(i as f64 * bin)) {
                    inb += e;
                }
            }
        }
        assert!(inb / tot > 0.99, "{}", inb / tot);
    }

    #[test]
    fn glyph_a_has_pixels() {
        assert!((0..8).any(|x| (0..8).any(|y| pixel(b'A', x, y))));
        assert!(!(0..8).any(|x| (0..8).any(|y| pixel(b' ', x, y))));
    }
}
