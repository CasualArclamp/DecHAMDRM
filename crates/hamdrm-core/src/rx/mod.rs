//! The HamDRM receiver chain.
//!
//! [`Receiver`] is push-driven: feed it real audio at 48 kHz (mono or interleaved
//! channels) with [`Receiver::push`] and collect [`ReceiverEvent`]s. The stages:
//!
//! ```text
//! audio ─▶ analytic signal ─▶ SRO resampler ─▶ DC search (pilots at +375/+1125/
//! +1500 Hz) / mixer ─▶ time sync (mode A/B/E, symbol timing) ─▶ OFDM ─▶ frame sync
//! (time pilots) + frequency tracking ─▶ channel estimation ─▶ FAC ─▶ MSC
//! ```
//!
//! with feedback of the frequency offset (pilots → mixer), timing (impulse response →
//! time sync) and sample-rate offset (impulse-response drift → resampler).
//!
//! Port of DecDRM's `rx` (`crates/decdrm-core/src/rx/`, GPL-3.0-or-later), itself a
//! port of Dream's `CDRMReceiver` (Volker Fischer, Alexander Kurpiers et al., GPL-2+),
//! reduced to HamDRM (`docs/spec/phy.md`): real USB audio with the DC carrier near
//! 350 Hz, robustness modes A/B/E detected from the guard interval (the mode is not
//! signalled), no SDC — the FAC alone gives bandwidth, QAM, protection and
//! interleaving — and one MSC stream of ⌊bits/8⌋ bytes per 400 ms frame.

pub mod chanest;
pub mod framesync;
pub mod freqacq;
pub mod input;
pub mod ofdm;
pub mod timesync;
mod chain;
mod mscdec;

pub use chain::{CHAN_ROWS, ChainVisuals};
pub use chanest::{PdsAxis, SNR_REF_BW_HZ};
pub use framesync::FrameSyncState;
pub use freqacq::SearchWindow;
pub use input::RealChannel;

use crate::fac::{Fac, LabelAssembler};
use crate::params::{Bandwidth, Layout, Mode, MscConfig, SAMPLE_RATE};
use crate::{Cplx, Real};
use chain::{FrameEnd, SymbolChain};
use decdrm_core::dsp::resampler::FracResampler;
use decdrm_core::fec::qam::{EqCell, MetricKind};
use freqacq::FreqAcquisition;
use input::InputConverter;
use mscdec::MscDecoder;
use std::f64::consts::PI;
use timesync::{TimeSync, TimeSyncEvent};

/// Frames of time sync without a valid FAC before restarting acquisition (DecDRM:
/// 150 symbols = 10 mode-B frames).
const MAX_FRAMES_WITHOUT_FAC: usize = 10;
/// A frequency acquisition that no FAC has confirmed yet is dropped at once when the
/// input power rises this many times (10 dB) above its level during the search: the
/// detection predates the signal (DecDRM `rx/mod.rs`).
const ACQ_POWER_JUMP: Real = 10.0;
/// The rise must last this long, s (a click is not a signal).
const ACQ_POWER_JUMP_S: Real = 0.05;
/// Largest frequency correction of the pilot loop before the first FAC, Hz (see
/// `process_window`).
const ACQ_TRACK_LIMIT_HZ: Real = 8.0;
/// Consecutive bad FACs (while locked) before restarting acquisition. A new robustness
/// mode on air (not signalled) is found this way.
const MAX_BAD_FACS_LOCKED: usize = 10;
/// Timing-loss detection from the cyclic-prefix correlation (DecDRM `TimingHealth`):
/// averaging lengths in symbols, collapse ratio, power ratio, symbols, bad FACs.
const CP_FAST_SYMBOLS: Real = 4.0;
const CP_SLOW_SYMBOLS: Real = 60.0;
const CP_LOSS_RATIO: Real = 0.35;
const CP_POWER_RATIO: Real = 0.5;
const CP_LOSS_SYMBOLS: usize = 8;
const CP_LOSS_BAD_FACS: usize = 2;
/// Good FACs before switching timing to impulse-response tracking.
const DELAYED_TRACKING_FACS: usize = 2;
/// Limit of the sample-rate offset correction, Hz (≈ 4000 ppm).
const MAX_SRO_HZ: Real = 200.0;
/// Pilot-slope SRO estimates above this fraction are applied during acquisition
/// (DecDRM: only offsets the receiver cannot lock on by itself need it).
const SRO_ACQ_THRESHOLD: Real = 600e-6;
/// Pilot-slope estimates are compared every this many symbols.
const SRO_ACQ_BLOCK: usize = 40;
/// Input spectrum for display: FFT length and the highest frequency kept, Hz.
const SPECTRUM_LEN: usize = 4096;
const SPECTRUM_MAX_HZ: Real = 4000.0;

