//! `deck-server bridge`: many API connections over one stdin/stdout pair.
//!
//! Netrunner Deck reaches a server on another machine by running
//! `ssh host deck-server bridge` once and opening every API connection as a
//! channel inside it. Each channel is a plain connection to a local API
//! socket on this machine (deck-server's, or any Herdr's), so what flows
//! through it is the server's own JSON-lines protocol, untouched. The
//! bridge adds no protocol of its own beyond the framing, and opens the
//! socket instantly, so one request costs one network round trip instead of
//! an SSH channel set-up.
//!
//! Framing: one JSON object per line in both directions.
//!
//! * Deck → bridge: `{"c":N,"open":"/abs/herdr.sock"}`, `{"c":N,"d":"text"}`
//!   (bytes for the socket), `{"c":N,"close":true}`.
//! * bridge → Deck: `{"bridge":1}` once at start (so Deck knows the bridge
//!   exists), `{"c":N,"d":"text"}` (one line the socket sent, with its
//!   newline), `{"c":N,"close":true}` (the socket closed),
//!   `{"c":N,"err":"…","kind":"not_found|refused|denied|other"}` (open failed).
//!
//! The bridge ends when stdin closes (ssh went away); its channels close.

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Write};
use std::sync::{Arc, Mutex, PoisonError};

use serde_json::{json, Value};

/// Run until stdin closes.
pub(crate) fn run() -> io::Result<()> {
    #[cfg(unix)]
    {
        unix::run()
    }
    #[cfg(not(unix))]
    {
        Err(io::Error::other(
            "deck-server bridge runs on Linux and macOS",
        ))
    }
}

fn kind_name(kind: io::ErrorKind) -> &'static str {
    match kind {
        io::ErrorKind::NotFound => "not_found",
        io::ErrorKind::ConnectionRefused => "refused",
        io::ErrorKind::PermissionDenied => "denied",
        _ => "other",
    }
}

/// Where the bridge writes frames: stdout, shared by every channel's reader.
type Out = Arc<Mutex<Box<dyn Write + Send>>>;

fn send(out: &Out, frame: &Value) {
    let mut w = out.lock().unwrap_or_else(PoisonError::into_inner);
    let _ = writeln!(w, "{frame}");
    let _ = w.flush();
}

/// One parsed frame from Deck.
#[derive(Debug, PartialEq)]
enum Frame {
    Open(u64, String),
    Data(u64, String),
    Close(u64),
    Bad,
}

fn parse(line: &str) -> Frame {
    let Ok(v) = serde_json::from_str::<Value>(line.trim()) else {
        return Frame::Bad;
    };
    let Some(c) = v.get("c").and_then(Value::as_u64) else {
        return Frame::Bad;
    };
    if let Some(path) = v.get("open").and_then(Value::as_str) {
        return Frame::Open(c, path.to_string());
    }
    if let Some(d) = v.get("d").and_then(Value::as_str) {
        return Frame::Data(c, d.to_string());
    }
    if v.get("close").is_some() {
        return Frame::Close(c);
    }
    Frame::Bad
}

#[cfg(unix)]
mod unix {
    use super::*;
    use std::net::Shutdown;
    use std::os::unix::net::UnixStream;

