//! The MSC decoding stage: the cell deinterleaver and the multilevel decoder that
//! turn one frame's equalised MSC cells into its bits (`docs/spec/phy.md` §5).
//! Port of DecDRM's `rx/mscdec.rs`; HamDRM has one stream and no protection parts,
//! so the frame is just `iNumDecodedBitsMSC` bits.

use crate::Real;
use crate::mlc::{MlcDecoder, MlcEncoder, MlcParams};
use crate::params::{MscConfig, Qam};
use decdrm_core::fec::qam::{EqCell, MetricKind};
use decdrm_core::tables::{QAM4, QAM16, QAM64_SM};
use decdrm_core::interleave::CellDeinterleaver;

/// Output of one decoded MSC frame (before the receiver packs it into bytes).
pub(crate) struct DecodedMsc {
    pub bits: Vec<u8>,
    /// Normalised final Viterbi path metric per level (lower is better).
    pub path_metrics: Vec<f64>,
    /// False while the long deinterleaver is still filling after a (re)start.
    pub complete: bool,
    /// Fraction of received cells (erasures excluded) whose nearest constellation
    /// point differs from the re-encoded decoded bits: the cell error rate before
    /// FEC, if the decoding was right.
    pub cell_error_rate: Real,
}

pub(crate) struct MscDecoder {
    config: MscConfig,
    deint: CellDeinterleaver,
    dec: MlcDecoder,
    /// Re-encodes the decoded bits for [`DecodedMsc::cell_error_rate`].
    enc: MlcEncoder,
    cells: usize,
    /// Frames pushed since the deinterleaver was (re)started.
    since_reset: usize,
}

impl MscDecoder {
    /// A decoder for frames of `cells` (N_MUX) cells.
    pub fn new(config: MscConfig, cells: usize, iterations: usize, metric: MetricKind) -> Self {
        let params = MlcParams::for_config(&config, cells);
        let enc = MlcEncoder::new(params.clone());
        let mut dec = MlcDecoder::new(params, iterations);
        dec.metric = metric;
        Self { config, deint: CellDeinterleaver::new(cells, config.interleave.depth()), dec, enc, cells, since_reset: 0 }
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
        let mut reenc = Vec::new();
        self.enc.encode(&bits, &mut reenc);
        let pam: &[Real] = match self.config.qam {
            Qam::Qam4 => &QAM4,
            Qam::Qam16 => &QAM16,
            Qam::Qam64 => &QAM64_SM,
        };
        let (mut errors, mut counted) = (0usize, 0usize);
        for (rx, tx) in cells.iter().zip(&reenc) {
            if rx.chan > 0.0 {
                counted += 1;
                if (nearest(rx.sig.re, pam) - tx.re).abs() > 1e-6 || (nearest(rx.sig.im, pam) - tx.im).abs() > 1e-6 {
                    errors += 1;
                }
            }
        }
        let cell_error_rate = errors as Real / counted.max(1) as Real;
        Some(DecodedMsc { bits, path_metrics, complete, cell_error_rate })
    }
}

/// The PAM level of `pam` nearest to `v`.
fn nearest(v: Real, pam: &[Real]) -> Real {
    pam.iter().copied().min_by(|a, b| (v - a).abs().total_cmp(&(v - b).abs())).unwrap_or(0.0)
}
