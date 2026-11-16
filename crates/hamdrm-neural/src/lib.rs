//! DecHAMDRM's neural voice: Kyutai's Mimi codec (streaming, 24 kHz, 12.5 frames/s,
//! up to 32 residual codebooks of 11 bits) running on the CPU with candle, framed per
//! 400 ms MSC frame as [`hamdrm_voice::nv`] describes. The bit rate follows the MSC:
//! 550 bit/s per codebook, 8 codebooks in mode B 2.3 kHz 4-QAM, 16 in B 2.5 kHz 16-QAM.
//!
//! Weights: `kyutai/mimi` (CC-BY-4.0), 385 MB, fetched on request ([`download`]).

// Index loops read like the algorithm (SHA-256 rounds, code matrices).
#![allow(clippy::needless_range_loop)]

// Vendored upstream code: keep it as it is rather than lint it.
#[allow(clippy::all, dead_code, unused)]
pub mod mimi;

use candle::{Device, IndexOp, Tensor};
use hamdrm_voice::nv::{self, FRAMES, MAX_CODEBOOKS};
use hamdrm_voice::resample::Resampler;
use hamdrm_voice::{SAMPLES_PER_FRAME, VoiceCodec};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};

/// Pinned weights: `kyutai/mimi` at commit 89091b3e, `model.safetensors`.
pub const WEIGHTS_URL: &str = "https://huggingface.co/kyutai/mimi/resolve/89091b3e466eb6a9d11e537bf26b144f194978f7/model.safetensors";
pub const WEIGHTS_SHA256: &str = "bac7e85083dcded655d24eaadde7e6eea34c0da1b35fa2d284e641bd2b942a5e";
pub const WEIGHTS_FILE: &str = "mimi.safetensors";
/// Mimi's sample rate and frame length.
const RATE: f64 = 24_000.0;
const FRAME: usize = 1920;

/// Where the weights may be: `$DECHAMDRM_MODELS`, `models\` next to the executable,
/// `%APPDATA%\DecHAMDRM\models`, `./models`.
pub fn weight_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(d) = std::env::var_os("DECHAMDRM_MODELS") {
        v.push(PathBuf::from(d));
    }
    if let Some(d) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|p| p.join("models")))
    {
        v.push(d);
    }
    if let Some(d) = std::env::var_os("APPDATA") {
        v.push(PathBuf::from(d).join("DecHAMDRM").join("models"));
    }
    v.push(PathBuf::from("models"));
    v
}

pub fn find_weights() -> Option<PathBuf> {
    weight_dirs()
        .into_iter()
        .map(|d| d.join(WEIGHTS_FILE))
        .find(|p| p.is_file())
}

/// Fetch the weights into `dir` with curl (or wget), then check the SHA-256.
pub fn download(dir: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let path = dir.join(WEIGHTS_FILE);
    let tmp = dir.join(format!("{WEIGHTS_FILE}.part"));
    let ok = std::process::Command::new("curl")
        .args(["-L", "--fail", "-o"])
        .arg(&tmp)
        .arg(WEIGHTS_URL)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
        || std::process::Command::new("wget")
            .arg("-O")
            .arg(&tmp)
            .arg(WEIGHTS_URL)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
    if !ok {
        return Err(format!(
            "could not download {WEIGHTS_URL}; save it by hand as {}",
            path.display()
        ));
    }
    let data = std::fs::read(&tmp).map_err(|e| e.to_string())?;
    let got = sha256_hex(&data);
    if got != WEIGHTS_SHA256 {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("SHA-256 mismatch: got {got}"));
    }
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    Ok(path)
}

/// SHA-256 (FIPS 180-4), small and dependency-free.
pub fn sha256_hex(data: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut msg = data.to_vec();
    let bitlen = (data.len() as u64) * 8;
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bitlen.to_be_bytes());
    for chunk in msg.chunks(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                chunk[4 * i],
                chunk[4 * i + 1],
                chunk[4 * i + 2],
                chunk[4 * i + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let mut v = h;
        for i in 0..64 {
            let s1 = v[4].rotate_right(6) ^ v[4].rotate_right(11) ^ v[4].rotate_right(25);
            let ch = (v[4] & v[5]) ^ (!v[4] & v[6]);
            let t1 = v[7]
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = v[0].rotate_right(2) ^ v[0].rotate_right(13) ^ v[0].rotate_right(22);
            let maj = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
            let t2 = s0.wrapping_add(maj);
            v = [
                t1.wrapping_add(t2),
                v[0],
                v[1],
                v[2],
                v[3].wrapping_add(t1),
                v[4],
                v[5],
                v[6],
            ];
        }
        for i in 0..8 {
            h[i] = h[i].wrapping_add(v[i]);
        }
    }
    h.iter().map(|x| format!("{x:08x}")).collect()
}

