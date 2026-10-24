mod herdr;

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};

/// `ping` → `{version, protocol}`; also the "is Herdr running" probe.
#[tauri::command]
fn herdr_ping() -> herdr::Result<Value> {
    herdr::call("ping", json!({}))
}

/// Everything the sidebar needs in one round trip set: workspaces, tabs,
/// panes and agents straight from the server.
#[tauri::command]
fn herdr_tree() -> herdr::Result<Value> {
    let workspaces = herdr::call("workspace.list", json!({}))?;
    let panes = herdr::call("pane.list", json!({}))?;
    let agents = herdr::call("agent.list", json!({}))?;
    let mut tabs = Vec::new();
    if let Some(list) = workspaces.get("workspaces").and_then(Value::as_array) {
        for ws in list {
            if let Some(id) = ws.get("workspace_id").and_then(Value::as_str) {
                if let Ok(t) = herdr::call("tab.list", json!({ "workspace_id": id })) {
                    if let Some(items) = t.get("tabs").and_then(Value::as_array) {
                        tabs.extend(items.iter().cloned());
                    }
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
}

/// ANSI snapshot of a pane (recent scrollback + screen). Milestone 1 renders
/// this into xterm; milestone 2 replaces it with a streaming attach.
#[tauri::command]
fn herdr_pane_read(pane_id: String, lines: Option<u32>) -> herdr::Result<Value> {
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
}

#[tauri::command]
fn herdr_pane_send_text(pane_id: String, text: String) -> herdr::Result<Value> {
    herdr::call("pane.send_text", json!({ "pane_id": pane_id, "text": text }))
}

#[tauri::command]
fn herdr_pane_send_keys(pane_id: String, keys: Vec<String>) -> herdr::Result<Value> {
    herdr::call("pane.send_keys", json!({ "pane_id": pane_id, "keys": keys }))
}

#[tauri::command]
fn herdr_focus(target: String, id: String) -> herdr::Result<Value> {
    match target.as_str() {
        "workspace" => herdr::call("workspace.focus", json!({ "workspace_id": id })),
        "tab" => herdr::call("tab.focus", json!({ "tab_id": id })),
        _ => herdr::call("pane.focus", json!({ "pane_id": id })),
    }
}

/// `cswap list --json`, as the accounts section needs it.
#[tauri::command]
fn cswap_list() -> Result<Value, String> {
    run_json("cswap", &["list", "--json"])
}

#[tauri::command]
fn cswap_switch(number: u64) -> Result<Value, String> {
    run_json("cswap", &["switch", &number.to_string(), "--json"])
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
        if let Err(err) = result {
            tracing::warn!(%err, "herdr event stream ended; retrying");
            let _ = app.emit("herdr://offline", err.to_string());
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
