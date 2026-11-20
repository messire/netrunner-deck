//! Session archive ("cold storage"): every Claude session on this computer
//! that is not open in Herdr right now, with what is needed to bring it back.
//!
//! Nothing has to be "archived" first — the list is a scan of what already
//! exists on disk, so sessions closed long before Deck existed are there too:
//!
//! * Herdr's `session.json` (live state) and `session-snapshots/*.json`
//!   (its periodic copies) give workspaces: label, worktree provenance, tabs,
//!   and the Claude session id Herdr recorded for every pane.
//! * `~/.claude/projects/*/<id>.jsonl` are the transcripts: a session is
//!   resumable while its transcript exists. `~/.claude/history.jsonl` gives
//!   the prompts and the cwd per session cheaply.
//! * `<app_data>/archive.json` is Deck's own: descriptions written by the
//!   shadow `claude -p`, restore status, and tombstones for deleted sessions.
//!
//! Entries are keyed by their Claude session ids (sorted, joined with `+`);
//! a Herdr workspace with several Claude panes is one entry.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};

use crate::runtime::{self, Runtime};

pub const EVENT: &str = "deck://archive";
const SUMMARY_TIMEOUT: Duration = Duration::from_secs(150);
const SUMMARY_MODEL: &str = "haiku";
const SUMMARY_PROMPT: &str = "Ниже — фрагменты транскрипта сессии Claude Code (сначала первое сообщение пользователя, затем последние реплики). \
Опиши по-русски в 1–2 предложениях (до 220 символов), что делалось в этой сессии и чем она закончилась. \
Без вступлений, без markdown, без кавычек — только сам текст описания.";
const TRANSCRIPT_HEAD_CHARS: usize = 1500;
const TRANSCRIPT_TAIL_CHARS: usize = 9000;
/// Transcript head read for cwd / first prompt when history.jsonl lacks the session.
const HEAD_BYTES: u64 = 256 * 1024;
/// A transcript can be hundreds of MB; the description needs its start and its end only.
const EXCERPT_HEAD_BYTES: u64 = 512 * 1024;
const EXCERPT_TAIL_BYTES: u64 = 2 * 1024 * 1024;
/// Shell start-up (profile, Herdr banner) before the resume command is typed.
const RESUME_SETTLE: Duration = Duration::from_millis(1800);
/// Descriptions are written automatically only for recent sessions, a few per scan.
const AUTO_SUMMARY_MAX_AGE: Duration = Duration::from_secs(14 * 24 * 3600);
const AUTO_SUMMARY_PER_SCAN: usize = 4;
/// … and at most this many per app run; WRITE in the table is unbounded.
const AUTO_SUMMARY_PER_RUN: usize = 30;

// ---- public model ------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ArchivedSession {
    pub tab_label: String,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    pub cwd: String,
    /// The transcript exists, so `claude --resume` can bring it back.
    #[serde(default)]
    pub resumable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Entry {
    /// Sorted Claude session ids joined with `+`.
    pub id: String,
    /// `herdr` (a workspace Herdr knew) or `claude` (transcript only).
    pub source: String,
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
    /// Last activity: newest transcript write or prompt.
    pub last_active_ms: u64,
    /// First prompt of the primary session (from history.jsonl / transcript).
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub prompts: u32,
    #[serde(default)]
    pub summary: Option<String>,
    /// `pending` | `ready` | `failed` | `none` (nothing written yet).
    pub summary_state: String,
    #[serde(default)]
    pub summary_error: Option<String>,
    /// `closed` | `restored`
    pub status: String,
    #[serde(default)]
    pub restored_at_ms: Option<u64>,
    #[serde(default)]
    pub restored_workspace_id: Option<String>,
}

// ---- Deck's own file ---------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Record {
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub summary_state: Option<String>,
    #[serde(default)]
    pub summary_error: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub restored_at_ms: Option<u64>,
    #[serde(default)]
    pub restored_workspace_id: Option<String>,
    #[serde(default)]
    pub archived_at_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Saved {
    #[serde(default)]
    pub records: BTreeMap<String, Record>,
    /// Session ids the user deleted: never listed again even if a Herdr snapshot still names them.
    #[serde(default)]
    pub deleted: BTreeSet<String>,
    /// Workspaces Deck itself closed, so a short-lived space missing from every Herdr snapshot is still known.
    #[serde(default)]
    pub closed: Vec<WorkspaceRecord>,
    /// User-made repository groups for the ARCHIVE tree (repos not listed are in the main group).
    #[serde(default)]
    pub groups: Vec<RepoGroup>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct RepoGroup {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub repos: Vec<String>,
    #[serde(default)]
    pub hidden: bool,
}

pub struct Store {
    path: PathBuf,
    saved: Mutex<Saved>,
    /// Description queue: ids waiting for the single worker thread.
    queue: Mutex<SummaryQueue>,
}

#[derive(Default)]
struct SummaryQueue {
    waiting: std::collections::VecDeque<String>,
    queued: HashSet<String>,
    worker_running: bool,
    auto_started: usize,
}

impl Store {
    pub fn open(dir: &Path) -> Store {
        let path = dir.join("archive.json");
        let mut saved = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<Saved>(&text).ok())
            .unwrap_or_default();
        // A `pending` left over from a previous run is not being written by
        // anyone; a failure caused by the machine (memory, process spawn) rather
        // than by the transcript is worth another try.
        for r in saved.records.values_mut() {
            let reset = match r.summary_state.as_deref() {
                Some("pending") => true,
                Some("failed") => r
                    .summary_error
                    .as_deref()
                    .map(environmental_failure)
                    .unwrap_or(false),
                _ => false,
            };
            if reset {
                r.summary_state = None;
                r.summary_error = None;
            }
        }
        Store {
            path,
            saved: Mutex::new(saved),
            queue: Mutex::new(SummaryQueue::default()),
        }
    }

    pub fn snapshot(&self) -> Saved {
        self.saved.lock().map(|s| s.clone()).unwrap_or_default()
    }

    fn update<T>(&self, f: impl FnOnce(&mut Saved) -> T) -> Result<T, String> {
        let mut guard = self
            .saved
            .lock()
            .map_err(|_| "archive store poisoned".to_string())?;
        let out = f(&mut guard);
        save(&self.path, &guard)?;
        Ok(out)
    }

    fn record(&self, id: &str, f: impl FnOnce(&mut Record)) -> Result<(), String> {
        self.update(|s| f(s.records.entry(id.to_string()).or_default()))
    }
}