/// The default MSC soft metric (DecDRM `MSC_METRIC`, measured there on DRM channels
/// 1–5): the Huber shape with Dream's amplitude weighting.
pub const MSC_METRIC: MetricKind = MetricKind::HuberAmplitude(0.25);

/// Receiver configuration.
#[derive(Debug, Clone)]
pub struct ReceiverConfig {
    /// Which channel of the input carries the signal.
    pub channel: RealChannel,
    /// Channels in the interleaved input (1 = mono).
    pub channels: usize,
    /// Mirror the spectrum.
    pub flip: bool,
    /// Where to look for the DC carrier (k = 0) in the audio. Default 350 ± 250 Hz.
    pub dc_search: SearchWindow,
    /// Additional MLC decoding passes for the MSC (DecDRM default 2).
    pub msc_iterations: usize,
    /// Soft metric of the FAC and MSC decoders.
    pub metric: MetricKind,
}

impl Default for ReceiverConfig {
    fn default() -> Self {
        Self {
            channel: RealChannel::Mix,
            channels: 1,
            flip: false,
            dc_search: SearchWindow::default(),
            msc_iterations: 2,
            metric: MSC_METRIC,
        }
    }
}

/// High-level receiver state for status displays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RxState {
    /// Looking for a HamDRM signal.
    #[default]
    Acquisition,
    /// First FAC decoded, synchronisation loops tightening.
    Tracking,
    /// Stable reception.
    Locked,
}

/// Snapshot of synchronisation and quality parameters.
#[derive(Debug, Clone, Default)]
pub struct RxStatus {
    pub state: RxState,
    pub mode: Option<Mode>,
    pub bandwidth: Option<Bandwidth>,
    /// The MSC configuration being decoded (from the FAC).
    pub msc: Option<MscConfig>,
    /// The station label, once all three parts were received.
    pub label: Option<String>,
    /// Audio frequency of the DC carrier (k = 0), Hz, incl. tracking. The nominal
    /// value is 350 Hz, so `dc_frequency_hz − 350` is the tuning offset.
    pub dc_frequency_hz: Option<Real>,
    /// Sample-rate offset being corrected (Hz at 48 kHz).
    pub sro_hz: Real,
    /// SNR estimate referred to [`SNR_REF_BW_HZ`] (2.5 kHz), dB.
    pub snr_db: Option<Real>,
    /// MER and weighted MER of the MSC cells over the last frame, dB.
    pub mer_db: Option<Real>,
    pub wmer_db: Option<Real>,
    pub fac_mer_db: Option<Real>,
    pub doppler_hz: Real,
    pub delay_ms: Real,
    pub fac_ok: u64,
    pub fac_bad: u64,
    /// MSC frames decoded with `trusted` set, and without.
    pub msc_ok: u64,
    pub msc_doubtful: u64,
    pub frame_sync: FrameSyncState,
}

/// One decoded MSC frame (400 ms).
#[derive(Debug, Clone)]
pub struct MscFrame {
    /// ⌊bits/8⌋ bytes (EasyDRF's streamlen; in data mode one packet of packlen + 3
    /// bytes, `docs/spec/phy.md` §5.6, §6). The trailing bits mod 8 are dropped.
    pub bytes: Vec<u8>,
    /// Decoded information bits per frame (iNumDecodedBitsMSC).
    pub bits: usize,
    /// The configuration it was decoded with.
    pub config: MscConfig,
    /// The frame's own FAC passed its CRC and describes `config`.
    pub fac_ok: bool,
    /// The cell deinterleaver had filled (always true with short interleaving).
    pub complete: bool,
    /// `fac_ok && complete`: the frame was decoded in sync with the right
    /// parameters. It says nothing about residual bit errors at low SNR — data
    /// packets carry their own CRC-16 for that.
    pub trusted: bool,
    /// Normalised final Viterbi path metric per MLC level (lower is better; scales
    /// with the input level).
    pub path_metrics: Vec<Real>,
    /// Cell error rate before FEC, judged against the re-encoded decoded bits: a
    /// level-independent quality figure (a few % near the threshold, ~50 % or more
    /// for a garbled frame).
    pub cell_error_rate: Real,
}

