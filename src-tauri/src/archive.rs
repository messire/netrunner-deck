//! Session archive ("cold storage") on Deck's side. Each server's machine
//! scans its own disk (`deck_accounts::archive`, asked as `deck.archive.*`
//! through that machine's bridge): every Claude session there that is not
//! open on the server right now. Deck adds what only it knows:
//!
//! * `<app_data>/archive.json`: descriptions written by the shadow
//!   `claude -p` (here, from an excerpt the machine sends), restore status,
//!   tombstones for deleted sessions, and the workspaces Deck closed itself
//!   (each tagged with its server);
//! * which sessions are open right now (the server's `pane.list`).
//!
//! Entries are keyed by their Claude session ids (sorted, joined with `+`)
//! and carry the server they belong to.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};

use crate::runtime::{self, Runtime, SharedRuntime};

pub use deck_accounts::archive::{
    looks_like_session_id, now_ms, str_at, take_chars, workspaces_in, Entry, ScanInput,
    WorkspaceRecord,
};

pub const EVENT: &str = "deck://archive";
const SUMMARY_TIMEOUT: Duration = Duration::from_secs(150);
const SUMMARY_MODEL: &str = "haiku";
const SUMMARY_PROMPT: &str = "Ниже — фрагменты транскрипта сессии Claude Code (сначала первое сообщение пользователя, затем последние реплики). \
Опиши по-русски в 1–2 предложениях (до 220 символов), что делалось в этой сессии и чем она закончилась. \
Без вступлений, без markdown, без кавычек — только сам текст описания.";
/// Shell start-up (profile, Herdr banner) before the resume command is typed.
const RESUME_SETTLE: Duration = Duration::from_millis(1800);
/// Descriptions are written automatically only for recent sessions, a few per scan.
const AUTO_SUMMARY_MAX_AGE: Duration = Duration::from_secs(14 * 24 * 3600);
const AUTO_SUMMARY_PER_SCAN: usize = 4;
/// … and at most this many per app run; WRITE in the table is unbounded.
const AUTO_SUMMARY_PER_RUN: usize = 30;

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
    waiting: std::collections::VecDeque<(String, String)>,
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

// ---- the scan (on the server's machine) ----------------------------------------

/// A server id, `None` meaning the primary one.
fn server_id(app: &AppHandle, server: Option<&str>) -> Result<String, String> {
    server
        .map(str::to_string)
        .or_else(|| app.state::<crate::servers::Registry>().primary_id())
        .ok_or_else(|| "no server is connected".to_string())
}

/// Ids open on the server right now; `None` when it does not answer.
fn live_ids(rt: &dyn Runtime) -> Option<HashSet<String>> {
    rt.pane_list().ok().map(|v| {
        v.get("panes")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|p| str_at(p, &["agent_session", "value"]).map(str::to_string))
            .collect()
    })
}

/// Everything restorable for `server` (Deck's records not yet overlaid).
fn scan(app: &AppHandle, server: &str) -> Result<(Vec<Entry>, SharedRuntime), String> {
    let reg = app.state::<crate::servers::Registry>();
    let rt = reg.runtime(Some(server))?;
    let local = !reg.is_remote(server);
    let saved = app.state::<Store>().snapshot();
    let input = ScanInput {
        config_dir: reg.session_dir(server),
        // Closed before closes carried their server: only this computer's servers had any.
        closed: saved
            .closed
            .iter()
            .filter(|c| c.server.as_deref() == Some(server) || (c.server.is_none() && local))
            .cloned()
            .collect(),
        deleted: saved.deleted.clone(),
        live: live_ids(&*rt),
        skip_cwd: local
            .then(|| shadow_workdir(app).map(|p| p.to_string_lossy().to_string()))
            .flatten(),
    };
    let reply = rt
        .machine_call("deck.archive.scan", json!(input))
        .map_err(|e| e.to_string())?;
    let entries: Vec<Entry> =
        serde_json::from_value(reply.get("entries").cloned().unwrap_or_default())
            .map_err(|e| format!("archive of {server}: {e}"))?;
    let entries = entries
        .into_iter()
        .map(|mut e| {
            e.server = Some(server.to_string());
            finish(e, &saved)
        })
        .collect();
    Ok((entries, rt))
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
pub fn list(app: &AppHandle, server: Option<&str>) -> Result<Vec<Entry>, String> {
    let server = server_id(app, server)?;
    let (entries, _) = scan(app, &server)?;
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
        if spawn_summary(app.clone(), e.id.clone(), server.clone()) {
            queued += 1;
        }
    }
    Ok(entries)
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
    server: Option<&str>,
    workspace_id: &str,
    close_group: bool,
) -> Result<Value, String> {
    let server = server_id(app, server)?;
    let rt = crate::servers::runtime_for(app, Some(&server))?;
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
    let record = workspaces_in(&fake, now_ms())
        .into_iter()
        .next()
        .map(|mut r| {
            r.server = Some(server.clone());
            r
        });

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
        spawn_summary(app.clone(), key, server);
    }
    Ok(json!({ "id": key }))
}

