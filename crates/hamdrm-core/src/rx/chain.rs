//! Per-layout symbol processing: OFDM demodulation, frame sync, channel estimation,
//! cell demapping and FAC decoding. Rebuilt when the robustness mode changes; a new
//! spectrum occupancy from the FAC swaps the carrier layout in place.
//!
//! Port of DecDRM's `rx/chain.rs` reduced to HamDRM (`docs/spec/phy.md` §3, §6): no
//! SDC, and every transmission frame carries exactly one MSC frame (N_MUX cells in
//! (symbol, carrier) order, no dummy cells), so a frame's MSC cells are complete at
//! its last symbol — the same symbol that completes its FAC in every mode. The MSC
//! decoding itself happens in the receiver, which knows the FAC configuration.

use super::ReceiverConfig;
use super::chanest::{ChanStats, ChannelEstimator, PdsAxis};
use super::framesync::{FrameSync, FrameSyncState};
use super::ofdm::OfdmDemod;
use super::timesync::SymbolWindow;
use crate::cellmap::CellMap;
use crate::fac::Fac;
use crate::mlc::{MlcDecoder, MlcParams};
use crate::params::{Bandwidth, Layout, Mode, SAMPLE_RATE};
use crate::tables::NUM_FAC_CELLS;
use crate::{Cplx, Real};
use decdrm_core::fec::qam::{EqCell, Mapping};
use std::collections::VecDeque;
use std::sync::Arc;

/// Plot data captured from the symbol chain (cheap to clone on demand).
#[derive(Debug, Clone, Default)]
pub struct ChainVisuals {
    /// The latest equalised FAC and MSC cells (a frame's worth each, updated symbol
    /// by symbol): the constellation plots.
    pub fac: Vec<Cplx>,
    pub msc: Vec<Cplx>,
    /// Carrier index of the first entry of `chan`.
    pub kmin: i32,
    /// Latest channel estimate per carrier (k = kmin + i).
    pub chan: Vec<Cplx>,
    /// Channel power per carrier of the latest symbols, dB (oldest first, at most
    /// [`CHAN_ROWS`]).
    pub chan_rows: Vec<Vec<f32>>,
    /// Symbols estimated so far: the last of `chan_rows` is symbol `chan_seq − 1`.
    pub chan_seq: u64,
    /// Duration of an OFDM symbol, seconds (the time step of `chan_rows`).
    pub symbol_s: Real,
    /// Averaged power delay profile (linear), value `i` at
    /// `pds_axis.start_ms + i · pds_axis.step_ms`.
    pub pds: Vec<Real>,
    pub pds_axis: Option<PdsAxis>,
    /// Per-carrier MSC SNR (carrier index, dB).
    pub snr_profile: Vec<(i32, Real)>,
    /// Carrier spacing, Hz.
    pub spacing_hz: Real,
}

/// Channel rows kept for the displays ([`ChainVisuals::chan_rows`]).
pub const CHAN_ROWS: usize = 64;

/// The end of one transmission frame.
pub(super) struct FrameEnd {
    /// Its FAC, `None` when the CRC failed.
    pub fac: Option<Fac>,
    /// Its MSC cells, when collected from symbol 0 on.
    pub msc: Option<Vec<EqCell>>,
    /// Frames were lost (or the alignment changed) before this one.
    pub gap: bool,
}

pub(super) struct ChainOutput {
    pub freq_delta_hz: Real,
    pub timing_adjust: i64,
    pub sro_delta_hz: Real,
    /// Coarse SRO estimate from the pilot phase slope (fraction).
    pub sro_estimate: Option<Real>,
    /// The frame alignment was lost while tracking (see `FrameSyncOutput`).
    pub alignment_lost: bool,
    pub frame_end: Option<FrameEnd>,
}

