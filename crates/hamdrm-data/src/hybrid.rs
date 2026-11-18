//! EasyPal / QSSTV "hybrid" mode: the picture is uploaded to an FTP server and only
//! a small pointer object goes over the air. Its name starts `de_` (or `.de`) and is
//! also the name of the file on the server; its body is the server address, login,
//! password and directory, scrambled.
//!
//! Format after QSSTV `utils/hybridcrypt.cpp` (GPL-3): the text
//! `?host"login<password>directory:` is reversed, written as lower-case hex digits,
//! and each pair of digit characters (n1, n2) becomes two 16-bit big-endian words
//! `4·n1 + 7·n2` and `5·n1 + n2` (`:55-66`), with zero bytes escaped (`:76-86`) and
//! `\r\n` at the end. The picture is `<directory>/HybridFiles1/<object name>` on
//! port 21 (`drmrx/sourcedecoder.cpp:541-543`, `config/hybridconfig.cpp:65`).

/// Folder below the announced directory that holds the pictures.
pub const FILES_DIR: &str = "HybridFiles1";

/// True for the on-air name of a hybrid pointer (QSSTV `sourcedecoder.cpp:525`).
pub fn is_hybrid_name(name: &str) -> bool {
    name.starts_with("de_") || name.starts_with(".de")
}

/// Where a hybrid picture is to be fetched from.
#[derive(Clone, PartialEq, Eq)]
pub struct HybridLink {
    pub host: String,
    pub login: String,
    pub password: String,
    pub directory: String,
}

// The password must not end up in logs.
impl std::fmt::Debug for HybridLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HybridLink({} /{})", self.host, self.directory)
    }
}

const KEY: [i32; 4] = [4, 5, 7, 1];

impl HybridLink {
    /// Unscramble a pointer body (`hybridCrypt::deCrypt`, `:103-175`).
    pub fn decode(body: &[u8]) -> Option<Self> {
        let body = body.strip_suffix(b"\r\n").unwrap_or(body);
        if body.len() < 20 || !body.len().is_multiple_of(4) {
            return None;
        }
        let det = KEY[0] * KEY[3] - KEY[2] * KEY[1];
        let word = |b: &[u8]| -> i32 {
            let (hi, lo) = match (b[0], b[1]) {
                (0xFF, _) => (0, 0),
                (0xFE, lo) => (0, lo),
                (0xFD, v) => (v, 0),
                p => p,
            };
            i32::from(hi) * 256 + i32::from(lo)
        };
        let mut text = Vec::new();
        for quad in body.as_chunks::<4>().0 {
            let (r1, r2) = (word(&quad[..2]), word(&quad[2..]));
            let n1 = (r1 * KEY[3] - r2 * KEY[2]) / det;
            let n2 = (r2 * KEY[0] - r1 * KEY[1]) / det;
            let digit = |n: i32| char::from_u32(n as u32).and_then(|c| c.to_digit(16));
            // Padding (zero characters) decodes to no digit: skip it.
            if let (Some(a), Some(b)) = (digit(n1), digit(n2)) {
                text.push((a * 16 + b) as u8);
            }
        }
        text.reverse();
        let s: String = text.iter().map(|&c| c as char).collect();
        let a = s.find('?')?;
        let b = a + 1 + s[a + 1..].find('"')?;
        let c = b + 1 + s[b + 1..].find('<')?;
        let d = c + 1 + s[c + 1..].find('>')?;
        let e = d + 1 + s[d + 1..].find(':')?;
        let link = Self {
            host: s[a + 1..b].to_string(),
            login: s[b + 1..c].to_string(),
            password: s[c + 1..d].to_string(),
            directory: s[d + 1..e].trim_matches('/').to_string(),
        };
        // A host is a name or address, nothing else; no control characters anywhere
        // (they would inject FTP commands).
        let host_ok = !link.host.is_empty()
            && link
                .host
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
        let clean = |t: &str| t.chars().all(|c| !c.is_control());
        (host_ok && clean(&link.login) && clean(&link.password) && clean(&link.directory))
            .then_some(link)
    }

    /// Scramble (`hybridCrypt::enCrypt`, `:19-98`).
    pub fn encode(&self) -> Vec<u8> {
        let text = format!(
            "?{}\"{}<{}>{}:",
            self.host, self.login, self.password, self.directory
        );
        let hex: Vec<u8> = text
            .bytes()
            .rev()
            .flat_map(|c| format!("{c:02x}").into_bytes())
            .collect();
        let mut out = Vec::new();
        for pair in hex.chunks(2) {
            let (n1, n2) = (i32::from(pair[0]), i32::from(pair[1]));
            for r in [n1 * KEY[0] + n2 * KEY[2], n1 * KEY[1] + n2 * KEY[3]] {
                let (hi, lo) = ((r >> 8) as u8, r as u8);
                match (hi, lo) {
                    (0, 0) => out.extend([0xFF, 0xFF]),
                    (hi, 0) => out.extend([0xFD, hi]),
                    (0xFF, lo) => out.extend([0xFE, lo]),
                    (hi, lo) => out.extend([hi, lo]),
                }
            }
        }
        out.extend(b"\r\n");
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link() -> HybridLink {
        HybridLink {
            host: "ftp.example.org".into(),
            login: "sstv".into(),
            password: "pass word".into(),
            directory: "pics".into(),
        }
    }

    #[test]
    fn round_trip() {
        let body = link().encode();
        assert_eq!(HybridLink::decode(&body), Some(link()));
        // Every character becomes four bytes, as on the air.
        assert_eq!(
            body.len(),
            4 * "?ftp.example.org\"sstv<pass word>pics:".len() + 2
        );
    }

    #[test]
    fn first_bytes_match_qsstv() {
        // ':' = "3a": n1 = '3' (51), n2 = 'a' (97): 4·51 + 7·97 = 883 = 0x0373,
        // 5·51 + 97 = 352 = 0x0160.
        assert_eq!(link().encode()[..4], [0x03, 0x73, 0x01, 0x60]);
    }

    #[test]
    fn padding_and_rubbish() {
        let mut body = link().encode();
        body.truncate(body.len() - 2);
        body.extend([0xFF; 4]);
        body.extend(b"\r\n");
        assert_eq!(HybridLink::decode(&body), Some(link()));
        assert_eq!(
            HybridLink::decode(b"not a pointer at all, just text!"),
            None
        );
        let bad = HybridLink {
            host: "evil host/..".into(),
            ..link()
        };
        assert_eq!(HybridLink::decode(&bad.encode()), None);
    }

    #[test]
    fn names() {
        assert!(is_hybrid_name("de_OE1XXX-5-Clip.jpg"));
        assert!(is_hybrid_name(".de1.jpg"));
        assert!(!is_hybrid_name("picture.jpg"));
    }

    #[test]
    fn debug_hides_the_password() {
        assert!(!format!("{:?}", link()).contains("pass word"));
    }
}
