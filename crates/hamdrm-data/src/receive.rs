//! MSC frames → received files: packets, MOT reassembly, Reed–Solomon decoding of
//! both schemes (EasyPal/QSSTV `.rsN`, EasyDRF `EasyDRFHeader/|`), LZMA `.lz`
//! unpacking and BSR requests (`docs/spec/app.md` §2.6, §3, §4, §9).

use crate::bsr::{BSR_NAME, BsrRequest};
use crate::mot::{MotDecoder, RxObject};
use crate::packet::PacketDecoder;
use crate::rs::{N, RsCode};
use crate::rsfile;

/// Something the file receiver noticed.
#[derive(Debug, Clone)]
pub enum FileEvent {
    /// Progress of an object (after every new segment).
    Progress { tid: u16, name: Option<String>, received: usize, total: Option<usize> },
    /// A file is complete (name sanitised for saving).
    File { tid: u16, name: String, data: Vec<u8>, rs: Option<String> },
    /// A repeat request (`bsr.bin`) arrived.
    Bsr(BsrRequest),
}

/// The receive side of a file transfer.
#[derive(Debug, Default)]
pub struct FileReceiver {
    packets: PacketDecoder,
    mot: MotDecoder,
}

impl FileReceiver {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn crc_errors(&self) -> u64 {
        self.packets.crc_errors
    }

    pub fn objects(&self) -> &[RxObject] {
        self.mot.objects()
    }

    pub fn object(&self, tid: u16) -> Option<&RxObject> {
        self.mot.get(tid)
    }

    /// A BSR asking for the segments this receiver still misses of object `tid`.
    pub fn bsr_for(&self, tid: u16) -> Option<BsrRequest> {
        let o = self.mot.get(tid)?;
        Some(BsrRequest {
            tid,
            header_ok: o.header.is_some(),
            segment_size: o.segment_size.unwrap_or(116),
            missing: o.missing(),
        })
    }

    /// Feed one MSC frame (`None` = lost frame).
    pub fn push(&mut self, frame: Option<&[u8]>) -> Vec<FileEvent> {
        let mut ev = Vec::new();
        let Some((_, unit)) = self.packets.push(frame) else { return ev };
        let Some(tid) = self.mot.push(&unit) else { return ev };
        let o = self.mot.get(tid).expect("just pushed");
        ev.push(FileEvent::Progress { tid, name: o.name(), received: o.received(), total: o.total });
        if o.delivered {
            return ev;
        }
        if let Some(e) = try_complete(o) {
            if let Some(m) = self.mot.get_mut(tid) {
                m.delivered = true;
            }
            ev.push(e);
        }
        ev
    }
}

/// Try to turn an object into a file (complete, or decodable by its RS code).
fn try_complete(o: &RxObject) -> Option<FileEvent> {
    let name = o.name();
    // EasyPal / QSSTV RS: named *.rsN; decode once enough segments are there.
    if let Some(level) = name.as_deref().and_then(rsfile::level_of)
        && o.total.is_some()
    {
        let (body, erased) = o.body_with_erasures();
        let need = [0.0, 0.88, 0.75, 0.63, 0.50][level as usize];
        let have = erased.iter().filter(|e| !**e).count() as f64 / erased.len().max(1) as f64;
        if have < need && !o.is_complete() {
            return None;
        }
        let d = rsfile::decode(&body, &erased, level)?;
        let base = name.as_deref().unwrap_or("file").split('.').next().unwrap_or("file").to_string();
        let out_name = if d.ext.is_empty() { base } else { format!("{base}.{}", d.ext) };
        return Some(FileEvent::File { tid: o.tid, name: sanitise(&out_name), data: d.data, rs: Some(format!("RS{level}")) });
    }
    // EasyDRF RS: level in the data-group Rfa bits.
    if o.rs_level > 0 && o.total.is_some() {
        let (body, erased) = o.body_with_erasures();
        let need = [0.0, 0.88, 0.75, 0.63, 0.50].get(o.rs_level as usize).copied().unwrap_or(1.0);
        let have = erased.iter().filter(|e| !**e).count() as f64 / erased.len().max(1) as f64;
        if have < need && !o.is_complete() {
            return None;
        }
        let (n, data) = easydrf_decode(&body, &erased, o.rs_level)?;
        let (n, data) = unpack_lz(n, data)?;
        return Some(FileEvent::File { tid: o.tid, name: sanitise(&n), data, rs: Some(format!("EasyDRF RS{}", o.rs_level)) });
    }
    if !o.is_complete() {
        return None;
    }
    let name = name.unwrap_or_else(|| "file".into());
    let body = o.body();
    if name.eq_ignore_ascii_case(BSR_NAME) {
        return BsrRequest::decode(&body).map(FileEvent::Bsr);
    }
    let (name, data) = unpack_lz(name, body)?;
    Some(FileEvent::File { tid: o.tid, name: sanitise(&name), data, rs: None })
}

