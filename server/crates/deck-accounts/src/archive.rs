//! Session archive of this machine ("cold storage"): every Claude session
//! here that is not open in its server right now, with what is needed to
//! bring it back. Answered on the machine (`deck.archive.*`), so ARCHIVE
//! shows the sessions of the server it is about; Deck keeps its own
//! records (descriptions, restore status, tombstones) and overlays them.
//!
//! Nothing has to be "archived" first — the list is a scan of what already
//! exists on disk, so sessions closed long before Deck existed are there too:
//!
//! * the server's `session.json` (live state) and `session-snapshots/*.json`
//!   (its periodic copies) give workspaces: label, worktree provenance, tabs,
//!   and the Claude session id recorded for every pane.
//! * `~/.claude/projects/*/<id>.jsonl` are the transcripts: a session is
//!   resumable while its transcript exists. `~/.claude/history.jsonl` gives
//!   the prompts and the cwd per session cheaply.
//!
//! Entries are keyed by their Claude session ids (sorted, joined with `+`);
//! a workspace with several Claude panes is one entry.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::uplink::git;

const TRANSCRIPT_HEAD_CHARS: usize = 1500;
const TRANSCRIPT_TAIL_CHARS: usize = 9000;
/// Transcript head read for cwd / first prompt when history.jsonl lacks the session.
const HEAD_BYTES: u64 = 256 * 1024;
/// A transcript can be hundreds of MB; the description needs its start and its end only.
const EXCERPT_HEAD_BYTES: u64 = 512 * 1024;
const EXCERPT_TAIL_BYTES: u64 = 2 * 1024 * 1024;

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
    /// The server whose sessions these are (Deck sets it).
    #[serde(default)]
    pub server: Option<String>,
}

pub fn now_ms() -> u64 {
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

pub fn same_path(a: &str, b: &str) -> bool {
    let n = |s: &str| plain_path(s).replace('\\', "/").to_lowercase();
    n(a) == n(b)
}

pub fn str_at<'a>(v: &'a Value, keys: &[&str]) -> Option<&'a str> {
    let mut cur = v;
    for k in keys {
        cur = cur.get(k)?;
    }
    cur.as_str()
}

fn home_dir() -> Option<PathBuf> {
    crate::home_dir()
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
    /// The server it was open on (Deck sets it when it closes one itself).
    #[serde(default)]
    pub server: Option<String>,
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
            server: None,
        });
    }
    out
}

