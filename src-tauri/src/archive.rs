//! Session archive ("cold storage"): close a Herdr workspace without losing
//! it. An entry remembers the workspace label, its checkout (worktree) and
//! every Claude session id Herdr reported for its panes, so the space can be
//! reopened later with `claude --resume`. A short description is written by a
//! shadow `claude -p` run over the session transcript.
//!
//! Storage: `<app_data_dir>/archive.json`, one JSON array; rewritten whole on
//! every change (tens of entries at most).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};

use crate::herdr;

pub const EVENT: &str = "deck://archive";
const SUMMARY_TIMEOUT: Duration = Duration::from_secs(150);
const SUMMARY_MODEL: &str = "haiku";
const SUMMARY_PROMPT: &str = "Ниже — фрагменты транскрипта сессии Claude Code (сначала первое сообщение пользователя, затем последние реплики). \
Опиши по-русски в 1–2 предложениях (до 220 символов), что делалось в этой сессии и чем она закончилась. \
Без вступлений, без markdown, без кавычек — только сам текст описания.";
const TRANSCRIPT_HEAD_CHARS: usize = 1500;
const TRANSCRIPT_TAIL_CHARS: usize = 9000;
/// Shell start-up (profile, Herdr banner) before the resume command is typed.
const RESUME_SETTLE: Duration = Duration::from_millis(1800);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ArchivedSession {
    pub tab_label: String,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    pub cwd: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Entry {
    pub id: String,
    pub archived_at_ms: u64,
    pub label: String,
    #[serde(default)]
    pub repo_name: Option<String>,
    #[serde(default)]
    pub branch: Option<String>,
    pub cwd: String,
    #[serde(default)]
    pub repo_root: Option<String>,
    #[serde(default)]
    pub linked_worktree: bool,
    pub sessions: Vec<ArchivedSession>,
    #[serde(default)]
    pub summary: Option<String>,
    /// `pending` | `ready` | `failed` | `none` (no Claude session to summarize).
    pub summary_state: String,
    #[serde(default)]
    pub summary_error: Option<String>,
    /// `archived` | `restored`
    pub status: String,
    #[serde(default)]
    pub restored_at_ms: Option<u64>,
    #[serde(default)]
    pub restored_workspace_id: Option<String>,
}

pub struct Store {
    path: PathBuf,
    entries: Mutex<Vec<Entry>>,
}

impl Store {
    pub fn open(dir: &Path) -> Store {
        let path = dir.join("archive.json");
        let entries = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<Vec<Entry>>(&text).ok())
            .unwrap_or_default();
        Store {
            path,
            entries: Mutex::new(entries),
        }
    }

    pub fn list(&self) -> Vec<Entry> {
        self.entries.lock().map(|e| e.clone()).unwrap_or_default()
    }

    /// Apply `f` to the entries and persist. Returns what `f` returned.
    fn update<T>(&self, f: impl FnOnce(&mut Vec<Entry>) -> T) -> Result<T, String> {
        let mut guard = self
            .entries
            .lock()
            .map_err(|_| "archive store poisoned".to_string())?;
        let out = f(&mut guard);
        save(&self.path, &guard)?;
        Ok(out)
    }

    fn get(&self, id: &str) -> Result<Entry, String> {
        self.list()
            .into_iter()
            .find(|e| e.id == id)
            .ok_or_else(|| format!("archive entry {id} not found"))
    }
}

fn save(path: &Path, entries: &[Entry]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    }
    let text = serde_json::to_string_pretty(entries).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("replace {}: {e}", path.display()))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn new_id() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    format!(
        "{:x}{:02x}",
        now_ms(),
        N.fetch_add(1, Ordering::Relaxed) & 0xff
    )
}

