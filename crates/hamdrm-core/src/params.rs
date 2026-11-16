//! HamDRM transmission parameters: robustness modes A/B/E, the two bandwidths and
//! the MSC coding choices signalled in the FAC (`docs/spec/phy.md` §1, §4, §5).
//!
//! Everything runs at [`SAMPLE_RATE`] (48 kHz), as in EasyDRF and DecDRM: every mode
//! then has an integer number of samples per useful symbol and guard interval.

use decdrm_core::fec::qam::Mapping;
use std::fmt;

/// Working sample rate, Hz (EasyDRF `TableDRMGlobal.h:43`).
pub const SAMPLE_RATE: u32 = 48_000;
/// Transmission frames per super frame.
pub const FRAMES_PER_SUPERFRAME: usize = 3;
/// One transmission frame (400 ms) in samples, the same in every mode.
pub const SAMPLES_PER_FRAME: usize = 19_200;
/// Default audio frequency of carrier k = 0 on air, Hz (EasyDRF `DrmTransmitter.cpp:212`,
/// QSSTV `CDrmTransmitter.cpp:217`; EasyDRF's receiver searches 350 ± 5 Hz).
pub const DEFAULT_DC_FREQ: f64 = 350.0;

/// HamDRM robustness mode. A and B share DRM30's numerology; E is HamDRM's own
/// (Tu = 13.33 ms, Tg/Tu = 1/2), not DRM30 C or D.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Mode {
    A,
    B,
    E,
}

impl Mode {
    pub const ALL: [Mode; 3] = [Self::A, Self::B, Self::E];

    /// Index 0..=2 used to address per-mode tables.
    pub const fn index(self) -> usize {
        self as usize
    }

    /// Useful symbol length Tu in samples at 48 kHz (= FFT size)
    /// (EasyDRF `TableDRMGlobal.h:49,54,59`).
    pub const fn fft_size(self) -> usize {
        match self {
            Self::A => 1152,
            Self::B => 1024,
            Self::E => 640,
        }
    }

    /// Tg/Tu as (numerator, denominator).
    pub const fn guard_ratio(self) -> (usize, usize) {
        match self {
            Self::A => (1, 9),
            Self::B => (1, 4),
            Self::E => (1, 2),
        }
    }

    pub const fn guard_len(self) -> usize {
        let (n, d) = self.guard_ratio();
        self.fft_size() * n / d
    }

    pub const fn symbol_len(self) -> usize {
        self.fft_size() + self.guard_len()
    }

    pub const fn symbols_per_frame(self) -> usize {
        match self {
            Self::A | Self::B => 15,
            Self::E => 20,
        }
    }

    pub const fn symbols_per_superframe(self) -> usize {
        self.symbols_per_frame() * FRAMES_PER_SUPERFRAME
    }

    /// Carrier spacing 1/Tu, Hz.
    pub fn carrier_spacing(self) -> f64 {
        f64::from(SAMPLE_RATE) / self.fft_size() as f64
    }
}

impl fmt::Display for Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::A => "A",
            Self::B => "B",
            Self::E => "E",
        })
    }
}

/// Spectrum occupancy: the FAC's 1-bit field. EasyDRF calls SO_0 "2.3 kHz" and SO_1
/// "2.5 kHz" (`FAC.cpp:404,429`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Bandwidth {
    Narrow,
    Wide,
}

impl Bandwidth {
    pub const ALL: [Bandwidth; 2] = [Self::Narrow, Self::Wide];

    pub const fn index(self) -> usize {
        self as usize
    }

    pub const fn from_bit(b: bool) -> Self {
        if b { Self::Wide } else { Self::Narrow }
    }
}

impl fmt::Display for Bandwidth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Narrow => "2.3 kHz",
            Self::Wide => "2.5 kHz",
        })
    }
}

/// Mode + bandwidth: fixes the cell map.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Layout {
    pub mode: Mode,
    pub bandwidth: Bandwidth,
}

impl Layout {
    pub const fn new(mode: Mode, bandwidth: Bandwidth) -> Self {
        Self { mode, bandwidth }
    }

    pub fn all() -> impl Iterator<Item = Layout> {
        Mode::ALL
            .into_iter()
            .flat_map(|m| Bandwidth::ALL.into_iter().map(move |b| Layout::new(m, b)))
    }

    /// Carrier range (Kmin, Kmax) (EasyDRF `TableCarrier.h:87-95`).
    pub const fn carrier_range(self) -> (i32, i32) {
        const KMIN: [i32; 3] = [2, 1, 1];
        const KMAX: [[i32; 3]; 2] = [[54, 45, 29], [58, 51, 31]];
        let m = self.mode.index();
        (KMIN[m], KMAX[self.bandwidth.index()][m])
    }

    pub const fn num_carriers(self) -> usize {
        let (a, b) = self.carrier_range();
        (b - a + 1) as usize
    }
}

impl fmt::Display for Layout {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "mode {} {}", self.mode, self.bandwidth)
    }
}

/// MSC constellation (FAC MSC-mode bit plus the 4-QAM extension bit).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Qam {
    Qam4,
    Qam16,
    Qam64,
}

impl Qam {
    pub const ALL: [Qam; 3] = [Self::Qam4, Self::Qam16, Self::Qam64];

    pub const fn mapping(self) -> Mapping {
        match self {
            Self::Qam4 => Mapping::Qam4,
            Self::Qam16 => Mapping::Qam16,
            Self::Qam64 => Mapping::Qam64Sm,
        }
    }
}

impl fmt::Display for Qam {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Qam4 => "4-QAM",
            Self::Qam16 => "16-QAM",
            Self::Qam64 => "64-QAM",
        })
    }
}

/// Cell interleaver depth (FAC bit 3): long = 5 frames (2 s), short = 1 frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Interleave {
    Long,
    Short,
}

impl Interleave {
    pub const fn depth(self) -> usize {
        match self {
            Self::Long => 5,
            Self::Short => 1,
        }
    }
}

/// Everything that fixes the MSC frame format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MscConfig {
    pub layout: Layout,
    pub qam: Qam,
    /// Protection level 0 ("normal", lower rate) or 1 ("low"). Ignored for 4-QAM.
    pub protection: u8,
    pub interleave: Interleave,
}

impl MscConfig {
    /// EasyPal's usual picture setting: mode B, 2.5 kHz, 16-QAM, normal protection.
    pub const fn default_picture() -> Self {
        Self {
            layout: Layout::new(Mode::B, Bandwidth::Wide),
            qam: Qam::Qam16,
            protection: 0,
            interleave: Interleave::Short,
        }
    }
}

impl fmt::Display for MscConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {} prot {}", self.layout, self.qam, self.protection)?;
        if self.interleave == Interleave::Long {
            f.write_str(" long")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_frame_is_400_ms() {
        for m in Mode::ALL {
            assert_eq!(
                m.symbol_len() * m.symbols_per_frame(),
                SAMPLES_PER_FRAME,
                "{m}"
            );
        }
    }

    #[test]
    fn frequency_pilots_are_at_the_same_audio_offsets() {
        // 375 Hz above DC in every mode (phy.md §2.1).
        for (m, k) in [(Mode::A, 9), (Mode::B, 8), (Mode::E, 5)] {
            assert!((m.carrier_spacing() * k as f64 - 375.0).abs() < 1e-9);
        }
    }
}
