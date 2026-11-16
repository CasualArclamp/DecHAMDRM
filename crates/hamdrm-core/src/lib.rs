//! DecHAMDRM core: the HamDRM physical layer (the 2.5 kHz DRM variant of EasyPal,
//! EasyDRF/WinDRM and QSSTV) — numerology, cell map, FAC, MSC coding, transmitter and
//! receiver chains — in pure Rust. Format: `docs/spec/phy.md`.
//!
//! Generic DRM pieces (convolutional code, Viterbi, QAM metrics, interleavers, CRC,
//! channel simulator) come from DecDRM's `decdrm-core`.

// Index loops over parallel arrays (carriers, pilots) read like the spec's formulas.
#![allow(clippy::needless_range_loop)]

pub mod cellmap;
pub mod fac;
pub mod mlc;
pub mod params;
pub mod rx;
pub mod tables;
pub mod tx;
pub mod wftext;

pub use decdrm_core::{Cplx, Real};
