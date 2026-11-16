//! Multilevel coding for HamDRM's FAC and MSC (`docs/spec/phy.md` §4.2, §5).
//!
//! HamDRM uses only DRM30's standard mapping with no higher protected part and no
//! hierarchical layer, so this is DecDRM's `fec/mlc.rs` (a port of Dream's `CMLC`)
//! cut down to "everything is part B", plus 4-QAM in the MSC (rate 3/5, t₀ = 21,
//! EasyDRF `MLC.cpp:494-530`). The convolutional code, puncturing, Viterbi decoder,
//! QAM metrics, bit interleaver and energy dispersal are DecDRM's, unchanged.

use crate::Cplx;
use crate::params::{MscConfig, Qam};
use crate::tables::{FAC_BITS, FAC_RATE, MSC4_RATES, MSC16_RATES, MSC64_RATES, NUM_FAC_CELLS};
use decdrm_core::fec::interleaver::BitInterleaver;
use decdrm_core::fec::puncture::{PunctureSpec, TailRule, coded_len, mask_table};
use decdrm_core::fec::qam::{EqCell, Mapping, MetricKind};
use decdrm_core::fec::viterbi::ViterbiDecoder;
use decdrm_core::fec::{BitMetric, conv, dispersal};
use decdrm_core::tables::{CODE_RATES, PunctureMask};

/// Bit-interleaver t₀ per interleaver type (13, 21) (ES 201 980 §7.3.3).
const T0: [usize; 2] = [13, 21];

/// Coding parameters of one FAC or MSC block.
#[derive(Debug, Clone)]
pub struct MlcParams {
    pub mapping: Mapping,
    /// Cells per block (45 for the FAC, N_MUX for the MSC).
    pub cells: usize,
    /// Code-rate index (into DecDRM's `CODE_RATES`) and information bits per level.
    pub rates: Vec<usize>,
    pub bits: Vec<usize>,
    /// Bit interleaver type per level (index into [`T0`]).
    pub interleaver: Vec<Option<usize>>,
    is_fac: bool,
}

impl MlcParams {
    /// FAC: 48 bits, 4-QAM, rate 3/5 over 45 cells; the tail bits keep the main
    /// pattern (EasyDRF `ChannelCode.cpp:122-137`).
    pub fn fac() -> Self {
        Self {
            mapping: Mapping::Qam4,
            cells: NUM_FAC_CELLS,
            rates: vec![FAC_RATE],
            bits: vec![FAC_BITS],
            interleaver: vec![Some(1)],
            is_fac: true,
        }
    }

    /// MSC over `n_mux` cells. Bits per level M_j = RX_j·⌊(2·N_MUX − 12)/RY_j⌋
    /// (EasyDRF `MLC.cpp:516-518`).
    pub fn msc(qam: Qam, protection: u8, n_mux: usize) -> Self {
        let p = usize::from(protection.min(1));
        let (rates, interleaver): (Vec<usize>, Vec<Option<usize>>) = match qam {
            Qam::Qam4 => (MSC4_RATES.to_vec(), vec![Some(1)]),
            Qam::Qam16 => (MSC16_RATES[p].to_vec(), vec![Some(0), Some(1)]),
            Qam::Qam64 => (MSC64_RATES[p].to_vec(), vec![None, Some(0), Some(1)]),
        };
        let coded = 2 * n_mux - 12;
        let bits = rates
            .iter()
            .map(|&r| CODE_RATES[r].rx * (coded / CODE_RATES[r].ry))
            .collect();
        Self {
            mapping: qam.mapping(),
            cells: n_mux,
            rates,
            bits,
            interleaver,
            is_fac: false,
        }
    }

    pub fn for_config(cfg: &MscConfig, n_mux: usize) -> Self {
        Self::msc(cfg.qam, cfg.protection, n_mux)
    }

    /// Information bits per block (iNumDecodedBitsMSC for the MSC).
    pub fn total_bits(&self) -> usize {
        self.bits.iter().sum()
    }

