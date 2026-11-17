//! DecHAMDRM engine: one worker thread that owns the sound card (or a WAV file), the
//! HamDRM receiver, the file receiver and the transmit queue, and keys the rig.
//!
//! The UI sends [`Command`]s and reads a [`View`] snapshot. HamDRM is half duplex:
//! while transmitting the receiver is paused and restarted afterwards.

mod tx;
mod waterfall;

pub use decdsss_rig::{
    Config as PttConfig, Method as PttMethod, Model as RigModel, Status as PttStatus,
};
pub use hamdrm_core::params::{Bandwidth, Interleave, Layout, Mode, MscConfig, Qam};
pub use hamdrm_core::rx::{ChainVisuals, PdsAxis, ReceiverConfig, RxState, RxStatus};
pub use hamdrm_data::bsr::BsrRequest;
pub use tx::TxSettings;

use decdsss_audio::{InputOptions, InputStream, OutputOptions, OutputStream};
use decdsss_rig::Ptt;
pub use hamdrm_core::Cplx;
use hamdrm_core::fac::{AudioCoding, EXT_NEURAL_VOICE, Service};
use hamdrm_core::rx::{Receiver, ReceiverEvent};
use hamdrm_data::receive::{FileEvent, FileReceiver};
use hamdrm_voice::VoiceCodec;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver as ChanRx, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};
use tx::TxJob;
use waterfall::Waterfall;

/// Working sample rate.
pub const RATE: u32 = 48_000;
/// Waterfall rows kept for the UI.
pub const WATERFALL_ROWS: usize = 600;
const LOG_LINES: usize = 300;

/// Where the receiver's audio comes from.
#[derive(Debug, Clone)]
pub enum Input {
    /// A sound card input (`None` = system default).
    Sound(Option<String>),
    /// A 48 kHz WAV file, played at `speed` × real time (0 = as fast as possible).
    File { path: PathBuf, speed: f64 },
    /// No input (transmit only).
    None,
}

/// Engine settings.
#[derive(Debug, Clone)]
pub struct Options {
    pub input: Input,
    /// Sound card output for transmitting (`None` = system default; `Some("")` = no
    /// output: transmissions are only drawn on the waterfall).
    pub output: Option<String>,
    pub ptt: PttConfig,
    /// Received files are saved here.
    pub out_dir: PathBuf,
    pub rx: ReceiverConfig,
    /// Send repairs for BSRs about our own files without asking.
    pub auto_bsr: bool,
    /// Microphone for voice transmissions (`None` = system default input).
    pub mic: Option<String>,
    /// Loudspeaker for received voice (`None` = system default; `Some("")` = none).
    pub speaker: Option<String>,
    /// Microphone gain (linear).
    pub mic_gain: f32,
}

/// Which voice codec to transmit with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VoiceKind {
    /// EasyDRF's LPC-10 (EasyDRF can receive it).
    Lpc10,
    /// DecHAMDRM neural voice (Mimi): better sound, only DecHAMDRM receives it.
    Neural,
}

/// Received digital voice.
#[derive(Debug, Clone, Default)]
pub struct VoiceView {
    pub codec: String,
    pub good: u64,
    pub bad: u64,
    pub active: bool,
}

/// What the UI can ask for.
#[derive(Debug, Clone)]
pub enum Command {
    SendFiles(Vec<PathBuf>, TxSettings),
    /// Ask the sender of object `tid` for its missing segments.
    SendBsr(u16, TxSettings),
    /// Answer a BSR about one of our files.
    SendRepair(BsrRequest, TxSettings),
    /// Waterfall text (callsign or a short message).
    SendWaterfall(String, TxSettings),
    Tune(bool, TxSettings),
    StopTx,
    /// Change the input (restarts the receiver).
    SetInput(Input),
    SetPtt(PttConfig),
    SetOutput(Option<String>),
    /// Voice transmission on (start) or off (flush and stop).
    Voice(bool, TxSettings, VoiceKind),
    SetAudioDevices {
        mic: Option<String>,
        speaker: Option<String>,
        mic_gain: f32,
    },
    Quit,
}

