//! DecHAMDRM application layer: packets, MOT objects, Reed–Solomon file coding and
//! BSR, compatible with EasyPal, EasyDRF and QSSTV (`docs/spec/app.md`).

pub mod bsr;
pub mod mot;
pub mod packet;
pub mod receive;
pub mod rs;
pub mod rsfile;
pub mod send;

/// CRC-16 CCITT (x¹⁶+x¹²+x⁵+1, preset ones, inverted), as for DRM packets and data
/// groups (EasyDRF `common/CRC.cpp`).
pub fn crc16(bytes: &[u8]) -> u16 {
    decdrm_core::fec::crc::crc16(bytes)
}