/// Herdr reports repo paths with the Windows verbatim prefix; git and the
/// filesystem are happier without it.
pub fn plain_path(p: &str) -> String {
    p.strip_prefix(r"\\?\").unwrap_or(p).to_string()
}

fn str_at<'a>(v: &'a Value, keys: &[&str]) -> Option<&'a str> {
    let mut cur = v;
    for k in keys {
        cur = cur.get(k)?;
    }
    cur.as_str()
}

// ---- archive (close) -------------------------------------------------------

/// Build the entry for a live workspace from Herdr's current facts.
pub fn snapshot_workspace(workspace_id: &str) -> Result<Entry, String> {
    let ws = herdr::call("workspace.get", json!({ "workspace_id": workspace_id }))
        .map_err(|e| e.to_string())?;
    let ws = ws.get("workspace").cloned().unwrap_or(ws);
    let label = str_at(&ws, &["label"]).unwrap_or(workspace_id).to_string();
    let wt = ws.get("worktree").filter(|v| !v.is_null());
    let tabs = herdr::call("tab.list", json!({ "workspace_id": workspace_id }))
        .map_err(|e| e.to_string())?;
    let panes = herdr::call("pane.list", json!({})).map_err(|e| e.to_string())?;
    let tab_label = |tab_id: &str| -> String {
        tabs.get("tabs")
            .and_then(Value::as_array)
            .and_then(|list| list.iter().find(|t| str_at(t, &["tab_id"]) == Some(tab_id)))
            .and_then(|t| str_at(t, &["label"]))
            .unwrap_or(tab_id)
            .to_string()
    };
    let mut sessions = Vec::new();
    for pane in panes
        .get("panes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if str_at(pane, &["workspace_id"]) != Some(workspace_id) {
            continue;
        }
        sessions.push(ArchivedSession {
            tab_label: tab_label(str_at(pane, &["tab_id"]).unwrap_or("")),
            agent: str_at(pane, &["agent"]).map(str::to_string),
            session_id: str_at(pane, &["agent_session", "value"])
                .filter(|_| str_at(pane, &["agent_session", "kind"]) == Some("id"))
                .map(str::to_string),
            cwd: str_at(pane, &["cwd"])
                .map(str::to_string)
                .unwrap_or_default(),
        });
    }
    let cwd = wt
        .and_then(|w| str_at(w, &["checkout_path"]))
        .map(plain_path)
        .or_else(|| {
            sessions
                .iter()
                .map(|s| s.cwd.clone())
                .find(|c| !c.is_empty())
        })
        .ok_or_else(|| format!("{label}: no checkout path and no pane cwd to come back to"))?;
    let has_claude = sessions.iter().any(|s| s.session_id.is_some());
    Ok(Entry {
        id: new_id(),
        archived_at_ms: now_ms(),
        label,
        repo_name: wt
            .and_then(|w| str_at(w, &["repo_name"]))
            .map(str::to_string),
        branch: wt.and_then(|w| str_at(w, &["branch"])).map(str::to_string),
        cwd,
        repo_root: wt.and_then(|w| str_at(w, &["repo_root"])).map(plain_path),
        linked_worktree: wt
            .and_then(|w| w.get("is_linked_worktree"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        sessions,
        summary: None,
        summary_state: if has_claude { "pending" } else { "none" }.to_string(),
        summary_error: None,
        status: "archived".to_string(),
        restored_at_ms: None,
        restored_workspace_id: None,
    })
}

/// Two archives of the same Claude sessions are one entry (the newer wins).
fn same_sessions(a: &Entry, b: &Entry) -> bool {
    let ids = |e: &Entry| -> Vec<String> {
        let mut v: Vec<String> = e
            .sessions
            .iter()
            .filter_map(|s| s.session_id.clone())
            .collect();
        v.sort();
        v
    };
    let (x, y) = (ids(a), ids(b));
    !x.is_empty() && x == y
}

pub fn archive_and_close(
    app: &AppHandle,
    workspace_id: &str,
    close_group: bool,
) -> Result<Entry, String> {
    let entry = snapshot_workspace(workspace_id)?;
    let mut params = json!({ "workspace_id": workspace_id });
    if close_group {
        params["close_group"] = Value::Bool(true);
    }
    herdr::call("workspace.close", params).map_err(|e| e.to_string())?;
    let store = app.state::<Store>();
    let saved = store.update(|entries| {
        entries.retain(|e| !same_sessions(e, &entry));
        entries.insert(0, entry.clone());
        entry.clone()
    })?;
    let _ = app.emit(EVENT, "changed");
    if saved.summary_state == "pending" {
        spawn_summary(app.clone(), saved.id.clone());
    }
    Ok(saved)
}

// ---- restore ---------------------------------------------------------------

pub fn restore(app: &AppHandle, id: &str) -> Result<Value, String> {
    let store = app.state::<Store>();
    let entry = store.get(id)?;
    if !Path::new(&entry.cwd).is_dir() {
        return Err(format!("directory is gone: {}", entry.cwd));
    }
    // A linked worktree reopens through Herdr so the space gets its worktree
    // provenance back; anything else is a plain workspace in the same cwd.
    let opened = match entry.repo_root.as_deref().filter(|_| entry.linked_worktree) {
        Some(root) if Path::new(root).is_dir() => herdr::call(
            "worktree.open",
            json!({ "cwd": root, "path": entry.cwd, "label": entry.label, "focus": true, "trust_repository": true }),
        )
        .or_else(|first| {
            tracing::warn!(%first, "worktree.open failed; falling back to workspace.create");
            herdr::call("workspace.create", json!({ "cwd": entry.cwd, "label": entry.label, "focus": true }))
        }),
        _ => herdr::call("workspace.create", json!({ "cwd": entry.cwd, "label": entry.label, "focus": true })),
    }
    .map_err(|e| e.to_string())?;
    let workspace_id = str_at(&opened, &["workspace", "workspace_id"])
        .ok_or("restore: no workspace in Herdr's reply")?
        .to_string();
    let root_pane = str_at(&opened, &["root_pane", "pane_id"]).map(str::to_string);

    // Pane 1 is the workspace's root pane; every further Claude session gets its own tab.
    let mut targets: Vec<(String, String)> = Vec::new(); // (pane_id, session_id)
    let mut root_free = root_pane;
    for s in entry.sessions.iter() {
        let Some(sid) = s.session_id.as_deref() else {
            continue;
        };
        let pane = match root_free.take() {
            Some(p) => p,
            None => {
                let cwd = if Path::new(&s.cwd).is_dir() {
                    s.cwd.as_str()
                } else {
                    entry.cwd.as_str()
                };
                let tab = herdr::call(
                    "tab.create",
                    json!({ "workspace_id": workspace_id, "cwd": cwd, "label": s.tab_label, "focus": false }),
                )
                .map_err(|e| e.to_string())?;
                match str_at(&tab, &["root_pane", "pane_id"]) {
                    Some(p) => p.to_string(),
                    None => continue,
                }
            }
        };
        targets.push((pane, sid.to_string()));
    }
    let resumed = targets.len();
    std::thread::spawn(move || {
        std::thread::sleep(RESUME_SETTLE);
        for (pane, sid) in targets {
            let cmd = format!("claude --resume {sid}");
            if let Err(e) = herdr::call("pane.send_text", json!({ "pane_id": pane, "text": cmd }))
                .and_then(|_| {
                    herdr::call(
                        "pane.send_keys",
                        json!({ "pane_id": pane, "keys": ["enter"] }),
                    )
                })
            {
                tracing::warn!(%e, pane, "resume command not delivered");
            }
            std::thread::sleep(Duration::from_millis(300));
        }
    });
    store.update(|entries| {
        if let Some(e) = entries.iter_mut().find(|e| e.id == id) {
            e.status = "restored".into();
            e.restored_at_ms = Some(now_ms());
            e.restored_workspace_id = Some(workspace_id.clone());
        }
    })?;
    let _ = app.emit(EVENT, "changed");
    Ok(json!({ "workspace_id": workspace_id, "resumed": resumed }))
}

// ---- delete ----------------------------------------------------------------

/// Only a *linked* worktree that is not the repository itself may be removed.
pub fn removable_worktree(entry: &Entry) -> Result<(String, String), String> {
    if !entry.linked_worktree {
        return Err("not a linked worktree: the main checkout is never removed".into());
    }
    let root = entry
        .repo_root
        .clone()
        .ok_or("no repository root recorded")?;
    let same = |a: &str, b: &str| {
        std::fs::canonicalize(a)
            .ok()
            .zip(std::fs::canonicalize(b).ok())
            .map(|(x, y)| x == y)
            .unwrap_or(a == b)
    };
    if same(&root, &entry.cwd) {
        return Err("worktree path equals the repository root; refusing".into());
    }
    Ok((root, entry.cwd.clone()))
}

fn git(root: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    } else {
        Err(format!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

pub fn delete(app: &AppHandle, id: &str, remove_worktree: bool) -> Result<Value, String> {
    let store = app.state::<Store>();
    let entry = store.get(id)?;
    let mut removed_path: Option<String> = None;
    if remove_worktree {
        let (root, path) = removable_worktree(&entry)?;
        if Path::new(&path).exists() {
            git(&root, &["worktree", "remove", "--force", &path])?;
        }
        let _ = git(&root, &["worktree", "prune"]);
        removed_path = Some(path);
    }
    store.update(|entries| entries.retain(|e| e.id != id))?;
    let _ = app.emit(EVENT, "changed");
    Ok(json!({ "removed_worktree": removed_path }))
}

// ---- summary ---------------------------------------------------------------

fn home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    let key = "USERPROFILE";
    #[cfg(not(windows))]
    let key = "HOME";
    std::env::var_os(key)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// `~/.claude/projects/*/<session_id>.jsonl` — the project directory name is a
/// mangled cwd, so the file is found by its name instead of derived.
pub fn transcript_path(session_id: &str) -> Option<PathBuf> {
    if session_id.is_empty() || session_id.contains(['/', '\\', '.']) {
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

/// First user message plus the tail of the conversation, text parts only.
pub fn transcript_excerpt(jsonl: &str) -> String {
    let mut turns: Vec<(String, String)> = Vec::new();
    for line in jsonl.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let role = match str_at(&v, &["type"]) {
            Some("user") => "USER",
            Some("assistant") => "CLAUDE",
            _ => continue,
        };
        let content = match v.get("message").and_then(|m| m.get("content")) {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Array(parts)) => parts
                .iter()
                .filter(|p| str_at(p, &["type"]) == Some("text"))
                .filter_map(|p| str_at(p, &["text"]))
                .collect::<Vec<_>>()
                .join("\n"),
            _ => continue,
        };
        let content = content.trim();
        if content.is_empty()
            || content.starts_with("<local-command")
            || content.starts_with("<command-name>")
        {
            continue;
        }
        turns.push((role.to_string(), content.to_string()));
    }
    let Some((first_role, first)) = turns.first().cloned() else {
        return String::new();
    };
    let mut out = format!(
        "[{first_role}] {}\n",
        take_chars(&first, TRANSCRIPT_HEAD_CHARS)
    );
    let mut tail = String::new();
    for (role, text) in turns.iter().skip(1).rev() {
        let piece = format!("[{role}] {}\n", take_chars(text, 1200));
        if tail.chars().count() + piece.chars().count() > TRANSCRIPT_TAIL_CHARS {
            break;
        }
        tail.insert_str(0, &piece);
    }
    if !tail.is_empty() {
        out.push_str("...\n");
        out.push_str(&tail);
    }
    out
}

fn take_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max).collect();
        t.push('…');
        t
    }
}

