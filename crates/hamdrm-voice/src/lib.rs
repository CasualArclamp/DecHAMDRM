//! DecHAMDRM digital voice: codecs that turn 400 ms of 48 kHz speech into the bits of
//! one MSC frame and back.
//!
//! - [`easydrf::EasyDrfLpc`]: EasyDRF's LPC-10 voice (interoperable with EasyDRF).
//! - [`nv`]: the frame format of DecHAMDRM's neural voice (the Mimi model itself is in
//!   `hamdrm-neural`, behind a large dependency).

pub mod easydrf;
pub mod lpc10;
pub mod nv;
pub mod resample;

/// Samples per 400 ms MSC frame at 48 kHz.
pub const SAMPLES_PER_FRAME: usize = 19_200;

/// A voice codec working a frame at a time.
pub trait VoiceCodec: Send {
    fn name(&self) -> &'static str;
    /// 400 ms of 48 kHz mono speech (`SAMPLES_PER_FRAME`, full scale ±1) → `bits` bits.
    fn encode(&mut self, pcm: &[f32], bits: usize) -> Vec<u8>;
    /// One frame of bits → 400 ms of 48 kHz speech, or `None` if the frame is bad.
    fn decode(&mut self, bits: &[u8]) -> Option<Vec<f32>>;
}
