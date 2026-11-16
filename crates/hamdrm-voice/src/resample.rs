//! Streaming windowed-sinc resampler for any rate ratio (voice paths: 48 kHz ↔ the
//! codec's rate). Kaiser-windowed sinc, cutoff 0.45 × the lower rate.

use std::f64::consts::PI;

/// Half-length of the kernel in samples of the lower rate.
const HALF: f64 = 12.0;
const BETA: f64 = 8.0;

pub struct Resampler {
    ratio: f64, // input samples per output sample
    cutoff: f64,
    half_in: f64,
    hist: Vec<f32>,
    /// Position of the next output sample, in input samples relative to `hist[0]`.
    pos: f64,
}

fn bessel_i0(x: f64) -> f64 {
    let (mut sum, mut term, mut k) = (1.0, 1.0, 1.0);
    while term > 1e-12 * sum {
        term *= (x / (2.0 * k)).powi(2);
        sum += term;
        k += 1.0;
    }
    sum
}

impl Resampler {
    pub fn new(in_rate: f64, out_rate: f64) -> Self {
        let ratio = in_rate / out_rate;
        // Cutoff as a fraction of the input rate.
        let cutoff = 0.45 * in_rate.min(out_rate) / in_rate;
        let half_in = HALF * ratio.max(1.0);
        Self {
            ratio,
            cutoff,
            half_in,
            hist: vec![0.0; half_in.ceil() as usize + 1],
            pos: half_in.ceil() + 1.0,
        }
    }

    fn kernel(&self, d: f64) -> f64 {
        if d.abs() >= self.half_in {
            return 0.0;
        }
        let x = 2.0 * self.cutoff * d;
        let sinc = if x.abs() < 1e-9 {
            1.0
        } else {
            (PI * x).sin() / (PI * x)
        };
        let w = bessel_i0(BETA * (1.0 - (d / self.half_in).powi(2)).sqrt()) / bessel_i0(BETA);
        2.0 * self.cutoff * sinc * w
    }

    /// Feed input, append the output samples that are ready.
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        self.hist.extend_from_slice(input);
        let h = self.half_in;
        while self.pos + h < self.hist.len() as f64 {
            let c = self.pos.floor() as i64;
            let lo = (self.pos - h).ceil() as i64;
            let hi = (self.pos + h).floor() as i64;
            let mut acc = 0.0;
            for i in lo.max(0)..=hi.min(self.hist.len() as i64 - 1) {
                acc += f64::from(self.hist[i as usize]) * self.kernel(self.pos - i as f64);
            }
            let _ = c;
            out.push(acc as f32);
            self.pos += self.ratio;
        }
        // Drop history no longer needed.
        let keep_from = (self.pos - h).floor().max(0.0) as usize;
        if keep_from > 0 {
            self.hist.drain(..keep_from.min(self.hist.len()));
            self.pos -= keep_from as f64;
        }
    }
}

/// Resample a whole block to exactly `n_out` samples (keeps state across calls; the
/// output is padded or trimmed by at most a sample or two of rounding).
pub fn block(r: &mut Resampler, input: &[f32], n_out: usize, carry: &mut Vec<f32>) -> Vec<f32> {
    r.process(input, carry);
    let take = n_out.min(carry.len());
    let mut out: Vec<f32> = carry.drain(..take).collect();
    out.resize(n_out, 0.0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tone_survives_down_and_up() {
        let tone: Vec<f32> = (0..48000)
            .map(|i| (2.0 * PI * 1000.0 * i as f64 / 48000.0).sin() as f32 * 0.5)
            .collect();
        let mut down = Resampler::new(48000.0, 8100.0);
        let mut up = Resampler::new(8100.0, 48000.0);
        let mut a = Vec::new();
        down.process(&tone, &mut a);
        let mut b = Vec::new();
        up.process(&a, &mut b);
        assert!((a.len() as f64 - 8100.0).abs() < 40.0, "{}", a.len());
        // Steady-state amplitude of the round trip.
        let mid = &b[b.len() / 4..b.len() / 2];
        let rms =
            (mid.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / mid.len() as f64).sqrt();
        assert!((rms - 0.5 / 2f64.sqrt()).abs() < 0.02, "{rms}");
    }
}
