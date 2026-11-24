//! `deck-server bridge`: many API connections over one stdin/stdout pair.
//!
//! This is the one way Netrunner Deck connects to any server, on its own
//! machine or another one. Deck runs a bridge per machine (locally as a
//! child process, remotely as `ssh host deck-server bridge`) and opens every
//! API connection as a channel inside it. Each channel is a plain connection
//! to an API socket on the bridge's machine (deck-server's, or any Herdr's),
//! so what flows through it is the server's own JSON-lines protocol,
//! untouched; the bridge adds only the framing. Opening a socket is instant,
//! so one request costs one round trip to the bridge.
//!
//! Framing: one JSON object per line in both directions.
//!
//! * Deck → bridge: `{"c":N,"open":"<socket path>"}`, `{"c":N,"d":"text"}`
//!   (bytes for the socket), `{"c":N,"close":true}`.
//! * bridge → Deck: `{"bridge":1}` once at start (so Deck knows the bridge
//!   is there), `{"c":N,"d":"text"}` (one line the socket sent, with its
//!   newline), `{"c":N,"close":true}` (the socket closed),
//!   `{"c":N,"err":"…","kind":"not_found|refused|denied|busy|other"}` (open failed).
//!
//! The socket path is the server's `herdr.sock` path; on Windows the bridge
//! opens it as the named pipe `\\.\pipe\<path>`, as Herdr names it.
//!
//! The bridge ends when stdin closes (Deck or ssh went away); its channels close.

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use serde_json::{json, Value};

fn kind_name(err: &io::Error) -> &'static str {
    // ERROR_PIPE_BUSY: every instance of the server's pipe is taken.
    if cfg!(windows) && err.raw_os_error() == Some(231) {
        return "busy";
    }
    match err.kind() {
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

/// A connection to a local API socket: its writing half, and the reading
/// half until its pump starts.
struct Conn {
    writer: Box<dyn Write + Send>,
    reader: Option<Box<dyn Read + Send>>,
    closed: Arc<AtomicBool>,
    shutdown: Box<dyn Fn() + Send>,
}

#[cfg(unix)]
fn connect(path: &str) -> io::Result<Conn> {
    use std::os::unix::net::UnixStream;
    let stream = UnixStream::connect(path)?;
    let reader = stream.try_clone()?;
    let stop = stream.try_clone()?;
    Ok(Conn {
        writer: Box::new(stream),
        reader: Some(Box::new(reader)),
        closed: Arc::new(AtomicBool::new(false)),
        shutdown: Box::new(move || {
            let _ = stop.shutdown(std::net::Shutdown::Both);
        }),
    })
}

#[cfg(windows)]
fn connect(path: &str) -> io::Result<Conn> {
    let name = format!(r"\\.\pipe\{path}");
    let mut tries = 0;
    let pipe = loop {
        match std::fs::OpenOptions::new().read(true).write(true).open(&name) {
            // ERROR_PIPE_BUSY: the server has not put up its next instance yet.
            Err(err) if err.raw_os_error() == Some(231) && tries < 40 => {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            other => break other?,
        }
    };
    let reader = pipe.try_clone()?;
    Ok(Conn {
        writer: Box::new(pipe),
        reader: Some(Box::new(reader)),
        closed: Arc::new(AtomicBool::new(false)),
        // A blocked read on a pipe handle cannot be interrupted from here;
        // the pump sees `closed` after its next read and drops the pipe.
        shutdown: Box::new(|| {}),
    })
}

/// Whether a channel's reader starts at open or after the request is sent.
/// A Windows pipe handle opened for synchronous I/O serialises reads and
/// writes, so a read waiting for the reply would hold the request back:
/// there the pump starts once a whole line (the request) has been written.
const READ_AFTER_REQUEST: bool = cfg!(windows);

/// Run until stdin closes.
pub(crate) fn run() -> io::Result<()> {
    let out: Out = Arc::new(Mutex::new(Box::new(io::stdout())));
    send(&out, &json!({ "bridge": 1 }));
    let mut channels: HashMap<u64, Conn> = HashMap::new();
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let mut line = String::new();
    loop {
        line.clear();
        if input.read_line(&mut line)? == 0 {
            break;
        }
        match parse(&line) {
            Frame::Open(c, path) => match connect(&path) {
                Ok(mut conn) => {
                    if !READ_AFTER_REQUEST {
                        start_pump(c, &mut conn, &out);
                    }
                    channels.insert(c, conn);
                }
                Err(err) => send(
                    &out,
                    &json!({ "c": c, "err": err.to_string(), "kind": kind_name(&err) }),
                ),
            },
            Frame::Data(c, d) => {
                let Some(conn) = channels.get_mut(&c) else {
                    continue;
                };
                let written = conn
                    .writer
                    .write_all(d.as_bytes())
                    .and_then(|()| conn.writer.flush());
                if written.is_err() {
                    if let Some(conn) = channels.remove(&c) {
                        close(conn);
                    }
                    send(&out, &json!({ "c": c, "close": true }));
                } else if d.ends_with('\n') {
                    start_pump(c, conn, &out);
                }
            }
            Frame::Close(c) => {
                if let Some(conn) = channels.remove(&c) {
                    close(conn);
                }
            }
            Frame::Bad => {}
        }
    }
    for (_, conn) in channels.drain() {
        close(conn);
    }
    Ok(())
}

fn close(conn: Conn) {
    conn.closed.store(true, Ordering::SeqCst);
    (conn.shutdown)();
}

/// Start forwarding what the socket says (once per channel).
fn start_pump(c: u64, conn: &mut Conn, out: &Out) {
    if let Some(reader) = conn.reader.take() {
        let out = out.clone();
        let closed = conn.closed.clone();
        std::thread::spawn(move || pump(c, reader, &closed, &out));
    }
}

/// Forward what the socket says, line by line, until it closes or Deck
/// closed the channel.
fn pump(c: u64, reader: Box<dyn Read + Send>, closed: &AtomicBool, out: &Out) {
    let mut reader = BufReader::new(reader);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) if closed.load(Ordering::SeqCst) => return,
            Ok(_) => {
                let text = String::from_utf8_lossy(&buf);
                send(out, &json!({ "c": c, "d": text }));
            }
        }
    }
    if !closed.load(Ordering::SeqCst) {
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
        let e = |k| io::Error::from(k);
        assert_eq!(kind_name(&e(io::ErrorKind::NotFound)), "not_found");
        assert_eq!(kind_name(&e(io::ErrorKind::ConnectionRefused)), "refused");
        assert_eq!(kind_name(&e(io::ErrorKind::PermissionDenied)), "denied");
        assert_eq!(kind_name(&e(io::ErrorKind::TimedOut)), "other");
    }

    #[test]
    fn pump_forwards_lines_then_closes() {
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
        let reply: Box<dyn Read + Send> = Box::new(io::Cursor::new(b"{\"echo\":1}\n".to_vec()));
        pump(7, reply, &AtomicBool::new(false), &out);
        let all: String = rx.try_iter().collect();
        assert!(all.contains(r#""c":7"#), "{all}");
        assert!(all.contains(r#"{\"echo\":1}\n"#), "{all}");
        assert!(all.contains(r#""close":true"#), "{all}");
    }
}