fn save(path: &Path, saved: &Saved) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    }
    let text = serde_json::to_string_pretty(saved).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("replace {}: {e}", path.display()))
}

/// Errors that say nothing about the session: out of memory, paging file,
/// process spawn limits, a crashed `claude.exe`.
pub fn environmental_failure(err: &str) -> bool {
    let e = err.to_lowercase();
    [
        "memory",
        "paging file",
        "insufficient system resources",
        "os error 1455",
        "os error 1450",
        "os error 8)",
        "0xc0",
        "exceeded",
    ]
    .iter()
    .any(|m| e.contains(m))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn mtime_ms(p: &Path) -> u64 {
    std::fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Herdr reports repo paths with the Windows verbatim prefix; git and the
/// filesystem are happier without it. Trailing separators go too.
pub fn plain_path(p: &str) -> String {
    let p = p.strip_prefix(r"\\?\").unwrap_or(p);
    let t = p.trim_end_matches(['\\', '/']);
    if t.is_empty() {
        p.to_string()
    } else {
        t.to_string()
    }
}

fn same_path(a: &str, b: &str) -> bool {
    let n = |s: &str| plain_path(s).replace('\\', "/").to_lowercase();
    n(a) == n(b)
}

pub(crate) fn str_at<'a>(v: &'a Value, keys: &[&str]) -> Option<&'a str> {
    let mut cur = v;
    for k in keys {
        cur = cur.get(k)?;
    }
    cur.as_str()
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

/// Herdr's config dir (its `session.json` and snapshots). `HERDR_CONFIG_DIR`
/// overrides it, as in `herdr.rs`, for a Deck pointed at another server.
fn herdr_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("HERDR_CONFIG_DIR").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    #[cfg(windows)]
    {
        std::env::var_os("APPDATA").map(|p| PathBuf::from(p).join("herdr"))
    }
    #[cfg(not(windows))]
    {
        home_dir().map(|h| h.join(".config").join("herdr"))
    }
}

pub fn key_of(ids: &[String]) -> String {
    let mut v: Vec<&str> = ids.iter().map(String::as_str).collect();
    v.sort_unstable();
    v.dedup();
    v.join("+")
}

// ---- Herdr workspaces (live state + snapshots) -------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkspaceRecord {
    pub key: String,
    pub label: String,
    pub cwd: String,
    #[serde(default)]
    pub repo_name: Option<String>,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub repo_root: Option<String>,
    #[serde(default)]
    pub linked_worktree: bool,
    pub sessions: Vec<ArchivedSession>,
    /// When this description of the workspace was taken (snapshot time or close time).
    pub seen_ms: u64,
}

/// Workspaces described by one Herdr session file (`session.json` or a snapshot).
pub fn workspaces_in(session_json: &Value, seen_ms: u64) -> Vec<WorkspaceRecord> {
    let mut out = Vec::new();
    for w in session_json
        .get("workspaces")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let wt = w.get("worktree_space").filter(|v| v.is_object());
        let mut sessions = Vec::new();
        for tab in w
            .get("tabs")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let tab_label = str_at(tab, &["custom_name"]).unwrap_or("").to_string();
            for pane in tab
                .get("panes")
                .and_then(Value::as_object)
                .into_iter()
                .flat_map(|m| m.values())
            {
                let sid = str_at(pane, &["agent_session", "value"])
                    .filter(|_| str_at(pane, &["agent_session", "kind"]) == Some("id"));
                sessions.push(ArchivedSession {
                    tab_label: tab_label.clone(),
                    agent: str_at(pane, &["agent_session", "agent"]).map(str::to_string),
                    session_id: sid.map(str::to_string),
                    cwd: str_at(pane, &["cwd"]).map(plain_path).unwrap_or_default(),
                    resumable: false,
                });
            }
        }
        let ids: Vec<String> = sessions
            .iter()
            .filter_map(|s| s.session_id.clone())
            .collect();
        if ids.is_empty() {
            continue; // shells only: nothing to resume
        }
        let wt_label = wt.and_then(|v| str_at(v, &["label"])).map(str::to_string);
        let label = str_at(w, &["custom_name"])
            .map(str::to_string)
            .or_else(|| wt_label.clone())
            .or_else(|| str_at(w, &["identity_cwd"]).map(|c| basename(&plain_path(c))))
            .unwrap_or_else(|| "space".into());
        let cwd = wt
            .and_then(|v| str_at(v, &["checkout_path"]))
            .map(plain_path)
            .or_else(|| str_at(w, &["identity_cwd"]).map(plain_path))
            .or_else(|| {
                sessions
                    .iter()
                    .map(|s| s.cwd.clone())
                    .find(|c| !c.is_empty())
            })
            .unwrap_or_default();
        let linked = wt
            .and_then(|v| v.get("is_linked_worktree"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        out.push(WorkspaceRecord {
            key: key_of(&ids),
            label,
            cwd,
            repo_name: wt
                .and_then(|v| str_at(v, &["repo_root"]))
                .map(|r| basename(&plain_path(r))),
            branch: if linked { wt_label } else { None },
            repo_root: wt.and_then(|v| str_at(v, &["repo_root"])).map(plain_path),
            linked_worktree: linked,
            sessions,
            seen_ms,
        });
    }
    out
}

fn basename(p: &str) -> String {
    Path::new(p)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| p.to_string())
}

/// Snapshot file names carry a nanosecond timestamp: `session-<ns>-<pid>-<n>.json`.
fn snapshot_time_ms(name: &str) -> Option<u64> {
    let rest = name.strip_prefix("session-")?;
    let ns: u128 = rest.split('-').next()?.parse().ok()?;
    Some((ns / 1_000_000) as u64)
}