fn resolve_claude() -> Option<PathBuf> {
    #[cfg(windows)]
    let names = ["claude.exe", "claude.cmd", "claude"];
    #[cfg(not(windows))]
    let names = ["claude"];
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    if let Some(home) = home_dir() {
        dirs.push(home.join(".local").join("bin"));
    }
    dirs.into_iter()
        .flat_map(|d| names.iter().map(move |n| d.join(n)))
        .find(|c| c.is_file())
}

/// `claude -p` over the excerpt, with a hard deadline. Runs in the app data
/// dir so no project settings or hooks apply to the shadow query.
fn summarize_text(workdir: &Path, excerpt: &str) -> Result<String, String> {
    let claude = resolve_claude().ok_or("claude CLI not found (PATH, ~/.local/bin)")?;
    let mut child = Command::new(&claude)
        .args([
            "-p",
            SUMMARY_PROMPT,
            "--model",
            SUMMARY_MODEL,
            "--output-format",
            "text",
        ])
        .current_dir(workdir)
        .env_remove("CLAUDECODE")
        .env_remove("CLAUDE_CODE_ENTRYPOINT")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn {}: {e}", claude.display()))?;
    {
        use std::io::Write;
        let mut stdin = child.stdin.take().ok_or("no stdin")?;
        let _ = stdin.write_all(excerpt.as_bytes());
    }
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() > SUMMARY_TIMEOUT => {
                let _ = child.kill();
                return Err(format!("claude -p exceeded {}s", SUMMARY_TIMEOUT.as_secs()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(200)),
            Err(e) => return Err(format!("wait: {e}")),
        }
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !out.status.success() || text.is_empty() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if err.is_empty() {
            format!("claude exited {}", out.status)
        } else {
            take_chars(&err, 300)
        });
    }
    Ok(take_chars(&text, 400))
}

