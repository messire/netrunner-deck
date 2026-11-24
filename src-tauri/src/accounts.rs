//! Claude Code accounts per server machine (`deck.accounts.*` of
//! deck-server, `server/src/api/accounts.rs`): the ACCOUNTS panel, the
//! account badge and the chat's sign-in card ask the server that owns the
//! session, over the same connection as everything else. Deck itself
//! never touches a machine's credentials.

use serde_json::Value;
use tauri::AppHandle;

/// The methods the webview may call.
pub const METHODS: &[&str] = &["list", "switch", "remove", "login.start", "login.finish"];

pub fn allowed(method: &str) -> Result<(), String> {
    if METHODS.contains(&method) {
        Ok(())
    } else {
        Err(format!("unknown accounts method {method}"))
    }
}

/// `deck.accounts.<method>` on `server`.
pub fn call(
    app: &AppHandle,
    server: Option<&str>,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    allowed(method)?;
    let rt = crate::servers::runtime_for(app, server)?;
    rt.accounts_call(method, params).map_err(|e| e.to_string())
}

/// `deck.repos.list` on `server`: the folders Claude Code knows on that
/// server's machine (OPEN REPOSITORY offers them for a server on another
/// machine).
pub fn repos(app: &AppHandle, server: Option<&str>) -> Result<Value, String> {
    let rt = crate::servers::runtime_for(app, server)?;
    rt.repos_list().map_err(|e| e.to_string())
}

/// `deck.fs.stat` / `deck.fs.mkdir {path}` on `server`'s machine.
pub fn fs(
    app: &AppHandle,
    server: Option<&str>,
    method: &str,
    path: &str,
) -> Result<Value, String> {
    if !matches!(method, "stat" | "mkdir") {
        return Err(format!("unknown fs method {method}"));
    }
    let rt = crate::servers::runtime_for(app, server)?;
    rt.fs_call(method, serde_json::json!({ "path": path }))
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_account_methods_pass() {
        for m in METHODS {
            assert!(allowed(m).is_ok());
        }
        assert!(allowed("send").is_err());
        assert!(allowed("login").is_err());
    }
}