/// Something the receiver produced.
#[derive(Debug, Clone)]
pub enum ReceiverEvent {
    /// A HamDRM signal was found: DC carrier at `dc_hz` in the audio.
    SignalFound { dc_hz: Real },
    ModeDetected(Mode),
    /// A FAC block with a valid CRC, and the station label once assembled.
    Fac { fac: Fac, label: Option<String> },
    FacError,
    /// The MSC decoder was (re)configured from the FAC.
    Configured(MscConfig),
    /// One decoded MSC frame.
    Msc(MscFrame),
    /// Synchronisation was lost; acquisition restarted.
    Restarted,
    /// The symbol timing jumped; timing and frame sync are re-acquired, keeping the
    /// frequency, mode, sample-rate correction and MSC configuration.
    Resynchronising,
}

/// Snapshot of the receiver's plot data (see [`Receiver::visuals`]).
#[derive(Debug, Clone, Default)]
pub struct Visuals {
    /// Constellations, channel estimate, impulse response, per-carrier SNR.
    pub chain: ChainVisuals,
    /// Averaged input power spectrum in dB, bin `i` at `i · spectrum_bin_hz`, up to
    /// 4 kHz.
    pub spectrum_db: Vec<Real>,
    pub spectrum_bin_hz: Real,
    /// DC carrier in the audio, Hz.
    pub dc_hz: Option<Real>,
    /// Occupied band (lowest carrier − ½ spacing … highest + ½ spacing) in the audio,
    /// Hz, once mode and bandwidth are known.
    pub signal_band_hz: Option<(Real, Real)>,
}

/// Averaged power spectrum of the input for display (DecDRM `InputSpectrum`,
/// without the waterfall rows).
#[derive(Debug)]
struct InputSpectrum {
    fft: decdrm_core::dsp::fft::Fft,
    window: Vec<Real>,
    buf: Vec<Cplx>,
    avg: Vec<Real>,
    count: u64,
}

impl InputSpectrum {
    fn new() -> Self {
        let keep = (SPECTRUM_MAX_HZ / Self::bin_hz()) as usize + 1;
        Self {
            fft: decdrm_core::dsp::fft::Fft::new(SPECTRUM_LEN),
            window: decdrm_core::dsp::hamming(SPECTRUM_LEN),
            buf: Vec::with_capacity(SPECTRUM_LEN),
            avg: vec![0.0; keep],
            count: 0,
        }
    }

    fn bin_hz() -> Real {
        Real::from(SAMPLE_RATE) / SPECTRUM_LEN as Real
    }

    fn push(&mut self, samples: &[Cplx]) {
        for &s in samples {
            self.buf.push(s);
            if self.buf.len() == SPECTRUM_LEN {
                let mut work: Vec<Cplx> = self.buf.iter().zip(&self.window).map(|(s, w)| s * *w).collect();
                self.fft.forward(&mut work);
                let lambda = if self.count < 8 { 0.5 } else { 0.9 };
                let n2 = (SPECTRUM_LEN * SPECTRUM_LEN) as Real;
                for (a, v) in self.avg.iter_mut().zip(&work) {
                    *a = lambda * *a + (1.0 - lambda) * v.norm_sqr() / n2;
                }
                self.count += 1;
                self.buf.clear();
            }
        }
    }

    fn db(&self) -> Vec<Real> {
        self.avg.iter().map(|p| 10.0 * p.max(1e-20).log10()).collect()
    }
}

