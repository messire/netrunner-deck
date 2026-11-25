//! Work Deck asks this machine to do on its own disk, with its own `git`
//! (`server/crates/deck-accounts`): Deck never touches a machine's files
//! itself, not even its own.
//!
//! * `deck.git.clone.start {url, path}` → `{job, path}`,
//!   `deck.git.clone.status {job}` → `{state, phase, percent, message, path,
//!   auth?, github?}`, `deck.git.clone.cancel {job}`;
//! * `deck.github.status` → `{configured, signed_in, user}`,
//!   `deck.github.login.start` → `{login_id, user_code, verification_uri,
//!   expires_in}`, `deck.github.login.status {login_id}` → `{state, user,
//!   message}`, `deck.github.logout`;
//!
//! Answered on the API thread before the typed request is parsed, like
//! `deck.accounts.*`; everything long runs as a job on its own thread.

use std::sync::OnceLock;

use deck_accounts::github::GitHub;
use deck_accounts::jobs::Jobs;
use deck_accounts::Curl;
use serde_json::{json, Value};

fn clones() -> &'static Jobs {
    static JOBS: OnceLock<Jobs> = OnceLock::new();
    JOBS.get_or_init(Jobs::default)
}

fn github() -> &'static GitHub<Curl> {
    static GH: OnceLock<GitHub<Curl>> = OnceLock::new();
    GH.get_or_init(|| GitHub::new(crate::config::config_dir().join("github"), Curl::default()))
}

fn text(params: &Value, key: &str) -> Result<String, String> {
    params
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("{key} is required"))
}

/// The reply line for a `deck.git.*` or `deck.github.*` request; `None` for any other line.
pub(crate) fn handle(line: &str) -> Option<String> {
    if !(line.contains("deck.git.") || line.contains("deck.github.")) {
        return None;
    }
    let request: Value = serde_json::from_str(line).ok()?;
    let method = request.get("method")?.as_str()?.to_string();
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let params = request.get("params").cloned().unwrap_or(Value::Null);
    let result: Result<Value, String> = if let Some(m) = method.strip_prefix("deck.git.clone.") {
        match m {
            "start" => text(&params, "url").and_then(|url| {
                let path = text(&params, "path")?;
                deck_accounts::clone::start(clones(), &url, &path, || github().token())
            }),
            "status" => {
                text(&params, "job").and_then(|j| deck_accounts::clone::status(clones(), &j))
            }
            "cancel" => {
                text(&params, "job").and_then(|j| deck_accounts::clone::cancel(clones(), &j))
            }
            other => Err(format!("unknown method deck.git.clone.{other}")),
        }
    } else if let Some(m) = method.strip_prefix("deck.github.") {
        match m {
            "status" => Ok(github().status()),
            "logout" => github().logout(),
            "login.start" => github().login_start(),
            "login.status" => text(&params, "login_id").and_then(|l| github().login_status(&l)),
            other => Err(format!("unknown method deck.github.{other}")),
        }
    } else {
        return None;
    };
    Some(
        match result {
            Ok(r) => json!({ "id": id, "result": r }),
            Err(m) => json!({ "id": id, "error": { "code": "machine_error", "message": m } }),
        }
        .to_string(),
    )
}
