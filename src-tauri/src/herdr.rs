//! Minimal client for Herdr's newline-delimited JSON API.
//!
//! Herdr listens on `<config dir>/herdr.sock`: a unix socket on unix, and on
//! Windows a named pipe whose name is that full path under `\\.\pipe\`.
//! Every request is one JSON line `{"id","method","params"}`; the reply is one
//! JSON line with either `result` or `error`. `events.subscribe` answers with
//! that same ack line, then keeps the connection open and streams one JSON
//! event per line (`{"event":"pane_updated", …}`, snake_case).
//!
//! Blocking discipline (roadmap R3): every [`call`] runs its socket I/O on a
//! helper thread and the caller waits at most [`CALL_TIMEOUT`]; at most
//! [`MAX_CONCURRENT_CALLS`] request/reply connections are open at once so the
//! Windows named-pipe listener is not hit with `ERROR_PIPE_BUSY`. The
//! long-lived [`subscribe`] connection is outside both limits.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::sync::{mpsc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

/// Upper bound for one request/reply exchange, including waiting for a
/// connection slot. Applies on every platform: `std::fs::File` has no read
/// timeout on Windows, so the deadline is enforced by the caller instead.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(15);

/// How many request/reply connections may be open at the same time.
/// Herdr's listener is an `interprocess` local socket; on Windows a client
/// that connects while no pipe instance is free gets `ERROR_PIPE_BUSY`.
const MAX_CONCURRENT_CALLS: usize = 2;

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
    #[error("timed out waiting for herdr to answer {method}")]
    Timeout { method: String },
}

impl serde::Serialize for HerdrError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
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

/// Socket-level read timeout for request/reply connections. Honoured on
/// unix only: `std::fs::File` has no such knob on Windows, where
/// `run_with_deadline` covers it.
const CALL_READ_TIMEOUT: Duration = Duration::from_secs(20);

/// Connection for one request/reply exchange (see [`call`]).
fn connect() -> Result<Stream> {
    connect_with(Some(CALL_READ_TIMEOUT))
}

/// Connect to Herdr. `read_timeout` is applied only where the platform
/// stream supports it (unix); `None` leaves reads unbounded, which the
/// long-lived event stream needs — an idle subscription must not be torn
/// down just because nothing happened for a while.
fn connect_with(read_timeout: Option<Duration>) -> Result<Stream> {
    let path = socket_path();
    let display = path.to_string_lossy().into_owned();
    #[cfg(windows)]
    {
        let _ = read_timeout;
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
        // `None` is the default (block forever); setting it is a no-op.
        stream.set_read_timeout(read_timeout).ok();
        Ok(stream)
    }
}

fn next_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(1);
    format!("deck:{}", N.fetch_add(1, Ordering::Relaxed))
}

/// Counting semaphore over `std::sync` only (no extra crates). A slot is held
/// for the lifetime of a [`SlotGuard`].
struct Semaphore {
    capacity: usize,
    used: Mutex<usize>,
    freed: Condvar,
}

/// Releases its slot on drop, wherever the guard ends up (including on an
/// abandoned call thread).
struct SlotGuard {
    sem: &'static Semaphore,
}

impl Semaphore {
    const fn new(capacity: usize) -> Self {
        Self {
            capacity,
            used: Mutex::new(0),
            freed: Condvar::new(),
        }
    }

    /// Take a slot, waiting until `deadline` at most. `None` means the
    /// deadline passed while every slot was busy.
    fn acquire_until(&'static self, deadline: Instant) -> Option<SlotGuard> {
        let mut used = self.used.lock().unwrap_or_else(PoisonError::into_inner);
        while *used >= self.capacity {
            let now = Instant::now();
            if now >= deadline {
                return None;
            }
            let (guard, _) = self
                .freed
                .wait_timeout(used, deadline - now)
                .unwrap_or_else(PoisonError::into_inner);
            used = guard;
        }
        *used += 1;
        Some(SlotGuard { sem: self })
    }
}

impl Drop for SlotGuard {
    fn drop(&mut self) {
        let mut used = self.sem.used.lock().unwrap_or_else(PoisonError::into_inner);
        *used = used.saturating_sub(1);
        self.sem.freed.notify_one();
    }
}

/// Slots for request/reply connections. `subscribe` does not use them.
static CALL_SLOTS: Semaphore = Semaphore::new(MAX_CONCURRENT_CALLS);

