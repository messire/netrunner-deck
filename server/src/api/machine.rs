//! Work Deck asks this machine to do on its own disk (clone, GitHub sign-in,
//! COPY TO / MOVE TO): `deck_accounts::machine`, the same code Deck's bridge
//! answers on its `deck:machine` channel. deck-server answers it on its API
//! too, for a Deck that asks the server directly, plus `deck.server.info`.
//!
//! Answered on the API thread before the typed request is parsed, like
//! `deck.accounts.*`; everything long runs as a job on its own thread.

use serde_json::{json, Value};

/// `deck.server.info` → `{version, commit, updates, os}`: this build, and
/// whether it keeps itself up to date (a local build without a commit does
/// not).
fn server_info() -> Value {
    let commit = option_env!("HERDR_BUILD_COMMIT")
        .map(str::trim)
        .filter(|c| !c.is_empty());
    json!({
        "version": env!("CARGO_PKG_VERSION"),
        "commit": commit,
        "updates": commit.is_some(),
        "os": std::env::consts::OS,
    })
}

/// The reply line for a `deck.git.*`, `deck.github.*`, `deck.transfer.*`,
/// `deck.secrets.*` or
/// `deck.server.info` request; `None` for any other line.
pub(crate) fn handle(line: &str) -> Option<String> {
    if line.contains("deck.server.info") {
        let request: Value = serde_json::from_str(line).ok()?;
        if request.get("method")?.as_str()? == "deck.server.info" {
            let id = request.get("id").cloned().unwrap_or(Value::Null);
            return Some(json!({ "id": id, "result": server_info() }).to_string());
        }
    }
    if !(line.contains("deck.git.")
        || line.contains("deck.github.")
        || line.contains("deck.transfer.")
        || line.contains("deck.secrets."))
    {
        return None;
    }
    deck_accounts::machine::handle(line)
}
