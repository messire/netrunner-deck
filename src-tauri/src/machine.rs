//! Work a server does on its own machine for Deck: clone a repository
//! (`deck.git.clone.*`), sign in to GitHub (`deck.github.*`) and the two
//! ends of COPY TO / MOVE TO (`deck.transfer.*`). Asked over the server's
//! one connection, like every other call; Deck never touches a machine's
//! disk or runs git itself.
//!
//! A transfer's bytes go from the source server to the target server
//! through Deck: [`relay_start`] reads the source's pack in chunks and
//! writes each to the target as it came (base64 in and out, never
//! decoded, never written to Deck's disk), on its own thread so the
//! dialog may close.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};
use tauri::AppHandle;

use crate::archive::str_at;
use crate::runtime::{self, Runtime};

/// The methods the webview may call.
pub const METHODS: &[&str] = &[
    "deck.git.clone.start",
    "deck.git.clone.status",
    "deck.git.clone.cancel",
    "deck.github.status",
    "deck.github.logout",
    "deck.github.login.start",
    "deck.github.login.status",
    "deck.transfer.export.start",
    "deck.transfer.import.start",
    "deck.transfer.import.commit",
    "deck.transfer.conflicts",
    "deck.transfer.remove.start",
    "deck.transfer.status",
    "deck.transfer.cancel",
    "deck.transfer.finish",
];

/// One chunk of a relay (raw bytes; base64 on the wire).
pub const CHUNK: u64 = 512 * 1024;

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

/// Bytes a base64 text stands for.
pub fn decoded_len(b64: &str) -> u64 {
    let pad = b64.bytes().rev().take_while(|b| *b == b'=').count() as u64;
    (b64.len() as u64 / 4 * 3).saturating_sub(pad)
}

#[derive(Debug, Clone, Default)]
struct Relay {
    sent: u64,
    size: u64,
    done: bool,
    error: Option<String>,
}

fn relays() -> &'static Mutex<HashMap<String, Relay>> {
    static R: OnceLock<Mutex<HashMap<String, Relay>>> = OnceLock::new();
    R.get_or_init(Mutex::default)
}

fn set_relay(id: &str, f: impl FnOnce(&mut Relay)) {
    let mut m = relays().lock().unwrap_or_else(PoisonError::into_inner);
    f(m.entry(id.to_string()).or_default());
}

