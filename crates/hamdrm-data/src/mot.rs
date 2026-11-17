//! MOT objects over MSC data groups, the way EasyPal / EasyDRF / QSSTV send files
//! (`docs/spec/app.md` §2; EasyDRF `common/datadecoding/DABMOT.cpp`, QSSTV
//! `drmtx/common/datadecoding/DABMOT.cpp`). Header mode only, header never segmented,
//! transport id = filename hash.

use crate::crc16;

/// Body segment size for a packet length (EasyDRF `DataDecoder.cpp:157`): one full
/// body data group (11 bytes of overhead) fills one packet exactly.
pub fn segment_size(packet_len: usize) -> usize {
    packet_len - 11
}

/// Longest file name sent (`DABMOT.cpp:72-74`).
pub const MAX_NAME: usize = 80;

/// Transport id from the file name as sent (EasyDRF `DABMOT.cpp:86-99`, QSSTV
/// `DABMOT.cpp:114-123`).
pub fn transport_id(name: &[u8]) -> u16 {
    let name = &name[..name.len().min(MAX_NAME)];
    let (mut x, mut a) = (0u8, 0u8);
    for (i, &c) in name.iter().enumerate() {
        x ^= c;
        a = a.wrapping_add(c);
        a ^= i as u8;
    }
    let tid = 256 * u16::from(a) + u16::from(x);
    if tid <= 2 {
        tid + name.len() as u16
    } else {
        tid
    }
}

/// File name bytes: Latin-1 (QSSTV decodes it so; EasyDRF writes ANSI bytes).
pub fn name_bytes(name: &str) -> Vec<u8> {
    name.chars()
        .map(|c| if (c as u32) < 256 { c as u8 } else { b'_' })
        .take(MAX_NAME)
        .collect()
}

pub fn name_from_bytes(b: &[u8]) -> String {
    b.iter().map(|&c| c as char).collect()
}

/// One MSC data group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataGroup {
    /// 3 = MOT header, 4 = MOT body.
    pub kind: u8,
    pub ci: u8,
    /// Repetition index; EasyDRF RS modes carry the RS block count here (§2.1).
    pub rep_idx: u8,
    pub last: bool,
    pub segment: u16,
    /// User-access Rfa bits; EasyDRF carries its RS level here.
    pub rfa: u8,
    pub tid: u16,
    pub data: Vec<u8>,
}

impl DataGroup {
    /// Serialise with the segmentation header and CRC (§2.1).
    pub fn encode(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(self.data.len() + 11);
        b.push(0b0111_0000 | (self.kind & 15));
        b.push(((self.ci & 15) << 4) | (self.rep_idx & 15));
        b.extend_from_slice(
            &((u16::from(self.last) << 15) | (self.segment & 0x7FFF)).to_be_bytes(),
        );
        b.push(((self.rfa & 7) << 5) | 0b1_0010);
        b.extend_from_slice(&self.tid.to_be_bytes());
        b.extend_from_slice(&((self.data.len() as u16) & 0x1FFF).to_be_bytes());
        b.extend_from_slice(&self.data);
        let crc = crc16(&b);
        b.extend_from_slice(&crc.to_be_bytes());
        b
    }

    /// Parse; `None` on a bad CRC or a layout we do not use.
    pub fn decode(b: &[u8]) -> Option<Self> {
        let n = b.len();
        if n < 11 || crc16(&b[..n - 2]) != u16::from_be_bytes([b[n - 2], b[n - 1]]) {
            return None;
        }
        let ext = b[0] & 0x80 != 0;
        let seg_flag = b[0] & 0x20 != 0;
        let ua_flag = b[0] & 0x10 != 0;
        let kind = b[0] & 15;
        let mut p = if ext { 4 } else { 2 };
        if !seg_flag || !ua_flag {
            return None;
        }
        let seg = u16::from_be_bytes([b[p], b[p + 1]]);
        p += 2;
        let rfa = b[p] >> 5;
        let tid_flag = b[p] & 0x10 != 0;
        let len_ind = (b[p] & 15) as usize;
        p += 1;
        if !tid_flag || len_ind < 2 || p + len_ind > n - 2 {
            return None;
        }
        let tid = u16::from_be_bytes([b[p], b[p + 1]]);
        p += len_ind;
        if p + 2 > n - 2 {
            return None;
        }
        let size = (u16::from_be_bytes([b[p], b[p + 1]]) & 0x1FFF) as usize;
        p += 2;
        let data = b[p..(p + size).min(n - 2)].to_vec();
        Some(Self {
            kind,
            ci: b[1] >> 4,
            rep_idx: b[1] & 15,
            last: seg & 0x8000 != 0,
            segment: seg & 0x7FFF,
            rfa,
            tid,
            data,
        })
    }
}

