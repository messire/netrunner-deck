//! Claude Code transcripts as a live source for the chat view.
//!
//! Claude Code appends one JSON line per event to
//! `~/.claude/projects/<mangled cwd>/<session id>.jsonl` while the session
//! runs. The chat view reads the file's tail once, then only what was appended
//! (byte offsets), and a single poller thread watches the file the view is
//! looking at and raises `deck://transcript` when it grows. No ANSI, no
//! screen scraping: the conversation itself.
//!
//! The transcript lives on the machine the agent runs on, so it is asked
//! for where the pane is: a server that serves transcripts (deck-server's
//! `deck.transcript.read` / `deck.transcript.stat`) is asked over the same
//! connection as everything else, local or remote. Only a local server
//! without them (stock Herdr) falls back to reading the file here.

use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

pub const EVENT: &str = "deck://transcript";
const POLL: Duration = Duration::from_millis(250);
/// Longest single read; a very large tail is served in several calls.
const MAX_CHUNK: u64 = 4 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chunk {
    pub session_id: String,
    pub path: String,
    /// Byte offset the returned lines start at (line-aligned).
    pub start: u64,
    /// Byte offset after the last complete line; pass it back as `from`.
    pub end: u64,
    /// File length at read time; `end < len` means more is already there.
    pub len: u64,
    pub lines: Vec<String>,
}

/// The transcript on screen: its server, session, where it is read from,
/// and the length last reported.
struct Watched {
    server: Option<String>,
    id: String,
    /// `None`: asked of the server; `Some(path)`: a local file (maybe not there yet).
    file: Option<Option<PathBuf>>,
    len: u64,
}

#[derive(Default)]
pub struct Watch {
    inner: Mutex<Option<Watched>>,
    /// Servers that answered `deck.transcript.*` with "unknown method", and when.
    unsupported: Mutex<std::collections::HashMap<String, std::time::Instant>>,
}

/// How long a server's "no transcripts here" is believed (it may update itself).
const UNSUPPORTED_FOR: Duration = Duration::from_secs(120);

/// The server answered that it has no such method (stock Herdr).
fn is_unknown_method(err: &crate::herdr::HerdrError) -> bool {
    matches!(err, crate::herdr::HerdrError::Api { message, .. }
        if message.contains("deck.transcript") || message.contains("unknown variant") || message.contains("unknown method"))
}

/// Where a server's transcripts come from.
enum Source {
    /// The server serves them (any machine).
    Server(crate::runtime::SharedRuntime),
    /// A local server that does not: Deck reads `~/.claude` itself.
    Local,
}

fn source(app: &AppHandle, server: Option<&str>) -> Result<Source, String> {
    let rt = crate::servers::runtime_for(app, server)?;
    let key = server.unwrap_or("").to_string();
    let known_bad = app
        .state::<Watch>()
        .unsupported
        .lock()
        .ok()
        .and_then(|m| m.get(&key).map(|at| at.elapsed() < UNSUPPORTED_FOR))
        .unwrap_or(false);
    if known_bad {
        return if rt.is_remote() {
            Err(not_served())
        } else {
            Ok(Source::Local)
        };
    }
    Ok(Source::Server(rt))
}

fn not_served() -> String {
    "this server does not serve transcripts (deck-server does); the chat view of a remote machine needs it".into()
}

/// Remember that `server` has no transcript methods.
fn mark_unsupported(app: &AppHandle, server: Option<&str>) {
    if let Ok(mut m) = app.state::<Watch>().unsupported.lock() {
        m.insert(server.unwrap_or("").to_string(), std::time::Instant::now());
    }
}

/// [`read`] wherever the pane's server keeps the transcript.
pub fn read_on(
    app: &AppHandle,
    server: Option<&str>,
    session_id: &str,
    from: Option<u64>,
    tail: Option<u64>,
) -> Result<Chunk, String> {
    if let Source::Server(rt) = source(app, server)? {
        match rt.transcript_read(session_id, from, tail) {
            Ok(v) => {
                return serde_json::from_value(v).map_err(|e| format!("transcript reply: {e}"))
            }
            Err(err) if is_unknown_method(&err) => {
                mark_unsupported(app, server);
                if rt.is_remote() {
                    return Err(not_served());
                }
            }
            Err(err) => return Err(err.to_string()),
        }
    }
    read(session_id, from, tail)
}

type Stats = std::collections::HashMap<String, (u64, u64)>;

