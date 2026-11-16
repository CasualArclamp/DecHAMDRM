//! Safe wrapper around the vendored HawkVoice LPC-10 (EasyDRF variant).
//!
//! The C code reads the frame length from a global, `LPC10_SAMPLES_PER_FRAME`
//! (180..540 samples), so every call sets it under one process-wide lock.

use std::os::raw::{c_int, c_short, c_uchar, c_void};
use std::sync::Mutex;

/// Bits per LPC-10 frame.
pub const FRAME_BITS: usize = 54;
pub const MIN_SAMPLES: usize = 180;
pub const MAX_SAMPLES: usize = 540;

unsafe extern "C" {
    static mut LPC10_SAMPLES_PER_FRAME: c_int;
    fn create_lpc10_encoder_state() -> *mut c_void;
    fn init_lpc10_encoder_state(st: *mut c_void);
    fn destroy_lpc10_encoder_state(st: *mut c_void);
    fn lpc10_encode(input: *mut c_short, out: *mut c_uchar, st: *mut c_void) -> c_int;
    fn create_lpc10_decoder_state() -> *mut c_void;
    fn init_lpc10_decoder_state(st: *mut c_void);
    fn destroy_lpc10_decoder_state(st: *mut c_void);
    fn lpc10_decode(input: *mut c_uchar, out: *mut c_short, st: *mut c_void) -> c_int;
}

static LOCK: Mutex<()> = Mutex::new(());

fn set_frame(n: usize) {
    // SAFETY: only written while LOCK is held, and the C code only reads it during the
    // calls made under the same lock.
    unsafe { LPC10_SAMPLES_PER_FRAME = n.clamp(MIN_SAMPLES, MAX_SAMPLES) as c_int };
}

pub struct Encoder(*mut c_void);
pub struct Decoder(*mut c_void);

// SAFETY: the C states are plain heap structs used from one thread at a time.
unsafe impl Send for Encoder {}
unsafe impl Send for Decoder {}

impl Encoder {
    pub fn new() -> Self {
        // `create` only allocates; `init` fills the state (at the default 180-sample
        // frame, as EasyDRF does).
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set_frame(MIN_SAMPLES);
        // SAFETY: allocates and initialises a state; freed in Drop.
        unsafe {
            let st = create_lpc10_encoder_state();
            assert!(!st.is_null(), "LPC-10 encoder allocation");
            init_lpc10_encoder_state(st);
            Self(st)
        }
    }

    /// Encode `pcm` (one frame of `pcm.len()` samples, 180..=540) to 54 bits.
    pub fn encode(&mut self, pcm: &[i16]) -> [u8; FRAME_BITS] {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set_frame(pcm.len());
        let mut inp = [0i16; MAX_SAMPLES * 2];
        inp[..pcm.len()].copy_from_slice(pcm);
        let mut out = [0u8; FRAME_BITS + 2];
        // SAFETY: buffers are larger than the C code reads/writes.
        unsafe { lpc10_encode(inp.as_mut_ptr(), out.as_mut_ptr(), self.0) };
        let mut bits = [0u8; FRAME_BITS];
        bits.copy_from_slice(&out[..FRAME_BITS]);
        bits
    }
}

impl Default for Encoder {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        // SAFETY: created by create_lpc10_encoder_state.
        unsafe { destroy_lpc10_encoder_state(self.0) }
    }
}

impl Decoder {
    pub fn new() -> Self {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set_frame(MIN_SAMPLES);
        // SAFETY: as above.
        unsafe {
            let st = create_lpc10_decoder_state();
            assert!(!st.is_null(), "LPC-10 decoder allocation");
            init_lpc10_decoder_state(st);
            Self(st)
        }
    }

    /// Decode 54 bits to `n` samples (180..=540).
    pub fn decode(&mut self, bits: &[u8], n: usize) -> Vec<i16> {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set_frame(n);
        let mut inp = [0u8; FRAME_BITS + 2];
        inp[..FRAME_BITS].copy_from_slice(&bits[..FRAME_BITS]);
        let mut out = [0i16; MAX_SAMPLES * 2];
        // SAFETY: buffers are larger than the C code reads/writes.
        unsafe { lpc10_decode(inp.as_mut_ptr(), out.as_mut_ptr(), self.0) };
        out[..n.clamp(MIN_SAMPLES, MAX_SAMPLES)].to_vec()
    }
}

impl Default for Decoder {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        // SAFETY: created by create_lpc10_decoder_state.
        unsafe { destroy_lpc10_decoder_state(self.0) }
    }
}
