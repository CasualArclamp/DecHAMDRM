//! Loopback tests: our [`Transmitter`] → (optional channel simulator) → [`Receiver`],
//! checking that the MSC bytes come out identical.
//!
//! The channel works on the analytic signal of the real TX audio (exact, by FFT):
//! multipath/fading, frequency offset and sample-rate offset from DecDRM's
//! `decdrm_core::channel`, then the real part is taken and real white Gaussian noise
//! added. SNR = mean TX audio power / noise power in [`SNR_REF_BW_HZ`] (2.5 kHz).
//!
//! Default run: one short case per layout and constellation plus the impairments.
//! Long sweeps (SNR thresholds) are `#[ignore]`d; run them with
//! `cargo test --release -p hamdrm-core --test loopback -- --ignored --nocapture`.

use decdrm_core::channel::{ChannelConfig, ChannelModel, ChannelSimulator, Path, Rng};
use decdrm_core::dsp::fft::Fft;
use hamdrm_core::params::{Bandwidth, Interleave, Layout, Mode, MscConfig, Qam, SAMPLE_RATE};
use hamdrm_core::rx::{Receiver, ReceiverConfig, ReceiverEvent, SNR_REF_BW_HZ};
use hamdrm_core::tx::{Transmitter, TxConfig};
use hamdrm_core::{Cplx, Real};
use std::collections::HashMap;

/// Channel impairments.
#[derive(Debug, Clone, Default)]
struct Impair {
    snr_db: Option<Real>,
    freq_offset_hz: Real,
    sro_ppm: Real,
    model: Option<ChannelModel>,
    seed: u64,
}

/// Result of one run.
#[derive(Debug, Default)]
struct Outcome {
    /// Frames sent per segment, and how many of each segment came out identical.
    sent: Vec<usize>,
    matched: Vec<usize>,
    /// Indices of the matched frames per segment.
    matched_idx: Vec<Vec<usize>>,
    /// Cell error rates of the matched (good) and of the trusted-but-wrong frames.
    cer_good: Vec<Real>,
    cer_bad: Vec<Real>,
    /// Decoded frames flagged trusted whose bytes were not sent (false trust).
    trusted_wrong: usize,
    /// Decoded frames matching nothing (garbage, trusted or not).
    unmatched: usize,
    modes: Vec<Mode>,
    snr_est: Option<Real>,
    dc_hz: Option<Real>,
    label: Option<String>,
}

fn random_bytes(rng: &mut Rng, n: usize) -> Vec<u8> {
    rng.bytes(n)
}

/// Transmit `segments` (config, frames) back to back, each by a fresh transmitter
/// (as a station changing settings), through `imp` into a receiver.
fn run(segments: &[(MscConfig, usize)], imp: &Impair) -> Outcome {
    let mut rng = Rng::new(imp.seed.wrapping_add(1000));
    let mut audio: Vec<f32> = Vec::new();
    // Half a second of silence first, like a real recording.
    audio.resize(SAMPLE_RATE as usize / 2, 0.0);
    let mut sent: HashMap<Vec<u8>, (usize, usize)> = HashMap::new();
    let mut out = Outcome::default();
    for (seg, &(cfg, frames)) in segments.iter().enumerate() {
        let mut tx = Transmitter::new(TxConfig::new(cfg, "TEST1"));
        for f in 0..frames {
            let bytes = random_bytes(&mut rng, tx.frame_bytes());
            tx.push_frame(&bytes, &mut audio);
            sent.insert(bytes, (seg, f));
        }
        out.sent.push(frames);
    }
    let signal_end = audio.len();
    // Let the last frames through the receiver's delay line.
    audio.extend(std::iter::repeat_n(0.0, SAMPLE_RATE as usize * 2));
    let audio = channel(&audio, imp);

    let mut rx = Receiver::new(ReceiverConfig::default());
    out.matched = vec![0; segments.len()];
    out.matched_idx = vec![Vec::new(); segments.len()];
    let mut seen = std::collections::HashSet::new();
    let trace = std::env::var_os("HAMDRM_TRACE").is_some();
    for (ci, chunk) in audio.chunks(4800).enumerate() {
        for ev in rx.push(chunk) {
            if trace {
                let t = ci as Real * 0.1;
                match &ev {
                    ReceiverEvent::Msc(m) => eprintln!("{t:5.1}s MSC trusted={} match={:?}", m.trusted, sent.get(&m.bytes)),
                    ReceiverEvent::Fac { fac, .. } => eprintln!("{t:5.1}s FAC frame {} {:?}", fac.frame, rx.status()),
                    e => eprintln!("{t:5.1}s {e:?}"),
                }
            }
            match ev {
                ReceiverEvent::Msc(m) => match sent.get(&m.bytes) {
                    Some(&(seg, f)) => {
                        if seen.insert((seg, f)) {
                            out.matched[seg] += 1;
                            out.matched_idx[seg].push(f);
                            out.cer_good.push(m.cell_error_rate);
                        }
                    }
                    None => {
                        out.unmatched += 1;
                        if m.fac_ok && m.complete {
                            out.cer_bad.push(m.cell_error_rate);
                        }
                        if m.trusted {
                            out.trusted_wrong += 1;
                        }
                    }
                },
                ReceiverEvent::ModeDetected(m) => out.modes.push(m),
                ReceiverEvent::Fac { label: Some(l), .. } => out.label = Some(l),
                _ => {}
            }
        }
        if std::env::var_os("HAMDRM_TRACE2").is_some() {
            let st = rx.status();
            eprintln!("{:5.1}s {:?} {:?} dc={:?} snr={:?} sro={:.2}", ci as Real * 0.1, st.state, st.frame_sync, st.dc_frequency_hz, st.snr_db, st.sro_hz);
        }
        // Quality as seen while the signal is on (not in the trailing silence).
        if (ci + 1) * 4800 < signal_end {
            if rx.status().snr_db.is_some() {
                out.snr_est = rx.status().snr_db;
            }
            if rx.status().dc_frequency_hz.is_some() {
                out.dc_hz = rx.status().dc_frequency_hz;
            }
        }
    }
    out
}

