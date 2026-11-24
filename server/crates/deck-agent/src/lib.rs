//! Headless Claude Code sessions for deck-server.
//!
//! deck-server runs Claude Code without a terminal: `deck-server agent`
//! (the *host*, one per pane) starts
//! `claude -p --input-format stream-json --output-format stream-json`,
//! the same control protocol the Agent SDK speaks, and relays it to the
//! server, which hands it to every Deck that shows the pane as a chat.
//!
//! This crate is the part of that which needs no server: the command line
//! for `claude`, the lines written to its stdin ([`proto`]), what an output
//! line means for the pane's status ([`Tracker`]), and the replay buffer a
//! client gets when it (re)subscribes ([`Backlog`]). The host and the
//! server's registry both use it, so they agree on every rule.
//!
//! The event stream is numbered by the host: `seq` grows by one per event
//! within an `epoch` (one host process). A client that comes back with the
//! same epoch and its last seq gets only what it missed.

use std::collections::{BTreeMap, VecDeque};

use serde_json::{json, Value};

/// Hook source the host reports the pane's state under (not one of Herdr's
/// native agent sources, so Herdr takes its state from these reports).
pub const SOURCE: &str = "deck:agent";
/// Agent label on the pane while a host runs in it.
pub const AGENT_LABEL: &str = "deck-claude";
/// Request id of the `initialize` control request the host sends first;
/// its response (slash commands, models, account) is kept for every client.
pub const INIT_REQUEST_ID: &str = "deck-init";

/// The lines written to Claude Code's stdin.
pub mod proto {
    use super::*;