/// A body object being received.
#[derive(Debug, Clone)]
pub struct ObjectView {
    pub tid: u16,
    pub name: Option<String>,
    pub received: usize,
    pub total: Option<usize>,
    /// Per body segment: received or not.
    pub segments: Vec<bool>,
    pub done: bool,
    pub updated: Instant,
}

/// A received file.
#[derive(Debug, Clone)]
pub struct RxFile {
    pub path: PathBuf,
    pub name: String,
    pub from: Option<String>,
    pub when: SystemTime,
    pub rs: Option<String>,
    pub bytes: usize,
}

/// A BSR that arrived, and whether it is about one of our files.
#[derive(Debug, Clone)]
pub struct BsrSeen {
    pub request: BsrRequest,
    pub from: Option<String>,
    /// Name of our file it refers to.
    pub ours: Option<String>,
}

/// Transmit progress.
#[derive(Debug, Clone)]
pub struct TxView {
    pub what: String,
    pub done_s: f64,
    pub total_s: f64,
}

/// Snapshot for the UI.
#[derive(Debug, Clone, Default)]
pub struct View {
    pub status: RxStatus,
    /// Constellations, channel estimate, impulse response, per-carrier SNR.
    pub chain: ChainVisuals,
    pub spectrum_db: Vec<f64>,
    pub spectrum_bin_hz: f64,
    pub signal_band_hz: Option<(f64, f64)>,
    pub dc_hz: Option<f64>,
    /// Newest waterfall rows last; dB, bin `i` at `i · waterfall_bin_hz`.
    pub waterfall: VecDeque<Vec<f32>>,
    pub waterfall_bin_hz: f64,
    /// Rows added since the engine started (the UI scrolls by the difference).
    pub waterfall_count: u64,
    pub input_db: f64,
    pub objects: Vec<ObjectView>,
    pub files: Vec<RxFile>,
    pub bsrs: Vec<BsrSeen>,
    pub log: VecDeque<String>,
    pub tx: Option<TxView>,
    pub ptt: Option<PttStatus>,
    pub input_name: String,
    pub output_name: String,
    pub input_error: Option<String>,
    /// Station heard most recently (FAC label).
    pub heard: Option<String>,
    pub crc_errors: u64,
    /// Cell error rate before FEC (0..1), smoothed over a few MSC frames: the cells
    /// the Viterbi decoders had to correct, judged against the re-encoded result.
    pub msc_cer: Option<f64>,
    /// File input finished.
    pub eof: bool,
    pub voice: Option<VoiceView>,
    /// Neural voice weights present.
    pub neural_ready: bool,
}

/// Handle to the worker thread.
pub struct Engine {
    tx: Sender<Command>,
    view: Arc<Mutex<View>>,
    handle: Option<JoinHandle<()>>,
}

impl Engine {
    pub fn start(opts: Options) -> Self {
        let (tx, rx) = channel();
        let view = Arc::new(Mutex::new(View {
            waterfall_bin_hz: waterfall::BIN_HZ,
            ..Default::default()
        }));
        let v = view.clone();
        let handle = std::thread::Builder::new()
            .name("hamdrm-engine".into())
            .spawn(move || Worker::new(opts, v, rx).run())
            .expect("spawn engine thread");
        Self {
            tx,
            view,
            handle: Some(handle),
        }
    }

    pub fn send(&self, c: Command) {
        let _ = self.tx.send(c);
    }