    fn masks(&self) -> Vec<Vec<PunctureMask>> {
        (0..self.rates.len())
            .map(|j| {
                mask_table(&PunctureSpec {
                    n1: 0,
                    n2: self.cells,
                    bits_a: 0,
                    bits_b: self.bits[j],
                    rate_a: 0,
                    rate_b: self.rates[j],
                    level: j,
                    tail_rule: TailRule::Standard,
                    is_fac: self.is_fac,
                })
            })
            .collect()
    }

    fn interleavers(&self) -> [BitInterleaver; 2] {
        let n = 2 * self.cells;
        [
            BitInterleaver::new(0, n, T0[0]),
            BitInterleaver::new(0, n, T0[1]),
        ]
    }

    fn split(&self, bits: &[u8]) -> Vec<Vec<u8>> {
        let mut pos = 0;
        self.bits
            .iter()
            .map(|&n| {
                let v = bits[pos..pos + n].to_vec();
                pos += n;
                v
            })
            .collect()
    }
}

/// Multilevel encoder.
#[derive(Debug, Clone)]
pub struct MlcEncoder {
    params: MlcParams,
    masks: Vec<Vec<PunctureMask>>,
    interleavers: [BitInterleaver; 2],
}

impl MlcEncoder {
    pub fn new(params: MlcParams) -> Self {
        let masks = params.masks();
        let interleavers = params.interleavers();
        Self {
            params,
            masks,
            interleavers,
        }
    }

    pub fn params(&self) -> &MlcParams {
        &self.params
    }

    /// Encode `total_bits()` information bits into `cells` QAM cells.
    pub fn encode(&self, bits: &[u8], out: &mut Vec<Cplx>) {
        let p = &self.params;
        assert_eq!(bits.len(), p.total_bits(), "MLC block size mismatch");
        let mut scrambled = bits.to_vec();
        dispersal::apply(&mut scrambled, 0);
        let info = p.split(&scrambled);
        let per_level = p.mapping.coded_bits_per_level(p.cells);
        let coded: Vec<Vec<u8>> = (0..p.rates.len())
            .map(|j| {
                let mut c = Vec::new();
                conv::encode(&info[j], &self.masks[j], &mut c);
                debug_assert_eq!(c.len(), per_level, "level {j} coded length");
                c.resize(per_level, 0);
                if let Some(t) = p.interleaver[j] {
                    self.interleavers[t].interleave(&mut c);
                }
                c
            })
            .collect();
        out.clear();
        out.resize(p.cells, Cplx::new(0.0, 0.0));
        p.mapping.map(&coded, out);
    }
}

/// Iterative multistage decoder.
#[derive(Debug)]
pub struct MlcDecoder {
    params: MlcParams,
    masks: Vec<Vec<PunctureMask>>,
    interleavers: [BitInterleaver; 2],
    viterbi: ViterbiDecoder,
    /// Extra decoding passes (0 = plain multistage decoding).
    pub iterations: usize,
    pub metric: MetricKind,
    decided: Vec<Vec<u8>>,
    metrics: Vec<BitMetric>,
    info: Vec<Vec<u8>>,
}

impl MlcDecoder {
    pub fn new(params: MlcParams, iterations: usize) -> Self {
        let masks = params.masks();
        let interleavers = params.interleavers();
        let n = params.rates.len();
        let iterations = if n == 1 { 0 } else { iterations };
        Self {
            params,
            masks,
            interleavers,
            viterbi: ViterbiDecoder::new(),
            iterations,
            metric: MetricKind::default(),
            decided: vec![Vec::new(); n],
            metrics: Vec::new(),
            info: vec![Vec::new(); n],
        }
    }

    pub fn params(&self) -> &MlcParams {
        &self.params
    }

