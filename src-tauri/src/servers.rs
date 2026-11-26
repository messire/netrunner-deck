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
//! `deck-server/try`, and on another machine the same prefixed with its SSH
//! host (`dev@build-box.example.com/deck-server`); an address outside the known
//! dirs is its own id.
//!
//! A remote server is the same kind of row with an `ssh://` address
//! (`endpoint.rs`): the same runtime, event pump and calls. A machine is
//! added from the ADD dialog (user, host, port): CHECK lists its sockets over
//! SSH and pings each through that machine's bridge ([`Registry::probe`]),
//! then the checked ones are saved and connected ([`Registry::add_host`]).
//! From then on the host is scanned like the local dirs: listed and pinged
//! through its bridge, and only servers that answer are offered. A host that
//! cannot be reached shows as one row with the reason; removing it
//! ([`Registry::remove_host`]) drops all its servers and closes its bridge.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};

use crate::endpoint::{self, Endpoint, SshHost};
use crate::herdr;
use crate::runtime::{HerdrRuntime, SharedRuntime};

/// App dirs under the OS config root that hold Herdr-protocol servers.
pub const APP_DIRS: &[&str] = &["herdr", "herdr-dev", "deck-server", "deck-server-dev"];

const PREFS_FILE: &str = "preferences.json";
const PING_TIMEOUT: Duration = Duration::from_millis(1500);
/// A remote ping may first start that machine's bridge (SSH login included).
const REMOTE_PING_TIMEOUT: Duration = Duration::from_secs(25);
const SCAN_EVERY: Duration = Duration::from_secs(60);
pub const EVENT: &str = "deck://servers";

// ---- ids, names, discovery (pure) ------------------------------------------------

/// Where a socket sits relative to the config root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    pub id: String,
    pub app: Option<String>,
    pub session: Option<String>,
    /// The SSH host (`user@host`) of a server on another machine.
    pub host: Option<String>,
}

/// The id of the server at `socket`: `app` for `<root>/<app>/herdr.sock`,
/// `app/session` for `<root>/<app>/sessions/<session>/herdr.sock`, else the path.
pub fn place_of(root: &Path, socket: &Path) -> Place {
    let other = || Place {
        id: socket.to_string_lossy().into_owned(),
        app: None,
        session: None,
        host: None,
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
                host: None,
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
                host: None,
            };
        }
    }
    other()
}

/// The id of a server on another machine: the remote socket is placed like
/// a local one (`<app>/herdr.sock`, `<app>/sessions/<s>/herdr.sock`, any
/// config root), prefixed with the host; anything else is its address.
pub fn place_of_remote(host: &SshHost, socket: &str) -> Place {
    let key = host.key();
    let parts: Vec<&str> = socket.trim_end_matches('/').rsplit('/').collect();
    let known = |a: &str| APP_DIRS.contains(&a);
    let (app, session) = match parts.as_slice() {
        ["herdr.sock", app, ..] if known(app) => (Some(app.to_string()), None),
        ["herdr.sock", session, "sessions", app, ..] if known(app) => {
            (Some(app.to_string()), Some(session.to_string()))
        }
        _ => (None, None),
    };
    let id = match (&app, &session) {
        (Some(a), Some(s)) => format!("{key}/{a}/{s}"),
        (Some(a), None) => format!("{key}/{a}"),
        _ => format!("{key}:{socket}"),
    };
    Place {
        id,
        app,
        session,
        host: Some(key),
    }
}

/// [`place_of`] or [`place_of_remote`], whichever the endpoint is.
pub fn place_of_endpoint(root: &Path, endpoint: &Endpoint) -> Place {
    match endpoint {
        Endpoint::Local(p) => place_of(root, p),
        Endpoint::Ssh { host, socket } => place_of_remote(host, socket),
    }
}

/// `herdr` or `deck-server`, from the app dir; unknown places count as Herdr.
pub fn kind_of(place: &Place) -> &'static str {
    match &place.app {
        Some(app) if app.starts_with("deck-server") => "deck-server",
        _ => "herdr",
    }
}

