//! Real-time factor of Mimi streaming encode + decode on the CPU.
use candle::{Device, Tensor};
use hamdrm_neural::mimi;
use std::time::Instant;

fn main() -> anyhow::Result<()> {
    let path = std::env::args()
        .nth(1)
        .unwrap_or("models/mimi-e351c8d8.safetensors".into());
    let nq: usize = std::env::args()
        .nth(2)
        .and_then(|v| v.parse().ok())
        .unwrap_or(8);
    let dev = Device::Cpu;
    let mut m = mimi::load(&path, Some(nq), &dev)?;
    // 10 s of a chirp at 24 kHz in 80 ms (1920-sample) steps.
    let secs = 10.0;
    let n = (24000.0 * secs) as usize;
    let pcm: Vec<f32> = (0..n)
        .map(|i| {
            let t = i as f32 / 24000.0;
            0.3 * (2.0 * std::f32::consts::PI * (200.0 + 100.0 * t) * t).sin()
        })
        .collect();
    let t0 = Instant::now();
    let mut out = Vec::new();
    let mut codes_n = 0;
    for chunk in pcm.chunks(1920) {
        let x = Tensor::from_slice(chunk, (1, 1, chunk.len()), &dev)?;
        let codes = m.encode_step(&x.into())?;
        if let Some(c) = codes.as_option() {
            codes_n += c.dim(2)?;
            let y = m.decode_step(&c.clone().into())?;
            if let Some(y) = y.as_option() {
                out.extend(y.flatten_all()?.to_vec1::<f32>()?);
            }
        }
    }
    let el = t0.elapsed().as_secs_f64();
    println!(
        "nq {nq}: {secs} s audio in {el:.2} s (RTF {:.3}), {codes_n} code frames, {} samples out",
        el / secs,
        out.len()
    );
    Ok(())
}
