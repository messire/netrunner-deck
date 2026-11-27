mod accounts;
mod agent;
mod archive;
mod bridge;
mod endpoint;
mod harness;
mod herdr;
mod machine;
mod quickhacks;
mod runtime;
mod servers;
mod transcript;
mod uplink;

use std::path::PathBuf;

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, State};

use runtime::Request;

// Every command below blocks on pipe I/O or a child process. An async command
// keeps that off the main (UI) thread, but tokio workers are few, so the blocking
// body is handed to `spawn_blocking` via `blocking()`. Per-call deadline and
// connection slots live in `herdr::call` (R3), reached through `runtime`.

/// Run a blocking Herdr call on tokio's blocking pool.
pub(crate) async fn blocking<T, E>(
    work: impl FnOnce() -> Result<T, E> + Send + 'static,
) -> Result<T, E>
where
    T: Send + 'static,
    E: Send + 'static + From<String>,
{
    match tauri::async_runtime::spawn_blocking(work).await {
        Ok(result) => result,
        Err(join) => Err(E::from(format!("blocking task failed: {join}"))),
    }
}

/// `ping` → `{version, protocol}`; also the "is Herdr running" probe.
#[tauri::command]
async fn herdr_ping(
    reg: State<'_, servers::Registry>,
    server: Option<String>,
) -> herdr::Result<Value> {
    let rt = reg.runtime(server.as_deref())?;
    blocking(move || rt.ping()).await
}

/// Everything the sidebar needs in one round trip set: workspaces, tabs,
/// panes and agents straight from the server.
#[tauri::command]
async fn herdr_tree(
    reg: State<'_, servers::Registry>,
    server: Option<String>,
) -> herdr::Result<Value> {
    let rt = reg.runtime(server.as_deref())?;
    blocking(move || {
        let workspaces = rt.workspace_list()?;
        let panes = rt.pane_list()?;
        let agents = rt.agent_list()?;
        let mut tabs = Vec::new();
        if let Some(list) = workspaces.get("workspaces").and_then(Value::as_array) {
            for ws in list {
                if let Some(id) = ws.get("workspace_id").and_then(Value::as_str) {
                    // A failing tab.list is an error, not an empty tab bar (audit #15).
                    let t = rt.tab_list(id)?;
                    if let Some(items) = t.get("tabs").and_then(Value::as_array) {
                        tabs.extend(items.iter().cloned());
                    }
                }
            }
        }
        Ok(json!({
            "workspaces": workspaces.get("workspaces").cloned().unwrap_or(Value::Array(vec![])),
            "tabs": tabs,
            "panes": panes.get("panes").cloned().unwrap_or(Value::Array(vec![])),
            "agents": agents.get("agents").cloned().unwrap_or(Value::Array(vec![])),
        }))
    })
    .await
}

/// ANSI snapshot of a pane (recent scrollback + screen). Milestone 1 renders
/// this into xterm; milestone 2 replaces it with a streaming attach.
#[tauri::command]
async fn herdr_pane_read(
    reg: State<'_, servers::Registry>,
    server: Option<String>,
    pane_id: String,
    lines: Option<u32>,
) -> herdr::Result<Value> {
    let rt = reg.runtime(server.as_deref())?;
    blocking(move || {
        let read = rt.pane_read(runtime::PaneRead {
            pane_id: pane_id.clone(),
            source: "recent".into(),
            lines: lines.unwrap_or(400),
            format: "ansi".into(),
            strip_ansi: Some(false),
        })?;
        // Debug trace of the read cadence (Herdr only logs failed reads).
        tracing::debug!(
            pane = %pane_id,
            revision = read["read"]["revision"].as_u64().unwrap_or(0),
            "pane.read"
        );
        Ok(read)
    })
    .await
}

#[tauri::command]
async fn herdr_pane_send_text(
    reg: State<'_, servers::Registry>,
    server: Option<String>,
    pane_id: String,
    text: String,
) -> herdr::Result<Value> {
    let rt = reg.runtime(server.as_deref())?;
    blocking(move || rt.pane_send_text(&pane_id, &text)).await
}

#[tauri::command]
async fn herdr_pane_send_keys(
    reg: State<'_, servers::Registry>,
    server: Option<String>,
    pane_id: String,
    keys: Vec<String>,
) -> herdr::Result<Value> {
    let rt = reg.runtime(server.as_deref())?;
    blocking(move || runtime::dispatch(&*rt, Request::PaneSendKeys { pane_id, keys })).await
}

#[tauri::command]
async fn herdr_focus(
    reg: State<'_, servers::Registry>,
    server: Option<String>,
    target: String,
    id: String,
) -> herdr::Result<Value> {
    // An unknown target is a caller bug, not a pane (audit: optional `herdr_focus` note).
    let req = focus_request(&target, id).map_err(herdr::HerdrError::Shape)?;
    let rt = reg.runtime(server.as_deref())?;
    blocking(move || runtime::dispatch(&*rt, req)).await
}

/// Tree actions (M3): create/close workspaces, tabs and panes. The webview
/// names the action, not the Herdr method, so the command surface stays an
/// allowlist (audit: command surface) and the params each action may carry
/// are fixed here.
#[tauri::command]
async fn herdr_action(
    reg: State<'_, servers::Registry>,
    server: Option<String>,
    action: String,
    id: Option<String>,
    options: Option<Value>,
) -> herdr::Result<Value> {
    let req = action_request(&action, id.as_deref(), options.as_ref())
        .map_err(herdr::HerdrError::Shape)?;
    let rt = reg.runtime(server.as_deref())?;
    blocking(move || runtime::dispatch(&*rt, req)).await
}

