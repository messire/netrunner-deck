//! Deck's side of `deck-server bridge` (see `server/src/deck_server/bridge.rs`):
//! one bridge process per machine (`deck-server bridge` here, or
//! `ssh host deck-server bridge`), and every connection to any server socket
//! on that machine is a channel inside it. This is the only way Deck
//! connects to a server (`endpoint.rs`).
//!
//! A channel is opened by sending its `open` frame and the request right
//! after it, without waiting: one request costs one network round trip.
//! What travels in a channel is the server's own JSON-lines protocol.

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use serde_json::{json, Value};

enum Msg {
    Data(Vec<u8>),
    Close,
    Failed(io::ErrorKind, String),
}

/// Why a bridge could not be started.
pub enum StartError {
    /// The machine has no deck-server with a bridge (it ran and said so).
    NoBridge(String),
    /// The bridge could not be run (no ssh, host down, key refused, …).
    Failed(io::Error),
}

pub struct Link {
    child: Mutex<Child>,
    stdin: Mutex<ChildStdin>,
    channels: Mutex<HashMap<u64, Sender<Msg>>>,
    next: AtomicU64,
    alive: AtomicBool,
}

impl Link {
    pub fn alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
            && matches!(
                self.child
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .try_wait(),
                Ok(None)
            )
    }

    fn send(&self, frame: &Value) -> io::Result<()> {
        if !self.alive.load(Ordering::SeqCst) {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "ssh bridge closed",
            ));
        }
        let mut w = self.stdin.lock().unwrap_or_else(PoisonError::into_inner);
        writeln!(w, "{frame}")?;
        w.flush()
    }

    /// A new channel to `socket` on the bridge's host.
    pub fn open(
        self: &Arc<Self>,
        socket: &str,
        read_timeout: Option<Duration>,
    ) -> io::Result<Channel> {
        let c = self.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel();
        self.channels
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(c, tx);
        if let Err(err) = self.send(&json!({ "c": c, "open": socket })) {
            self.channels
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(&c);
            return Err(err);
        }
        Ok(Channel {
            link: self.clone(),
            c,
            rx,
            buf: Vec::new(),
            pos: 0,
            done: false,
            read_timeout,
        })
    }

    pub fn kill(&self) {
        self.alive.store(false, Ordering::SeqCst);
        let mut child = self.child.lock().unwrap_or_else(PoisonError::into_inner);
        let _ = child.kill();
        let _ = child.wait();
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        self.kill();
    }
}

fn kind_of(name: &str) -> io::ErrorKind {
    match name {
        "not_found" => io::ErrorKind::NotFound,
        "refused" => io::ErrorKind::ConnectionRefused,
        "denied" => io::ErrorKind::PermissionDenied,
        _ => io::ErrorKind::Other,
    }
}