pub fn spawn_summary(app: AppHandle, id: String) {
    std::thread::spawn(move || {
        let result = (|| -> Result<String, String> {
            let store = app.state::<Store>();
            let entry = store.get(&id)?;
            let mut excerpt = String::new();
            for s in entry.sessions.iter() {
                let Some(sid) = s.session_id.as_deref() else {
                    continue;
                };
                let Some(path) = transcript_path(sid) else {
                    continue;
                };
                let text = std::fs::read_to_string(&path)
                    .map_err(|e| format!("{}: {e}", path.display()))?;
                let part = transcript_excerpt(&text);
                if part.is_empty() {
                    continue;
                }
                if !excerpt.is_empty() {
                    excerpt.push_str("\n===== другая сессия в той же папке =====\n");
                }
                excerpt.push_str(&part);
            }
            if excerpt.is_empty() {
                return Err("no transcript found under ~/.claude/projects".into());
            }
            let workdir = app.path().app_data_dir().map_err(|e| e.to_string())?;
            std::fs::create_dir_all(&workdir).map_err(|e| e.to_string())?;
            summarize_text(&workdir, &excerpt)
        })();
        let store = app.state::<Store>();
        let _ = store.update(|entries| {
            if let Some(e) = entries.iter_mut().find(|e| e.id == id) {
                match &result {
                    Ok(text) => {
                        e.summary = Some(text.clone());
                        e.summary_state = "ready".into();
                        e.summary_error = None;
                    }
                    Err(err) => {
                        e.summary_state = "failed".into();
                        e.summary_error = Some(err.clone());
                    }
                }
            }
        });
        if let Err(err) = &result {
            tracing::warn!(%err, id, "archive summary failed");
        }
        let _ = app.emit(EVENT, "changed");
    });
}

