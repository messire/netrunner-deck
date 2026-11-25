//! QUICKHACKS: harness entries (skills, commands) pinned to a side panel and
//! launched into a fresh agent session in a chosen repository worktree.
//! Deck keeps the pinned list in its own file; launching is orchestration
//! over Herdr (`worktree.list`, `worktree.open`/`workspace.create`/`tab.create`,
//! `pane.read`, `pane.send_text`) — the agent itself runs in Herdr's pane.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};

use crate::archive::str_at;
use crate::runtime::{self, Runtime};

pub const EVENT: &str = "deck://quickhack";

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Hack {
    pub id: String,
    /// `/name` the agent runs
    pub name: String,
    /// skill | command
    pub kind: String,
    /// personal | project
    pub source: String,
    #[serde(default)]
    pub repo_key: Option<String>,
    #[serde(default)]
    pub repo_name: Option<String>,
    pub path: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub argument_hint: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Saved {
    #[serde(default)]
    pub hacks: Vec<Hack>,
    #[serde(default)]
    pub collapsed: bool,
}

pub struct Store {
    path: PathBuf,
    saved: Mutex<Saved>,
}

impl Store {
    pub fn open(dir: &Path) -> Self {
        let _ = std::fs::create_dir_all(dir);
        let path = dir.join("quickhacks.json");
        let saved = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<Saved>(&b).ok())
            .unwrap_or_default();
        Self {
            path,
            saved: Mutex::new(saved),
        }
    }
    pub fn get(&self) -> Saved {
        self.saved.lock().map(|s| s.clone()).unwrap_or_default()
    }
    pub fn set(&self, next: Saved) -> Result<Saved, String> {
        let mut g = self
            .saved
            .lock()
            .map_err(|_| "quickhacks poisoned".to_string())?;
        *g = next;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(
            &tmp,
            serde_json::to_vec_pretty(&*g).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &self.path).map_err(|e| e.to_string())?;
        Ok(g.clone())
    }
}

/// Worktrees of the repository that contains `cwd` (main checkout first).
pub fn worktrees(rt: &dyn Runtime, cwd: &str) -> Result<Value, String> {
    rt.worktree_list(cwd).map_err(|e| e.to_string())
}

#[derive(Debug, Clone, Deserialize)]
pub struct Launch {
    /// The worktree (or main checkout) directory the session runs in.
    pub cwd: String,
    /// The repository's main checkout; differs from `cwd` for a linked worktree.
    pub repo_root: String,
    #[serde(default)]
    pub label: Option<String>,
    /// The agent command, e.g. `claude`.
    pub agent: String,
    /// The first prompt, e.g. `/ir-feedback 7d`.
    pub prompt: String,
}

fn emit(app: &AppHandle, run: &str, stage: &str, detail: Value) {
    let _ = app.emit(
        EVENT,
        json!({ "run": run, "stage": stage, "detail": detail }),
    );
}

fn last_line(text: &str) -> String {
    text.lines()
        .map(str::trim_end)
        .rfind(|l| !l.trim().is_empty())
        .unwrap_or("")
        .to_string()
}

/// Poll the pane until `ready(screen)` holds or `limit` passes; true when it held.
fn wait_for(
    rt: &dyn Runtime,
    pane: &str,
    limit: Duration,
    source: &str,
    ready: impl Fn(&str) -> bool,
) -> bool {
    let t0 = Instant::now();
    while t0.elapsed() < limit {
        std::thread::sleep(Duration::from_millis(350));
        if let Ok(v) = rt.pane_read(runtime::PaneRead {
            pane_id: pane.to_string(),
            source: source.to_string(),
            lines: 40,
            format: "text".into(),
            strip_ansi: None,
        }) {
            if let Some(text) = str_at(&v, &["read", "text"]) {
                if ready(text) {
                    return true;
                }
            }
        }
    }
    false
}

