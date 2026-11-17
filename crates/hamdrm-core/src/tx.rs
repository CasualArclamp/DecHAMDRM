//! HamDRM transmitter chain: MSC frame bytes → MLC → cell interleaver → cell map with
//! FAC and pilots → OFDM → real upper-sideband audio with carrier k at
//! F_dc + k·Δf (`docs/spec/phy.md` §1.3, §4–§6).
//!
//! Port of DecDRM's `tx` (Dream's `CDRMTransmitter`), reduced to HamDRM: no SDC, one
//! stream, and the output is real audio around 350 Hz instead of a complex IF.

use crate::cellmap::CellMap;
use crate::fac::{Fac, Service};
use crate::mlc::{MlcEncoder, MlcParams};
use crate::params::{FRAMES_PER_SUPERFRAME, MscConfig, Qam, SAMPLE_RATE};
use crate::{Cplx, Real};
use decdrm_core::dsp::fft::Fft;
use decdrm_core::interleave::CellInterleaver;
use std::f64::consts::PI;

/// Transmitter settings that are not part of the MSC format.
#[derive(Debug, Clone)]
pub struct TxConfig {
    pub msc: MscConfig,
    pub service: Service,
    /// Station label (callsign), at most 9 characters.
    pub label: String,
    /// FAC extension byte (bits 32–39); 0 for plain HamDRM.
    pub ext: u8,
    /// Audio frequency of carrier k = 0, Hz.
    pub dc_freq: Real,
    /// RMS output level (full scale = 1.0).
    pub rms: Real,
}

impl TxConfig {
    pub fn new(msc: MscConfig, label: &str) -> Self {
        Self {
            msc,
            service: Service::Data { packet_id: 0 },
            label: label.to_string(),
            ext: 0,
            dc_freq: crate::params::DEFAULT_DC_FREQ,
            rms: 0.2,
        }
    }
}

/// One-frame-at-a-time transmitter.
pub struct Transmitter {
    cfg: TxConfig,
    map: CellMap,
    fac_enc: MlcEncoder,
    msc_enc: MlcEncoder,
    interleaver: CellInterleaver,
    fft: Fft,
    frame: u8,
    /// Mixer phase (radians) of the F_dc up-shift, continuous across symbols.
    phase: Real,
    gain: Real,
    work: Vec<Cplx>,
}

impl Transmitter {
    pub fn new(cfg: TxConfig) -> Self {
        let map = CellMap::new(cfg.msc.layout);
        let n_mux = map.msc_cells_per_frame;
        let msc_enc = MlcEncoder::new(MlcParams::for_config(&cfg.msc, n_mux));
        let n = map.mode().fft_size();
        // Real output Re{x·e^{jωn}} has half the power of x; x's power per sample is
        // the cell power summed over a symbol (unnormalised IFFT).
        let gain = cfg.rms * (2.0 / map.avg_power_per_symbol).sqrt();
        Self {
            interleaver: CellInterleaver::new(n_mux, cfg.msc.interleave.depth()),
            fac_enc: MlcEncoder::new(MlcParams::fac()),
            msc_enc,
            fft: Fft::new(n),
            frame: 0,
            phase: 0.0,
            gain,
            work: vec![Cplx::new(0.0, 0.0); n],
            map,
            cfg,
        }
    }

    /// Change the service (data / audio) and extension byte from the next frame on.
    pub fn set_service(&mut self, service: Service, ext: u8) {
        self.cfg.service = service;
        self.cfg.ext = ext;
    }

    pub fn config(&self) -> &TxConfig {
        &self.cfg
    }

    /// Information bits per MSC frame (iNumDecodedBitsMSC).
    pub fn msc_bits(&self) -> usize {
        self.msc_enc.params().total_bits()
    }

    /// Bytes per MSC frame (EasyDRF's streamlen); one packet of `packlen + 3` bytes.
    pub fn frame_bytes(&self) -> usize {
        self.msc_bits() / 8
    }

    /// Packet data-field length (EasyDRF `calcpacklen`, `callsign2.cpp:91-95`).
    pub fn packet_len(&self) -> usize {
        self.frame_bytes() - 3
    }

    /// Modulate one transmission frame carrying `msc` (at most [`Self::frame_bytes`]
    /// bytes; shorter is zero padded). Appends 19 200 samples to `out`.
    pub fn push_frame(&mut self, msc: &[u8], out: &mut Vec<f32>) {
        let nbits = self.msc_bits();
        let mut bits = vec![0u8; nbits];
        for (i, b) in bits.iter_mut().enumerate().take(msc.len() * 8) {
            *b = (msc[i / 8] >> (7 - i % 8)) & 1;
        }
        let mut msc_cells = Vec::new();
        self.msc_enc.encode(&bits, &mut msc_cells);
        let msc_cells = self.interleaver.push(&msc_cells);

        let fac = Fac {
            frame: self.frame,
            bandwidth: self.cfg.msc.layout.bandwidth,
            interleave: self.cfg.msc.interleave,
            qam: self.cfg.msc.qam,
            protection: self.cfg.msc.protection,
            service: self.cfg.service,
            label: Fac::label_part(&self.cfg.label, self.frame),
            // Our own 4-QAM rate 1/2 is flagged so that legacy receivers ignore it.
            ext: if self.cfg.msc.qam == Qam::Qam4 && self.cfg.msc.protection == 0 {
                self.cfg.ext | crate::fac::EXT_QAM4_HALF
            } else {
                self.cfg.ext
            },
        };
        let mut fac_cells = Vec::new();
        self.fac_enc.encode(&fac.encode(), &mut fac_cells);

        let (mut mi, mut fi) = (0, 0);
        let mode = self.map.mode();
        let (n, g) = (mode.fft_size(), mode.guard_len());
        let w = 2.0 * PI * self.cfg.dc_freq / f64::from(SAMPLE_RATE);
        for s in 0..self.map.symbols_per_frame {
            self.work.iter_mut().for_each(|v| *v = Cplx::new(0.0, 0.0));
            for (c, (&ty, &pilot)) in self
                .map
                .symbol_cells(s)
                .iter()
                .zip(self.map.symbol_pilots(s))
                .enumerate()
            {
                let v = if ty.is_msc() {
                    mi += 1;
                    msc_cells[mi - 1]
                } else if ty.is_fac() {
                    fi += 1;
                    fac_cells[fi - 1]
                } else {
                    pilot
                };
                let k = self.map.carrier_index(c);
                self.work[k.rem_euclid(n as i32) as usize] = v;
            }
            self.fft.inverse(&mut self.work);
            // Cyclic prefix then useful part, shifted up by F_dc.
            for i in (n - g..n).chain(0..n) {
                let z = self.work[i] * Cplx::from_polar(self.gain, self.phase);
                out.push(z.re as f32);
                self.phase = (self.phase + w) % (2.0 * PI);
            }
        }
        debug_assert_eq!(mi, self.map.msc_cells_per_frame);
        self.frame = (self.frame + 1) % FRAMES_PER_SUPERFRAME as u8;
    }
}