    /// One image the user pasted, base64 data with its media type.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Image {
        pub media_type: String,
        pub data: String,
    }

    const IMAGE_TYPES: &[&str] = &["image/png", "image/jpeg", "image/gif", "image/webp"];

    /// Arguments for `claude` (without the program name).
    pub fn claude_args(
        session: Session<'_>,
        model: Option<&str>,
        permission_mode: Option<&str>,
    ) -> Vec<String> {
        let mut args: Vec<String> = [
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--include-partial-messages",
            "--replay-user-messages",
            "--permission-prompt-tool",
            "stdio",
            // Bypass stays off until the user picks it in the chat; without
            // this flag Claude Code refuses to switch to it at all.
            "--allow-dangerously-skip-permissions",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        match session {
            Session::New(id) => args.extend(["--session-id".into(), id.into()]),
            Session::Resume(id) => args.extend(["--resume".into(), id.into()]),
        }
        if let Some(model) = model.filter(|m| !m.is_empty()) {
            args.extend(["--model".into(), model.into()]);
        }
        if let Some(mode) = permission_mode.filter(|m| !m.is_empty()) {
            args.extend(["--permission-mode".into(), mode.into()]);
        }
        args
    }

    /// Which conversation `claude` opens.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Session<'a> {
        /// A new one with this id (`--session-id`), so the id is known at once.
        New(&'a str),
        /// An existing one (`--resume`).
        Resume(&'a str),
    }

    pub fn initialize() -> String {
        control_request(INIT_REQUEST_ID, json!({ "subtype": "initialize" }))
    }

    pub fn control_request(request_id: &str, request: Value) -> String {
        json!({ "type": "control_request", "request_id": request_id, "request": request })
            .to_string()
    }

    /// A user turn: text and pasted images.
    pub fn user_message(text: &str, images: &[Image]) -> Result<String, String> {
        if text.trim().is_empty() && images.is_empty() {
            return Err("empty message".into());
        }
        let content = if images.is_empty() {
            Value::String(text.to_string())
        } else {
            let mut blocks = Vec::new();
            for image in images {
                if !IMAGE_TYPES.contains(&image.media_type.as_str()) {
                    return Err(format!("unsupported image type {}", image.media_type));
                }
                blocks.push(json!({
                    "type": "image",
                    "source": { "type": "base64", "media_type": image.media_type, "data": image.data },
                }));
            }
            if !text.trim().is_empty() {
                blocks.push(json!({ "type": "text", "text": text }));
            }
            Value::Array(blocks)
        };
        Ok(json!({
            "type": "user",
            "message": { "role": "user", "content": content },
            "parent_tool_use_id": null,
            "session_id": "",
        })
        .to_string())
    }

    /// The answer to a `can_use_tool` request. `response` is what the SDK
    /// returns from `canUseTool`: `{"behavior":"allow","updatedInput":{..}}`
    /// (the input may carry `answers` for AskUserQuestion) or
    /// `{"behavior":"deny","message":".."}`.
    pub fn permission_response(request_id: &str, response: &Value) -> Result<String, String> {
        match response.get("behavior").and_then(Value::as_str) {
            Some("allow") => {
                if !response.get("updatedInput").is_some_and(Value::is_object) {
                    return Err(
                        "allow needs updatedInput (the tool input, possibly changed)".into(),
                    );
                }
            }
            Some("deny") => {}
            _ => return Err("behavior must be allow or deny".into()),
        }
        Ok(json!({
            "type": "control_response",
            "response": { "subtype": "success", "request_id": request_id, "response": response },
        })
        .to_string())
    }

    pub fn interrupt(request_id: &str) -> String {
        control_request(request_id, json!({ "subtype": "interrupt" }))
    }

    pub fn set_model(request_id: &str, model: Option<&str>) -> String {
        control_request(
            request_id,
            json!({ "subtype": "set_model", "model": model }),
        )
    }

    /// Claude Code's permission modes (`claude --permission-mode`; `default`
    /// is the older name of `manual`).
    pub const PERMISSION_MODES: &[&str] = &[
        "default",
        "manual",
        "acceptEdits",
        "plan",
        "auto",
        "dontAsk",
        "bypassPermissions",
    ];

    pub fn set_permission_mode(request_id: &str, mode: &str) -> Result<String, String> {
        if !PERMISSION_MODES.contains(&mode) {
            return Err(format!("unknown permission mode {mode}"));
        }
        Ok(control_request(
            request_id,
            json!({ "subtype": "set_permission_mode", "mode": mode }),
        ))
    }

    /// The host's own note in the event stream that a permission request
    /// was answered (by any client), so the others close their card.
    pub fn resolved(request_id: &str) -> Value {
        json!({ "type": "deck_resolved", "request_id": request_id })
    }

    /// The host's own note that `claude` accepted a model or permission
    /// mode change (`set_model` / `set_permission_mode` succeeded).
    pub fn settings(model: Option<Option<&str>>, permission_mode: Option<&str>) -> Value {
        let mut v = json!({ "type": "deck_settings" });
        if let Some(model) = model {
            v["model"] = json!(model);
        }
        if let Some(mode) = permission_mode {
            v["permission_mode"] = json!(mode);
        }
        v
    }

    /// The host's own note about the `claude` process: `state` is
    /// `running`, `exited` (with the exit code in `detail`) or `log` (a
    /// line `claude` wrote to stderr, in `detail`).
    pub fn host_state(state: &str, detail: Option<&str>) -> Value {
        json!({ "type": "deck_host", "state": state, "detail": detail })
    }

    /// `claude` started on this conversation: clients know the session id
    /// (and can show its transcript) before `claude` says anything.
    pub fn running(session_id: &str) -> Value {
        json!({ "type": "deck_host", "state": "running", "detail": null, "session_id": session_id })
    }
}

/// One operation a client asks the host to perform (the server forwards
/// these over the host's inbox, one JSON line each).
#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    Send {
        text: String,
        images: Vec<proto::Image>,
    },
    Respond {
        request_id: String,
        response: Value,
    },
    Interrupt,
    Set {
        model: Option<Option<String>>,
        permission_mode: Option<String>,
    },
    /// Open this existing conversation instead (archive restore): the host
    /// stops its `claude` and starts it again with `--resume`.
    Resume {
        session_id: String,
    },
    /// Start `claude` again on the same conversation once it is idle (it
    /// reads the account's credentials only when it starts; the server asks
    /// for this after an account switch or a sign-in).
    Restart,
}

