//! The runtime contract (roadmap S0): exactly the part of a terminal/agent
//! server that Deck uses, behind one object-safe trait. `HerdrRuntime` is the
//! current implementation, a thin adapter over `herdr.rs`; the own server gets
//! its own adapter later (S5) on the same contract.
//!
//! One `Runtime` instance stands for one server. Deck manages a single
//! [`SharedRuntime`] in Tauri state; every module reaches the server through it
//! and nothing outside this file calls `herdr::call`/`herdr::subscribe`.
//!
//! Results stay `serde_json::Value` in Herdr's reply shape for now; the params
//! are typed so the wire mapping lives in one place ([`Request::herdr`]).

use std::sync::Arc;

use serde_json::{json, Map, Value};
use tauri::{AppHandle, Manager};

use crate::herdr;

pub use crate::herdr::Result;

/// The runtime as Deck holds it (Tauri state, worker threads).
pub type SharedRuntime = Arc<dyn Runtime>;

/// The runtime managed in `app`'s state.
pub fn of(app: &AppHandle) -> SharedRuntime {
    app.state::<SharedRuntime>().inner().clone()
}

/// What Deck asks of a server. Blocking: call from a worker thread, never
/// from the main thread.
///
/// The `*_focus` methods exist for Herdr compatibility (its TUI has a focused
/// pane); the own server will not implement them.
pub trait Runtime: Send + Sync {
    /// `{version, protocol}`; also the "is the server running" probe.
    fn ping(&self) -> Result<Value>;

    fn workspace_list(&self) -> Result<Value>;
    fn workspace_get(&self, workspace_id: &str) -> Result<Value>;
    fn workspace_create(&self, params: WorkspaceCreate) -> Result<Value>;
    fn workspace_rename(&self, workspace_id: &str, label: &str) -> Result<Value>;
    fn workspace_focus(&self, workspace_id: &str) -> Result<Value>;
    fn workspace_close(&self, workspace_id: &str, close_group: bool) -> Result<Value>;

    fn tab_list(&self, workspace_id: &str) -> Result<Value>;
    fn tab_create(&self, params: TabCreate) -> Result<Value>;
    fn tab_rename(&self, tab_id: &str, label: &str) -> Result<Value>;
    fn tab_focus(&self, tab_id: &str) -> Result<Value>;
    fn tab_close(&self, tab_id: &str) -> Result<Value>;

    fn pane_list(&self) -> Result<Value>;
    fn pane_get(&self, pane_id: &str) -> Result<Value>;
    fn pane_read(&self, params: PaneRead) -> Result<Value>;
    fn pane_send_text(&self, pane_id: &str, text: &str) -> Result<Value>;
    fn pane_send_keys(&self, pane_id: &str, keys: &[&str]) -> Result<Value>;
    fn pane_split(&self, params: PaneSplit) -> Result<Value>;
    fn pane_rename(&self, pane_id: &str, label: &str) -> Result<Value>;
    fn pane_focus(&self, pane_id: &str) -> Result<Value>;
    fn pane_close(&self, pane_id: &str) -> Result<Value>;

    fn agent_list(&self) -> Result<Value>;

    fn worktree_list(&self, cwd: &str) -> Result<Value>;
    fn worktree_open(&self, params: WorktreeOpen) -> Result<Value>;
    fn worktree_create(&self, params: WorktreeCreate) -> Result<Value>;

    /// Stream the named events to `on_event` until it returns `false`
    /// (then `Ok`) or the stream fails (`Err`, including a rejected name set).
    fn subscribe(&self, events: &[&str], on_event: &mut dyn FnMut(Value) -> bool) -> Result<()>;
}