/// Default display name: `herdr`, `herdr-dev · decktest`, the file path;
/// a remote one adds `@ host` (`deck-server @ dev@build-box.example.com`).
pub fn default_name(place: &Place) -> String {
    let base = match (&place.app, &place.session) {
        (Some(app), Some(session)) => format!("{app} · {session}"),
        (Some(app), None) => app.clone(),
        _ => return place.id.clone(),
    };
    match &place.host {
        Some(h) => format!("{base} @ {h}"),
        None => base,
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

/// Which of a host's servers a bare `user@host` connects: deck-server's root
/// session, else Herdr's, else the first one listed.
pub fn preferred_remote(sockets: &[String]) -> Option<&String> {
    let ends = |app: &str| {
        let tail = format!("/{app}/herdr.sock");
        sockets.iter().find(move |s| s.ends_with(&tail))
    };
    ends("deck-server")
        .or_else(|| ends("herdr"))
        .or_else(|| sockets.first())
}

/// A ping failure that means "nothing is listening there": not offered.
fn is_absent(err: &herdr::HerdrError) -> bool {
    matches!(err, herdr::HerdrError::Connect { source, .. }
        if matches!(source.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused))
}

type Pinged = Vec<(String, herdr::Result<Value>)>;

/// Ping each of `host`'s listed sockets through that machine's bridge, in
/// parallel (the same [`Endpoint::connect`] path every call takes).
fn ping_remote(host: &SshHost, sockets: Vec<String>) -> Pinged {
    let pings: Vec<_> = sockets
        .into_iter()
        .map(|socket| {
            let e = Endpoint::Ssh {
                host: host.clone(),
                socket: socket.clone(),
            };
            std::thread::spawn(move || {
                let r = herdr::call_within(&e, "ping", json!({}), REMOTE_PING_TIMEOUT);
                (socket, r)
            })
        })
        .collect();
    pings.into_iter().filter_map(|h| h.join().ok()).collect()
}

/// List `host`'s sockets over SSH and ping each through its bridge.
fn list_and_ping(host: &SshHost) -> Result<Pinged, String> {
    let sockets = endpoint::list_remote(host)?;
    Ok(ping_remote(host, sockets))
}

/// What one remote host shows: the servers that answered, or (`Err`) why
/// the machine cannot be used. Sockets with nothing behind them are left
/// out; when every listed socket failed otherwise (no bridge there, SSH
/// refused), the machine is unreachable with the first reason.
fn host_found(host: &SshHost, pinged: Result<Pinged, String>) -> Result<Vec<Found>, String> {
    let mut found = Vec::new();
    let mut why: Option<String> = None;
    for (socket, result) in pinged? {
        match &result {
            Ok(_) => {
                let endpoint = Endpoint::Ssh {
                    host: host.clone(),
                    socket,
                };
                found.push(Found {
                    place: place_of_endpoint(Path::new(""), &endpoint),
                    status: Status::from_ping(&result),
                    endpoint,
                });
            }
            // The bridge's own failure (ssh refused) is not "nothing there".
            Err(e) if is_absent(e) && !e.to_string().contains("bridge on ") => {}
            Err(e) => {
                why.get_or_insert_with(|| e.to_string());
            }
        }
    }
    match why {
        Some(why) if found.is_empty() => Err(why),
        _ => Ok(found),
    }
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
    /// The address (`endpoint.rs`): a local socket path as before, or an
    /// `ssh://` URL. The field keeps its old name so older files load.
    pub socket: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default)]
    pub connected: bool,
    #[serde(default)]
    pub manual: bool,
}

impl SavedServer {
    pub fn endpoint(&self) -> Option<Endpoint> {
        Endpoint::parse(&self.socket)
    }
}

/// How an endpoint is written to `preferences.json`: a local server by its
/// path (as Deck always did), a remote one by its URL.
fn stored(endpoint: &Endpoint) -> String {
    match endpoint {
        Endpoint::Local(p) => p.to_string_lossy().into_owned(),
        Endpoint::Ssh { .. } => endpoint.address(),
    }
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
                socket: default_socket.to_string_lossy().into_owned(),
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

    fn entry(&mut self, id: &str, endpoint: &Endpoint, manual: bool) -> &mut SavedServer {
        if let Some(i) = self.servers.iter().position(|s| s.id == id) {
            return &mut self.servers[i];
        }
        self.servers.push(SavedServer {
            id: id.to_string(),
            socket: stored(endpoint),
            name: None,
            connected: false,
            manual,
        });
        self.servers.last_mut().expect("just pushed")
    }

    /// Drop every saved server on `host`; returns their ids.
    pub fn remove_host(&mut self, host: &SshHost) -> Vec<String> {
        let on_host = |s: &SavedServer| matches!(s.endpoint(), Some(Endpoint::Ssh { host: h, .. }) if &h == host);
        let ids = self
            .servers
            .iter()
            .filter(|s| on_host(s))
            .map(|s| s.id.clone())
            .collect();
        self.servers.retain(|s| !on_host(s));
        ids
    }

    /// The SSH hosts of the saved servers: each is scanned for more servers.
    pub fn remote_hosts(&self) -> Vec<SshHost> {
        let mut hosts: Vec<SshHost> = Vec::new();
        for s in &self.servers {
            if let Some(Endpoint::Ssh { host, .. }) = s.endpoint() {
                if !hosts.contains(&host) {
                    hosts.push(host);
                }
            }
        }
        hosts
    }
}

// ---- registry ---------------------------------------------------------------------

/// Link state of a connected server, or the ping result of a found one.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Status {
    /// `connecting` | `online` | `offline`; empty for a remote server that
    /// was listed but not pinged (it is not connected).
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
    endpoint: Endpoint,
    status: Status,
}

struct Live {
    rt: SharedRuntime,
    endpoint: Endpoint,
    status: Status,
    stop: Arc<AtomicBool>,
}

#[derive(Default)]
struct State {
    prefs: Prefs,
    found: Vec<Found>,
    scanned: bool,
    live: HashMap<String, Live>,
    /// Saved SSH hosts the last scan could not use, with the reason.
    unreachable: Vec<(SshHost, String)>,
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
    /// On another machine (SSH).
    pub remote: bool,
    /// That machine (`user@host[:port]`), the key REMOVE takes.
    pub machine: Option<String>,
    /// The server's session when it is not its app's default one (`try` of
    /// `deck-server/try`), so two sessions on one machine never look alike.
    pub session: Option<String>,
    pub status: Status,
}