/// `(len, mtime_ms)` of each session's transcript on its server; `None` when
/// a local server does not serve them (read the files here).
fn stat_on(
    app: &AppHandle,
    server: Option<&str>,
    session_ids: &[String],
) -> Result<Option<Stats>, String> {
    let Source::Server(rt) = source(app, server)? else {
        return Ok(None);
    };
    match rt.transcript_stat(session_ids) {
        Ok(v) => {
            let mut out = Stats::new();
            if let Some(map) = v.get("sessions").and_then(|s| s.as_object()) {
                for (id, st) in map {
                    let len = st.get("len").and_then(|x| x.as_u64()).unwrap_or(0);
                    let ms = st.get("mtime_ms").and_then(|x| x.as_u64()).unwrap_or(0);
                    out.insert(id.clone(), (len, ms));
                }
            }
            Ok(Some(out))
        }
        Err(err) if is_unknown_method(&err) => {
            mark_unsupported(app, server);
            if rt.is_remote() {
                Err(not_served())
            } else {
                Ok(None)
            }
        }
        Err(err) => Err(err.to_string()),
    }
}

/// [`mtimes`] wherever the server keeps the transcripts.
pub fn mtimes_on(
    app: &AppHandle,
    server: Option<&str>,
    session_ids: &[String],
) -> Result<std::collections::HashMap<String, u64>, String> {
    match stat_on(app, server, session_ids)? {
        Some(m) => Ok(m.into_iter().map(|(id, (_, ms))| (id, ms)).collect()),
        None => Ok(mtimes(session_ids)),
    }
}

fn home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    let key = "USERPROFILE";
    #[cfg(not(windows))]
    let key = "HOME";
    std::env::var_os(key)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// `<projects>/<any dir>/<id>.jsonl`: the project directory name is a mangled
