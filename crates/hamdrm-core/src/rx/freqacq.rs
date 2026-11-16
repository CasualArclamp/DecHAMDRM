//! Coarse frequency acquisition: find the HamDRM DC carrier (k = 0, nominally at
//! 350 Hz in the audio) from the three continuous frequency pilots, which sit 375,
//! 1125 and 1500 Hz above DC in every robustness mode (`docs/spec/phy.md` §2.1, §7).
//!
//! Port of DecDRM's `rx/freqacq.rs` (Dream's `CFreqSyncAcq`), with HamDRM's pilot
//! offsets as in EasyDRF `common/sync/FreqSyncAcq.cpp:272-311` (which also uses a
//! 6144-point PSD and mode B's pilot table for every mode). EasyDRF searches DC only
//! in 350 ± 5 Hz by default and QSSTV the whole band; here the window is a setting
//! ([`SearchWindow`]) with a generous default of 350 ± 250 Hz. No inverted-spectrum
//! search: HamDRM is always real USB audio.

use crate::params::{DEFAULT_DC_FREQ, SAMPLE_RATE};
use crate::{Cplx, Real};
use decdrm_core::dsp::fft::Fft;
use decdrm_core::dsp::hamming;
use std::collections::VecDeque;

/// FFT size: 7.8125 Hz resolution at 48 kHz (EasyDRF `FreqSyncAcq.h:54`).
const FFT_LEN: usize = 6 * 1024;
/// Pilot offsets from DC in FFT bins: 375, 1125, 1500 Hz / 7.8125 Hz.
const PILOT_BINS: [usize; 3] = [48, 144, 192];
/// Number of periodograms averaged (Dream: NUM_FFT_RES_AV_BLOCKS).
const NUM_AVERAGE: usize = 13;
/// Hop between periodograms in samples (one mode-A/B symbol).
const HOP: usize = 1280;
/// Samples spanned by the averaged periodograms (≈ 0.45 s).
pub const ANALYSIS_SPAN: usize = FFT_LEN + (NUM_AVERAGE - 1) * HOP;
/// Noise-floor smoother (IIR over frequency).
const LAMBDA_FLOOR: Real = 0.87;
/// Detection threshold on the sum of the three noise-normalised pilot powers.
const PEAK_BOUND: Real = 9.0;
/// Sinusoid rejection ratios (second-highest/highest, lowest/highest).
const MAX_RATIO_HIGH: Real = 0.99;
const MAX_RATIO_LOW: Real = 0.8;

/// Where to search for the DC carrier, Hz in the audio spectrum.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SearchWindow {
    pub min_hz: Real,
    pub max_hz: Real,
}

impl SearchWindow {
    /// `centre ± span`.
    pub fn around(centre_hz: Real, span_hz: Real) -> Self {
        Self {
            min_hz: centre_hz - span_hz,
            max_hz: centre_hz + span_hz,
        }
    }
}

impl Default for SearchWindow {
    /// 350 ± 250 Hz: covers SSB mistuning well beyond EasyDRF's default ±5 Hz.
    fn default() -> Self {
        Self::around(DEFAULT_DC_FREQ, 250.0)
    }
}

/// Successful acquisition.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Acquisition {
    /// Frequency of the DC carrier in the audio, Hz (bin-quantised, ±3.9 Hz).
    pub dc_hz: Real,
    /// Detection metric (sum of normalised pilot powers).
    pub score: Real,
    /// Mean input power over the analysed span (window-weighted), |sample|².
    pub power: Real,
}

#[derive(Debug)]
pub struct FreqAcquisition {
    fft: Fft,
    window: Vec<Real>,
    /// Σ window², for the input power.
    window_energy: Real,
    /// Most recent FFT_LEN samples.
    history: VecDeque<Cplx>,
    since_last: usize,
    psds: VecDeque<Vec<Real>>,
    psd_sum: Vec<Real>,
    search: SearchWindow,
    work: Vec<Cplx>,
}