/// Every workspace Herdr has described, newest description per key, plus the live ids.
fn herdr_workspaces() -> (HashMap<String, WorkspaceRecord>, HashSet<String>) {
    let mut by_key: HashMap<String, WorkspaceRecord> = HashMap::new();
    let mut live = HashSet::new();
    let Some(dir) = herdr_dir() else {
        return (by_key, live);
    };
    let mut files: Vec<(u64, PathBuf)> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir.join("session-snapshots")) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if let Some(ms) = snapshot_time_ms(&name) {
                files.push((ms, e.path()));
            }
        }
    }
    files.sort();
    let current = dir.join("session.json");
    files.push((mtime_ms(&current).max(u64::MAX / 2), current));
    for (i, (ms, path)) in files.iter().enumerate() {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        let is_current = i + 1 == files.len();
        for w in workspaces_in(&v, if is_current { now_ms() } else { *ms }) {
            if is_current {
                live.extend(w.sessions.iter().filter_map(|s| s.session_id.clone()));
            }
            by_key.insert(w.key.clone(), w);
        }
    }
    (by_key, live)
}

/// Ids open in Herdr right now, from the API when it answers (else `session.json`).
fn live_ids(rt: &dyn Runtime, fallback: HashSet<String>) -> HashSet<String> {
    match rt.pane_list() {
        Ok(v) => v
            .get("panes")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|p| str_at(p, &["agent_session", "value"]).map(str::to_string))
            .collect(),
        Err(_) => fallback,
    }
}

// ---- Claude transcripts & history --------------------------------------------

#[derive(Debug, Default, Clone)]
struct Transcript {
    path: PathBuf,
    mtime_ms: u64,
}

fn projects_dir() -> Option<PathBuf> {
    home_dir().map(|h| h.join(".claude").join("projects"))
}

/// Top-level `<projects>/<slug>/<id>.jsonl` only; subagent and tool-result files live in subdirectories.
fn transcripts() -> HashMap<String, Transcript> {
    let mut out = HashMap::new();
    let Some(root) = projects_dir() else {
        return out;
    };
    for proj in std::fs::read_dir(root).into_iter().flatten().flatten() {
        if !proj.path().is_dir() {
            continue;
        }
        for f in std::fs::read_dir(proj.path())
            .into_iter()
            .flatten()
            .flatten()
        {
            let path = f.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") || !path.is_file() {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if !looks_like_session_id(stem) {
                continue;
            }
            out.insert(
                stem.to_string(),
                Transcript {
                    mtime_ms: mtime_ms(&path),
                    path,
                },
            );
        }
    }
    out
}

pub fn looks_like_session_id(s: &str) -> bool {
    s.len() == 36
        && s.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
        && s.matches('-').count() == 4
}

#[derive(Debug, Default, Clone)]
struct PromptInfo {
    cwd: String,
    first: String,
    first_ms: u64,
    last_ms: u64,
    count: u32,
}

/// `~/.claude/history.jsonl`: one line per prompt with the session id and project.
fn prompt_history() -> HashMap<String, PromptInfo> {
    let mut out: HashMap<String, PromptInfo> = HashMap::new();
    let Some(path) = home_dir().map(|h| h.join(".claude").join("history.jsonl")) else {
        return out;
    };
    let Ok(file) = std::fs::File::open(path) else {
        return out;
    };
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(sid) = str_at(&v, &["sessionId"]) else {
            continue;
        };
        let ts = v.get("timestamp").and_then(Value::as_u64).unwrap_or(0);
        let display = str_at(&v, &["display"]).unwrap_or("").trim();
        let info = out.entry(sid.to_string()).or_default();
        if info.cwd.is_empty() {
            info.cwd = str_at(&v, &["project"]).map(plain_path).unwrap_or_default();
        }
        if info.first.is_empty() && !display.is_empty() {
            info.first = take_chars(display, 240);
            info.first_ms = ts;
        }
        info.last_ms = info.last_ms.max(ts);
        info.count += 1;
    }
    out
}

/// Heads already parsed, by path: `(file length when read, info)`. A transcript's
/// first lines never change once written, so a head read past `HEAD_BYTES` is
/// final; a shorter (young) file is re-read only when it has grown.
type HeadCache = Mutex<HashMap<PathBuf, (u64, PromptInfo)>>;
fn head_cache() -> &'static HeadCache {
    static CACHE: std::sync::OnceLock<HeadCache> = std::sync::OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Fallback for a transcript history.jsonl does not know: cwd and first prompt from its head.
fn transcript_head(path: &Path) -> PromptInfo {
    let len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    if let Ok(cache) = head_cache().lock() {
        if let Some((seen, info)) = cache.get(path) {
            if *seen >= HEAD_BYTES || *seen == len {
                return info.clone();
            }
        }
    }
    let info = transcript_head_uncached(path);
    if let Ok(mut cache) = head_cache().lock() {
        cache.insert(path.to_path_buf(), (len, info.clone()));
    }
    info
}

fn transcript_head_uncached(path: &Path) -> PromptInfo {
    let mut info = PromptInfo::default();
    let Ok(file) = std::fs::File::open(path) else {
        return info;
    };
    let mut text = String::new();
    let _ = file.take(HEAD_BYTES).read_to_string(&mut text);
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if info.cwd.is_empty() {
            if let Some(c) = str_at(&v, &["cwd"]) {
                info.cwd = plain_path(c);
            }
        }
        if str_at(&v, &["type"]) == Some("user")
            && v.get("isSidechain").and_then(Value::as_bool) != Some(true)
        {
            let content = match v.get("message").and_then(|m| m.get("content")) {
                Some(Value::String(s)) => s.clone(),
                Some(Value::Array(parts)) => parts
                    .iter()
                    .filter(|p| str_at(p, &["type"]) == Some("text"))
                    .filter_map(|p| str_at(p, &["text"]))
                    .collect::<Vec<_>>()
                    .join("\n"),
                _ => String::new(),
            };
            let content = content.trim();
            if !content.is_empty() && !content.starts_with('<') {
                info.first = take_chars(content, 240);
                info.count = 1;
                break;
            }
        }
    }
    info
}

// ---- the scan -----------------------------------------------------------------

