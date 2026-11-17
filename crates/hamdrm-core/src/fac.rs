//! HamDRM Fast Access Channel: the 48-bit block sent in every frame
//! (`docs/spec/phy.md` §4.1; EasyDRF `common/FAC/FAC.cpp:46-227` writer,
//! `:286-777` reader). There is no SDC, so the FAC alone describes the signal.
//!
//! | bits  | field |
//! |-------|-------|
//! | 0–1   | frame identity: frame 0 `11` (or `00`), 1 `01`, 2 `10` |
//! | 2     | spectrum occupancy (0 = 2.3 kHz, 1 = 2.5 kHz) |
//! | 3     | interleaver (0 long, 1 short) |
//! | 4     | MSC mode (0 = 64-QAM, 1 = 16-QAM; also 1 with 4-QAM) |
//! | 5     | protection level |
//! | 6     | 0 audio, 1 data |
//! | 7–8   | data: packet id; audio: coding (0 LPC, 1 CELP, 2 SPEEX, 3 SSTV) |
//! | 9     | data: 4-QAM flag; audio: text flag |
//! | 10–30 | 3 label characters × 7 bits (characters 3f..3f+2 in frame f) |
//! | 31    | audio: 4-QAM flag (EasyDRF 2022), otherwise 0 |
//! | 32–39 | DecHAMDRM extension byte (0 for legacy); EasyDRF and QSSTV never write these bits and ignore them (only the CRC covers them) |
//! | 40–47 | CRC-8 over bits 0–39 |

use crate::params::{Bandwidth, Interleave, Layout, Mode, MscConfig, Qam};
use crate::tables::FAC_BITS;
use decdrm_core::bits::{BitReader, BitWriter};
use decdrm_core::fec::crc::Crc;

/// Longest station label (3 characters per frame × 3 frames).
pub const MAX_LABEL: usize = 9;

/// Digital-voice codec signalled in bits 7–8 of an audio FAC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AudioCoding {
    Lpc,
    Celp,
    Speex,
    Sstv,
}

impl AudioCoding {
    fn bits(self) -> u32 {
        self as u32
    }
    fn from_bits(v: u32) -> Self {
        match v & 3 {
            0 => Self::Lpc,
            1 => Self::Celp,
            2 => Self::Speex,
            _ => Self::Sstv,
        }
    }
}

/// Audio or data service.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Service {
    Data { packet_id: u8 },
    Audio { coding: AudioCoding, text: bool },
}

/// Contents of one FAC block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fac {
    /// Frame index within the super frame, 0..=2.
    pub frame: u8,
    pub bandwidth: Bandwidth,
    pub interleave: Interleave,
    pub qam: Qam,
    pub protection: u8,
    pub service: Service,
    /// The three label characters of this frame (7-bit; 0 = none).
    pub label: [u8; 3],
    /// Extension byte in bits 32–39: 0 = plain HamDRM, [`EXT_NEURAL_VOICE`] = our
    /// neural voice (sent with audio coding CELP, which EasyDRF does not decode).
    pub ext: u8,
}

/// FAC extension byte of DecHAMDRM neural voice ("NV1").
pub const EXT_NEURAL_VOICE: u8 = 0xA1;
/// FAC extension byte of DecHAMDRM's 4-QAM normal protection (rate 1/2 instead of
/// the standard's single 3/5). Legacy receivers decode the FAC, fail the packet CRCs
/// and ignore the MSC.
pub const EXT_QAM4_HALF: u8 = 0xA2;
/// The extension byte is `0xA0` plus flags, so both extensions can be on (`0xA3`).
const EXT_MAGIC: u8 = 0xA0;

/// Whether extension byte `ext` carries the flag of `which` ([`EXT_NEURAL_VOICE`] or
/// [`EXT_QAM4_HALF`]).
pub fn ext_has(ext: u8, which: u8) -> bool {
    ext & 0xF0 == EXT_MAGIC && ext & which & 0x0F != 0
}

impl Fac {
    /// The MSC configuration this FAC describes (the robustness mode comes from the
    /// receiver's mode detection: it is not signalled).
    pub fn msc_config(&self, mode: Mode) -> MscConfig {
        MscConfig {
            layout: Layout::new(mode, self.bandwidth),
            qam: self.qam,
            // 4-QAM: the standard has one rate (3/5, our protection 1) and legacy
            // transmitters set the protection bit freely; only our extension flag
            // selects rate 1/2 (protection 0).
            protection: match self.qam {
                Qam::Qam4 => u8::from(!ext_has(self.ext, EXT_QAM4_HALF)),
                _ => self.protection,
            },
            interleave: self.interleave,
        }
    }

    /// Label characters `3·frame .. 3·frame+3` of `label`.
    pub fn label_part(label: &str, frame: u8) -> [u8; 3] {
        let b = label.as_bytes();
        let mut out = [0u8; 3];
        for (i, o) in out.iter_mut().enumerate() {
            let j = 3 * frame as usize + i;
            if j < b.len().min(MAX_LABEL) {
                *o = b[j] & 127;
            }
        }
        out
    }