// ---- params ---------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq)]
pub struct WorkspaceCreate {
    pub cwd: Option<String>,
    pub label: Option<String>,
    /// Create next to (in the group of) this workspace.
    pub source_workspace_id: Option<String>,
    pub focus: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct TabCreate {
    pub workspace_id: String,
    pub cwd: Option<String>,
    pub label: Option<String>,
    pub focus: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PaneRead {
    pub pane_id: String,
    /// `recent` | `viewport` | `detection`
    pub source: String,
    pub lines: u32,
    /// `ansi` | `text`
    pub format: String,
    pub strip_ansi: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SplitDirection {
    #[default]
    Right,
    Down,
}

impl SplitDirection {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "right" => Some(Self::Right),
            "down" => Some(Self::Down),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Right => "right",
            Self::Down => "down",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PaneSplit {
    pub target_pane_id: String,
    pub direction: SplitDirection,
    pub focus: bool,
}

/// Open the worktree (or main checkout) at `path` of the repository at `cwd`
/// as a workspace attached to its repository group.
#[derive(Debug, Clone, PartialEq)]
pub struct WorktreeOpen {
    pub cwd: String,
    pub path: String,
    pub label: Option<String>,
    pub focus: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorktreeCreate {
    pub cwd: String,
    pub branch: String,
    pub base: Option<String>,
    pub focus: bool,
}

// ---- requests -------------------------------------------------------------------

/// One call of the contract as data: what the webview-facing allowlists
/// (`herdr_action`, `herdr_focus`) build, run through [`dispatch`].
#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    Ping,
    WorkspaceList,
    WorkspaceGet {
        workspace_id: String,
    },
    WorkspaceCreate(WorkspaceCreate),
    WorkspaceRename {
        workspace_id: String,
        label: String,
    },
    WorkspaceFocus {
        workspace_id: String,
    },
    WorkspaceClose {
        workspace_id: String,
        close_group: bool,
    },
    TabList {
        workspace_id: String,
    },
    TabCreate(TabCreate),
    TabRename {
        tab_id: String,
        label: String,
    },
    TabFocus {
        tab_id: String,
    },
    TabClose {
        tab_id: String,
    },
    PaneList,
    PaneGet {
        pane_id: String,
    },
    PaneRead(PaneRead),
    PaneSendText {
        pane_id: String,
        text: String,
    },
    PaneSendKeys {
        pane_id: String,
        keys: Vec<String>,
    },
    PaneSplit(PaneSplit),
    PaneRename {
        pane_id: String,
        label: String,
    },
    PaneFocus {
        pane_id: String,
    },
    PaneClose {
        pane_id: String,
    },
    AgentList,
    WorktreeList {
        cwd: String,
    },
    WorktreeOpen(WorktreeOpen),
    WorktreeCreate(WorktreeCreate),
}

/// Run `req` on `rt` through the matching trait method.
pub fn dispatch(rt: &dyn Runtime, req: Request) -> Result<Value> {
    match req {
        Request::Ping => rt.ping(),
        Request::WorkspaceList => rt.workspace_list(),
        Request::WorkspaceGet { workspace_id } => rt.workspace_get(&workspace_id),
        Request::WorkspaceCreate(p) => rt.workspace_create(p),
        Request::WorkspaceRename {
            workspace_id,
            label,
        } => rt.workspace_rename(&workspace_id, &label),
        Request::WorkspaceFocus { workspace_id } => rt.workspace_focus(&workspace_id),
        Request::WorkspaceClose {
            workspace_id,
            close_group,
        } => rt.workspace_close(&workspace_id, close_group),
        Request::TabList { workspace_id } => rt.tab_list(&workspace_id),
        Request::TabCreate(p) => rt.tab_create(p),
        Request::TabRename { tab_id, label } => rt.tab_rename(&tab_id, &label),
        Request::TabFocus { tab_id } => rt.tab_focus(&tab_id),
        Request::TabClose { tab_id } => rt.tab_close(&tab_id),
        Request::PaneList => rt.pane_list(),
        Request::PaneGet { pane_id } => rt.pane_get(&pane_id),
        Request::PaneRead(p) => rt.pane_read(p),
        Request::PaneSendText { pane_id, text } => rt.pane_send_text(&pane_id, &text),
        Request::PaneSendKeys { pane_id, keys } => {
            let keys: Vec<&str> = keys.iter().map(String::as_str).collect();
            rt.pane_send_keys(&pane_id, &keys)
        }
        Request::PaneSplit(p) => rt.pane_split(p),
        Request::PaneRename { pane_id, label } => rt.pane_rename(&pane_id, &label),
        Request::PaneFocus { pane_id } => rt.pane_focus(&pane_id),
        Request::PaneClose { pane_id } => rt.pane_close(&pane_id),
        Request::AgentList => rt.agent_list(),
        Request::WorktreeList { cwd } => rt.worktree_list(&cwd),
        Request::WorktreeOpen(p) => rt.worktree_open(p),
        Request::WorktreeCreate(p) => rt.worktree_create(p),
    }
}

// ---- Herdr ----------------------------------------------------------------------

/// The Herdr server at `herdr::socket_path()`. Stateless: the connection
/// slots, deadline and reply parsing stay in `herdr.rs`.
#[derive(Debug, Default, Clone, Copy)]
pub struct HerdrRuntime;

impl HerdrRuntime {
    fn send(&self, req: Request) -> Result<Value> {
        let (method, params) = req.herdr();
        herdr::call(method, params)
    }
}

fn put(map: &mut Map<String, Value>, key: &str, value: Option<&String>) {
    if let Some(v) = value {
        map.insert(key.into(), Value::String(v.clone()));
    }
}

impl Request {
    /// Herdr method name and params for this request.
    pub fn herdr(&self) -> (&'static str, Value) {
        match self {
            Request::Ping => ("ping", json!({})),
            Request::WorkspaceList => ("workspace.list", json!({})),
            Request::WorkspaceGet { workspace_id } => {
                ("workspace.get", json!({ "workspace_id": workspace_id }))
            }
            Request::WorkspaceCreate(p) => {
                let mut m = Map::new();
                put(
                    &mut m,
                    "source_workspace_id",
                    p.source_workspace_id.as_ref(),
                );
                m.insert("focus".into(), Value::Bool(p.focus));
                put(&mut m, "cwd", p.cwd.as_ref());
                put(&mut m, "label", p.label.as_ref());
                ("workspace.create", Value::Object(m))
            }
            Request::WorkspaceRename {
                workspace_id,
                label,
            } => (
                "workspace.rename",
                json!({ "workspace_id": workspace_id, "label": label }),
            ),
            Request::WorkspaceFocus { workspace_id } => {
                ("workspace.focus", json!({ "workspace_id": workspace_id }))
            }
            Request::WorkspaceClose {
                workspace_id,
                close_group,
            } => {
                let mut params = json!({ "workspace_id": workspace_id });
                // Only sent when set, as Deck always has.
                if *close_group {
                    params["close_group"] = Value::Bool(true);
                }
                ("workspace.close", params)
            }
            Request::TabList { workspace_id } => {
                ("tab.list", json!({ "workspace_id": workspace_id }))
            }
            Request::TabCreate(p) => {
                let mut m = Map::new();
                m.insert("workspace_id".into(), Value::String(p.workspace_id.clone()));
                m.insert("focus".into(), Value::Bool(p.focus));
                put(&mut m, "cwd", p.cwd.as_ref());
                put(&mut m, "label", p.label.as_ref());
                ("tab.create", Value::Object(m))
            }
            Request::TabRename { tab_id, label } => {
                ("tab.rename", json!({ "tab_id": tab_id, "label": label }))
            }
            Request::TabFocus { tab_id } => ("tab.focus", json!({ "tab_id": tab_id })),
            Request::TabClose { tab_id } => ("tab.close", json!({ "tab_id": tab_id })),
            Request::PaneList => ("pane.list", json!({})),
            Request::PaneGet { pane_id } => ("pane.get", json!({ "pane_id": pane_id })),
            Request::PaneRead(p) => {
                let mut params = json!({
                    "pane_id": p.pane_id,
                    "source": p.source,
                    "lines": p.lines,
                    "format": p.format,
                });
                if let Some(strip) = p.strip_ansi {
                    params["strip_ansi"] = Value::Bool(strip);
                }
                ("pane.read", params)
            }
            Request::PaneSendText { pane_id, text } => (
                "pane.send_text",
                json!({ "pane_id": pane_id, "text": text }),
            ),
            Request::PaneSendKeys { pane_id, keys } => (
                "pane.send_keys",
                json!({ "pane_id": pane_id, "keys": keys }),
            ),
            Request::PaneSplit(p) => (
                "pane.split",
                json!({
                    "target_pane_id": p.target_pane_id,
                    "direction": p.direction.as_str(),
                    "focus": p.focus,
                }),
            ),
            Request::PaneRename { pane_id, label } => {
                ("pane.rename", json!({ "pane_id": pane_id, "label": label }))
            }
            Request::PaneFocus { pane_id } => ("pane.focus", json!({ "pane_id": pane_id })),
            Request::PaneClose { pane_id } => ("pane.close", json!({ "pane_id": pane_id })),
            Request::AgentList => ("agent.list", json!({})),
            // Deck only lists, opens and creates worktrees the user pointed it
            // at, so the repository is trusted (Herdr asks otherwise).
            Request::WorktreeList { cwd } => (
                "worktree.list",
                json!({ "cwd": cwd, "trust_repository": true }),
            ),
            Request::WorktreeOpen(p) => {
                let mut m = Map::new();
                m.insert("cwd".into(), Value::String(p.cwd.clone()));
                m.insert("path".into(), Value::String(p.path.clone()));
                put(&mut m, "label", p.label.as_ref());
                m.insert("focus".into(), Value::Bool(p.focus));
                m.insert("trust_repository".into(), Value::Bool(true));
                ("worktree.open", Value::Object(m))
            }
            Request::WorktreeCreate(p) => {
                let mut m = Map::new();
                m.insert("cwd".into(), Value::String(p.cwd.clone()));
                m.insert("branch".into(), Value::String(p.branch.clone()));
                m.insert("focus".into(), Value::Bool(p.focus));
                m.insert("trust_repository".into(), Value::Bool(true));
                put(&mut m, "base", p.base.as_ref());
                ("worktree.create", Value::Object(m))
            }
        }
    }
}

fn s(v: &str) -> String {
    v.to_string()
}

impl Runtime for HerdrRuntime {
    fn ping(&self) -> Result<Value> {
        self.send(Request::Ping)
    }

    fn workspace_list(&self) -> Result<Value> {
        self.send(Request::WorkspaceList)
    }

    fn workspace_get(&self, workspace_id: &str) -> Result<Value> {
        self.send(Request::WorkspaceGet {
            workspace_id: s(workspace_id),
        })
    }

    fn workspace_create(&self, params: WorkspaceCreate) -> Result<Value> {
        self.send(Request::WorkspaceCreate(params))
    }

    fn workspace_rename(&self, workspace_id: &str, label: &str) -> Result<Value> {
        self.send(Request::WorkspaceRename {
            workspace_id: s(workspace_id),
            label: s(label),
        })
    }

    fn workspace_focus(&self, workspace_id: &str) -> Result<Value> {
        self.send(Request::WorkspaceFocus {
            workspace_id: s(workspace_id),
        })
    }

    fn workspace_close(&self, workspace_id: &str, close_group: bool) -> Result<Value> {
        self.send(Request::WorkspaceClose {
            workspace_id: s(workspace_id),
            close_group,
        })
    }

    fn tab_list(&self, workspace_id: &str) -> Result<Value> {
        self.send(Request::TabList {
            workspace_id: s(workspace_id),
        })
    }

    fn tab_create(&self, params: TabCreate) -> Result<Value> {
        self.send(Request::TabCreate(params))
    }

    fn tab_rename(&self, tab_id: &str, label: &str) -> Result<Value> {
        self.send(Request::TabRename {
            tab_id: s(tab_id),
            label: s(label),
        })
    }

    fn tab_focus(&self, tab_id: &str) -> Result<Value> {
        self.send(Request::TabFocus { tab_id: s(tab_id) })
    }

    fn tab_close(&self, tab_id: &str) -> Result<Value> {
        self.send(Request::TabClose { tab_id: s(tab_id) })
    }

    fn pane_list(&self) -> Result<Value> {
        self.send(Request::PaneList)
    }

    fn pane_get(&self, pane_id: &str) -> Result<Value> {
        self.send(Request::PaneGet {
            pane_id: s(pane_id),
        })
    }

    fn pane_read(&self, params: PaneRead) -> Result<Value> {
        self.send(Request::PaneRead(params))
    }

    fn pane_send_text(&self, pane_id: &str, text: &str) -> Result<Value> {
        self.send(Request::PaneSendText {
            pane_id: s(pane_id),
            text: s(text),
        })
    }

    fn pane_send_keys(&self, pane_id: &str, keys: &[&str]) -> Result<Value> {
        self.send(Request::PaneSendKeys {
            pane_id: s(pane_id),
            keys: keys.iter().map(|k| s(k)).collect(),
        })
    }

    fn pane_split(&self, params: PaneSplit) -> Result<Value> {
        self.send(Request::PaneSplit(params))
    }

    fn pane_rename(&self, pane_id: &str, label: &str) -> Result<Value> {
        self.send(Request::PaneRename {
            pane_id: s(pane_id),
            label: s(label),
        })
    }

    fn pane_focus(&self, pane_id: &str) -> Result<Value> {
        self.send(Request::PaneFocus {
            pane_id: s(pane_id),
        })
    }

    fn pane_close(&self, pane_id: &str) -> Result<Value> {
        self.send(Request::PaneClose {
            pane_id: s(pane_id),
        })
    }

    fn agent_list(&self) -> Result<Value> {
        self.send(Request::AgentList)
    }

    fn worktree_list(&self, cwd: &str) -> Result<Value> {
        self.send(Request::WorktreeList { cwd: s(cwd) })
    }

    fn worktree_open(&self, params: WorktreeOpen) -> Result<Value> {
        self.send(Request::WorktreeOpen(params))
    }

    fn worktree_create(&self, params: WorktreeCreate) -> Result<Value> {
        self.send(Request::WorktreeCreate(params))
    }

    fn subscribe(&self, events: &[&str], on_event: &mut dyn FnMut(Value) -> bool) -> Result<()> {
        herdr::subscribe(events, on_event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn workspace_create_sends_only_set_fields() {
        let (m, p) = Request::WorkspaceCreate(WorkspaceCreate {
            focus: true,
            ..Default::default()
        })
        .herdr();
        assert_eq!((m, p), ("workspace.create", json!({ "focus": true })));
        let (_, p) = Request::WorkspaceCreate(WorkspaceCreate {
            cwd: Some("C:/r".into()),
            label: Some("r".into()),
            source_workspace_id: Some("ws_1".into()),
            focus: false,
        })
        .herdr();
        assert_eq!(
            p,
            json!({ "cwd": "C:/r", "label": "r", "source_workspace_id": "ws_1", "focus": false })
        );
    }

    #[test]
    fn workspace_close_sends_close_group_only_when_set() {
        let req = |close_group| Request::WorkspaceClose {
            workspace_id: "ws".into(),
            close_group,
        };
        assert_eq!(req(false).herdr().1, json!({ "workspace_id": "ws" }));
        assert_eq!(
            req(true).herdr().1,
            json!({ "workspace_id": "ws", "close_group": true })
        );
    }

    #[test]
    fn tab_create_always_sends_focus() {
        let (m, p) = Request::TabCreate(TabCreate {
            workspace_id: "ws".into(),
            cwd: Some("/d".into()),
            label: None,
            focus: false,
        })
        .herdr();
        assert_eq!(m, "tab.create");
        assert_eq!(
            p,
            json!({ "workspace_id": "ws", "cwd": "/d", "focus": false })
        );
    }

    #[test]
    fn pane_read_sends_strip_ansi_only_when_given() {
        let read = |strip_ansi| {
            Request::PaneRead(PaneRead {
                pane_id: "p".into(),
                source: "recent".into(),
                lines: 400,
                format: "ansi".into(),
                strip_ansi,
            })
            .herdr()
            .1
        };
        assert_eq!(
            read(Some(false)),
            json!({ "pane_id": "p", "source": "recent", "lines": 400, "format": "ansi", "strip_ansi": false })
        );
        assert!(read(None).get("strip_ansi").is_none());
    }

    #[test]
    fn worktree_requests_trust_the_repository() {
        let (m, p) = Request::WorktreeList { cwd: "/r".into() }.herdr();
        assert_eq!(
            (m, p),
            (
                "worktree.list",
                json!({ "cwd": "/r", "trust_repository": true })
            )
        );
        let (m, p) = Request::WorktreeOpen(WorktreeOpen {
            cwd: "/r".into(),
            path: "/r/wt".into(),
            label: None,
            focus: true,
        })
        .herdr();
        assert_eq!(m, "worktree.open");
        assert_eq!(
            p,
            json!({ "cwd": "/r", "path": "/r/wt", "focus": true, "trust_repository": true })
        );
        let (m, p) = Request::WorktreeCreate(WorktreeCreate {
            cwd: "/r".into(),
            branch: "feat".into(),
            base: Some("main".into()),
            focus: true,
        })
        .herdr();
        assert_eq!(m, "worktree.create");
        assert_eq!(
            p,
            json!({ "cwd": "/r", "branch": "feat", "base": "main", "focus": true, "trust_repository": true })
        );
    }

    #[test]
    fn pane_input_and_split_params() {
        assert_eq!(
            Request::PaneSendKeys {
                pane_id: "p".into(),
                keys: vec!["enter".into()]
            }
            .herdr(),
            (
                "pane.send_keys",
                json!({ "pane_id": "p", "keys": ["enter"] })
            )
        );
        assert_eq!(
            Request::PaneSplit(PaneSplit {
                target_pane_id: "p".into(),
                direction: SplitDirection::Down,
                focus: true,
            })
            .herdr(),
            (
                "pane.split",
                json!({ "target_pane_id": "p", "direction": "down", "focus": true })
            )
        );
        assert_eq!(SplitDirection::parse("down"), Some(SplitDirection::Down));
        assert_eq!(SplitDirection::parse("left"), None);
    }

    /// Records the request each trait method stands for.
    #[derive(Default)]
    struct Recorder(Mutex<Vec<Request>>);

    impl Recorder {
        fn rec(&self, r: Request) -> Result<Value> {
            self.0.lock().unwrap().push(r);
            Ok(Value::Null)
        }
    }

    impl Runtime for Recorder {
        fn ping(&self) -> Result<Value> {
            self.rec(Request::Ping)
        }
        fn workspace_list(&self) -> Result<Value> {
            self.rec(Request::WorkspaceList)
        }
        fn workspace_get(&self, id: &str) -> Result<Value> {
            self.rec(Request::WorkspaceGet {
                workspace_id: s(id),
            })
        }
        fn workspace_create(&self, p: WorkspaceCreate) -> Result<Value> {
            self.rec(Request::WorkspaceCreate(p))
        }
        fn workspace_rename(&self, id: &str, label: &str) -> Result<Value> {
            self.rec(Request::WorkspaceRename {
                workspace_id: s(id),
                label: s(label),
            })
        }
        fn workspace_focus(&self, id: &str) -> Result<Value> {
            self.rec(Request::WorkspaceFocus {
                workspace_id: s(id),
            })
        }
        fn workspace_close(&self, id: &str, close_group: bool) -> Result<Value> {
            self.rec(Request::WorkspaceClose {
                workspace_id: s(id),
                close_group,
            })
        }
        fn tab_list(&self, id: &str) -> Result<Value> {
            self.rec(Request::TabList {
                workspace_id: s(id),
            })
        }
        fn tab_create(&self, p: TabCreate) -> Result<Value> {
            self.rec(Request::TabCreate(p))
        }
        fn tab_rename(&self, id: &str, label: &str) -> Result<Value> {
            self.rec(Request::TabRename {
                tab_id: s(id),
                label: s(label),
            })
        }
        fn tab_focus(&self, id: &str) -> Result<Value> {
            self.rec(Request::TabFocus { tab_id: s(id) })
        }
        fn tab_close(&self, id: &str) -> Result<Value> {
            self.rec(Request::TabClose { tab_id: s(id) })
        }
        fn pane_list(&self) -> Result<Value> {
            self.rec(Request::PaneList)
        }
        fn pane_get(&self, id: &str) -> Result<Value> {
            self.rec(Request::PaneGet { pane_id: s(id) })
        }
        fn pane_read(&self, p: PaneRead) -> Result<Value> {
            self.rec(Request::PaneRead(p))
        }
        fn pane_send_text(&self, id: &str, text: &str) -> Result<Value> {
            self.rec(Request::PaneSendText {
                pane_id: s(id),
                text: s(text),
            })
        }
        fn pane_send_keys(&self, id: &str, keys: &[&str]) -> Result<Value> {
            self.rec(Request::PaneSendKeys {
                pane_id: s(id),
                keys: keys.iter().map(|k| s(k)).collect(),
            })
        }
        fn pane_split(&self, p: PaneSplit) -> Result<Value> {
            self.rec(Request::PaneSplit(p))
        }
        fn pane_rename(&self, id: &str, label: &str) -> Result<Value> {
            self.rec(Request::PaneRename {
                pane_id: s(id),
                label: s(label),
            })
        }
        fn pane_focus(&self, id: &str) -> Result<Value> {
            self.rec(Request::PaneFocus { pane_id: s(id) })
        }
        fn pane_close(&self, id: &str) -> Result<Value> {
            self.rec(Request::PaneClose { pane_id: s(id) })
        }
        fn agent_list(&self) -> Result<Value> {
            self.rec(Request::AgentList)
        }
        fn worktree_list(&self, cwd: &str) -> Result<Value> {
            self.rec(Request::WorktreeList { cwd: s(cwd) })
        }
        fn worktree_open(&self, p: WorktreeOpen) -> Result<Value> {
            self.rec(Request::WorktreeOpen(p))
        }
        fn worktree_create(&self, p: WorktreeCreate) -> Result<Value> {
            self.rec(Request::WorktreeCreate(p))
        }
        fn subscribe(&self, _: &[&str], _: &mut dyn FnMut(Value) -> bool) -> Result<()> {
            Ok(())
        }
    }

    #[test]
    fn dispatch_reaches_the_matching_method_with_the_same_request() {
        let all = vec![
            Request::Ping,
            Request::WorkspaceList,
            Request::WorkspaceGet {
                workspace_id: "w".into(),
            },
            Request::WorkspaceCreate(WorkspaceCreate::default()),
            Request::WorkspaceRename {
                workspace_id: "w".into(),
                label: "l".into(),
            },
            Request::WorkspaceFocus {
                workspace_id: "w".into(),
            },
            Request::WorkspaceClose {
                workspace_id: "w".into(),
                close_group: true,
            },
            Request::TabList {
                workspace_id: "w".into(),
            },
            Request::TabCreate(TabCreate::default()),
            Request::TabRename {
                tab_id: "t".into(),
                label: "l".into(),
            },
            Request::TabFocus { tab_id: "t".into() },
            Request::TabClose { tab_id: "t".into() },
            Request::PaneList,
            Request::PaneGet {
                pane_id: "p".into(),
            },
            Request::PaneSendText {
                pane_id: "p".into(),
                text: "x".into(),
            },
            Request::PaneSendKeys {
                pane_id: "p".into(),
                keys: vec!["enter".into(), "esc".into()],
            },
            Request::PaneSplit(PaneSplit {
                target_pane_id: "p".into(),
                direction: SplitDirection::Right,
                focus: true,
            }),
            Request::PaneRename {
                pane_id: "p".into(),
                label: "l".into(),
            },
            Request::PaneFocus {
                pane_id: "p".into(),
            },
            Request::PaneClose {
                pane_id: "p".into(),
            },
            Request::AgentList,
            Request::WorktreeList { cwd: "/r".into() },
        ];
        let rec = Recorder::default();
        for req in &all {
            dispatch(&rec, req.clone()).unwrap();
        }
        assert_eq!(*rec.0.lock().unwrap(), all);
    }
}
