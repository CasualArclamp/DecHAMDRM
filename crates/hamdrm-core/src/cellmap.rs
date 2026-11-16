//! OFDM cell map of one HamDRM transmission frame: which cell carries MSC, FAC or a
//! pilot, and the reference values of the pilots (`docs/spec/phy.md` §2–§3).
//!
//! Port of DecDRM's `cellmap.rs` (itself Dream's `CCellMappingTable::MakeTable`), with
//! EasyDRF's HamDRM tables. HamDRM has no SDC and every frame of a super frame has the
//! same pattern, so one frame is enough and there are no MSC dummy cells.

use crate::params::{FRAMES_PER_SUPERFRAME, Layout, Mode};
use crate::tables::{self, ScatteredPilots};
use crate::{Cplx, Real};
use std::f64::consts::PI;

pub const DATA_CELL_POWER: Real = 1.0;
pub const PILOT_POWER: Real = 2.0;
pub const BOOSTED_PILOT_POWER: Real = 4.0;

/// Classification of one cell. A scattered pilot overwritten by a time or frequency
/// pilot keeps both flags (the value is the later pilot's), as in Dream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CellType(u8);

impl CellType {
    pub const MSC: Self = Self(1);
    pub const FAC: Self = Self(2);
    pub const TIME_PILOT: Self = Self(4);
    pub const FREQ_PILOT: Self = Self(8);
    pub const SCAT_PILOT: Self = Self(16);
    pub const BOOSTED: Self = Self(32);

    pub const fn contains(self, o: Self) -> bool {
        self.0 & o.0 != 0
    }
    pub const fn is_msc(self) -> bool {
        self.contains(Self::MSC)
    }
    pub const fn is_fac(self) -> bool {
        self.contains(Self::FAC)
    }
    pub const fn is_data(self) -> bool {
        self.0 & (Self::MSC.0 | Self::FAC.0) != 0
    }
    pub const fn is_pilot(self) -> bool {
        self.0 & (Self::TIME_PILOT.0 | Self::FREQ_PILOT.0 | Self::SCAT_PILOT.0) != 0
    }
    pub const fn is_scattered(self) -> bool {
        self.contains(Self::SCAT_PILOT)
    }
    pub const fn is_time_pilot(self) -> bool {
        self.contains(Self::TIME_PILOT)
    }
    pub const fn is_freq_pilot(self) -> bool {
        self.contains(Self::FREQ_PILOT)
    }
    pub const fn is_boosted(self) -> bool {
        self.contains(Self::BOOSTED)
    }
}

impl std::ops::BitOr for CellType {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

/// Cell map of one frame for a given layout.
#[derive(Debug, Clone)]
pub struct CellMap {
    pub layout: Layout,
    pub kmin: i32,
    pub kmax: i32,
    pub num_carriers: usize,
    pub symbols_per_frame: usize,
    pub scattered: ScatteredPilots,
    /// `symbols_per_frame × num_carriers`, row-major, carrier offset `k − kmin`.
    cells: Vec<CellType>,
    pilots: Vec<Cplx>,
    msc_carriers: Vec<Vec<u16>>,
    fac_carriers: Vec<Vec<u16>>,
    /// MSC cells per frame (N_MUX).
    pub msc_cells_per_frame: usize,
    /// Mean transmitted power per symbol (sum over carriers).
    pub avg_power_per_symbol: Real,
    pub avg_scattered_pilot_power: Real,
}

impl CellMap {
    pub fn new(layout: Layout) -> Self {
        let mode = layout.mode;
        let (kmin, kmax) = layout.carrier_range();
        let num_carriers = layout.num_carriers();
        let ns = mode.symbols_per_frame();
        let sp = tables::scattered_pilots(mode);
        let boosted = tables::boosted_pilots(mode, layout.bandwidth);
        let fac = tables::fac_positions(mode);
        let time_pilots = tables::time_pilots(mode);
        let freq_pilots = tables::freq_pilots(mode);

        let mut cells = vec![CellType::default(); ns * num_carriers];
        let mut pilots = vec![Cplx::new(0.0, 0.0); ns * num_carriers];
        let x = sp.freq_int as i32;
        let y = sp.time_int as i32;
        for s in 0..ns as i32 {
            for k in kmin..=kmax {
                let idx = s as usize * num_carriers + (k - kmin) as usize;
                let mut ty = CellType::MSC;
                if fac.iter().any(|&(fs, fk)| i32::from(fs) == s && i32::from(fk) == k) {
                    ty = CellType::FAC;
                }
                // Gain references; time/frequency references take precedence.
                if (k - sp.k0 - x * (s % y)).rem_euclid(x * y) == 0 {
                    ty = CellType::SCAT_PILOT;
                    let n = (s % y) as usize;
                    let m = (s / y) as usize;
                    let p = (k - sp.k0 - x * (s % y)) / (x * y);
                    let w = sp.w[n * sp.wz_cols + m];
                    let z = sp.z[n * sp.wz_cols + m];
                    let phase = (4 * z + p * w + p * p * (1 + s) * sp.q).rem_euclid(1024);
                    let power = if boosted.contains(&k) {
                        ty = ty | CellType::BOOSTED;
                        BOOSTED_PILOT_POWER
                    } else {
                        PILOT_POWER
                    };
                    pilots[idx] = polar_1024(power.sqrt(), phase);
                }
                if s == 0
                    && let Some(&(_, phase)) = time_pilots.iter().find(|&&(tk, _)| i32::from(tk) == k)
                {
                    ty = if ty.is_scattered() { ty | CellType::TIME_PILOT } else { CellType::TIME_PILOT };
                    pilots[idx] = polar_1024(PILOT_POWER.sqrt(), i32::from(phase));
                }
                if let Some(pos) = freq_pilots.iter().position(|&(fk, _)| i32::from(fk) == k) {
                    ty = if ty.is_time_pilot() || ty.is_scattered() {
                        ty | CellType::FREQ_PILOT
                    } else {
                        CellType::FREQ_PILOT
                    };
                    let mut phase = i32::from(freq_pilots[pos].1);
                    // Mode E: the first two frequency pilots are inverted on odd
                    // symbols (DRM30's mode-D rule; EasyDRF CellMappingTable.cpp:379-402).
                    if mode == Mode::E && pos != 2 && s % 2 == 1 {
                        phase = (phase + 512) % 1024;
                    }
                    pilots[idx] = polar_1024(PILOT_POWER.sqrt(), phase);
                }
                cells[idx] = ty;
            }
        }

        let mut msc_carriers = vec![Vec::new(); ns];
        let mut fac_carriers = vec![Vec::new(); ns];
        let (mut n_msc, mut power, mut scat_power, mut scat_n) = (0, 0.0, 0.0, 0usize);
        for s in 0..ns {
            for c in 0..num_carriers {
                let ty = cells[s * num_carriers + c];
                if ty.is_msc() {
                    msc_carriers[s].push(c as u16);
                    n_msc += 1;
                }
                if ty.is_fac() {
                    fac_carriers[s].push(c as u16);
                }
                let p = if ty.is_data() {
                    DATA_CELL_POWER
                } else if ty.is_boosted() {
                    BOOSTED_PILOT_POWER
                } else {
                    PILOT_POWER
                };
                power += p;
                if ty.is_scattered() {
                    scat_power += p;
                    scat_n += 1;
                }
            }
        }
        Self {
            layout,
            kmin,
            kmax,
            num_carriers,
            symbols_per_frame: ns,
            scattered: sp,
            cells,
            pilots,
            msc_carriers,
            fac_carriers,
            msc_cells_per_frame: n_msc,
            avg_power_per_symbol: power / ns as Real,
            avg_scattered_pilot_power: scat_power / scat_n.max(1) as Real,
        }
    }

