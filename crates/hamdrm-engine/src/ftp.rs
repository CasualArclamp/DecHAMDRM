//! The little FTP that hybrid mode needs: log in, change directory, fetch one file in
//! passive binary mode. The server is named by whoever is transmitting, so nothing it
//! says is trusted: replies and the file are size-limited, everything has a timeout,
//! the data connection goes to the control connection's address (not the one the
//! server announces), and nothing is ever uploaded.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::Duration;

const CONNECT: Duration = Duration::from_secs(15);
const IDLE: Duration = Duration::from_secs(30);
const MAX_REPLY: u64 = 16 * 1024;

/// What to fetch.
pub struct Request<'a> {
    pub host: &'a str,
    pub port: u16,
    pub login: &'a str,
    pub password: &'a str,
    /// Directory to change to (may be empty).
    pub directory: &'a str,
    pub file: &'a str,
    /// Give up on files larger than this.
    pub max_bytes: usize,
}

struct Control {
    reader: BufReader<TcpStream>,
}

impl Control {
    /// Read one reply (possibly multi-line, RFC 959 §4.2): code and last line.
    fn reply(&mut self) -> Result<(u16, String), String> {
        let mut first: Option<String> = None;
        let mut budget = MAX_REPLY;
        loop {
            let mut line = String::new();
            let n = (&mut self.reader)
                .take(budget)
                .read_line(&mut line)
                .map_err(|e| format!("server went quiet: {e}"))?;
            if n == 0 {
                return Err("server closed the connection".into());
            }
            budget = budget.saturating_sub(n as u64);
            if budget == 0 {
                return Err("server reply too long".into());
            }
            let line = line.trim_end().to_string();
            let code = line.get(..3).and_then(|c| c.parse::<u16>().ok());
            match (&first, code) {
                (None, Some(c)) if line.as_bytes().get(3) == Some(&b'-') => {
                    first = Some(format!("{c}"))
                }
                (None, Some(c)) => return Ok((c, line)),
                (None, None) => return Err(format!("not an FTP reply: {line:.60}")),
                (Some(f), Some(_))
                    if line.starts_with(f.as_str()) && line.as_bytes().get(3) == Some(&b' ') =>
                {
                    return Ok((f.parse().unwrap_or(0), line));
                }
                _ => {}
            }
        }
    }

    /// Send a command and require a reply code in `ok`.
    fn cmd(&mut self, line: &str, ok: &[u16], what: &str) -> Result<String, String> {
        if line.contains(['\r', '\n']) {
            return Err(format!("{what}: bad characters"));
        }
        self.reader
            .get_mut()
            .write_all(format!("{line}\r\n").as_bytes())
            .map_err(|e| format!("{what}: {e}"))?;
        let (code, text) = self.reply()?;
        if ok.contains(&code) {
            Ok(text)
        } else {
            // The reply text is the server's; keep it short.
            Err(format!("{what} refused ({code})"))
        }
    }
}

fn connect(addr: SocketAddr) -> Result<TcpStream, String> {
    let s = TcpStream::connect_timeout(&addr, CONNECT).map_err(|e| format!("connect: {e}"))?;
    let _ = s.set_read_timeout(Some(IDLE));
    let _ = s.set_write_timeout(Some(IDLE));
    Ok(s)
}

/// Port from a `227 … (h1,h2,h3,h4,p1,p2)` reply.
fn pasv_port(reply: &str) -> Option<u16> {
    let inner = reply.split_once('(')?.1.split_once(')')?.0;
    let n: Vec<u16> = inner
        .split(',')
        .map(|v| v.trim().parse().ok())
        .collect::<Option<_>>()?;
    (n.len() == 6 && n.iter().all(|v| *v < 256)).then(|| n[4] * 256 + n[5])
}