    /// Encode to 48 bits (one bit per byte), CRC included.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = BitWriter::new();
        w.write(
            match self.frame {
                0 => 3,
                1 => 1,
                _ => 2,
            },
            2,
        );
        w.write(u32::from(self.bandwidth == Bandwidth::Wide), 1);
        w.write(u32::from(self.interleave == Interleave::Short), 1);
        w.write(u32::from(self.qam != Qam::Qam64), 1);
        w.write(u32::from(self.protection & 1), 1);
        let qam4 = self.qam == Qam::Qam4;
        let mut audio = false;
        match self.service {
            Service::Data { packet_id } => {
                w.write(1, 1);
                w.write(u32::from(packet_id & 3), 2);
                w.write(u32::from(qam4), 1);
            }
            Service::Audio { coding, text } => {
                audio = true;
                w.write(0, 1);
                w.write(coding.bits(), 2);
                w.write(u32::from(text), 1);
            }
        }
        for &c in &self.label {
            w.write(u32::from(c & 127), 7);
        }
        w.write(u32::from(audio && qam4), 1);
        w.write(u32::from(self.ext), 8);
        let mut bits = w.into_bits();
        debug_assert_eq!(bits.len(), FAC_BITS - 8);
        let crc = crc8(&bits);
        for i in (0..8).rev() {
            bits.push((crc >> i) & 1);
        }
        bits
    }

    /// Decode 48 bits; `None` if the CRC fails.
    pub fn decode(bits: &[u8]) -> Option<Self> {
        if bits.len() < FAC_BITS {
            return None;
        }
        let mut r = BitReader::from_bits(&bits[FAC_BITS - 8..FAC_BITS]);
        if r.read(8) as u8 != crc8(&bits[..FAC_BITS - 8]) {
            return None;
        }
        let mut r = BitReader::from_bits(bits);
        let frame = match r.read(2) {
            1 => 1,
            2 => 2,
            _ => 0,
        };
        let bandwidth = Bandwidth::from_bit(r.read_bool());
        let interleave = if r.read_bool() {
            Interleave::Short
        } else {
            Interleave::Long
        };
        let msc16 = r.read_bool();
        let protection = r.read(1) as u8;
        let is_data = r.read_bool();
        let f78 = r.read(2);
        let f9 = r.read_bool();
        let mut label = [0u8; 3];
        for c in &mut label {
            *c = r.read(7) as u8;
        }
        let f31 = r.read_bool();
        let ext = r.read(8) as u8;
        // EasyDRF FAC.cpp:756-762: the 4-QAM flag wins over the MSC-mode bit.
        let (service, qam4) = if is_data {
            (
                Service::Data {
                    packet_id: f78 as u8,
                },
                f9,
            )
        } else {
            (
                Service::Audio {
                    coding: AudioCoding::from_bits(f78),
                    text: f9,
                },
                f31,
            )
        };
        let qam = if qam4 {
            Qam::Qam4
        } else if msc16 {
            Qam::Qam16
        } else {
            Qam::Qam64
        };
        Some(Self {
            frame,
            bandwidth,
            interleave,
            qam,
            protection,
            service,
            label,
            ext,
        })
    }
}

/// DRM CRC-8 (x⁸+x⁴+x³+x²+1, preset ones, inverted) over a bit stream whose length is
/// a multiple of 8 (EasyDRF `CRC.cpp`).
fn crc8(bits: &[u8]) -> u8 {
    let mut c = Crc::crc8();
    for &b in bits {
        c.add_bit(b != 0);
    }
    c.value() as u8
}

/// Assembles the station label from the FAC blocks of a super frame
/// (EasyDRF `FAC.cpp:681-708`: zero characters dropped).
#[derive(Debug, Clone, Default)]
pub struct LabelAssembler {
    parts: [[u8; 3]; 3],
    seen: [bool; 3],
}

impl LabelAssembler {
    pub fn push(&mut self, fac: &Fac) {
        let f = usize::from(fac.frame.min(2));
        self.parts[f] = fac.label;
        self.seen[f] = true;
    }

    /// The label once all three parts have been seen.
    pub fn label(&self) -> Option<String> {
        if !self.seen.iter().all(|&s| s) {
            return None;
        }
        Some(
            self.parts
                .iter()
                .flatten()
                .filter(|&&c| c != 0)
                .map(|&c| c as char)
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_crc() {
        for qam in Qam::ALL {
            for service in [
                Service::Data { packet_id: 2 },
                Service::Audio {
                    coding: AudioCoding::Speex,
                    text: true,
                },
            ] {
                for frame in 0..3 {
                    let f = Fac {
                        frame,
                        bandwidth: Bandwidth::Wide,
                        interleave: Interleave::Short,
                        qam,
                        protection: 1,
                        service,
                        label: Fac::label_part("G4ABCDEFG", frame),
                        ext: 0,
                    };
                    let mut bits = f.encode();
                    assert_eq!(bits.len(), FAC_BITS);
                    assert_eq!(Fac::decode(&bits), Some(f));
                    bits[20] ^= 1;
                    assert_eq!(Fac::decode(&bits), None);
                }
            }
        }
    }

    #[test]
    fn label_assembly() {
        let mut a = LabelAssembler::default();
        for frame in 0..3 {
            a.push(&Fac {
                frame,
                bandwidth: Bandwidth::Narrow,
                interleave: Interleave::Long,
                qam: Qam::Qam16,
                protection: 0,
                service: Service::Data { packet_id: 0 },
                label: Fac::label_part("M0XYZ", frame),
                ext: 0,
            });
        }
        assert_eq!(a.label().as_deref(), Some("M0XYZ"));
    }
}
