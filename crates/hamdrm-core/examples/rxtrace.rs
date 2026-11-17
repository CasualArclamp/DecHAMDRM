//! Trace the receiver's synchronisation over a recording: one line per second with
//! state, DC frequency, sample-rate correction and FAC counts.
//! `cargo run --release -p hamdrm-core --example rxtrace -- FILE.wav` (48 kHz mono 16-bit).

use hamdrm_core::rx::{Receiver, ReceiverConfig, ReceiverEvent};

fn main() {
    let path = std::env::args().nth(1).expect("usage: rxtrace FILE.wav");
    let bytes = std::fs::read(&path).expect("read");
    let x: Vec<f32> = bytes[44..]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| f32::from(i16::from_le_bytes(*b)) / 32768.0)
        .collect();
    let mut rx = Receiver::new(ReceiverConfig::default());
    for (sec, chunk) in x.chunks(48_000).enumerate() {
        let mut notes = Vec::new();
        for ev in rx.push(chunk) {
            match ev {
                ReceiverEvent::SignalFound { dc_hz } => notes.push(format!("found {dc_hz:.1}")),
                ReceiverEvent::Restarted => notes.push("restart".into()),
                ReceiverEvent::Resynchronising => notes.push("resync".into()),
                ReceiverEvent::ModeDetected(m) => notes.push(format!("mode {m:?}")),
                ReceiverEvent::Configured(c) => notes.push(format!("{c:?}")),
                _ => {}
            }
        }
        let s = rx.status();
        println!(
            "{sec:3} s {:?} dc {} sro {:+.2} Hz fac {}/{} msc {}/{}  {}",
            s.state,
            s.dc_frequency_hz.map_or("-".into(), |d| format!("{d:.2}")),
            s.sro_hz,
            s.fac_ok,
            s.fac_bad,
            s.msc_ok,
            s.msc_doubtful,
            notes.join(", ")
        );
    }
}