/// A saved machine the last scan could not reach (one grey row).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MachineDown {
    pub machine: String,
    pub error: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ServersView {
    pub servers: Vec<ServerView>,
    pub scanned: bool,
    pub unreachable: Vec<MachineDown>,
}

/// One server CHECK found on a machine, answering through its bridge.
#[derive(Debug, Clone, Serialize)]
pub struct Probed {
    pub id: String,
    pub name: String,
    pub kind: &'static str,
    pub socket: String,
    pub version: Option<String>,
    pub protocol: Option<u64>,
    /// Pre-checked in the dialog: deck-server's root session, else Herdr's.
    pub preferred: bool,
    /// Already connected in this Deck.
    pub connected: bool,
}

fn machine_of(endpoint: &Endpoint) -> Option<String> {
    match endpoint {
        Endpoint::Ssh { host, .. } => Some(host.key()),
        Endpoint::Local(_) => None,
    }
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
                    .entry(&place.id, &Endpoint::Local(socket), place.app.is_none())
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
            match s.endpoint() {
                Some(e) => reg.attach(app, &s.id, e),
                None => {
                    tracing::warn!(id = %s.id, address = %s.socket, "saved server has no usable address")
                }
            }
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
            .unwrap_or_else(|| Arc::new(HerdrRuntime::local(herdr::socket_path())))
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

    /// Whether `server` is on another machine.
    pub fn is_remote(&self, server: &str) -> bool {
        self.lock()
            .live
            .get(server)
            .map(|l| l.endpoint.is_remote())
            .unwrap_or(false)
    }

    /// The session dir of `server` on its own machine (where its
    /// `session.json` is), as that machine spells it: the archive asks the
    /// machine to read it there.
    pub fn session_dir(&self, server: &str) -> Option<String> {
        let st = self.lock();
        match &st.live.get(server)?.endpoint {
            Endpoint::Local(p) => p.parent().map(|d| d.to_string_lossy().into_owned()),
            Endpoint::Ssh { socket, .. } => socket
                .rsplit_once('/')
                .map(|(dir, _)| dir.to_string())
                .filter(|d| !d.is_empty()),
        }
    }

    pub fn view(&self) -> ServersView {
        let st = self.lock();
        let mut rows: Vec<ServerView> = Vec::new();
        for s in &st.prefs.servers {
            let Some(endpoint) = s.endpoint() else {
                continue;
            };
            let place = place_of_endpoint(&self.root, &endpoint);
            let found = st.found.iter().find(|f| f.place.id == s.id);
            let status = match st.live.get(&s.id) {
                Some(l) => l.status.clone(),
                None => found.map(|f| f.status.clone()).unwrap_or_default(),
            };
            // A server that is neither connected nor found any more is not
            // offered; on another machine only the ones that answered are.
            if !s.connected && found.is_none() && (!s.manual || endpoint.is_remote()) {
                continue;
            }
            rows.push(ServerView {
                id: s.id.clone(),
                name: s.name.clone().unwrap_or_else(|| default_name(&place)),
                kind: kind_of(&place),
                address: endpoint.address(),
                connected: s.connected,
                manual: s.manual,
                found: found.is_some(),
                new: false,
                remote: endpoint.is_remote(),
                machine: machine_of(&endpoint),
                session: place.session.clone(),
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
                address: f.endpoint.address(),
                connected: false,
                manual: false,
                found: true,
                new: true,
                remote: f.endpoint.is_remote(),
                machine: machine_of(&f.endpoint),
                session: f.place.session.clone(),
                status: f.status.clone(),
            });
        }
        ServersView {
            servers: rows,
            scanned: st.scanned,
            unreachable: st
                .unreachable
                .iter()
                .map(|(h, error)| MachineDown {
                    machine: h.key(),
                    error: error.clone(),
                })
                .collect(),
        }
    }

    /// Ping every local candidate and saved local address, and list the
    /// servers on every saved SSH host (all in parallel); keep the ones that
    /// answer, exist but refuse us (an elevated server), or are listed on a
    /// host. A host that cannot be reached keeps what it had: its rows are
    /// saved anyway. Emits [`EVENT`] on change.
    pub fn scan(&self, app: &AppHandle) {
        let mut sockets = candidates(&self.root);
        let (saved, hosts) = {
            let st = self.lock();
            (st.prefs.servers.clone(), st.prefs.remote_hosts())
        };
        // Manual and saved addresses are pinged too, so their rows have a status.
        for s in &saved {
            if let Some(Endpoint::Local(p)) = s.endpoint() {
                if !sockets.contains(&p) {
                    sockets.push(p);
                }
            }
        }
        let pings: Vec<_> = sockets
            .into_iter()
            .map(|socket| {
                std::thread::spawn(move || {
                    let e = Endpoint::Local(socket);
                    let r = herdr::call_within(&e, "ping", json!({}), PING_TIMEOUT);
                    (e, r)
                })
            })
            .collect();
        let listings: Vec<_> = hosts
            .into_iter()
            .map(|host| std::thread::spawn(move || (host_found(&host, list_and_ping(&host)), host)))
            .collect();
        let mut found = Vec::new();
        for h in pings {
            let Ok((endpoint, result)) = h.join() else {
                continue;
            };
            if matches!(&result, Err(e) if is_absent(e)) {
                continue;
            }
            found.push(Found {
                place: place_of_endpoint(&self.root, &endpoint),
                status: Status::from_ping(&result),
                endpoint,
            });
        }
        let mut unreachable: Vec<(SshHost, String)> = Vec::new();
        for h in listings {
            let Ok((result, host)) = h.join() else {
                continue;
            };
            match result {
                Ok(remote) => found.extend(remote),
                Err(err) => {
                    tracing::info!(host = %host.key(), %err, "remote scan failed");
                    unreachable.push((host, err));
                }
            }
        }
        if self.apply_scan(found, unreachable) {
            let _ = app.emit(EVENT, "changed");
        }
    }

    /// Replace the last scan's result (nothing of an earlier scan is kept);
    /// `true` when the list changed. A host removed while the scan ran
    /// stays removed.
    fn apply_scan(&self, mut found: Vec<Found>, mut unreachable: Vec<(SshHost, String)>) -> bool {
        let mut st = self.lock();
        let hosts = st.prefs.remote_hosts();
        found.retain(|f| match &f.endpoint {
            Endpoint::Ssh { host, .. } => hosts.contains(host),
            Endpoint::Local(_) => true,
        });
        unreachable.retain(|(h, _)| hosts.contains(h));
        let ids = |v: &[Found]| -> Vec<(String, Status)> {
            v.iter()
                .map(|f| (f.place.id.clone(), f.status.clone()))
                .collect()
        };
        let changed = ids(&st.found) != ids(&found) || st.unreachable != unreachable;
        st.found = found;
        st.unreachable = unreachable;
        let first = !st.scanned;
        st.scanned = true;
        first || changed
    }

    fn save(&self, st: &State) -> Result<(), String> {
        st.prefs.save(&self.prefs_path)
    }

    fn endpoint_of(&self, st: &State, id: &str) -> Option<(Endpoint, bool)> {
        st.prefs
            .get(id)
            .and_then(|s| Some((s.endpoint()?, s.manual)))
            .or_else(|| {
                st.found
                    .iter()
                    .find(|f| f.place.id == id)
                    .map(|f| (f.endpoint.clone(), false))
            })
    }

    pub fn connect(&self, app: &AppHandle, id: &str) -> Result<(), String> {
        let endpoint = {
            let mut st = self.lock();
            let (endpoint, manual) = self
                .endpoint_of(&st, id)
                .ok_or_else(|| format!("unknown server {id}"))?;
            st.prefs.entry(id, &endpoint, manual).connected = true;
            self.save(&st)?;
            endpoint
        };
        self.attach(app, id, endpoint);
        Ok(())
    }

    pub fn disconnect(&self, app: &AppHandle, id: &str) -> Result<(), String> {
        let mut st = self.lock();
        if let Some(s) = st.prefs.servers.iter_mut().find(|s| s.id == id) {
            s.connected = false;
        }
        let live = st.live.remove(id);
        self.save(&st)?;
        drop(st);
        if let Some(live) = live {
            live.stop.store(true, Ordering::SeqCst);
            // A remote machine's bridge goes when its last server does; this
            // machine's stays (discovery uses it too).
            let machine = live.endpoint.machine();
            let in_use = self
                .lock()
                .live
                .values()
                .any(|l| l.endpoint.machine() == machine);
            if machine != endpoint::Machine::Local && !in_use {
                endpoint::close(&machine);
            }
        }
        let _ = app.emit(EVENT, "changed");
        Ok(())
    }

    pub fn rename(&self, app: &AppHandle, id: &str, name: &str) -> Result<(), String> {
        let mut st = self.lock();
        let (endpoint, manual) = self
            .endpoint_of(&st, id)
            .ok_or_else(|| format!("unknown server {id}"))?;
        let name = name.trim();
        st.prefs.entry(id, &endpoint, manual).name = (!name.is_empty()).then(|| name.to_string());
        self.save(&st)?;
        drop(st);
        let _ = app.emit(EVENT, "changed");
        Ok(())
    }

    /// Add an address by hand and connect it. Returns its id. A bare SSH
    /// host (`user@host`, `ssh://user@host`) is asked for its servers first
    /// (blocking: one SSH session); the preferred one is connected and the
    /// others are offered like found local ones.
    pub fn add(&self, app: &AppHandle, address: &str) -> Result<String, String> {
        let endpoint = match Endpoint::parse_host(address) {
            Some(host) => {
                let sockets = endpoint::list_remote(&host)?;
                let socket = preferred_remote(&sockets).cloned().ok_or_else(|| {
                    format!("no Herdr or deck-server is running on {}", host.key())
                })?;
                let mut st = self.lock();
                for s in &sockets {
                    let e = Endpoint::Ssh {
                        host: host.clone(),
                        socket: s.clone(),
                    };
                    let place = place_of_endpoint(&self.root, &e);
                    if !st.found.iter().any(|f| f.place.id == place.id) {
                        st.found.push(Found {
                            place,
                            endpoint: e,
                            status: Status::default(),
                        });
                    }
                }
                Endpoint::Ssh { host, socket }
            }
            None => Endpoint::parse(address).ok_or("empty address")?,
        };
        let place = place_of_endpoint(&self.root, &endpoint);
        {
            let mut st = self.lock();
            // A remote server stays in the list (its host is scanned through
            // it) even when it is not in a known app dir.
            let manual = place.app.is_none() || endpoint.is_remote();
            st.prefs.entry(&place.id, &endpoint, manual).connected = true;
            self.save(&st)?;
        }
        self.attach(app, &place.id, endpoint);
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

    /// CHECK in the ADD dialog: list `host`'s sockets over SSH and ping each
    /// through that machine's bridge. Only servers that answer come back.
    /// Blocking (one SSH session, then the bridge).
    pub fn probe(&self, host: &SshHost) -> Result<Vec<Probed>, String> {
        let found = host_found(host, list_and_ping(host))?;
        self.probed(host, found)
    }

    fn probed(&self, host: &SshHost, found: Vec<Found>) -> Result<Vec<Probed>, String> {
        let socket_of = |f: &Found| match &f.endpoint {
            Endpoint::Ssh { socket, .. } => socket.clone(),
            Endpoint::Local(p) => p.to_string_lossy().into_owned(),
        };
        let sockets: Vec<String> = found.iter().map(socket_of).collect();
        let preferred = preferred_remote(&sockets)
            .cloned()
            .ok_or_else(|| format!("no Herdr or deck-server is running on {}", host.key()))?;
        let st = self.lock();
        Ok(found
            .into_iter()
            .map(|f| {
                let socket = socket_of(&f);
                Probed {
                    name: st
                        .prefs
                        .get(&f.place.id)
                        .and_then(|s| s.name.clone())
                        .unwrap_or_else(|| {
                            // The dialog names the machine already.
                            default_name(&Place {
                                host: None,
                                ..f.place.clone()
                            })
                        }),
                    kind: kind_of(&f.place),
                    preferred: socket == preferred,
                    connected: st.live.contains_key(&f.place.id),
                    version: f.status.version.clone(),
                    protocol: f.status.protocol,
                    id: f.place.id,
                    socket,
                }
            })
            .collect())
    }

    /// Save `host` and mark the servers at `sockets` on it connected (no
    /// runtime yet); returns their ids and endpoints.
    fn save_host(
        &self,
        host: &SshHost,
        sockets: &[String],
    ) -> Result<Vec<(String, Endpoint)>, String> {
        if sockets.is_empty() {
            return Err("no server is checked".into());
        }
        if let Some(bad) = sockets.iter().find(|s| !s.starts_with('/')) {
            return Err(format!("{bad:?} is not a socket path"));
        }
        let mut st = self.lock();
        let mut added = Vec::new();
        for socket in sockets {
            let endpoint = Endpoint::Ssh {
                host: host.clone(),
                socket: socket.clone(),
            };
            let place = place_of_endpoint(&self.root, &endpoint);
            st.prefs.entry(&place.id, &endpoint, true).connected = true;
            added.push((place.id, endpoint));
        }
        st.unreachable.retain(|(h, _)| h != host);
        self.save(&st)?;
        Ok(added)
    }

    /// ADD in the dialog: save `host` and connect the servers at `sockets`
    /// on it. From now on the host is scanned too. Returns their ids.
    pub fn add_host(
        &self,
        app: &AppHandle,
        host: &SshHost,
        sockets: &[String],
    ) -> Result<Vec<String>, String> {
        let added = self.save_host(host, sockets)?;
        for (id, endpoint) in &added {
            self.attach(app, id, endpoint.clone());
        }
        // Offer the host's other servers right away.
        let app2 = app.clone();
        std::thread::spawn(move || app2.state::<Registry>().scan(&app2));
        Ok(added.into_iter().map(|(id, _)| id).collect())
    }

    /// Forget `machine` (`user@host[:port]`) in the registry: its saved
    /// servers, found rows and unreachable row go; returns the host and the
    /// runtimes that were connected to it.
    fn drop_host(&self, machine: &str) -> Result<(SshHost, Vec<Live>), String> {
        let mut st = self.lock();
        let host = st
            .prefs
            .remote_hosts()
            .into_iter()
            .chain(st.unreachable.iter().map(|(h, _)| h.clone()))
            .chain(st.found.iter().filter_map(|f| match &f.endpoint {
                Endpoint::Ssh { host, .. } => Some(host.clone()),
                Endpoint::Local(_) => None,
            }))
            .find(|h| h.key() == machine)
            .ok_or_else(|| format!("unknown machine {machine}"))?;
        st.prefs.remove_host(&host);
        st.found
            .retain(|f| !matches!(&f.endpoint, Endpoint::Ssh { host: h, .. } if h == &host));
        st.unreachable.retain(|(h, _)| h != &host);
        let on_host = endpoint::Machine::Ssh(host.clone());
        let ids: Vec<String> = st
            .live
            .iter()
            .filter(|(_, l)| l.endpoint.machine() == on_host)
            .map(|(id, _)| id.clone())
            .collect();
        let lives = ids.iter().filter_map(|id| st.live.remove(id)).collect();
        self.save(&st)?;
        Ok((host, lives))
    }

    /// REMOVE a machine: every server on it leaves preferences and the
    /// list, connected ones are disconnected and its bridge is closed; the
    /// host is not scanned any more.
    pub fn remove_host(&self, app: &AppHandle, machine: &str) -> Result<(), String> {
        let (host, lives) = self.drop_host(machine)?;
        for l in lives {
            l.stop.store(true, Ordering::SeqCst);
        }
        endpoint::close(&endpoint::Machine::Ssh(host));
        let _ = app.emit(EVENT, "changed");
        Ok(())
    }

    /// The user saw the found servers: they stop being "new".
    pub fn mark_seen(&self, app: &AppHandle) -> Result<(), String> {
        let mut st = self.lock();
        let fresh: Vec<(String, Endpoint)> = st
            .found
            .iter()
            .filter(|f| st.prefs.get(&f.place.id).is_none())
            .map(|f| (f.place.id.clone(), f.endpoint.clone()))
            .collect();
        if fresh.is_empty() {
            return Ok(());
        }
        for (id, endpoint) in fresh {
            st.prefs.entry(&id, &endpoint, false);
        }
        self.save(&st)?;
        drop(st);
        let _ = app.emit(EVENT, "changed");
        Ok(())
    }

    /// Start the runtime and event pump of a server (no-op when running).
    fn attach(&self, app: &AppHandle, id: &str, endpoint: Endpoint) {
        let stop = Arc::new(AtomicBool::new(false));
        let rt: SharedRuntime = Arc::new(HerdrRuntime::new(endpoint.clone()));
        {
            let mut st = self.lock();
            if st.live.contains_key(id) {
                return;
            }
            st.live.insert(
                id.to_string(),
                Live {
                    rt: rt.clone(),
                    endpoint,
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

/// Async: a bare SSH host is listed over SSH first, which takes a moment.
#[tauri::command]
pub async fn server_add(app: AppHandle, address: String) -> Result<ServersView, String> {
    crate::blocking(move || {
        app.state::<Registry>().add(&app, &address)?;
        Ok(view_of(&app))
    })
    .await
}

/// CHECK in the ADD dialog. Async: an SSH listing, then pings through the bridge.
#[tauri::command]
pub async fn server_probe(
    app: AppHandle,
    user: String,
    host: String,
    port: u32,
) -> Result<Vec<Probed>, String> {
    let host = SshHost::from_fields(&user, &host, port)?;
    crate::blocking(move || app.state::<Registry>().probe(&host)).await
}

#[tauri::command]
pub fn server_add_host(
    app: AppHandle,
    user: String,
    host: String,
    port: u32,
    sockets: Vec<String>,
) -> Result<ServersView, String> {
    let host = SshHost::from_fields(&user, &host, port)?;
    app.state::<Registry>().add_host(&app, &host, &sockets)?;
    Ok(view_of(&app))
}

#[tauri::command]
pub fn server_remove_host(app: AppHandle, machine: String) -> Result<ServersView, String> {
    app.state::<Registry>().remove_host(&app, &machine)?;
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
    fn remote_places_mirror_local_ones_under_the_host() {
        let host = Endpoint::parse_host("dev@build-box.example.com").unwrap();
        let p = place_of_remote(&host, "/home/dev/.config/deck-server/herdr.sock");
        assert_eq!(p.id, "dev@build-box.example.com/deck-server");
        assert_eq!(kind_of(&p), "deck-server");
        assert_eq!(default_name(&p), "deck-server @ dev@build-box.example.com");
        let q = place_of_remote(&host, "/srv/x/.config/herdr/sessions/try/herdr.sock");
        assert_eq!(q.id, "dev@build-box.example.com/herdr/try");
        assert_eq!(default_name(&q), "herdr · try @ dev@build-box.example.com");
        let r = place_of_remote(&host, "/tmp/odd.sock");
        assert_eq!(r.id, "dev@build-box.example.com:/tmp/odd.sock");
        assert_eq!(r.app, None);
    }

    #[test]
    fn preferred_remote_is_deck_server_then_herdr() {
        let v = |xs: &[&str]| xs.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let all = v(&[
            "/h/.config/herdr/herdr.sock",
            "/h/.config/deck-server/sessions/a/herdr.sock",
            "/h/.config/deck-server/herdr.sock",
        ]);
        assert_eq!(
            preferred_remote(&all).unwrap(),
            "/h/.config/deck-server/herdr.sock"
        );
        let herdr = v(&[
            "/h/.config/deck-server/sessions/a/herdr.sock",
            "/h/.config/herdr/herdr.sock",
        ]);
        assert_eq!(
            preferred_remote(&herdr).unwrap(),
            "/h/.config/herdr/herdr.sock"
        );
        let odd = v(&["/h/.config/deck-server/sessions/a/herdr.sock"]);
        assert_eq!(preferred_remote(&odd).unwrap(), &odd[0]);
        assert_eq!(preferred_remote(&[]), None);
    }

    #[test]
    fn saved_servers_keep_local_paths_and_store_remote_urls() {
        let mut p = Prefs::default();
        let local = Endpoint::Local(PathBuf::from("/x/herdr.sock"));
        let remote = Endpoint::parse("ssh://u@h/home/u/.config/deck-server/herdr.sock").unwrap();
        p.entry("deck-server/try", &local, false);
        p.entry("u@h/deck-server", &remote, true);
        assert_eq!(p.servers[0].socket, "/x/herdr.sock");
        assert_eq!(p.servers[0].endpoint(), Some(local));
        assert_eq!(
            p.servers[1].socket,
            "u@h:/home/u/.config/deck-server/herdr.sock"
        );
        assert_eq!(p.servers[1].endpoint(), Some(remote));
        assert_eq!(p.remote_hosts(), vec![Endpoint::parse_host("u@h").unwrap()]);
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
        p.entry(
            "deck-server/try",
            &Endpoint::Local("/x/herdr.sock".into()),
            false,
        )
        .connected = true;
        p.entry("manual", &Endpoint::Local("/y.sock".into()), true)
            .name = Some("box".into());
        p.save(&path).unwrap();
        assert_eq!(Prefs::load(&path), Some(p.clone()));
        // Entry is idempotent: the same id is not added twice.
        p.entry("manual", &Endpoint::Local("/y.sock".into()), true);
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

    /// Live check against the servers running on this machine:
    /// `cargo test live_discovery -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live_discovery_and_two_runtimes() {
        use crate::runtime::Runtime;
        let root = herdr::config_root();
        let mut answered = 0;
        for socket in candidates(&root) {
            let place = place_of(&root, &socket);
            let r = herdr::call_within(
                &Endpoint::Local(socket.clone()),
                "ping",
                json!({}),
                PING_TIMEOUT,
            );
            if matches!(&r, Err(e) if is_absent(e)) {
                continue;
            }
            let st = Status::from_ping(&r);
            let ws = HerdrRuntime::local(&socket)
                .workspace_list()
                .map(|v| v["workspaces"].as_array().map_or(0, Vec::len));
            println!(
                "{:<24} {:<12} {:?} {:?} workspaces={:?}",
                place.id,
                kind_of(&place),
                st.state,
                st.version,
                ws
            );
            if st.state == "online" {
                answered += 1;
            }
        }
        assert!(answered >= 1, "no server answered");
    }

    fn registry(tag: &str) -> (Registry, PathBuf) {
        let dir = std::env::temp_dir().join(format!("deck-reg-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let reg = Registry {
            prefs_path: dir.join(PREFS_FILE),
            root: root(),
            state: Mutex::new(State::default()),
        };
        (reg, dir)
    }

    fn absent() -> herdr::HerdrError {
        herdr::HerdrError::Connect {
            path: "x".into(),
            source: std::io::Error::from(std::io::ErrorKind::NotFound),
            hint: "",
        }
    }

    fn no_bridge() -> herdr::HerdrError {
        herdr::HerdrError::Io(std::io::Error::other(
            "bridge on dev@build-box.example.com:2222: no deck-server with a bridge there",
        ))
    }

    const DS: &str = "/home/dev/.config/deck-server/herdr.sock";
    const HD: &str = "/home/dev/.config/herdr/herdr.sock";

    #[test]
    fn a_host_shows_only_the_servers_that_answer() {
        let host = SshHost::from_fields("dev", "build-box.example.com", 2222).unwrap();
        let ok = || Ok(json!({"version":"0.4.0","protocol":22}));
        let got = host_found(
            &host,
            Ok(vec![(DS.into(), ok()), (HD.into(), Err(absent()))]),
        )
        .unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(
            got[0].place.id,
            "dev@build-box.example.com:2222/deck-server"
        );
        assert_eq!(got[0].status.version.as_deref(), Some("0.4.0"));
        // One answers, another fails otherwise: the machine is usable.
        let mixed = host_found(
            &host,
            Ok(vec![(DS.into(), ok()), (HD.into(), Err(no_bridge()))]),
        );
        assert_eq!(mixed.unwrap().len(), 1);
        // Stale sockets only: reachable, nothing running.
        assert!(host_found(&host, Ok(vec![(DS.into(), Err(absent()))]))
            .unwrap()
            .is_empty());
        let ssh_refused = herdr::HerdrError::Connect {
            path: "x".into(),
            source: std::io::Error::new(
                std::io::ErrorKind::ConnectionRefused,
                "bridge on dev@build-box.example.com:2222: Permission denied (publickey).",
            ),
            hint: "",
        };
        let err = host_found(&host, Ok(vec![(DS.into(), Err(ssh_refused))])).unwrap_err();
        assert!(err.contains("publickey"), "{err}");
        // No bridge there / SSH refused: unreachable, with the reason.
        let err = host_found(&host, Ok(vec![(DS.into(), Err(no_bridge()))])).unwrap_err();
        assert!(err.contains("no deck-server with a bridge"), "{err}");
        let err = host_found(
            &host,
            Err("ssh dev@build-box.example.com:2222: no answer within 20s".into()),
        )
        .unwrap_err();
        assert!(err.contains("no answer"), "{err}");
    }

    #[test]
    fn removing_a_host_prunes_all_its_saved_servers() {
        let a = SshHost::from_fields("dev", "build-box.example.com", 2222).unwrap();
        let b = SshHost::from_fields("dev", "old-box.example.com", 22).unwrap();
        let on = |h: &SshHost, s: &str| Endpoint::Ssh {
            host: h.clone(),
            socket: s.into(),
        };
        let mut p = Prefs::default();
        p.entry("local", &Endpoint::Local("/x/herdr.sock".into()), true);
        p.entry("a1", &on(&a, DS), true).connected = true;
        p.entry("a2", &on(&a, HD), false);
        p.entry("b1", &on(&b, DS), true);
        assert_eq!(p.remove_host(&a), ["a1", "a2"]);
        let ids: Vec<&str> = p.servers.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["local", "b1"]);
        assert_eq!(p.remote_hosts(), vec![b]);
    }

    #[test]
    fn an_unreachable_host_keeps_no_rows_but_one_machine_row() {
        let (reg, dir) = registry("unreach");
        let host = SshHost::from_fields("dev", "old-box.example.com", 2222).unwrap();
        let added = reg.save_host(&host, &[DS.to_string()]).unwrap();
        assert_eq!(added[0].0, "dev@old-box.example.com:2222/deck-server");
        // The first scan finds both servers, the user leaves one unconnected.
        let ok = || Ok(json!({"version":"0.4.0"}));
        let found = host_found(&host, Ok(vec![(DS.into(), ok()), (HD.into(), ok())])).unwrap();
        assert!(reg.apply_scan(found, vec![]));
        let v = reg.view();
        assert_eq!(v.servers.len(), 2);
        assert!(v.unreachable.is_empty());
        assert_eq!(
            v.servers[0].machine.as_deref(),
            Some("dev@old-box.example.com:2222")
        );
        // The machine goes away: its found rows are not kept.
        assert!(reg.apply_scan(vec![], vec![(host.clone(), "ssh: no answer".into())]));
        let v = reg.view();
        let ids: Vec<&str> = v.servers.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(
            ids,
            ["dev@old-box.example.com:2222/deck-server"],
            "only the connected one stays"
        );
        assert_eq!(
            v.unreachable,
            vec![MachineDown {
                machine: "dev@old-box.example.com:2222".into(),
                error: "ssh: no answer".into()
            }]
        );
        assert!(!reg.apply_scan(vec![], vec![(host.clone(), "ssh: no answer".into())]));
        // REMOVE: every trace of the machine goes, saved too.
        let (gone, lives) = reg.drop_host("dev@old-box.example.com:2222").unwrap();
        assert_eq!(gone, host);
        assert!(lives.is_empty());
        let v = reg.view();
        assert!(v.servers.is_empty() && v.unreachable.is_empty());
        let saved = Prefs::load(&reg.prefs_path).unwrap();
        assert!(saved.remote_hosts().is_empty());
        // A scan that was under way when it was removed does not bring it back.
        reg.apply_scan(vec![], vec![(host, "late".into())]);
        assert!(reg.view().unreachable.is_empty());
        assert!(reg.drop_host("dev@old-box.example.com:2222").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn probe_prechecks_the_preferred_server() {
        let (reg, _dir) = registry("probe");
        let host = SshHost::from_fields("dev", "build-box.example.com", 22).unwrap();
        let ok = || Ok(json!({"version":"0.9.5","protocol":22}));
        let found = host_found(&host, Ok(vec![(HD.into(), ok()), (DS.into(), ok())])).unwrap();
        let got = reg.probed(&host, found).unwrap();
        let pre: Vec<(&str, &str, bool)> = got
            .iter()
            .map(|p| (p.id.as_str(), p.kind, p.preferred))
            .collect();
        assert_eq!(
            pre,
            [
                ("dev@build-box.example.com/herdr", "herdr", false),
                ("dev@build-box.example.com/deck-server", "deck-server", true)
            ]
        );
        assert!(reg
            .probed(&host, vec![])
            .unwrap_err()
            .contains("no Herdr or deck-server"));
        assert!(reg.save_host(&host, &[]).is_err());
        assert!(reg.save_host(&host, &["relative.sock".into()]).is_err());
    }

    /// CHECK against a real machine: `cargo test live_probe -- --ignored
    /// --nocapture` with `DECK_TEST_SSH_USER`, `DECK_TEST_SSH_HOST` and
    /// optionally `DECK_TEST_SSH_PORT` set. Lists and pings through the bridge.
    #[test]
    #[ignore]
    fn live_probe() {
        let (Ok(user), Ok(host)) = (
            std::env::var("DECK_TEST_SSH_USER"),
            std::env::var("DECK_TEST_SSH_HOST"),
        ) else {
            return;
        };
        let port = std::env::var("DECK_TEST_SSH_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(22);
        let host = SshHost::from_fields(&user, &host, port).expect("fields");
        let (reg, _dir) = registry("live");
        let started = std::time::Instant::now();
        let got = reg.probe(&host);
        println!("{:?} in {:?}", got, started.elapsed());
        assert!(!got.expect("probe").is_empty());
        endpoint::close_all();
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
