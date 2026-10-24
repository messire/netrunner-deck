mod herdr;

use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};

// Every command below blocks on pipe I/O or a child process. An async command
// keeps that off the main (UI) thread, but tokio workers are few, so the blocking
// body is handed to `spawn_blocking` via `blocking()`. Per-call deadline and
// connection slots live in `herdr::call` (R3).

/// Run a blocking Herdr/cswap call on tokio's blocking pool.
async fn blocking<T, E>(work: impl FnOnce() -> Result<T, E> + Send + 'static) -> Result<T, E>
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
async fn herdr_ping() -> herdr::Result<Value> {
    blocking(move || {
        herdr::call("ping", json!({}))
    })
    .await
}

/// Everything the sidebar needs in one round trip set: workspaces, tabs,
/// panes and agents straight from the server.
#[tauri::command]
async fn herdr_tree() -> herdr::Result<Value> {
    blocking(move || {
        let workspaces = herdr::call("workspace.list", json!({}))?;
        let panes = herdr::call("pane.list", json!({}))?;
        let agents = herdr::call("agent.list", json!({}))?;
        let mut tabs = Vec::new();
        if let Some(list) = workspaces.get("workspaces").and_then(Value::as_array) {
            for ws in list {
                if let Some(id) = ws.get("workspace_id").and_then(Value::as_str) {
                    // A failing tab.list is an error, not an empty tab bar (audit #15).
                    let t = herdr::call("tab.list", json!({ "workspace_id": id }))?;
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
async fn herdr_pane_read(pane_id: String, lines: Option<u32>) -> herdr::Result<Value> {
    blocking(move || {
        herdr::call(
            "pane.read",
            json!({
                "pane_id": pane_id,
                "source": "recent",
                "lines": lines.unwrap_or(400),
                "format": "ansi",
                "strip_ansi": false
            }),
        )
    })
    .await
}

#[tauri::command]
async fn herdr_pane_send_text(pane_id: String, text: String) -> herdr::Result<Value> {
    blocking(move || {
        herdr::call("pane.send_text", json!({ "pane_id": pane_id, "text": text }))
    })
    .await
}

#[tauri::command]
async fn herdr_pane_send_keys(pane_id: String, keys: Vec<String>) -> herdr::Result<Value> {
    blocking(move || {
        herdr::call("pane.send_keys", json!({ "pane_id": pane_id, "keys": keys }))
    })
    .await
}

#[tauri::command]
async fn herdr_focus(target: String, id: String) -> herdr::Result<Value> {
    // An unknown target is a caller bug, not a pane (audit: optional `herdr_focus` note).
    let (method, id_field) = focus_method(&target).map_err(herdr::HerdrError::Shape)?;
    blocking(move || {
        let mut params = serde_json::Map::new();
        params.insert(id_field.to_string(), Value::String(id));
        herdr::call(method, Value::Object(params))
    })
    .await
}

/// Pick the Herdr method and id field for a `herdr_focus` target.
fn focus_method(target: &str) -> Result<(&'static str, &'static str), String> {
    match target {
        "workspace" => Ok(("workspace.focus", "workspace_id")),
        "tab" => Ok(("tab.focus", "tab_id")),
        "pane" => Ok(("pane.focus", "pane_id")),
        _ => Err(format!("unknown focus target: {target}")),
    }
}

/// Absolute path of the `cswap` executable, resolved once at startup (audit #18).
/// `None` means neither `PATH` nor `~/.local/bin` had it; the commands report that.
pub struct CswapPath(pub Option<PathBuf>);

const CSWAP_NOT_FOUND: &str = "cswap not found (searched PATH and ~/.local/bin)";

/// File names to try for `cswap` in each directory, in priority order.
fn cswap_candidates() -> Vec<String> {
    #[cfg(windows)]
    {
        windows_candidates(std::env::var("PATHEXT").ok().as_deref())
    }
    #[cfg(not(windows))]
    {
        vec!["cswap".to_string()]
    }
}

/// Windows names: every `PATHEXT` extension in order (when set), then
/// `cswap.exe`, `cswap.cmd`, `cswap` for anything the variable did not cover.
#[cfg_attr(not(windows), allow(dead_code))]
fn windows_candidates(pathext: Option<&str>) -> Vec<String> {
    let mut names: Vec<String> = pathext
        .map(|exts| {
            exts.split(';')
                .filter(|ext| !ext.is_empty())
                .map(|ext| format!("cswap{}", ext.to_ascii_lowercase()))
                .collect()
        })
        .unwrap_or_default();
    for fixed in ["cswap.exe", "cswap.cmd", "cswap"] {
        if !names.iter().any(|n| n == fixed) {
            names.push(fixed.to_string());
        }
    }
    names
}

/// `~` for the fallback lookup: `USERPROFILE` on Windows, `HOME` elsewhere.
fn home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    let key = "USERPROFILE";
    #[cfg(not(windows))]
    let key = "HOME";
    std::env::var_os(key)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// Search `PATH` entries, then `~/.local/bin`, for a `cswap` file.
fn resolve_cswap() -> Option<PathBuf> {
    let names = cswap_candidates();
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    if let Some(home) = home_dir() {
        dirs.push(home.join(".local").join("bin"));
    }
    dirs.into_iter()
        .filter(|dir| !dir.as_os_str().is_empty())
        .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
        .find(|candidate| candidate.is_file())
}

/// `cswap list --json`, as the accounts section needs it.
#[tauri::command]
async fn cswap_list(cswap: tauri::State<'_, CswapPath>) -> Result<Value, String> {
    // State is borrowed from the app; copy the path out before leaving the thread.
    let program = cswap.0.clone().ok_or_else(|| CSWAP_NOT_FOUND.to_string())?;
    blocking(move || run_json(&program, &["list", "--json"])).await
}

#[tauri::command]
async fn cswap_switch(cswap: tauri::State<'_, CswapPath>, number: u64) -> Result<Value, String> {
    let program = cswap.0.clone().ok_or_else(|| CSWAP_NOT_FOUND.to_string())?;
    blocking(move || run_json(&program, &["switch", &number.to_string(), "--json"])).await
}

/// Run `program args` and parse the JSON object it prints to stdout.
///
/// A non-zero exit is an error carrying trimmed stderr; the JSON is taken from
/// the first line that starts with `{`, so warnings printed before it (even
/// ones containing a brace) do not confuse the parser.
fn run_json(program: &Path, args: &[&str]) -> Result<Value, String> {
    let name = program
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| program.display().to_string());
    let mut command = std::process::Command::new(program);
    command.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let output = command
        .output()
        .map_err(|e| format!("{name} ({}): {e}", program.display()))?;
    if !output.status.success() {
        let code = output
            .status
            .code()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "signal".to_string());
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "{name} exited with {code}: {}",
            truncate_chars(stderr.trim(), 300)
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_json_stdout(&stdout).map_err(|e| format!("{name}: {e}"))
}

/// Parse the JSON object from a process's stdout: the first line starting
/// with `{` (after trimming leading whitespace) through the end of the text.
fn parse_json_stdout(stdout: &str) -> Result<Value, String> {
    let mut offset = 0usize;
    let mut start = None;
    for line in stdout.split_inclusive('\n') {
        if line.trim_start().starts_with('{') {
            start = Some(offset);
            break;
        }
        offset += line.len();
    }
    let Some(start) = start else {
        return Err(format!(
            "no json object in output: {:?}",
            truncate_chars(stdout.trim(), 200)
        ));
    };
    serde_json::from_str(stdout[start..].trim_start()).map_err(|e| {
        format!(
            "bad json: {e}; output starts with {:?}",
            truncate_chars(stdout.trim(), 200)
        )
    })
}

/// First `max` chars of `text` (char-safe), with `...` when cut.
fn truncate_chars(text: &str, max: usize) -> String {
    let mut chars = text.char_indices();
    match chars.nth(max) {
        Some((idx, _)) => format!("{}...", &text[..idx]),
        None => text.to_string(),
    }
}

/// Forward Herdr events to the webview as `herdr://event` until the window dies.
///
/// `herdr::subscribe` only returns `Ok` when the webview stopped accepting
/// events; every other exit (refused subscription, closed stream, transport
/// error) is an `Err` and is surfaced as `herdr://offline` with the error
/// text, then retried after a short pause.
fn start_event_pump(app: AppHandle) {
    std::thread::spawn(move || loop {
        let handle = app.clone();
        let result = herdr::subscribe(
            // Names must match Herdr's `Subscription` enum exactly; an unknown
            // one makes the server reject the whole subscription (R4 surfaces it).
            &[
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
            ],
            |event| handle.emit("herdr://event", event).is_ok(),
        );
        match result {
            Ok(()) => {
                tracing::info!("herdr event consumer stopped accepting events; retrying");
            }
            Err(err) => {
                tracing::warn!(%err, "herdr event stream ended; reconnecting");
                let _ = app.emit("herdr://offline", err.to_string());
            }
        }
        std::thread::sleep(std::time::Duration::from_secs(2));
    });
}

pub fn run() {
    // `RUST_LOG` overrides; default to info so event-pump warnings are visible.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    let cswap = resolve_cswap();
    match &cswap {
        Some(path) => tracing::info!(path = %path.display(), "cswap resolved"),
        None => tracing::info!("cswap not found in PATH or ~/.local/bin; accounts disabled"),
    }
    tauri::Builder::default()
        .manage(CswapPath(cswap))
        .setup(|app| {
            start_event_pump(app.handle().clone());
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
            cswap_list,
            cswap_switch,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Netrunner Deck");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_json_stdout_accepts_plain_object() {
        let value = parse_json_stdout("{\"accounts\": []}\n").unwrap();
        assert_eq!(value, json!({ "accounts": [] }));
    }

    #[test]
    fn parse_json_stdout_skips_warning_line_before_json() {
        let out = "warning: config is stale\n{\"ok\": true}\n";
        assert_eq!(parse_json_stdout(out).unwrap(), json!({ "ok": true }));
    }

    #[test]
    fn parse_json_stdout_ignores_brace_inside_warning() {
        // The old `find('{')` would have started here and failed on `profile}`.
        let out = "warning: missing key {profile} in config\n  {\"ok\": 1}\n";
        assert_eq!(parse_json_stdout(out).unwrap(), json!({ "ok": 1 }));
    }

    #[test]
    fn parse_json_stdout_handles_crlf_and_multiline_json() {
        let out = "note\r\n{\r\n  \"a\": [1, 2]\r\n}\r\n";
        assert_eq!(parse_json_stdout(out).unwrap(), json!({ "a": [1, 2] }));
    }

    #[test]
    fn parse_json_stdout_rejects_empty_output() {
        let err = parse_json_stdout("").unwrap_err();
        assert!(err.starts_with("no json object in output"), "got {err}");
        let err = parse_json_stdout("just text\n").unwrap_err();
        assert!(err.contains("just text"), "got {err}");
    }

    #[test]
    fn parse_json_stdout_reports_bad_json_with_prefix() {
        let err = parse_json_stdout("{not json").unwrap_err();
        assert!(err.starts_with("bad json:"), "got {err}");
        assert!(err.contains("{not json"), "got {err}");
    }

    #[test]
    fn truncate_chars_is_char_safe() {
        assert_eq!(truncate_chars("abc", 5), "abc");
        assert_eq!(truncate_chars("abcdef", 3), "abc...");
        assert_eq!(truncate_chars("ééé", 2), "éé...");
        let long = "x".repeat(500);
        assert_eq!(truncate_chars(&long, 300).chars().count(), 303);
    }

    #[test]
    fn focus_method_dispatches_known_targets() {
        assert_eq!(
            focus_method("workspace").unwrap(),
            ("workspace.focus", "workspace_id")
        );
        assert_eq!(focus_method("tab").unwrap(), ("tab.focus", "tab_id"));
        assert_eq!(focus_method("pane").unwrap(), ("pane.focus", "pane_id"));
    }

    #[test]
    fn focus_method_rejects_unknown_target() {
        let err = focus_method("window").unwrap_err();
        assert_eq!(err, "unknown focus target: window");
        assert!(focus_method("").is_err());
        assert!(focus_method("Pane").is_err());
    }

    #[test]
    fn windows_candidates_follow_pathext_then_fixed_list() {
        assert_eq!(
            windows_candidates(None),
            vec!["cswap.exe", "cswap.cmd", "cswap"]
        );
        assert_eq!(
            windows_candidates(Some(".COM;.EXE;.BAT;.CMD")),
            vec!["cswap.com", "cswap.exe", "cswap.bat", "cswap.cmd", "cswap"]
        );
        assert_eq!(
            windows_candidates(Some("")),
            vec!["cswap.exe", "cswap.cmd", "cswap"]
        );
    }

    #[test]
    fn cswap_candidates_always_include_bare_name() {
        let names = cswap_candidates();
        assert!(names.iter().any(|n| n == "cswap"), "got {names:?}");
        #[cfg(not(windows))]
        assert_eq!(names, vec!["cswap".to_string()]);
    }
}
