//! `deck.transcript.read` / `deck.transcript.stat`: Claude Code transcripts
//! of this machine for Netrunner Deck's chat view.
//!
//! Claude Code appends one JSON line per event to
//! `~/.claude/projects/<mangled cwd>/<session id>.jsonl`. Deck shows a
//! pane's conversation from that file; when the pane runs on another
//! machine the file is only here, so the server hands it out over its API,
//! the same connection Deck uses for everything else.
//!
//! These two methods are a Deck extension next to Herdr's API: they are
//! answered on the API thread before the typed request is parsed, so
//! Herdr's method set and schema stay as they are, and a client that does
//! not know them never sees a difference.
//!
//! * `deck.transcript.read {session_id, from?, tail?}` → `{session_id,
//!   path, start, end, len, lines}`: complete lines from byte `from` (or the
//!   last `tail` bytes, line-aligned), at most 4 MiB; a session that has not
//!   written yet is an empty chunk.
//! * `deck.transcript.stat {session_ids}` → `{sessions: {id: {len,
//!   mtime_ms}}}`; sessions without a transcript are left out.

use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;

use serde_json::{json, Value};

const PREFIX: &str = "deck.transcript.";
/// Longest single read; a very large tail is served in several calls.
const MAX_CHUNK: u64 = 4 * 1024 * 1024;
/// At most this many sessions in one `stat`.
const MAX_STAT: usize = 512;

/// The reply line for a `deck.transcript.*` request; `None` for any other
/// line (it goes on to the normal API path).
pub(crate) fn handle(line: &str) -> Option<String> {
    if !line.contains(PREFIX) {
        return None;
    }
    let request: Value = serde_json::from_str(line).ok()?;
    let method = request.get("method")?.as_str()?;
    if !method.starts_with(PREFIX) {
        return None;
    }
    let id = request
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let params = request.get("params").cloned().unwrap_or(Value::Null);
    let result = match method {
        "deck.transcript.read" => read(&params),
        "deck.transcript.stat" => stat(&params),
        other => Err(("unknown_method", format!("unknown method {other}"))),
    };
    Some(
        match result {
            Ok(result) => json!({ "id": id, "result": result }),
            Err((code, message)) => {
                json!({ "id": id, "error": { "code": code, "message": message } })
            }
        }
        .to_string(),
    )
}

type Failure = (&'static str, String);

/// A Claude session id: a UUID, nothing that could walk the file system.
fn looks_like_session_id(s: &str) -> bool {
    s.len() == 36
        && s.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
        && s.matches('-').count() == 4
}

fn projects_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").filter(|v| !v.is_empty())?;
    Some(PathBuf::from(home).join(".claude").join("projects"))
}

/// `<projects>/<any dir>/<id>.jsonl`: the project directory name is a
/// mangled cwd, so the file is found by its name.
fn find(session_id: &str) -> Option<PathBuf> {
    if !looks_like_session_id(session_id) {
        return None;
    }
    let name = format!("{session_id}.jsonl");
    std::fs::read_dir(projects_dir()?)
        .ok()?
        .flatten()
        .map(|dir| dir.path().join(&name))
        .find(|candidate| candidate.is_file())
}

fn read(params: &Value) -> Result<Value, Failure> {
    let session_id = params
        .get("session_id")
        .and_then(Value::as_str)
        .ok_or(("invalid_request", "session_id is required".to_string()))?;
    if !looks_like_session_id(session_id) {
        return Err(("invalid_request", "session_id is not a session id".into()));
    }
    let from = params.get("from").and_then(Value::as_u64);
    let tail = params.get("tail").and_then(Value::as_u64);
    let Some(path) = find(session_id) else {
        return Ok(json!({
            "session_id": session_id, "path": "", "start": 0, "end": 0, "len": 0, "lines": []
        }));
    };
    let io = |e: std::io::Error| ("io_error", format!("{}: {e}", path.display()));
    let mut file = std::fs::File::open(&path).map_err(io)?;
    let len = file.metadata().map_err(io)?.len();
    let mut start = match (from, tail) {
        (Some(f), _) => f.min(len),
        (None, Some(t)) => len.saturating_sub(t),
        (None, None) => 0,
    };
    let want = (len - start).min(MAX_CHUNK);
    file.seek(SeekFrom::Start(start)).map_err(io)?;
    let mut buf = vec![0u8; want as usize];
    file.read_exact(&mut buf).map_err(io)?;
    let mut text = String::from_utf8_lossy(&buf).to_string();
    // A tail read lands mid-line: drop the partial first line.
    if from.is_none() && start > 0 {
        match text.find('\n') {
            Some(i) => {
                start += (i + 1) as u64;
                text = text[i + 1..].to_string();
            }
            None => {
                text.clear();
                start = len;
            }
        }
    }
    // Only complete lines: a line still being written waits for the next read.
    let complete = text.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let body = &text[..complete];
    let end = start + body.len() as u64;
    let lines: Vec<&str> = body.lines().filter(|l| !l.trim().is_empty()).collect();
    Ok(json!({
        "session_id": session_id,
        "path": path.to_string_lossy(),
        "start": start,
        "end": end,
        "len": len,
        "lines": lines,
    }))
}

