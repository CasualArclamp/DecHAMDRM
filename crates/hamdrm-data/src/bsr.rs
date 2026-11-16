//! BSR — bad segment report: the receiver asks the sender to repeat missing body
//! segments (`docs/spec/app.md` §4; EasyDRF `common/bsr.cpp`, `DABMOT.cpp:1306-1361`;
//! QSSTV `drmrx/sourcedecoder.cpp:726-791`, `drmtx/drmtx.cpp:240-340`).
//!
//! A BSR is a file named `bsr.bin` with ASCII lines:
//!
//! ```text
//! <transport id>
//! H_OK | H_NO          MOT header received or not
//! <segment size>
//! <missing segment numbers, ascending; "a, -1, b" = a..=b>
//! -99                  terminator (optional for EasyDRF)
//! ```

pub const BSR_NAME: &str = "bsr.bin";

/// A repeat request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BsrRequest {
    pub tid: u16,
    pub header_ok: bool,
    pub segment_size: usize,
    /// Missing body segments, ascending.
    pub missing: Vec<usize>,
}

impl BsrRequest {
    /// Serialise. `compress` writes runs of three or more as `a, -1, b` (EasyDRF's GUI
    /// always does; EasyPal's DLL only in "fast" mode). The `-99` terminator is added
    /// as QSSTV does; EasyDRF ignores what follows it.
    pub fn encode(&self, compress: bool) -> Vec<u8> {
        let mut s = format!(
            "{}\n{}\n{}\n",
            self.tid,
            if self.header_ok { "H_OK" } else { "H_NO" },
            self.segment_size
        );
        let m = &self.missing;
        let mut i = 0;
        while i < m.len() {
            let mut j = i;
            while j + 1 < m.len() && m[j + 1] == m[j] + 1 {
                j += 1;
            }
            if compress && j - i >= 2 {
                s += &format!("{}\n-1\n{}\n", m[i], m[j]);
            } else {
                for v in &m[i..=j] {
                    s += &format!("{v}\n");
                }
            }
            i = j + 1;
        }
        s += "-99\n";
        s.into_bytes()
    }

    /// Parse either form; stops at `-99`.
    pub fn decode(data: &[u8]) -> Option<Self> {
        let text = String::from_utf8_lossy(data);
        let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
        let tid: u16 = lines.next()?.parse().ok()?;
        let header_ok = match lines.next()? {
            "H_OK" => true,
            "H_NO" => false,
            _ => return None,
        };
        let segment_size = lines.next()?.parse().ok()?;
        let mut missing: Vec<usize> = Vec::new();
        let mut range = false;
        for l in lines {
            let Ok(v) = l.parse::<i64>() else { break };
            if v == -99 {
                break;
            }
            if v == -1 {
                range = true;
                continue;
            }
            if v < 0 {
                break;
            }
            let v = v as usize;
            if range {
                if let Some(&prev) = missing.last() {
                    missing.extend(prev + 1..=v);
                }
                range = false;
            } else {
                missing.push(v);
            }
        }
        missing.sort_unstable();
        missing.dedup();
        Some(Self {
            tid,
            header_ok,
            segment_size,
            missing,
        })
    }
}

/// Extra copies of a repair transmission for short lists (EasyDRF `bsr.cpp:280-294`).
pub fn repair_copies(instances: usize, segments: usize) -> usize {
    instances
        + usize::from(segments <= 50)
        + usize::from(segments <= 10)
        + usize::from(segments <= 3)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_forms_roundtrip() {
        let r = BsrRequest {
            tid: 41234,
            header_ok: true,
            segment_size: 116,
            missing: vec![1, 2, 3, 4, 7, 9, 10, 20, 21, 22],
        };
        for c in [false, true] {
            assert_eq!(BsrRequest::decode(&r.encode(c)), Some(r.clone()));
        }
        let text = String::from_utf8(r.encode(true)).unwrap();
        assert!(text.contains("1\n-1\n4\n7\n9\n10\n20\n-1\n22\n-99"));
    }
}
