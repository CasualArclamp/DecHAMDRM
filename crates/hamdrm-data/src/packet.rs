//! HamDRM packet mode: exactly one packet per 400 ms MSC frame (`docs/spec/app.md` §1;
//! EasyDRF `common/datadecoding/DataDecoder.cpp:36-368`).
//!
//! ```text
//! byte 0 : First 1 | Last 1 | PacketId 2 | PPI 1 | CI 3
//! bytes  : data field, packlen bytes (PPI = 1: count byte, data, 0x00 padding)
//! 2 bytes: CRC-16 over header + data field
//! ```
//!
//! packlen is not transmitted: it is ⌊MSC bits / 8⌋ − 3 and can exceed 255 in mode A
//! 64-QAM, where the 8-bit PPI count wraps (§1.2): we wrap it the same way.

use crate::crc16;

/// Splits data units (MSC data groups) into packets.
#[derive(Debug, Clone)]
pub struct PacketEncoder {
    pub packet_len: usize,
    pub packet_id: u8,
    ci: u8,
}

impl PacketEncoder {
    pub fn new(packet_len: usize, packet_id: u8) -> Self {
        Self {
            packet_len,
            packet_id: packet_id & 3,
            ci: 0,
        }
    }

    /// Packets (each `packet_len + 3` bytes) for one data unit.
    pub fn encode(&mut self, unit: &[u8]) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        let mut pos = 0;
        loop {
            let left = unit.len() - pos;
            let first = pos == 0;
            let last = left <= self.packet_len;
            let ppi = left < self.packet_len;
            let mut p = Vec::with_capacity(self.packet_len + 3);
            p.push(
                (u8::from(first) << 7)
                    | (u8::from(last) << 6)
                    | (self.packet_id << 4)
                    | (u8::from(ppi) << 3)
                    | self.ci,
            );
            self.ci = (self.ci + 1) & 7;
            if ppi {
                p.push(left as u8); // wraps for left ≥ 256, as on air
                p.extend_from_slice(&unit[pos..]);
                p.resize(self.packet_len + 1, 0);
                pos = unit.len();
            } else {
                p.extend_from_slice(&unit[pos..pos + self.packet_len]);
                pos += self.packet_len;
            }
            let crc = crc16(&p);
            p.extend_from_slice(&crc.to_be_bytes());
            out.push(p);
            if last {
                return out;
            }
        }
    }
}

/// Reassembles data units from packets; drops a unit on a CRC failure or a
/// continuity break (EasyDRF `DataDecoder.cpp:256-257, 363-365`).
#[derive(Debug, Clone, Default)]
pub struct PacketDecoder {
    buf: Vec<u8>,
    active: bool,
    next_ci: Option<u8>,
    /// Packets with a bad CRC so far.
    pub crc_errors: u64,
}

impl PacketDecoder {
    /// Feed one MSC frame (`packet_len + 3` bytes, or `None` when the frame was lost).
    /// Returns a completed data unit and the packet id.
    pub fn push(&mut self, frame: Option<&[u8]>) -> Option<(u8, Vec<u8>)> {
        let Some(p) = frame.filter(|p| p.len() >= 4) else {
            self.active = false;
            self.next_ci = None;
            return None;
        };
        let n = p.len();
        if crc16(&p[..n - 2]) != u16::from_be_bytes([p[n - 2], p[n - 1]]) {
            self.crc_errors += 1;
            self.active = false;
            self.next_ci = None;
            return None;
        }
        let h = p[0];
        let (first, last, id, ppi, ci) = (
            h & 0x80 != 0,
            h & 0x40 != 0,
            (h >> 4) & 3,
            h & 8 != 0,
            h & 7,
        );
        let in_order = self.next_ci == Some(ci);
        self.next_ci = Some((ci + 1) & 7);
        let field = &p[1..n - 2];
        let data = if ppi {
            // 8-bit count: a unit tail of 256.. bytes arrives truncated (and then
            // fails its data-group CRC) on every HamDRM receiver; we match that.
            let len = field[0] as usize;
            &field[1..(1 + len).min(field.len())]
        } else {
            field
        };
        if first {
            self.buf.clear();
            self.active = true;
        } else if !in_order || !self.active {
            self.active = false;
            return None;
        }
        self.buf.extend_from_slice(data);
        if last {
            self.active = false;
            return Some((id, std::mem::take(&mut self.buf)));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_various_sizes() {
        for plen in [46, 127, 310] {
            for size in [0, 1, plen - 1, plen, plen + 1, 3 * plen + 7, 2 * plen + 200] {
                let tail = size % plen;
                if (256..plen).contains(&tail) {
                    continue; // the 8-bit count wraps here on every HamDRM receiver
                }
                let unit: Vec<u8> = (0..size).map(|i| (i * 7) as u8).collect();
                let mut enc = PacketEncoder::new(plen, 1);
                let mut dec = PacketDecoder::default();
                let mut got = None;
                for p in enc.encode(&unit) {
                    assert_eq!(p.len(), plen + 3);
                    got = dec.push(Some(&p)).or(got);
                }
                assert_eq!(got, Some((1, unit)), "plen {plen} size {size}");
            }
        }
    }
}