pub(super) struct SymbolChain {
    map: Arc<CellMap>,
    ofdm: OfdmDemod,
    frame: FrameSync,
    chanest: ChannelEstimator,
    cells: Vec<Cplx>,
    /// The most recent windows given to the channel estimator (samples, timing
    /// shift, frame symbol index), replayed through a new carrier layout when the
    /// bandwidth changes so the estimator's delay line is not lost.
    recent: VecDeque<(Vec<Cplx>, i64, usize)>,
    fac_cells: Vec<EqCell>,
    last_fac_symbol: usize,
    fac_dec: MlcDecoder,
    msc_cells: Vec<EqCell>,
    /// Collecting a frame's MSC cells that started at its symbol 0.
    msc_collecting: bool,
    msc_gap: bool,
    tracking: bool,
    timing_tracking: bool,
    vis: ChainVisuals,
    vis_fac: VecDeque<Cplx>,
    vis_msc: VecDeque<Cplx>,
    chan_rows: VecDeque<Vec<f32>>,
    chan_seq: u64,
}

impl SymbolChain {
    pub fn new(layout: Layout, cfg: &ReceiverConfig) -> Self {
        let map = Arc::new(CellMap::new(layout));
        let last_fac_symbol = crate::tables::fac_positions(layout.mode)
            .iter()
            .map(|&(s, _)| s as usize)
            .max()
            .unwrap_or(0);
        let mut fac_dec = MlcDecoder::new(MlcParams::fac(), 0);
        fac_dec.metric = cfg.metric;
        Self {
            ofdm: OfdmDemod::new(&map),
            frame: FrameSync::new(&map),
            chanest: ChannelEstimator::new(Arc::clone(&map)),
            cells: Vec::new(),
            recent: VecDeque::new(),
            fac_cells: Vec::with_capacity(NUM_FAC_CELLS),
            last_fac_symbol,
            fac_dec,
            msc_cells: Vec::new(),
            msc_collecting: false,
            msc_gap: true,
            tracking: false,
            timing_tracking: false,
            vis: ChainVisuals::default(),
            vis_fac: VecDeque::new(),
            vis_msc: VecDeque::new(),
            chan_rows: VecDeque::new(),
            chan_seq: 0,
            map,
        }
    }

    pub fn mode(&self) -> Mode {
        self.map.mode()
    }

    pub fn bandwidth(&self) -> Bandwidth {
        self.map.layout.bandwidth
    }

    /// MSC cells per frame of the current layout.
    pub fn msc_cells_per_frame(&self) -> usize {
        self.map.msc_cells_per_frame
    }

    pub fn stats(&self) -> ChanStats {
        self.chanest.stats
    }

    pub fn frame_sync_state(&self) -> FrameSyncState {
        self.frame.state
    }

    /// The MSC constellation, for the MER/WMER measurement.
    pub fn set_msc_mapping(&mut self, mapping: Option<Mapping>) {
        self.chanest.msc_mapping = mapping;
    }

    pub fn enter_tracking(&mut self) {
        self.tracking = true;
        self.chanest.start_time_wiener_tracking();
        self.frame.stop_acquisition();
        self.frame.set_freq_time_constant(1.0);
    }

    /// Called after a coarse SRO correction: restart both SRO estimators.
    pub fn reset_sro_estimate(&mut self) {
        self.frame.reset_sro_estimate();
        self.chanest.reset_sro_tracking();
    }

    pub fn enter_timing_tracking(&mut self) {
        self.timing_tracking = true;
        self.chanest.start_timing_tracking();
    }

