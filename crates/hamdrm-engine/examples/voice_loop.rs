//! Live voice loop over two virtual cables, silent: speech WAV → "CABLE-B Input";
//! engine A talks from "CABLE-B Output" into "CABLE-A Input"; engine B receives from
//! "CABLE-A Output" (no loudspeaker). PTT stays VOX.
//!
//! `cargo run --release -p hamdrm-engine --example voice_loop -- SPEECH.wav [neural|lpc10]`

use hamdrm_engine::decdsss_audio::{OutputOptions, OutputStream};
use hamdrm_engine::*;
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let speech = read_wav(&PathBuf::from(&a[0])).expect("speech wav");
    let kind = if a.get(1).map(String::as_str) == Some("lpc10") {
        VoiceKind::Lpc10
    } else {
        VoiceKind::Neural
    };
    let opts = |input: Input, output: &str, mic: Option<&str>| Options {
        input,
        output: Some(output.to_string()),
        ptt: PttConfig::default(),
        out_dir: std::env::temp_dir(),
        rx: ReceiverConfig::default(),
        auto_bsr: false,
        mic: mic.map(str::to_string),
        speaker: Some(String::new()),
        mic_gain: 1.0,
    };
    let rx = Engine::start(opts(Input::Sound(Some("CABLE-A Output".into())), "", None));
    let tx = Engine::start(opts(Input::None, "CABLE-A Input", Some("CABLE-B Output")));
    let mut feed = OutputStream::open(&OutputOptions {
        device: Some("CABLE-B Input".into()),
        sample_rate: Some(48_000),
        channels: Some(1),
        buffer: Duration::from_secs(2),
        start_threshold: Duration::from_millis(100),
    })
    .expect("CABLE-B Input");
    std::thread::sleep(Duration::from_millis(500));
    tx.send(Command::Voice(
        true,
        TxSettings {
            call: "VOICE".into(),
            ..Default::default()
        },
        kind,
    ));
    let t0 = Instant::now();
    let mut pos = 0;
    let total = speech.len() as f64 / 48_000.0;
    let mut stopped = false;
    loop {
        // Feed the "microphone" in real time.
        while pos < speech.len() && feed.free_frames() >= 4800 {
            let end = (pos + 4800).min(speech.len());
            let _ = feed.write(&speech[pos..end]);
            pos = end;
        }
        std::thread::sleep(Duration::from_millis(50));
        let el = t0.elapsed().as_secs_f64();
        if !stopped && el > total + 1.5 {
            tx.send(Command::Voice(false, TxSettings::default(), kind));
            stopped = true;
        }
        if el > total + 6.0 {
            break;
        }
    }
    let (voice, st, log) = rx.with_view(|v| (v.voice.clone(), v.status.clone(), v.log.clone()));
    let txlog = tx.with_view(|v| v.log.clone());
    println!(
        "rx voice: {voice:?}\nFAC ok {} MSC ok {}\nrx log: {log:?}\ntx log: {txlog:?}",
        st.fac_ok, st.msc_ok
    );
}