/// Run `work` on a helper thread under a connection slot from `slots` and
/// wait for its result at most `timeout` (measured from entry, so slot
/// waiting and I/O share one deadline).
///
/// On timeout the helper thread is *not* killed — Rust has no safe way to
/// interrupt a blocking `read` on a pipe. It keeps its slot and exits on its
/// own when the pipe answers or closes (Herdr replies late, restarts or
/// exits); its late result is dropped because the receiver is gone. While
/// such a thread lingers, one of `MAX_CONCURRENT_CALLS` slots is unavailable;
/// if all slots are stuck, further calls fail with `Timeout` at the deadline
/// without connecting and without spawning more threads, so the number of
/// abandoned threads is bounded by the slot count. A mutating request that
/// times out may still be executed by the server later — only its reply is lost.
fn run_with_deadline<T, F>(
    slots: &'static Semaphore,
    method: &str,
    timeout: Duration,
    work: F,
) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T> + Send + 'static,
{
    let deadline = Instant::now() + timeout;
    let timed_out = || HerdrError::Timeout {
        method: method.to_owned(),
    };
    let slot = slots.acquire_until(deadline).ok_or_else(timed_out)?;
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name(format!("herdr-call:{method}"))
        .spawn(move || {
            // Moved in so the slot is released exactly when this thread ends,
            // whether or not anyone is still waiting for the answer.
            let _slot = slot;
            // Receiver may be gone after a timeout; nothing to do then.
            let _ = tx.send(work());
        })?;
    match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => Err(timed_out()),
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(HerdrError::Shape(format!(
            "call thread for {method} ended without a reply"
        ))),
    }
}

/// One request/reply exchange. Returns the `result` object.
///
/// Blocking: safe to call from a Tauri thread-pool command but never from the
/// main thread. Bounded by [`CALL_TIMEOUT`] and [`MAX_CONCURRENT_CALLS`].
pub fn call(method: &str, params: Value) -> Result<Value> {
    let owned_method = method.to_owned();
    run_with_deadline(&CALL_SLOTS, method, CALL_TIMEOUT, move || {
        call_blocking(&owned_method, params)
    })
}

/// The raw exchange: connect, write one request line, read one reply line.
fn call_blocking(method: &str, params: Value) -> Result<Value> {
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

impl From<String> for HerdrError {
    fn from(message: String) -> Self {
        HerdrError::Shape(message)
    }
}

impl From<serde_json::Error> for HerdrError {
    fn from(err: serde_json::Error) -> Self {
        HerdrError::Shape(err.to_string())
    }
}

/// What one line of the `events.subscribe` stream means.
///
/// Wire format (docs/architecture.md F1): the subscription is acknowledged
/// with a normal reply `{"id","result":{…}}`; a refusal is
/// `{"id","error":{"message"}}`; every event afterwards is an object with an
/// `event` field in snake_case (`{"event":"pane_updated", …}`).
#[derive(Debug, PartialEq)]
enum LineClass {
    /// Subscription acknowledged (`result` present, no `event`).
    Ack,
    /// An event object, returned whole for the consumer.
    Event(Value),
    /// Server refused or aborted the subscription; carries the message.
    Error(String),
    /// Blank line, non-JSON, or JSON of a shape we do not know: ignore.
    Skip,
}

/// Pure classifier for one line of the event stream. Never fails: anything
/// unrecognisable is [`LineClass::Skip`] so a stray line cannot kill the
/// subscription, while a real `error` reply is surfaced as such.
fn classify_line(line: &str) -> LineClass {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return LineClass::Skip;
    }
    let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
        return LineClass::Skip;
    };
    if !value.is_object() {
        return LineClass::Skip;
    }
    if value.get("event").is_some() {
        return LineClass::Event(value);
    }
    if let Some(error) = value.get("error") {
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown error")
            .to_string();
        return LineClass::Error(message);
    }
    if value.get("result").is_some() {
        return LineClass::Ack;
    }
    LineClass::Skip
}

