//! The servers Deck talks to (docs/plan/DECK-SERVERS-UI.md).
//!
//! * **Discovery** finds local Herdr-protocol servers: in each known app dir
//!   (`herdr`, `herdr-dev`, `deck-server`, `deck-server-dev`) the root
//!   `herdr.sock` and every `sessions/<name>/herdr.sock`, each pinged with a
//!   short deadline. Found servers are only *offered*; nothing connects by itself.
//! * **Preferences** (`preferences.json` in Deck's data dir) remember which
//!   servers the user connected, their names and manually added addresses.
//!   A missing file means the old single-server Deck: the default Herdr is
//!   connected, as before.
//! * **Connected** servers each get a [`HerdrRuntime`] and an event pump that
//!   tags every event with the server id (`herdr://event` gains `server`).
//!
//! Server ids are short and stable: `herdr`, `herdr-dev/decktest`,
//! `deck-server/try`; an address outside the known dirs is its own id.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};

use crate::herdr;
use crate::runtime::{HerdrRuntime, SharedRuntime};

/// App dirs under the OS config root that hold Herdr-protocol servers.
pub const APP_DIRS: &[&str] = &["herdr", "herdr-dev", "deck-server", "deck-server-dev"];

const PREFS_FILE: &str = "preferences.json";
const PING_TIMEOUT: Duration = Duration::from_millis(1500);
const SCAN_EVERY: Duration = Duration::from_secs(60);
pub const EVENT: &str = "deck://servers";

// ---- ids, names, discovery (pure) ------------------------------------------------

/// Where a socket sits relative to the config root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    pub id: String,
    pub app: Option<String>,
    pub session: Option<String>,
}

/// The id of the server at `socket`: `app` for `<root>/<app>/herdr.sock`,
/// `app/session` for `<root>/<app>/sessions/<session>/herdr.sock`, else the path.
pub fn place_of(root: &Path, socket: &Path) -> Place {
    let other = || Place {
        id: socket.to_string_lossy().into_owned(),
        app: None,
        session: None,
    };
    if socket.file_name().and_then(|n| n.to_str()) != Some("herdr.sock") {
        return other();
    }
    let Some(dir) = socket.parent() else {
        return other();
    };
    let name = |p: &Path| p.file_name().and_then(|n| n.to_str()).map(str::to_string);
    let same = |a: &Path, b: &Path| {
        a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
    };
    if let (Some(app_root), Some(app)) = (dir.parent(), name(dir)) {
        if same(app_root, root) && APP_DIRS.contains(&app.as_str()) {
            return Place {
                id: app.clone(),
                app: Some(app),
                session: None,
            };
        }
    }
    let sessions = dir.parent();
    let app_dir = sessions.and_then(Path::parent);
    if let (Some(sessions), Some(app_dir), Some(session)) = (sessions, app_dir, name(dir)) {
        let app = name(app_dir).unwrap_or_default();
        if name(sessions).as_deref() == Some("sessions")
            && app_dir.parent().is_some_and(|r| same(r, root))
            && APP_DIRS.contains(&app.as_str())
        {
            return Place {
                id: format!("{app}/{session}"),
                app: Some(app),
                session: Some(session),
            };
        }
    }
    other()
}

/// `herdr` or `deck-server`, from the app dir; unknown places count as Herdr.
pub fn kind_of(place: &Place) -> &'static str {
    match &place.app {
        Some(app) if app.starts_with("deck-server") => "deck-server",
        _ => "herdr",
    }
}

/// Default display name: `herdr`, `herdr-dev · decktest`, or the file path.
pub fn default_name(place: &Place) -> String {
    match (&place.app, &place.session) {
        (Some(app), Some(session)) => format!("{app} · {session}"),
        (Some(app), None) => app.clone(),
        _ => place.id.clone(),
    }
}

