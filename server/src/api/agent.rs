//! `deck.agent.*`: headless Claude Code sessions for Netrunner Deck's chat.
//!
//! A pane that runs `deck-server agent` (the host, see
//! `src/deck_server/agent_host.rs`) has Claude Code running without a
//! terminal, on the stream-json control protocol. The host keeps two
//! connections to this server, one per direction, and every Deck that shows
//! the pane talks to it through the methods below. The registry here is a
//! relay with a replay buffer; the event stream itself is the host's.
//!
//! Like `deck.transcript.*` these are answered on the API thread before the
//! typed request is parsed, so Herdr's method set and schema stay as they
//! are.
//!
//! Host side (only `deck-server agent` uses these):
//!
//! * `deck.agent.host {pane_id, epoch}`: after the reply the host only
//!   writes, one `{"seq":N,"msg":{..}}` per event.
//! * `deck.agent.inbox {pane_id, epoch}`: after the reply the server only
//!   writes, one operation per line (see `deck_agent::Op`).
//!
//! Client side:
//!
//! * `deck.agent.subscribe {pane_id, epoch?, after_seq?}`: reply
//!   `{pane_id, epoch, connected, status, session_id}`, then the buffered
//!   events (only those after `after_seq` when `epoch` is the current one),
//!   then live ones, as `{"event":"deck.agent.event","data":{pane_id, epoch,
//!   seq, msg}}`. A new host in the pane is announced with
//!   `deck.agent.reset {pane_id, epoch}`. The client sends nothing more.
//!   A pane with no host yet can be subscribed to; its events come when
//!   the host connects.
//! * `deck.agent.send {pane_id, text, images?: [{media_type, data}]}`,
//!   `deck.agent.respond {pane_id, request_id, response}`,
//!   `deck.agent.interrupt {pane_id}`,
//!   `deck.agent.set {pane_id, model?, permission_mode?}`,
//!   `deck.agent.resume {pane_id, session_id}` (open that conversation in
//!   the pane instead: archive restore),
//!   `deck.agent.restart {pane_id}` (start `claude` again once idle) → `{queued}`
//!   (`queued: true` = the host is not connected yet; sent when it is).
//!   `respond` to a request that is no longer open is `already_answered`.
//! * `deck.agent.list` → `{agents: [{pane_id, epoch, connected, status,
//!   session_id, model, permission_mode, open_requests}]}`.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::Duration;

use deck_agent::{Backlog, Op, Tracker};
use serde_json::{json, Value};

use crate::ipc::{
    is_connection_closed_error, local_stream_peer_closed, poll_local_stream_read_count,
    set_local_stream_polling, LocalStream, LocalStreamReadCount,
};

const PREFIX: &str = "deck.agent.";
const POLL: Duration = Duration::from_millis(100);
/// Operations held for a host that is not connected yet.
const MAX_QUEUED: usize = 32;
/// Longest event line a host may send.
const MAX_EVENT_LINE: usize = 32 * 1024 * 1024;

#[derive(Default)]
struct Agent {
    epoch: String,
    backlog: Backlog,
    tracker: Tracker,
    last_seq: Option<u64>,
    host_connected: bool,
    inbox: Option<(u64, mpsc::Sender<String>)>,
    queued: VecDeque<String>,
    subscribers: Vec<mpsc::Sender<String>>,
    /// Requests a client answered whose close has not come back from the
    /// host yet: a second answer is refused at once.
    answered: HashSet<String>,
}

impl Agent {
    /// A different host process took the pane: start its stream afresh.
    fn adopt(&mut self, pane_id: &str, epoch: &str) {
        if self.epoch == epoch {
            return;
        }
        self.epoch = epoch.to_string();
        self.backlog = Backlog::default();
        self.tracker = Tracker::default();
        self.last_seq = None;
        self.answered.clear();
        let reset =
            json!({ "event": "deck.agent.reset", "data": { "pane_id": pane_id, "epoch": epoch } })
                .to_string();
        self.broadcast(&reset);
    }

    fn broadcast(&mut self, line: &str) {
        self.subscribers
            .retain(|tx| tx.send(line.to_string()).is_ok());
    }

    fn summary(&self, pane_id: &str) -> Value {
        json!({
            "pane_id": pane_id,
            "epoch": self.epoch,
            "connected": self.host_connected,
            "status": self.tracker.status().as_str(),
            "session_id": self.tracker.session_id,
            "model": self.tracker.model,
            "permission_mode": self.tracker.permission_mode,
            "open_requests": self.tracker.open_requests(),
        })
    }
}

fn registry() -> std::sync::MutexGuard<'static, HashMap<String, Agent>> {
    static REGISTRY: OnceLock<Mutex<HashMap<String, Agent>>> = OnceLock::new();
    REGISTRY
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

fn event_line(pane_id: &str, epoch: &str, seq: u64, msg: &str) -> String {
    format!(
        r#"{{"event":"deck.agent.event","data":{{"pane_id":{},"epoch":{},"seq":{seq},"msg":{msg}}}}}"#,
        Value::from(pane_id),
        Value::from(epoch),
    )
}