/// MOT header core + TriggerTime + VersionNumber + ContentName (§2.2).
pub fn encode_header(body_size: usize, name: &[u8], content_type: u8, sub_type: u16) -> Vec<u8> {
    let name = &name[..name.len().min(MAX_NAME)];
    let header_size = 17 + name.len();
    let core: u64 = ((body_size as u64 & 0x0FFF_FFFF) << 28)
        | ((header_size as u64 & 0x1FFF) << 15)
        | ((u64::from(content_type) & 0x3F) << 9)
        | (u64::from(sub_type) & 0x1FF);
    let mut h = core.to_be_bytes()[1..].to_vec();
    h.extend_from_slice(&[0x85, 0, 0, 0, 0]); // TriggerTime "now"
    h.extend_from_slice(&[0x46, 0]); // VersionNumber 0
    h.push(0xCC); // ContentName, PLI 3
    h.push((1 + name.len()) as u8);
    h.push(0); // charset 0
    h.extend_from_slice(name);
    debug_assert_eq!(h.len(), header_size);
    h
}

/// Parsed MOT header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MotHeader {
    pub body_size: usize,
    pub content_type: u8,
    pub sub_type: u16,
    pub name: Vec<u8>,
}

pub fn parse_header(h: &[u8]) -> Option<MotHeader> {
    if h.len() < 7 {
        return None;
    }
    let mut core = [0u8; 8];
    core[1..].copy_from_slice(&h[..7]);
    let core = u64::from_be_bytes(core);
    let body_size = (core >> 28) as usize & 0x0FFF_FFFF;
    let header_size = ((core >> 15) & 0x1FFF) as usize;
    let mut out = MotHeader {
        body_size,
        content_type: ((core >> 9) & 0x3F) as u8,
        sub_type: (core & 0x1FF) as u16,
        name: Vec::new(),
    };
    let end = header_size.min(h.len());
    let mut p = 7;
    while p < end {
        let pli = h[p] >> 6;
        let id = h[p] & 0x3F;
        p += 1;
        let len = match pli {
            0 => 0,
            1 => 1,
            2 => 4,
            _ => {
                let ext = h.get(p)? & 0x80 != 0;

                if ext {
                    let v = (usize::from(*h.get(p)? & 0x7F) << 8) | usize::from(*h.get(p + 1)?);
                    p += 2;
                    v
                } else {
                    let v = usize::from(*h.get(p)? & 0x7F);
                    p += 1;
                    v
                }
            }
        };
        let field = h.get(p..p + len)?;
        if id == 12 && !field.is_empty() {
            out.name = field[1..].to_vec();
        }
        p += len;
    }
    Some(out)
}

/// A file to send.
#[derive(Debug, Clone)]
pub struct MotObject {
    pub name: Vec<u8>,
    pub body: Vec<u8>,
    pub tid: u16,
}

impl MotObject {
    pub fn new(name: &str, body: Vec<u8>) -> Self {
        let name = name_bytes(name);
        let tid = transport_id(&name);
        Self { name, body, tid }
    }
}

/// Produces the data groups of a transmission: a lead-in object (header, the first
/// `lead_in` body segments and the last segment), then each object in full (EasyDRF
/// `MOTSlideShow.cpp:256-348`, `DABMOT.cpp:566-643`).
#[derive(Debug, Clone)]
pub struct MotEncoder {
    pub segment_size: usize,
    ci: [u8; 2],
}

impl MotEncoder {
    pub fn new(segment_size: usize) -> Self {
        Self {
            segment_size,
            ci: [0; 2],
        }
    }

    fn group(
        &mut self,
        kind: u8,
        obj: &MotObject,
        segment: u16,
        last: bool,
        data: &[u8],
    ) -> Vec<u8> {
        let c = &mut self.ci[usize::from(kind == 4)];
        let g = DataGroup {
            kind,
            ci: *c,
            rep_idx: 0,
            last,
            segment,
            rfa: 0,
            tid: obj.tid,
            data: data.to_vec(),
        };
        *c = (*c + 1) & 15;
        g.encode()
    }

