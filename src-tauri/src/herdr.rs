//! Minimal client for Herdr's newline-delimited JSON API.
//!
//! Herdr listens on `<config dir>/herdr.sock`: a unix socket on unix, and on
//! Windows a named pipe whose name is that full path under `\\.\pipe\`.
//! Every request is one JSON line `{"id","method","params"}`; the reply is one
//! JSON line with either `result` or `error`. `events.subscribe` keeps the
//! connection open and streams one JSON event per line.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::time::Duration;

use serde_json::{json, Value};

#[derive(Debug, thiserror::Error)]
pub enum HerdrError {
    #[error("herdr server is not reachable at {path}: {source}")]
    Connect { path: String, source: std::io::Error },
    #[error("io error talking to herdr: {0}")]
    Io(#[from] std::io::Error),
    #[error("herdr returned an error for {method}: {message}")]
    Api { method: String, message: String },
    #[error("unexpected reply from herdr: {0}")]
    Shape(String),
}

impl serde::Serialize for HerdrError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

pub type Result<T> = std::result::Result<T, HerdrError>;

/// Where the active Herdr server's API endpoint lives.
pub fn socket_path() -> PathBuf {
    if let Ok(explicit) = std::env::var("HERDR_SOCKET_PATH") {
        return PathBuf::from(explicit);
    }
    config_dir().join("herdr.sock")
}

fn config_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("HERDR_CONFIG_DIR") {
        return PathBuf::from(dir);
    }
    #[cfg(windows)]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            return PathBuf::from(appdata).join("herdr");
        }
    }
    #[cfg(not(windows))]
    {
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            return PathBuf::from(xdg).join("herdr");
        }
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(".config").join("herdr");
        }
    }
    PathBuf::from("herdr")
}

#[cfg(windows)]
type Stream = std::fs::File;
#[cfg(not(windows))]
type Stream = std::os::unix::net::UnixStream;

fn connect() -> Result<Stream> {
    let path = socket_path();
    let display = path.to_string_lossy().into_owned();
    #[cfg(windows)]
    {
        let name = format!(r"\\.\pipe\{display}");
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&name)
            .map_err(|source| HerdrError::Connect { path: name, source })
    }
    #[cfg(not(windows))]
    {
        let stream = std::os::unix::net::UnixStream::connect(&path)
            .map_err(|source| HerdrError::Connect { path: display, source })?;
        stream.set_read_timeout(Some(Duration::from_secs(20))).ok();
        Ok(stream)
    }
}

fn next_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(1);
    format!("deck:{}", N.fetch_add(1, Ordering::Relaxed))
}

/// One request/reply exchange. Returns the `result` object.
pub fn call(method: &str, params: Value) -> Result<Value> {
    let mut stream = connect()?;
    let request = json!({ "id": next_id(), "method": method, "params": params });
    stream.write_all(serde_json::to_string(&request)?.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    parse_reply(method, &line)
}

fn parse_reply(method: &str, line: &str) -> Result<Value> {
    let value: Value = serde_json::from_str(line.trim())
        .map_err(|err| HerdrError::Shape(format!("{err}: {}", line.trim())))?;
    if let Some(error) = value.get("error") {
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown error")
            .to_string();
        return Err(HerdrError::Api { method: method.into(), message });
    }
    value
        .get("result")
        .cloned()
        .ok_or_else(|| HerdrError::Shape(format!("no result in reply to {method}")))
}

impl From<serde_json::Error> for HerdrError {
    fn from(err: serde_json::Error) -> Self {
        HerdrError::Shape(err.to_string())
    }
}

/// Open an event subscription and hand every event line to `on_event` until
/// the connection drops or `on_event` returns false.
pub fn subscribe(types: &[&str], mut on_event: impl FnMut(Value) -> bool) -> Result<()> {
    let mut stream = connect()?;
    let subscriptions: Vec<Value> = types.iter().map(|t| json!({ "type": t })).collect();
    let request = json!({
        "id": next_id(),
        "method": "events.subscribe",
        "params": { "subscriptions": subscriptions }
    });
    stream.write_all(serde_json::to_string(&request)?.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            return Ok(());
        }
        let Ok(value) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        // The first line acknowledges the subscription (a normal reply).
        if value.get("result").is_some() && value.get("event").is_none() {
            continue;
        }
        if !on_event(value) {
            return Ok(());
        }
    }
}

/// Read the whole reply body when a method may answer with more than one line
/// (not used by the current methods; kept for `pane.stream` in the fork).
#[allow(dead_code)]
pub fn read_all(mut stream: impl Read) -> std::io::Result<String> {
    let mut out = String::new();
    stream.read_to_string(&mut out)?;
    Ok(out)
}
