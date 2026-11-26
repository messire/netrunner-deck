//! The work Deck asks a machine to do on its own disk, with its own `git`:
//! clone (`deck.git.clone.*`), GitHub sign-in (`deck.github.*`) and the two
//! ends of COPY TO / MOVE TO (`deck.transfer.*`), and `~/.claude/.env`
//! (`deck.secrets.*`).
//!
//! Answered by Deck's bridge on that machine (`deck_server/bridge.rs`: the
//! channel `deck:machine`), so it works on every machine Deck reaches —
//! one with deck-server, one with only Herdr, Deck's own — over the same
//! single connection. Herdr itself is never asked and never changed.
//! deck-server answers the same methods on its own API too.
//!
//! Its files live in Deck's data dir on that machine (`~/.config/deck-server`,
//! `%APPDATA%\deck-server` on Windows): `transfer/`, `tools/gh/`.

use std::path::PathBuf;
use std::sync::OnceLock;

use serde_json::{json, Value};

use crate::github::GitHub;
use crate::jobs::Jobs;
use crate::transfer::{Places, Transfer};

/// The bridge channel that is this module instead of a server socket.
pub const CHANNEL: &str = "deck:machine";

/// Deck's data dir on this machine (deck-server's config dir).
pub fn data_dir() -> PathBuf {
    app_config_dir("deck-server")
}

/// The config dir of an app on this machine, the way Herdr and deck-server
/// find theirs: `$XDG_CONFIG_HOME/<app>`, else `%APPDATA%\<app>` on
/// Windows, `~/.config/<app>` elsewhere.
pub fn app_config_dir(app: &str) -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME").filter(|d| !d.is_empty()) {
        return PathBuf::from(dir).join(app);
    }
    if cfg!(windows) {
        if let Some(dir) = std::env::var_os("APPDATA").filter(|d| !d.is_empty()) {
            return PathBuf::from(dir).join(app);
        }
    }
    crate::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config")
        .join(app)
}

/// `[worktrees] directory` of a Herdr-style `config.toml` (Herdr's and
/// deck-server's share the key); `None` when it is not set.
pub fn worktrees_setting(config_toml: &str) -> Option<String> {
    let mut in_section = false;
    for line in config_toml.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            in_section = l.trim_start_matches('[').trim_end_matches(']').trim() == "worktrees";
            continue;
        }
        if !in_section {
            continue;
        }
        let Some((key, value)) = l.split_once('=') else {
            continue;
        };
        if key.trim() != "directory" {
            continue;
        }
        let v = value.split('#').next().unwrap_or("").trim();
        let v = v.trim_matches('"').trim_matches('\'');
        return (!v.is_empty()).then(|| v.to_string());
    }
    None
}

/// Where `app` (`herdr` or `deck-server`) puts new worktrees on this
/// machine: its `[worktrees] directory`, default `~/.herdr/worktrees`.
pub fn worktrees_dir(app: &str) -> PathBuf {
    let set = std::fs::read_to_string(app_config_dir(app).join("config.toml"))
        .ok()
        .and_then(|t| worktrees_setting(&t))
        .unwrap_or_else(|| "~/.herdr/worktrees".into());
    let p = crate::expand_home(&set);
    if p.is_absolute() {
        p
    } else {
        crate::home_dir().unwrap_or_default().join(p)
    }
}

fn clones() -> &'static Jobs {
    static JOBS: OnceLock<Jobs> = OnceLock::new();
    JOBS.get_or_init(Jobs::default)
}

fn github() -> &'static GitHub {
    static GH: OnceLock<GitHub> = OnceLock::new();
    GH.get_or_init(|| GitHub::new(data_dir().join("tools")))
}

fn transfer() -> &'static Transfer {
    static T: OnceLock<Transfer> = OnceLock::new();
    T.get_or_init(|| {
        Transfer::new(Places {
            work: data_dir().join("transfer"),
            claude_home: crate::Paths::from_env(data_dir().join("accounts")).claude_home,
            worktrees: worktrees_dir("deck-server"),
            // On a server machine a move removes nothing outside the home.
            removable: if cfg!(unix) { crate::home_dir() } else { None },
        })
    })
}

fn text(params: &Value, key: &str) -> crate::Result<String> {
    params
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("{key} is required"))
}

/// Whether `method` is one of these.
pub fn is_machine_method(method: &str) -> bool {
    method.starts_with("deck.git.")
        || method.starts_with("deck.github.")
        || method.starts_with("deck.transfer.")
        || method.starts_with("deck.secrets.")
        || method.starts_with("deck.harness.")
        || method.starts_with("deck.uplink.")
        || method.starts_with("deck.archive.")
}