    /// Number of body segments of an object (at least one, possibly empty).
    pub fn segments(&self, obj: &MotObject) -> usize {
        obj.body.len().div_ceil(self.segment_size).max(1)
    }

    fn segment<'a>(&self, obj: &'a MotObject, i: usize) -> &'a [u8] {
        let a = (i * self.segment_size).min(obj.body.len());
        let b = ((i + 1) * self.segment_size).min(obj.body.len());
        &obj.body[a..b]
    }

    /// Data groups for the header and the given body segments (ascending; the last
    /// segment is always added, as EasyDRF does).
    pub fn object(&mut self, obj: &MotObject, which: &[usize]) -> Vec<Vec<u8>> {
        let n = self.segments(obj);
        let header = encode_header(obj.body.len(), &obj.name, 2, 1);
        let mut out = vec![self.group(3, obj, 0, true, &header)];
        let mut list: Vec<usize> = which.iter().copied().filter(|&i| i < n).collect();
        if list.last() != Some(&(n - 1)) {
            list.push(n - 1);
        }
        for i in list {
            let data = self.segment(obj, i).to_vec();
            out.push(self.group(4, obj, i as u16, i == n - 1, &data));
        }
        out
    }

    /// Whole transmission of `objects` with a lead-in of `lead_in` segments.
    pub fn transmission(&mut self, objects: &[MotObject], lead_in: usize) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        // Lead-in: copies of the first object's header, first segments and last
        // segment until `lead_in` body segments have gone out, so a receiver has
        // time to lock before the real object starts (EasyDRF
        // `MOTSlideShow.cpp:256-307` repeats the leader for small files the same way).
        if let Some(first) = objects.first() {
            let k: Vec<usize> = (0..lead_in.min(self.segments(first))).collect();
            let mut sent = 0;
            while sent < lead_in {
                let groups = self.object(first, &k);
                sent += groups.len() - 1;
                out.extend(groups);
            }
        }
        for o in objects {
            let all: Vec<usize> = (0..self.segments(o)).collect();
            out.extend(self.object(o, &all));
        }
        out
    }
}

/// A received object (complete or not).
#[derive(Debug, Clone, Default)]
pub struct RxObject {
    pub tid: u16,
    pub header: Option<MotHeader>,
    pub segment_size: Option<usize>,
    /// Body segments by number.
    pub segments: Vec<Option<Vec<u8>>>,
    /// Total segments once the last one has been seen.
    pub total: Option<usize>,
    /// EasyDRF RS signalling seen in the data groups (Rfa level, RepIdx nibbles).
    pub rs_level: u8,
    pub rep_nibbles: [Option<u8>; 4],
    pub delivered: bool,
}

impl RxObject {
    pub fn received(&self) -> usize {
        self.segments.iter().filter(|s| s.is_some()).count()
    }

    pub fn missing(&self) -> Vec<usize> {
        let n = self.total.unwrap_or(self.segments.len());
        (0..n)
            .filter(|&i| self.segments.get(i).is_none_or(|s| s.is_none()))
            .collect()
    }

    pub fn is_complete(&self) -> bool {
        self.header.is_some() && self.total.is_some() && self.missing().is_empty()
    }

    pub fn name(&self) -> Option<String> {
        self.header.as_ref().map(|h| name_from_bytes(&h.name))
    }

    /// Body with missing segments as `None` ranges filled with zeros, plus a per-byte
    /// erasure map (for Reed–Solomon decoding).
    pub fn body_with_erasures(&self) -> (Vec<u8>, Vec<bool>) {
        let seg = self.segment_size.unwrap_or(0);
        let n = self.total.unwrap_or(self.segments.len());
        let size = self.header.as_ref().map(|h| h.body_size).unwrap_or(n * seg);
        let mut body = vec![0u8; size];
        let mut erased = vec![true; size];
        for (i, s) in self.segments.iter().enumerate().take(n) {
            if let Some(s) = s {
                let a = (i * seg).min(size);
                let b = (a + s.len()).min(size);
                body[a..b].copy_from_slice(&s[..b - a]);
                erased[a..b].iter_mut().for_each(|e| *e = false);
            }
        }
        (body, erased)
    }