pub fn basename(p: &str) -> String {
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
pub fn herdr_workspaces(
    dir: Option<PathBuf>,
) -> (HashMap<String, WorkspaceRecord>, HashSet<String>) {
    let mut by_key: HashMap<String, WorkspaceRecord> = HashMap::new();
    let mut live = HashSet::new();
    let Some(dir) = dir.or_else(herdr_dir) else {
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

// ---- Claude transcripts & history --------------------------------------------

#[derive(Debug, Default, Clone)]
pub struct Transcript {
    pub path: PathBuf,
    pub mtime_ms: u64,
}

fn projects_dir() -> Option<PathBuf> {
    home_dir().map(|h| h.join(".claude").join("projects"))
}

/// Top-level `<projects>/<slug>/<id>.jsonl` only; subagent and tool-result files live in subdirectories.
pub fn transcripts() -> HashMap<String, Transcript> {
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

/// What the scan needs besides this machine's disk.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ScanInput {
    /// The server's session dir (its `session.json`); `None`: Herdr's own.
    #[serde(default)]
    pub config_dir: Option<String>,
    /// Workspaces Deck closed itself on this server.
    #[serde(default)]
    pub closed: Vec<WorkspaceRecord>,
    /// Session ids the user deleted.
    #[serde(default)]
    pub deleted: BTreeSet<String>,
    /// Ids open on the server right now; `None`: as `session.json` says.
    #[serde(default)]
    pub live: Option<HashSet<String>>,
    /// Transcripts run in this folder are not sessions (Deck's shadow queries).
    #[serde(default)]
    pub skip_cwd: Option<String>,
}

/// A path as typed on this machine: `~` and `~/x` are under the home.
pub fn expand_home(p: &str) -> PathBuf {
    let rest = p.strip_prefix('~');
    match (rest, home_dir()) {
        (Some(rest), Some(home)) if rest.is_empty() || rest.starts_with(['/', '\\']) => {
            home.join(rest.trim_start_matches(['/', '\\']))
        }
        _ => PathBuf::from(p),
    }
}

fn blank(id: String, source: &str, label: String, cwd: String) -> Entry {
    Entry {
        id,
        source: source.into(),
        label,
        repo_name: None,
        branch: None,
        cwd,
        repo_root: None,
        linked_worktree: false,
        sessions: Vec::new(),
        last_active_ms: 0,
        prompt: None,
        prompts: 0,
        summary: None,
        summary_state: "none".into(),
        summary_error: None,
        status: "closed".into(),
        restored_at_ms: None,
        restored_workspace_id: None,
        server: None,
    }
}

/// Everything restorable on this machine that is not open on the server.
pub fn scan(input: &ScanInput) -> Vec<Entry> {
    let dir = input.config_dir.as_deref().map(expand_home);
    let (mut groups, live_fallback) = herdr_workspaces(dir);
    for w in &input.closed {
        let newer = groups
            .get(&w.key)
            .map(|g| g.seen_ms < w.seen_ms)
            .unwrap_or(true);
        if newer {
            groups.insert(w.key.clone(), w.clone());
        }
    }
    let live = input.live.clone().unwrap_or(live_fallback);
    let deleted = &input.deleted;
    let files = transcripts();
    let history = prompt_history();

    let mut entries: Vec<Entry> = Vec::new();
    let mut claimed: HashSet<String> = HashSet::new();

    for (key, w) in groups {
        let ids: Vec<&str> = key.split('+').collect();
        if ids
            .iter()
            .any(|id| live.contains(*id) || deleted.contains(*id))
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
        entries.push(Entry {
            repo_name: w.repo_name,
            branch: w.branch,
            repo_root: w.repo_root,
            linked_worktree: w.linked_worktree,
            sessions,
            last_active_ms: active.max(w.seen_ms.min(now_ms())),
            prompt,
            prompts,
            ..blank(key, "herdr", w.label, w.cwd)
        });
    }

    for (id, t) in &files {
        if claimed.contains(id) || live.contains(id) || deleted.contains(id) {
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
        if input
            .skip_cwd
            .as_deref()
            .map(|d| same_path(d, &info.cwd))
            .unwrap_or(false)
        {
            continue; // Deck's own shadow queries
        }
        let active = t.mtime_ms.max(info.last_ms);
        let cwd = info.cwd.clone();
        entries.push(Entry {
            sessions: vec![ArchivedSession {
                tab_label: String::new(),
                agent: Some("claude".into()),
                session_id: Some(id.clone()),
                cwd: info.cwd.clone(),
                resumable: true,
            }],
            last_active_ms: active,
            prompt: (!info.first.is_empty()).then_some(info.first),
            prompts: info.count,
            ..blank(id.clone(), "claude", basename(&cwd), cwd)
        });
    }
    entries.sort_by_key(|e| std::cmp::Reverse(e.last_active_ms));
    entries
}

/// First prompt, prompt count and last activity of an entry's sessions.
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

/// Forget sessions on this machine: delete their transcripts (`<id>.jsonl`
/// and the `<id>/` side directory); with `remove_worktree`, also `git
/// worktree remove --force` the entry's linked checkout first.
pub fn delete(entry: &Entry, remove_worktree: bool) -> Result<Value, String> {
    let mut removed_path: Option<String> = None;
    if remove_worktree {
        let (root, path) = removable_worktree(entry)?;
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
    Ok(json!({ "removed_worktree": removed_path, "deleted_transcripts": deleted_files }))
}

// ---- description material ---------------------------------------------------------

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

pub fn take_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max).collect();
        t.push('…');
        t
    }
}

/// The excerpt a description is written from: each session of `id`
/// (`a+b`), first user turn and tail.
pub fn excerpt(id: &str) -> Result<String, String> {
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
    Ok(excerpt)
}

// ---- deck.archive.* --------------------------------------------------------------

/// `deck.archive.scan {ScanInput}` → `{entries}`; `deck.archive.delete
/// {entry, remove_worktree}` → `{removed_worktree, deleted_transcripts}`;
/// `deck.archive.excerpt {id}` → `{text}`; `deck.archive.dir {path}` →
/// `{exists}` (a restore checks the folder is still there).
pub fn call(method: &str, params: &Value) -> crate::Result<Value> {
    let text = |k: &str| {
        params
            .get(k)
            .and_then(Value::as_str)
            .ok_or_else(|| format!("{k} is required"))
    };
    match method {
        "scan" => {
            let input: ScanInput =
                serde_json::from_value(params.clone()).map_err(|e| format!("scan: {e}"))?;
            Ok(json!({ "entries": scan(&input) }))
        }
        "delete" => {
            let entry: Entry =
                serde_json::from_value(params.get("entry").cloned().ok_or("entry is required")?)
                    .map_err(|e| format!("entry: {e}"))?;
            let remove = params
                .get("remove_worktree")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            delete(&entry, remove)
        }
        "excerpt" => Ok(json!({ "text": excerpt(text("id")?)? })),
        "dir" => Ok(json!({ "exists": expand_home(text("path")?).is_dir() })),
        other => Err(format!("unknown method deck.archive.{other}")),
    }
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
            server: None,
        }
    }

    #[test]
    fn plain_path_strips_verbatim_prefix_and_trailing_separator() {
        assert_eq!(plain_path(r"\\?\C:\repo\.git"), r"C:\repo\.git");
        assert_eq!(plain_path(r"C:\Workspace\Atlas\"), r"C:\Workspace\Atlas");
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
            { "custom_name": null, "identity_cwd": "C:\\repo\\Atlas\\", "worktree_space": { "label": "Atlas", "repo_root": "C:\\repo\\Atlas", "checkout_path": "C:/repo/Atlas", "is_linked_worktree": false },
              "tabs": [ { "custom_name": "", "panes": { "1": { "cwd": "C:\\repo\\Atlas\\", "agent_session": { "kind": "id", "value": "s1", "agent": "claude" } } } } ] },
            { "custom_name": null, "identity_cwd": "C:\\plain\\dir\\", "tabs": [ { "custom_name": "", "panes": { "1": { "cwd": "C:\\plain\\dir\\", "agent_session": { "kind": "id", "value": "s2", "agent": "claude" } } } } ] }
        ]});
        let ws = workspaces_in(&v, 0);
        assert_eq!(ws[0].label, "Atlas");
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
}
