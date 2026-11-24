//! Headless Claude sessions of deck-server (`deck.agent.*`,
//! docs/adr/0003-headless-claude.md): one stream per pane the chat shows.
//!
//! [`watch`] keeps a `deck.agent.subscribe` open for a pane on its server
//! and hands every line to the webview as `deck://agent`
//! (`{server, pane_id, line}`); `agent.ts` folds them. When the stream
//! breaks it reports `deck.agent.link` and resubscribes from the last event
//! it saw (same epoch, `after_seq`), so nothing is shown twice or lost.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};

/// Open watches by `server|pane`, each with its stop flag.
#[derive(Default)]
pub struct Watches(Mutex<HashMap<String, Arc<AtomicBool>>>);

fn key(server: Option<&str>, pane_id: &str) -> String {
    format!("{}|{pane_id}", server.unwrap_or(""))
}

/// Where the stream resumes after a break.
#[derive(Default)]
struct Cursor {
    epoch: Option<String>,
    seq: Option<u64>,
}

impl Cursor {
    fn see(&mut self, line: &Value) {
        if let Some(epoch) = line.pointer("/result/epoch").and_then(Value::as_str) {
            if self.epoch.as_deref() != Some(epoch) {
                self.epoch = Some(epoch.to_string());
                self.seq = None;
            }
        }
        match line.get("event").and_then(Value::as_str) {
            Some("deck.agent.event") => {
                let data = &line["data"];
                if let Some(epoch) = data.get("epoch").and_then(Value::as_str) {
                    if self.epoch.as_deref() != Some(epoch) {
                        self.epoch = Some(epoch.to_string());
                        self.seq = None;
                    }
                }
                if let Some(seq) = data.get("seq").and_then(Value::as_u64) {
                    self.seq = Some(self.seq.map_or(seq, |s| s.max(seq)));
                }
            }
            Some("deck.agent.reset") => {
                self.epoch = line.pointer("/data/epoch").and_then(Value::as_str).map(str::to_string);
                self.seq = None;
            }
            _ => {}
        }
    }

    fn params(&self, pane_id: &str) -> Value {
        json!({ "pane_id": pane_id, "epoch": self.epoch, "after_seq": self.seq })
    }
}

/// Start streaming `pane_id` of `server` to the webview (no-op when it is
/// already streamed).
pub fn watch(app: &AppHandle, server: Option<String>, pane_id: String) {
    let stop = {
        let watches = app.state::<Watches>();
        let mut map = watches.0.lock().unwrap_or_else(|e| e.into_inner());
        let k = key(server.as_deref(), &pane_id);
        if map.contains_key(&k) {
            return;
        }
        let stop = Arc::new(AtomicBool::new(false));
        map.insert(k, stop.clone());
        stop
    };
    let app = app.clone();
    std::thread::spawn(move || {
        let mut cursor = Cursor::default();
        let mut pause = Duration::from_millis(500);
        let emit = |line: Value| {
            let _ = app.emit("deck://agent", json!({ "server": server, "pane_id": pane_id, "line": line }));
        };
        while !stop.load(Ordering::Relaxed) {
            let result = crate::servers::runtime_for(&app, server.as_deref()).map_err(|e| e.to_string()).and_then(|rt| {
                rt.agent_subscribe(cursor.params(&pane_id), &mut |line| {
                    if stop.load(Ordering::Relaxed) {
                        return false;
                    }
                    cursor.see(&line);
                    emit(line);
                    true
                })
                .map_err(|e| e.to_string())
            });
            if stop.load(Ordering::Relaxed) {
                break;
            }
            let error = match result {
                Ok(()) => "stream ended".to_string(),
                Err(e) => e,
            };
            emit(json!({ "event": "deck.agent.link", "data": { "pane_id": pane_id, "error": error } }));
            std::thread::sleep(pause);
            pause = (pause * 2).min(Duration::from_secs(10));
        }
    });
}

/// Stop streaming a pane (the thread ends at its next line).
pub fn unwatch(app: &AppHandle, server: Option<&str>, pane_id: &str) {
    if let Some(stop) = app
        .state::<Watches>()
        .0
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&key(server, pane_id))
    {
        stop.store(true, Ordering::Relaxed);
    }
}

/// `deck.agent.<method>` on the pane's server.
pub fn call(app: &AppHandle, server: Option<&str>, method: &str, params: Value) -> Result<Value, String> {
    const METHODS: &[&str] = &["send", "respond", "interrupt", "set", "list"];
    if !METHODS.contains(&method) {
        return Err(format!("unknown agent method {method}"));
    }
    let rt = crate::servers::runtime_for(app, server)?;
    rt.agent_call(method, params).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_follows_epoch_and_seq() {
        let mut c = Cursor::default();
        assert_eq!(c.params("p"), json!({"pane_id":"p","epoch":null,"after_seq":null}));
        c.see(&json!({"id":"s","result":{"epoch":"e1"}}));
        c.see(&json!({"event":"deck.agent.event","data":{"epoch":"e1","seq":4,"msg":{}}}));
        c.see(&json!({"event":"deck.agent.event","data":{"epoch":"e1","seq":3,"msg":{}}}));
        assert_eq!(c.params("p")["after_seq"], 4);
        c.see(&json!({"event":"deck.agent.reset","data":{"epoch":"e2"}}));
        assert_eq!(c.params("p"), json!({"pane_id":"p","epoch":"e2","after_seq":null}));
        c.see(&json!({"event":"deck.agent.event","data":{"epoch":"e3","seq":1,"msg":{}}}));
        assert_eq!(c.params("p"), json!({"pane_id":"p","epoch":"e3","after_seq":1}));
    }
}