/// Everything restorable on this computer that is not open in Herdr.
pub fn scan(app: &AppHandle) -> Vec<Entry> {
    let store = app.state::<Store>();
    let saved = store.snapshot();
    let shadow_dir = shadow_workdir(app).map(|p| p.to_string_lossy().to_string());

    let (mut groups, live_fallback) = herdr_workspaces();
    for w in &saved.closed {
        let newer = groups
            .get(&w.key)
            .map(|g| g.seen_ms < w.seen_ms)
            .unwrap_or(true);
        if newer {
            groups.insert(w.key.clone(), w.clone());
        }
    }
    let live = live_ids(&*runtime::of(app), live_fallback);
    let files = transcripts();
    let history = prompt_history();

    let mut entries: Vec<Entry> = Vec::new();
    let mut claimed: HashSet<String> = HashSet::new();

    for (key, w) in groups {
        let ids: Vec<&str> = key.split('+').collect();
        if ids
            .iter()
            .any(|id| live.contains(*id) || saved.deleted.contains(*id))
        {
            continue;
        }
        let mut sessions = w.sessions.clone();
        for s in sessions.iter_mut() {
            s.resumable = s
                .session_id
                .as_deref()
                .map(|id| files.contains_key(id))
                .unwrap_or(false);
        }
        if !sessions.iter().any(|s| s.resumable) {
            continue; // transcripts gone: nothing to come back to
        }
        claimed.extend(ids.iter().map(|s| s.to_string()));
        let primary = sessions
            .iter()
            .find(|s| s.resumable)
            .and_then(|s| s.session_id.clone())
            .unwrap_or_default();
        let (prompt, prompts, active) = prompt_facts(&primary, &ids, &files, &history);
        entries.push(finish(
            Entry {
                id: key,
                source: "herdr".into(),
                label: w.label,
                repo_name: w.repo_name,
                branch: w.branch,
                cwd: w.cwd,
                repo_root: w.repo_root,
                linked_worktree: w.linked_worktree,
                sessions,
                last_active_ms: active.max(w.seen_ms.min(now_ms())),
                prompt,
                prompts,
                summary: None,
                summary_state: "none".into(),
                summary_error: None,
                status: "closed".into(),
                restored_at_ms: None,
                restored_workspace_id: None,
            },
            &saved,
        ));
    }

    for (id, t) in &files {
        if claimed.contains(id) || live.contains(id) || saved.deleted.contains(id) {
            continue;
        }
        let mut info = history.get(id).cloned().unwrap_or_default();
        if info.cwd.is_empty() || info.first.is_empty() {
            let head = transcript_head(&t.path);
            if info.cwd.is_empty() {
                info.cwd = head.cwd;
            }
            if info.first.is_empty() {
                info.first = head.first;
            }
        }
        if info.cwd.is_empty() {
            continue;
        }
        if shadow_dir
            .as_deref()
            .map(|d| same_path(d, &info.cwd))
            .unwrap_or(false)
        {
            continue; // Deck's own shadow queries
        }
        let active = t.mtime_ms.max(info.last_ms);
        entries.push(finish(
            Entry {
                id: id.clone(),
                source: "claude".into(),
                label: basename(&info.cwd),
                repo_name: None,
                branch: None,
                cwd: info.cwd.clone(),
                repo_root: None,
                linked_worktree: false,
                sessions: vec![ArchivedSession {
                    tab_label: String::new(),
                    agent: Some("claude".into()),
                    session_id: Some(id.clone()),
                    cwd: info.cwd,
                    resumable: true,
                }],
                last_active_ms: active,
                prompt: (!info.first.is_empty()).then_some(info.first),
                prompts: info.count,
                summary: None,
                summary_state: "none".into(),
                summary_error: None,
                status: "closed".into(),
                restored_at_ms: None,
                restored_workspace_id: None,
            },
            &saved,
        ));
    }
    entries.sort_by_key(|e| std::cmp::Reverse(e.last_active_ms));
    entries
}

fn prompt_facts(
    primary: &str,
    ids: &[&str],
    files: &HashMap<String, Transcript>,
    history: &HashMap<String, PromptInfo>,
) -> (Option<String>, u32, u64) {
    let mut prompt = None;
    let mut prompts = 0;
    let mut active = 0;
    for id in ids {
        if let Some(t) = files.get(*id) {
            active = active.max(t.mtime_ms);
        }
        if let Some(h) = history.get(*id) {
            active = active.max(h.last_ms);
            prompts += h.count;
            if *id == primary && !h.first.is_empty() {
                prompt = Some(h.first.clone());
            }
        }
    }
    if prompt.is_none() {
        if let Some(t) = files.get(primary) {
            let head = transcript_head(&t.path);
            if !head.first.is_empty() {
                prompt = Some(head.first);
            }
        }
    }
    (prompt, prompts, active)
}

/// Overlay Deck's record (description, status, label) on a scanned entry.
fn finish(mut e: Entry, saved: &Saved) -> Entry {
    if let Some(r) = saved.records.get(&e.id) {
        if let Some(l) = &r.label {
            e.label = l.clone();
        }
        e.summary = r.summary.clone();
        e.summary_state = r.summary_state.clone().unwrap_or_else(|| "none".into());
        e.summary_error = r.summary_error.clone();
        if r.status.as_deref() == Some("restored") {
            e.status = "restored".into();
            e.restored_at_ms = r.restored_at_ms;
            e.restored_workspace_id = r.restored_workspace_id.clone();
        }
    }
    e
}

/// Scan, then queue descriptions for the newest sessions that have none.
pub fn list(app: &AppHandle) -> Vec<Entry> {
    let entries = scan(app);
    let cutoff = now_ms().saturating_sub(AUTO_SUMMARY_MAX_AGE.as_millis() as u64);
    let mut queued = 0;
    for e in entries.iter() {
        if queued >= AUTO_SUMMARY_PER_SCAN {
            break;
        }
        if e.summary_state != "none" || e.last_active_ms < cutoff || e.status != "closed" {
            continue;
        }
        let allowed = {
            let store = app.state::<Store>();
            let Ok(mut q) = store.queue.lock() else { break };
            if q.auto_started >= AUTO_SUMMARY_PER_RUN {
                false
            } else {
                q.auto_started += 1;
                true
            }
        };
        if !allowed {
            break;
        }
        if spawn_summary(app.clone(), e.id.clone()) {
            queued += 1;
        }
    }
    entries
}

// ---- repository groups ---------------------------------------------------------

pub fn groups(app: &AppHandle) -> Vec<RepoGroup> {
    app.state::<Store>().snapshot().groups
}

/// Replace the groups; a repo may sit in one group only, empty names are dropped.
pub fn set_groups(app: &AppHandle, groups: Vec<RepoGroup>) -> Result<Vec<RepoGroup>, String> {
    let mut seen: HashSet<String> = HashSet::new();
    let cleaned: Vec<RepoGroup> = groups
        .into_iter()
        .filter(|g| !g.id.trim().is_empty() && !g.name.trim().is_empty())
        .map(|mut g| {
            g.repos.retain(|r| seen.insert(r.to_lowercase()));
            g
        })
        .collect();
    app.state::<Store>()
        .update(|s| s.groups = cleaned.clone())?;
    Ok(cleaned)
}