/// Fetch one file.
pub fn fetch(r: &Request) -> Result<Vec<u8>, String> {
    let addr = (r.host, r.port)
        .to_socket_addrs()
        .map_err(|e| format!("looking up {}: {e}", r.host))?
        .next()
        .ok_or_else(|| format!("{}: no address", r.host))?;
    let mut c = Control {
        reader: BufReader::new(connect(addr)?),
    };
    let (code, _) = c.reply()?;
    if code != 220 {
        return Err(format!("server not ready ({code})"));
    }
    let user = c.cmd(&format!("USER {}", r.login), &[230, 331], "login")?;
    if user.starts_with("331") {
        c.cmd(&format!("PASS {}", r.password), &[230, 202], "password")?;
    }
    c.cmd("TYPE I", &[200], "binary mode")?;
    for part in r.directory.split('/').filter(|p| !p.is_empty()) {
        c.cmd(&format!("CWD {part}"), &[250, 200], "directory")?;
    }
    let pasv = c.cmd("PASV", &[227], "passive mode")?;
    let port = pasv_port(&pasv).ok_or("passive mode: unreadable reply")?;
    let mut data = connect(SocketAddr::new(addr.ip(), port))?;
    c.cmd(&format!("RETR {}", r.file), &[125, 150], "file")?;
    let mut out = Vec::new();
    (&mut data)
        .take(r.max_bytes as u64 + 1)
        .read_to_end(&mut out)
        .map_err(|e| format!("download: {e}"))?;
    drop(data);
    if out.len() > r.max_bytes {
        return Err(format!("file larger than {} bytes", r.max_bytes));
    }
    let (code, _) = c.reply()?;
    if code != 226 && code != 250 {
        return Err(format!("download not completed ({code})"));
    }
    let _ = c.cmd("QUIT", &[221], "quit");
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    /// A one-connection FTP server on localhost holding `/pics/HybridFiles1/a.jpg`.
    fn server(file: Vec<u8>) -> u16 {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (s, _) = l.accept().unwrap();
            let mut w = s.try_clone().unwrap();
            let mut r = BufReader::new(s);
            let mut say = |t: &str| w.write_all(format!("{t}\r\n").as_bytes()).unwrap();
            say("220-hello\r\n220 ready");
            let mut cwd = String::new();
            let mut data: Option<TcpListener> = None;
            loop {
                let mut line = String::new();
                if r.read_line(&mut line).unwrap_or(0) == 0 {
                    break;
                }
                let line = line.trim_end();
                let (cmd, arg) = line.split_once(' ').unwrap_or((line, ""));
                match cmd {
                    "USER" if arg == "sstv" => say("331 password please"),
                    "PASS" if arg == "secret" => say("230 in"),
                    "TYPE" => say("200 ok"),
                    "CWD" => {
                        cwd = format!("{cwd}/{arg}");
                        say("250 ok")
                    }
                    "PASV" => {
                        let d = TcpListener::bind("127.0.0.1:0").unwrap();
                        let p = d.local_addr().unwrap().port();
                        // Announce a wrong address: the client must ignore it.
                        say(&format!("227 passive (10,9,8,7,{},{})", p / 256, p % 256));
                        data = Some(d);
                    }
                    "RETR" if cwd == "/pics/HybridFiles1" && arg == "a.jpg" => {
                        say("150 here it comes");
                        let (mut d, _) = data.take().unwrap().accept().unwrap();
                        d.write_all(&file).unwrap();
                        drop(d);
                        say("226 done");
                    }
                    "QUIT" => {
                        say("221 bye");
                        break;
                    }
                    _ => say("550 no"),
                }
            }
        });
        port
    }

    fn request(port: u16, password: &'static str, max: usize) -> Request<'static> {
        Request {
            host: "127.0.0.1",
            port,
            login: "sstv",
            password,
            directory: "pics/HybridFiles1",
            file: "a.jpg",
            max_bytes: max,
        }
    }

    #[test]
    fn fetches_a_file() {
        let file: Vec<u8> = (0..50_000u32).map(|i| (i * 31 % 251) as u8).collect();
        let port = server(file.clone());
        assert_eq!(fetch(&request(port, "secret", 1 << 20)), Ok(file));
    }

    #[test]
    fn wrong_password_and_too_large() {
        let port = server(vec![1; 10]);
        assert_eq!(
            fetch(&request(port, "wrong", 1 << 20)),
            Err("password refused (550)".into())
        );
        let port = server(vec![1; 5000]);
        assert!(
            fetch(&request(port, "secret", 1000))
                .unwrap_err()
                .contains("larger")
        );
    }

    #[test]
    fn command_injection_is_refused() {
        let port = server(vec![1; 10]);
        let mut r = request(port, "secret", 1000);
        r.file = "a.jpg\r\nDELE a.jpg";
        assert!(fetch(&r).unwrap_err().contains("bad characters"));
    }
}
