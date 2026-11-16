//! EasyDRF's digital voice: LPC-10 frames packed into each 400 ms MSC frame
//! (`docs/spec/app.md` §7.1; EasyDRF `AudioSourceDecoder.cpp:297-346` TX,
//! `:648-740` RX).
//!
//! With T MSC bits per frame: `Tb = ⌊T/8⌋·8`, `blocks = ⌊(Tb − 27)/54⌋` LPC frames at
//! bits 54·i, then a 27-bit check `C[j] = XOR over w < 2·blocks of bit[27w + j]`.
//! Each LPC frame covers `Ns = clamp(⌊180 / max(blocks, 6)⌋·18, 180, 540)` samples, so
//! 400 ms of speech is time-scaled to `blocks·Ns` samples (about 8.1 kHz).

use crate::lpc10::{self, FRAME_BITS};
use crate::resample::{Resampler, block};
use crate::{SAMPLES_PER_FRAME, VoiceCodec};

/// Check bits (`lpcsumT`).
pub const CHECK_BITS: usize = 27;

/// LPC frames and samples per LPC frame for `t` MSC bits.
pub fn layout(t: usize) -> (usize, usize) {
    let tb = t / 8 * 8;
    let blocks = tb.saturating_sub(CHECK_BITS) / FRAME_BITS;
    let ns = ((180 / blocks.max(6)) * 18).clamp(lpc10::MIN_SAMPLES, lpc10::MAX_SAMPLES);
    (blocks, ns)
}

fn check(bits: &[u8], blocks: usize) -> [u8; CHECK_BITS] {
    let mut c = [0u8; CHECK_BITS];
    for w in 0..(blocks * FRAME_BITS / CHECK_BITS) {
        for j in 0..CHECK_BITS {
            c[j] ^= bits[w * CHECK_BITS + j];
        }
    }
    c
}

/// EasyDRF LPC-10 voice, both directions.
pub struct EasyDrfLpc {
    enc: lpc10::Encoder,
    dec: lpc10::Decoder,
    layout: (usize, usize),
    down: Option<Resampler>,
    up: Option<Resampler>,
    carry_down: Vec<f32>,
    carry_up: Vec<f32>,
}

impl Default for EasyDrfLpc {
    fn default() -> Self {
        Self::new()
    }
}

impl EasyDrfLpc {
    pub fn new() -> Self {
        Self {
            enc: lpc10::Encoder::new(),
            dec: lpc10::Decoder::new(),
            layout: (0, 0),
            down: None,
            up: None,
            carry_down: Vec::new(),
            carry_up: Vec::new(),
        }
    }

    fn set_layout(&mut self, t: usize) -> (usize, usize) {
        let l = layout(t);
        if l != self.layout {
            self.layout = l;
            let rate = (l.0 * l.1) as f64 / 0.4;
            self.down = Some(Resampler::new(48_000.0, rate));
            self.up = Some(Resampler::new(rate, 48_000.0));
            self.carry_down.clear();
            self.carry_up.clear();
        }
        l
    }
}

impl VoiceCodec for EasyDrfLpc {
    fn name(&self) -> &'static str {
        "LPC-10 (EasyDRF)"
    }

    fn encode(&mut self, pcm: &[f32], t: usize) -> Vec<u8> {
        let (blocks, ns) = self.set_layout(t);
        let mut bits = vec![0u8; t];
        if blocks == 0 {
            return bits;
        }
        let down = self.down.as_mut().expect("layout");
        let x = block(down, pcm, blocks * ns, &mut self.carry_down);
        for b in 0..blocks {
            let frame: Vec<i16> = x[b * ns..(b + 1) * ns]
                .iter()
                .map(|v| (v * 32767.0).clamp(-32700.0, 32700.0) as i16)
                .collect();
            let f = self.enc.encode(&frame);
            bits[b * FRAME_BITS..(b + 1) * FRAME_BITS].copy_from_slice(&f);
        }
        let c = check(&bits, blocks);
        bits[blocks * FRAME_BITS..blocks * FRAME_BITS + CHECK_BITS].copy_from_slice(&c);
        bits
    }

    fn decode(&mut self, bits: &[u8]) -> Option<Vec<f32>> {
        let (blocks, ns) = self.set_layout(bits.len());
        if blocks == 0 || bits.len() < blocks * FRAME_BITS + CHECK_BITS {
            return None;
        }
        if check(bits, blocks)[..] != bits[blocks * FRAME_BITS..blocks * FRAME_BITS + CHECK_BITS] {
            return None;
        }
        let mut x = Vec::with_capacity(blocks * ns);
        for b in 0..blocks {
            let s = self
                .dec
                .decode(&bits[b * FRAME_BITS..(b + 1) * FRAME_BITS], ns);
            // EasyDRF scales the decoder output by 0.7 (`AudioSourceDecoder.cpp:699`).
            x.extend(s.iter().map(|&v| f32::from(v) / 32768.0 * 0.7));
        }
        let up = self.up.as_mut().expect("layout");
        Some(block(up, &x, SAMPLES_PER_FRAME, &mut self.carry_up))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts_match_easydrf() {
        // Mode B 2.5 kHz 16-QAM p0: 1047 bits → 18 blocks of 180 samples.
        assert_eq!(layout(1047), (18, 180));
        // Mode B 2.3 kHz 4-QAM: 537 bits → 9 blocks of 360.
        assert_eq!(layout(537), (9, 360));
    }

    #[test]
    fn voice_roundtrip_has_energy_and_check() {
        let mut c = EasyDrfLpc::new();
        let mut d = EasyDrfLpc::new();
        let mut energy = 0.0;
        for f in 0..6 {
            // A buzzy vowel-like signal: 120 Hz pulse train through a resonance.
            let pcm: Vec<f32> = (0..SAMPLES_PER_FRAME)
                .map(|i| {
                    let t = (f * SAMPLES_PER_FRAME + i) as f32 / 48000.0;
                    0.3 * (2.0 * std::f32::consts::PI * 700.0 * t).sin()
                        * ((t * 120.0).fract() < 0.1) as i32 as f32
                })
                .collect();
            let bits = c.encode(&pcm, 1047);
            assert_eq!(bits.len(), 1047);
            let out = d.decode(&bits).expect("check passes");
            assert_eq!(out.len(), SAMPLES_PER_FRAME);
            if f > 1 {
                energy += out.iter().map(|v| v * v).sum::<f32>();
            }
            let mut bad = bits.clone();
            bad[5] ^= 1;
            assert!(EasyDrfLpc::new().decode(&bad).is_none());
        }
        assert!(energy > 1.0, "{energy}");
    }
}