/// A session id the host may pass to `claude --resume`: a uuid-like token.
pub fn valid_session_id(id: &str) -> bool {
    id.len() <= 128
        && id.bytes().next().is_some_and(|b| b.is_ascii_alphanumeric())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

impl Op {
    /// From a client's `deck.agent.*` method and params.
    pub fn from_request(method: &str, params: &Value) -> Result<Op, String> {
        let text = |key: &str| params.get(key).and_then(Value::as_str).map(str::to_string);
        match method {
            "send" => {
                let mut images = Vec::new();
                if let Some(list) = params.get("images").and_then(Value::as_array) {
                    for image in list {
                        let (Some(media_type), Some(data)) = (
                            image.get("media_type").and_then(Value::as_str),
                            image.get("data").and_then(Value::as_str),
                        ) else {
                            return Err("an image needs media_type and data".into());
                        };
                        images.push(proto::Image {
                            media_type: media_type.into(),
                            data: data.into(),
                        });
                    }
                }
                let text = text("text").unwrap_or_default();
                proto::user_message(&text, &images)?;
                Ok(Op::Send { text, images })
            }
            "respond" => {
                let request_id = text("request_id").ok_or("request_id is required")?;
                let response = params
                    .get("response")
                    .cloned()
                    .ok_or("response is required")?;
                proto::permission_response(&request_id, &response)?;
                Ok(Op::Respond {
                    request_id,
                    response,
                })
            }
            "interrupt" => Ok(Op::Interrupt),
            "set" => {
                let model = match params.get("model") {
                    None => None,
                    Some(Value::Null) => Some(None),
                    Some(Value::String(m)) => Some(Some(m.clone())),
                    Some(_) => return Err("model must be a string or null".into()),
                };
                let permission_mode = text("permission_mode");
                if let Some(mode) = &permission_mode {
                    proto::set_permission_mode("check", mode)?;
                }
                if model.is_none() && permission_mode.is_none() {
                    return Err("nothing to set (model, permission_mode)".into());
                }
                Ok(Op::Set {
                    model,
                    permission_mode,
                })
            }
            "resume" => {
                let session_id = text("session_id").ok_or("session_id is required")?;
                if !valid_session_id(&session_id) {
                    return Err(format!("not a session id: {session_id}"));
                }
                Ok(Op::Resume { session_id })
            }
            "restart" => Ok(Op::Restart),
            other => Err(format!("unknown agent operation {other}")),
        }
    }

    pub fn to_line(&self) -> String {
        match self {
            Op::Send { text, images } => {
                json!({ "op": "send", "text": text, "images": images.iter().map(|i| json!({"media_type": i.media_type, "data": i.data})).collect::<Vec<_>>() })
            }
            Op::Respond { request_id, response } => {
                json!({ "op": "respond", "request_id": request_id, "response": response })
            }
            Op::Interrupt => json!({ "op": "interrupt" }),
            Op::Set { model, permission_mode } => {
                let mut v = json!({ "op": "set" });
                if let Some(model) = model {
                    v["model"] = json!(model);
                }
                if let Some(mode) = permission_mode {
                    v["permission_mode"] = json!(mode);
                }
                v
            }
            Op::Resume { session_id } => json!({ "op": "resume", "session_id": session_id }),
            Op::Restart => json!({ "op": "restart" }),
        }
        .to_string()
    }

    pub fn from_line(line: &str) -> Result<Op, String> {
        let v: Value = serde_json::from_str(line).map_err(|e| e.to_string())?;
        let op = v.get("op").and_then(Value::as_str).ok_or("op is missing")?;
        Op::from_request(op, &v)
    }

    /// For [`Op::Set`]: the control request ids [`Op::stdin_lines`] uses
    /// with `n`, each with the [`proto::settings`] note to emit once
    /// `claude` confirms it.
    pub fn settings_requests(&self, n: u64) -> Vec<(String, Value)> {
        let Op::Set {
            model,
            permission_mode,
        } = self
        else {
            return Vec::new();
        };
        let mut out = Vec::new();
        if let Some(model) = model {
            out.push((
                format!("deck-model-{n}"),
                proto::settings(Some(model.as_deref()), None),
            ));
        }
        if let Some(mode) = permission_mode {
            out.push((format!("deck-mode-{n}"), proto::settings(None, Some(mode))));
        }
        out
    }

    /// The lines this operation writes to `claude`'s stdin; `n` makes the
    /// control request ids unique.
    pub fn stdin_lines(&self, n: u64) -> Result<Vec<String>, String> {
        Ok(match self {
            Op::Send { text, images } => vec![proto::user_message(text, images)?],
            Op::Respond {
                request_id,
                response,
            } => {
                vec![proto::permission_response(request_id, response)?]
            }
            Op::Interrupt => vec![proto::interrupt(&format!("deck-interrupt-{n}"))],
            Op::Set {
                model,
                permission_mode,
            } => {
                let mut lines = Vec::new();
                if let Some(model) = model {
                    lines.push(proto::set_model(
                        &format!("deck-model-{n}"),
                        model.as_deref(),
                    ));
                }
                if let Some(mode) = permission_mode {
                    lines.push(proto::set_permission_mode(&format!("deck-mode-{n}"), mode)?);
                }
                lines
            }
            // The host handles these itself; nothing goes to `claude`.
            Op::Resume { .. } | Op::Restart => Vec::new(),
        })
    }
}

/// What the pane shows as its agent status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Idle,
    Working,
    Blocked,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Idle => "idle",
            Status::Working => "working",
            Status::Blocked => "blocked",
        }
    }
}