// ---- close to archive -----------------------------------------------------------

/// Close a live workspace and remember it even if no Herdr snapshot ever caught it.
pub fn archive_and_close(
    app: &AppHandle,
    workspace_id: &str,
    close_group: bool,
) -> Result<Value, String> {
    let rt = runtime::of(app);
    let ws = rt.workspace_get(workspace_id).map_err(|e| e.to_string())?;
    let ws = ws.get("workspace").cloned().unwrap_or(ws);
    let tabs = rt.tab_list(workspace_id).map_err(|e| e.to_string())?;
    let panes = rt.pane_list().map_err(|e| e.to_string())?;
    // Reuse the session.json shape so one parser serves both.
    let mut tab_objs: BTreeMap<String, Value> = BTreeMap::new();
    for tab in tabs
        .get("tabs")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let tid = str_at(tab, &["tab_id"]).unwrap_or("").to_string();
        tab_objs.insert(
            tid,
            json!({ "custom_name": str_at(tab, &["label"]).unwrap_or(""), "panes": {} }),
        );
    }
    for pane in panes
        .get("panes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if str_at(pane, &["workspace_id"]) != Some(workspace_id) {
            continue;
        }
        let tid = str_at(pane, &["tab_id"]).unwrap_or("").to_string();
        let slot = tab_objs
            .entry(tid)
            .or_insert_with(|| json!({ "custom_name": "", "panes": {} }));
        let pid = str_at(pane, &["pane_id"]).unwrap_or("p").to_string();
        slot["panes"][pid] = json!({ "cwd": pane.get("cwd").cloned().unwrap_or(Value::Null), "agent_session": pane.get("agent_session").cloned().unwrap_or(Value::Null) });
    }
    let wt = ws.get("worktree").filter(|v| v.is_object()).map(|w| {
        json!({ "label": w.get("branch").cloned().unwrap_or(Value::Null), "repo_root": w.get("repo_root").cloned().unwrap_or(Value::Null),
                "checkout_path": w.get("checkout_path").cloned().unwrap_or(Value::Null), "is_linked_worktree": w.get("is_linked_worktree").cloned().unwrap_or(Value::Bool(false)) })
    });
    let fake = json!({ "workspaces": [{ "custom_name": str_at(&ws, &["label"]), "identity_cwd": Value::Null, "worktree_space": wt, "tabs": tab_objs.values().collect::<Vec<_>>() }] });
    let record = workspaces_in(&fake, now_ms()).into_iter().next();

    rt.workspace_close(workspace_id, close_group)
        .map_err(|e| e.to_string())?;

    let store = app.state::<Store>();
    let key = record.as_ref().map(|r| r.key.clone());
    store.update(|s| {
        if let Some(r) = record {
            s.closed.retain(|c| c.key != r.key);
            s.closed.insert(0, r);
            s.closed.truncate(200);
        }
    })?;
    if let Some(key) = &key {
        store.record(key, |r| {
            r.archived_at_ms = Some(now_ms());
            r.status = Some("closed".into());
            r.restored_at_ms = None;
            r.restored_workspace_id = None;
        })?;
    }
    let _ = app.emit(EVENT, "changed");
    if let Some(key) = key.clone() {
        spawn_summary(app.clone(), key);
    }
    Ok(json!({ "id": key }))
}

// ---- restore --------------------------------------------------------------------

fn find_entry(app: &AppHandle, id: &str) -> Result<Entry, String> {
    scan(app)
        .into_iter()
        .find(|e| e.id == id)
        .ok_or_else(|| format!("session {id} is not in the archive any more"))
}

pub fn restore(app: &AppHandle, id: &str) -> Result<Value, String> {
    let entry = find_entry(app, id)?;
    if !Path::new(&entry.cwd).is_dir() {
        return Err(format!("directory is gone: {}", entry.cwd));
    }
    // A root session goes back into the repository's ROOT space (a new tab
    // when it is open, `worktree.open` on the main checkout otherwise), a
    // worktree session into its worktree's space — never a detached folder.
    let rt = runtime::of(app);
    let (workspace_id, root_pane) = crate::quickhacks::open_session_dir(
        &*rt,
        &entry.cwd,
        entry.repo_root.as_deref(),
        Some(entry.label.as_str()),
        true,
    )
    .map_err(|e| format!("restore: {e}"))?;
    let mut root_free = Some(root_pane);

    let mut targets: Vec<(String, String)> = Vec::new();
    for s in entry.sessions.iter().filter(|s| s.resumable) {
        let Some(sid) = s.session_id.as_deref() else {
            continue;
        };
        let pane = match root_free.take() {
            Some(p) => {
                // The first session lands in the fresh tab: give it the saved tab name too.
                if !s.tab_label.is_empty() {
                    if let Some(tab_id) = rt.pane_get(&p).ok().and_then(|v| {
                        str_at(&v, &["pane", "tab_id"])
                            .or_else(|| str_at(&v, &["tab_id"]))
                            .map(str::to_string)
                    }) {
                        if let Err(e) = rt.tab_rename(&tab_id, &s.tab_label) {
                            tracing::warn!(%e, "restored tab not renamed");
                        }
                    }
                }
                p
            }
            None => {
                let cwd = if Path::new(&s.cwd).is_dir() {
                    s.cwd.as_str()
                } else {
                    entry.cwd.as_str()
                };
                let tab = rt
                    .tab_create(runtime::TabCreate {
                        workspace_id: workspace_id.clone(),
                        cwd: Some(cwd.to_string()),
                        label: (!s.tab_label.is_empty()).then(|| s.tab_label.clone()),
                        focus: false,
                    })
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
            if let Err(e) = rt
                .pane_send_text(&pane, &cmd)
                .and_then(|_| rt.pane_send_keys(&pane, &["enter"]))
            {
                tracing::warn!(%e, pane, "resume command not delivered");
            }
            std::thread::sleep(Duration::from_millis(300));
        }
    });
    app.state::<Store>().record(id, |r| {
        r.status = Some("restored".into());
        r.restored_at_ms = Some(now_ms());
        r.restored_workspace_id = Some(workspace_id.clone());
    })?;
    let _ = app.emit(EVENT, "changed");
    Ok(json!({ "workspace_id": workspace_id, "resumed": resumed }))
}

