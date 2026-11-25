//! Work a server does on its own machine for Deck: clone a repository
//! (`deck.git.clone.*`) and sign in to GitHub (`deck.github.*`). Asked over
//! the server's one connection, like every other call; Deck never touches a
//! machine's disk or runs git itself.

use serde_json::Value;
use tauri::AppHandle;

/// The methods the webview may call.
pub const METHODS: &[&str] = &[
    "deck.git.clone.start",
    "deck.git.clone.status",
    "deck.git.clone.cancel",
    "deck.github.status",
    "deck.github.logout",
    "deck.github.login.start",
    "deck.github.login.status",
];

pub fn allowed(method: &str) -> Result<(), String> {
    if METHODS.contains(&method) {
        Ok(())
    } else {
        Err(format!("unknown machine method {method}"))
    }
}

/// `method` on `server`.
pub fn call(
    app: &AppHandle,
    server: Option<&str>,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    allowed(method)?;
    let rt = crate::servers::runtime_for(app, server)?;
    rt.machine_call(method, params).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_machine_methods_pass() {
        for m in METHODS {
            assert!(allowed(m).is_ok());
        }
        assert!(allowed("pane.send_text").is_err());
        assert!(allowed("deck.accounts.list").is_err());
    }
}