/// Map an action name to its runtime request. `id` is the workspace, tab or
/// pane the action applies to (required where the method needs one);
/// `options` may carry `cwd`/`label` for create actions.
fn action_request(
    action: &str,
    id: Option<&str>,
    options: Option<&Value>,
) -> Result<Request, String> {
    let opt = |key: &str| {
        options
            .and_then(|o| o.get(key))
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    let need_id = || -> Result<String, String> {
        id.filter(|s| !s.is_empty())
            .map(str::to_string)
            .ok_or_else(|| format!("{action} needs an id"))
    };
    let need_label = || opt("label").ok_or_else(|| format!("{action} needs a label"));
    Ok(match action {
        "workspace.create" => Request::WorkspaceCreate(runtime::WorkspaceCreate {
            cwd: opt("cwd"),
            label: opt("label"),
            source_workspace_id: id.filter(|s| !s.is_empty()).map(str::to_string),
            focus: true,
        }),
        "workspace.close" => Request::WorkspaceClose {
            workspace_id: need_id()?,
            close_group: options
                .and_then(|o| o.get("close_group"))
                .and_then(Value::as_bool)
                == Some(true),
        },
        "tab.create" => Request::TabCreate(runtime::TabCreate {
            workspace_id: need_id()?,
            cwd: opt("cwd"),
            label: opt("label"),
            focus: true,
        }),
        "tab.close" => Request::TabClose { tab_id: need_id()? },
        "pane.close" => Request::PaneClose {
            pane_id: need_id()?,
        },
        // Rename: `label` from options (required; the server trims).
        "workspace.rename" => Request::WorkspaceRename {
            workspace_id: need_id()?,
            label: need_label()?,
        },
        "tab.rename" => Request::TabRename {
            tab_id: need_id()?,
            label: need_label()?,
        },
        "pane.rename" => Request::PaneRename {
            pane_id: need_id()?,
            label: need_label()?,
        },
        // Split next to a pane (M6 tree): `direction` is right|down, default right.
        "pane.split" => Request::PaneSplit(runtime::PaneSplit {
            target_pane_id: need_id()?,
            direction: opt("direction")
                .and_then(|d| runtime::SplitDirection::parse(&d))
                .unwrap_or_default(),
            focus: true,
        }),
        other => return Err(format!("unknown action: {other}")),
    })
}

/// Desktop notification (M4): the webview decides when (an agent turned
/// `blocked` while its pane is not on screen); the OS shows it.
#[tauri::command]
async fn notify(app: AppHandle, title: String, body: String) -> Result<(), String> {
    // Windows: always under Deck's own AppUserModelID (registered by
    // `register_toast_identity`), also in a dev build, which the plugin would
    // otherwise show as "Windows PowerShell".
    #[cfg(windows)]
    {
        let _ = &app;
        tauri_winrt_notification::Toast::new(TOAST_AUMID)
            .title(&title)
            .text1(&body)
            .show()
            .map_err(|e| e.to_string())
    }
    #[cfg(not(windows))]
    {
        use tauri_plugin_notification::NotificationExt;
        app.notification()
            .builder()
            .title(title)
            .body(body)
            .show()
            .map_err(|e| e.to_string())
    }
}

/// The AppUserModelID Deck's toasts are shown under; the installer's Start
/// menu shortcut carries the same one.
#[cfg(windows)]
const TOAST_AUMID: &str = "dev.messire.netrunner-deck";

/// Gives `TOAST_AUMID` a name and an icon, so toasts read "Netrunner Deck"
/// with Deck's icon: `HKCU\Software\Classes\AppUserModelId\<id>` plus a PNG
/// of the icon in the data folder. Done on every start; cheap and idempotent.
#[cfg(windows)]
fn register_toast_identity(data_dir: &std::path::Path) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let icon = data_dir.join("notify-icon.png");
    let _ = std::fs::create_dir_all(data_dir);
    let _ = std::fs::write(&icon, include_bytes!("../icons/128x128.png"));
    let key = format!(r"HKCU\Software\Classes\AppUserModelId\{TOAST_AUMID}");
    for (name, value) in [
        ("DisplayName", "Netrunner Deck".to_string()),
        ("IconUri", icon.display().to_string()),
    ] {
        let _ = std::process::Command::new("reg")
            .args(["add", &key, "/v", name, "/t", "REG_SZ", "/d", &value, "/f"])
            .creation_flags(CREATE_NO_WINDOW)
            .output();
    }
}

/// Frontend diagnostics (the pane view's consistency check, M2) appended to
/// `<app log dir>/deck-diag.log` — the release build has no console, so this
/// is the only trace of a glitch the user saw. The file is capped at 2 MB.
#[tauri::command]
fn diag_log(app: AppHandle, line: String) -> Result<(), String> {
    use std::io::Write;
    let dir = app.path().app_log_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join("deck-diag.log");
    let truncate = std::fs::metadata(&path)
        .map(|m| m.len() > 2 * 1024 * 1024)
        .unwrap_or(false);
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(!truncate)
        .write(true)
        .truncate(truncate)
        .open(&path)
        .map_err(|e| e.to_string())?;
    writeln!(file, "{line}").map_err(|e| e.to_string())?;
    tracing::warn!(target: "deck::diag", "{line}");
    Ok(())
}