pub struct Receiver {
    cfg: ReceiverConfig,
    input: InputConverter,
    conv: Vec<Cplx>,
    resampler: FracResampler,
    sro_hz: Real,
    res: Vec<Cplx>,
    acq: Option<FreqAcquisition>,
    acq_power: Option<Real>,
    loud_samples: usize,
    pending: Vec<Cplx>,
    dc_hz: Real,
    track_hz: Real,
    nco_phase: Real,
    mixed: Vec<Cplx>,
    timesync: TimeSync,
    chain: Option<SymbolChain>,
    mode: Mode,
    state: RxState,
    symbols_without_fac: usize,
    bad_facs: usize,
    good_facs: usize,
    delayed_cnt: usize,
    delayed_done: bool,
    cp: TimingHealth,
    sro_reports: usize,
    sro_prev: Option<Real>,
    /// MSC decoder for the confirmed FAC configuration.
    msc: Option<MscDecoder>,
    /// A frame was skipped since the decoder's last frame (restart its deinterleaver).
    msc_gap: bool,
    /// A new configuration a FAC brought, waiting for the next FAC to confirm it, and
    /// that frame's cells (decoded once the configuration is settled).
    pending_cfg: Option<MscConfig>,
    pending_frame: Option<(Vec<EqCell>, bool)>,
    labels: LabelAssembler,
    spectrum: InputSpectrum,
    status: RxStatus,
    events: Vec<ReceiverEvent>,
}

impl Receiver {
    pub fn new(cfg: ReceiverConfig) -> Self {
        let mode = Mode::B;
        Self {
            input: InputConverter::new(cfg.channel, cfg.channels, cfg.flip),
            conv: Vec::new(),
            resampler: FracResampler::new(),
            sro_hz: 0.0,
            res: Vec::new(),
            acq: Some(FreqAcquisition::new(cfg.dc_search)),
            acq_power: None,
            loud_samples: 0,
            pending: Vec::new(),
            dc_hz: 0.0,
            track_hz: 0.0,
            nco_phase: 0.0,
            mixed: Vec::new(),
            timesync: TimeSync::new(mode),
            chain: None,
            mode,
            state: RxState::Acquisition,
            symbols_without_fac: 0,
            bad_facs: 0,
            good_facs: 0,
            delayed_cnt: DELAYED_TRACKING_FACS,
            delayed_done: false,
            cp: TimingHealth::default(),
            sro_reports: 0,
            sro_prev: None,
            msc: None,
            msc_gap: true,
            pending_cfg: None,
            pending_frame: None,
            labels: LabelAssembler::default(),
            spectrum: InputSpectrum::new(),
            status: RxStatus::default(),
            events: Vec::new(),
            cfg,
        }
    }

    pub fn config(&self) -> &ReceiverConfig {
        &self.cfg
    }

    pub fn status(&self) -> &RxStatus {
        &self.status
    }

    /// Plot data: constellations, channel, impulse response, per-carrier SNR and the
    /// input spectrum.
    pub fn visuals(&self) -> Visuals {
        let dc_hz = self.status.dc_frequency_hz;
        let signal_band_hz = match (dc_hz, self.status.mode, self.status.bandwidth) {
            (Some(dc), Some(mode), Some(bw)) => {
                let (kmin, kmax) = Layout::new(mode, bw).carrier_range();
                let df = mode.carrier_spacing();
                Some((dc + Real::from(kmin) * df - df / 2.0, dc + Real::from(kmax) * df + df / 2.0))
            }
            _ => None,
        };
        Visuals {
            chain: self.chain.as_ref().map(|c| c.visuals()).unwrap_or_default(),
            spectrum_db: self.spectrum.db(),
            spectrum_bin_hz: InputSpectrum::bin_hz(),
            dc_hz,
            signal_band_hz,
        }
    }

    /// The latest frame's worth of equalised MSC cells (the MSC constellation).
    pub fn msc_constellation(&self) -> Vec<Cplx> {
        self.chain.as_ref().map(|c| c.msc_constellation()).unwrap_or_default()
    }

    /// Latest robustness-mode detection scores (A, B, E), for diagnostics.
    pub fn mode_scores(&self) -> [Real; 3] {
        self.timesync.last_mode_scores
    }