/// Every socket worth pinging under `root`: the root session of each app dir
/// that exists, then its `sessions/*` (sorted, so the list is stable).
pub fn candidates(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for app in APP_DIRS {
        let dir = root.join(app);
        if !dir.is_dir() {
            continue;
        }
        out.push(dir.join("herdr.sock"));
        let mut sessions: Vec<PathBuf> = std::fs::read_dir(dir.join("sessions"))
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().is_dir())
            .map(|e| e.path().join("herdr.sock"))
            .collect();
        sessions.sort();
        out.extend(sessions);
    }
    out
}

/// A user-typed address as a socket path: `\\.\pipe\` is dropped on Windows,
/// surrounding quotes and whitespace too.
pub fn normalize_address(input: &str) -> Option<PathBuf> {
    let t = input.trim().trim_matches('"').trim();
    let t = t
        .strip_prefix(r"\\.\pipe\")
        .or_else(|| t.strip_prefix(r"\\?\pipe\"))
        .unwrap_or(t);
    (!t.is_empty()).then(|| PathBuf::from(t))
}

/// A ping failure that means "nothing is listening there": not offered.
fn is_absent(err: &herdr::HerdrError) -> bool {
    matches!(err, herdr::HerdrError::Connect { source, .. }
        if matches!(source.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused))
}

// ---- preferences ------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Prefs {
    #[serde(default)]
    pub servers: Vec<SavedServer>,
}

/// A server the user has seen in preferences (connected or not).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedServer {
    pub id: String,
    pub socket: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default)]
    pub connected: bool,
    #[serde(default)]
    pub manual: bool,
}

impl Prefs {
    /// No preferences yet: connect what the single-server Deck used
    /// (`HERDR_SOCKET_PATH`, else the default Herdr).
    pub fn first_run(root: &Path, default_socket: &Path) -> Self {
        let place = place_of(root, default_socket);
        Prefs {
            servers: vec![SavedServer {
                manual: place.app.is_none(),
                id: place.id,
                socket: default_socket.to_path_buf(),
                name: None,
                connected: true,
            }],
        }
    }

    /// Read `path`; `None` when it does not exist. A broken file is treated as
    /// empty (logged) so Deck still starts.
    pub fn load(path: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(path).ok()?;
        match serde_json::from_str(&text) {
            Ok(p) => Some(p),
            Err(err) => {
                tracing::warn!(%err, path = %path.display(), "preferences.json is unreadable; starting empty");
                Some(Prefs::default())
            }
        }
    }

    /// Write through a temp file so a crash never leaves half a file.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let tmp = path.with_extension("json.tmp");
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, path).map_err(|e| e.to_string())
    }

    pub fn get(&self, id: &str) -> Option<&SavedServer> {
        self.servers.iter().find(|s| s.id == id)
    }

    fn entry(&mut self, id: &str, socket: &Path, manual: bool) -> &mut SavedServer {
        if let Some(i) = self.servers.iter().position(|s| s.id == id) {
            return &mut self.servers[i];
        }
        self.servers.push(SavedServer {
            id: id.to_string(),
            socket: socket.to_path_buf(),
            name: None,
            connected: false,
            manual,
        });
        self.servers.last_mut().expect("just pushed")
    }
}

// ---- registry ---------------------------------------------------------------------

/// Link state of a connected server, or the ping result of a found one.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Status {
    /// `connecting` | `online` | `offline`
    pub state: &'static str,
    pub version: Option<String>,
    pub protocol: Option<u64>,
    pub error: Option<String>,
}

impl Status {
    pub fn offline(error: String) -> Self {
        Status {
            state: "offline",
            error: Some(error),
            ..Status::default()
        }
    }

    pub fn from_ping(result: &herdr::Result<Value>) -> Self {
        match result {
            Ok(v) => Status {
                state: "online",
                version: v.get("version").and_then(Value::as_str).map(str::to_string),
                protocol: v.get("protocol").and_then(Value::as_u64),
                error: None,
            },
            Err(err) => Status {
                state: "offline",
                error: Some(err.to_string()),
                ..Status::default()
            },
        }
    }
}