/// Clipboard text for the pane view's ctrl+v / shift+insert. Read on the
/// Rust side: WebView2 gates `navigator.clipboard.readText` behind a
/// permission prompt. No text on the clipboard (an image, nothing) is an
/// empty string, not an error — Claude Code's own alt+v handles images.
#[tauri::command]
async fn clipboard_read_text(app: AppHandle) -> Result<String, String> {
    use tauri_plugin_clipboard_manager::ClipboardExt;
    match app.clipboard().read_text() {
        Ok(text) => Ok(text),
        Err(err) => {
            tracing::debug!(%err, "clipboard has no text");
            Ok(String::new())
        }
    }
}

/// ctrl+c / ctrl+insert / the pane menu's COPY with a selection. Written on
/// the Rust side: the xterm holding the selection (the chat) is never the
/// focused one (the input box), so the browser's own copy has nothing to copy.
#[tauri::command]
async fn clipboard_write_text(app: AppHandle, text: String) -> Result<(), String> {
    use tauri_plugin_clipboard_manager::ClipboardExt;
    app.clipboard().write_text(text).map_err(|e| e.to_string())
}

/// The update `update_check` found, kept for `update_install` (M5).
pub struct PendingUpdate(pub std::sync::Mutex<Option<tauri_plugin_updater::Update>>);

#[derive(serde::Serialize)]
pub struct UpdateInfo {
    version: String,
    current: String,
    notes: Option<String>,
    date: Option<String>,
}

/// Ask the release endpoint (`plugins.updater` in `tauri.conf.json`) whether a
/// newer signed build exists. `None` = up to date. Network/endpoint failures are
/// errors the webview may ignore: the check is advisory.
#[tauri::command]
async fn update_check(app: AppHandle) -> Result<Option<UpdateInfo>, String> {
    use tauri_plugin_updater::UpdaterExt;
    let updater = app.updater().map_err(|e| e.to_string())?;
    let Some(update) = updater.check().await.map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    let info = UpdateInfo {
        version: update.version.clone(),
        current: update.current_version.clone(),
        notes: update.body.clone(),
        date: update.date.map(|d| d.to_string()),
    };
    *app.state::<PendingUpdate>()
        .0
        .lock()
        .map_err(|e| e.to_string())? = Some(update);
    Ok(Some(info))
}

/// Download, verify the signature and run the installer of the pending update.
/// On Windows the installer closes the running app itself (passive mode).
#[tauri::command]
async fn update_install(app: AppHandle) -> Result<(), String> {
    let pending = app
        .state::<PendingUpdate>()
        .0
        .lock()
        .map_err(|e| e.to_string())?
        .take();
    let Some(update) = pending else {
        return Err("no pending update; run update_check first".into());
    };
    update
        .download_and_install(|_, _| {}, || {})
        .await
        .map_err(|e| e.to_string())
}

/// The focus request for a `herdr_focus` target.
fn focus_request(target: &str, id: String) -> Result<Request, String> {
    match target {
        "workspace" => Ok(Request::WorkspaceFocus { workspace_id: id }),
        "tab" => Ok(Request::TabFocus { tab_id: id }),
        "pane" => Ok(Request::PaneFocus { pane_id: id }),
        _ => Err(format!("unknown focus target: {target}")),
    }
}

// ---- transcript (chat view source) -----------------------------------------

/// Lines of a session transcript from a byte offset (or its last `tail`
/// bytes), from the machine of `server` (the pane's; null = the primary).
#[tauri::command]
async fn transcript_read(
    app: AppHandle,
    server: Option<String>,
    session_id: String,
    from: Option<u64>,
    tail: Option<u64>,
) -> Result<transcript::Chunk, String> {
    blocking(move || transcript::read_on(&app, server.as_deref(), &session_id, from, tail)).await
}

/// Last-modified time (ms since epoch) of each session's transcript on
/// `server`; sessions without one are left out. Orders the session map by
/// real recency.
#[tauri::command]
async fn transcript_mtimes(
    app: AppHandle,
    server: Option<String>,
    session_ids: Vec<String>,
) -> Result<std::collections::HashMap<String, u64>, String> {
    blocking(move || transcript::mtimes_on(&app, server.as_deref(), &session_ids)).await
}

/// Watch one transcript for growth (`deck://transcript`); null stops.
#[tauri::command]
async fn transcript_watch(
    app: AppHandle,
    server: Option<String>,
    session_id: Option<String>,
) -> Result<(), String> {
    blocking(move || transcript::watch(&app, server, session_id)).await
}

/// Folders Claude Code knows on `server`'s machine (`deck.repos.list`).
#[tauri::command]
async fn repos_list(app: AppHandle, server: Option<String>) -> Result<Value, String> {
    blocking(move || accounts::repos(&app, server.as_deref())).await
}

/// `deck.fs.stat` / `deck.fs.mkdir` for a path on `server`'s machine.
#[tauri::command]
async fn fs_call(
    app: AppHandle,
    server: Option<String>,
    method: String,
    path: String,
) -> Result<Value, String> {
    blocking(move || accounts::fs(&app, server.as_deref(), &method, &path)).await
}

/// `deck.git.*` (clone), `deck.github.*` (sign-in) and `deck.transfer.*`
/// on `server`'s machine: the server does the work on its own disk.
#[tauri::command]
async fn machine_call(
    app: AppHandle,
    server: Option<String>,
    method: String,
    params: Value,
) -> Result<Value, String> {
    blocking(move || machine::call(&app, server.as_deref(), &method, params)).await
}