    /// Restart acquisition from scratch (e.g. after retuning).
    pub fn restart(&mut self) {
        self.acq = Some(FreqAcquisition::new(self.cfg.dc_search));
        self.acq_power = None;
        self.loud_samples = 0;
        self.pending.clear();
        self.track_hz = 0.0;
        self.mode = Mode::B;
        self.timesync.restart(self.mode);
        self.chain = None;
        self.state = RxState::Acquisition;
        self.symbols_without_fac = 0;
        self.bad_facs = 0;
        self.good_facs = 0;
        self.delayed_cnt = DELAYED_TRACKING_FACS;
        self.delayed_done = false;
        self.cp = TimingHealth::default();
        // The mode may differ after the restart, and the FAC gives the rest.
        self.msc = None;
        self.msc_gap = true;
        self.pending_cfg = None;
        self.pending_frame = None;
        self.labels = LabelAssembler::default();
        let s = &self.status;
        let (ok, bad, mok, mbad) = (s.fac_ok, s.fac_bad, s.msc_ok, s.msc_doubtful);
        self.status = RxStatus { fac_ok: ok, fac_bad: bad, msc_ok: mok, msc_doubtful: mbad, sro_hz: self.sro_hz, ..Default::default() };
    }

    /// Change the sample-rate correction. Frequencies at the resampler output scale
    /// with (fs − sro), so the mixer frequency is rescaled to stay on the signal.
    fn set_sro(&mut self, new_sro: Real) {
        let fs = Real::from(SAMPLE_RATE);
        let new_sro = new_sro.clamp(-MAX_SRO_HZ, MAX_SRO_HZ);
        let scale = (fs - new_sro) / (fs - self.sro_hz);
        self.dc_hz *= scale;
        self.track_hz *= scale;
        self.sro_hz = new_sro;
    }

    /// Feed interleaved audio samples (any number of frames) at 48 kHz and return the
    /// events produced.
    pub fn push(&mut self, interleaved: &[f32]) -> Vec<ReceiverEvent> {
        let ch = self.cfg.channels.max(1);
        // Small chunks keep the feedback loops responsive.
        for chunk in interleaved.chunks(512 * ch) {
            self.push_chunk(chunk);
        }
        std::mem::take(&mut self.events)
    }

    fn push_chunk(&mut self, chunk: &[f32]) {
        self.conv.clear();
        self.input.process(chunk, &mut self.conv);
        self.spectrum.push(&self.conv);
        self.res.clear();
        let fs = Real::from(SAMPLE_RATE);
        let sro = self.sro_hz.clamp(-MAX_SRO_HZ, MAX_SRO_HZ);
        self.resampler.process(&self.conv, fs / (fs - sro), &mut self.res);
        let samples = std::mem::take(&mut self.res);
        if let Some(p0) = self.acq_power
            && !samples.is_empty()
        {
            let p = samples.iter().map(|s| s.norm_sqr()).sum::<Real>() / samples.len() as Real;
            self.loud_samples = if p > ACQ_POWER_JUMP * p0 { self.loud_samples + samples.len() } else { 0 };
        }

        if let Some(acq) = self.acq.as_mut() {
            self.pending.extend_from_slice(&samples);
            // Keep only what the frequency search looked at, so no pre-signal data
            // reaches time sync.
            if self.pending.len() > freqacq::ANALYSIS_SPAN {
                let d = self.pending.len() - freqacq::ANALYSIS_SPAN;
                self.pending.drain(..d);
            }
            if let Some(a) = acq.push(&samples) {
                self.acq = None;
                self.dc_hz = a.dc_hz;
                self.track_hz = 0.0;
                self.events.push(ReceiverEvent::SignalFound { dc_hz: a.dc_hz });
                self.acq_power = Some(a.power);
                self.loud_samples = 0;
                let pending = std::mem::take(&mut self.pending);
                self.feed_baseband(&pending);
            }
        } else {
            self.feed_baseband(&samples);
        }
        if self.acq_power.is_some() && self.loud_samples as Real >= ACQ_POWER_JUMP_S * fs {
            self.restart();
            self.events.push(ReceiverEvent::Restarted);
        }
        self.res = samples;
        self.status.sro_hz = self.sro_hz;
        self.status.dc_frequency_hz = self.acq.is_none().then_some(self.dc_hz + self.track_hz);
    }