    pub(super) fn run() -> io::Result<()> {
        let out: Out = Arc::new(Mutex::new(Box::new(io::stdout())));
        send(&out, &json!({ "bridge": 1 }));
        let mut channels: HashMap<u64, UnixStream> = HashMap::new();
        let stdin = io::stdin();
        let mut line = String::new();
        let mut input = stdin.lock();
        loop {
            line.clear();
            if input.read_line(&mut line)? == 0 {
                break;
            }
            match parse(&line) {
                Frame::Open(c, path) => match UnixStream::connect(&path) {
                    Ok(stream) => {
                        if let Ok(reader) = stream.try_clone() {
                            let out = out.clone();
                            std::thread::spawn(move || pump(c, reader, &out));
                        }
                        channels.insert(c, stream);
                    }
                    Err(err) => send(
                        &out,
                        &json!({ "c": c, "err": err.to_string(), "kind": kind_name(err.kind()) }),
                    ),
                },
                Frame::Data(c, d) => {
                    let failed = match channels.get_mut(&c) {
                        Some(s) => s.write_all(d.as_bytes()).and_then(|()| s.flush()).is_err(),
                        None => false,
                    };
                    if failed {
                        if let Some(s) = channels.remove(&c) {
                            let _ = s.shutdown(Shutdown::Both);
                        }
                    }
                }
                Frame::Close(c) => {
                    if let Some(s) = channels.remove(&c) {
                        let _ = s.shutdown(Shutdown::Both);
                    }
                }
                Frame::Bad => {}
            }
        }
        for (_, s) in channels.drain() {
            let _ = s.shutdown(Shutdown::Both);
        }
        Ok(())
    }

    /// Forward what the socket says, line by line, until it closes.
    pub(super) fn pump(c: u64, stream: UnixStream, out: &Out) {
        let mut reader = BufReader::new(stream);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match reader.read_until(b'\n', &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let text = String::from_utf8_lossy(&buf);
                    send(out, &json!({ "c": c, "d": text }));
                }
            }
        }
        send(out, &json!({ "c": c, "close": true }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_parse() {
        assert_eq!(
            parse(r#"{"c":3,"open":"/h/herdr.sock"}"#),
            Frame::Open(3, "/h/herdr.sock".into())
        );
        assert_eq!(
            parse("{\"c\":3,\"d\":\"{\\\"id\\\":1}\\n\"}\n"),
            Frame::Data(3, "{\"id\":1}\n".into())
        );
        assert_eq!(parse(r#"{"c":3,"close":true}"#), Frame::Close(3));
        assert_eq!(parse("nope"), Frame::Bad);
        assert_eq!(parse(r#"{"open":"/x"}"#), Frame::Bad);
    }

    #[test]
    fn error_kinds_have_names() {
        assert_eq!(kind_name(io::ErrorKind::NotFound), "not_found");
        assert_eq!(kind_name(io::ErrorKind::ConnectionRefused), "refused");
        assert_eq!(kind_name(io::ErrorKind::PermissionDenied), "denied");
        assert_eq!(kind_name(io::ErrorKind::TimedOut), "other");
    }

    #[cfg(unix)]
    #[test]
    fn a_channel_carries_one_request_and_its_reply() {
        use std::os::unix::net::UnixListener;
        let dir = std::env::temp_dir().join(format!("deck-bridge-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let sock = dir.join("s.sock");
        let _ = std::fs::remove_file(&sock);
        let listener = UnixListener::bind(&sock).unwrap();
        std::thread::spawn(move || {
            let (s, _) = listener.accept().unwrap();
            let mut r = BufReader::new(s.try_clone().unwrap());
            let mut l = String::new();
            r.read_line(&mut l).unwrap();
            let mut w = s;
            writeln!(w, "{{\"echo\":{}}}", l.trim()).unwrap();
        });
        let stream = std::os::unix::net::UnixStream::connect(&sock).unwrap();
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        struct Chan(std::sync::mpsc::Sender<String>);
        impl Write for Chan {
            fn write(&mut self, b: &[u8]) -> io::Result<usize> {
                let _ = self.0.send(String::from_utf8_lossy(b).into_owned());
                Ok(b.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let out: Out = Arc::new(Mutex::new(Box::new(Chan(tx))));
        let mut w = stream.try_clone().unwrap();
        writeln!(w, "{{\"id\":1}}").unwrap();
        super::unix::pump(7, stream, &out);
        let all: String = rx.try_iter().collect();
        assert!(all.contains(r#""c":7"#), "{all}");
        assert!(all.contains(r#"{\"echo\":{\"id\":1}}\n"#), "{all}");
        assert!(all.contains(r#""close":true"#), "{all}");
        let _ = std::fs::remove_dir_all(dir);
    }
}