/// Open an event subscription and hand every event to `on_event`.
///
/// Returns `Ok(())` only when `on_event` returns `false` (the consumer is
/// gone). Every other way out is an error so the caller can report the link
/// as down: the server refusing the subscription (`Api`), closing the stream
/// (`Io`, "event stream closed"), or a transport failure (`Io`/`Connect`).
///
/// Long-lived by design: it takes no slot from `CALL_SLOTS`, has no deadline
/// and no socket read timeout (an idle stream is normal); the caller already
/// runs it on a dedicated thread.
pub fn subscribe(types: &[&str], mut on_event: impl FnMut(Value) -> bool) -> Result<()> {
    const METHOD: &str = "events.subscribe";
    let mut stream = connect_with(None)?;
    let subscriptions: Vec<Value> = types.iter().map(|t| json!({ "type": t })).collect();
    let request = json!({
        "id": next_id(),
        "method": METHOD,
        "params": { "subscriptions": subscriptions }
    });
    stream.write_all(serde_json::to_string(&request)?.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return Err(HerdrError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "event stream closed",
            )));
        }
        match classify_line(&line) {
            // Blank/garbage lines, and any repeated ack, are ignored. The
            // first meaningful line is expected to be the ack; an event
            // arriving before it is still forwarded (defensive against a
            // server that starts streaming immediately).
            LineClass::Skip | LineClass::Ack => continue,
            LineClass::Error(message) => {
                return Err(HerdrError::Api {
                    method: METHOD.to_owned(),
                    message,
                });
            }
            LineClass::Event(event) => {
                if !on_event(event) {
                    return Ok(());
                }
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    const SHORT: Duration = Duration::from_millis(150);
    const HANG: Duration = Duration::from_millis(1500);

    fn is_timeout(result: &Result<Value>, method: &str) -> bool {
        matches!(result, Err(HerdrError::Timeout { method: m }) if m == method)
    }

    #[test]
    fn deadline_returns_work_result_when_it_finishes_in_time() {
        static SLOTS: Semaphore = Semaphore::new(2);
        let result = run_with_deadline(&SLOTS, "ping", HANG, || Ok(json!({ "ok": true })));
        assert_eq!(result.unwrap(), json!({ "ok": true }));
    }

    #[test]
    fn deadline_propagates_work_error() {
        static SLOTS: Semaphore = Semaphore::new(2);
        let result: Result<Value> = run_with_deadline(&SLOTS, "ping", HANG, || {
            Err(HerdrError::Shape("boom".into()))
        });
        assert!(matches!(result, Err(HerdrError::Shape(m)) if m == "boom"));
    }

    #[test]
    fn deadline_returns_timeout_when_work_hangs() {
        static SLOTS: Semaphore = Semaphore::new(2);
        let started = Instant::now();
        let result: Result<Value> = run_with_deadline(&SLOTS, "pane.read", SHORT, || {
            std::thread::sleep(HANG);
            Ok(Value::Null)
        });
        assert!(is_timeout(&result, "pane.read"), "got {result:?}");
        assert!(started.elapsed() < HANG, "caller waited for the hung work");
        assert_eq!(
            result.unwrap_err().to_string(),
            "timed out waiting for herdr to answer pane.read"
        );
    }

    #[test]
    fn deadline_fails_without_running_work_when_all_slots_are_busy() {
        static SLOTS: Semaphore = Semaphore::new(1);
        let held = SLOTS
            .acquire_until(Instant::now())
            .expect("fresh semaphore has a slot");
        let ran = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&ran);
        let result: Result<Value> = run_with_deadline(&SLOTS, "tab.list", SHORT, move || {
            flag.store(true, Ordering::SeqCst);
            Ok(Value::Null)
        });
        assert!(is_timeout(&result, "tab.list"), "got {result:?}");
        assert!(!ran.load(Ordering::SeqCst), "work ran without a slot");
        drop(held);
        let result = run_with_deadline(&SLOTS, "tab.list", HANG, || Ok(Value::Null));
        assert!(result.is_ok(), "slot was not released: {result:?}");
    }

    #[test]
    fn abandoned_thread_keeps_its_slot_until_it_exits() {
        static SLOTS: Semaphore = Semaphore::new(1);
        let result: Result<Value> = run_with_deadline(&SLOTS, "ping", SHORT, || {
            std::thread::sleep(HANG);
            Ok(Value::Null)
        });
        assert!(is_timeout(&result, "ping"), "got {result:?}");
        // Still hanging: the slot is taken.
        assert!(SLOTS.acquire_until(Instant::now() + SHORT).is_none());
        // Once the work finishes the thread exits and frees the slot.
        let slot = SLOTS.acquire_until(Instant::now() + HANG * 4);
        assert!(
            slot.is_some(),
            "slot never came back after the thread exited"
        );
    }

    // --- parse_reply -------------------------------------------------------

    #[test]
    fn parse_reply_returns_result_object() {
        let reply = parse_reply(
            "ping",
            r#"{"id":"deck:1","result":{"type":"pong","protocol":22}}"#,
        );
        assert_eq!(reply.unwrap(), json!({ "type": "pong", "protocol": 22 }));
    }

    #[test]
    fn parse_reply_tolerates_surrounding_whitespace_and_newline() {
        let reply = parse_reply("ping", "  {\"id\":\"x\",\"result\":{\"ok\":true}}\r\n");
        assert_eq!(reply.unwrap(), json!({ "ok": true }));
    }

    #[test]
    fn parse_reply_maps_error_with_message_to_api_error() {
        let reply = parse_reply(
            "pane.read",
            r#"{"id":"deck:2","error":{"code":"not_found","message":"pane not found"}}"#,
        );
        match reply {
            Err(HerdrError::Api { method, message }) => {
                assert_eq!(method, "pane.read");
                assert_eq!(message, "pane not found");
            }
            other => panic!("expected Api error, got {other:?}"),
        }
    }

    #[test]
    fn parse_reply_error_without_message_uses_placeholder() {
        let reply = parse_reply("ping", r#"{"id":"deck:3","error":{"code":"weird"}}"#);
        match reply {
            Err(HerdrError::Api { method, message }) => {
                assert_eq!(method, "ping");
                assert_eq!(message, "unknown error");
            }
            other => panic!("expected Api error, got {other:?}"),
        }
    }

    #[test]
    fn parse_reply_error_wins_over_result_when_both_present() {
        let reply = parse_reply("ping", r#"{"id":"x","result":{},"error":{"message":"no"}}"#);
        assert!(matches!(reply, Err(HerdrError::Api { message, .. }) if message == "no"));
    }

    #[test]
    fn parse_reply_without_result_is_shape_error() {
        let reply = parse_reply("tab.list", r#"{"id":"deck:4"}"#);
        match reply {
            Err(HerdrError::Shape(text)) => assert_eq!(text, "no result in reply to tab.list"),
            other => panic!("expected Shape error, got {other:?}"),
        }
    }

    #[test]
    fn parse_reply_garbage_is_shape_error_mentioning_the_line() {
        let reply = parse_reply("ping", "this is not json\n");
        match reply {
            Err(HerdrError::Shape(text)) => {
                assert!(text.ends_with("this is not json"), "got {text}");
            }
            other => panic!("expected Shape error, got {other:?}"),
        }
    }

    #[test]
    fn parse_reply_empty_line_is_shape_error() {
        assert!(matches!(parse_reply("ping", ""), Err(HerdrError::Shape(_))));
        assert!(matches!(
            parse_reply("ping", "\n"),
            Err(HerdrError::Shape(_))
        ));
    }

    // --- classify_line -----------------------------------------------------

    #[test]
    fn classify_line_subscribe_ack_is_ack() {
        let line = r#"{"id":"deck:5","result":{"type":"events_subscribed"}}"#;
        assert_eq!(classify_line(line), LineClass::Ack);
        assert_eq!(
            classify_line("{\"id\":\"deck:5\",\"result\":null}\n"),
            LineClass::Ack
        );
    }

    #[test]
    fn classify_line_error_reply_carries_message() {
        let line = r#"{"id":"deck:6","error":{"code":"bad_request","message":"fake: refused"}}"#;
        assert_eq!(
            classify_line(line),
            LineClass::Error("fake: refused".into())
        );
    }

    #[test]
    fn classify_line_error_without_message_uses_placeholder() {
        let line = r#"{"id":"deck:7","error":{"code":"bad_request"}}"#;
        assert_eq!(
            classify_line(line),
            LineClass::Error("unknown error".into())
        );
    }

    #[test]
    fn classify_line_event_is_returned_whole() {
        let line = r#"{"event":"pane_updated","pane":{"pane_id":"w1:p1","agent":"claude"}}"#;
        let expected = json!({
            "event": "pane_updated",
            "pane": { "pane_id": "w1:p1", "agent": "claude" }
        });
        assert_eq!(classify_line(line), LineClass::Event(expected));
    }

    #[test]
    fn classify_line_event_key_wins_over_result_or_error() {
        // Defensive: a line that has `event` is an event even if a server
        // version also attaches `result`/`error` fields.
        let line = r#"{"event":"tab_closed","result":{}}"#;
        assert!(matches!(classify_line(line), LineClass::Event(_)));
    }

    #[test]
    fn classify_line_blank_is_skip() {
        assert_eq!(classify_line(""), LineClass::Skip);
        assert_eq!(classify_line("\n"), LineClass::Skip);
        assert_eq!(classify_line("   \r\n"), LineClass::Skip);
    }

    #[test]
    fn classify_line_garbage_is_skip() {
        assert_eq!(classify_line("not json at all"), LineClass::Skip);
        assert_eq!(classify_line("{\"id\":\"deck:8\""), LineClass::Skip);
        // Valid JSON but not a reply/event shape we know.
        assert_eq!(classify_line("42"), LineClass::Skip);
        assert_eq!(classify_line("{\"id\":\"deck:9\"}"), LineClass::Skip);
    }

    #[test]
    fn semaphore_blocks_at_capacity_and_wakes_on_release() {
        static SLOTS: Semaphore = Semaphore::new(2);
        let a = SLOTS.acquire_until(Instant::now()).expect("slot 1");
        let b = SLOTS.acquire_until(Instant::now()).expect("slot 2");
        assert!(SLOTS.acquire_until(Instant::now() + SHORT).is_none());
        let waiter = std::thread::spawn(|| SLOTS.acquire_until(Instant::now() + HANG).is_some());
        std::thread::sleep(SHORT);
        drop(a);
        assert!(waiter.join().expect("waiter thread panicked"));
        drop(b);
    }
}