fn type_line(rt: &dyn Runtime, pane: &str, text: &str) -> Result<(), String> {
    rt.pane_send_text(pane, text)
        .and_then(|_| rt.pane_send_keys(pane, &["enter"]))
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// A shell prompt: the last non-empty line ends in a prompt character.
fn shell_ready(screen: &str) -> bool {
    let l = last_line(screen);
    l.ends_with('>') || l.ends_with('$') || l.ends_with('%') || l.ends_with('#') || l.ends_with('❯')
}

/// Claude Code's input box: a line that is just the prompt glyph.
fn agent_ready(screen: &str) -> bool {
    screen.lines().any(|l| {
        let t = l.trim_end();
        t == "❯" || t == "›" || t.starts_with("❯ ") && t.trim_start_matches("❯ ").trim().is_empty()
    })
}

/// `deck.agent.<method>` once the pane's headless agent is up (a new
/// deck-server pane starts it at once, but until it connects the server
/// does not know the pane: "no headless agent in pane …").
pub fn agent_call_when_ready(
    rt: &dyn Runtime,
    pane: &str,
    method: &str,
    params: Value,
    limit: Duration,
) -> Result<(), String> {
    let t0 = Instant::now();
    loop {
        match rt.agent_call(method, params.clone()) {
            Ok(_) => return Ok(()),
            Err(e) if t0.elapsed() < limit && e.to_string().contains("no headless agent") => {
                std::thread::sleep(Duration::from_millis(400));
            }
            Err(e) => return Err(format!("the headless agent did not start in {pane}: {e}")),
        }
    }
}

/// Open (or reuse) the workspace for `cwd`, create a tab, start the agent and
/// hand it the prompt. Returns at once with the pane; progress goes out as
/// `deck://quickhack` events (`workspace`, `shell`, `agent`, `prompt`, `done`, `error`).
pub fn launch(
    app: AppHandle,
    server: Option<String>,
    run: String,
    l: Launch,
) -> Result<Value, String> {
    // The folder is on the server's machine: the server says if it is gone.
    let rt = crate::servers::runtime_for(&app, server.as_deref())?;
    let (workspace_id, pane_id) =
        open_session_dir(&*rt, &l.cwd, Some(&l.repo_root), l.label.as_deref(), true)?;
    emit(
        &app,
        &run,
        "workspace",
        json!({ "workspace_id": workspace_id, "pane_id": pane_id }),
    );
    let pane = pane_id.clone();
    std::thread::spawn(move || {
        let result = (|| -> Result<(), String> {
            // deck-server: the new pane already runs Claude headless; hand
            // it the prompt (any agent other than Claude needs a shell).
            if rt.agent_call("list", json!({})).is_ok() {
                emit(&app, &run, "shell", Value::Null);
                agent_call_when_ready(
                    &*rt,
                    &pane,
                    "send",
                    json!({ "pane_id": pane, "text": l.prompt }),
                    Duration::from_secs(60),
                )?;
                emit(&app, &run, "agent", Value::Null);
                emit(&app, &run, "prompt", Value::Null);
                return Ok(());
            }
            if !wait_for(
                &*rt,
                &pane,
                Duration::from_secs(10),
                "viewport",
                shell_ready,
            ) {
                tracing::warn!(pane, "shell prompt not seen; typing anyway");
            }
            emit(&app, &run, "shell", Value::Null);
            type_line(&*rt, &pane, &l.agent)?;
            if !wait_for(
                &*rt,
                &pane,
                Duration::from_secs(90),
                "detection",
                agent_ready,
            ) {
                return Err("the agent did not show its prompt within 90 s".into());
            }
            emit(&app, &run, "agent", Value::Null);
            // the CLI settles its input box a moment after the prompt appears
            std::thread::sleep(Duration::from_millis(600));
            type_line(&*rt, &pane, &l.prompt)?;
            emit(&app, &run, "prompt", Value::Null);
            Ok(())
        })();
        match result {
            Ok(()) => emit(&app, &run, "done", Value::Null),
            Err(e) => emit(&app, &run, "error", Value::String(e)),
        }
    });
    Ok(json!({ "workspace_id": workspace_id, "pane_id": pane_id }))
}

/// A workspace already open on `dir` (by worktree checkout path, else cwd).
pub fn open_workspace_for(rt: &dyn Runtime, dir: &str) -> Option<String> {
    let v = rt.workspace_list().ok()?;
    v.get("workspaces")?.as_array()?.iter().find_map(|w| {
        let path = str_at(w, &["worktree", "checkout_path"]).or_else(|| str_at(w, &["cwd"]))?;
        same_dir(path, dir).then(|| str_at(w, &["workspace_id"]).map(str::to_string))?
    })
}

/// A fresh pane in `dir`: a new tab when a workspace is already open there
/// (the repository's ROOT space for its main checkout, a worktree's own space
/// for a linked worktree); otherwise the space is opened through
/// `worktree.open`, which attaches it to its repository group — root and
/// linked alike — with `workspace.create` as the fallback for plain folders.
/// Returns `(workspace_id, pane_id)`.
pub fn open_session_dir(
    rt: &dyn Runtime,
    dir: &str,
    repo_root: Option<&str>,
    label: Option<&str>,
    focus: bool,
) -> Result<(String, String), String> {
    if let Some(ws) = open_workspace_for(rt, dir) {
        let tab = rt
            .tab_create(runtime::TabCreate {
                workspace_id: ws.clone(),
                cwd: Some(dir.to_string()),
                label: label.map(str::to_string),
                focus,
            })
            .map_err(|e| e.to_string())?;
        let pane = str_at(&tab, &["root_pane", "pane_id"])
            .ok_or("tab.create: no root pane")?
            .to_string();
        return Ok((ws, pane));
    }
    // Paths are the server's machine's: Deck never looks at its own disk for them.
    let root = repo_root.filter(|r| !r.trim().is_empty()).unwrap_or(dir);
    let opened = rt
        .worktree_open(runtime::WorktreeOpen {
            cwd: root.to_string(),
            path: dir.to_string(),
            label: label.map(str::to_string),
            focus,
        })
        .or_else(|first| {
            tracing::warn!(%first, dir, "worktree.open failed; falling back to workspace.create");
            rt.workspace_create(runtime::WorkspaceCreate {
                cwd: Some(dir.to_string()),
                label: label.map(str::to_string),
                source_workspace_id: None,
                focus,
            })
        })
        .map_err(|e| e.to_string())?;
    let ws = str_at(&opened, &["workspace", "workspace_id"])
        .ok_or("no workspace in Herdr's reply")?
        .to_string();
    let pane = str_at(&opened, &["root_pane", "pane_id"])
        .ok_or("no root pane in Herdr's reply")?
        .to_string();
    Ok((ws, pane))
}

fn same_dir(a: &str, b: &str) -> bool {
    let n = |p: &str| {
        p.trim_start_matches(r"\\?\")
            .replace('\\', "/")
            .trim_end_matches('/')
            .to_lowercase()
    };
    n(a) == n(b)
}

/// Open a repository (or any folder) as a workspace: a new tab when it is
/// already open, else attached through `worktree.open` on itself.
/// The folder is on the server's machine (often another one): only the
/// server can say whether it is there, so Deck does not look.
pub fn repo_open(rt: &dyn Runtime, path: &str, label: Option<&str>) -> Result<Value, String> {
    if path.trim().is_empty() {
        return Err("a folder is needed".into());
    }
    let (workspace_id, pane_id) = open_session_dir(rt, path, None, label, true)?;
    Ok(json!({ "workspace_id": workspace_id, "pane_id": pane_id }))
}

/// A new linked worktree of the repository at `cwd` (Herdr creates the
/// checkout and its workspace + tab): `{workspace_id, pane_id}`.
pub fn worktree_create(
    rt: &dyn Runtime,
    cwd: &str,
    branch: &str,
    base: Option<&str>,
) -> Result<Value, String> {
    let branch = branch.trim();
    if branch.is_empty() || branch.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("branch name must be one word".into());
    }
    let v = rt
        .worktree_create(runtime::WorktreeCreate {
            cwd: cwd.to_string(),
            branch: branch.to_string(),
            base: base
                .map(str::trim)
                .filter(|b| !b.is_empty())
                .map(str::to_string),
            focus: true,
        })
        .map_err(|e| e.to_string())?;
    let ws = str_at(&v, &["workspace", "workspace_id"]).ok_or("no workspace in Herdr's reply")?;
    let pane = str_at(&v, &["root_pane", "pane_id"]).ok_or("no root pane in Herdr's reply")?;
    Ok(json!({ "workspace_id": ws, "pane_id": pane, "path": str_at(&v, &["worktree", "path"]) }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder that exists only on the server's machine (here: nowhere on
    /// this one) is still asked for: Deck never checks its own disk.
    #[test]
    fn a_remote_folder_is_opened_by_the_server_not_checked_here() {
        let rt = crate::runtime::tests::Recorder::default();
        let err = repo_open(&rt, "/home/someone/definitely-not-on-this-pc", None).unwrap_err();
        assert!(!err.contains("not a directory"), "{err}");
        let asked = rt.requests();
        assert!(
            asked.iter().any(|r| matches!(r, crate::runtime::Request::WorktreeOpen(w) if w.path == "/home/someone/definitely-not-on-this-pc")),
            "{asked:?}"
        );
    }

    #[test]
    fn readiness_heuristics() {
        assert!(shell_ready("banner\n\nPS C:\\repo> "));
        assert!(shell_ready("user@host:~/x$"));
        assert!(!shell_ready("Loading…\n"));
        assert!(agent_ready("● hi\n────\n❯ \n────\n? for shortcuts"));
        assert!(agent_ready("❯"));
        assert!(!agent_ready("❯ typed text"));
        assert!(!agent_ready("PS C:\\repo>"));
        assert!(same_dir(r"\\?\C:\Work\Repo\", "c:/work/repo"));
        assert!(!same_dir("C:/a", "C:/a/b"));
    }
}