/// Pass real TX audio through the impairments.
fn channel(audio: &[f32], imp: &Impair) -> Vec<f32> {
    let x: Vec<Real> = audio.iter().map(|&v| Real::from(v)).collect();
    let needs_complex = imp.model.is_some() || imp.freq_offset_hz != 0.0 || imp.sro_ppm != 0.0;
    let mut y: Vec<Real> = if needs_complex {
        let z = analytic(&x);
        let cfg = ChannelConfig {
            model: imp.model.clone().unwrap_or_else(ChannelModel::awgn),
            snr_db: None,
            freq_offset_hz: imp.freq_offset_hz,
            sample_rate_offset_ppm: imp.sro_ppm,
            seed: imp.seed,
        };
        // Reference power and bandwidths only matter for the simulator's own noise,
        // which is off here.
        let mut sim = ChannelSimulator::with_reference(1.0, SNR_REF_BW_HZ, SNR_REF_BW_HZ, cfg);
        let mut out = Vec::new();
        for c in z.chunks(4096) {
            sim.process(c, &mut out);
        }
        out.iter().map(|c| c.re).collect()
    } else {
        x.clone()
    };
    if let Some(snr_db) = imp.snr_db {
        // Signal power: the TX audio while it transmits (skip the silences).
        let active: Vec<Real> = x.iter().copied().filter(|v| v.abs() > 1e-9).collect();
        let p = active.iter().map(|v| v * v).sum::<Real>() / active.len().max(1) as Real;
        // Real white noise of variance σ² has σ²/(fs/2) per Hz of the 0..fs/2 band.
        let fs = Real::from(SAMPLE_RATE);
        let sigma = (p / 10f64.powf(snr_db / 10.0) / SNR_REF_BW_HZ * fs / 2.0).sqrt();
        let mut rng = Rng::new(imp.seed ^ 0x5eed);
        for v in &mut y {
            *v += sigma * rng.gaussian();
        }
    }
    y.iter().map(|&v| v as f32).collect()
}

/// Exact analytic signal (FFT, zero the negative frequencies, double the positive).
fn analytic(x: &[Real]) -> Vec<Cplx> {
    let n = x.len();
    let mut fft = Fft::new(n);
    let mut v: Vec<Cplx> = x.iter().map(|&r| Cplx::new(r, 0.0)).collect();
    fft.forward(&mut v);
    for (k, c) in v.iter_mut().enumerate() {
        let w = if k == 0 || (n.is_multiple_of(2) && k == n / 2) {
            1.0
        } else if k < n.div_ceil(2) {
            2.0
        } else {
            0.0
        };
        *c *= w / n as Real;
    }
    fft.inverse(&mut v);
    v
}

fn cfg(mode: Mode, bw: Bandwidth, qam: Qam, protection: u8, interleave: Interleave) -> MscConfig {
    MscConfig { layout: Layout::new(mode, bw), qam, protection, interleave }
}

fn short(mode: Mode, bw: Bandwidth, qam: Qam) -> MscConfig {
    cfg(mode, bw, qam, 0, Interleave::Short)
}

/// Frames that a clean run must decode out of `sent`: the receiver needs about
/// 2–3 s (5–7 frames) to acquire after the signal starts.
const ACQ_FRAMES: usize = 7;

