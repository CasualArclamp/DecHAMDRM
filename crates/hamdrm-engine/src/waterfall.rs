//! Waterfall rows and the averaged spectrum of the audio (0–3.5 kHz) for the UI.
//! 16384-point FFT: 2.9 Hz bins, so single OFDM carriers (37.5–46.9 Hz apart) and
//! pilots are resolved; rows overlap (a new one every 50 ms).

use rustfft::num_complex::Complex32;
use rustfft::{Fft, FftPlanner};
use std::sync::Arc;

const N: usize = 16384;
const HOP: usize = 2400; // 50 ms: 20 rows per second
/// Bin spacing, Hz.
pub const BIN_HZ: f64 = 48_000.0 / N as f64;
/// Highest frequency shown, Hz.
pub const MAX_HZ: f64 = 3500.0;

pub(crate) struct Waterfall {
    fft: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    hist: Vec<f32>,
    since: usize,
    /// Exponentially averaged power per bin (the spectrum trace).
    avg: Vec<f32>,
}

impl Waterfall {
    pub fn new() -> Self {
        let window = (0..N)
            .map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / N as f32).cos())
            .collect();
        Self {
            fft: FftPlanner::new().plan_fft_forward(N),
            window,
            hist: Vec::with_capacity(2 * N),
            since: 0,
            avg: Vec::new(),
        }
    }

    /// Averaged spectrum in dB, bin `i` at `i · BIN_HZ` (empty before the first row).
    pub fn spectrum_db(&self) -> Vec<f64> {
        self.avg
            .iter()
            .map(|p| 10.0 * f64::from(p + 1e-14).log10())
            .collect()
    }

    pub fn push(&mut self, x: &[f32], rows: &mut Vec<Vec<f32>>) {
        for &v in x {
            self.hist.push(v);
            self.since += 1;
            if self.hist.len() > N {
                self.hist.drain(..self.hist.len() - N);
            }
            if self.since >= HOP && self.hist.len() == N {
                self.since = 0;
                let mut buf: Vec<Complex32> = self
                    .hist
                    .iter()
                    .zip(&self.window)
                    .map(|(a, w)| Complex32::new(a * w, 0.0))
                    .collect();
                self.fft.process(&mut buf);
                let keep = (MAX_HZ / BIN_HZ) as usize + 1;
                let scale = 1.0 / (N as f32 * N as f32);
                let pow: Vec<f32> = buf[..keep].iter().map(|c| c.norm_sqr() * scale).collect();
                if self.avg.len() != keep {
                    self.avg = pow.clone();
                } else {
                    for (a, p) in self.avg.iter_mut().zip(&pow) {
                        *a = 0.85 * *a + 0.15 * p;
                    }
                }
                rows.push(pow.iter().map(|p| 10.0 * (p + 1e-14).log10()).collect());
            }
        }
    }
}