    pub fn body(&self) -> Vec<u8> {
        self.body_with_erasures().0
    }

    /// Number of body segments received without a gap from the start.
    pub fn leading_segments(&self) -> usize {
        self.segments.iter().take_while(|s| s.is_some()).count()
    }

    /// The start of the body up to the first missing segment (for showing a picture
    /// while it arrives).
    pub fn leading_bytes(&self) -> Vec<u8> {
        let mut out: Vec<u8> = self
            .segments
            .iter()
            .map_while(|s| s.as_deref())
            .flatten()
            .copied()
            .collect();
        if let Some(h) = &self.header {
            out.truncate(h.body_size);
        }
        out
    }
}

/// Collects data groups into objects; keeps the last few transport ids so that
/// retransmissions and BSR repairs merge (EasyDRF's 5-entry picture pool).
#[derive(Debug, Clone, Default)]
pub struct MotDecoder {
    pool: Vec<RxObject>,
}

/// Objects kept per decoder.
const POOL: usize = 6;

impl MotDecoder {
    /// Feed a data group; returns the transport id it touched.
    pub fn push(&mut self, raw: &[u8]) -> Option<u16> {
        let g = DataGroup::decode(raw)?;
        if g.kind != 3 && g.kind != 4 {
            return None;
        }
        let idx = match self.pool.iter().position(|o| o.tid == g.tid) {
            Some(i) => i,
            None => {
                if self.pool.len() == POOL {
                    self.pool.remove(0);
                }
                self.pool.push(RxObject {
                    tid: g.tid,
                    ..Default::default()
                });
                self.pool.len() - 1
            }
        };
        // Most recent at the back.
        let mut o = self.pool.remove(idx);
        if g.kind == 3 {
            if g.segment == 0
                && let Some(h) = parse_header(&g.data)
            {
                o.header = Some(h);
            }
        } else {
            let i = g.segment as usize;
            if !g.last {
                if o.segment_size.is_some_and(|s| s != g.data.len()) {
                    // Segment size changed (new mode): start over.
                    o = RxObject {
                        tid: g.tid,
                        header: o.header.take(),
                        ..Default::default()
                    };
                }
                o.segment_size = Some(g.data.len());
            }
            if g.last {
                o.total = Some(i + 1);
            }
            if o.segments.len() <= i {
                o.segments.resize(i + 1, None);
            }
            o.segments[i] = Some(g.data);
            o.rs_level = o.rs_level.max(g.rfa);
            o.rep_nibbles[i % 4] = Some(g.rep_idx);
        }
        self.pool.push(o);
        Some(g.tid)
    }

    pub fn get(&self, tid: u16) -> Option<&RxObject> {
        self.pool.iter().find(|o| o.tid == tid)
    }

    pub fn get_mut(&mut self, tid: u16) -> Option<&mut RxObject> {
        self.pool.iter_mut().find(|o| o.tid == tid)
    }

    pub fn objects(&self) -> &[RxObject] {
        &self.pool
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_roundtrip() {
        let h = encode_header(12345, b"picture.jpg", 2, 1);
        assert_eq!(h.len(), 17 + 11);
        let p = parse_header(&h).unwrap();
        assert_eq!(
            (p.body_size, p.content_type, p.sub_type, p.name.as_slice()),
            (12345, 2, 1, &b"picture.jpg"[..])
        );
    }

    #[test]
    fn tid_small_values_shift() {
        assert!(transport_id(b"bsr.bin") > 2);
        assert_eq!(transport_id(b"a"), 256 * u16::from(b'a') + u16::from(b'a'));
    }

    #[test]
    fn object_roundtrip_with_lead_in() {
        let body: Vec<u8> = (0..5000u32).map(|i| (i * 13 % 251) as u8).collect();
        let obj = MotObject::new("test.jpg", body.clone());
        let mut enc = MotEncoder::new(116);
        let groups = enc.transmission(std::slice::from_ref(&obj), 9);
        let mut dec = MotDecoder::default();
        for g in &groups {
            dec.push(g);
        }
        let o = dec.get(obj.tid).unwrap();
        assert!(o.is_complete());
        assert_eq!(o.body(), body);
        assert_eq!(o.name().as_deref(), Some("test.jpg"));
    }
}