fn msg_type(msg: &Value) -> &str {
    msg.get("type").and_then(Value::as_str).unwrap_or("")
}

fn subtype(msg: &Value) -> &str {
    msg.get("subtype").and_then(Value::as_str).unwrap_or("")
}

/// A successful `control_response` from `claude`: the request id it answers.
pub fn control_success_id(msg: &Value) -> Option<&str> {
    if msg_type(msg) != "control_response" {
        return None;
    }
    let response = msg.get("response")?;
    (response.get("subtype").and_then(Value::as_str) == Some("success"))
        .then(|| response.get("request_id").and_then(Value::as_str))
        .flatten()
}

/// A control request `claude` refused: its request id and why.
pub fn control_error(msg: &Value) -> Option<(&str, &str)> {
    if msg_type(msg) != "control_response" {
        return None;
    }
    let response = msg.get("response")?;
    if response.get("subtype").and_then(Value::as_str) != Some("error") {
        return None;
    }
    let id = response.get("request_id").and_then(Value::as_str)?;
    let why = response
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or("refused");
    Some((id, why))
}

/// A `can_use_tool` request from `claude`: its request id.
pub fn permission_request_id(msg: &Value) -> Option<&str> {
    if msg_type(msg) != "control_request" {
        return None;
    }
    let request = msg.get("request")?;
    (request.get("subtype").and_then(Value::as_str) == Some("can_use_tool"))
        .then(|| msg.get("request_id").and_then(Value::as_str))
        .flatten()
}

/// A request that is no longer open: answered (`deck_resolved`) or
/// withdrawn by `claude` (`control_cancel_request`, e.g. after an interrupt).
pub fn closed_request_id(msg: &Value) -> Option<&str> {
    match msg_type(msg) {
        "deck_resolved" | "control_cancel_request" => msg.get("request_id").and_then(Value::as_str),
        _ => None,
    }
}

/// A user turn echoed back (`--replay-user-messages`), not a tool result.
fn is_user_turn(msg: &Value) -> bool {
    if msg_type(msg) != "user" {
        return false;
    }
    match msg.pointer("/message/content") {
        Some(Value::String(_)) => true,
        Some(Value::Array(blocks)) => !blocks
            .iter()
            .any(|b| b.get("type").and_then(Value::as_str) == Some("tool_result")),
        _ => false,
    }
}

/// Follows the event stream and says what the pane's status is.
#[derive(Debug, Default, Clone)]
pub struct Tracker {
    pub session_id: Option<String>,
    pub model: Option<String>,
    pub permission_mode: Option<String>,
    busy: bool,
    open: Vec<String>,
}

impl Tracker {
    pub fn status(&self) -> Status {
        if !self.open.is_empty() {
            Status::Blocked
        } else if self.busy {
            Status::Working
        } else {
            Status::Idle
        }
    }

    pub fn open_requests(&self) -> &[String] {
        &self.open
    }

    /// Feeds one event; returns the status when it changed.
    pub fn observe(&mut self, msg: &Value) -> Option<Status> {
        let before = self.status();
        if let Some(id) = msg
            .get("session_id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            self.session_id = Some(id.to_string());
        }
        match (msg_type(msg), subtype(msg)) {
            ("system", "init") => {
                if let Some(model) = msg.get("model").and_then(Value::as_str) {
                    self.model = Some(model.into());
                }
                if let Some(mode) = msg.get("permissionMode").and_then(Value::as_str) {
                    self.permission_mode = Some(mode.into());
                }
            }
            ("deck_settings", _) => {
                if let Some(model) = msg.get("model") {
                    self.model = model.as_str().map(str::to_string);
                }
                if let Some(mode) = msg.get("permission_mode").and_then(Value::as_str) {
                    self.permission_mode = Some(mode.into());
                }
            }
            ("result", _) => {
                self.busy = false;
                self.open.clear();
            }
            ("deck_host", _) => {
                // The process went away: nothing is running or waiting any more.
                if msg.get("state").and_then(Value::as_str) == Some("exited") {
                    self.busy = false;
                    self.open.clear();
                }
            }
            ("stream_event", _) | ("assistant", _) => self.busy = true,
            _ => {}
        }
        if is_user_turn(msg) {
            self.busy = true;
        }
        if let Some(id) = permission_request_id(msg) {
            if !self.open.iter().any(|o| o == id) {
                self.open.push(id.to_string());
            }
        }
        if let Some(id) = closed_request_id(msg) {
            self.open.retain(|o| o != id);
        }
        let after = self.status();
        (after != before).then_some(after)
    }
}