/// Relays a transfer's pack from the source server's export to the target
/// server's import, on its own thread: `{relay}`.
#[tauri::command]
async fn transfer_relay(
    app: AppHandle,
    from: Option<String>,
    to: Option<String>,
    export_job: String,
    import_job: String,
) -> Result<Value, String> {
    blocking(move || {
        machine::relay_start(&app, from.as_deref(), to.as_deref(), export_job, import_job)
    })
    .await
}

/// `{sent, size, done, error}` of a relay.
#[tauri::command]
fn transfer_relay_status(relay: String) -> Result<Value, String> {
    machine::relay_status(&relay)
}

/// Opens what a transfer brought to `server` as spaces and resumes its sessions.
#[tauri::command]
async fn transfer_open(
    app: AppHandle,
    server: Option<String>,
    worktrees: Vec<machine::Placed>,
    sessions: Vec<machine::Arrived>,
) -> Result<Value, String> {
    blocking(move || machine::open_on(&app, server.as_deref(), worktrees, sessions)).await
}

// ---- headless Claude sessions (deck-server) ---------------------------------

/// `deck.agent.<method>` (send, respond, interrupt, set, resume, restart, list) on `server`.
#[tauri::command]
async fn agent_call(
    app: AppHandle,
    server: Option<String>,
    method: String,
    params: Value,
) -> Result<Value, String> {
    blocking(move || agent::call(&app, server.as_deref(), &method, params)).await
}

/// Stream a headless pane's events to the webview (`deck://agent`).
#[tauri::command]
fn agent_watch(app: AppHandle, server: Option<String>, pane_id: String) {
    agent::watch(&app, server, pane_id);
}

#[tauri::command]
fn agent_unwatch(app: AppHandle, server: Option<String>, pane_id: String) {
    agent::unwatch(&app, server.as_deref(), &pane_id);
}

// ---- external links ---------------------------------------------------------

/// Open an http(s) link in the system browser. Chat markdown may carry any
/// URL; the webview itself never navigates there (see `ChatView`'s click guard).
#[tauri::command]
fn open_external(url: String) -> Result<(), String> {
    let ok = (url.starts_with("https://") || url.starts_with("http://"))
        && url.len() < 4096
        && !url.chars().any(|c| c.is_control() || c.is_whitespace());
    if !ok {
        return Err("only http(s) links are opened".into());
    }
    #[cfg(windows)]
    let status = archive::quiet_command("rundll32")
        .args(["url.dll,FileProtocolHandler", &url])
        .status();
    #[cfg(target_os = "macos")]
    let status = std::process::Command::new("open").arg(&url).status();
    #[cfg(all(unix, not(target_os = "macos")))]
    let status = std::process::Command::new("xdg-open").arg(&url).status();
    status.map(|_| ()).map_err(|e| e.to_string())
}

// ---- quickhacks (pinned harness launchers) ---------------------------------

#[tauri::command]
fn quickhacks_get(app: AppHandle) -> quickhacks::Saved {
    app.state::<quickhacks::Store>().get()
}

#[tauri::command]
fn quickhacks_set(app: AppHandle, saved: quickhacks::Saved) -> Result<quickhacks::Saved, String> {
    app.state::<quickhacks::Store>().set(saved)
}

/// `worktree.list` for the repository containing `cwd`.
#[tauri::command]
async fn quickhack_worktrees(
    reg: State<'_, servers::Registry>,
    server: Option<String>,
    cwd: String,
) -> Result<Value, String> {
    let rt = reg.runtime(server.as_deref())?;
    blocking(move || quickhacks::worktrees(&*rt, &cwd)).await
}

/// Open a session in `launch.cwd`, start the agent, send the first prompt. Progress: `deck://quickhack`.
#[tauri::command]
async fn quickhack_launch(
    app: AppHandle,
    run: String,
    launch: quickhacks::Launch,
    server: Option<String>,
) -> Result<Value, String> {
    blocking(move || quickhacks::launch(app, server, run, launch)).await
}

/// Open a repository folder as a workspace (sidebar "+" next to SESSIONS).
#[tauri::command]
async fn repo_open(
    reg: State<'_, servers::Registry>,
    server: Option<String>,
    path: String,
    label: Option<String>,
) -> Result<Value, String> {
    let rt = reg.runtime(server.as_deref())?;
    blocking(move || quickhacks::repo_open(&*rt, &path, label.as_deref())).await
}

/// New linked worktree inside a repository (sidebar "+" on a repo row).
#[tauri::command]
async fn worktree_create(
    reg: State<'_, servers::Registry>,
    server: Option<String>,
    cwd: String,
    branch: String,
    base: Option<String>,
) -> Result<Value, String> {
    let rt = reg.runtime(server.as_deref())?;
    blocking(move || quickhacks::worktree_create(&*rt, &cwd, &branch, base.as_deref())).await
}

// ---- uplink (git behind/ahead, pull, what came in) ---------------------------
//
// The git work runs on the server's machine (`deck.uplink.*` through its
// bridge), where the checkouts are; Deck keeps the log of pulls.

/// `method` of a machine on `server`'s machine, off the async runtime.
async fn on_machine(
    reg: &servers::Registry,
    server: Option<String>,
    method: &'static str,
    params: Value,
) -> Result<Value, String> {
    let rt = reg.runtime(server.as_deref())?;
    blocking(move || rt.machine_call(method, params).map_err(|e| e.to_string())).await
}