type Reply = Result<Value, (&'static str, String)>;

fn reply_line(id: &str, result: Reply) -> String {
    match result {
        Ok(result) => json!({ "id": id, "result": result }),
        Err((code, message)) => json!({ "id": id, "error": { "code": code, "message": message } }),
    }
    .to_string()
}

fn write_line(stream: &mut LocalStream, line: &str) -> io::Result<()> {
    stream.write_all(line.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()
}

/// Handles a `deck.agent.*` connection; `None` for any other request line.
pub(crate) fn handle(
    line: &str,
    stream: &mut LocalStream,
    running: &AtomicBool,
) -> Option<io::Result<()>> {
    if !line.contains(PREFIX) {
        return None;
    }
    let request: Value = serde_json::from_str(line).ok()?;
    let method = request
        .get("method")?
        .as_str()?
        .strip_prefix(PREFIX)?
        .to_string();
    let id = request
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let params = request.get("params").cloned().unwrap_or(Value::Null);
    let pane_id = params
        .get("pane_id")
        .and_then(Value::as_str)
        .filter(|p| !p.is_empty())
        .map(str::to_string);
    let epoch = params
        .get("epoch")
        .and_then(Value::as_str)
        .map(str::to_string);
    let missing = |what: &str| Err(("invalid_params", format!("{what} is required")));

    let result = match (method.as_str(), pane_id) {
        ("list", _) => Ok(list()),
        ("host", Some(pane)) => match epoch {
            Some(epoch) => return Some(host(&id, &pane, &epoch, stream, running)),
            None => missing("epoch"),
        },
        ("inbox", Some(pane)) => match epoch {
            Some(epoch) => return Some(inbox(&id, &pane, &epoch, stream, running)),
            None => missing("epoch"),
        },
        ("subscribe", Some(pane)) => {
            let after = params.get("after_seq").and_then(Value::as_u64);
            return Some(subscribe(
                &id,
                &pane,
                epoch.as_deref(),
                after,
                stream,
                running,
            ));
        }
        ("send" | "respond" | "interrupt" | "set" | "resume" | "restart", Some(pane)) => {
            operation(&method, &pane, &params)
        }
        (
            "host" | "inbox" | "subscribe" | "send" | "respond" | "interrupt" | "set" | "resume"
            | "restart",
            None,
        ) => missing("pane_id"),
        (other, _) => Err(("unknown_method", format!("unknown method {PREFIX}{other}"))),
    };
    let line = reply_line(&id, result);
    Some(match write_line(stream, &line) {
        Err(err) if is_connection_closed_error(&err) => Ok(()),
        other => other,
    })
}

fn list() -> Value {
    let agents: Vec<Value> = registry()
        .iter()
        .filter(|(_, a)| !a.epoch.is_empty())
        .map(|(pane, a)| a.summary(pane))
        .collect();
    json!({ "agents": agents })
}

/// Asks every headless agent to start `claude` again once it is idle (the
/// machine's account changed; `claude` reads its login when it starts).
pub(crate) fn restart_all() {
    let line = Op::Restart.to_line();
    let mut reg = registry();
    for agent in reg.values_mut().filter(|a| !a.epoch.is_empty()) {
        if let Some((_, tx)) = &agent.inbox {
            if tx.send(line.clone()).is_ok() {
                continue;
            }
            agent.inbox = None;
        }
        if agent.queued.len() < MAX_QUEUED {
            agent.queued.push_back(line.clone());
        }
    }
}

fn operation(method: &str, pane_id: &str, params: &Value) -> Reply {
    let op = Op::from_request(method, params).map_err(|e| ("invalid_params", e))?;
    let mut reg = registry();
    let agent = reg
        .get_mut(pane_id)
        .filter(|a| !a.epoch.is_empty())
        .ok_or((
            "agent_not_found",
            format!("no headless agent in pane {pane_id}"),
        ))?;
    if let Op::Respond { request_id, .. } = &op {
        if !agent.backlog.is_open(request_id) || !agent.answered.insert(request_id.clone()) {
            return Err((
                "already_answered",
                format!("request {request_id} is not open"),
            ));
        }
    }
    let line = op.to_line();
    if let Some((_, tx)) = &agent.inbox {
        if tx.send(line.clone()).is_ok() {
            return Ok(json!({ "queued": false }));
        }
        agent.inbox = None;
    }
    if agent.queued.len() >= MAX_QUEUED {
        return Err((
            "agent_unavailable",
            "the agent host is not connected".into(),
        ));
    }
    agent.queued.push_back(line);
    Ok(json!({ "queued": true }))
}

/// The host's event connection: read `{"seq","msg"}` lines until it closes.
fn host(
    id: &str,
    pane_id: &str,
    epoch: &str,
    stream: &mut LocalStream,
    running: &AtomicBool,
) -> io::Result<()> {
    {
        let mut reg = registry();
        let agent = reg.entry(pane_id.to_string()).or_default();
        agent.adopt(pane_id, epoch);
        agent.host_connected = true;
    }
    let result = (|| {
        set_local_stream_polling(stream, false)?;
        write_line(stream, &reply_line(id, Ok(json!({}))))?;
        set_local_stream_polling(stream, true)?;
        let mut pending: Vec<u8> = Vec::new();
        let mut buf = vec![0u8; 64 * 1024];
        while running.load(Ordering::Relaxed) {
            match poll_local_stream_read_count(stream, &mut buf)? {
                LocalStreamReadCount::Closed => break,
                LocalStreamReadCount::Pending => std::thread::sleep(POLL),
                LocalStreamReadCount::Data(n) => {
                    pending.extend_from_slice(&buf[..n]);
                    while let Some(end) = pending.iter().position(|b| *b == b'\n') {
                        let line: Vec<u8> = pending.drain(..=end).collect();
                        ingest(pane_id, epoch, &String::from_utf8_lossy(&line));
                    }
                    if pending.len() > MAX_EVENT_LINE {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "agent event line is too large",
                        ));
                    }
                }
            }
        }
        Ok(())
    })();
    if let Some(agent) = registry().get_mut(pane_id) {
        if agent.epoch == epoch {
            agent.host_connected = false;
        }
    }
    match result {
        Err(err) if is_connection_closed_error(&err) => Ok(()),
        other => other,
    }
}