fn stat(params: &Value) -> Result<Value, Failure> {
    let ids = params
        .get("session_ids")
        .and_then(Value::as_array)
        .ok_or(("invalid_request", "session_ids is required".to_string()))?;
    let mut sessions = serde_json::Map::new();
    for id in ids.iter().filter_map(Value::as_str).take(MAX_STAT) {
        let Some(meta) = find(id).and_then(|p| std::fs::metadata(p).ok()) else {
            continue;
        };
        let mtime_ms = meta
            .modified()
            .ok()
            .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_millis() as u64);
        sessions.insert(
            id.to_string(),
            json!({ "len": meta.len(), "mtime_ms": mtime_ms }),
        );
    }
    Ok(json!({ "sessions": sessions }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn other_methods_pass_through() {
        assert_eq!(handle(r#"{"id":"1","method":"ping","params":{}}"#), None);
        assert_eq!(
            handle(
                r#"{"id":"1","method":"pane.send_text","params":{"text":"deck.transcript.read"}}"#
            ),
            None
        );
    }

    #[test]
    fn bad_session_ids_are_refused_not_looked_up() {
        let r = handle(r#"{"id":"7","method":"deck.transcript.read","params":{"session_id":"../../etc/passwd"}}"#)
            .unwrap();
        let v: Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["id"], "7");
        assert_eq!(v["error"]["code"], "invalid_request");
        let u = handle(r#"{"id":"8","method":"deck.transcript.nope","params":{}}"#).unwrap();
        assert!(u.contains("unknown_method"), "{u}");
    }

    #[test]
    fn read_and_stat_serve_the_transcript() {
        let dir = std::env::temp_dir().join(format!("deck-srv-tr-{}", std::process::id()));
        let projects = dir.join(".claude").join("projects").join("proj");
        std::fs::create_dir_all(&projects).unwrap();
        let id = "7f0cff8f-729c-4907-b694-6f5ace18360e";
        std::fs::write(
            projects.join(format!("{id}.jsonl")),
            "{\"a\":1}\n{\"b\":2}\n{\"c\":3}\n{\"partial\":",
        )
        .unwrap();
        std::env::set_var("HOME", &dir);
        let call = |m: &str, p: Value| -> Value {
            let line = json!({ "id": "x", "method": m, "params": p }).to_string();
            serde_json::from_str(&handle(&line).unwrap()).unwrap()
        };
        let tail = call(
            "deck.transcript.read",
            json!({ "session_id": id, "tail": 20 }),
        );
        assert_eq!(tail["result"]["lines"], json!(["{\"c\":3}"]));
        assert_eq!(tail["result"]["start"], 16);
        let whole = call(
            "deck.transcript.read",
            json!({ "session_id": id, "from": 0 }),
        );
        assert_eq!(whole["result"]["lines"].as_array().unwrap().len(), 3);
        assert_eq!(whole["result"]["end"], 24);
        let st = call(
            "deck.transcript.stat",
            json!({ "session_ids": [id, "00000000-0000-0000-0000-000000000000"] }),
        );
        assert_eq!(st["result"]["sessions"][id]["len"], 35);
        assert!(st["result"]["sessions"]
            .get("00000000-0000-0000-0000-000000000000")
            .is_none());
        let none = call(
            "deck.transcript.read",
            json!({ "session_id": "00000000-0000-0000-0000-000000000000" }),
        );
        assert_eq!(none["result"]["lines"], json!([]));
        let _ = std::fs::remove_dir_all(dir);
    }
}
