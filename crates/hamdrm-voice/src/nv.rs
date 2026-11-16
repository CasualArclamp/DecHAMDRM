//! Frame format of DecHAMDRM's neural voice ("NV1"): Mimi codes for 400 ms.
//!
//! Mimi produces 12.5 frames/s, each with up to 32 residual codebooks of 2048 entries
//! (11 bits). A 400 ms MSC frame carries 5 Mimi frames. The codebooks are sent in
//! groups, most important first, each group with its own CRC-8, so a frame whose
//! later groups were damaged still decodes with the codebooks before them:
//!
//! ```text
//! header 8 bits: version 2 (= 1) | codebooks 6
//! group g (sizes 1,1,2,4,4,4,8,8): 5 frames × size × 11 bits, then CRC-8
//!     (the first group's CRC also covers the header)
//! remaining bits: 0
//! ```

/// Mimi frames per 400 ms MSC frame.
pub const FRAMES: usize = 5;
pub const CODE_BITS: usize = 11;
pub const MAX_CODEBOOKS: usize = 32;
const HEADER_BITS: usize = 8;
const VERSION: u32 = 1;
const GROUPS: [usize; 8] = [1, 1, 2, 4, 4, 4, 8, 8];

fn group_bits(g: usize) -> usize {
    FRAMES * g * CODE_BITS + 8
}

/// Codebooks that fit in `t` bits (0 if not even one).
pub fn codebooks_for(t: usize) -> usize {
    let mut used = HEADER_BITS;
    let mut nq = 0;
    for g in GROUPS {
        if used + group_bits(g) > t {
            break;
        }
        used += group_bits(g);
        nq += g;
    }
    nq
}

fn crc8(bits: &[u8]) -> u8 {
    let mut c = decdrm_core::fec::crc::Crc::crc8();
    for &b in bits {
        c.add_bit(b != 0);
    }
    c.value() as u8
}

fn put(bits: &mut Vec<u8>, v: u32, n: usize) {
    for i in (0..n).rev() {
        bits.push(((v >> i) & 1) as u8);
    }
}

fn get(bits: &[u8], pos: &mut usize, n: usize) -> u32 {
    let mut v = 0;
    for _ in 0..n {
        v = (v << 1) | u32::from(bits[*pos]);
        *pos += 1;
    }
    v
}

/// Pack codes `codes[frame][codebook]` (5 frames, `nq` codebooks each, `nq` a group
/// boundary from [`codebooks_for`]) into `t` bits.
pub fn pack(codes: &[Vec<u16>], t: usize) -> Vec<u8> {
    let nq = codebooks_for(t).min(codes.first().map_or(0, Vec::len));
    let mut bits = Vec::with_capacity(t);
    put(&mut bits, VERSION, 2);
    put(&mut bits, nq as u32, 6);
    let mut start = 0;
    let mut group_start = 0;
    for g in GROUPS {
        if start + g > nq {
            break;
        }
        for f in codes.iter().take(FRAMES) {
            for &c in &f[start..start + g] {
                put(&mut bits, u32::from(c), CODE_BITS);
            }
        }
        let crc = crc8(&bits[group_start..]);
        put(&mut bits, u32::from(crc), 8);
        group_start = bits.len();
        start += g;
    }
    bits.resize(t, 0);
    bits
}

/// Unpack: the codes of the leading groups whose CRCs are good, `codes[frame][k]`
/// with k < the number of good codebooks; `None` if the header or the first group is
/// bad.
pub fn unpack(bits: &[u8]) -> Option<Vec<Vec<u16>>> {
    if bits.len() < HEADER_BITS {
        return None;
    }
    let mut pos = 0;
    if get(bits, &mut pos, 2) != VERSION {
        return None;
    }
    let nq = get(bits, &mut pos, 6) as usize;
    let mut codes = vec![Vec::new(); FRAMES];
    let mut start = 0;
    let mut group_start = 0;
    for g in GROUPS {
        if start + g > nq || pos + group_bits(g) > bits.len() {
            break;
        }
        let mut grp = vec![Vec::new(); FRAMES];
        for f in grp.iter_mut() {
            for _ in 0..g {
                f.push(get(bits, &mut pos, CODE_BITS) as u16);
            }
        }
        let want = crc8(&bits[group_start..pos]);
        if get(bits, &mut pos, 8) as u8 != want {
            break;
        }
        group_start = pos;
        for (c, g) in codes.iter_mut().zip(grp) {
            c.extend(g);
        }
        start += g;
    }
    if codes[0].is_empty() {
        None
    } else {
        Some(codes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capacities() {
        assert_eq!(codebooks_for(537), 8); // B 2.3 kHz 4-QAM
        assert_eq!(codebooks_for(1047), 16); // B 2.5 kHz 16-QAM p0
        assert_eq!(codebooks_for(2511), 32); // A 2.5 kHz 64-QAM p1
        assert_eq!(codebooks_for(60), 0);
    }

    #[test]
    fn roundtrip_and_partial() {
        let codes: Vec<Vec<u16>> = (0..5)
            .map(|f| {
                (0..16)
                    .map(|k| ((f * 131 + k * 17) % 2048) as u16)
                    .collect()
            })
            .collect();
        let bits = pack(&codes, 1047);
        assert_eq!(bits.len(), 1047);
        assert_eq!(unpack(&bits).unwrap(), codes);
        // Damage the last group: the first 12 codebooks survive.
        let mut b = bits.clone();
        let last_group_start = 8 + [1, 1, 2, 4, 4]
            .iter()
            .map(|&g| group_bits(g))
            .sum::<usize>();
        b[last_group_start + 3] ^= 1;
        let got = unpack(&b).unwrap();
        assert_eq!(got[0].len(), 12);
        assert_eq!(got[2][..], codes[2][..12]);
        // Damage the first group: nothing.
        let mut b = bits;
        b[10] ^= 1;
        assert!(unpack(&b).is_none());
    }
}