// ---- delete ---------------------------------------------------------------------

/// Where a linked worktree may be removed from: `(repo_root, path)`. Never the main checkout.
pub fn removable_worktree(entry: &Entry) -> Result<(String, String), String> {
    let (root, linked) = match (&entry.repo_root, entry.linked_worktree) {
        (Some(root), true) => (root.clone(), true),
        _ => git_worktree_facts(&entry.cwd)
            .ok_or("not a linked worktree: the main checkout is never removed")?,
    };
    if !linked {
        return Err("not a linked worktree: the main checkout is never removed".into());
    }
    if same_path(&root, &entry.cwd) {
        return Err("worktree path equals the repository root; refusing".into());
    }
    Ok((root, entry.cwd.clone()))
}

/// `(repo_root, is_linked)` from git itself, for sessions Herdr never described.
fn git_worktree_facts(cwd: &str) -> Option<(String, bool)> {
    if !Path::new(cwd).is_dir() {
        return None;
    }
    let git_dir = git(cwd, &["rev-parse", "--git-dir"])
        .ok()?
        .trim()
        .to_string();
    let common = git(cwd, &["rev-parse", "--git-common-dir"])
        .ok()?
        .trim()
        .to_string();
    let abs = |p: &str| {
        let pb = Path::new(p);
        if pb.is_absolute() {
            pb.to_path_buf()
        } else {
            Path::new(cwd).join(pb)
        }
    };
    let common_abs = abs(&common);
    let linked = !same_path(
        &abs(&git_dir).to_string_lossy(),
        &common_abs.to_string_lossy(),
    );
    let root = common_abs.parent()?.to_string_lossy().to_string();
    Some((root, linked))
}

/// A child process that never opens a console window (Deck is a GUI app;
/// without this flag Windows pops a terminal for every `git`/`claude -p`).
pub(crate) fn quiet_command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    #[allow(unused_mut)] // mutated only on Windows
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

