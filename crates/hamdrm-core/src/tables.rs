//! HamDRM tables: FAC cell positions, pilots and code rates.
//!
//! Copied from EasyDRF (github.com/DazDSP/EasyDRF, GPL-3; itself from WinDRM / Dream,
//! Volker Fischer, GPL-2+): `common/tables/TableCarrier.h`, `TableCarMap.h`,
//! `TableMLC.h`. See `docs/spec/phy.md` §2–§5 for where each differs from DRM30.

use crate::params::{Bandwidth, Mode};

/// FAC cells per frame (EasyDRF `TableCarrier.h:7`).
pub const NUM_FAC_CELLS: usize = 45;
/// FAC information bits per frame incl. CRC-8 (EasyDRF `TableFAC.h:33`).
pub const FAC_BITS: usize = 48;
/// FAC code-rate index (3/5) into `decdrm_core::tables::CODE_RATES` (`TableMLC.h:467`).
pub const FAC_RATE: usize = 6;

/// FAC positions {symbol, carrier} (EasyDRF `TableCarrier.h:9-64`).
const FAC_A: [(u8, u8); NUM_FAC_CELLS] = [
    (1, 10),
    (1, 22),
    (1, 30),
    (1, 50),
    (2, 14),
    (2, 26),
    (2, 34),
    (3, 18),
    (3, 30),
    (3, 38),
    (4, 22),
    (4, 34),
    (4, 42),
    (5, 18),
    (5, 26),
    (5, 38),
    (5, 46),
    (6, 22),
    (6, 30),
    (6, 42),
    (6, 50),
    (7, 26),
    (7, 34),
    (7, 46),
    (8, 10),
    (8, 30),
    (8, 38),
    (8, 50),
    (9, 14),
    (9, 34),
    (9, 42),
    (10, 18),
    (10, 38),
    (10, 46),
    (11, 10),
    (11, 22),
    (11, 42),
    (11, 50),
    (12, 14),
    (12, 26),
    (12, 46),
    (13, 18),
    (13, 30),
    (14, 22),
    (14, 34),
];
const FAC_B: [(u8, u8); NUM_FAC_CELLS] = [
    (0, 21),
    (1, 11),
    (1, 23),
    (1, 35),
    (2, 13),
    (2, 25),
    (2, 37),
    (3, 15),
    (3, 27),
    (3, 39),
    (4, 5),
    (4, 17),
    (4, 29),
    (4, 41),
    (5, 7),
    (5, 19),
    (5, 31),
    (6, 9),
    (6, 21),
    (6, 33),
    (7, 11),
    (7, 23),
    (7, 35),
    (8, 13),
    (8, 25),
    (8, 37),
    (9, 15),
    (9, 27),
    (9, 39),
    (10, 5),
    (10, 17),
    (10, 29),
    (10, 41),
    (11, 7),
    (11, 19),
    (11, 31),
    (12, 9),
    (12, 21),
    (12, 33),
    (13, 11),
    (13, 23),
    (13, 35),
    (14, 13),
    (14, 25),
    (14, 37),
];
const FAC_E: [(u8, u8); NUM_FAC_CELLS] = [
    (1, 7),
    (1, 23),
    (2, 8),
    (2, 16),
    (2, 24),
    (3, 9),
    (3, 17),
    (4, 10),
    (4, 18),
    (5, 11),
    (5, 19),
    (6, 4),
    (6, 12),
    (7, 13),
    (7, 21),
    (8, 6),
    (8, 14),
    (8, 22),
    (9, 7),
    (9, 23),
    (10, 8),
    (10, 16),
    (10, 24),
    (11, 9),
    (11, 13),
    (11, 17),
    (12, 10),
    (12, 18),
    (13, 11),
    (13, 19),
    (14, 4),
    (14, 12),
    (14, 16),
    (15, 13),
    (15, 21),
    (16, 6),
    (16, 14),
    (16, 22),
    (17, 7),
    (17, 23),
    (18, 8),
    (18, 16),
    (18, 24),
    (19, 9),
    (19, 17),
];

pub fn fac_positions(mode: Mode) -> &'static [(u8, u8)] {
    match mode {
        Mode::A => &FAC_A,
        Mode::B => &FAC_B,
        Mode::E => &FAC_E,
    }
}

/// Frequency pilots {carrier, phase/1024}, every symbol (`TableCarrier.h:68-84`).
/// At 375, 1125 and 1500 Hz above DC in every mode.
pub fn freq_pilots(mode: Mode) -> &'static [(u8, u16)] {
    match mode {
        Mode::A => &[(9, 205), (27, 836), (36, 215)],
        Mode::B => &[(8, 331), (24, 651), (32, 555)],
        Mode::E => &[(5, 788), (15, 1014), (20, 332)],
    }
}

