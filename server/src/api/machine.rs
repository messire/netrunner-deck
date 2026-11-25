//! Work Deck asks this machine to do on its own disk, with its own `git`
//! (`server/crates/deck-accounts`): Deck never touches a machine's files
//! itself, not even its own.
//!
//! * `deck.git.clone.start {url, path}` → `{job, path}`,
//!   `deck.git.clone.status {job}` → `{state, phase, percent, message, path,
//!   auth?, github?}`, `deck.git.clone.cancel {job}`;
//! * `deck.github.status` → `{available, signed_in, user}`,
//!   `deck.github.login.start` → `{login_id}`, `deck.github.login.status
//!   {login_id}` → `{state, user_code, verification_uri, user, message}`,
//!   `deck.github.logout` (the official GitHub CLI, `deck_accounts::github`);
//! * `deck.transfer.export.start`, `.export.read`, `.import.start`,
//!   `.import.write`, `.import.commit`, `.conflicts`, `.remove.start`,
//!   `.status`, `.cancel`, `.finish` (`deck_accounts::transfer`).
//!
//! Answered on the API thread before the typed request is parsed, like
//! `deck.accounts.*`; everything long runs as a job on its own thread.

use std::sync::OnceLock;

use deck_accounts::github::GitHub;
use deck_accounts::jobs::Jobs;
use deck_accounts::transfer::{Places, Transfer};
use serde_json::{json, Value};

fn clones() -> &'static Jobs {
    static JOBS: OnceLock<Jobs> = OnceLock::new();
    JOBS.get_or_init(Jobs::default)
}

fn github() -> &'static GitHub {
    static GH: OnceLock<GitHub> = OnceLock::new();
    GH.get_or_init(|| GitHub::new(crate::config::config_dir().join("tools")))
}

fn transfer() -> &'static Transfer {
    static T: OnceLock<Transfer> = OnceLock::new();
    T.get_or_init(|| {
        let claude_home =
            deck_accounts::Paths::from_env(crate::config::config_dir().join("accounts"))
                .claude_home;
        let worktrees = crate::worktree::expand_tilde_absolute_path(
            &crate::config::Config::load().config.worktrees.directory,
        );
        Transfer::new(Places {
            work: crate::config::config_dir().join("transfer"),
            claude_home,
            worktrees,
            // On a server machine a move removes nothing outside the home.
            removable: if cfg!(unix) {
                deck_accounts::home_dir()
            } else {
                None
            },
        })
    })
}

fn text(params: &Value, key: &str) -> Result<String, String> {
    params
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("{key} is required"))
}

/// The reply line for a `deck.git.*`, `deck.github.*` or `deck.transfer.*`
/// request; `None` for any other line.
pub(crate) fn handle(line: &str) -> Option<String> {
    if !(line.contains("deck.git.")
        || line.contains("deck.github.")
        || line.contains("deck.transfer."))
    {
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
                deck_accounts::clone::start(clones(), &url, &path)
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
    } else if let Some(m) = method.strip_prefix("deck.transfer.") {
        let t = transfer();
        match m {
            "export.start" => t.export_start(&params),
            "export.read" => t.export_read(&params),
            "import.start" => t.import_start(&params),
            "import.write" => t.import_write(&params),
            "import.commit" => t.import_commit(&params),
            "conflicts" => t.conflicts(&params),
            "remove.start" => t.remove_start(&params),
            "status" => text(&params, "job").and_then(|j| t.status(&j)),
            "cancel" => text(&params, "job").and_then(|j| t.cancel(&j)),
            "finish" => text(&params, "job").and_then(|j| t.finish(&j)),
            other => Err(format!("unknown method deck.transfer.{other}")),
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