/// Mimi as a [`VoiceCodec`]. One instance per direction (the model keeps streaming
/// state).
pub struct MimiVoice {
    model: mimi::Model,
    dev: Device,
    down: Resampler,
    up: Resampler,
    pcm24: Vec<f32>,
    codes: VecDeque<Vec<u16>>,
    out48: VecDeque<f32>,
    last_good: usize,
}

impl MimiVoice {
    pub fn load(weights: &Path) -> Result<Self, String> {
        let dev = Device::Cpu;
        let model = mimi::load(&weights.to_string_lossy(), Some(MAX_CODEBOOKS), &dev)
            .map_err(|e| e.to_string())?;
        Ok(Self {
            model,
            dev,
            down: Resampler::new(48_000.0, RATE),
            up: Resampler::new(RATE, 48_000.0),
            pcm24: Vec::new(),
            codes: VecDeque::new(),
            out48: VecDeque::new(),
            last_good: 0,
        })
    }

    /// Codebooks decoded in the last good frame.
    pub fn last_codebooks(&self) -> usize {
        self.last_good
    }

    fn encode_frames(&mut self) -> Result<(), candle::Error> {
        while self.pcm24.len() >= FRAME {
            let chunk: Vec<f32> = self.pcm24.drain(..FRAME).collect();
            let x = Tensor::from_slice(&chunk, (1, 1, FRAME), &self.dev)?;
            let c = self.model.encode_step(&x.into())?;
            if let Some(c) = c.as_option() {
                // [1, K, T] → per frame T: K codes.
                let c = c.i(0)?.to_dtype(candle::DType::U32)?;
                let (k, t) = c.dims2()?;
                let v = c.to_vec2::<u32>()?;
                for ti in 0..t {
                    self.codes
                        .push_back((0..k).map(|ki| v[ki][ti] as u16).collect());
                }
            }
        }
        Ok(())
    }

    fn decode_frames(&mut self, codes: &[Vec<u16>]) -> Result<(), candle::Error> {
        let k = codes[0].len();
        let flat: Vec<u32> = (0..k)
            .flat_map(|ki| codes.iter().map(move |f| u32::from(f[ki])))
            .collect();
        let t = Tensor::from_vec(flat, (1, k, codes.len()), &self.dev)?;
        let y = self.model.decode_step(&t.into())?;
        if let Some(y) = y.as_option() {
            let pcm: Vec<f32> = y.flatten_all()?.to_vec1()?;
            let mut o = Vec::new();
            self.up.process(&pcm, &mut o);
            self.out48.extend(o);
        }
        Ok(())
    }
}

impl VoiceCodec for MimiVoice {
    fn name(&self) -> &'static str {
        "Neural (Mimi)"
    }

    fn encode(&mut self, pcm: &[f32], t: usize) -> Vec<u8> {
        let nq = nv::codebooks_for(t);
        if nq == 0 {
            return vec![0; t];
        }
        let mut x = Vec::new();
        self.down.process(pcm, &mut x);
        self.pcm24.extend(x);
        let _ = self.encode_frames();
        let mut frames = Vec::with_capacity(FRAMES);
        for _ in 0..FRAMES {
            // Not enough yet (start-up latency): repeat the newest frame or zeros.
            let f = self
                .codes
                .pop_front()
                .unwrap_or_else(|| vec![0; MAX_CODEBOOKS]);
            frames.push(f[..nq.min(f.len())].to_vec());
        }
        nv::pack(&frames, t)
    }

    fn decode(&mut self, bits: &[u8]) -> Option<Vec<f32>> {
        let codes = nv::unpack(bits)?;
        self.last_good = codes[0].len();
        self.decode_frames(&codes).ok()?;
        // Fixed 400 ms out; the first frames carry the model's start-up delay.
        let n = SAMPLES_PER_FRAME;
        if self.out48.len() < n {
            let pad = n - self.out48.len();
            for _ in 0..pad {
                self.out48.push_front(0.0);
            }
        }
        Some(self.out48.drain(..n).collect())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn sha256_known_answer() {
        assert_eq!(
            super::sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