#[derive(Debug, Clone)]
struct Found {
    place: Place,
    socket: PathBuf,
    status: Status,
}

struct Live {
    rt: SharedRuntime,
    socket: PathBuf,
    status: Status,
    stop: Arc<AtomicBool>,
}

#[derive(Default)]
struct State {
    prefs: Prefs,
    found: Vec<Found>,
    scanned: bool,
    live: HashMap<String, Live>,
}

/// One row of the SERVERS list, as the webview shows it.
#[derive(Debug, Clone, Serialize)]
pub struct ServerView {
    pub id: String,
    pub name: String,
    pub kind: &'static str,
    pub address: String,
    pub connected: bool,
    pub manual: bool,
    /// Found by the last scan.
    pub found: bool,
    /// Found and never shown in preferences before (the gear's cyan dot).
    pub new: bool,
    pub status: Status,
}

#[derive(Debug, Clone, Serialize)]
pub struct ServersView {
    pub servers: Vec<ServerView>,
    pub scanned: bool,
}

pub struct Registry {
    prefs_path: PathBuf,
    root: PathBuf,
    state: Mutex<State>,
}

impl Registry {
    /// Load preferences from `data_dir` (or the first-run default). Nothing is
    /// connected until [`Registry::start`].
    pub fn open(data_dir: &Path) -> Self {
        let root = herdr::config_root();
        let prefs_path = data_dir.join(PREFS_FILE);
        let env_socket = std::env::var_os("HERDR_SOCKET_PATH")
            .filter(|s| !s.is_empty())
            .map(PathBuf::from);
        let mut prefs = Prefs::load(&prefs_path).unwrap_or_else(|| {
            let p = Prefs::first_run(&root, &herdr::socket_path());
            if let Err(err) = p.save(&prefs_path) {
                tracing::warn!(%err, "could not write preferences.json");
            }
            p
        });
        // An explicit HERDR_SOCKET_PATH (scripts/try-deck-server.ps1) is
        // connected unless the user disconnected it before.
        if let Some(socket) = env_socket {
            let place = place_of(&root, &socket);
            if prefs.get(&place.id).is_none() {
                prefs
                    .entry(&place.id, &socket, place.app.is_none())
                    .connected = true;
                let _ = prefs.save(&prefs_path);
            }
        }
        Registry {
            prefs_path,
            root,
            state: Mutex::new(State {
                prefs,
                ..State::default()
            }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Connect the saved servers and start background discovery.
    pub fn start(app: &AppHandle) {
        let reg = app.state::<Registry>();
        let saved: Vec<SavedServer> = reg.lock().prefs.servers.clone();
        for s in saved.iter().filter(|s| s.connected) {
            reg.attach(app, &s.id, &s.socket);
        }
        let app = app.clone();
        std::thread::Builder::new()
            .name("deck-servers-scan".into())
            .spawn(move || loop {
                app.state::<Registry>().scan(&app);
                std::thread::sleep(SCAN_EVERY);
            })
            .ok();
    }

    /// Runtime of a connected server; `None` = the primary one.
    pub fn runtime(&self, server: Option<&str>) -> Result<SharedRuntime, String> {
        match server.filter(|s| !s.is_empty()) {
            None => Ok(self.primary()),
            Some(id) => self
                .lock()
                .live
                .get(id)
                .map(|l| l.rt.clone())
                .ok_or_else(|| format!("server {id} is not connected")),
        }
    }

    /// The first connected server in preference order, online ones first.
    /// With nothing connected, the default Herdr address (calls then fail
    /// with a "not reachable" error the UI already knows how to show).
    pub fn primary(&self) -> SharedRuntime {
        let st = self.lock();
        let order = || st.prefs.servers.iter().filter_map(|s| st.live.get(&s.id));
        order()
            .find(|l| l.status.state == "online")
            .or_else(|| order().next())
            .map(|l| l.rt.clone())
            .unwrap_or_else(|| Arc::new(HerdrRuntime::new(herdr::socket_path())))
    }

    /// The primary server's id, if any is connected.
    pub fn primary_id(&self) -> Option<String> {
        let st = self.lock();
        let order = || {
            st.prefs
                .servers
                .iter()
                .filter(|s| st.live.contains_key(&s.id))
        };
        order()
            .find(|s| st.live[&s.id].status.state == "online")
            .or_else(|| order().next())
            .map(|s| s.id.clone())
    }

    /// Config dir of a connected server (its `session.json`), for the archive.
    pub fn config_dir(&self, server: Option<&str>) -> Option<PathBuf> {
        let id = server.map(str::to_string).or_else(|| self.primary_id())?;
        let st = self.lock();
        let socket = st.live.get(&id).map(|l| l.socket.clone())?;
        socket.parent().map(Path::to_path_buf)
    }

    pub fn view(&self) -> ServersView {
        let st = self.lock();
        let mut rows: Vec<ServerView> = Vec::new();
        for s in &st.prefs.servers {
            let place = place_of(&self.root, &s.socket);
            let found = st.found.iter().find(|f| f.place.id == s.id);
            let status = match st.live.get(&s.id) {
                Some(l) => l.status.clone(),
                None => found.map(|f| f.status.clone()).unwrap_or_default(),
            };
            // A server that is neither connected nor found any more is not offered.
            if !s.connected && found.is_none() && !s.manual {
                continue;
            }
            rows.push(ServerView {
                id: s.id.clone(),
                name: s.name.clone().unwrap_or_else(|| default_name(&place)),
                kind: kind_of(&place),
                address: herdr::address_of(&s.socket),
                connected: s.connected,
                manual: s.manual,
                found: found.is_some(),
                new: false,
                status,
            });
        }
        for f in &st.found {
            if st.prefs.get(&f.place.id).is_some() {
                continue;
            }
            rows.push(ServerView {
                id: f.place.id.clone(),
                name: default_name(&f.place),
                kind: kind_of(&f.place),
                address: herdr::address_of(&f.socket),
                connected: false,
                manual: false,
                found: true,
                new: true,
                status: f.status.clone(),
            });
        }
        ServersView {
            servers: rows,
            scanned: st.scanned,
        }
    }

    /// Ping every candidate (in parallel) and keep the ones that answer or
    /// exist but refuse us (an elevated server). Emits [`EVENT`] on change.
    pub fn scan(&self, app: &AppHandle) {
        let mut sockets = candidates(&self.root);
        // Manual and saved addresses are pinged too, so their rows have a status.
        for s in self.lock().prefs.servers.iter() {
            if !sockets.contains(&s.socket) {
                sockets.push(s.socket.clone());
            }
        }
        let handles: Vec<_> = sockets
            .into_iter()
            .map(|socket| {
                std::thread::spawn(move || {
                    let r = herdr::call_within(&socket, "ping", json!({}), PING_TIMEOUT);
                    (socket, r)
                })
            })
            .collect();
        let mut found = Vec::new();
        for h in handles {
            let Ok((socket, result)) = h.join() else {
                continue;
            };
            if matches!(&result, Err(e) if is_absent(e)) {
                continue;
            }
            found.push(Found {
                place: place_of(&self.root, &socket),
                status: Status::from_ping(&result),
                socket,
            });
        }
        let changed = {
            let mut st = self.lock();
            let before: Vec<(String, Status)> = st
                .found
                .iter()
                .map(|f| (f.place.id.clone(), f.status.clone()))
                .collect();
            let after: Vec<(String, Status)> = found
                .iter()
                .map(|f| (f.place.id.clone(), f.status.clone()))
                .collect();
            st.found = found;
            let first = !st.scanned;
            st.scanned = true;
            first || before != after
        };
        if changed {
            let _ = app.emit(EVENT, "changed");
        }
    }

    fn save(&self, st: &State) -> Result<(), String> {
        st.prefs.save(&self.prefs_path)
    }

    fn socket_of(&self, st: &State, id: &str) -> Option<(PathBuf, bool)> {
        st.prefs
            .get(id)
            .map(|s| (s.socket.clone(), s.manual))
            .or_else(|| {
                st.found
                    .iter()
                    .find(|f| f.place.id == id)
                    .map(|f| (f.socket.clone(), false))
            })
    }

    pub fn connect(&self, app: &AppHandle, id: &str) -> Result<(), String> {
        let socket = {
            let mut st = self.lock();
            let (socket, manual) = self
                .socket_of(&st, id)
                .ok_or_else(|| format!("unknown server {id}"))?;
            st.prefs.entry(id, &socket, manual).connected = true;
            self.save(&st)?;
            socket
        };
        self.attach(app, id, &socket);
        Ok(())
    }

    pub fn disconnect(&self, app: &AppHandle, id: &str) -> Result<(), String> {
        let mut st = self.lock();
        if let Some(s) = st.prefs.servers.iter_mut().find(|s| s.id == id) {
            s.connected = false;
        }
        if let Some(live) = st.live.remove(id) {
            live.stop.store(true, Ordering::SeqCst);
        }
        self.save(&st)?;
        drop(st);
        let _ = app.emit(EVENT, "changed");
        Ok(())
    }

    pub fn rename(&self, app: &AppHandle, id: &str, name: &str) -> Result<(), String> {
        let mut st = self.lock();
        let (socket, manual) = self
            .socket_of(&st, id)
            .ok_or_else(|| format!("unknown server {id}"))?;
        let name = name.trim();
        st.prefs.entry(id, &socket, manual).name = (!name.is_empty()).then(|| name.to_string());
        self.save(&st)?;
        drop(st);
        let _ = app.emit(EVENT, "changed");
        Ok(())
    }

    /// Add an address by hand and connect it. Returns its id.
    pub fn add(&self, app: &AppHandle, address: &str) -> Result<String, String> {
        let socket = normalize_address(address).ok_or("empty address")?;
        let place = place_of(&self.root, &socket);
        {
            let mut st = self.lock();
            let manual = place.app.is_none();
            st.prefs.entry(&place.id, &socket, manual).connected = true;
            self.save(&st)?;
        }
        self.attach(app, &place.id, &socket);
        Ok(place.id)
    }

    /// Remove a manually added server from the list.
    pub fn forget(&self, app: &AppHandle, id: &str) -> Result<(), String> {
        self.disconnect(app, id)?;
        let mut st = self.lock();
        st.prefs.servers.retain(|s| s.id != id || !s.manual);
        self.save(&st)?;
        Ok(())
    }

    /// The user saw the found servers: they stop being "new".
    pub fn mark_seen(&self, app: &AppHandle) -> Result<(), String> {
        let mut st = self.lock();
        let fresh: Vec<(String, PathBuf)> = st
            .found
            .iter()
            .filter(|f| st.prefs.get(&f.place.id).is_none())
            .map(|f| (f.place.id.clone(), f.socket.clone()))
            .collect();
        if fresh.is_empty() {
            return Ok(());
        }
        for (id, socket) in fresh {
            st.prefs.entry(&id, &socket, false);
        }
        self.save(&st)?;
        drop(st);
        let _ = app.emit(EVENT, "changed");
        Ok(())
    }

    /// Start the runtime and event pump of a server (no-op when running).
    fn attach(&self, app: &AppHandle, id: &str, socket: &Path) {
        let stop = Arc::new(AtomicBool::new(false));
        let rt: SharedRuntime = Arc::new(HerdrRuntime::new(socket));
        {
            let mut st = self.lock();
            if st.live.contains_key(id) {
                return;
            }
            st.live.insert(
                id.to_string(),
                Live {
                    rt: rt.clone(),
                    socket: socket.to_path_buf(),
                    status: Status {
                        state: "connecting",
                        ..Status::default()
                    },
                    stop: stop.clone(),
                },
            );
        }
        let _ = app.emit(EVENT, "changed");
        crate::start_event_pump(app.clone(), id.to_string(), rt, stop);
    }

    /// Record a connected server's link state; emits [`EVENT`] on change.
    pub fn set_status(&self, app: &AppHandle, id: &str, status: Status) {
        let changed = {
            let mut st = self.lock();
            match st.live.get_mut(id) {
                Some(l) if l.status != status => {
                    l.status = status;
                    true
                }
                _ => false,
            }
        };
        if changed {
            let _ = app.emit(EVENT, "changed");
        }
    }
}

// ---- commands -------------------------------------------------------------------

fn view_of(app: &AppHandle) -> ServersView {
    app.state::<Registry>().view()
}

#[tauri::command]
pub fn servers_list(app: AppHandle) -> ServersView {
    view_of(&app)
}

#[tauri::command]
pub async fn servers_scan(app: AppHandle) -> Result<ServersView, String> {
    crate::blocking(move || {
        app.state::<Registry>().scan(&app);
        Ok(view_of(&app))
    })
    .await
}

#[tauri::command]
pub fn servers_seen(app: AppHandle) -> Result<ServersView, String> {
    app.state::<Registry>().mark_seen(&app)?;
    Ok(view_of(&app))
}

#[tauri::command]
pub fn server_connect(app: AppHandle, id: String) -> Result<ServersView, String> {
    app.state::<Registry>().connect(&app, &id)?;
    Ok(view_of(&app))
}

#[tauri::command]
pub fn server_disconnect(app: AppHandle, id: String) -> Result<ServersView, String> {
    app.state::<Registry>().disconnect(&app, &id)?;
    Ok(view_of(&app))
}

#[tauri::command]
pub fn server_rename(app: AppHandle, id: String, name: String) -> Result<ServersView, String> {
    app.state::<Registry>().rename(&app, &id, &name)?;
    Ok(view_of(&app))
}

#[tauri::command]
pub fn server_add(app: AppHandle, address: String) -> Result<ServersView, String> {
    app.state::<Registry>().add(&app, &address)?;
    Ok(view_of(&app))
}

#[tauri::command]
pub fn server_forget(app: AppHandle, id: String) -> Result<ServersView, String> {
    app.state::<Registry>().forget(&app, &id)?;
    Ok(view_of(&app))
}

/// A connected server's runtime for a module that only has the app handle.
pub fn runtime_for(app: &AppHandle, server: Option<&str>) -> Result<SharedRuntime, String> {
    app.state::<Registry>().runtime(server)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> PathBuf {
        PathBuf::from(if cfg!(windows) {
            r"C:\Users\u\AppData\Roaming"
        } else {
            "/home/u/.config"
        })
    }

    #[test]
    fn place_of_root_session_is_the_app_dir() {
        let p = place_of(&root(), &root().join("herdr").join("herdr.sock"));
        assert_eq!(p.id, "herdr");
        assert_eq!(kind_of(&p), "herdr");
        assert_eq!(default_name(&p), "herdr");
    }

    #[test]
    fn place_of_named_session_is_app_slash_session() {
        let s = root()
            .join("deck-server")
            .join("sessions")
            .join("try")
            .join("herdr.sock");
        let p = place_of(&root(), &s);
        assert_eq!(p.id, "deck-server/try");
        assert_eq!(kind_of(&p), "deck-server");
        assert_eq!(default_name(&p), "deck-server · try");
    }

    #[test]
    fn place_of_unknown_dir_is_the_path() {
        let s = PathBuf::from(if cfg!(windows) {
            r"D:\x\herdr.sock"
        } else {
            "/tmp/x/herdr.sock"
        });
        let p = place_of(&root(), &s);
        assert_eq!(p.id, s.to_string_lossy());
        assert_eq!(p.app, None);
        assert_eq!(kind_of(&p), "herdr");
        // Another app dir name under the root is not ours either.
        let q = place_of(&root(), &root().join("other").join("herdr.sock"));
        assert_eq!(q.app, None);
    }

    #[test]
    fn candidates_lists_root_and_sessions_of_existing_app_dirs() {
        let tmp = std::env::temp_dir().join(format!("deck-servers-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("herdr")).unwrap();
        std::fs::create_dir_all(tmp.join("deck-server").join("sessions").join("b")).unwrap();
        std::fs::create_dir_all(tmp.join("deck-server").join("sessions").join("a")).unwrap();
        std::fs::write(
            tmp.join("deck-server").join("sessions").join("stray.txt"),
            "",
        )
        .unwrap();
        let got = candidates(&tmp);
        let ids: Vec<String> = got.iter().map(|s| place_of(&tmp, s).id).collect();
        assert_eq!(
            ids,
            ["herdr", "deck-server", "deck-server/a", "deck-server/b"]
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn normalize_address_strips_pipe_prefix_and_quotes() {
        assert_eq!(
            normalize_address(r#"  "\\.\pipe\C:\x\herdr.sock" "#),
            Some(PathBuf::from(r"C:\x\herdr.sock"))
        );
        assert_eq!(
            normalize_address("/tmp/h.sock"),
            Some(PathBuf::from("/tmp/h.sock"))
        );
        assert_eq!(normalize_address("   "), None);
    }

    #[test]
    fn first_run_connects_the_default_server() {
        let p = Prefs::first_run(&root(), &root().join("herdr").join("herdr.sock"));
        assert_eq!(p.servers.len(), 1);
        assert_eq!(p.servers[0].id, "herdr");
        assert!(p.servers[0].connected);
        assert!(!p.servers[0].manual);
    }

    #[test]
    fn prefs_round_trip_and_missing_file() {
        let dir = std::env::temp_dir().join(format!("deck-prefs-test-{}", std::process::id()));
        let path = dir.join(PREFS_FILE);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(Prefs::load(&path), None);
        let mut p = Prefs::default();
        p.entry("deck-server/try", Path::new("/x/herdr.sock"), false)
            .connected = true;
        p.entry("manual", Path::new("/y.sock"), true).name = Some("box".into());
        p.save(&path).unwrap();
        assert_eq!(Prefs::load(&path), Some(p.clone()));
        // Entry is idempotent: the same id is not added twice.
        p.entry("manual", Path::new("/y.sock"), true);
        assert_eq!(p.servers.len(), 2);
        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(Prefs::load(&path), Some(Prefs::default()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn old_prefs_without_optional_fields_load() {
        let p: Prefs =
            serde_json::from_str(r#"{"servers":[{"id":"herdr","socket":"/h"}]}"#).unwrap();
        assert!(!p.servers[0].connected);
        assert_eq!(p.servers[0].name, None);
    }

    #[test]
    fn absent_server_is_not_offered_but_refusing_one_is() {
        let e = |k| herdr::HerdrError::Connect {
            path: "x".into(),
            source: std::io::Error::from(k),
            hint: "",
        };
        assert!(is_absent(&e(std::io::ErrorKind::NotFound)));
        assert!(is_absent(&e(std::io::ErrorKind::ConnectionRefused)));
        assert!(!is_absent(&e(std::io::ErrorKind::PermissionDenied)));
        assert!(!is_absent(&herdr::HerdrError::Timeout {
            method: "ping".into()
        }));
    }

    #[test]
    fn status_from_ping() {
        let ok = Status::from_ping(&Ok(json!({"version":"0.9.5","protocol":22})));
        assert_eq!(
            (ok.state, ok.version.as_deref(), ok.protocol),
            ("online", Some("0.9.5"), Some(22))
        );
        let bad = Status::from_ping(&Err(herdr::HerdrError::Timeout {
            method: "ping".into(),
        }));
        assert_eq!(bad.state, "offline");
        assert!(bad.error.unwrap().contains("timed out"));
    }
}