/// EasyDRF's RS: field 0x187, roots α¹²⁰…, schifra byte order (first byte = highest
/// power), body = interleaved blocks of `EasyDRFHeader/|` + version + size + name +
/// file (app.md §3.4). Returns the name and the file bytes.
pub fn easydrf_decode(body: &[u8], erased: &[bool], level: u8) -> Option<(String, Vec<u8>)> {
    let k = rsfile::data_len(level)?;
    let code = RsCode::new(0x187, 120, k);
    if body.is_empty() || body.len() % N != 0 {
        return None;
    }
    let coded = rsfile::deinterleave(body);
    let er = rsfile::deinterleave(erased);
    let mut stream = Vec::with_capacity(body.len() / N * k);
    for b in 0..body.len() / N {
        // Reverse to x-power order: memory m ↔ x^(254 − m).
        let mut blk: Vec<u8> = coded[b * N..(b + 1) * N].iter().rev().copied().collect();
        let pos: Vec<usize> = (0..N).filter(|&m| er[b * N + m]).map(|m| N - 1 - m).collect();
        code.decode(&mut blk, &pos)?;
        blk.reverse();
        stream.extend_from_slice(&blk[..k]);
    }
    if stream.get(..15)? != b"EasyDRFHeader/|" {
        return None;
    }
    let h = usize::from(stream[16]) | usize::from(stream[17]) << 8;
    let size = usize::from(stream[18]) | usize::from(stream[19]) << 8 | usize::from(stream[20]) << 16;
    let name: String = stream.get(21..h)?.iter().map(|&c| c as char).collect();
    let body = stream.get(h..)?;
    // `size` is the original (pre-LZMA) size; for `.lz` the body is self-sized.
    let data = if name.to_ascii_lowercase().ends_with(".lz") { body.to_vec() } else { body.get(..size)?.to_vec() };
    Some((name, data))
}

/// EasyDRF `.lz`: 5 LZMA property bytes, 3-byte LE original size, raw LZMA stream
/// without end marker (app.md §3.3). Other names pass through.
pub fn unpack_lz(name: String, data: Vec<u8>) -> Option<(String, Vec<u8>)> {
    let Some(stem) = name.strip_suffix(".lz").or_else(|| name.strip_suffix(".LZ")) else {
        return Some((name, data));
    };
    if data.len() < 8 {
        return None;
    }
    let size = usize::from(data[5]) | usize::from(data[6]) << 8 | usize::from(data[7]) << 16;
    let mut input = data[..5].to_vec();
    input.extend_from_slice(&data[8..]);
    let mut out = Vec::new();
    let opts = lzma_rs::decompress::Options {
        unpacked_size: lzma_rs::decompress::UnpackedSize::UseProvided(Some(size as u64)),
        ..Default::default()
    };
    lzma_rs::lzma_decompress_with_options(&mut &input[..], &mut out, &opts).ok()?;
    Some((stem.to_string(), out))
}