impl FreqAcquisition {
    pub fn new(search: SearchWindow) -> Self {
        let window = hamming(FFT_LEN);
        Self {
            fft: Fft::new(FFT_LEN),
            window_energy: window.iter().map(|w| w * w).sum(),
            window,
            history: VecDeque::with_capacity(FFT_LEN),
            since_last: 0,
            psds: VecDeque::with_capacity(NUM_AVERAGE),
            psd_sum: vec![0.0; FFT_LEN],
            search,
            work: vec![Cplx::new(0.0, 0.0); FFT_LEN],
        }
    }

    /// Feed (analytic) samples; returns an acquisition as soon as the pilots are found.
    pub fn push(&mut self, samples: &[Cplx]) -> Option<Acquisition> {
        let mut result = None;
        for &s in samples {
            if self.history.len() == FFT_LEN {
                self.history.pop_front();
            }
            self.history.push_back(s);
            self.since_last += 1;
            if self.history.len() == FFT_LEN && self.since_last >= HOP {
                self.since_last = 0;
                self.add_periodogram();
                if self.psds.len() == NUM_AVERAGE
                    && let Some(r) = self.detect()
                {
                    result = Some(r);
                }
            }
        }
        result
    }

    fn add_periodogram(&mut self) {
        for (i, (w, s)) in self.window.iter().zip(&self.history).enumerate() {
            self.work[i] = s * *w;
        }
        self.fft.forward(&mut self.work);
        // Bin order 0..N ↔ 0..fs; the audio band of interest is the low positive bins.
        let psd: Vec<Real> = self.work.iter().map(|v| v.norm_sqr()).collect();
        if self.psds.len() == NUM_AVERAGE
            && let Some(old) = self.psds.pop_front()
        {
            for (acc, v) in self.psd_sum.iter_mut().zip(&old) {
                *acc -= v;
            }
        }
        for (acc, v) in self.psd_sum.iter_mut().zip(&psd) {
            *acc += v;
        }
        self.psds.push_back(psd);
    }

    fn detect(&mut self) -> Option<Acquisition> {
        let n = FFT_LEN;
        let psd: Vec<Real> = self
            .psd_sum
            .iter()
            .map(|v| v / NUM_AVERAGE as Real)
            .collect();
        // Noise floor: forward and backward one-pole smoothing, averaged.
        let mut lr = vec![0.0; n];
        let mut rl = vec![0.0; n];
        lr[0] = psd[0];
        for i in 1..n {
            lr[i] = (lr[i - 1] - psd[i]) * LAMBDA_FLOOR + psd[i];
        }
        rl[n - 1] = psd[n - 1];
        for i in (0..n - 1).rev() {
            rl[i] = (rl[i + 1] - psd[i]) * LAMBDA_FLOOR + psd[i];
        }
        let norm: Vec<Real> = (0..n)
            .map(|i| {
                let floor = 0.5 * (lr[i] + rl[i]);
                if floor > 0.0 { psd[i] / floor } else { 0.0 }
            })
            .collect();

        let bin_hz = Real::from(SAMPLE_RATE) / n as Real;
        let top = n / 2 - PILOT_BINS[2] - 1;
        let lo = ((self.search.min_hz / bin_hz).floor().max(1.0) as usize).min(top);
        let hi = ((self.search.max_hz / bin_hz).ceil().max(1.0) as usize).min(top);

        let mut best: Option<(usize, Real)> = None;
        for d in lo..=hi {
            let (a, b, c) = (d + PILOT_BINS[0], d + PILOT_BINS[1], d + PILOT_BINS[2]);
            let score = norm[a] + norm[b] + norm[c];
            if score <= PEAK_BOUND {
                continue;
            }
            // Reject single sinusoids: the three pilots must have similar power.
            let mut v = [norm[a], norm[b], norm[c]];
            v.sort_by(Real::total_cmp);
            if v[1] / v[2] < MAX_RATIO_HIGH && v[0] / v[2] < MAX_RATIO_LOW {
                continue;
            }
            if best.is_none_or(|(_, s)| score > s) {
                best = Some((d, score));
            }
        }
        // Parseval: Σ|X|² = N·Σ|x·w|².
        let power = psd.iter().sum::<Real>() / (n as Real * self.window_energy);
        best.map(|(d, score)| Acquisition {
            dc_hz: d as Real * bin_hz,
            score,
            power,
        })
    }
}