/// Behind/ahead/dirty for the given checkouts of `server` (no network).
#[tauri::command]
async fn uplink_sync(
    reg: State<'_, servers::Registry>,
    server: Option<String>,
    paths: Vec<String>,
) -> Result<std::collections::HashMap<String, uplink::Sync>, String> {
    let v = on_machine(&reg, server, "deck.uplink.sync", json!({ "paths": paths })).await?;
    serde_json::from_value(v).map_err(|e| e.to_string())
}

/// `git fetch` for the given repository roots of `server` (network; errors
/// per root are returned, not raised).
#[tauri::command]
async fn uplink_fetch(
    reg: State<'_, servers::Registry>,
    server: Option<String>,
    roots: Vec<String>,
) -> Result<std::collections::HashMap<String, String>, String> {
    let v = on_machine(&reg, server, "deck.uplink.fetch", json!({ "roots": roots })).await?;
    serde_json::from_value(v).map_err(|e| e.to_string())
}

/// `git pull --ff-only` in a checkout of `server`; the update (commits, PR
/// links) is logged with its server and returned.
#[tauri::command]
async fn uplink_pull(
    app: AppHandle,
    reg: State<'_, servers::Registry>,
    server: Option<String>,
    repo: String,
    path: String,
) -> Result<uplink::Update, String> {
    let id = server
        .clone()
        .or_else(|| reg.primary_id())
        .ok_or("no server is connected")?;
    let v = on_machine(
        &reg,
        Some(id.clone()),
        "deck.uplink.pull",
        json!({ "repo": repo, "path": path }),
    )
    .await?;
    let mut u: uplink::Update = serde_json::from_value(v).map_err(|e| e.to_string())?;
    u.server = Some(id);
    app.state::<uplink::Store>().push(u.clone())?;
    let _ = app.emit("deck://uplink", "changed");
    Ok(u)
}

#[tauri::command]
fn uplink_list(app: AppHandle) -> uplink::Saved {
    app.state::<uplink::Store>().get()
}

#[tauri::command]
fn uplink_seen(app: AppHandle) -> Result<uplink::Saved, String> {
    app.state::<uplink::Store>().mark_seen()
}

#[tauri::command]
fn uplink_remove(app: AppHandle, id: String) -> Result<uplink::Saved, String> {
    app.state::<uplink::Store>().remove(&id)
}

// ---- harness (Claude Code configuration per repository) --------------------

/// Personal (`~/.claude`) first, then the given repositories' Claude Code
/// files, as `server`'s machine has them.
#[tauri::command]
async fn harness_scan(
    app: AppHandle,
    reg: State<'_, servers::Registry>,
    server: Option<String>,
    repos: Vec<harness::RepoRef>,
) -> Result<Vec<harness::HarnessRepo>, String> {
    let id = server
        .clone()
        .or_else(|| reg.primary_id())
        .ok_or("no server is connected")?;
    let refs: Vec<Value> = repos
        .iter()
        .map(|r| json!({ "key": r.key, "name": r.name, "root": r.root }))
        .collect();
    let v = on_machine(
        &reg,
        Some(id.clone()),
        "deck.harness.scan",
        json!({ "repos": refs }),
    )
    .await?;
    let list: Vec<harness::HarnessRepo> =
        serde_json::from_value(v.get("repos").cloned().unwrap_or_default())
            .map_err(|e| e.to_string())?;
    let roots = list
        .iter()
        .map(|r| std::path::PathBuf::from(&r.root))
        .collect();
    app.state::<harness::Roots>()
        .0
        .lock()
        .map_err(|_| "roots poisoned".to_string())?
        .insert(id, roots);
    Ok(list)
}

/// Tag the untagged skills/agents/commands/rules of one repo (shadow claude -p),
/// writing `tags:` into their frontmatter; `deck://harness` when done. Only
/// for a server on this computer: the files are written here.
#[tauri::command]
async fn harness_classify(
    app: AppHandle,
    reg: State<'_, servers::Registry>,
    server: Option<String>,
    key: String,
    items: Vec<harness::Item>,
) -> Result<bool, String> {
    let id = server
        .or_else(|| reg.primary_id())
        .ok_or("no server is connected")?;
    if reg.is_remote(&id) {
        return Err("CLASSIFY writes files on this computer only".into());
    }
    // Only files under a scanned root are ever written, whatever the caller lists.
    let roots = app
        .state::<harness::Roots>()
        .0
        .lock()
        .map_err(|_| "roots poisoned".to_string())?
        .get(&id)
        .cloned()
        .unwrap_or_default();
    let items: Vec<harness::Item> = items
        .into_iter()
        .filter(|i| harness::is_under_roots(&i.path, &roots))
        .collect();
    Ok(harness::classify(app, key, items))
}

/// Text of one file listed by `harness_scan` of `server`.
#[tauri::command]
async fn harness_read(
    app: AppHandle,
    reg: State<'_, servers::Registry>,
    server: Option<String>,
    path: String,
) -> Result<String, String> {
    let id = server
        .or_else(|| reg.primary_id())
        .ok_or("no server is connected")?;
    let roots: Vec<String> = app
        .state::<harness::Roots>()
        .0
        .lock()
        .map_err(|_| "roots poisoned".to_string())?
        .get(&id)
        .map(|r| r.iter().map(|p| p.to_string_lossy().into_owned()).collect())
        .unwrap_or_default();
    let v = on_machine(
        &reg,
        Some(id),
        "deck.harness.read",
        json!({ "path": path, "roots": roots }),
    )
    .await?;
    Ok(v.get("text")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string())
}

// ---- session archive (cold storage) ---------------------------------------

