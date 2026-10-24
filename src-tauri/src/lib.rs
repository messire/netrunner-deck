mod herdr;

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
    blocking(move || {
        match target.as_str() {
            "workspace" => herdr::call("workspace.focus", json!({ "workspace_id": id })),
            "tab" => herdr::call("tab.focus", json!({ "tab_id": id })),
            _ => herdr::call("pane.focus", json!({ "pane_id": id })),
        }
    })
    .await
}

/// `cswap list --json`, as the accounts section needs it.
#[tauri::command]
async fn cswap_list() -> Result<Value, String> {
    blocking(move || {
        run_json("cswap", &["list", "--json"])
    })
    .await
}

#[tauri::command]
async fn cswap_switch(number: u64) -> Result<Value, String> {
    blocking(move || {
        run_json("cswap", &["switch", &number.to_string(), "--json"])
    })
    .await
}

fn run_json(program: &str, args: &[&str]) -> Result<Value, String> {
    let mut command = std::process::Command::new(program);
    command.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let output = command.output().map_err(|e| format!("{program}: {e}"))?;
    let text = String::from_utf8_lossy(&output.stdout);
    let start = text.find('{').unwrap_or(0);
    serde_json::from_str(&text[start..]).map_err(|e| format!("{program}: bad json: {e}"))
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
            &[
                "workspace.created",
                "workspace.updated",
                "workspace.closed",
                "tab.created",
                "tab.updated",
                "tab.closed",
                "pane.created",
                "pane.updated",
                "pane.closed",
                "pane.focused",
                "pane.agent_detected",
                "pane.agent_status_changed",
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
    tauri::Builder::default()
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