    /// Switch to another spectrum occupancy (same mode, same Kmin), keeping frame
    /// sync and replaying the recent windows through the new layout (DecDRM
    /// `SymbolChain::reconfigure`).
    pub fn set_bandwidth(&mut self, bandwidth: Bandwidth) {
        if bandwidth == self.bandwidth() {
            return;
        }
        let map = Arc::new(CellMap::new(Layout::new(self.mode(), bandwidth)));
        self.ofdm = OfdmDemod::new(&map);
        self.frame.reconfigure(&map);
        let mapping = self.chanest.msc_mapping;
        self.chanest = ChannelEstimator::new(Arc::clone(&map));
        self.chanest.msc_mapping = mapping;
        if self.tracking {
            self.chanest.start_time_wiener_tracking();
        }
        // The new estimator continues with the symbol after the last one the old one
        // delivered (its outputs during the replay were already demapped).
        let mut cells = Vec::new();
        for (samples, shift, symbol) in &self.recent {
            self.ofdm.demodulate(samples, &mut cells);
            let _ = self.chanest.process(&cells, *symbol, *shift);
        }
        track_reset(&mut self.chanest);
        if self.timing_tracking {
            self.chanest.start_timing_tracking();
        }
        self.map = map;
        self.msc_cells.clear();
        self.msc_collecting = false;
        self.msc_gap = true;
    }

    pub fn process(&mut self, win: &SymbolWindow) -> ChainOutput {
        let mut out = ChainOutput {
            freq_delta_hz: 0.0,
            timing_adjust: 0,
            sro_delta_hz: 0.0,
            sro_estimate: None,
            alignment_lost: false,
            frame_end: None,
        };
        // `mem::take` moves the scratch buffer out of `self` (leaving an empty Vec)
        // so `self` can be borrowed again below; it is put back afterwards.
        let mut cells = std::mem::take(&mut self.cells);
        self.ofdm.demodulate(&win.samples, &mut cells);
        let fs = self.frame.process(&cells, win.shift);
        out.sro_estimate = fs.sro_estimate;
        out.alignment_lost = fs.alignment_lost;
        // Track frequency from the start: the pilots are there in every symbol.
        if !self.frame.track_freq {
            self.frame.track_freq = true;
            self.frame.set_freq_time_constant(0.1);
        }
        out.freq_delta_hz = fs.freq_delta_hz;
        if !fs.ready {
            self.cells = cells;
            return out;
        }
        if fs.id_changed {
            // Frame alignment changed: channel history and demapper are invalid.
            let mapping = self.chanest.msc_mapping;
            self.chanest = ChannelEstimator::new(Arc::clone(&self.map));
            self.chanest.msc_mapping = mapping;
            if self.tracking {
                self.chanest.start_time_wiener_tracking();
            }
            if self.timing_tracking {
                self.chanest.start_timing_tracking();
            }
            self.fac_cells.clear();
            self.msc_cells.clear();
            self.msc_collecting = false;
            self.msc_gap = true;
            self.recent.clear();
        }
        let eq = self.chanest.process(&cells, fs.symbol, win.shift);
        self.cells = cells;
        self.remember(win, fs.symbol);
        let track = self.chanest.last_track;
        if self.chanest.timing_tracking() {
            out.timing_adjust = track.timing_adjust;
        }
        out.sro_delta_hz = track.sro_delta_hz;
        track_reset(&mut self.chanest);

        let Some(eq) = eq else { return out };
        self.capture(&eq.cells, eq.symbol, &eq.chan);
        out.frame_end = self.demap(&eq.cells, eq.symbol);
        out
    }