/// The archive of `server` (`None`: the primary one).
#[tauri::command]
async fn archive_list(
    app: AppHandle,
    server: Option<String>,
) -> Result<Vec<archive::Entry>, String> {
    blocking(move || archive::list(&app, server.as_deref())).await
}

/// Remember the workspace (label, checkout, Claude session ids), close it in
/// Herdr, then write its description in the background.
#[tauri::command]
async fn archive_close(
    app: AppHandle,
    workspace_id: String,
    close_group: Option<bool>,
    server: Option<String>,
) -> Result<Value, String> {
    blocking(move || {
        archive::archive_and_close(
            &app,
            server.as_deref(),
            &workspace_id,
            close_group.unwrap_or(false),
        )
    })
    .await
}

/// Reopen the archived space on its server and resume each session in its own tab.
#[tauri::command]
async fn archive_restore(
    app: AppHandle,
    id: String,
    server: Option<String>,
) -> Result<Value, String> {
    blocking(move || archive::restore(&app, server.as_deref(), &id)).await
}

/// Forget the entry; with `remove_worktree`, also `git worktree remove` its linked checkout.
#[tauri::command]
async fn archive_delete(
    app: AppHandle,
    id: String,
    server: Option<String>,
    remove_worktree: Option<bool>,
) -> Result<Value, String> {
    blocking(move || {
        archive::delete(
            &app,
            server.as_deref(),
            &id,
            remove_worktree.unwrap_or(false),
        )
    })
    .await
}

#[tauri::command]
async fn archive_groups_get(app: AppHandle) -> Result<Vec<archive::RepoGroup>, String> {
    Ok(archive::groups(&app))
}

#[tauri::command]
async fn archive_groups_set(
    app: AppHandle,
    groups: Vec<archive::RepoGroup>,
) -> Result<Vec<archive::RepoGroup>, String> {
    archive::set_groups(&app, groups)
}

#[tauri::command]
async fn archive_summarize(
    app: AppHandle,
    id: String,
    server: Option<String>,
) -> Result<(), String> {
    archive::resummarize(&app, server.as_deref(), &id)
}

/// `deck.accounts.<method>` on `server`: the Claude Code accounts of that
/// server's machine (list, switch, remove, login.start, login.finish).
/// A stock Herdr has none: `{managed: false}`.
#[tauri::command]
async fn accounts_call(
    app: AppHandle,
    server: Option<String>,
    method: String,
    params: Value,
) -> Result<Value, String> {
    blocking(move || accounts::call(&app, server.as_deref(), &method, params)).await
}

/// Forward Herdr events to the webview as `herdr://event` until the window dies.
///
/// `Runtime::subscribe` only returns `Ok` when the webview stopped accepting
/// events; every other exit (refused subscription, closed stream, transport
/// error) is an `Err` and is surfaced as `herdr://offline` with the error
/// text, then retried after a short pause.
/// Subscriptions every Herdr build accepts (names must match Herdr's
/// `Subscription` enum exactly; one unknown name rejects the whole set).
const BASE_SUBSCRIPTIONS: &[&str] = &[
    "workspace.created",
    "workspace.updated",
    "workspace.renamed",
    "workspace.closed",
    "workspace.focused",
    "tab.created",
    "tab.closed",
    "tab.focused",
    "tab.renamed",
    "tab.moved",
    "pane.created",
    "pane.closed",
    "pane.updated",
    "pane.focused",
    "pane.agent_detected",
    // `pane.agent_status_changed` is per-pane (needs pane_id); status
    // changes also arrive as `pane_updated`, so it is not subscribed here.
    "layout.updated",
];

/// Fork-only (M1.5): coalesced output notifications with a content revision.
/// Upstream Herdr rejects the name, so the pump falls back to the base set.
const OUTPUT_SUBSCRIPTION: &str = "pane.output_changed";

fn subscription_set(with_output: bool) -> Vec<&'static str> {
    let mut names = BASE_SUBSCRIPTIONS.to_vec();
    if with_output {
        names.push(OUTPUT_SUBSCRIPTION);
    }
    names
}

/// A subscription-set rejection (as opposed to a transport failure) is the
/// server saying it does not know one of the names.
fn is_subscription_rejection(err: &herdr::HerdrError) -> bool {
    matches!(err, herdr::HerdrError::Api { .. })
}