/// Copies an export into an import chunk by chunk: `read(offset)` gives
/// `{data, eof, size}` from the source, `write(offset, data)` hands the
/// same base64 text to the target.
pub fn relay(
    read: &mut dyn FnMut(u64) -> Result<Value, String>,
    write: &mut dyn FnMut(u64, &str) -> Result<(), String>,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<u64, String> {
    let mut offset = 0u64;
    loop {
        let chunk = read(offset).map_err(|e| format!("reading from the source: {e}"))?;
        let data = chunk
            .get("data")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let size = chunk.get("size").and_then(Value::as_u64).unwrap_or(0);
        let n = decoded_len(data);
        if n > 0 {
            write(offset, data).map_err(|e| format!("writing to the target: {e}"))?;
        }
        offset += n;
        progress(offset, size);
        if chunk.get("eof").and_then(Value::as_bool).unwrap_or(n == 0) {
            return Ok(offset);
        }
        if n == 0 {
            return Err("the source sent an empty chunk before the end".into());
        }
    }
}

/// Starts the relay on its own thread; `{relay}`, asked with [`relay_status`].
pub fn relay_start(
    app: &AppHandle,
    from: Option<&str>,
    to: Option<&str>,
    export_job: String,
    import_job: String,
) -> Result<Value, String> {
    let src = crate::servers::runtime_for(app, from)?;
    let dst = crate::servers::runtime_for(app, to)?;
    let id = format!("relay-{export_job}");
    set_relay(&id, |r| *r = Relay::default());
    let rid = id.clone();
    std::thread::Builder::new()
        .name("deck-transfer-relay".into())
        .spawn(move || {
            let r = relay(
                &mut |offset| {
                    src.machine_call(
                        "deck.transfer.export.read",
                        json!({ "job": export_job, "offset": offset, "length": CHUNK }),
                    )
                    .map_err(|e| e.to_string())
                },
                &mut |offset, data| {
                    dst.machine_call(
                        "deck.transfer.import.write",
                        json!({ "job": import_job, "offset": offset, "data": data }),
                    )
                    .map(|_| ())
                    .map_err(|e| e.to_string())
                },
                &mut |sent, size| {
                    set_relay(&rid, |r| {
                        r.sent = sent;
                        r.size = size;
                    })
                },
            );
            set_relay(&rid, |s| match r {
                Ok(_) => s.done = true,
                Err(e) => {
                    s.done = true;
                    s.error = Some(e);
                }
            });
        })
        .map_err(|e| format!("cannot start the relay: {e}"))?;
    Ok(json!({ "relay": id }))
}

/// `{sent, size, done, error}` of a relay.
pub fn relay_status(id: &str) -> Result<Value, String> {
    let m = relays().lock().unwrap_or_else(PoisonError::into_inner);
    let r = m.get(id).ok_or_else(|| format!("no relay {id}"))?;
    Ok(json!({ "sent": r.sent, "size": r.size, "done": r.done, "error": r.error }))
}

#[derive(Debug, Clone, Deserialize)]
pub struct Placed {
    pub path: String,
    #[serde(default)]
    pub root: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Arrived {
    pub id: String,
    pub cwd: String,
}

fn norm(p: &str) -> String {
    p.trim_start_matches(r"\\?\")
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_lowercase()
}

/// The worktree a session belongs to: the longest one its cwd is inside,
/// else the main checkout.
pub fn home_of<'a>(worktrees: &'a [Placed], cwd: &str) -> Option<&'a Placed> {
    let c = norm(cwd);
    worktrees
        .iter()
        .filter(|w| {
            let w = norm(&w.path);
            c == w || c.starts_with(&format!("{w}/"))
        })
        .max_by_key(|w| w.path.len())
        .or_else(|| worktrees.iter().find(|w| w.root))
        .or_else(|| worktrees.first())
}

/// After an import: every worktree that arrived opens as a space on the
/// target (the main checkout first, so the others join its group), and
/// each session resumes in a tab of its worktree.
pub fn open_arrived(
    rt: &dyn Runtime,
    worktrees: &[Placed],
    sessions: &[Arrived],
) -> Result<Vec<(String, String)>, String> {
    let root = worktrees.iter().find(|w| w.root).map(|w| w.path.clone());
    let mut order: Vec<&Placed> = worktrees.iter().filter(|w| w.root).collect();
    order.extend(worktrees.iter().filter(|w| !w.root));
    let mut resume = Vec::new();
    for wt in order {
        let mine: Vec<&Arrived> = sessions
            .iter()
            .filter(|s| {
                home_of(worktrees, &s.cwd)
                    .map(|h| h.path == wt.path)
                    .unwrap_or(false)
            })
            .collect();
        let (_, first) =
            crate::quickhacks::open_session_dir(rt, &wt.path, root.as_deref(), None, false)?;
        let mut free = Some(first);
        for s in mine {
            let pane = match free.take() {
                Some(p) => p,
                None => {
                    let ws = crate::quickhacks::open_workspace_for(rt, &wt.path)
                        .ok_or("the new space is gone")?;
                    let tab = rt
                        .tab_create(runtime::TabCreate {
                            workspace_id: ws,
                            cwd: Some(wt.path.clone()),
                            label: None,
                            focus: false,
                        })
                        .map_err(|e| e.to_string())?;
                    match str_at(&tab, &["root_pane", "pane_id"]) {
                        Some(p) => p.to_string(),
                        None => continue,
                    }
                }
            };
            resume.push((pane, s.id.clone()));
        }
    }
    Ok(resume)
}

/// [`open_arrived`] on `server`; sessions resume in the background once
/// their pane's agent is up.
pub fn open_on(
    app: &AppHandle,
    server: Option<&str>,
    worktrees: Vec<Placed>,
    sessions: Vec<Arrived>,
) -> Result<Value, String> {
    let rt = crate::servers::runtime_for(app, server)?;
    let resume = open_arrived(&*rt, &worktrees, &sessions)?;
    let n = resume.len();
    std::thread::spawn(move || {
        for (pane, sid) in resume {
            if let Err(e) = crate::quickhacks::agent_call_when_ready(
                &*rt,
                &pane,
                "resume",
                json!({ "pane_id": pane, "session_id": sid }),
                Duration::from_secs(30),
            ) {
                tracing::warn!(%e, "transferred session not resumed");
            }
        }
    });
    Ok(json!({ "resumed": n }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_machine_methods_pass() {
        for m in METHODS {
            assert!(allowed(m).is_ok());
        }
        assert!(
            allowed("deck.transfer.export.read").is_err(),
            "the relay reads, never the webview"
        );
        assert!(allowed("pane.send_text").is_err());
        assert!(allowed("deck.accounts.list").is_err());
    }

    #[test]
    fn base64_lengths() {
        assert_eq!(decoded_len(""), 0);
        assert_eq!(decoded_len("YQ=="), 1);
        assert_eq!(decoded_len("YWI="), 2);
        assert_eq!(decoded_len("YWJj"), 3);
    }

    #[test]
    fn sessions_find_their_worktree() {
        let w = vec![
            Placed {
                path: "/home/me/atlas".into(),
                root: true,
            },
            Placed {
                path: "/home/me/.herdr/worktrees/atlas/feat".into(),
                root: false,
            },
        ];
        assert_eq!(
            home_of(&w, "/home/me/atlas/src").unwrap().path,
            "/home/me/atlas"
        );
        assert_eq!(
            home_of(&w, "/home/me/.herdr/worktrees/atlas/feat")
                .unwrap()
                .path,
            w[1].path
        );
        assert_eq!(
            home_of(&w, "/somewhere/else").unwrap().path,
            "/home/me/atlas"
        );
        assert_eq!(
            home_of(&w, "/home/me/atlas2").unwrap().path,
            "/home/me/atlas"
        );
    }

    /// base64 of `n` zero bytes.
    fn zeros(n: u64) -> String {
        let mut s = "A".repeat((n / 3 * 4) as usize);
        match n % 3 {
            1 => s.push_str("AA=="),
            2 => s.push_str("AAA="),
            _ => {}
        }
        s
    }

    #[test]
    fn a_relay_passes_every_chunk_in_order() {
        let size = CHUNK * 2 + 10;
        let mut writes: Vec<(u64, u64)> = Vec::new();
        let mut seen = Vec::new();
        let n = relay(
            &mut |off| {
                let end = (off + CHUNK).min(size);
                Ok(json!({ "data": zeros(end - off), "eof": end >= size, "size": size }))
            },
            &mut |off, data| {
                writes.push((off, decoded_len(data)));
                Ok(())
            },
            &mut |sent, total| seen.push((sent, total)),
        )
        .unwrap();
        assert_eq!(n, size);
        assert_eq!(writes, vec![(0, CHUNK), (CHUNK, CHUNK), (CHUNK * 2, 10)]);
        assert_eq!(seen.last(), Some(&(size, size)));
        // A failing target stops the relay with its words.
        let err = relay(
            &mut |_| Ok(json!({ "data": zeros(3), "eof": false, "size": 9 })),
            &mut |_, _| Err("disk full".into()),
            &mut |_, _| {},
        )
        .unwrap_err();
        assert!(err.contains("disk full"));
    }
}