    /// Keep the window just given to the channel estimator for a possible replay.
    fn remember(&mut self, win: &SymbolWindow, symbol: usize) {
        let keep = 2 * self.chanest.delay() + 1;
        let mut buf = if self.recent.len() >= keep {
            self.recent
                .pop_front()
                .map(|(b, _, _)| b)
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        buf.clear();
        buf.extend_from_slice(&win.samples);
        self.recent.push_back((buf, win.shift, symbol));
    }

    /// Keep the latest cells for the constellation plots and the channel power rows.
    fn capture(&mut self, cells: &[EqCell], s: usize, chan: &[Cplx]) {
        let map = &self.map;
        for &c in map.fac_carriers(s) {
            self.vis_fac.push_back(cells[c as usize].sig);
        }
        for &c in map.msc_carriers(s) {
            self.vis_msc.push_back(cells[c as usize].sig);
        }
        // Three frames of FAC cells: 45 per frame is too few for a constellation.
        keep_last(&mut self.vis_fac, 3 * NUM_FAC_CELLS);
        keep_last(&mut self.vis_msc, map.msc_cells_per_frame);
        let mut row = if self.chan_rows.len() >= CHAN_ROWS {
            self.chan_rows.pop_front().unwrap_or_default()
        } else {
            Vec::new()
        };
        row.clear();
        row.extend(
            chan.iter()
                .map(|h| (10.0 * h.norm_sqr().max(1e-12).log10()) as f32),
        );
        self.chan_rows.push_back(row);
        self.chan_seq += 1;
        self.vis.kmin = map.kmin;
        self.vis.spacing_hz = map.mode().carrier_spacing();
        self.vis.chan.clear();
        self.vis.chan.extend_from_slice(chan);
    }

    /// The latest frame's worth of equalised MSC cells.
    pub fn msc_constellation(&self) -> Vec<Cplx> {
        self.vis_msc.iter().copied().collect()
    }

    pub fn visuals(&self) -> ChainVisuals {
        let mut v = self.vis.clone();
        v.fac = self.vis_fac.iter().copied().collect();
        v.msc = self.vis_msc.iter().copied().collect();
        v.chan_rows = self.chan_rows.iter().cloned().collect();
        v.chan_seq = self.chan_seq;
        v.symbol_s = self.map.mode().symbol_len() as Real / Real::from(SAMPLE_RATE);
        let (pds, axis) = self.chanest.power_delay_profile();
        v.pds = pds;
        v.pds_axis = Some(axis);
        v.snr_profile = self.chanest.snr_profile();
        v
    }

    /// Gather FAC and MSC cells of frame symbol `s`; at the frame's last symbol
    /// decode the FAC and hand the frame out.
    fn demap(&mut self, cells: &[EqCell], s: usize) -> Option<FrameEnd> {
        let map = Arc::clone(&self.map);
        if s == 0 {
            self.fac_cells.clear();
            self.msc_cells.clear();
            self.msc_collecting = true;
        }
        for &c in map.fac_carriers(s) {
            self.fac_cells.push(cells[c as usize]);
        }
        if self.msc_collecting {
            for &c in map.msc_carriers(s) {
                self.msc_cells.push(cells[c as usize]);
            }
        }
        if s != map.symbols_per_frame - 1 {
            return None;
        }
        // The last FAC cell is always in the frame's last symbol (phy.md §3.1).
        debug_assert_eq!(self.last_fac_symbol, map.symbols_per_frame - 1);
        if self.fac_cells.len() != NUM_FAC_CELLS {
            // A partial first frame (sync found mid-frame): nothing to decode.
            self.fac_cells.clear();
            self.msc_collecting = false;
            self.msc_gap = true;
            return None;
        }
        let mut bits = Vec::new();
        self.fac_dec.decode(&self.fac_cells, &mut bits);
        let fac = Fac::decode(&bits);
        self.fac_cells.clear();
        let msc = (self.msc_collecting && self.msc_cells.len() == map.msc_cells_per_frame)
            .then(|| std::mem::take(&mut self.msc_cells));
        let gap = if msc.is_some() {
            std::mem::replace(&mut self.msc_gap, false)
        } else {
            self.msc_gap = true;
            true
        };
        self.msc_collecting = false;
        Some(FrameEnd { fac, msc, gap })
    }
}

fn track_reset(chanest: &mut ChannelEstimator) {
    // Tracking outputs are one-shot increments.
    chanest.last_track.timing_adjust = 0;
    chanest.last_track.sro_delta_hz = 0.0;
}

/// Drop the oldest entries of `w` beyond the last `n`.
fn keep_last<T>(w: &mut VecDeque<T>, n: usize) {
    let excess = w.len().saturating_sub(n);
    w.drain(..excess);
}