    /// Mix the DC carrier to 0 Hz and run time sync and the symbol chain.
    fn feed_baseband(&mut self, samples: &[Cplx]) {
        // Process in pieces so frequency corrections take effect quickly.
        for piece in samples.chunks(256) {
            self.mixed.clear();
            let w = 2.0 * PI * (self.dc_hz + self.track_hz) / Real::from(SAMPLE_RATE);
            for &s in piece {
                self.mixed.push(s * Cplx::from_polar(1.0, -self.nco_phase));
                self.nco_phase += w;
                if self.nco_phase > PI {
                    self.nco_phase -= 2.0 * PI;
                }
            }
            let mixed = std::mem::take(&mut self.mixed);
            for ev in self.timesync.push(&mixed) {
                let TimeSyncEvent::ModeDetected { mode, .. } = ev;
                self.events.push(ReceiverEvent::ModeDetected(mode));
                if mode != self.mode || self.chain.is_none() {
                    self.mode = mode;
                    self.timesync.configure(mode);
                    self.chain = None;
                }
                self.symbols_without_fac = 0;
            }
            self.mixed = mixed;
            while let Some(win) = self.timesync.next_window() {
                self.process_window(win);
                if self.acq.is_some() {
                    // A restart happened inside; drop the rest of this block.
                    return;
                }
            }
            self.timesync.trim_unsynchronised();
        }
    }

    fn process_window(&mut self, win: timesync::SymbolWindow) {
        if self.chain.is_none() {
            // Start with the wide layout (a superset of the narrow one's carriers,
            // same Kmin) unless the FAC already told us; the FAC corrects it.
            let bw = self.msc.as_ref().map(|m| m.config().layout).filter(|l| l.mode == self.mode).map_or(Bandwidth::Wide, |l| l.bandwidth);
            let mut chain = SymbolChain::new(Layout::new(self.mode, bw), &self.cfg);
            chain.set_msc_mapping(self.msc.as_ref().map(|m| m.config().qam.mapping()));
            self.chain = Some(chain);
            self.msc_gap = true;
            self.sro_reports = 0;
            self.sro_prev = None;
        }
        if let Some(rho) = win.guard_corr {
            self.cp.update(rho, win.power, self.state == RxState::Locked && self.bad_facs == 0);
        }
        // `let … else`: bind the chain or leave the function.
        let Some(chain) = self.chain.as_mut() else { return };
        let out = chain.process(&win);
        if out.alignment_lost && self.state == RxState::Locked && self.bad_facs >= 1 {
            self.resync();
            self.events.push(ReceiverEvent::Resynchronising);
            return;
        }
        self.track_hz += out.freq_delta_hz;
        if self.state == RxState::Acquisition {
            // The DC search is accurate to ±3.9 Hz. Before the first FAC the pilot
            // loop may see noise or the leading edge of a burst (HamDRM transmissions
            // start abruptly), and a free loop then walks off by multiples of 1/Ts
            // (37.5 Hz in mode B), where it locks on wrongly.
            self.track_hz = self.track_hz.clamp(-ACQ_TRACK_LIMIT_HZ, ACQ_TRACK_LIMIT_HZ);
        }
        if self.delayed_done && out.timing_adjust != 0 {
            self.timesync.adjust(out.timing_adjust);
        }
        // Coarse SRO acquisition from the pilot slope, only while searching.
        let mut sro_delta = out.sro_delta_hz;
        if self.state == RxState::Acquisition
            && let Some(eps) = out.sro_estimate
        {
            self.sro_reports += 1;
            if self.sro_reports.is_multiple_of(SRO_ACQ_BLOCK) {
                let prev = self.sro_prev.replace(eps);
                if let Some(prev) = prev
                    && prev.abs() > SRO_ACQ_THRESHOLD
                    && eps.abs() > SRO_ACQ_THRESHOLD
                    && (prev - eps).abs() < 0.25 * prev.abs().max(eps.abs())
                {
                    sro_delta += 0.5 * (prev + eps) * Real::from(SAMPLE_RATE);
                    chain.reset_sro_estimate();
                    self.sro_reports = 0;
                    self.sro_prev = None;
                }
            }
        }
        let st = chain.stats();
        self.status.snr_db = st.snr_db;
        self.status.mer_db = st.mer_db;
        self.status.wmer_db = st.wmer_db;
        self.status.fac_mer_db = st.fac_mer_db;
        self.status.doppler_hz = st.doppler_hz;
        self.status.delay_ms = st.delay_ms;
        self.status.frame_sync = chain.frame_sync_state();
        self.status.mode = Some(self.mode);
        self.status.bandwidth = Some(chain.bandwidth());
        self.status.state = self.state;
        if sro_delta != 0.0 {
            self.set_sro(self.sro_hz + sro_delta);
        }
        if let Some(fe) = out.frame_end {
            self.on_frame_end(fe);
        }
        if self.state == RxState::Acquisition && self.acq.is_none() {
            self.symbols_without_fac += 1;
            if self.symbols_without_fac > MAX_FRAMES_WITHOUT_FAC * self.mode.symbols_per_frame() {
                self.restart();
                self.events.push(ReceiverEvent::Restarted);
            }
        }
    }

