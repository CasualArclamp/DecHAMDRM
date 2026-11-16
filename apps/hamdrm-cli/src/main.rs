//! `dechamdrm` — DecHAMDRM command line.
//!
//! `dechamdrm tx FILE… --out signal.wav [--mode B --bw wide --qam 16 --prot 0 --rs 0]`
//! writes a HamDRM transmission (48 kHz mono 16-bit WAV, carrier 0 at 350 Hz) that
//! EasyPal, EasyDRF and QSSTV can receive.

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use hamdrm_core::params::{Bandwidth, Interleave, Layout, Mode, MscConfig, Qam, SAMPLE_RATE};
use hamdrm_core::rx::{Receiver, ReceiverConfig, ReceiverEvent};
use hamdrm_core::tx::{Transmitter, TxConfig};
use hamdrm_data::receive::{FileEvent, FileReceiver};
use hamdrm_data::send::{OutFile, SendConfig, frames};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "dechamdrm",
    version,
    about = "HamDRM (EasyPal / EasyDRF / QSSTV) modem"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Encode files into a HamDRM transmission WAV.
    Tx(TxArgs),
    /// Decode a recording (48 kHz WAV) and save the received files.
    Rx(RxArgs),
}

#[derive(clap::Args)]
struct RxArgs {
    /// Recording (WAV, 48 kHz).
    input: PathBuf,
    /// Where to save received files.
    #[arg(long, default_value = "rx")]
    out_dir: PathBuf,
    /// Print every FAC and MSC frame.
    #[arg(long, short)]
    verbose: bool,
}

#[derive(clap::Args)]
struct TxArgs {
    /// Files to send.
    #[arg(required = true)]
    files: Vec<PathBuf>,
    /// Output WAV.
    #[arg(long, short)]
    out: PathBuf,
    /// Robustness mode: A, B or E.
    #[arg(long, default_value = "B")]
    mode: String,
    /// Bandwidth: narrow (2.3 kHz) or wide (2.5 kHz).
    #[arg(long, default_value = "wide")]
    bw: String,
    /// MSC constellation: 4, 16 or 64.
    #[arg(long, default_value_t = 16)]
    qam: u32,
    /// Protection level 0 (normal) or 1 (low).
    #[arg(long, default_value_t = 0)]
    prot: u8,
    /// Long (2 s) cell interleaver.
    #[arg(long)]
    long: bool,
    /// EasyPal/QSSTV Reed–Solomon level 1..4 (0 = plain).
    #[arg(long, default_value_t = 0)]
    rs: u8,
    /// Callsign / label (max 9 characters).
    #[arg(long, default_value = "NOCALL")]
    call: String,
    /// Lead-in body segments before the first file.
    #[arg(long, default_value_t = 9)]
    lead_in: usize,
    /// Audio frequency of carrier 0, Hz.
    #[arg(long, default_value_t = 350.0)]
    dc: f64,
    /// Silence before and after, seconds.
    #[arg(long, default_value_t = 0.5)]
    pad: f64,
}

fn parse_config(a: &TxArgs) -> Result<MscConfig> {
    let mode = match a.mode.to_ascii_uppercase().as_str() {
        "A" => Mode::A,
        "B" => Mode::B,
        "E" => Mode::E,
        m => bail!("unknown mode {m} (A, B or E)"),
    };
    let bandwidth = match a.bw.to_ascii_lowercase().as_str() {
        "narrow" | "2.3" | "0" => Bandwidth::Narrow,
        "wide" | "2.5" | "1" => Bandwidth::Wide,
        b => bail!("unknown bandwidth {b} (narrow or wide)"),
    };
    let qam = match a.qam {
        4 => Qam::Qam4,
        16 => Qam::Qam16,
        64 => Qam::Qam64,
        q => bail!("unknown QAM {q} (4, 16 or 64)"),
    };
    if a.prot > 1 {
        bail!("protection level is 0 or 1");
    }
    Ok(MscConfig {
        layout: Layout::new(mode, bandwidth),
        qam,
        protection: a.prot,
        interleave: if a.long {
            Interleave::Long
        } else {
            Interleave::Short
        },
    })
}

