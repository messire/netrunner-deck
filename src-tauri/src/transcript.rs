//! Claude Code transcripts as a live source for the chat view.
//!
//! Claude Code appends one JSON line per event to
//! `~/.claude/projects/<mangled cwd>/<session id>.jsonl` while the session
//! runs. The chat view reads the file's tail once, then only what was appended
//! (byte offsets), and a single poller thread watches the file the view is
//! looking at and raises `deck://transcript` when it grows. No ANSI, no
//! screen scraping: the conversation itself.

use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

pub const EVENT: &str = "deck://transcript";
const POLL: Duration = Duration::from_millis(250);
/// Longest single read; a very large tail is served in several calls.
const MAX_CHUNK: u64 = 4 * 1024 * 1024;

#[derive(Debug, Clone, Serialize)]
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

#[derive(Default)]
pub struct Watch {
    inner: Mutex<Option<(String, Option<PathBuf>, u64)>>,
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

/// Watch one transcript (the pane on screen); `None` stops watching. The file
/// may not exist yet (a fresh session): the poller keeps looking for it.
pub fn watch(app: &AppHandle, session_id: Option<String>) -> Result<(), String> {
    let next = session_id.map(|id| {
        let path = find_transcript(&id);
        let len = path
            .as_ref()
            .and_then(|p| std::fs::metadata(p).ok())
            .map(|m| m.len())
            .unwrap_or(0);
        (id, path, len)
    });
    let state = app.state::<Watch>();
    let mut guard = state
        .inner
        .lock()
        .map_err(|_| "watch poisoned".to_string())?;
    *guard = next;
    Ok(())
}

/// One thread for the app's lifetime: size polling is cheap and needs no
/// extra crate; 250 ms is well under Claude Code's own write cadence.
pub fn start_poller(app: AppHandle) {
    std::thread::spawn(move || loop {
        std::thread::sleep(POLL);
        let state = app.state::<Watch>();
        let Ok(mut guard) = state.inner.lock() else {
            continue;
        };
        let Some((id, path, last)) = guard.as_mut() else {
            continue;
        };
        if path.is_none() {
            *path = find_transcript(id);
        }
        let len = path
            .as_ref()
            .and_then(|p| std::fs::metadata(p).ok())
            .map(|m| m.len())
            .unwrap_or(*last);
        if len != *last {
            *last = len;
            let payload = serde_json::json!({ "session_id": id, "len": len });
            drop(guard);
            let _ = app.emit(EVENT, payload);
        }
    });
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