/// One request: the result, or the error's words.
pub fn call(method: &str, params: &Value) -> crate::Result<Value> {
    if let Some(m) = method.strip_prefix("deck.git.clone.") {
        return match m {
            "start" => crate::clone::start(clones(), &text(params, "url")?, &text(params, "path")?),
            "status" => crate::clone::status(clones(), &text(params, "job")?),
            "cancel" => crate::clone::cancel(clones(), &text(params, "job")?),
            other => Err(format!("unknown method deck.git.clone.{other}")),
        };
    }
    if let Some(m) = method.strip_prefix("deck.github.") {
        return match m {
            "status" => Ok(github().status()),
            "logout" => github().logout(),
            "login.start" => github().login_start(),
            "login.status" => github().login_status(&text(params, "login_id")?),
            other => Err(format!("unknown method deck.github.{other}")),
        };
    }
    if let Some(m) = method.strip_prefix("deck.transfer.") {
        let t = transfer();
        return match m {
            "export.start" => t.export_start(params),
            "export.read" => t.export_read(params),
            "import.start" => {
                // Worktrees go where the server that will show them puts its own.
                let app = params
                    .get("for")
                    .and_then(Value::as_str)
                    .unwrap_or("deck-server");
                let app = if app == "herdr" {
                    "herdr"
                } else {
                    "deck-server"
                };
                let mut p = params.clone();
                p["worktrees"] = json!(worktrees_dir(app).to_string_lossy());
                t.import_start(&p)
            }
            "import.write" => t.import_write(params),
            "import.commit" => t.import_commit(params),
            "conflicts" => t.conflicts(params),
            "remove.start" => t.remove_start(params),
            "status" => t.status(&text(params, "job")?),
            "cancel" => t.cancel(&text(params, "job")?),
            "finish" => t.finish(&text(params, "job")?),
            other => Err(format!("unknown method deck.transfer.{other}")),
        };
    }
    if let Some(m) = method.strip_prefix("deck.secrets.") {
        return crate::secrets::call(m, params);
    }
    if let Some(m) = method.strip_prefix("deck.harness.") {
        return crate::harness::call(m, params);
    }
    if let Some(m) = method.strip_prefix("deck.uplink.") {
        return crate::uplink::call(m, params);
    }
    if let Some(m) = method.strip_prefix("deck.archive.") {
        return crate::archive::call(m, params);
    }
    Err(format!("unknown method {method}"))
}

/// The reply line for one request line; `None` when it is not one of these.
pub fn handle(line: &str) -> Option<String> {
    let request: Value = serde_json::from_str(line.trim()).ok()?;
    let method = request.get("method")?.as_str()?.to_string();
    if !is_machine_method(&method) {
        return None;
    }
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let params = request.get("params").cloned().unwrap_or(Value::Null);
    Some(
        match call(&method, &params) {
            Ok(r) => json!({ "id": id, "result": r }),
            Err(m) => json!({ "id": id, "error": { "code": "machine_error", "message": m } }),
        }
        .to_string(),
    )
}

/// The reply line for any line on the `deck:machine` channel (an unknown
/// method is an error reply, never silence).
pub fn answer(line: &str) -> String {
    handle(line).unwrap_or_else(|| {
        let id = serde_json::from_str::<Value>(line.trim())
            .ok()
            .and_then(|v| v.get("id").cloned())
            .unwrap_or(Value::Null);
        json!({ "id": id, "error": { "code": "unknown_method", "message": "not a deck machine method" } }).to_string()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_worktrees_directory_is_read_from_the_config() {
        let toml = "[ui]\ndirectory = \"nope\"\n\n[worktrees]\n# where\ndirectory = \"~/Projects/wt\" # comment\n\n[other]\n";
        assert_eq!(worktrees_setting(toml).as_deref(), Some("~/Projects/wt"));
        assert_eq!(worktrees_setting("[worktrees]\n"), None);
        assert_eq!(worktrees_setting("[ui]\ndirectory = \"x\"\n"), None);
        assert_eq!(
            worktrees_setting("[worktrees]\ndirectory = 'C:\\wt'\n").as_deref(),
            Some("C:\\wt")
        );
    }

    #[test]
    fn only_machine_methods_are_answered() {
        assert!(handle(r#"{"id":"1","method":"pane.list","params":{}}"#).is_none());
        let r: Value = serde_json::from_str(&answer(r#"{"id":"2","method":"pane.list"}"#)).unwrap();
        assert_eq!(r["id"], json!("2"));
        assert_eq!(r["error"]["code"], json!("unknown_method"));
        let r: Value = serde_json::from_str(&answer(
            r#"{"id":"3","method":"deck.transfer.status","params":{"job":"none"}}"#,
        ))
        .unwrap();
        assert_eq!(r["error"]["code"], json!("machine_error"));
        assert!(is_machine_method("deck.github.status"));
        assert!(is_machine_method("deck.secrets.read"));
        assert!(!is_machine_method("deck.accounts.list"));
    }
}
