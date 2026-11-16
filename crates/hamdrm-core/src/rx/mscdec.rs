//! The MSC decoding stage: the cell deinterleaver and the multilevel decoder that
//! turn one frame's equalised MSC cells into its bits (`docs/spec/phy.md` §5).
//! Port of DecDRM's `rx/mscdec.rs`; HamDRM has one stream and no protection parts,
//! so the frame is just `iNumDecodedBitsMSC` bits.

use crate::mlc::{MlcDecoder, MlcParams};
use crate::params::MscConfig;
use decdrm_core::fec::qam::{EqCell, MetricKind};
use decdrm_core::interleave::CellDeinterleaver;

/// Output of one decoded MSC frame (before the receiver packs it into bytes).
pub(crate) struct DecodedMsc {
    pub bits: Vec<u8>,
    /// Normalised final Viterbi path metric per level (lower is better).
    pub path_metrics: Vec<f64>,
    /// False while the long deinterleaver is still filling after a (re)start.
    pub complete: bool,
}

pub(crate) struct MscDecoder {
    config: MscConfig,
    deint: CellDeinterleaver,
    dec: MlcDecoder,
    cells: usize,
    /// Frames pushed since the deinterleaver was (re)started.
    since_reset: usize,
}

impl MscDecoder {
    /// A decoder for frames of `cells` (N_MUX) cells.
    pub fn new(config: MscConfig, cells: usize, iterations: usize, metric: MetricKind) -> Self {
        let mut dec = MlcDecoder::new(MlcParams::for_config(&config, cells), iterations);
        dec.metric = metric;
        Self { config, deint: CellDeinterleaver::new(cells, config.interleave.depth()), dec, cells, since_reset: 0 }
    }

    pub fn config(&self) -> MscConfig {
        self.config
    }

    pub fn cells(&self) -> usize {
        self.cells
    }

    /// Decode the next frame from its cells; `gap`: frames were lost before it, so
    /// the deinterleaver starts afresh (missing cells become erasures).
    pub fn decode(&mut self, frame: &[EqCell], gap: bool) -> Option<DecodedMsc> {
        if gap {
            self.deint = CellDeinterleaver::new(self.cells, self.deint.depth());
            self.since_reset = 0;
        }
        self.since_reset += 1;
        let complete = self.since_reset >= self.deint.depth();
        let cells = self.deint.push(frame)?;
        let mut bits = Vec::new();
        let path_metrics = self.dec.decode(&cells, &mut bits);
        Some(DecodedMsc { bits, path_metrics, complete })
    }
}