fn tx(a: TxArgs) -> Result<()> {
    let msc = parse_config(&a)?;
    let mut cfg = TxConfig::new(msc, &a.call);
    cfg.dc_freq = a.dc;
    let mut tx = Transmitter::new(cfg);
    let mut files = Vec::new();
    for p in &a.files {
        let data = std::fs::read(p).with_context(|| format!("reading {}", p.display()))?;
        let name = p
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file")
            .to_string();
        files.push(OutFile { name, data });
    }
    let send = SendConfig {
        packet_len: tx.packet_len(),
        packet_id: 0,
        rs_level: a.rs,
        lead_in: a.lead_in,
    };
    let frames = frames(&files, &send);
    // The long interleaver delays cells by up to 4 frames: flush with empty frames.
    let flush = msc.interleave.depth() - 1;
    let mut audio = Vec::new();
    let pad = (a.pad * f64::from(SAMPLE_RATE)) as usize;
    audio.resize(pad, 0.0f32);
    for f in &frames {
        tx.push_frame(f, &mut audio);
    }
    for _ in 0..flush {
        tx.push_frame(&[], &mut audio);
    }
    audio.resize(audio.len() + pad, 0.0);

    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(&a.out, spec)?;
    for s in &audio {
        w.write_sample((s.clamp(-1.0, 1.0) * 32767.0) as i16)?;
    }
    w.finalize()?;
    eprintln!(
        "{msc}: {} frames ({:.1} s), {} bytes/frame, packet {} → {}",
        frames.len(),
        audio.len() as f64 / f64::from(SAMPLE_RATE),
        tx.frame_bytes(),
        tx.packet_len(),
        a.out.display()
    );
    Ok(())
}

fn rx(a: RxArgs) -> Result<()> {
    let mut r = hound::WavReader::open(&a.input)
        .with_context(|| format!("opening {}", a.input.display()))?;
    let spec = r.spec();
    if spec.sample_rate != SAMPLE_RATE {
        bail!("{} Hz: only 48 kHz recordings for now", spec.sample_rate);
    }
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => r.samples::<f32>().collect::<Result<_, _>>()?,
        hound::SampleFormat::Int => {
            let scale = 1.0 / (1u64 << (spec.bits_per_sample - 1)) as f32;
            r.samples::<i32>()
                .map(|s| s.map(|v| v as f32 * scale))
                .collect::<Result<_, _>>()?
        }
    };
    let cfg = ReceiverConfig {
        channels: usize::from(spec.channels),
        ..Default::default()
    };
    let mut rx = Receiver::new(cfg);
    let mut files = FileReceiver::new();
    std::fs::create_dir_all(&a.out_dir)?;
    let block = 4800 * usize::from(spec.channels);
    let mut saved = 0;
    for chunk in samples.chunks(block) {
        for ev in rx.push(chunk) {
            match ev {
                ReceiverEvent::SignalFound { dc_hz } => eprintln!("signal: DC at {dc_hz:.1} Hz"),
                ReceiverEvent::ModeDetected(m) => eprintln!("mode {m}"),
                ReceiverEvent::Configured(c) => eprintln!("MSC: {c}"),
                ReceiverEvent::Fac { fac, label } => {
                    if a.verbose {
                        eprintln!(
                            "FAC frame {} {:?} label {:?}",
                            fac.frame, fac.service, label
                        );
                    }
                }
                ReceiverEvent::Restarted => eprintln!("sync lost, restarting"),
                ReceiverEvent::Msc(f) => {
                    if a.verbose {
                        eprintln!("MSC {} bytes, FAC ok {}", f.bytes.len(), f.fac_ok);
                    }
                    for fe in files.push(Some(&f.bytes)) {
                        match fe {
                            FileEvent::Progress {
                                name,
                                received,
                                total,
                                ..
                            } => {
                                if a.verbose {
                                    eprintln!(
                                        "  {}: {received}/{}",
                                        name.as_deref().unwrap_or("?"),
                                        total.map_or("?".into(), |t| t.to_string())
                                    );
                                }
                            }
                            FileEvent::File { name, data, rs, .. } => {
                                let path = a.out_dir.join(&name);
                                std::fs::write(&path, &data)?;
                                saved += 1;
                                eprintln!(
                                    "received {} ({} bytes{})",
                                    path.display(),
                                    data.len(),
                                    rs.map(|r| format!(", {r}")).unwrap_or_default()
                                );
                            }
                            FileEvent::Bsr(req) => eprintln!(
                                "BSR for transport id {}: {} segments missing",
                                req.tid,
                                req.missing.len()
                            ),
                        }
                    }
                }
                _ => {}
            }
        }
    }
    let st = rx.status();
    eprintln!(
        "done: {saved} file(s); FAC {}/{} ok, MSC {} ok, label {:?}, SNR {:?} dB, MER {:?} dB, WMER {:?} dB",
        st.fac_ok,
        st.fac_ok + st.fac_bad,
        st.msc_ok,
        st.label,
        st.snr_db.map(|v| (v * 10.0).round() / 10.0),
        st.mer_db.map(|v| (v * 10.0).round() / 10.0),
        st.wmer_db.map(|v| (v * 10.0).round() / 10.0)
    );
    Ok(())
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Tx(a) => tx(a),
        Cmd::Rx(a) => rx(a),
    }
}
