//! Files → MSC frames: the transmit side of a picture/file transfer
//! (MOT objects → data groups → one packet per frame).

use crate::mot::{MotEncoder, MotObject, segment_size};
use crate::packet::PacketEncoder;
use crate::rsfile;

/// How to send files.
#[derive(Debug, Clone)]
pub struct SendConfig {
    /// Packet data-field length (MSC frame bytes − 3).
    pub packet_len: usize,
    pub packet_id: u8,
    /// EasyPal/QSSTV RS level 1..=4, or 0 for plain.
    pub rs_level: u8,
    /// Body segments sent ahead of the first file (EasyPal default 9, long 16).
    pub lead_in: usize,
}

/// One file to send: name and contents.
#[derive(Debug, Clone)]
pub struct OutFile {
    pub name: String,
    pub data: Vec<u8>,
}

/// The MOT object actually sent for a file (RS-coded and renamed when asked).
pub fn object_for(file: &OutFile, rs_level: u8) -> MotObject {
    if rs_level > 0 && !file.name.eq_ignore_ascii_case("bsr.bin") {
        let ext = file.name.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
        if let Some(body) = rsfile::encode(&file.data, ext, rs_level) {
            return MotObject::new(&rsfile::coded_name(&file.name, rs_level), body);
        }
    }
    MotObject::new(&file.name, file.data.clone())
}

/// All MSC frames (each `packet_len + 3` bytes) for a transmission of `files`.
pub fn frames(files: &[OutFile], cfg: &SendConfig) -> Vec<Vec<u8>> {
    let objects: Vec<MotObject> = files.iter().map(|f| object_for(f, cfg.rs_level)).collect();
    let mut mot = MotEncoder::new(segment_size(cfg.packet_len));
    let mut pk = PacketEncoder::new(cfg.packet_len, cfg.packet_id);
    mot.transmission(&objects, cfg.lead_in).iter().flat_map(|g| pk.encode(g)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mot::MotDecoder;
    use crate::packet::PacketDecoder;

    #[test]
    fn frames_decode_back_to_the_file() {
        let data: Vec<u8> = (0..3000u32).map(|i| (i % 255) as u8).collect();
        for rs in [0, 2] {
            let cfg = SendConfig { packet_len: 127, packet_id: 0, rs_level: rs, lead_in: 9 };
            let f = OutFile { name: "photo.jpg".into(), data: data.clone() };
            let mut pd = PacketDecoder::default();
            let mut md = MotDecoder::default();
            let mut tid = None;
            for fr in frames(std::slice::from_ref(&f), &cfg) {
                assert_eq!(fr.len(), 130);
                if let Some((_, unit)) = pd.push(Some(&fr)) {
                    tid = md.push(&unit).or(tid);
                }
            }
            let o = md.get(tid.unwrap()).unwrap();
            assert!(o.is_complete());
            let name = o.name().unwrap();
            let body = if rs > 0 {
                assert_eq!(name, "photo.rs2");
                let (b, e) = o.body_with_erasures();
                rsfile::decode(&b, &e, rs).unwrap().data
            } else {
                assert_eq!(name, "photo.jpg");
                o.body()
            };
            assert_eq!(body, data);
        }
    }
}