    pub fn mode(&self) -> Mode {
        self.layout.mode
    }

    /// Cell type at frame symbol `s`, carrier offset `c = k − kmin`.
    pub fn cell(&self, s: usize, c: usize) -> CellType {
        self.cells[s * self.num_carriers + c]
    }

    pub fn pilot(&self, s: usize, c: usize) -> Cplx {
        self.pilots[s * self.num_carriers + c]
    }

    pub fn symbol_cells(&self, s: usize) -> &[CellType] {
        &self.cells[s * self.num_carriers..(s + 1) * self.num_carriers]
    }

    pub fn symbol_pilots(&self, s: usize) -> &[Cplx] {
        &self.pilots[s * self.num_carriers..(s + 1) * self.num_carriers]
    }

    /// Carrier offsets of the MSC cells of frame symbol `s`, in mapping order.
    pub fn msc_carriers(&self, s: usize) -> &[u16] {
        &self.msc_carriers[s]
    }

    pub fn fac_carriers(&self, s: usize) -> &[u16] {
        &self.fac_carriers[s]
    }

    pub fn carrier_index(&self, c: usize) -> i32 {
        self.kmin + c as i32
    }

    /// MSC cells per super frame.
    pub fn msc_cells_per_superframe(&self) -> usize {
        self.msc_cells_per_frame * FRAMES_PER_SUPERFRAME
    }
}

fn polar_1024(amp: Real, phase: i32) -> Cplx {
    Cplx::from_polar(amp, 2.0 * PI * f64::from(phase) / 1024.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::Bandwidth::*;
    use crate::params::Mode::*;
    use crate::tables::NUM_FAC_CELLS;

    /// N_MUX per layout (phy.md §3.2, checked there against EasyDRF's streamlen table).
    #[test]
    fn msc_cells_per_frame() {
        for (m, b, n) in [(A, Narrow, 647), (A, Wide, 704), (B, Narrow, 455), (B, Wide, 530), (E, Narrow, 339), (E, Wide, 369)] {
            let map = CellMap::new(Layout::new(m, b));
            assert_eq!(map.msc_cells_per_frame, n, "{}", map.layout);
            let fac: usize = (0..map.symbols_per_frame).map(|s| map.fac_carriers(s).len()).sum();
            assert_eq!(fac, NUM_FAC_CELLS);
        }
    }

    /// Per-symbol MSC counts of mode E 2.3 kHz (phy.md §3.2).
    #[test]
    fn mode_e_symbol_counts() {
        let map = CellMap::new(Layout::new(E, Narrow));
        let got: Vec<usize> = (0..20).map(|s| map.msc_carriers(s).len()).collect();
        assert_eq!(got, [13, 17, 17, 18, 17, 17, 18, 18, 16, 17, 17, 17, 17, 17, 17, 18, 16, 17, 17, 18]);
    }

    /// rAvPowPerSymbol (phy.md §2.4).
    #[test]
    fn average_power() {
        for (m, b, p) in [(A, Narrow, 61.0667), (A, Wide, 65.6667), (B, Narrow, 59.3333), (B, Wide, 66.3333), (E, Narrow, 39.8), (E, Wide, 42.3)] {
            let map = CellMap::new(Layout::new(m, b));
            assert!((map.avg_power_per_symbol - p).abs() < 1e-3, "{} {}", map.layout, map.avg_power_per_symbol);
        }
    }
}
