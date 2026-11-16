//! Speech WAV (48 kHz mono) through a voice codec at a given MSC frame size:
//! `voice_file IN.wav OUT.wav lpc|mimi BITS [BER]`. Prints the time per frame.
use hamdrm_voice::{SAMPLES_PER_FRAME, VoiceCodec};
use std::time::Instant;

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let bits: usize = a[3].parse()?;
    let ber: f64 = a.get(4).and_then(|v| v.parse().ok()).unwrap_or(0.0);
    let mut r = hound::WavReader::open(&a[0])?;
    let pcm: Vec<f32> = r
        .samples::<i16>()
        .map(|s| f32::from(s.unwrap()) / 32768.0)
        .collect();
    let make = |w: &str| -> anyhow::Result<Box<dyn VoiceCodec>> {
        Ok(match w {
            "lpc" => Box::new(hamdrm_voice::easydrf::EasyDrfLpc::new()),
            _ => Box::new(
                hamdrm_neural::MimiVoice::load(&hamdrm_neural::find_weights().expect("weights"))
                    .map_err(anyhow::Error::msg)?,
            ),
        })
    };
    let mut enc = make(&a[2])?;
    let mut dec = make(&a[2])?;
    let mut out = Vec::new();
    let (mut te, mut td, mut bad) = (0.0, 0.0, 0);
    let mut seed = 12345u64;
    for chunk in pcm.chunks(SAMPLES_PER_FRAME) {
        let mut c = chunk.to_vec();
        c.resize(SAMPLES_PER_FRAME, 0.0);
        let t0 = Instant::now();
        let mut b = enc.encode(&c, bits);
        te += t0.elapsed().as_secs_f64();
        for v in b.iter_mut() {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            if ((seed >> 11) as f64 / (1u64 << 53) as f64) < ber {
                *v ^= 1;
            }
        }
        let t0 = Instant::now();
        match dec.decode(&b) {
            Some(o) => out.extend(o),
            None => {
                bad += 1;
                out.extend(std::iter::repeat_n(0.0, SAMPLES_PER_FRAME));
            }
        }
        td += t0.elapsed().as_secs_f64();
    }
    let n = pcm.len().div_ceil(SAMPLES_PER_FRAME);
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 48000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(&a[1], spec)?;
    for s in &out {
        w.write_sample((s.clamp(-1.0, 1.0) * 32767.0) as i16)?;
    }
    w.finalize()?;
    println!(
        "{} frames: encode {:.1} ms/frame, decode {:.1} ms/frame, {bad} bad",
        n,
        te * 1e3 / n as f64,
        td * 1e3 / n as f64
    );
    Ok(())
}