// ---- restore --------------------------------------------------------------------

fn find_entry(app: &AppHandle, server: &str, id: &str) -> Result<(Entry, SharedRuntime), String> {
    let (entries, rt) = scan(app, server)?;
    let entry = entries
        .into_iter()
        .find(|e| e.id == id)
        .ok_or_else(|| format!("session {id} is not in the archive any more"))?;
    Ok((entry, rt))
}

/// Reopen `id` of `server`'s archive on that server (`None` = the primary server).
pub fn restore(app: &AppHandle, server: Option<&str>, id: &str) -> Result<Value, String> {
    let server = server_id(app, server)?;
    let (entry, rt) = find_entry(app, &server, id)?;
    let is_dir = |p: &str| {
        rt.machine_call("deck.archive.dir", json!({ "path": p }))
            .ok()
            .and_then(|v| v.get("exists").and_then(Value::as_bool))
            .unwrap_or(false)
    };
    if !is_dir(&entry.cwd) {
        return Err(format!("directory is gone: {}", entry.cwd));
    }
    // A root session goes back into the repository's ROOT space (a new tab
    // when it is open, `worktree.open` on the main checkout otherwise), a
    // worktree session into its worktree's space — never a detached folder.
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
                let cwd = if is_dir(&s.cwd) {
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
    // deck-server: every pane already runs Claude headless (only ever a
    // chat, docs/adr/0003-headless-claude.md); it is told which
    // conversation to open. A stock Herdr pane is a shell: type the resume.
    let headless = rt.agent_call("list", json!({})).is_ok();
    std::thread::spawn(move || {
        if !headless {
            std::thread::sleep(RESUME_SETTLE);
        }
        for (pane, sid) in targets {
            let delivered = if headless {
                crate::quickhacks::agent_call_when_ready(
                    &*rt,
                    &pane,
                    "resume",
                    json!({ "pane_id": pane, "session_id": sid }),
                    Duration::from_secs(30),
                )
            } else {
                rt.pane_send_text(&pane, &format!("claude --resume {sid}"))
                    .and_then(|_| rt.pane_send_keys(&pane, &["enter"]))
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            };
            if let Err(e) = delivered {
                tracing::warn!(%e, pane, "resume not delivered");
            }
            if !headless {
                std::thread::sleep(Duration::from_millis(300));
            }
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

/// Forget the session(s): tombstone the ids; the server's machine deletes
/// their transcripts and, with `remove_worktree`, the linked checkout.
pub fn delete(
    app: &AppHandle,
    server: Option<&str>,
    id: &str,
    remove_worktree: bool,
) -> Result<Value, String> {
    let server = server_id(app, server)?;
    let (entry, rt) = find_entry(app, &server, id)?;
    let result = rt
        .machine_call(
            "deck.archive.delete",
            json!({ "entry": entry, "remove_worktree": remove_worktree }),
        )
        .map_err(|e| e.to_string())?;
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
    Ok(result)
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

// ---- description (shadow claude -p) ---------------------------------------------

fn resolve_claude() -> Option<PathBuf> {
    #[cfg(windows)]
    let names = ["claude.exe", "claude.cmd", "claude"];
    #[cfg(not(windows))]
    let names = ["claude"];
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    if let Some(home) = deck_accounts::home_dir() {
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
pub fn spawn_summary(app: AppHandle, id: String, server: String) -> bool {
    let store = app.state::<Store>();
    let start_worker = {
        let Ok(mut q) = store.queue.lock() else {
            return false;
        };
        if !q.queued.insert(id.clone()) {
            return false;
        }
        q.waiting.push_back((id.clone(), server));
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
                Some(next) => next,
                None => {
                    q.worker_running = false;
                    return;
                }
            }
        };
        let (next, server) = next;
        let result = write_description(&app, &server, &next);
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

fn write_description(app: &AppHandle, server: &str, id: &str) -> Result<String, String> {
    let rt = crate::servers::runtime_for(app, Some(server))?;
    let reply = rt
        .machine_call("deck.archive.excerpt", json!({ "id": id }))
        .map_err(|e| e.to_string())?;
    let excerpt = reply
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let workdir = shadow_workdir(app).ok_or("no app data dir")?;
    summarize_text(&workdir, excerpt)
}

pub fn resummarize(app: &AppHandle, server: Option<&str>, id: &str) -> Result<(), String> {
    let server = server_id(app, server)?;
    if spawn_summary(app.clone(), id.to_string(), server) {
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
            server: None,
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