    /// Run `f` on the current view.
    pub fn with_view<R>(&self, f: impl FnOnce(&View) -> R) -> R {
        f(&self.view.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.tx.send(Command::Quit);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

enum Source {
    Sound(InputStream),
    File {
        samples: Vec<f32>,
        pos: usize,
        speed: f64,
        started: Instant,
    },
    None,
}

/// A file we sent recently (for BSR repairs).
struct Sent {
    name: String,
    data: Vec<u8>,
    tid: u16,
}

struct Worker {
    opts: Options,
    view: Arc<Mutex<View>>,
    cmds: ChanRx<Command>,
    source: Source,
    output: Option<OutputStream>,
    ptt: Arc<Ptt>,
    rx: Receiver,
    files: FileReceiver,
    wf: Waterfall,
    job: Option<TxJob>,
    keyed: bool,
    sent: VecDeque<Sent>,
    log: VecDeque<String>,
    new_rows: Vec<Vec<f32>>,
    heard: Option<String>,
    rx_files: Vec<RxFile>,
    bsrs: Vec<BsrSeen>,
    input_db: f64,
    last_settings: TxSettings,
    /// Service and FAC extension byte of the newest good FAC.
    service: Option<(Service, u8)>,
    rx_voice: Option<(VoiceKey, Box<dyn VoiceCodec>)>,
    voice_view: Option<VoiceView>,
    speaker: Option<OutputStream>,
    neural_failed: bool,
    unsupported_voice_logged: bool,
    msc_cer: Option<f64>,
    quit: bool,
}

impl Worker {
    fn new(opts: Options, view: Arc<Mutex<View>>, cmds: ChanRx<Command>) -> Self {
        let ptt = Arc::new(Ptt::open(opts.ptt.clone()));
        let mut w = Self {
            rx: Receiver::new(opts.rx.clone()),
            files: FileReceiver::new(),
            wf: Waterfall::new(),
            source: Source::None,
            output: None,
            ptt,
            job: None,
            keyed: false,
            sent: VecDeque::new(),
            log: VecDeque::new(),
            new_rows: Vec::new(),
            heard: None,
            rx_files: Vec::new(),
            bsrs: Vec::new(),
            input_db: -120.0,
            last_settings: TxSettings::default(),
            service: None,
            rx_voice: None,
            voice_view: None,
            speaker: None,
            neural_failed: false,
            unsupported_voice_logged: false,
            msc_cer: None,
            quit: false,
            opts,
            view,
            cmds,
        };
        let input = w.opts.input.clone();
        w.open_input(input);
        w
    }

    fn log(&mut self, s: impl Into<String>) {
        let t = chrono_hms();
        self.log.push_back(format!("{t} {}", s.into()));
        while self.log.len() > LOG_LINES {
            self.log.pop_front();
        }
    }

    fn open_input(&mut self, input: Input) {
        self.source = Source::None;
        let mut err = None;
        match &input {
            Input::Sound(dev) => {
                let o = InputOptions {
                    device: dev.clone(),
                    sample_rate: Some(RATE),
                    channels: Some(1),
                    ..Default::default()
                };
                match InputStream::open(&o) {
                    Ok(s) if s.format().sample_rate == RATE => {
                        let n = s.device_name().to_string();
                        self.log(format!("input: {n}"));
                        self.set_names(Some(n), None);
                        self.source = Source::Sound(s);
                    }
                    Ok(s) => {
                        err = Some(format!(
                            "{} runs at {} Hz; 48 kHz needed",
                            s.device_name(),
                            s.format().sample_rate
                        ))
                    }
                    Err(e) => err = Some(format!("input: {e}")),
                }
            }
            Input::File { path, speed } => match read_wav(path) {
                Ok(samples) => {
                    self.log(format!("input: {}", path.display()));
                    self.set_names(Some(path.display().to_string()), None);
                    self.source = Source::File {
                        samples,
                        pos: 0,
                        speed: *speed,
                        started: Instant::now(),
                    };
                }
                Err(e) => err = Some(e),
            },
            Input::None => {}
        }
        if let Some(e) = &err {
            self.log(e.clone());
        }
        self.view
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .input_error = err;
        self.rx.restart();
        self.opts.input = input;
    }

    fn set_names(&mut self, input: Option<String>, output: Option<String>) {
        let mut v = self.view.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(i) = input {
            v.input_name = i;
        }
        if let Some(o) = output {
            v.output_name = o;
        }
    }

    fn open_output(&mut self) -> bool {
        if self.output.is_some() {
            return true;
        }
        if self.opts.output.as_deref() == Some("") {
            return false;
        }
        let o = OutputOptions {
            device: self.opts.output.clone(),
            sample_rate: Some(RATE),
            channels: Some(1),
            buffer: Duration::from_millis(1500),
            start_threshold: Duration::from_millis(200),
        };
        match OutputStream::open(&o) {
            Ok(s) if s.format().sample_rate == RATE => {
                let n = s.device_name().to_string();
                self.log(format!("output: {n}"));
                self.set_names(None, Some(n));
                self.output = Some(s);
                true
            }
            Ok(s) => {
                self.log(format!(
                    "output {} runs at {} Hz; 48 kHz needed",
                    s.device_name(),
                    s.format().sample_rate
                ));
                false
            }
            Err(e) => {
                self.log(format!("output: {e}"));
                false
            }
        }
    }

    fn run(mut self) {
        while !self.quit {
            while let Ok(c) = self.cmds.try_recv() {
                self.command(c);
            }
            if self.job.is_some() {
                self.transmit_step();
            } else {
                self.receive_step();
            }
            self.publish();
        }
        if self.keyed {
            let _ = self.ptt.key(false, decdsss_rig::UNKEY_WAIT);
        }
    }

    fn command(&mut self, c: Command) {
        if let Command::SendFiles(_, s) | Command::SendBsr(_, s) | Command::SendWaterfall(_, s) = &c
        {
            self.last_settings = s.clone();
        }
        match c {
            Command::SendFiles(paths, s) => {
                let mut files = Vec::new();
                for p in &paths {
                    match std::fs::read(p) {
                        Ok(d) => files.push((file_name(p), d)),
                        Err(e) => self.log(format!("{}: {e}", p.display())),
                    }
                }
                if files.is_empty() {
                    return;
                }
                for (name, data) in &files {
                    let obj = hamdrm_data::send::object_for(
                        &hamdrm_data::send::OutFile {
                            name: name.clone(),
                            data: data.clone(),
                        },
                        s.rs,
                    );
                    self.sent.push_back(Sent {
                        name: name.clone(),
                        data: data.clone(),
                        tid: obj.tid,
                    });
                    while self.sent.len() > 6 {
                        self.sent.pop_front();
                    }
                }
                let names: Vec<&str> = files.iter().map(|f| f.0.as_str()).collect();
                let what = format!("sending {}", names.join(", "));
                self.start_job(TxJob::files(&files, &s), what);
            }
            Command::SendBsr(tid, s) => match self.files.bsr_for(tid) {
                Some(req) if !req.missing.is_empty() || !req.header_ok => {
                    let what = format!("BSR: {} segments of transport {tid}", req.missing.len());
                    self.start_job(TxJob::bsr(&req, &s), what);
                }
                _ => self.log(format!("nothing missing for transport {tid}")),
            },
            Command::SendRepair(req, s) => {
                if let Some(sent) = self.sent.iter().find(|x| x.tid == req.tid) {
                    let what = format!("repair: {} segments of {}", req.missing.len(), sent.name);
                    let job = TxJob::repair(&sent.name, &sent.data, &req, &s);
                    self.start_job(job, what);
                } else {
                    self.log(format!(
                        "BSR for transport {} is not about our files",
                        req.tid
                    ));
                }
            }
            Command::SendWaterfall(text, s) => {
                let what = format!("waterfall text \"{text}\"");
                self.start_job(TxJob::waterfall(&text, &s), what);
            }
            Command::Tune(on, s) => {
                if on {
                    self.start_job(TxJob::tune(&s), "tune".into());
                } else if self.job.as_ref().is_some_and(|j| j.is_tune()) {
                    self.stop_tx();
                }
            }
            Command::StopTx => self.stop_tx(),
            Command::Voice(true, s, kind) => self.start_voice(s, kind),
            Command::Voice(false, ..) => {
                if let Some(j) = self.job.as_mut().filter(|j| j.is_voice()) {
                    j.stop_voice();
                }
            }
            Command::SetAudioDevices {
                mic,
                speaker,
                mic_gain,
            } => {
                if mic != self.opts.mic {
                    self.log(format!(
                        "microphone: {}",
                        mic.as_deref().unwrap_or("system default")
                    ));
                }
                self.opts.mic = mic;
                self.opts.speaker = speaker;
                self.opts.mic_gain = mic_gain;
                self.speaker = None;
            }
            Command::SetInput(i) => self.open_input(i),
            Command::SetPtt(p) => {
                self.opts.ptt = p.clone();
                self.ptt = Arc::new(Ptt::open(p));
            }
            Command::SetOutput(o) => {
                self.output = None;
                self.opts.output = o;
            }
            Command::Quit => self.quit = true,
        }
    }

    fn start_job(&mut self, job: TxJob, what: String) {
        if self.job.is_some() {
            self.log("busy transmitting");
            return;
        }
        if !self.open_output() {
            self.log("no output: transmission only shown on the waterfall");
        }
        if self.opts.ptt.keys() {
            match self.ptt.key(true, decdsss_rig::KEY_WAIT) {
                Ok(()) => self.keyed = true,
                Err(e) => {
                    self.log(format!("PTT: {e}; not transmitting"));
                    return;
                }
            }
        }
        self.log(format!("TX {what} ({:.1} s)", job.total_s()));
        self.job = Some(job.with_label(what));
    }

    fn stop_tx(&mut self) {
        if let Some(j) = self.job.take() {
            self.log(format!("TX stopped: {}", j.label()));
            if let Some(o) = &mut self.output {
                // Drop what is queued: re-opening flushes the ring.
                let _ = o.pause();
            }
            self.output = None;
            self.end_tx();
        }
    }

    fn end_tx(&mut self) {
        if self.keyed {
            if let Err(e) = self.ptt.key(false, decdsss_rig::UNKEY_WAIT) {
                self.log(format!("PTT release: {e}"));
            }
            self.keyed = false;
        }
        // Discard what the input captured during TX, then listen again.
        if let Source::Sound(s) = &mut self.source {
            let n = s.available_frames();
            let _ = s.read(n);
        }
        self.rx.restart();
    }

    fn transmit_step(&mut self) {
        let job = self.job.as_mut().expect("job");
        let free = match &self.output {
            Some(o) => o.free_frames(),
            None => 4800,
        };
        if free >= 4800 && !job.finished() {
            let block = job.next_block(free.min(19_200));
            self.wf.push(&block, &mut self.new_rows);
            match &mut self.output {
                Some(o) => {
                    let _ = o.write(&block);
                }
                // No output: pace the waterfall at real time.
                None => std::thread::sleep(Duration::from_secs_f64(
                    block.len() as f64 / f64::from(RATE),
                )),
            }
        } else if job.finished() {
            let drained = self
                .output
                .as_mut()
                .is_none_or(|o| o.drain(Duration::from_millis(20)));
            if drained {
                let j = self.job.take().expect("job");
                self.log(format!("TX done: {}", j.label()));
                self.end_tx();
            }
        } else {
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn receive_step(&mut self) {
        let block = match &mut self.source {
            Source::Sound(s) => {
                // 40 ms blocks: the plots follow the signal symbol by symbol.
                if s.available_frames() < 1920 {
                    std::thread::sleep(Duration::from_millis(5));
                    return;
                }
                s.read(1920)
            }
            Source::File {
                samples,
                pos,
                speed,
                started,
            } => {
                if *pos >= samples.len() {
                    self.view.lock().unwrap_or_else(|e| e.into_inner()).eof = true;
                    std::thread::sleep(Duration::from_millis(20));
                    return;
                }
                if *speed > 0.0 {
                    let due = started.elapsed().as_secs_f64() * *speed * f64::from(RATE);
                    if (*pos as f64) > due {
                        std::thread::sleep(Duration::from_millis(5));
                        return;
                    }
                }
                let end = (*pos + 1920).min(samples.len());
                let b = samples[*pos..end].to_vec();
                *pos = end;
                b
            }
            Source::None => {
                std::thread::sleep(Duration::from_millis(20));
                return;
            }
        };
        let p = block
            .iter()
            .map(|v| f64::from(*v) * f64::from(*v))
            .sum::<f64>()
            / block.len().max(1) as f64;
        self.input_db = 10.0 * (p + 1e-12).log10();
        self.wf.push(&block, &mut self.new_rows);
        for ev in self.rx.push(&block) {
            self.rx_event(ev);
        }
    }

    fn rx_event(&mut self, ev: ReceiverEvent) {
        match ev {
            ReceiverEvent::SignalFound { dc_hz } => self.log(format!("signal at {dc_hz:.0} Hz")),
            ReceiverEvent::Configured(c) => self.log(format!("receiving {c}")),
            ReceiverEvent::Fac { fac, label } => {
                self.service = Some((fac.service, fac.ext));
                if let Some(l) = label
                    && self.heard.as_deref() != Some(&l)
                    && !l.is_empty()
                {
                    self.log(format!("station {l}"));
                    self.heard = Some(l);
                }
            }
            ReceiverEvent::Restarted => {
                if let Some(v) = &mut self.voice_view {
                    v.active = false;
                }
            }
            ReceiverEvent::Msc(f) => {
                if f.complete {
                    let c = f.cell_error_rate;
                    self.msc_cer = Some(self.msc_cer.map_or(c, |a| a + 0.3 * (c - a)));
                }
                self.msc_frame(&f.bytes);
            }
            _ => {}
        }
    }

    fn msc_frame(&mut self, bytes: &[u8]) {
        match self.service {
            Some((Service::Audio { coding, .. }, ext)) => self.voice_frame(bytes, coding, ext),
            _ => {
                for fe in self.files.push(Some(bytes)) {
                    self.file_event(fe);
                }
            }
        }
    }

    fn file_event(&mut self, fe: FileEvent) {
        match fe {
            FileEvent::Progress { .. } => {}
            FileEvent::File { name, data, rs, .. } => {
                let _ = std::fs::create_dir_all(&self.opts.out_dir);
                let path = unique_path(&self.opts.out_dir, &name);
                match std::fs::write(&path, &data) {
                    Ok(()) => {
                        self.log(format!("received {name} ({} bytes)", data.len()));
                        self.rx_files.push(RxFile {
                            path,
                            name,
                            from: self.heard.clone(),
                            when: SystemTime::now(),
                            rs,
                            bytes: data.len(),
                        });
                    }
                    Err(e) => self.log(format!("saving {name}: {e}")),
                }
            }
            FileEvent::Bsr(req) => {
                let ours = self
                    .sent
                    .iter()
                    .find(|s| s.tid == req.tid)
                    .map(|s| s.name.clone());
                self.log(format!(
                    "BSR from {}: {} segments{}",
                    self.heard.as_deref().unwrap_or("?"),
                    req.missing.len(),
                    ours.as_ref()
                        .map(|n| format!(" of our {n}"))
                        .unwrap_or_default()
                ));
                let seen = BsrSeen {
                    request: req,
                    from: self.heard.clone(),
                    ours,
                };
                if self.opts.auto_bsr && seen.ours.is_some() {
                    let s = self.last_settings.clone();
                    self.command(Command::SendRepair(seen.request.clone(), s));
                }
                self.bsrs.push(seen);
            }
        }
    }

    fn start_voice(&mut self, s: TxSettings, kind: VoiceKind) {
        if self.job.is_some() {
            self.log("busy transmitting");
            return;
        }
        let (codec, service, ext): (Box<dyn VoiceCodec>, Service, u8) = match kind {
            VoiceKind::Lpc10 => (
                Box::new(hamdrm_voice::easydrf::EasyDrfLpc::new()),
                Service::Audio {
                    coding: AudioCoding::Lpc,
                    text: false,
                },
                0,
            ),
            VoiceKind::Neural => match load_neural() {
                Ok(c) => (
                    c,
                    Service::Audio {
                        coding: AudioCoding::Celp,
                        text: false,
                    },
                    EXT_NEURAL_VOICE,
                ),
                Err(e) => {
                    self.log(format!("neural voice: {e}"));
                    return;
                }
            },
        };
        let o = InputOptions {
            device: self.opts.mic.clone(),
            sample_rate: Some(RATE),
            channels: Some(1),
            ..Default::default()
        };
        let mic = match InputStream::open(&o) {
            Ok(m) if m.format().sample_rate == RATE => m,
            Ok(m) => {
                self.log(format!(
                    "microphone {} runs at {} Hz; 48 kHz needed",
                    m.device_name(),
                    m.format().sample_rate
                ));
                return;
            }
            Err(e) => {
                self.log(format!("microphone: {e}"));
                return;
            }
        };
        let what = format!("voice ({}) from {}", codec.name(), mic.device_name());
        self.last_settings = s.clone();
        let job = TxJob::voice(&s, codec, mic, service, ext, self.opts.mic_gain);
        self.start_job(job, what);
    }

    /// One MSC frame of an audio service: decode and play.
    fn voice_frame(&mut self, bytes: &[u8], coding: AudioCoding, ext: u8) {
        let key = match (coding, ext) {
            (AudioCoding::Lpc, _) => VoiceKey::Lpc10,
            (AudioCoding::Celp, EXT_NEURAL_VOICE) => VoiceKey::Neural,
            _ => {
                if !self.unsupported_voice_logged {
                    self.unsupported_voice_logged = true;
                    self.log(format!("voice heard in {coding:?} coding: not supported"));
                }
                return;
            }
        };
        if self.rx_voice.as_ref().map(|(k, _)| *k) != Some(key) {
            let codec: Box<dyn VoiceCodec> = match key {
                VoiceKey::Lpc10 => Box::new(hamdrm_voice::easydrf::EasyDrfLpc::new()),
                VoiceKey::Neural => {
                    if self.neural_failed {
                        return;
                    }
                    match load_neural() {
                        Ok(c) => c,
                        Err(e) => {
                            self.neural_failed = true;
                            self.log(format!("neural voice heard, but {e}"));
                            return;
                        }
                    }
                }
            };
            self.log(format!("receiving voice: {}", codec.name()));
            self.voice_view = Some(VoiceView {
                codec: codec.name().into(),
                ..Default::default()
            });
            self.rx_voice = Some((key, codec));
        }
        let bits: Vec<u8> = bytes
            .iter()
            .flat_map(|b| (0..8).rev().map(move |i| (b >> i) & 1))
            .collect();
        let (_, codec) = self.rx_voice.as_mut().expect("set above");
        let out = codec.decode(&bits);
        let view = self.voice_view.get_or_insert_with(Default::default);
        view.active = true;
        let pcm = match out {
            Some(p) => {
                view.good += 1;
                p
            }
            None => {
                view.bad += 1;
                vec![0.0; hamdrm_voice::SAMPLES_PER_FRAME]
            }
        };
        if self.speaker.is_none() && self.opts.speaker.as_deref() != Some("") {
            let o = OutputOptions {
                device: self.opts.speaker.clone(),
                sample_rate: Some(RATE),
                channels: Some(1),
                buffer: Duration::from_secs(3),
                start_threshold: Duration::from_millis(300),
            };
            match OutputStream::open(&o) {
                Ok(s) if s.format().sample_rate == RATE => self.speaker = Some(s),
                Ok(s) => self.log(format!(
                    "speaker {} runs at {} Hz; 48 kHz needed",
                    s.device_name(),
                    s.format().sample_rate
                )),
                Err(e) => self.log(format!("speaker: {e}")),
            }
        }
        if let Some(s) = &mut self.speaker {
            let _ = s.write(&pcm);
        }
    }

    fn publish(&mut self) {
        let vis = self.rx.visuals();
        let objects: Vec<ObjectView> = self
            .files
            .objects()
            .iter()
            .map(|o| {
                let n = o.total.unwrap_or(o.segments.len());
                ObjectView {
                    tid: o.tid,
                    name: o.name(),
                    received: o.received(),
                    total: o.total,
                    segments: (0..n)
                        .map(|i| o.segments.get(i).is_some_and(|s| s.is_some()))
                        .collect(),
                    done: o.delivered,
                    updated: Instant::now(),
                }
            })
            .collect();
        let mut v = self.view.lock().unwrap_or_else(|e| e.into_inner());
        v.status = self.rx.status().clone();
        v.chain = vis.chain;
        // The trace comes from the waterfall's high-resolution FFT (2.9 Hz bins).
        v.spectrum_db = self.wf.spectrum_db();
        v.spectrum_bin_hz = waterfall::BIN_HZ;
        v.signal_band_hz = vis.signal_band_hz;
        v.dc_hz = vis.dc_hz;
        for r in self.new_rows.drain(..) {
            v.waterfall.push_back(r);
            v.waterfall_count += 1;
        }
        while v.waterfall.len() > WATERFALL_ROWS {
            v.waterfall.pop_front();
        }
        v.input_db = self.input_db;
        v.objects = objects;
        v.files.clone_from(&self.rx_files);
        v.bsrs.clone_from(&self.bsrs);
        v.log.clone_from(&self.log);
        v.tx = self.job.as_ref().map(|j| TxView {
            what: j.label().to_string(),
            done_s: j.done_s(),
            total_s: j.total_s(),
        });
        v.ptt = Some(self.ptt.status());
        v.heard.clone_from(&self.heard);
        v.crc_errors = self.files.crc_errors();
        v.msc_cer = self
            .msc_cer
            .filter(|_| self.rx.status().state != RxState::Acquisition);
        v.voice.clone_from(&self.voice_view);
        v.neural_ready = neural_weights().is_some();
    }
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file")
        .to_string()
}

/// `dir/name`, or `dir/name (2).ext` … if it exists.
fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let p = dir.join(name);
    if !p.exists() {
        return p;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) => (s.to_string(), format!(".{e}")),
        None => (name.to_string(), String::new()),
    };
    (2..)
        .map(|i| dir.join(format!("{stem} ({i}){ext}")))
        .find(|p| !p.exists())
        .expect("free name")
}

/// Read a WAV as mono f32 at 48 kHz.
pub fn read_wav(path: &Path) -> Result<Vec<f32>, String> {
    let mut r = hound::WavReader::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let spec = r.spec();
    if spec.sample_rate != RATE {
        return Err(format!(
            "{}: {} Hz, 48 kHz needed",
            path.display(),
            spec.sample_rate
        ));
    }
    let ch = usize::from(spec.channels.max(1));
    let all: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => r.samples::<f32>().filter_map(Result::ok).collect(),
        hound::SampleFormat::Int => {
            let scale = 1.0 / (1u64 << (spec.bits_per_sample - 1)) as f32;
            r.samples::<i32>()
                .filter_map(Result::ok)
                .map(|v| v as f32 * scale)
                .collect()
        }
    };
    Ok(all
        .chunks(ch)
        .map(|c| c.iter().sum::<f32>() / ch as f32)
        .collect())
}

/// Local wall-clock time as HH:MM:SS (UTC; no time-zone crate).
fn chrono_hms() -> String {
    let s = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{:02}:{:02}:{:02}z", s / 3600 % 24, s / 60 % 60, s % 60)
}

pub use decdsss_audio;
pub use decdsss_rig;

/// Bytes per 400 ms MSC frame for a configuration.
pub fn frame_bytes(msc: &MscConfig) -> usize {
    let map = hamdrm_core::cellmap::CellMap::new(msc.layout);
    hamdrm_core::mlc::MlcParams::for_config(msc, map.msc_cells_per_frame).total_bits() / 8
}

/// Rough air time of a file of `size` bytes (lead-in, header, RS overhead included).
pub fn estimate_seconds(s: &TxSettings, size: usize) -> f64 {
    let pl = frame_bytes(&s.msc).saturating_sub(3);
    let seg = pl.saturating_sub(11).max(1);
    let body = match s.rs {
        0 => size,
        r => (size + 7).div_ceil(hamdrm_data::rsfile::data_len(r).unwrap_or(224)) * 255,
    };
    let frames = body.div_ceil(seg) + s.lead_in.min(body.div_ceil(seg)) + 4;
    frames as f64 * 0.4 + 0.4
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VoiceKey {
    Lpc10,
    Neural,
}

/// The neural-voice weights, if present.
pub fn neural_weights() -> Option<PathBuf> {
    #[cfg(feature = "neural")]
    {
        hamdrm_neural::find_weights()
    }
    #[cfg(not(feature = "neural"))]
    {
        None
    }
}

/// Fetch the neural-voice weights into the per-user models folder.
pub fn download_neural_weights() -> Result<PathBuf, String> {
    #[cfg(feature = "neural")]
    {
        let dir = hamdrm_neural::weight_dirs()
            .into_iter()
            .find(|d| d.ends_with("DecHAMDRM\\models") || d.ends_with("DecHAMDRM/models"))
            .unwrap_or_else(|| PathBuf::from("models"));
        hamdrm_neural::download(&dir)
    }
    #[cfg(not(feature = "neural"))]
    {
        Err("this build has no neural voice".into())
    }
}

fn load_neural() -> Result<Box<dyn VoiceCodec>, String> {
    #[cfg(feature = "neural")]
    {
        let w = hamdrm_neural::find_weights()
            .ok_or("the neural voice weights are missing (Settings → Download, or `dechamdrm models download`)")?;
        Ok(Box::new(hamdrm_neural::MimiVoice::load(&w)?))
    }
    #[cfg(not(feature = "neural"))]
    {
        Err("this build has no neural voice".into())
    }
}
