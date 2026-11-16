//! Conversion of raw sound-card / file samples into the complex 48 kHz stream the
//! receiver works on. Port of DecDRM's `rx/input.rs` (Dream's `CReceiveData`), cut
//! down to real audio: HamDRM is always real upper-sideband audio (`docs/spec/phy.md`
//! §1.3).
//!
//! The real signal is turned into its analytic signal (x + j·H{x}) so that
//! everything downstream can work in complex baseband. HamDRM's lowest carrier sits
//! at only ~400 Hz, so the Hilbert transformer must be much longer than DecDRM's
//! (127 taps, enough for a 12 kHz IF): 511 taps at 48 kHz reach full gain above
//! ~200 Hz, which keeps the negative-frequency image of the carriers > 60 dB down.

use crate::{Cplx, Real};
use decdrm_core::dsp::fir::AnalyticConverter;

/// Hilbert transformer length (odd). See the module docs.
const HILBERT_TAPS: usize = 511;

/// Which channel(s) of the interleaved input carry the signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RealChannel {
    #[default]
    Left,
    Right,
    /// (L + R) / 2 — also right for mono sources.
    Mix,
    /// (L − R) / 2.
    Diff,
}

/// Streaming converter from interleaved `f32` frames to complex samples.
#[derive(Debug, Clone)]
pub struct InputConverter {
    channel: RealChannel,
    channels: usize,
    flip: bool,
    analytic: AnalyticConverter,
    real_buf: Vec<Real>,
    sign: Real,
}

impl InputConverter {
    /// `channels` is the number of interleaved channels of the source (1 = mono).
    /// `flip` mirrors the spectrum (an LSB receiver on a USB signal).
    pub fn new(channel: RealChannel, channels: usize, flip: bool) -> Self {
        Self {
            channel,
            channels: channels.max(1),
            flip,
            analytic: AnalyticConverter::new(HILBERT_TAPS),
            real_buf: Vec::new(),
            sign: 1.0,
        }
    }

    /// Convert interleaved frames into complex samples appended to `out`.
    pub fn process(&mut self, interleaved: &[f32], out: &mut Vec<Cplx>) {
        let ch = self.channels;
        let frames = interleaved.len() / ch;
        // A closure capturing `interleaved`; a missing channel (mono input with
        // `Right` selected) falls back to channel 0.
        let sample = |f: usize, c: usize| -> Real {
            if c < ch {
                Real::from(interleaved[f * ch + c])
            } else {
                Real::from(interleaved[f * ch])
            }
        };
        self.real_buf.clear();
        for f in 0..frames {
            let v = match self.channel {
                RealChannel::Left => sample(f, 0),
                RealChannel::Right => sample(f, 1),
                RealChannel::Mix if ch == 1 => sample(f, 0),
                RealChannel::Mix => 0.5 * (sample(f, 0) + sample(f, 1)),
                RealChannel::Diff => 0.5 * (sample(f, 0) - sample(f, 1)),
            };
            // Mirroring a real spectrum (f → fs/2 − f) = negating every other sample.
            // For audio this moves the signal to 21–23.6 kHz; only useful for
            // recordings made at an IF, kept for parity with DecDRM.
            let v = if self.flip {
                self.sign = -self.sign;
                v * self.sign
            } else {
                v
            };
            self.real_buf.push(v);
        }
        self.analytic.process(&self.real_buf, out);
    }
}