/// Run `cmd` (a bridge on machine `name`) and wait for its hello.
pub fn start(
    mut cmd: std::process::Command,
    name: &str,
    timeout: Duration,
) -> Result<Arc<Link>, StartError> {
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(StartError::Failed)?;
    let stdin = child.stdin.take().expect("piped");
    let stdout = child.stdout.take().expect("piped");
    let stderr = child.stderr.take().expect("piped");
    let err_tail = Arc::new(Mutex::new(String::new()));
    {
        let tail = err_tail.clone();
        std::thread::Builder::new()
            .name("deck-bridge-stderr".into())
            .spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    if !line.trim().is_empty() {
                        *tail.lock().unwrap_or_else(PoisonError::into_inner) = line;
                    }
                }
            })
            .ok();
    }
    let link = Arc::new(Link {
        child: Mutex::new(child),
        stdin: Mutex::new(stdin),
        channels: Mutex::new(HashMap::new()),
        next: AtomicU64::new(1),
        alive: AtomicBool::new(true),
    });
    let (hello_tx, hello_rx) = mpsc::channel::<bool>();
    let weak = Arc::downgrade(&link);
    std::thread::Builder::new()
        .name(format!("deck-bridge:{name}"))
        .spawn(move || read_frames(stdout, weak, hello_tx))
        .map_err(StartError::Failed)?;
    let hello = hello_rx.recv_timeout(timeout);
    if matches!(hello, Ok(true)) {
        return Ok(link);
    }
    // No hello: why? Give the process a moment to exit and say so.
    let status = {
        let mut waited = None;
        for _ in 0..20 {
            if let Ok(Some(s)) = link
                .child
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .try_wait()
            {
                waited = Some(s);
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        waited
    };
    link.kill();
    std::thread::sleep(Duration::from_millis(50));
    let why = err_tail
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    let what = |d: &str| {
        format!(
            "bridge on {name}: {}",
            if why.is_empty() { d } else { &why }
        )
    };
    match status.and_then(|s| s.code()) {
        // 255 is ssh's own failure (connect, auth, host key).
        Some(255) => Err(StartError::Failed(io::Error::new(
            io::ErrorKind::ConnectionRefused,
            what("ssh failed"),
        ))),
        Some(_) => Err(StartError::NoBridge(what(
            "no deck-server with a bridge there",
        ))),
        None => Err(StartError::Failed(io::Error::new(
            io::ErrorKind::TimedOut,
            what("no answer"),
        ))),
    }
}

/// The bridge's stdout: hello first, then frames for the channels.
fn read_frames(stdout: impl Read, link: std::sync::Weak<Link>, hello: Sender<bool>) {
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    let mut greeted = false;
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let Ok(v) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        if !greeted {
            greeted = v.get("bridge").is_some();
            let _ = hello.send(greeted);
            if !greeted {
                break;
            }
            continue;
        }
        let Some(link) = link.upgrade() else {
            break;
        };
        let Some(c) = v.get("c").and_then(Value::as_u64) else {
            continue;
        };
        let mut chans = link.channels.lock().unwrap_or_else(PoisonError::into_inner);
        let msg = if let Some(d) = v.get("d").and_then(Value::as_str) {
            Msg::Data(d.as_bytes().to_vec())
        } else if let Some(e) = v.get("err").and_then(Value::as_str) {
            let kind = kind_of(v.get("kind").and_then(Value::as_str).unwrap_or(""));
            Msg::Failed(kind, e.to_string())
        } else {
            Msg::Close
        };
        let last = !matches!(msg, Msg::Data(_));
        if let Some(tx) = chans.get(&c) {
            let _ = tx.send(msg);
        }
        if last {
            chans.remove(&c);
        }
    }
    if !greeted {
        let _ = hello.send(false);
    }
    if let Some(link) = link.upgrade() {
        link.alive.store(false, Ordering::SeqCst);
        // Dropping the senders ends every open channel's reads.
        link.channels
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }
}

/// One connection through the bridge: a byte stream like any other.
pub struct Channel {
    link: Arc<Link>,
    c: u64,
    rx: Receiver<Msg>,
    buf: Vec<u8>,
    pos: usize,
    done: bool,
    read_timeout: Option<Duration>,
}

impl Read for Channel {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        while self.pos >= self.buf.len() {
            if self.done {
                return Ok(0);
            }
            let msg = match self.read_timeout {
                Some(t) => match self.rx.recv_timeout(t) {
                    Ok(m) => m,
                    Err(RecvTimeoutError::Timeout) => {
                        return Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            "no reply through the ssh bridge",
                        ))
                    }
                    Err(RecvTimeoutError::Disconnected) => Msg::Close,
                },
                None => self.rx.recv().unwrap_or(Msg::Close),
            };
            match msg {
                Msg::Data(d) => {
                    self.buf = d;
                    self.pos = 0;
                }
                Msg::Close => self.done = true,
                Msg::Failed(kind, why) => {
                    self.done = true;
                    return Err(io::Error::new(kind, why));
                }
            }
        }
        let n = out.len().min(self.buf.len() - self.pos);
        out[..n].copy_from_slice(&self.buf[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

impl Write for Channel {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        // Deck writes whole JSON text (and its newline): valid UTF-8 pieces.
        self.link
            .send(&json!({ "c": self.c, "d": String::from_utf8_lossy(buf) }))?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for Channel {
    fn drop(&mut self) {
        let open = self
            .link
            .channels
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.c)
            .is_some();
        if open {
            let _ = self.link.send(&json!({ "c": self.c, "close": true }));
        }
    }
}