/// Time (sync) pilots {carrier, phase/1024} in symbol 0 of every frame
/// (`TableCarMap.h:40-90`).
pub fn time_pilots(mode: Mode) -> &'static [(u8, u16)] {
    match mode {
        Mode::A => &[
            (6, 973),
            (7, 205),
            (11, 717),
            (12, 264),
            (15, 357),
            (16, 357),
            (23, 952),
            (29, 440),
            (30, 856),
            (33, 88),
            (34, 88),
            (38, 68),
            (39, 836),
            (41, 836),
            (45, 836),
            (46, 1008),
        ],
        Mode::B => &[
            (6, 304),
            (10, 331),
            (11, 108),
            (14, 620),
            (17, 192),
            (18, 704),
            (27, 44),
            (28, 432),
            (30, 588),
            (33, 844),
            (34, 651),
            (38, 651),
            (40, 651),
            (41, 460),
            (44, 944),
        ],
        Mode::E => &[
            (7, 432),
            (8, 331),
            (13, 108),
            (14, 620),
            (21, 192),
            (22, 704),
            (26, 44),
            (27, 304),
        ],
    }
}

/// Scattered (gain reference) pilot parameters (`TableCarMap.h:95-157`).
#[derive(Debug, Clone, Copy)]
pub struct ScatteredPilots {
    /// Carrier spacing x and symbol period y of the pattern.
    pub freq_int: usize,
    pub time_int: usize,
    pub k0: i32,
    /// W and Z, row-major `[n][m]` with `wz_cols` columns; Q.
    pub w: &'static [i32],
    pub z: &'static [i32],
    pub wz_cols: usize,
    pub q: i32,
}

pub fn scattered_pilots(mode: Mode) -> ScatteredPilots {
    match mode {
        Mode::A => ScatteredPilots {
            freq_int: 4,
            time_int: 5,
            k0: 2,
            w: &[
                228, 341, 455, 455, 569, 683, 683, 796, 910, 910, 0, 114, 114, 228, 341,
            ],
            z: &[
                0, 81, 248, 18, 106, 106, 122, 116, 31, 129, 129, 39, 33, 32, 111,
            ],
            wz_cols: 3,
            q: 36,
        },
        Mode::B => ScatteredPilots {
            freq_int: 2,
            time_int: 3,
            k0: 1,
            w: &[512, 0, 512, 0, 512, 0, 512, 0, 512, 0, 512, 0, 512, 0, 512],
            z: &[
                0, 57, 164, 64, 12, 168, 255, 161, 106, 118, 25, 232, 132, 233, 38,
            ],
            wz_cols: 5,
            q: 12,
        },
        Mode::E => ScatteredPilots {
            freq_int: 1,
            time_int: 4,
            k0: 1,
            w: &[
                512, 0, 512, 0, 512, 0, 512, 0, 512, 0, 512, 0, 512, 0, 512, 0, 512, 0, 512, 0,
            ],
            z: &[
                0, 57, 164, 64, 12, 168, 255, 161, 106, 118, 25, 232, 132, 233, 38, 168, 255, 161,
                106, 118,
            ],
            wz_cols: 5,
            q: 10,
        },
    }
}

/// Carriers whose scattered pilots are boosted (`TableCarMap.h:161-174`). Zeros never
/// match. Mode A 2.3 kHz lists k = 4, which is never a scattered position, so only
/// three carriers are boosted there; that is what EasyDRF and QSSTV transmit
/// (phy.md §9 item 1).
pub fn boosted_pilots(mode: Mode, bw: Bandwidth) -> [i32; 4] {
    match (mode, bw) {
        (Mode::A, Bandwidth::Narrow) => [2, 4, 50, 54],
        (Mode::A, Bandwidth::Wide) => [2, 6, 54, 58],
        (Mode::B, Bandwidth::Narrow) => [1, 3, 43, 45],
        (Mode::B, Bandwidth::Wide) => [1, 3, 49, 51],
        (Mode::E, Bandwidth::Narrow) => [1, 29, 0, 0],
        (Mode::E, Bandwidth::Wide) => [1, 31, 0, 0],
    }
}

/// MSC code-rate indices per level, `[prot 0, prot 1]` (`TableMLC.h:449-464`).
/// 4-QAM: HamDRM has one rate, 3/5, whatever the protection bit says; here that is
/// protection 1 ("low"), in line with 16- and 64-QAM whose low protection is about
/// 0.6. Protection 0 ("normal") with rate 1/2, like theirs, is DecHAMDRM's own
/// (non-compliant) mode, signalled by `fac::EXT_QAM4_HALF`; EasyPal, EasyDRF and
/// QSSTV cannot receive it.
pub const MSC4_RATES: [[usize; 1]; 2] = [[4], [6]];
pub const MSC16_RATES: [[usize; 2]; 2] = [[2, 7], [4, 9]];
pub const MSC64_RATES: [[usize; 3]; 2] = [[0, 4, 9], [2, 7, 10]];
