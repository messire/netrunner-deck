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