pub fn resummarize(app: &AppHandle, id: &str) -> Result<(), String> {
    let store = app.state::<Store>();
    store.update(|entries| {
        if let Some(e) = entries.iter_mut().find(|e| e.id == id) {
            e.summary_state = "pending".into();
            e.summary_error = None;
        }
    })?;
    let _ = app.emit(EVENT, "changed");
    spawn_summary(app.clone(), id.to_string());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(linked: bool, root: Option<&str>, cwd: &str) -> Entry {
        Entry {
            id: "x".into(),
            archived_at_ms: 0,
            label: "L".into(),
            repo_name: None,
            branch: None,
            cwd: cwd.into(),
            repo_root: root.map(str::to_string),
            linked_worktree: linked,
            sessions: vec![],
            summary: None,
            summary_state: "none".into(),
            summary_error: None,
            status: "archived".into(),
            restored_at_ms: None,
            restored_workspace_id: None,
        }
    }

    #[test]
    fn plain_path_strips_verbatim_prefix() {
        assert_eq!(plain_path(r"\\?\C:\repo\.git"), r"C:\repo\.git");
        assert_eq!(plain_path("/home/x"), "/home/x");
    }

    #[test]
    fn transcript_excerpt_keeps_first_user_turn_and_tail_text_only() {
        let lines = [
            r#"{"type":"user","message":{"role":"user","content":"сделай X"}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"делаю"},{"type":"tool_use","name":"Bash"}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"ok"}]}}"#,
            r#"{"type":"summary","summary":"ignored"}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"готово"}]}}"#,
        ];
        let out = transcript_excerpt(&lines.join("\n"));
        assert!(out.starts_with("[USER] сделай X\n"), "{out}");
        assert!(out.contains("[CLAUDE] делаю\n"));
        assert!(out.ends_with("[CLAUDE] готово\n"));
        assert!(!out.contains("tool_result") && !out.contains("ignored"));
    }

    #[test]
    fn transcript_excerpt_bounds_the_tail() {
        let big = "x".repeat(2000);
        let mut lines = vec![r#"{"type":"user","message":{"content":"start"}}"#.to_string()];
        for _ in 0..20 {
            lines.push(format!(r#"{{"type":"assistant","message":{{"content":[{{"type":"text","text":"{big}"}}]}}}}"#));
        }
        let out = transcript_excerpt(&lines.join("\n"));
        assert!(out.chars().count() < TRANSCRIPT_HEAD_CHARS + TRANSCRIPT_TAIL_CHARS + 100);
        assert!(out.starts_with("[USER] start\n...\n"));
    }

    #[test]
    fn transcript_path_rejects_unsafe_ids() {
        assert!(transcript_path("../etc").is_none());
        assert!(transcript_path("").is_none());
    }

    #[test]
    fn removable_worktree_guards() {
        assert!(removable_worktree(&entry(false, Some("/r"), "/r/wt")).is_err());
        assert!(removable_worktree(&entry(true, None, "/r/wt")).is_err());
        assert!(removable_worktree(&entry(true, Some("/r"), "/r")).is_err());
        assert_eq!(
            removable_worktree(&entry(true, Some("/r"), "/r/wt")).unwrap(),
            ("/r".into(), "/r/wt".into())
        );
    }

    #[test]
    fn same_sessions_compares_claude_ids_only() {
        let mut a = entry(false, None, "/a");
        let mut b = entry(false, None, "/b");
        assert!(!same_sessions(&a, &b), "no ids: never the same");
        a.sessions.push(ArchivedSession {
            tab_label: "1".into(),
            agent: Some("claude".into()),
            session_id: Some("s1".into()),
            cwd: "/a".into(),
        });
        b.sessions.push(ArchivedSession {
            tab_label: "2".into(),
            agent: Some("claude".into()),
            session_id: Some("s1".into()),
            cwd: "/b".into(),
        });
        assert!(same_sessions(&a, &b));
    }

    #[test]
    fn store_roundtrip() {
        let dir = std::env::temp_dir().join(format!("deck-archive-test-{}", new_id()));
        let store = Store::open(&dir);
        store.update(|e| e.push(entry(false, None, "/a"))).unwrap();
        let again = Store::open(&dir);
        assert_eq!(again.list().len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }
}