    /// Decode `cells` equalised cells into `total_bits()` bits (dispersal removed).
    /// Returns the normalised final Viterbi path metric of each level.
    pub fn decode(&mut self, cells: &[EqCell], out: &mut Vec<u8>) -> Vec<f64> {
        let p = &self.params;
        assert_eq!(cells.len(), p.cells, "MLC cell count mismatch");
        let nl = p.rates.len();
        let mut path = vec![0.0; nl];
        for d in &mut self.decided {
            d.clear();
        }
        for pass in 0..=self.iterations {
            for j in 0..nl {
                p.mapping.metrics(
                    cells,
                    j,
                    &self.decided,
                    pass > 0,
                    self.metric,
                    &mut self.metrics,
                );
                if let Some(t) = p.interleaver[j] {
                    self.interleavers[t].deinterleave(&mut self.metrics);
                }
                path[j] = self
                    .viterbi
                    .decode(&self.metrics, &self.masks[j], &mut self.info[j]);
                if pass < self.iterations || j + 1 < nl {
                    let mut c = std::mem::take(&mut self.decided[j]);
                    conv::encode(&self.info[j], &self.masks[j], &mut c);
                    if let Some(t) = p.interleaver[j] {
                        self.interleavers[t].interleave(&mut c);
                    }
                    self.decided[j] = c;
                }
            }
        }
        out.clear();
        for j in 0..nl {
            out.extend_from_slice(&self.info[j][..p.bits[j]]);
        }
        dispersal::apply(out, 0);
        path
    }
}

/// The coded length of every level must exactly fill the cells.
pub fn check_params(params: &MlcParams) -> Result<(), String> {
    let want = params.mapping.coded_bits_per_level(params.cells);
    for (j, m) in params.masks().iter().enumerate() {
        let got = coded_len(m);
        if got != want {
            return Err(format!("level {j}: {got} coded bits, expected {want}"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cellmap::CellMap;
    use crate::params::{Bandwidth, Layout, Mode};

    fn bits(n: usize, seed: u32) -> Vec<u8> {
        let mut x = seed.wrapping_mul(2654435761).wrapping_add(12345);
        (0..n)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                (x & 1) as u8
            })
            .collect()
    }

    fn roundtrip(params: MlcParams) {
        check_params(&params).unwrap();
        let data = bits(params.total_bits(), params.cells as u32);
        let mut cells = Vec::new();
        MlcEncoder::new(params.clone()).encode(&data, &mut cells);
        let eq: Vec<EqCell> = cells
            .iter()
            .map(|&s| EqCell { sig: s, chan: 1.0 })
            .collect();
        let mut out = Vec::new();
        MlcDecoder::new(params, 1).decode(&eq, &mut out);
        assert_eq!(out, data);
    }

    #[test]
    fn fac_roundtrip() {
        roundtrip(MlcParams::fac());
    }

    /// iNumDecodedBitsMSC for every layout and scheme (phy.md §5.5).
    #[test]
    fn msc_bit_counts_and_roundtrip() {
        use Bandwidth::*;
        use Mode::*;
        let table: [(Mode, Bandwidth, [usize; 5]); 6] = [
            (A, Narrow, [768, 1281, 1601, 1921, 2305]),
            (A, Wide, [837, 1395, 1745, 2094, 2511]),
            (B, Narrow, [537, 897, 1121, 1345, 1613]),
            (B, Wide, [627, 1047, 1310, 1572, 1883]),
            (E, Narrow, [399, 666, 831, 997, 1198]),
            (E, Wide, [435, 726, 906, 1087, 1306]),
        ];
        for (m, b, want) in table {
            let n = CellMap::new(Layout::new(m, b)).msc_cells_per_frame;
            let cfgs = [
                (Qam::Qam4, 0),
                (Qam::Qam16, 0),
                (Qam::Qam16, 1),
                (Qam::Qam64, 0),
                (Qam::Qam64, 1),
            ];
            for ((q, p), w) in cfgs.into_iter().zip(want) {
                let params = MlcParams::msc(q, p, n);
                assert_eq!(params.total_bits(), w, "{m} {b} {q} {p}");
                roundtrip(params);
            }
        }
    }
}