    /// A transmission frame is complete: its FAC decides the MSC configuration, then
    /// its MSC cells are decoded.
    ///
    /// A FAC that changes the configuration is acted on only when the next FAC
    /// confirms it (an 8-bit CRC lets one corrupted block in 256 through; acting on
    /// it would reset the long deinterleaver, DecDRM `fac_confirmed`). Its frame's
    /// cells are kept meanwhile and decoded with whatever configuration wins, so a
    /// genuine change loses no frame. The first FAC after acquisition counts at once.
    fn on_frame_end(&mut self, fe: FrameEnd) {
        let FrameEnd { fac, msc, gap } = fe;
        let fac_cfg = fac.as_ref().map(|f| f.msc_config(self.mode));
        match fac {
            Some(f) => {
                self.status.fac_ok += 1;
                self.on_good_fac();
                self.labels.push(&f);
                let label = self.labels.label();
                if label.is_some() {
                    self.status.label.clone_from(&label);
                }
                self.events.push(ReceiverEvent::Fac { fac: f, label });
            }
            None => {
                self.status.fac_bad += 1;
                self.events.push(ReceiverEvent::FacError);
                self.on_bad_fac();
            }
        }
        if self.acq.is_some() || self.chain.is_none() {
            // `on_bad_fac` restarted the receiver.
            return;
        }
        let current = self.msc.as_ref().map(|m| m.config());
        let adopt = match (current, fac_cfg) {
            (_, None) => None,
            (None, Some(c)) => Some(c),
            (Some(cur), Some(c)) if cur == c => None,
            (Some(_), Some(c)) if self.pending_cfg == Some(c) => Some(c),
            (Some(_), Some(c)) => {
                // An unconfirmed change: settle an older pending frame, keep this one.
                self.flush_pending();
                self.pending_cfg = Some(c);
                self.pending_frame = msc.map(|cells| (cells, gap));
                return;
            }
        };
        if let Some(c) = adopt {
            self.configure(c);
        }
        self.flush_pending();
        match msc {
            Some(cells) => self.decode_frame(&cells, gap, fac_cfg),
            None => self.msc_gap = true,
        }
    }

    /// Decode the frame kept by an unconfirmed configuration change (with the
    /// configuration now in force) and forget the pending change.
    fn flush_pending(&mut self) {
        let cfg = self.pending_cfg.take();
        if let Some((cells, gap)) = self.pending_frame.take() {
            self.decode_frame(&cells, gap, cfg);
        }
    }

    /// Set up the MSC decoder (and, for a new bandwidth, the carrier layout).
    fn configure(&mut self, c: MscConfig) {
        let Some(chain) = self.chain.as_mut() else { return };
        chain.set_bandwidth(c.layout.bandwidth);
        chain.set_msc_mapping(Some(c.qam.mapping()));
        let cells = chain.msc_cells_per_frame();
        self.msc = Some(MscDecoder::new(c, cells, self.cfg.msc_iterations, self.cfg.metric));
        self.msc_gap = true;
        self.status.msc = Some(c);
        self.status.bandwidth = Some(c.layout.bandwidth);
        self.events.push(ReceiverEvent::Configured(c));
    }

    /// Decode one frame's MSC cells; `fac_cfg` is what the frame's own FAC said.
    fn decode_frame(&mut self, cells: &[EqCell], gap: bool, fac_cfg: Option<MscConfig>) {
        let Some(dec) = self.msc.as_mut() else { return };
        if cells.len() != dec.cells() {
            // Collected with another carrier layout (bandwidth just changed).
            self.msc_gap = true;
            return;
        }
        let gap = gap || std::mem::replace(&mut self.msc_gap, false);
        let Some(d) = dec.decode(cells, gap) else { return };
        let config = dec.config();
        let nbits = d.bits.len();
        // Pack MSB first, dropping the trailing bits mod 8. `as_chunks::<8>()` splits
        // the slice into fixed `[u8; 8]` arrays (`.0`) and a remainder (`.1`, dropped).
        let bytes: Vec<u8> = d.bits.as_chunks::<8>().0.iter().map(|b| b.iter().fold(0u8, |acc, &x| (acc << 1) | (x & 1))).collect();
        let fac_ok = fac_cfg == Some(config);
        let trusted = fac_ok && d.complete;
        if trusted {
            self.status.msc_ok += 1;
        } else {
            self.status.msc_doubtful += 1;
        }
        self.events.push(ReceiverEvent::Msc(MscFrame {
            bytes,
            bits: nbits,
            config,
            fac_ok,
            complete: d.complete,
            trusted,
            path_metrics: d.path_metrics,
            cell_error_rate: d.cell_error_rate,
        }));
    }