fn check_clean(o: &Outcome, what: &str) {
    eprintln!("{what}: {o:?}");
    assert_eq!(o.trusted_wrong, 0, "{what}: false trusted frames");
    for (s, m) in o.sent.iter().zip(&o.matched) {
        assert!(*m + ACQ_FRAMES >= *s, "{what}: decoded {m} of {s} frames");
    }
}

#[test]
fn clean_every_layout_16qam() {
    for layout in Layout::all() {
        let c = short(layout.mode, layout.bandwidth, Qam::Qam16);
        let o = run(&[(c, 16)], &Impair { seed: 1, ..Default::default() });
        check_clean(&o, &c.to_string());
        assert_eq!(o.modes.last(), Some(&layout.mode));
        assert_eq!(o.label.as_deref(), Some("TEST1"));
    }
}

#[test]
fn clean_4qam_and_64qam() {
    for (mode, bw) in [(Mode::A, Bandwidth::Narrow), (Mode::B, Bandwidth::Wide), (Mode::E, Bandwidth::Wide)] {
        for (qam, prot) in [(Qam::Qam4, 0), (Qam::Qam64, 0), (Qam::Qam64, 1)] {
            let c = cfg(mode, bw, qam, prot, Interleave::Short);
            let o = run(&[(c, 14)], &Impair { seed: 2, ..Default::default() });
            check_clean(&o, &c.to_string());
        }
    }
}

#[test]
fn long_interleaver() {
    let c = cfg(Mode::B, Bandwidth::Wide, Qam::Qam16, 1, Interleave::Long);
    let o = run(&[(c, 20)], &Impair { snr_db: Some(20.0), seed: 3, ..Default::default() });
    // The long interleaver adds four frames of delay and fill.
    eprintln!("long: {o:?}");
    assert_eq!(o.trusted_wrong, 0);
    assert!(o.matched[0] + ACQ_FRAMES + 4 >= 20, "{o:?}");
}

#[test]
fn awgn_offset_and_sample_rate() {
    // +37 Hz tuning error, 100 ppm clock offset, 15 dB SNR.
    let c = short(Mode::B, Bandwidth::Wide, Qam::Qam16);
    let imp = Impair { snr_db: Some(15.0), freq_offset_hz: 37.0, sro_ppm: 100.0, seed: 4, ..Default::default() };
    let o = run(&[(c, 25)], &imp);
    check_clean(&o, "B 16-QAM +37 Hz 100 ppm 15 dB");
    let dc = o.dc_hz.unwrap();
    assert!((dc - 387.0).abs() < 2.0, "DC at {dc} Hz");
    let snr = o.snr_est.unwrap();
    assert!((snr - 15.0).abs() < 3.0, "SNR estimate {snr} dB");
}

#[test]
fn large_tuning_offsets() {
    // The default DC search covers 350 ± 250 Hz.
    for (foff, mode) in [(-150.0, Mode::E), (200.0, Mode::A)] {
        let c = short(mode, Bandwidth::Wide, Qam::Qam16);
        let imp = Impair { snr_db: Some(20.0), freq_offset_hz: foff, seed: 8, ..Default::default() };
        let o = run(&[(c, 16)], &imp);
        check_clean(&o, &format!("{c} {foff:+} Hz"));
        let dc = o.dc_hz.unwrap();
        assert!((dc - 350.0 - foff).abs() < 2.0, "DC at {dc} Hz");
    }
}

#[test]
fn noise_only_decodes_nothing() {
    let mut rng = Rng::new(9);
    let noise: Vec<f32> = (0..SAMPLE_RATE as usize * 15).map(|_| (0.1 * rng.gaussian()) as f32).collect();
    let mut rx = Receiver::new(ReceiverConfig::default());
    for chunk in noise.chunks(4800) {
        for ev in rx.push(chunk) {
            assert!(!matches!(ev, ReceiverEvent::Msc(_) | ReceiverEvent::Fac { .. }), "{ev:?}");
        }
    }
}

#[test]
fn hf_fading_two_path() {
    // CCIR-poor-like: two equal paths 2 ms apart, 1 Hz Doppler spread each, 4-QAM.
    let model = ChannelModel::custom(vec![Path::new(0.0, 1.0, 0.0, 1.0), Path::new(2.0, 1.0, 0.0, 1.0)]);
    let c = short(Mode::B, Bandwidth::Wide, Qam::Qam4);
    let imp = Impair { snr_db: Some(20.0), model: Some(model), seed: 5, ..Default::default() };
    let o = run(&[(c, 30)], &imp);
    eprintln!("fading: {o:?}");
    assert_eq!(o.trusted_wrong, 0);
    // Deep fades lose some frames; most must survive.
    assert!(o.matched[0] >= 15, "{o:?}");
}

