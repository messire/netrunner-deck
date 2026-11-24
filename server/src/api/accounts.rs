//! `deck.accounts.*`: the Claude Code accounts of this server's machine
//! (`server/crates/deck-accounts`), for Deck's ACCOUNTS panel, its account
//! badge and the sign-in card of a chat.
//!
//! Answered on the API thread before the typed request is parsed, like
//! `deck.agent.*`, so Herdr's method set and schema stay as they are.
//!
//! * `deck.accounts.list` → `{schemaVersion, managed, signedIn,
//!   activeAccountNumber, accounts: [{number, email, organizationName,
//!   active, usageStatus, usage, lastGoodUsage}]}` (the shape of
//!   `cswap list --json`);
//! * `deck.accounts.switch {number}` → the list;
//! * `deck.accounts.remove {number}` → the list;
//! * `deck.accounts.login.start` → `{login_id, url}`;
//! * `deck.accounts.login.finish {login_id, code, activate?}` → the list
//!   with `added` (the account's number);
//! * `deck.repos.list` → `{folders: [{name, root, git}]}`: the folders
//!   Claude Code knows on this machine, for Deck's OPEN REPOSITORY.
//!
//! A switch or a sign-in that changed the machine's login asks every
//! headless agent to start `claude` again at its next idle moment, so it
//! runs under the new account.

use std::sync::OnceLock;

use deck_accounts::{Accounts, Curl};
use serde_json::{json, Value};

const PREFIX: &str = "deck.accounts.";

fn accounts() -> &'static Accounts<Curl> {
    static ACCOUNTS: OnceLock<Accounts<Curl>> = OnceLock::new();
    ACCOUNTS.get_or_init(|| {
        let store = crate::config::config_dir().join("accounts");
        Accounts::new(deck_accounts::Paths::from_env(store), Curl::default())
    })
}

/// The reply line for a `deck.accounts.*` or `deck.repos.list` request;
/// `None` for any other line.
pub(crate) fn handle(line: &str) -> Option<String> {
    if line.contains("deck.repos.list") {
        let request: Value = serde_json::from_str(line).ok()?;
        if request.get("method")?.as_str()? == "deck.repos.list" {
            let id = request.get("id").cloned().unwrap_or(Value::Null);
            let folders = deck_accounts::known_folders(accounts().paths());
            return Some(json!({ "id": id, "result": { "folders": folders } }).to_string());
        }
    }
    if !line.contains(PREFIX) {
        return None;
    }
    let request: Value = serde_json::from_str(line).ok()?;
    let method = request.get("method")?.as_str()?.strip_prefix(PREFIX)?;
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let params = request.get("params").cloned().unwrap_or(Value::Null);
    let number = || {
        params
            .get("number")
            .and_then(Value::as_u64)
            .map(|n| n as u32)
            .ok_or_else(|| ("invalid_params", "number is required".to_string()))
    };
    let text = |key: &str| {
        params
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| ("invalid_params", format!("{key} is required")))
    };
    let failed = |e: String| ("accounts_error", e);
    let result: Result<Value, (&str, String)> = match method {
        "list" => accounts().list().map_err(failed),
        "switch" => number().and_then(|n| {
            accounts()
                .switch(n)
                .map(restart_agents_if_changed)
                .map_err(failed)
        }),
        "remove" => number().and_then(|n| accounts().remove(n).map_err(failed)),
        "login.start" => accounts().login_start().map_err(failed),
        "login.finish" => text("login_id").and_then(|login| {
            let code = text("code")?;
            let activate = params
                .get("activate")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            accounts()
                .login_finish(&login, &code, activate)
                .map(restart_agents_if_changed)
                .map_err(failed)
        }),
        other => Err(("unknown_method", format!("unknown method {PREFIX}{other}"))),
    };
    Some(
        match result {
            Ok(result) => json!({ "id": id, "result": result }),
            Err((code, message)) => {
                json!({ "id": id, "error": { "code": code, "message": message } })
            }
        }
        .to_string(),
    )
}

fn restart_agents_if_changed(changed: deck_accounts::Changed) -> Value {
    if changed.live_changed {
        crate::api::agent::restart_all();
    }
    changed.result
}