/// Forward one server's events to the webview as `herdr://event`, each
/// tagged with `server: <id>`, until `stop` is set (the server was
/// disconnected). Each round pings first so the server's status (version,
/// online/offline) is current; a failed round reports `herdr://offline`
/// (`{server, error}`) and retries after a short pause.
pub(crate) fn start_event_pump(
    app: AppHandle,
    server: String,
    rt: runtime::SharedRuntime,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    use std::sync::atomic::Ordering;
    let name = format!("herdr-events:{server}");
    let spawned = std::thread::Builder::new().name(name).spawn(move || {
        let mut with_output = true;
        while !stop.load(Ordering::SeqCst) {
            let ping = rt.ping();
            app.state::<servers::Registry>()
                .set_status(&app, &server, servers::Status::from_ping(&ping));
            let result = match ping {
                Err(err) => Err(err),
                Ok(_) => {
                    let handle = app.clone();
                    let names = subscription_set(with_output);
                    let stop_flag = stop.clone();
                    let id = server.clone();
                    rt.subscribe(&names, &mut |mut event| {
                        if stop_flag.load(Ordering::SeqCst) {
                            return false;
                        }
                        if let Some(obj) = event.as_object_mut() {
                            obj.insert("server".into(), Value::String(id.clone()));
                        }
                        handle.emit("herdr://event", event).is_ok()
                    })
                }
            };
            if stop.load(Ordering::SeqCst) {
                break;
            }
            match result {
                Ok(()) => {
                    tracing::info!(%server, "herdr event consumer stopped accepting events; retrying");
                }
                Err(err) if with_output && is_subscription_rejection(&err) => {
                    // Older server without the fork's output event: drop it and
                    // reconnect at once; the pane view keeps polling.
                    tracing::warn!(%server, %err, "server rejected {OUTPUT_SUBSCRIPTION}; subscribing without it");
                    with_output = false;
                    continue;
                }
                Err(err) => {
                    tracing::warn!(%server, %err, "herdr event stream ended; reconnecting");
                    app.state::<servers::Registry>().set_status(
                        &app,
                        &server,
                        servers::Status::offline(err.to_string()),
                    );
                    let _ = app.emit(
                        "herdr://offline",
                        json!({ "server": server, "error": err.to_string() }),
                    );
                }
            }
            for _ in 0..20 {
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
        tracing::info!(%server, "event pump stopped (server disconnected)");
    });
    if let Err(err) = spawned {
        tracing::warn!(%err, "could not start the event pump");
    }
}

/// Tracing → `<app log dir>/deck.log` (the window has no console; truncated
/// at 4 MB on start) and stdout, which a dev build started from a terminal
/// still has. `RUST_LOG` overrides; the default is info so event-pump
/// warnings are visible.
fn init_tracing(app: &AppHandle) {
    use tracing_subscriber::fmt::writer::MakeWriterExt;
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let file = app.path().app_log_dir().ok().and_then(|dir| {
        std::fs::create_dir_all(&dir).ok()?;
        let path = dir.join("deck.log");
        let truncate = std::fs::metadata(&path)
            .map(|m| m.len() > 4 * 1024 * 1024)
            .unwrap_or(false);
        std::fs::OpenOptions::new()
            .create(true)
            .append(!truncate)
            .write(true)
            .truncate(truncate)
            .open(path)
            .ok()
    });
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false);
    match file {
        Some(file) => builder
            .with_writer(std::sync::Mutex::new(file).and(std::io::stdout))
            .init(),
        None => builder.with_writer(std::io::stdout).init(),
    }
}

/// Deck's own data (archive, quickhacks, uplink, shadow workdir).
/// `DECK_DATA_DIR` moves it, so a Deck run against a test server
/// (`scripts/try-deck-server.ps1`) never writes the normal Deck's files.
pub(crate) fn data_dir(app: &AppHandle) -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("DECK_DATA_DIR").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    app.path().app_data_dir().ok()
}

/// Window size/position survive restarts. A Deck with its own `DECK_DATA_DIR`
/// keeps its own window-state file, so it does not move the normal Deck's window.
fn window_state_plugin<R: tauri::Runtime>() -> tauri::plugin::TauriPlugin<R> {
    let builder = tauri_plugin_window_state::Builder::new();
    if std::env::var_os("DECK_DATA_DIR").is_some_and(|d| !d.is_empty()) {
        builder.with_filename(".window-state.try.json").build()
    } else {
        builder.build()
    }
}