#[test]
fn config_change_mid_stream() {
    // Same mode (not signalled), new bandwidth, constellation and protection.
    let a = short(Mode::B, Bandwidth::Wide, Qam::Qam16);
    let b = cfg(Mode::B, Bandwidth::Narrow, Qam::Qam64, 1, Interleave::Short);
    let o = run(&[(a, 12), (b, 12)], &Impair { snr_db: Some(25.0), seed: 6, ..Default::default() });
    eprintln!("change: {o:?}");
    assert_eq!(o.trusted_wrong, 0);
    assert!(o.matched[0] + ACQ_FRAMES >= 12, "{o:?}");
    // The new setting must decode after at most a couple of frames.
    assert!(o.matched[1] >= 9, "{o:?}");
}

/// Ad-hoc diagnosis: `HAMDRM_DIAG="mode qam snr foff ppm seed frames"`.
#[test]
#[ignore]
fn diag() {
    let Ok(spec) = std::env::var("HAMDRM_DIAG") else { return };
    let v: Vec<&str> = spec.split_whitespace().collect();
    let mode = match v[0] { "A" => Mode::A, "E" => Mode::E, _ => Mode::B };
    let qam = match v[1] { "4" => Qam::Qam4, "64" => Qam::Qam64, _ => Qam::Qam16 };
    let p = |i: usize| v[i].parse::<Real>().unwrap();
    let snr = p(2);
    let imp = Impair {
        snr_db: (snr < 99.0).then_some(snr),
        freq_offset_hz: p(3),
        sro_ppm: p(4),
        seed: p(5) as u64,
        ..Default::default()
    };
    let o = run(&[(short(mode, Bandwidth::Wide, qam), p(6) as usize)], &imp);
    eprintln!("{o:?}");
}

#[test]
#[ignore]
fn mode_change_mid_stream() {
    // The robustness mode is not signalled: the receiver must lose the FAC, restart
    // and detect the new mode.
    let a = short(Mode::B, Bandwidth::Wide, Qam::Qam16);
    let b = short(Mode::E, Bandwidth::Wide, Qam::Qam16);
    let o = run(&[(a, 12), (b, 30)], &Impair { snr_db: Some(25.0), seed: 7, ..Default::default() });
    eprintln!("mode change: {o:?}");
    assert!(o.matched[1] >= 10, "{o:?}");
}

/// Fraction of the frames from index `ACQ_FRAMES` on that decoded, and the
/// outcome (for the cell-error-rate statistics).
fn success(c: MscConfig, imp: &Impair, frames: usize) -> (Real, Outcome) {
    let o = run(&[(c, frames)], imp);
    let ok = o.matched_idx[0].iter().filter(|&&f| f >= ACQ_FRAMES).count();
    (ok as Real / (frames - ACQ_FRAMES) as Real, o)
}

/// AWGN thresholds: lowest SNR (0.5 dB steps) at which ≥ 99 % of the frames after
/// acquisition decode, per configuration. Two seeds × 60 frames per point.
#[test]
#[ignore]
fn awgn_threshold_sweep() {
    let cases = [
        cfg(Mode::B, Bandwidth::Wide, Qam::Qam4, 0, Interleave::Short),
        cfg(Mode::B, Bandwidth::Wide, Qam::Qam16, 0, Interleave::Short),
        cfg(Mode::B, Bandwidth::Wide, Qam::Qam16, 1, Interleave::Short),
        cfg(Mode::B, Bandwidth::Wide, Qam::Qam64, 0, Interleave::Short),
        cfg(Mode::A, Bandwidth::Wide, Qam::Qam16, 0, Interleave::Short),
        cfg(Mode::E, Bandwidth::Wide, Qam::Qam16, 0, Interleave::Short),
    ];
    for c in cases {
        let mut snr = 20.0;
        let mut last_ok = None;
        while snr > -2.0 {
            let (mut rate, mut good, mut bad, mut false_trust) = (0.0, Vec::new(), Vec::new(), 0);
            for seed in 0..2 {
                let (r, o) = success(c, &Impair { snr_db: Some(snr), seed: 100 + seed, ..Default::default() }, 60);
                rate += r / 2.0;
                good.extend(o.cer_good);
                bad.extend(o.cer_bad);
                false_trust += o.trusted_wrong;
            }
            let max_good = good.iter().copied().fold(0.0, Real::max);
            let min_bad = bad.iter().copied().fold(1.0, Real::min);
            eprintln!(
                "{c}: {snr:.1} dB → {:.1} % (CER good ≤ {max_good:.3}, in-sync wrong {} ≥ {min_bad:.3}, false trusted {false_trust})",
                rate * 100.0,
                bad.len()
            );
            if rate < 0.99 {
                break;
            }
            last_ok = Some(snr);
            snr -= 0.5;
        }
        eprintln!("THRESHOLD {c}: {last_ok:?} dB");
    }
}