/// cwd, so the file is found by its name (a few dozen directories).
pub fn find_transcript(session_id: &str) -> Option<PathBuf> {
    if !crate::archive::looks_like_session_id(session_id) {
        return None;
    }
    let projects = home_dir()?.join(".claude").join("projects");
    let name = format!("{session_id}.jsonl");
    for dir in std::fs::read_dir(projects).ok()?.flatten() {
        let candidate = dir.path().join(&name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Modified time in ms since the epoch for each session that has a transcript.
pub fn mtimes(session_ids: &[String]) -> std::collections::HashMap<String, u64> {
    session_ids
        .iter()
        .filter_map(|id| {
            let path = find_transcript(id)?;
            let modified = std::fs::metadata(path).ok()?.modified().ok()?;
            let ms = modified
                .duration_since(std::time::UNIX_EPOCH)
                .ok()?
                .as_millis() as u64;
            Some((id.clone(), ms))
        })
        .collect()
}

/// Read complete lines from `from` (or from `len - tail` when `from` is None),
/// at most `MAX_CHUNK` bytes.
pub fn read(session_id: &str, from: Option<u64>, tail: Option<u64>) -> Result<Chunk, String> {
    // A session that has not spoken yet has no file: an empty chunk, not an
    // error; the poller reports when the file appears.
    let Some(path) = find_transcript(session_id) else {
        return Ok(Chunk {
            session_id: session_id.to_string(),
            path: String::new(),
            start: 0,
            end: 0,
            len: 0,
            lines: Vec::new(),
        });
    };
    let mut file = std::fs::File::open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let len = file.metadata().map_err(|e| e.to_string())?.len();
    let mut start = match (from, tail) {
        (Some(f), _) => f.min(len),
        (None, Some(t)) => len.saturating_sub(t),
        (None, None) => 0,
    };
    let want = (len - start).min(MAX_CHUNK);
    file.seek(SeekFrom::Start(start))
        .map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; want as usize];
    file.read_exact(&mut buf).map_err(|e| e.to_string())?;
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
    let lines = body
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(str::to_string)
        .collect();
    Ok(Chunk {
        session_id: session_id.to_string(),
        path: path.to_string_lossy().to_string(),
        start,
        end,
        len,
        lines,
    })
}

/// Watch one transcript (the pane on screen) on `server`; `None` stops
/// watching. The file may not exist yet (a fresh session): the poller keeps
/// looking for it.
pub fn watch(
    app: &AppHandle,
    server: Option<String>,
    session_id: Option<String>,
) -> Result<(), String> {
    let next = match session_id {
        None => None,
        Some(id) => {
            let local = matches!(source(app, server.as_deref())?, Source::Local);
            let (file, len) = if local {
                let path = find_transcript(&id);
                let len = path
                    .as_ref()
                    .and_then(|p| std::fs::metadata(p).ok())
                    .map(|m| m.len())
                    .unwrap_or(0);
                (Some(path), len)
            } else {
                // Unknown until the first poll; the view has just read it anyway.
                (None, UNKNOWN_LEN)
            };
            Some(Watched {
                server,
                id,
                file,
                len,
            })
        }
    };
    let state = app.state::<Watch>();
    let mut guard = state
        .inner
        .lock()
        .map_err(|_| "watch poisoned".to_string())?;
    *guard = next;
    Ok(())
}

const UNKNOWN_LEN: u64 = u64::MAX;

/// Ask a server about its transcript at most this often (a call is a round
/// trip, maybe over SSH); a local file is looked at every [`POLL`].
const SERVER_POLL: Duration = Duration::from_secs(1);

/// One thread for the app's lifetime: size polling is cheap and needs no
/// extra crate; 250 ms is well under Claude Code's own write cadence.
pub fn start_poller(app: AppHandle) {
    std::thread::spawn(move || {
        let mut server_polled = std::time::Instant::now();
        loop {
            std::thread::sleep(POLL);
            if let Some((id, len)) = poll_once(&app, &mut server_polled) {
                let _ = app.emit(EVENT, serde_json::json!({ "session_id": id, "len": len }));
            }
        }
    });
}

/// One look at the watched transcript: `Some((id, len))` when it grew.
fn poll_once(app: &AppHandle, server_polled: &mut std::time::Instant) -> Option<(String, u64)> {
    let state = app.state::<Watch>();
    let mut guard = state.inner.lock().ok()?;
    let w = guard.as_mut()?;
    if let Some(path) = &mut w.file {
        if path.is_none() {
            *path = find_transcript(&w.id);
        }
        let len = path
            .as_ref()
            .and_then(|p| std::fs::metadata(p).ok())
            .map(|m| m.len())
            .unwrap_or(w.len);
        if len == w.len {
            return None;
        }
        w.len = len;
        return Some((w.id.clone(), len));
    }
    if server_polled.elapsed() < SERVER_POLL {
        return None;
    }
    *server_polled = std::time::Instant::now();
    let (server, id) = (w.server.clone(), w.id.clone());
    // No lock across the call: it is a round trip to the server.
    drop(guard);
    let stat = stat_on(app, server.as_deref(), std::slice::from_ref(&id));
    let mut guard = state.inner.lock().ok()?;
    // The view may have moved on while the call ran.
    let w = guard
        .as_mut()
        .filter(|w| w.id == id && w.server == server)?;
    let len = match stat {
        Ok(Some(m)) => m.get(&id).map(|(len, _)| *len).unwrap_or(0),
        // A local server without transcript methods after all: read the file.
        Ok(None) => {
            w.file = Some(find_transcript(&id));
            return None;
        }
        Err(_) => return None,
    };
    let grew = w.len != UNKNOWN_LEN && w.len != len;
    w.len = len;
    grew.then_some((id, len))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_read_is_line_aligned_and_resumable() {
        let dir = std::env::temp_dir().join(format!("deck-tr-{}", std::process::id()));
        let projects = dir.join(".claude").join("projects").join("proj");
        std::fs::create_dir_all(&projects).unwrap();
        let id = "7f0cff8f-729c-4907-b694-6f5ace18360e";
        let path = projects.join(format!("{id}.jsonl"));
        std::fs::write(&path, "{\"a\":1}\n{\"b\":2}\n{\"c\":3}\n{\"partial\":").unwrap();
        // Point the lookup at the temp home for this test.
        #[cfg(windows)]
        std::env::set_var("USERPROFILE", &dir);
        #[cfg(not(windows))]
        std::env::set_var("HOME", &dir);

        let tail = read(id, None, Some(20)).unwrap();
        assert_eq!(
            tail.lines,
            vec!["{\"c\":3}"],
            "partial first and last lines are dropped: {tail:?}"
        );
        assert_eq!(tail.start, 16);
        let whole = read(id, Some(0), None).unwrap();
        assert_eq!(whole.lines.len(), 3);
        assert_eq!(whole.end, 24);
        let more = read(id, Some(whole.end), None).unwrap();
        assert!(more.lines.is_empty());
        std::fs::write(&path, "{\"a\":1}\n{\"b\":2}\n{\"c\":3}\n{\"partial\":4}\n").unwrap();
        let more = read(id, Some(whole.end), None).unwrap();
        assert_eq!(more.lines, vec!["{\"partial\":4}"]);
        let _ = std::fs::remove_dir_all(dir);
    }
}