pub fn run() {
    tauri::Builder::default()
        // Window size/position survive restarts (UI-only state, roadmap M4).
        .plugin(window_state_plugin())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_clipboard_manager::init())
        .manage(PendingUpdate(std::sync::Mutex::new(None)))
        .setup(|app| {
            init_tracing(app.handle());
            let data_dir = data_dir(app.handle()).unwrap_or_else(std::env::temp_dir);
            #[cfg(windows)]
            {
                let dir = data_dir.clone();
                std::thread::spawn(move || register_toast_identity(&dir));
            }
            app.manage(servers::Registry::open(&data_dir));
            app.manage(archive::Store::open(&data_dir));
            app.manage(transcript::Watch::default());
            app.manage(agent::Watches::default());
            app.manage(harness::Roots::default());
            app.manage(harness::Classifying::default());
            app.manage(quickhacks::Store::open(&data_dir));
            app.manage(uplink::Store::open(&data_dir));
            transcript::start_poller(app.handle().clone());
            servers::Registry::start(app.handle());
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_title("Netrunner Deck");
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            herdr_ping,
            herdr_tree,
            herdr_pane_read,
            herdr_pane_send_text,
            herdr_pane_send_keys,
            herdr_focus,
            herdr_action,
            notify,
            diag_log,
            clipboard_read_text,
            clipboard_write_text,
            update_check,
            update_install,
            accounts_call,
            repos_list,
            fs_call,
            machine_call,
            transfer_relay,
            transfer_relay_status,
            transfer_open,
            archive_list,
            archive_close,
            archive_restore,
            archive_delete,
            archive_summarize,
            archive_groups_get,
            archive_groups_set,
            harness_scan,
            harness_read,
            harness_classify,
            open_external,
            quickhacks_get,
            quickhacks_set,
            quickhack_worktrees,
            quickhack_launch,
            repo_open,
            worktree_create,
            uplink_sync,
            uplink_fetch,
            uplink_pull,
            uplink_list,
            uplink_seen,
            uplink_remove,
            transcript_read,
            transcript_mtimes,
            transcript_watch,
            agent_call,
            agent_watch,
            agent_unwatch,
            servers::servers_list,
            servers::servers_scan,
            servers::servers_seen,
            servers::server_connect,
            servers::server_disconnect,
            servers::server_stop,
            servers::server_rename,
            servers::server_add,
            servers::server_forget,
            servers::server_probe,
            servers::server_add_host,
            servers::server_remove_host,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Netrunner Deck")
        .run(|_, event| {
            // SSH tunnels are child processes: they must not outlive Deck.
            if let tauri::RunEvent::Exit = event {
                endpoint::close_all();
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `action_request` as the Herdr method and params it sends.
    fn req_of(
        action: &str,
        id: Option<&str>,
        options: Option<&Value>,
    ) -> Result<(&'static str, Value), String> {
        action_request(action, id, options).map(|r| r.herdr())
    }

    #[test]
    fn action_request_builds_pane_split_params() {
        let (m, p) = req_of(
            "pane.split",
            Some("p_1"),
            Some(&json!({ "direction": "down" })),
        )
        .unwrap();
        assert_eq!(m, "pane.split");
        assert_eq!(
            p,
            json!({ "target_pane_id": "p_1", "direction": "down", "focus": true })
        );
        let (_, p) = req_of(
            "pane.split",
            Some("p_1"),
            Some(&json!({ "direction": "sideways" })),
        )
        .unwrap();
        assert_eq!(p["direction"], "right");
        assert!(req_of("pane.split", None, None).is_err());
    }

    #[test]
    fn action_request_builds_create_and_close_params() {
        let (m, p) = req_of(
            "workspace.create",
            Some("ws_1"),
            Some(&json!({ "cwd": "C:\\x", "label": "L", "env": { "A": "1" } })),
        )
        .unwrap();
        assert_eq!(m, "workspace.create");
        assert_eq!(
            p,
            json!({ "source_workspace_id": "ws_1", "focus": true, "cwd": "C:\\x", "label": "L" })
        );
        let (m, p) = req_of("workspace.create", None, None).unwrap();
        assert_eq!((m, p), ("workspace.create", json!({ "focus": true })));
        let (m, p) = req_of("tab.create", Some("ws_1"), None).unwrap();
        assert_eq!(
            (m, p),
            (
                "tab.create",
                json!({ "workspace_id": "ws_1", "focus": true })
            )
        );
        let (m, p) = req_of(
            "tab.rename",
            Some("t_1"),
            Some(&serde_json::json!({ "label": "dev" })),
        )
        .unwrap();
        assert_eq!(m, "tab.rename");
        assert_eq!(p, serde_json::json!({ "tab_id": "t_1", "label": "dev" }));
        assert!(req_of("tab.rename", Some("t_1"), None).is_err());
        let (m, p) = req_of("tab.close", Some("t_1"), None).unwrap();
        assert_eq!((m, p), ("tab.close", json!({ "tab_id": "t_1" })));
        let (m, p) = req_of("pane.close", Some("p_1"), None).unwrap();
        assert_eq!((m, p), ("pane.close", json!({ "pane_id": "p_1" })));
        let (m, p) = req_of(
            "workspace.close",
            Some("ws_1"),
            Some(&json!({ "close_group": true })),
        )
        .unwrap();
        assert_eq!(
            (m, p),
            (
                "workspace.close",
                json!({ "workspace_id": "ws_1", "close_group": true })
            )
        );
    }

    #[test]
    fn action_request_rejects_unknown_actions_and_missing_ids() {
        assert!(req_of("server.stop", None, None)
            .unwrap_err()
            .contains("unknown action"));
        assert!(req_of("tab.close", None, None)
            .unwrap_err()
            .contains("needs an id"));
        assert!(req_of("pane.close", Some(""), None).is_err());
        // Options cannot smuggle arbitrary params.
        let (_, p) = req_of(
            "tab.create",
            Some("ws"),
            Some(&json!({ "workspace_id": "other", "focus": false })),
        )
        .unwrap();
        assert_eq!(p, json!({ "workspace_id": "ws", "focus": true }));
    }

    #[test]
    fn focus_method_dispatches_known_targets() {
        let focus = |target| focus_request(target, "x".into()).unwrap().herdr();
        assert_eq!(
            focus("workspace"),
            ("workspace.focus", json!({ "workspace_id": "x" }))
        );
        assert_eq!(focus("tab"), ("tab.focus", json!({ "tab_id": "x" })));
        assert_eq!(focus("pane"), ("pane.focus", json!({ "pane_id": "x" })));
    }

    #[test]
    fn subscription_set_adds_output_event_only_when_asked() {
        let base = subscription_set(false);
        assert!(!base.contains(&OUTPUT_SUBSCRIPTION));
        assert!(!base.contains(&"tab.updated"), "not a Herdr event");
        assert!(
            !base.contains(&"pane.agent_status_changed"),
            "per-pane only"
        );
        let full = subscription_set(true);
        assert_eq!(full.len(), base.len() + 1);
        assert_eq!(full.last(), Some(&OUTPUT_SUBSCRIPTION));
    }

    #[test]
    fn only_api_errors_count_as_subscription_rejection() {
        assert!(is_subscription_rejection(&herdr::HerdrError::Api {
            method: "events.subscribe".into(),
            message: "unknown variant `pane.output_changed`".into(),
        }));
        assert!(!is_subscription_rejection(&herdr::HerdrError::Io(
            std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "closed")
        )));
        assert!(!is_subscription_rejection(&herdr::HerdrError::Shape(
            "x".into()
        )));
    }

    #[test]
    fn focus_method_rejects_unknown_target() {
        let err = focus_request("window", "x".into()).unwrap_err();
        assert_eq!(err, "unknown focus target: window");
        assert!(focus_request("", "x".into()).is_err());
        assert!(focus_request("Pane", "x".into()).is_err());
    }
}
