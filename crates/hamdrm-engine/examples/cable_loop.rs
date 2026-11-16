//! Live sound-card loop: one engine transmits a file into an output device, a second
//! engine receives it from the matching input (e.g. VB-Audio cable A). PTT stays VOX.
//!
//! `cargo run --release -p hamdrm-engine --example cable_loop -- FILE OUT_DIR
//!  ["CABLE-A Input"] ["CABLE-A Output"] [mode B|A|E] [qam 4|16|64]`

use hamdrm_engine::*;
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let file = PathBuf::from(&a[0]);
    let out_dir = PathBuf::from(&a[1]);
    let out_dev = a.get(2).cloned().unwrap_or("CABLE-A Input".into());
    let in_dev = a.get(3).cloned().unwrap_or("CABLE-A Output".into());
    let mode = match a.get(4).map(String::as_str) {
        Some("A") => Mode::A,
        Some("E") => Mode::E,
        _ => Mode::B,
    };
    let qam = match a.get(5).map(String::as_str) {
        Some("4") => Qam::Qam4,
        Some("64") => Qam::Qam64,
        _ => Qam::Qam16,
    };
    let rx = Engine::start(Options {
        input: Input::Sound(Some(in_dev)),
        output: Some(String::new()),
        ptt: PttConfig::default(),
        out_dir,
        rx: ReceiverConfig::default(),
        auto_bsr: false,
        mic: None,
        speaker: Some(String::new()),
        mic_gain: 1.0,
    });
    let tx = Engine::start(Options {
        input: Input::None,
        output: Some(out_dev),
        ptt: PttConfig::default(),
        out_dir: std::env::temp_dir(),
        rx: ReceiverConfig::default(),
        auto_bsr: false,
        mic: None,
        speaker: Some(String::new()),
        mic_gain: 1.0,
    });
    std::thread::sleep(Duration::from_millis(500));
    let mut s = TxSettings {
        call: "LOOP".into(),
        ..Default::default()
    };
    s.msc.layout = Layout::new(mode, Bandwidth::Wide);
    s.msc.qam = qam;
    tx.send(Command::SendFiles(vec![file], s));
    let t0 = Instant::now();
    loop {
        std::thread::sleep(Duration::from_secs(1));
        let (txv, txlog) = tx.with_view(|v| (v.tx.clone(), v.log.clone()));
        let (files, st, rxlog) =
            rx.with_view(|v| (v.files.clone(), v.status.clone(), v.log.clone()));
        eprintln!(
            "{:>4.0} s  tx {}  rx {:?} SNR {:?} FAC {} MSC {}",
            t0.elapsed().as_secs_f64(),
            txv.map_or("idle".into(), |t| format!(
                "{:.0}/{:.0} s",
                t.done_s, t.total_s
            )),
            st.state,
            st.snr_db.map(|v| v.round()),
            st.fac_ok,
            st.msc_ok
        );
        if let Some(f) = files.first() {
            eprintln!("received {} ({} bytes)", f.path.display(), f.bytes);
            break;
        }
        if t0.elapsed() > Duration::from_secs(180) {
            eprintln!("timeout\nTX log: {txlog:?}\nRX log: {rxlog:?}");
            break;
        }
    }
}