/// One numbered event, kept as its JSON text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub seq: u64,
    pub msg: String,
}

/// Why an entry is kept past the end of a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pin {
    /// Dropped at the next turn end (or when the buffer is full).
    None,
    /// The latest of its kind is kept for good (`system/init`, the
    /// `initialize` response): a new client needs it to show the session.
    Meta(&'static str),
    /// An open permission request, kept until it is closed.
    Request,
}

/// What a (re)subscribing client is sent: the current turn, the session's
/// meta events and every open permission request, in seq order.
///
/// A turn's full history is in the transcript, so at each `result` the
/// buffer drops what came before it; this keeps a long session small.
#[derive(Debug, Default)]
pub struct Backlog {
    entries: VecDeque<(Entry, Pin)>,
    bytes: usize,
    open: BTreeMap<String, u64>,
}

/// Oldest unpinned events go first beyond this size.
pub const BACKLOG_MAX_BYTES: usize = 8 * 1024 * 1024;

impl Backlog {
    pub fn push(&mut self, seq: u64, msg: &Value) {
        let text = msg.to_string();
        let pin = if permission_request_id(msg).is_some() {
            Pin::Request
        } else {
            match (msg_type(msg), subtype(msg)) {
                ("system", "init") => Pin::Meta("init"),
                ("control_response", _)
                    if msg.pointer("/response/request_id").and_then(Value::as_str)
                        == Some(INIT_REQUEST_ID) =>
                {
                    Pin::Meta("initialize")
                }
                ("deck_host", _) if msg.get("state").and_then(Value::as_str) == Some("running") => {
                    Pin::Meta("running")
                }
                _ => Pin::None,
            }
        };
        if let Pin::Meta(kind) = pin {
            self.remove_where(|p, _| *p == Pin::Meta(kind));
        }
        if let Some(id) = closed_request_id(msg) {
            if let Some(req_seq) = self.open.remove(id) {
                self.remove_where(|_, e| e.seq == req_seq);
            }
        }
        if msg_type(msg) == "result" {
            self.remove_where(|p, _| *p == Pin::None);
        }
        if let Some(id) = permission_request_id(msg) {
            self.open.insert(id.to_string(), seq);
        }
        self.bytes += text.len();
        self.entries.push_back((Entry { seq, msg: text }, pin));
        while self.bytes > BACKLOG_MAX_BYTES {
            let Some(pos) = self.entries.iter().position(|(_, p)| *p == Pin::None) else {
                break;
            };
            if let Some((e, _)) = self.entries.remove(pos) {
                self.bytes -= e.msg.len();
            }
        }
    }

    fn remove_where(&mut self, mut drop: impl FnMut(&Pin, &Entry) -> bool) {
        let mut bytes = self.bytes;
        self.entries.retain(|(e, p)| {
            let gone = drop(p, e);
            if gone {
                bytes -= e.msg.len();
            }
            !gone
        });
        self.bytes = bytes;
    }

    /// Entries after `after` (all of them for `None`).
    pub fn since(&self, after: Option<u64>) -> Vec<Entry> {
        self.entries
            .iter()
            .filter(|(e, _)| after.is_none_or(|a| e.seq > a))
            .map(|(e, _)| e.clone())
            .collect()
    }

    pub fn is_open(&self, request_id: &str) -> bool {
        self.open.contains_key(request_id)
    }

    pub fn last_seq(&self) -> Option<u64> {
        self.entries.back().map(|(e, _)| e.seq)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// A line the host sends the server for each event: `{"seq":N,"msg":{..}}`.
pub fn event_line(seq: u64, msg: &Value) -> String {
    json!({ "seq": seq, "msg": msg }).to_string()
}

/// Parses an [`event_line`].
pub fn parse_event_line(line: &str) -> Option<(u64, Value)> {
    let mut v: Value = serde_json::from_str(line).ok()?;
    let seq = v.get("seq")?.as_u64()?;
    let msg = v.get_mut("msg")?.take();
    msg.is_object().then_some((seq, msg))
}

/// A random-enough version-4 UUID from a seed the caller mixes (time, pid,
/// a counter); `claude --session-id` only needs it to be unique.
pub fn uuid_from_seed(seed: [u8; 16]) -> String {
    let mut b = seed;
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

#[cfg(test)]
mod tests;