fn ingest(pane_id: &str, epoch: &str, line: &str) {
    let Some((seq, msg)) = deck_agent::parse_event_line(line.trim()) else {
        return;
    };
    let mut reg = registry();
    let Some(agent) = reg.get_mut(pane_id) else {
        return;
    };
    // A reconnecting host replays its buffer: skip what is already here.
    if agent.epoch != epoch || agent.last_seq.is_some_and(|last| seq <= last) {
        return;
    }
    agent.last_seq = Some(seq);
    if let Some(id) = deck_agent::closed_request_id(&msg) {
        agent.answered.remove(id);
    }
    agent.backlog.push(seq, &msg);
    agent.tracker.observe(&msg);
    let line = event_line(pane_id, epoch, seq, &msg.to_string());
    agent.broadcast(&line);
}

static INBOX_TOKENS: AtomicU64 = AtomicU64::new(1);

/// The host's command connection: write operations to it as they come.
fn inbox(
    id: &str,
    pane_id: &str,
    epoch: &str,
    stream: &mut LocalStream,
    running: &AtomicBool,
) -> io::Result<()> {
    let token = INBOX_TOKENS.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = mpsc::channel();
    {
        let mut reg = registry();
        let agent = reg.entry(pane_id.to_string()).or_default();
        agent.adopt(pane_id, epoch);
        for line in agent.queued.drain(..) {
            let _ = tx.send(line);
        }
        agent.inbox = Some((token, tx));
    }
    let result = (|| {
        set_local_stream_polling(stream, false)?;
        write_line(stream, &reply_line(id, Ok(json!({}))))?;
        pump(rx, stream, running)
    })();
    if let Some(agent) = registry().get_mut(pane_id) {
        if agent.inbox.as_ref().is_some_and(|(t, _)| *t == token) {
            agent.inbox = None;
        }
    }
    match result {
        Err(err) if is_connection_closed_error(&err) => Ok(()),
        other => other,
    }
}

/// Writes lines from `rx` until the peer goes, the sender is dropped or the
/// server stops.
fn pump(
    rx: mpsc::Receiver<String>,
    stream: &mut LocalStream,
    running: &AtomicBool,
) -> io::Result<()> {
    loop {
        match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(line) => write_line(stream, &line)?,
            Err(RecvTimeoutError::Timeout) => {
                if !running.load(Ordering::Relaxed) || local_stream_peer_closed(stream)? {
                    return Ok(());
                }
            }
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        }
    }
}

fn subscribe(
    id: &str,
    pane_id: &str,
    epoch: Option<&str>,
    after_seq: Option<u64>,
    stream: &mut LocalStream,
    running: &AtomicBool,
) -> io::Result<()> {
    let (tx, rx) = mpsc::channel();
    let (head, replay) = {
        let mut reg = registry();
        let agent = reg.entry(pane_id.to_string()).or_default();
        let after = if epoch == Some(agent.epoch.as_str()) {
            after_seq
        } else {
            None
        };
        let replay: Vec<String> = agent
            .backlog
            .since(after)
            .into_iter()
            .map(|e| event_line(pane_id, &agent.epoch, e.seq, &e.msg))
            .collect();
        agent.subscribers.push(tx);
        (agent.summary(pane_id), replay)
    };
    let result = (|| {
        set_local_stream_polling(stream, false)?;
        write_line(stream, &reply_line(id, Ok(head)))?;
        for line in &replay {
            write_line(stream, line)?;
        }
        pump(rx, stream, running)
    })();
    match result {
        Err(err) if is_connection_closed_error(&err) => Ok(()),
        other => other,
    }
}