/// A file name safe to save: no directories, no reserved characters, not empty.
pub fn sanitise(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let s: String = base
        .chars()
        .map(|c| if c.is_control() || "<>:\"|?*".contains(c) { '_' } else { c })
        .collect();
    let s = s.trim_matches(|c: char| c == '.' || c == ' ').to_string();
    if s.is_empty() { "file".into() } else { s }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::send::{OutFile, SendConfig, frames};

    #[test]
    fn plain_and_rs_with_losses() {
        let data: Vec<u8> = (0..6000u32).map(|i| (i * 7 % 256) as u8).collect();
        for rs in [0u8, 1, 4] {
            let cfg = SendConfig { packet_len: 127, packet_id: 0, rs_level: rs, lead_in: 0 };
            let fr = frames(&[OutFile { name: "pic.jpg".into(), data: data.clone() }], &cfg);
            let mut rx = FileReceiver::new();
            let mut got = None;
            for (i, f) in fr.iter().enumerate() {
                // RS levels lose every 10th frame (header frame 0 kept).
                let lost = rs > 0 && i % 10 == 5;
                for e in rx.push(if lost { None } else { Some(f) }) {
                    if let FileEvent::File { name, data, .. } = e {
                        got = Some((name, data));
                    }
                }
            }
            let (name, d) = got.unwrap_or_else(|| panic!("rs {rs}"));
            assert_eq!(name, "pic.jpg");
            assert_eq!(d, data);
        }
    }

    #[test]
    fn bsr_arrives_as_request() {
        let req = BsrRequest { tid: 1234, header_ok: true, segment_size: 116, missing: vec![3, 4, 5] };
        let cfg = SendConfig { packet_len: 127, packet_id: 0, rs_level: 0, lead_in: 0 };
        let fr = frames(&[OutFile { name: "bsr.bin".into(), data: req.encode(true) }], &cfg);
        let mut rx = FileReceiver::new();
        let mut got = None;
        for f in &fr {
            for e in rx.push(Some(f)) {
                if let FileEvent::Bsr(r) = e {
                    got = Some(r);
                }
            }
        }
        assert_eq!(got, Some(req));
    }

    #[test]
    fn names_are_sanitised() {
        assert_eq!(sanitise("..\\..\\evil.exe"), "evil.exe");
        assert_eq!(sanitise("a/b:c?.jpg"), "b_c_.jpg");
        assert_eq!(sanitise(".."), "file");
    }
}

#[cfg(test)]
mod easydrf_tests {
    use super::*;

    /// Test-only EasyDRF encoder (schifra order: memory m ↔ x^(254 − m)).
    fn encode(name: &str, file: &[u8], level: u8) -> Vec<u8> {
        let k = rsfile::data_len(level).unwrap();
        let code = RsCode::new(0x187, 120, k);
        let mut s = b"EasyDRFHeader/|".to_vec();
        s.push(2);
        let h = 21 + name.len();
        s.extend_from_slice(&(h as u16).to_le_bytes());
        s.extend_from_slice(&(file.len() as u32).to_le_bytes()[..3]);
        s.extend_from_slice(name.as_bytes());
        s.extend_from_slice(file);
        let blocks = s.len().div_ceil(k);
        s.resize(blocks * k, 0);
        let mut coded = Vec::new();
        for chunk in s.chunks(k) {
            let msg: Vec<u8> = chunk.iter().rev().copied().collect();
            let p = code.parity(&msg);
            coded.extend_from_slice(chunk);
            coded.extend(p.iter().rev());
        }
        rsfile::interleave(&coded)
    }

    #[test]
    fn easydrf_rs_roundtrip_with_erasures() {
        let file: Vec<u8> = (0..3000u32).map(|i| (i * 11 % 256) as u8).collect();
        let mut body = encode("x.jpg", &file, 2);
        let mut erased = vec![false; body.len()];
        for i in (0..body.len()).step_by(9) {
            body[i] ^= 0xA5;
            erased[i] = true;
        }
        let (name, data) = easydrf_decode(&body, &erased, 2).unwrap();
        assert_eq!(name, "x.jpg");
        assert_eq!(data, file);
    }
}
