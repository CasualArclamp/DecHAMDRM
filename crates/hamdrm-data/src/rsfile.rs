//! EasyPal / QSSTV Reed–Solomon file coding: `name.rs1` … `name.rs4`
//! (`docs/spec/app.md` §3.1–§3.2; QSSTV `utils/reedsolomoncoder.cpp:181-305`).
//!
//! The file is prefixed with a 7-byte header, cut into k-byte chunks (k = 224, 192,
//! 160, 128), each coded to a 255-byte block `[data, parity]`, and the blocks are
//! interleaved byte-wise (block b, byte p sent at p·B + b), so a lost MSC segment
//! becomes a few erasures in every block.

use crate::rs::{N, RsCode};

/// RS level 1..=4 → data bytes per block.
pub fn data_len(level: u8) -> Option<usize> {
    match level {
        1 => Some(224),
        2 => Some(192),
        3 => Some(160),
        4 => Some(128),
        _ => None,
    }
}

/// The EasyPal/QSSTV code: field 0x11D, roots α¹.. (QSSTV `rs.cpp:26`, `rs.h:38-39`).
pub fn code(level: u8) -> Option<RsCode> {
    Some(RsCode::new(0x11D, 1, data_len(level)?))
}

/// RS level of an on-air name ending in `.rs1` … `.rs4`.
pub fn level_of(name: &str) -> Option<u8> {
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    match ext.as_str() {
        "rs1" => Some(1),
        "rs2" => Some(2),
        "rs3" => Some(3),
        "rs4" => Some(4),
        _ => None,
    }
}

/// On-air name: base name up to the first dot + `.rsN` (QSSTV `drmtx.cpp:117-126`).
pub fn coded_name(name: &str, level: u8) -> String {
    let base = name.split('.').next().unwrap_or(name);
    format!("{base}.rs{level}")
}

/// Block interleaver (`distribute`): `tx[p·B + b] = coded[255·b + p]`.
pub fn interleave(coded: &[u8]) -> Vec<u8> {
    let b = coded.len() / N;
    let mut out = vec![0u8; coded.len()];
    for blk in 0..b {
        for p in 0..N {
            out[p * b + blk] = coded[N * blk + p];
        }
    }
    out
}

pub fn deinterleave<T: Copy + Default>(rx: &[T]) -> Vec<T> {
    let b = rx.len() / N;
    let mut out = vec![T::default(); rx.len()];
    for blk in 0..b {
        for p in 0..N {
            out[N * blk + p] = rx[p * b + blk];
        }
    }
    out
}

/// Encode a file: returns the MOT body. `ext` is the original extension (first three
/// characters are kept).
pub fn encode(file: &[u8], ext: &str, level: u8) -> Option<Vec<u8>> {
    let code = code(level)?;
    let k = code.k;
    let got = file.len();
    let blocks = (got + 7).div_ceil(k);
    let mut stream = Vec::with_capacity(blocks * k);
    stream.push((k - got % k) as u8);
    stream.push((blocks % 256) as u8);
    stream.push((blocks / 256) as u8);
    let e = ext.as_bytes();
    for i in 0..3 {
        stream.push(e.get(i).copied().unwrap_or(0));
    }
    stream.push(0);
    stream.extend_from_slice(file);
    stream.resize(blocks * k, 0);
    let mut coded = Vec::with_capacity(blocks * N);
    for chunk in stream.chunks(k) {
        coded.extend_from_slice(chunk);
        coded.extend_from_slice(&code.parity(chunk));
    }
    Some(interleave(&coded))
}

/// Result of decoding an RS-coded body.
#[derive(Debug, Clone)]
pub struct Decoded {
    pub data: Vec<u8>,
    /// Original extension (up to 3 characters).
    pub ext: String,
    pub corrected: usize,
}

/// Decode a body (missing bytes flagged in `erased`). `None` if any block fails.
pub fn decode(body: &[u8], erased: &[bool], level: u8) -> Option<Decoded> {
    let code = code(level)?;
    let k = code.k;
    if body.is_empty() || !body.len().is_multiple_of(N) {
        return None;
    }
    let blocks = body.len() / N;
    let coded = deinterleave(body);
    let er = deinterleave(erased);
    let mut stream = Vec::with_capacity(blocks * k);
    let mut corrected = 0;
    for b in 0..blocks {
        // Memory index j is treated as the coefficient of xʲ: a cyclic shift of the
        // encoder's codeword, hence also a codeword (Karn's decoder does the same).
        let mut blk = coded[b * N..(b + 1) * N].to_vec();
        let pos: Vec<usize> = (0..N).filter(|&j| er[b * N + j]).collect();
        corrected += code.decode(&mut blk, &pos)?;
        stream.extend_from_slice(&blk[..k]);
    }
    let n_blocks = usize::from(stream[1]) + 256 * usize::from(stream[2]);
    if n_blocks != blocks {
        return None;
    }
    let surplus = usize::from(stream[0]);
    // QSSTV's encoder adds a whole extra block when (size mod k) > k − 7; the
    // surplus byte is then < 7, which identifies that case.
    let size = if surplus < 7 {
        (blocks - 1) * k - surplus
    } else {
        blocks * k - surplus
    };
    let ext: String = stream[3..6]
        .iter()
        .take_while(|&&c| c != 0)
        .map(|&c| c as char)
        .collect();
    let data = stream.get(7..7 + size)?.to_vec();
    Some(Decoded {
        data,
        ext,
        corrected,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_with_lost_segments() {
        for level in 1..=4u8 {
            let k = data_len(level).unwrap();
            for size in [0, 1, k - 8, k - 7, k - 6, k - 1, k, 3 * k + 50, 9000] {
                let file: Vec<u8> = (0..size).map(|i| (i * 31 % 253) as u8).collect();
                let body = encode(&file, "jpg", level).unwrap();
                let mut rx = body.clone();
                let mut erased = vec![false; rx.len()];
                // Lose segments of 116 bytes up to the code's erasure budget.
                let budget = (N - k) * (body.len() / N) * 8 / 10;
                let mut lost = 0;
                for seg in (0..rx.len().div_ceil(116)).step_by(3) {
                    let a = seg * 116;
                    let b = (a + 116).min(rx.len());
                    if lost + (b - a) > budget {
                        break;
                    }
                    lost += b - a;
                    rx[a..b].iter_mut().for_each(|v| *v = 0x55);
                    erased[a..b].iter_mut().for_each(|e| *e = true);
                }
                let d = decode(&rx, &erased, level)
                    .unwrap_or_else(|| panic!("level {level} size {size}"));
                assert_eq!(d.data, file, "level {level} size {size}");
                assert_eq!(d.ext, "jpg");
            }
        }
    }

    #[test]
    fn names() {
        assert_eq!(coded_name("pic.big.jpg", 2), "pic.rs2");
        assert_eq!(level_of("pic.RS3"), Some(3));
    }
}