pub(crate) fn git(cwd: &str, args: &[&str]) -> Result<String, String> {
    let out = quiet_command("git")
        .arg("-C")
        .arg(cwd)
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

/// Forget the session(s): tombstone the ids and delete their transcripts
/// (`<id>.jsonl` and the `<id>/` side directory). With `remove_worktree`,
/// also `git worktree remove --force` a linked checkout.
pub fn delete(app: &AppHandle, id: &str, remove_worktree: bool) -> Result<Value, String> {
    let entry = find_entry(app, id)?;
    let mut removed_path: Option<String> = None;
    if remove_worktree {
        let (root, path) = removable_worktree(&entry)?;
        if Path::new(&path).exists() {
            git(&root, &["worktree", "remove", "--force", &path])?;
        }
        let _ = git(&root, &["worktree", "prune"]);
        removed_path = Some(path);
    }
    let files = transcripts();
    let mut deleted_files = 0;
    for sid in entry
        .sessions
        .iter()
        .filter_map(|s| s.session_id.as_deref())
    {
        if let Some(t) = files.get(sid) {
            if std::fs::remove_file(&t.path).is_ok() {
                deleted_files += 1;
            }
            let side = t.path.with_extension("");
            if side.is_dir() {
                let _ = std::fs::remove_dir_all(side);
            }
        }
    }
    let ids: Vec<String> = entry
        .sessions
        .iter()
        .filter_map(|s| s.session_id.clone())
        .collect();
    app.state::<Store>().update(|s| {
        s.deleted.extend(ids);
        s.records.remove(id);
        s.closed.retain(|c| c.key != id);
    })?;
    let _ = app.emit(EVENT, "changed");
    Ok(json!({ "removed_worktree": removed_path, "deleted_transcripts": deleted_files }))
}

// ---- description (shadow claude -p) ---------------------------------------------

/// First user message plus the tail of the conversation, text parts only.
pub fn transcript_excerpt(jsonl: &str) -> String {
    let mut turns: Vec<(String, String)> = Vec::new();
    for line in jsonl.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if v.get("isSidechain").and_then(Value::as_bool) == Some(true) {
            continue;
        }
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

/// The first `head` bytes and the last `tail` bytes of a file as text, joined
/// with a newline; whole-line boundaries are kept (a cut line is dropped).
pub fn read_head_and_tail(path: &Path, head: u64, tail: u64) -> std::io::Result<String> {
    use std::io::{Seek, SeekFrom};
    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    if len <= head + tail {
        let mut all = String::new();
        file.read_to_string(&mut all)?;
        return Ok(all);
    }
    let mut buf = vec![0u8; head as usize];
    file.read_exact(&mut buf)?;
    let mut text = String::from_utf8_lossy(&buf).to_string();
    if let Some(cut) = text.rfind('\n') {
        text.truncate(cut + 1);
    }
    file.seek(SeekFrom::Start(len - tail))?;
    let mut rest = Vec::with_capacity(tail as usize);
    file.read_to_end(&mut rest)?;
    let rest = String::from_utf8_lossy(&rest).to_string();
    let rest = rest.find('\n').map(|i| &rest[i + 1..]).unwrap_or("");
    text.push_str(rest);
    Ok(text)
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

/// The shadow query runs here, so its own transcript is recognisable (and skipped) by cwd.
pub(crate) fn shadow_workdir(app: &AppHandle) -> Option<PathBuf> {
    crate::data_dir(app).map(|d| d.join("shadow"))
}

/// `claude -p` over the excerpt, with a hard deadline, outside any project.
fn summarize_text(workdir: &Path, excerpt: &str) -> Result<String, String> {
    shadow_prompt(workdir, SUMMARY_PROMPT, excerpt, 400)
}

/// One shadow `claude -p PROMPT` (haiku, text output) over `input` on stdin,
/// run in `workdir` (outside any project) with a hard deadline; the reply is
/// cut at `max_chars`. Shared by the archive descriptions and the harness tags.
pub(crate) fn shadow_prompt(
    workdir: &Path,
    prompt: &str,
    input: &str,
    max_chars: usize,
) -> Result<String, String> {
    let claude = resolve_claude().ok_or("claude CLI not found (PATH, ~/.local/bin)")?;
    std::fs::create_dir_all(workdir).map_err(|e| e.to_string())?;
    let mut child = quiet_command(&claude)
        .args([
            "-p",
            prompt,
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
        let _ = stdin.write_all(input.as_bytes());
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
    Ok(take_chars(&text, max_chars))
}

/// Queue the description for `id`; one worker thread writes them in order.
/// Returns false when it is already queued or being written.
pub fn spawn_summary(app: AppHandle, id: String) -> bool {
    let store = app.state::<Store>();
    let start_worker = {
        let Ok(mut q) = store.queue.lock() else {
            return false;
        };
        if !q.queued.insert(id.clone()) {
            return false;
        }
        q.waiting.push_back(id.clone());
        let start = !q.worker_running;
        q.worker_running = true;
        start
    };
    let _ = store.record(&id, |r| {
        r.summary_state = Some("pending".into());
        r.summary_error = None;
    });
    if start_worker {
        std::thread::spawn(move || summary_worker(app));
    }
    true
}

fn summary_worker(app: AppHandle) {
    loop {
        let next = {
            let store = app.state::<Store>();
            let Ok(mut q) = store.queue.lock() else {
                return;
            };
            match q.waiting.pop_front() {
                Some(id) => id,
                None => {
                    q.worker_running = false;
                    return;
                }
            }
        };
        let result = write_description(&app, &next);
        let store = app.state::<Store>();
        let _ = store.record(&next, |r| match &result {
            Ok(text) => {
                r.summary = Some(text.clone());
                r.summary_state = Some("ready".into());
                r.summary_error = None;
            }
            Err(err) => {
                r.summary_state = Some("failed".into());
                r.summary_error = Some(err.clone());
            }
        });
        if let Ok(mut q) = store.queue.lock() {
            q.queued.remove(&next);
        }
        if let Err(err) = &result {
            tracing::warn!(%err, id = next, "archive description failed");
        }
        let _ = app.emit(EVENT, "changed");
    }
}

fn write_description(app: &AppHandle, id: &str) -> Result<String, String> {
    let files = transcripts();
    let mut excerpt = String::new();
    for sid in id.split('+') {
        let Some(t) = files.get(sid) else { continue };
        let text = read_head_and_tail(&t.path, EXCERPT_HEAD_BYTES, EXCERPT_TAIL_BYTES)
            .map_err(|e| format!("{}: {e}", t.path.display()))?;
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
        return Err("transcript not found under ~/.claude/projects".into());
    }
    let workdir = shadow_workdir(app).ok_or("no app data dir")?;
    summarize_text(&workdir, &excerpt)
}

pub fn resummarize(app: &AppHandle, id: &str) -> Result<(), String> {
    if spawn_summary(app.clone(), id.to_string()) {
        let _ = app.emit(EVENT, "changed");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(linked: bool, root: Option<&str>, cwd: &str) -> Entry {
        Entry {
            id: "x".into(),
            source: "herdr".into(),
            label: "L".into(),
            repo_name: None,
            branch: None,
            cwd: cwd.into(),
            repo_root: root.map(str::to_string),
            linked_worktree: linked,
            sessions: vec![],
            last_active_ms: 0,
            prompt: None,
            prompts: 0,
            summary: None,
            summary_state: "none".into(),
            summary_error: None,
            status: "closed".into(),
            restored_at_ms: None,
            restored_workspace_id: None,
        }
    }

    #[test]
    fn groups_keep_a_repo_in_one_group_only() {
        let dir = std::env::temp_dir().join(format!("deck-groups-{}", now_ms()));
        let store = Store::open(&dir);
        let g = vec![
            RepoGroup {
                id: "a".into(),
                name: "Personal".into(),
                repos: vec!["X".into(), "y".into()],
                hidden: true,
            },
            RepoGroup {
                id: "b".into(),
                name: "Work".into(),
                repos: vec!["x".into(), "z".into()],
                hidden: false,
            },
            RepoGroup {
                id: "".into(),
                name: "bad".into(),
                repos: vec![],
                hidden: false,
            },
        ];
        let mut seen: HashSet<String> = HashSet::new();
        let cleaned: Vec<RepoGroup> = g
            .into_iter()
            .filter(|g| !g.id.trim().is_empty() && !g.name.trim().is_empty())
            .map(|mut g| {
                g.repos.retain(|r| seen.insert(r.to_lowercase()));
                g
            })
            .collect();
        assert_eq!(cleaned.len(), 2);
        assert_eq!(cleaned[1].repos, vec!["z".to_string()]);
        store.update(|s| s.groups = cleaned.clone()).unwrap();
        assert_eq!(Store::open(&dir).snapshot().groups, cleaned);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn plain_path_strips_verbatim_prefix_and_trailing_separator() {
        assert_eq!(plain_path(r"\\?\C:\repo\.git"), r"C:\repo\.git");
        assert_eq!(plain_path(r"C:\Workspace\Elly2\"), r"C:\Workspace\Elly2");
        assert_eq!(plain_path("/home/x/"), "/home/x");
    }

    #[test]
    fn key_of_sorts_and_dedupes() {
        assert_eq!(key_of(&["b".into(), "a".into(), "b".into()]), "a+b");
    }

    #[test]
    fn workspaces_in_reads_herdr_session_shape() {
        let v = json!({ "workspaces": [
            { "id": "w8", "custom_name": "b1-vu4", "identity_cwd": "C:\\Users\\me\\.herdr\\worktrees\\IR\\feature-x\\",
              "worktree_space": { "label": "feature-x", "repo_root": "\\\\?\\C:\\Workspace\\IR", "checkout_path": "C:/Users/me/.herdr/worktrees/IR/feature-x", "is_linked_worktree": true },
              "tabs": [ { "custom_name": "main", "panes": { "1": { "cwd": "C:\\x\\", "agent_session": { "source": "herdr:claude", "agent": "claude", "kind": "id", "value": "aaaa" } } } },
                        { "custom_name": "shell", "panes": { "2": { "cwd": "C:\\x\\" } } } ] },
            { "id": "w9", "custom_name": null, "identity_cwd": "C:\\only-shells\\", "tabs": [ { "custom_name": "t", "panes": { "3": { "cwd": "C:\\only-shells\\" } } } ] }
        ]});
        let ws = workspaces_in(&v, 5);
        assert_eq!(ws.len(), 1, "shell-only workspaces are skipped");
        let w = &ws[0];
        assert_eq!(w.key, "aaaa");
        assert_eq!(w.label, "b1-vu4");
        assert_eq!(w.branch.as_deref(), Some("feature-x"));
        assert_eq!(w.repo_name.as_deref(), Some("IR"));
        assert_eq!(w.repo_root.as_deref(), Some(r"C:\Workspace\IR"));
        assert_eq!(w.cwd, "C:/Users/me/.herdr/worktrees/IR/feature-x");
        assert!(w.linked_worktree);
        assert_eq!(w.sessions.len(), 2);
        assert_eq!(w.sessions[0].tab_label, "main");
        assert_eq!(w.sessions[0].session_id.as_deref(), Some("aaaa"));
        assert_eq!(w.seen_ms, 5);
    }

    #[test]
    fn workspace_without_custom_name_uses_worktree_label_then_folder() {
        let v = json!({ "workspaces": [
            { "custom_name": null, "identity_cwd": "C:\\repo\\Elly2\\", "worktree_space": { "label": "Elly2", "repo_root": "C:\\repo\\Elly2", "checkout_path": "C:/repo/Elly2", "is_linked_worktree": false },
              "tabs": [ { "custom_name": "", "panes": { "1": { "cwd": "C:\\repo\\Elly2\\", "agent_session": { "kind": "id", "value": "s1", "agent": "claude" } } } } ] },
            { "custom_name": null, "identity_cwd": "C:\\plain\\dir\\", "tabs": [ { "custom_name": "", "panes": { "1": { "cwd": "C:\\plain\\dir\\", "agent_session": { "kind": "id", "value": "s2", "agent": "claude" } } } } ] }
        ]});
        let ws = workspaces_in(&v, 0);
        assert_eq!(ws[0].label, "Elly2");
        assert_eq!(
            ws[0].branch, None,
            "primary checkout: the label is not a branch"
        );
        assert_eq!(ws[1].label, "dir");
        assert_eq!(ws[1].cwd, r"C:\plain\dir");
    }

    #[test]
    fn snapshot_file_names_carry_their_time() {
        assert_eq!(
            snapshot_time_ms("session-000000000000000000001794418690757385500-21508-0.json"),
            Some(1_794_418_690_757)
        );
        assert_eq!(snapshot_time_ms("other.json"), None);
    }

    #[test]
    fn session_id_shape() {
        assert!(looks_like_session_id(
            "7f0cff8f-729c-4907-b694-6f5ace18360e"
        ));
        assert!(!looks_like_session_id("agent-7f0cff8f"));
        assert!(!looks_like_session_id("sessions-index"));
    }

    #[test]
    fn head_and_tail_keep_whole_lines() {
        let dir = std::env::temp_dir().join(format!("deck-ht-{}", now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("t.jsonl");
        let lines: Vec<String> = (0..200)
            .map(|i| format!("{{\"n\":{i},\"pad\":\"{}\"}}", "x".repeat(50)))
            .collect();
        std::fs::write(&p, lines.join("\n")).unwrap();
        let text = read_head_and_tail(&p, 300, 400).unwrap();
        let got: Vec<&str> = text.lines().collect();
        assert!(
            got.iter().all(|l| serde_json::from_str::<Value>(l).is_ok()),
            "no cut lines: {got:?}"
        );
        assert_eq!(got.first().copied(), Some(lines[0].as_str()));
        assert_eq!(got.last().copied(), Some(lines[199].as_str()));
        assert!(got.len() < 20);
        let small = read_head_and_tail(&p, 1 << 20, 1 << 20).unwrap();
        assert_eq!(small.lines().count(), 200);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn environmental_failures_are_retried() {
        assert!(environmental_failure(
            "spawn x: The paging file is too small for this operation to complete. (os error 1455)"
        ));
        assert!(environmental_failure("claude exited exit code: 0xc0000409"));
        assert!(environmental_failure("C:\\x.jsonl: out of memory"));
        assert!(!environmental_failure(
            "transcript not found under ~/.claude/projects"
        ));
    }

    #[test]
    fn transcript_excerpt_keeps_first_user_turn_and_tail_text_only() {
        let lines = [
            r#"{"type":"user","message":{"role":"user","content":"сделай X"}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"делаю"},{"type":"tool_use","name":"Bash"}]}}"#,
            r#"{"type":"user","isSidechain":true,"message":{"content":"subagent prompt"}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"ok"}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"готово"}]}}"#,
        ];
        let out = transcript_excerpt(&lines.join("\n"));
        assert!(out.starts_with("[USER] сделай X\n"), "{out}");
        assert!(out.contains("[CLAUDE] делаю\n"));
        assert!(out.ends_with("[CLAUDE] готово\n"));
        assert!(!out.contains("tool_result") && !out.contains("subagent"));
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
    fn removable_worktree_guards() {
        assert!(removable_worktree(&entry(true, None, "/definitely/not/a/dir")).is_err());
        assert!(removable_worktree(&entry(true, Some("/r"), "/r")).is_err());
        assert!(removable_worktree(&entry(true, Some(r"\\?\C:\r"), "C:/r/")).is_err());
        assert_eq!(
            removable_worktree(&entry(true, Some("/r"), "/r/wt")).unwrap(),
            ("/r".into(), "/r/wt".into())
        );
    }

    #[test]
    fn finish_overlays_deck_record() {
        let mut saved = Saved::default();
        saved.records.insert(
            "x".into(),
            Record {
                summary: Some("S".into()),
                summary_state: Some("ready".into()),
                status: Some("restored".into()),
                restored_workspace_id: Some("w5".into()),
                ..Default::default()
            },
        );
        let e = finish(entry(false, None, "/a"), &saved);
        assert_eq!(e.summary.as_deref(), Some("S"));
        assert_eq!(e.status, "restored");
        assert_eq!(e.restored_workspace_id.as_deref(), Some("w5"));
        let plain = finish(entry(false, None, "/a"), &Saved::default());
        assert_eq!(plain.summary_state, "none");
    }

    #[test]
    fn store_roundtrip_with_tombstones() {
        let dir = std::env::temp_dir().join(format!("deck-archive-test-{}", now_ms()));
        let store = Store::open(&dir);
        store
            .update(|s| {
                s.deleted.insert("gone".into());
            })
            .unwrap();
        store.record("k", |r| r.summary = Some("S".into())).unwrap();
        let again = Store::open(&dir).snapshot();
        assert!(again.deleted.contains("gone"));
        assert_eq!(again.records["k"].summary.as_deref(), Some("S"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