    fn on_good_fac(&mut self) {
        self.bad_facs = 0;
        self.acq_power = None;
        if self.state == RxState::Acquisition {
            self.state = RxState::Tracking;
            self.symbols_without_fac = 0;
            self.timesync.stop_mode_detection();
            if let Some(c) = self.chain.as_mut() {
                c.enter_tracking();
            }
            self.good_facs = 1;
            return;
        }
        self.good_facs += 1;
        if self.good_facs >= 2 {
            self.state = RxState::Locked;
            if !self.delayed_done {
                if self.delayed_cnt > 0 {
                    self.delayed_cnt -= 1;
                } else {
                    self.delayed_done = true;
                    self.timesync.stop_timing_acquisition();
                    if let Some(c) = self.chain.as_mut() {
                        c.enter_timing_tracking();
                    }
                }
            }
        }
    }

    fn on_bad_fac(&mut self) {
        self.good_facs = 0;
        self.bad_facs += 1;
        if self.state != RxState::Locked {
            return;
        }
        if self.bad_facs >= CP_LOSS_BAD_FACS && self.cp.lost_symbols >= CP_LOSS_SYMBOLS {
            self.resync();
            self.events.push(ReceiverEvent::Resynchronising);
        } else if self.bad_facs > MAX_BAD_FACS_LOCKED {
            self.restart();
            self.events.push(ReceiverEvent::Restarted);
        }
    }

    /// Re-acquire symbol timing and frame sync after a timing jump, keeping the
    /// frequency, mode, sample-rate correction and MSC configuration.
    fn resync(&mut self) {
        self.timesync.configure(self.mode);
        self.chain = None;
        self.state = RxState::Acquisition;
        self.symbols_without_fac = 0;
        self.bad_facs = 0;
        self.good_facs = 0;
        self.delayed_cnt = DELAYED_TRACKING_FACS;
        self.delayed_done = false;
        self.cp = TimingHealth::default();
        self.msc_gap = true;
        self.status.state = RxState::Acquisition;
    }
}

/// Cyclic-prefix correlation and window power, averaged fast and slow (the slow
/// reference only while reception is good), for detecting timing jumps (DecDRM).
#[derive(Debug, Clone, Default)]
struct TimingHealth {
    fast: Real,
    slow: Real,
    power_fast: Real,
    power_slow: Real,
    ref_symbols: usize,
    /// Consecutive symbols with a collapsed correlation at normal power.
    lost_symbols: usize,
}

impl TimingHealth {
    fn update(&mut self, rho: Real, power: Real, good: bool) {
        let (af, as_) = (1.0 / CP_FAST_SYMBOLS, 1.0 / CP_SLOW_SYMBOLS);
        if self.ref_symbols == 0 && self.fast == 0.0 {
            self.fast = rho;
            self.power_fast = power;
        }
        self.fast += af * (rho - self.fast);
        self.power_fast += af * (power - self.power_fast);
        if good && (self.ref_symbols == 0 || self.fast > 0.7 * self.slow) {
            if self.ref_symbols == 0 {
                self.slow = rho;
                self.power_slow = power;
            }
            self.slow += as_ * (rho - self.slow);
            self.power_slow += as_ * (power - self.power_slow);
            self.ref_symbols += 1;
        }
        let lost = self.ref_symbols >= CP_SLOW_SYMBOLS as usize
            && self.fast < CP_LOSS_RATIO * self.slow
            && self.power_fast > CP_POWER_RATIO * self.power_slow;
        self.lost_symbols = if lost { self.lost_symbols + 1 } else { 0 };
    }
}
