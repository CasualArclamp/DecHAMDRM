//! Waterfall rows of the audio (0–3.5 kHz) for the UI.

use rustfft::num_complex::Complex32;
use rustfft::{Fft, FftPlanner};
use std::sync::Arc;

const N: usize = 2048;
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
        }
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
                rows.push(
                    buf[..keep]
                        .iter()
                        .map(|c| 10.0 * (c.norm_sqr() / (N * N) as f32 + 1e-14).log10())
                        .collect(),
                );
            }
        }
    }
}
