//! Where a Herdr-protocol server is and how a connection to it is opened.
//!
//! Deck has exactly one way to connect to a server, local or remote: a
//! channel through a `deck-server bridge` running on the server's machine
//! (`bridge.rs`). The bridge opens the server's API socket there and the
//! channel carries Herdr's newline-delimited JSON API unchanged (one request
//! per connection, `events.subscribe` keeps its connection).
//!
//! The address only says which machine's bridge, and which socket on it:
//!
//! * a **local** address is the server's socket path (shown on Windows as
//!   the named pipe `\\.\pipe\<path>`): the bridge is the same bridge code
//!   running inside Deck;
//! * an **ssh** address, `ssh://[user@]host[:port]/<absolute socket path>`:
//!   the bridge is `ssh host deck-server bridge`, with the user's own
//!   OpenSSH setup (keys, agent, `~/.ssh/config`, `known_hosts`).
//!
//! One bridge per machine carries every connection to every server on it.
//! Everything above this module (calls, deadlines, the event stream, the
//! registry) sees only [`Endpoint`] and [`Stream`].

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use crate::bridge::{Link, StartError};

/// How long a bridge may take to come up (SSH authentication included).
const BRIDGE_START: Duration = Duration::from_secs(20);
/// After a bridge failed to start, wait this long before trying again so a
/// down host is not dialled for every call.
const BRIDGE_RETRY: Duration = Duration::from_secs(10);
/// One remote listing of server sockets (discovery).
const LIST_TIMEOUT: Duration = Duration::from_secs(20);

/// The machine part of a local address.
const LOCAL: &str = "local:";

/// A connection to a server: a channel of its machine's bridge.
pub type Stream = crate::bridge::Channel;

/// The app dirs a server can live in, on any machine (`servers::APP_DIRS`).
use crate::servers::APP_DIRS;

/// An SSH destination: `[user@]host[:port]`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SshHost {
    pub user: Option<String>,
    pub host: String,
    pub port: Option<u16>,
}

impl SshHost {
    /// The host from the ADD dialog's fields. Port 22 is stored as no port,
    /// so the machine keeps the id it has when typed as `user@host`.
    pub fn from_fields(user: &str, host: &str, port: u32) -> Result<SshHost, String> {
        let (user, host) = (user.trim(), host.trim());
        if user.is_empty() {
            return Err("user is empty".into());
        }
        if user.contains(['@', '/', '\\', ':'])
            || user.contains(char::is_whitespace)
            || user.starts_with('-')
        {
            return Err(format!("{user:?} is not a user name"));
        }
        if host.is_empty() {
            return Err("host is empty".into());
        }
        let port = u16::try_from(port)
            .ok()
            .filter(|p| *p != 0)
            .ok_or_else(|| format!("port {port} is not 1..65535"))?;
        let bare = host.trim_start_matches('[').trim_end_matches(']');
        // A `:` is only part of an IPv6 address; a port goes in its own field.
        if bare.contains(':') && bare.parse::<std::net::Ipv6Addr>().is_err() {
            return Err(format!(
                "{host:?} is not a host name or address (the port has its own field)"
            ));
        }
        let typed = if bare.contains(':') {
            format!("[{bare}]")
        } else {
            bare.to_string()
        };
        match split_host_path(&format!("{user}@{typed}")) {
            Some((h, None)) if h.port.is_none() => Ok(SshHost {
                port: (port != 22).then_some(port),
                ..h
            }),
            _ => Err(format!("{host:?} is not a host name or address")),
        }
    }

    /// `user@host:port` as typed (port only when given): the host part of ids.
    pub fn key(&self) -> String {
        let mut s = String::new();
        if let Some(u) = &self.user {
            s.push_str(u);
            s.push('@');
        }
        if self.host.contains(':') {
            s.push('[');
            s.push_str(&self.host);
            s.push(']');
        } else {
            s.push_str(&self.host);
        }
        if let Some(p) = self.port {
            s.push_str(&format!(":{p}"));
        }
        s
    }

    /// `ssh` arguments that name the destination.
    fn args(&self) -> Vec<String> {
        let mut a = Vec::new();
        if let Some(p) = self.port {
            a.push("-p".into());
            a.push(p.to_string());
        }
        a.push(match &self.user {
            Some(u) => format!("{u}@{}", self.host),
            None => self.host.clone(),
        });
        a
    }
}

/// A server address, parsed.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Endpoint {
    /// The socket path of a server on this machine.
    Local(PathBuf),
    /// The socket path (absolute, on that machine) of a server reachable over SSH.
    Ssh { host: SshHost, socket: String },
}

impl Endpoint {
    /// Parse a stored or typed address. Local: a socket path, with or without
    /// `\\.\pipe\`, quotes trimmed. Remote: `ssh://[user@]host[:port]/path`,
    /// or `ssh://[user@]host[:port]` alone (no socket yet: see
    /// [`Endpoint::parse_host`]).
    pub fn parse(input: &str) -> Option<Endpoint> {
        let t = input.trim().trim_matches('"').trim();
        if t.is_empty() {
            return None;
        }
        if let Some(rest) = t.strip_prefix("ssh://") {
            let (host, path) = split_host_path(rest)?;
            let socket = path.filter(|p| p.len() > 1)?;
            return Some(Endpoint::Ssh { host, socket });
        }
        if let Some(path) = t.strip_prefix(LOCAL) {
            return (!path.is_empty()).then(|| Endpoint::Local(PathBuf::from(path)));
        }
        // `user@host[:port]:/abs/path`, as [`Endpoint::address`] writes it. A
        // one-letter machine is a Windows drive (`C:/…`), not a host.
        if let Some(i) = t.find(":/") {
            let (machine, socket) = (&t[..i], &t[i + 1..]);
            if machine.len() > 1 && !machine.contains(['\\', '/']) && socket.len() > 1 {
                let (host, _) = split_host_path(machine)?;
                return Some(Endpoint::Ssh {
                    host,
                    socket: socket.to_string(),
                });
            }
        }
        let t = t
            .strip_prefix(r"\\.\pipe\")
            .or_else(|| t.strip_prefix(r"\\?\pipe\"))
            .unwrap_or(t);
        Some(Endpoint::Local(PathBuf::from(t)))
    }

    /// A host typed without a socket path: `ssh://user@host[:port]`,
    /// `ssh://user@host/`, or the bare `user@host` (an `@` and no path
    /// separator, so it cannot be a local socket path).
    pub fn parse_host(input: &str) -> Option<SshHost> {
        let t = input.trim().trim_matches('"').trim();
        let (host, path) = match t.strip_prefix("ssh://") {
            Some(rest) => split_host_path(rest)?,
            None if t.contains('@') && !t.contains(['/', '\\']) => split_host_path(t)?,
            None => return None,
        };
        match path {
            None => Some(host),
            Some(p) if p == "/" => Some(host),
            Some(_) => None,
        }
    }

    /// The address as shown: `<machine>:<socket path>`, the same shape for
    /// every server (`local:C:\\…\\herdr.sock`, `user@host:/…/herdr.sock`),
    /// since every server is reached the same way, through its machine's
    /// bridge. [`Endpoint::parse`] reads it back.
    pub fn address(&self) -> String {
        match self {
            Endpoint::Local(path) => format!("{LOCAL}{}", path.to_string_lossy()),
            Endpoint::Ssh { host, socket } => format!("{}:{socket}", host.key()),
        }
    }

    pub fn is_remote(&self) -> bool {
        matches!(self, Endpoint::Ssh { .. })
    }

    /// The machine whose bridge reaches this server.
    pub fn machine(&self) -> Machine {
        match self {
            Endpoint::Local(_) => Machine::Local,
            Endpoint::Ssh { host, .. } => Machine::Ssh(host.clone()),
        }
    }

    /// The bridge of this server's machine itself (its `deck:machine`
    /// channel, `deck_accounts::machine`): the machine's own work for Deck,
    /// whether the server there is deck-server or Herdr.
    pub fn machine_channel(&self) -> Endpoint {
        let socket = deck_accounts::machine::CHANNEL.to_string();
        match self {
            Endpoint::Local(_) => Endpoint::Local(PathBuf::from(socket)),
            Endpoint::Ssh { host, .. } => Endpoint::Ssh {
                host: host.clone(),
                socket,
            },
        }
    }

    /// The socket path as the bridge on that machine opens it.
    fn socket(&self) -> String {
        match self {
            Endpoint::Local(p) => p.to_string_lossy().into_owned(),
            Endpoint::Ssh { socket, .. } => socket.clone(),
        }
    }

    /// Open one connection to the server: a new channel of its machine's
    /// bridge (started when needed). `read_timeout` bounds each read; `None`
    /// blocks forever, which the long-lived event stream needs.
    pub fn connect(&self, read_timeout: Option<Duration>) -> io::Result<Stream> {
        link(&self.machine())?.open(&self.socket(), read_timeout)
    }
}

/// `[user@]host[:port][/path]` → host and the path (with its leading `/`).
fn split_host_path(rest: &str) -> Option<(SshHost, Option<String>)> {
    let (auth, path) = match rest.find('/') {
        Some(i) => (&rest[..i], Some(rest[i..].to_string())),
        None => (rest, None),
    };
    let (user, hostport) = match auth.rsplit_once('@') {
        Some((u, h)) => (Some(u.to_string()).filter(|u| !u.is_empty()), h),
        None => (None, auth),
    };
    let (host, port) = if let Some(v6) = hostport.strip_prefix('[') {
        let (h, tail) = v6.split_once(']')?;
        let port = match tail.strip_prefix(':') {
            Some(p) => Some(p.parse().ok()?),
            None if tail.is_empty() => None,
            None => return None,
        };
        (h.to_string(), port)
    } else {
        match hostport.rsplit_once(':') {
            Some((h, p)) => (h.to_string(), Some(p.parse().ok()?)),
            None => (hostport.to_string(), None),
        }
    };
    if host.is_empty() || host.contains(char::is_whitespace) || host.starts_with('-') {
        return None;
    }
    Some((SshHost { user, host, port }, path))
}

// ---- ssh ------------------------------------------------------------------------

/// The OpenSSH client: Windows' own one when installed, else `ssh` on PATH.
fn ssh_program() -> PathBuf {
    #[cfg(windows)]
    {
        if let Some(root) = std::env::var_os("SystemRoot") {
            let p = PathBuf::from(root).join(r"System32\OpenSSH\ssh.exe");
            if p.is_file() {
                return p;
            }
        }
    }
    PathBuf::from("ssh")
}

/// `ssh` with the options every Deck connection uses: never prompt (there is
/// no console), keep the session alive, accept a first-seen host key but
/// never a changed one.
fn ssh_command(host: &SshHost) -> std::process::Command {
    let mut cmd = crate::archive::quiet_command(ssh_program());
    cmd.args([
        "-T",
        "-o",
        "BatchMode=yes",
        "-o",
        "ConnectTimeout=10",
        "-o",
        "ServerAliveInterval=15",
        "-o",
        "ServerAliveCountMax=3",
        "-o",
        "StrictHostKeyChecking=accept-new",
    ]);
    cmd.stdin(Stdio::null());
    cmd.args(host.args());
    cmd
}

// ---- one bridge per machine -------------------------------------------------------

/// A machine with servers on it: this one, or an SSH host.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Machine {
    Local,
    Ssh(SshHost),
}

impl Machine {
    fn name(&self) -> String {
        match self {
            Machine::Local => "this machine".into(),
            Machine::Ssh(h) => h.key(),
        }
    }

    /// Start this machine's bridge: in Deck's own process for this
    /// machine, `ssh host deck-server bridge` for another one.
    fn start_bridge(&self) -> io::Result<Arc<Link>> {
        match self {
            Machine::Local => crate::bridge::start_local(BRIDGE_START),
            Machine::Ssh(host) => {
                let mut cmd = ssh_command(host);
                cmd.arg(REMOTE_BRIDGE);
                match crate::bridge::start_process(cmd, &host.key(), BRIDGE_START) {
                    Ok(link) => Ok(link),
                    Err(StartError::NoBridge(why)) => Err(io::Error::other(why)),
                    Err(StartError::Failed(err)) => Err(err),
                }
            }
        }
    }
}

/// On an SSH host: deck-server from PATH, else where it is installed for
/// the user (`~/.local/bin`; a non-login shell may not have it on PATH).
const REMOTE_BRIDGE: &str =
    "exec \"$(command -v deck-server || echo \"$HOME/.local/bin/deck-server\")\" bridge";

#[derive(Default)]
struct Slot {
    link: Option<Arc<Link>>,
    failed: Option<(Instant, String)>,
}

type Slots = Mutex<HashMap<Machine, Arc<Mutex<Slot>>>>;

fn slots() -> &'static Slots {
    static SLOTS: std::sync::OnceLock<Slots> = std::sync::OnceLock::new();
    SLOTS.get_or_init(Default::default)
}

/// The bridge of `machine`, started when needed. A failure is remembered
/// for [`BRIDGE_RETRY`] so a down host is not dialled on every call.
fn link(machine: &Machine) -> io::Result<Arc<Link>> {
    let slot = slots()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .entry(machine.clone())
        .or_default()
        .clone();
    let mut slot = slot.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(link) = &slot.link {
        if link.alive() {
            return Ok(link.clone());
        }
        tracing::warn!(machine = %machine.name(), "bridge ended; restarting");
        slot.link = None;
    }
    if let Some((at, why)) = &slot.failed {
        if at.elapsed() < BRIDGE_RETRY {
            return Err(io::Error::new(io::ErrorKind::NotConnected, why.clone()));
        }
    }
    let started = machine.start_bridge();
    match started {
        Ok(link) => {
            tracing::info!(machine = %machine.name(), "bridge up");
            slot.link = Some(link.clone());
            slot.failed = None;
            Ok(link)
        }
        Err(err) => {
            slot.failed = Some((Instant::now(), err.to_string()));
            Err(err)
        }
    }
}

/// Stop `machine`'s bridge (no connected server is left on it); its open
/// channels end, event streams too.
pub fn close(machine: &Machine) {
    let slot = slots()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(machine);
    if let Some(slot) = slot {
        if let Some(link) = slot
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .link
            .take()
        {
            link.kill();
        }
    }
}

/// Stop every bridge (Deck is exiting; the children would outlive it).
pub fn close_all() {
    let all: Vec<_> = slots()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .drain()
        .map(|(_, v)| v)
        .collect();
    for slot in all {
        if let Some(link) = slot
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .link
            .take()
        {
            link.kill();
        }
    }
}

/// The remote shell snippet that lists the server sockets on a host, one
/// absolute path per line: the root and every named session of each app dir
/// under `$XDG_CONFIG_HOME` (or `~/.config`), like local discovery.
fn list_script() -> String {
    let dirs: Vec<String> = APP_DIRS.iter().map(|a| format!("\"$c/{a}\"")).collect();
    format!(
        "c=\"${{XDG_CONFIG_HOME:-$HOME/.config}}\"; for d in {}; do for s in \"$d/herdr.sock\" \"$d\"/sessions/*/herdr.sock; do [ -S \"$s\" ] && printf '%s\\n' \"$s\"; done; done; true",
        dirs.join(" ")
    )
}

/// The server sockets on `host` (discovery over SSH). One short SSH session.
pub fn list_remote(host: &SshHost) -> Result<Vec<String>, String> {
    let mut cmd = ssh_command(host);
    cmd.arg(list_script());
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let child = cmd
        .spawn()
        .map_err(|e| format!("cannot start ssh ({}): {e}", ssh_program().display()))?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });
    let out = match rx.recv_timeout(LIST_TIMEOUT) {
        Ok(r) => r.map_err(|e| e.to_string())?,
        Err(_) => {
            return Err(format!(
                "ssh {}: no answer within {}s",
                host.key(),
                LIST_TIMEOUT.as_secs()
            ))
        }
    };
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let why = err
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .trim();
        return Err(format!(
            "ssh {}: {}",
            host.key(),
            if why.is_empty() { "failed" } else { why }
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with('/'))
        .map(str::to_string)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(user: Option<&str>, host: &str, port: Option<u16>) -> SshHost {
        SshHost {
            user: user.map(str::to_string),
            host: host.into(),
            port,
        }
    }

    #[test]
    fn local_addresses_parse_with_or_without_the_pipe_prefix() {
        assert_eq!(
            Endpoint::parse(r#"  "\\.\pipe\C:\x\herdr.sock" "#),
            Some(Endpoint::Local(PathBuf::from(r"C:\x\herdr.sock")))
        );
        assert_eq!(
            Endpoint::parse("/tmp/h.sock"),
            Some(Endpoint::Local(PathBuf::from("/tmp/h.sock")))
        );
        assert_eq!(Endpoint::parse("   "), None);
    }

    #[test]
    fn ssh_addresses_parse_and_round_trip() {
        let e = Endpoint::parse(
            "ssh://dev@build-box.example.com/home/dev/.config/deck-server/herdr.sock",
        )
        .unwrap();
        assert_eq!(
            e,
            Endpoint::Ssh {
                host: h(Some("dev"), "build-box.example.com", None),
                socket: "/home/dev/.config/deck-server/herdr.sock".into()
            }
        );
        assert_eq!(
            e.address(),
            "dev@build-box.example.com:/home/dev/.config/deck-server/herdr.sock"
        );
        assert_eq!(Endpoint::parse(&e.address()), Some(e.clone()));
        assert!(e.is_remote());
        let p = Endpoint::parse("ssh://box:2222/s.sock").unwrap();
        assert_eq!(
            p,
            Endpoint::Ssh {
                host: h(None, "box", Some(2222)),
                socket: "/s.sock".into()
            }
        );
        assert_eq!(p.address(), "box:2222:/s.sock");
        assert_eq!(Endpoint::parse(&p.address()), Some(p.clone()));
        let v6 = Endpoint::parse("ssh://u@[fe80::1]:22/s").unwrap();
        assert_eq!(v6.address(), "u@[fe80::1]:22:/s");
        assert_eq!(Endpoint::parse(&v6.address()), Some(v6.clone()));
        // A local address has the same shape; a drive letter is no host.
        let l = Endpoint::Local(PathBuf::from(r"C:\x\herdr.sock"));
        assert_eq!(l.address(), r"local:C:\x\herdr.sock");
        assert_eq!(Endpoint::parse(&l.address()), Some(l.clone()));
        assert_eq!(
            Endpoint::parse("C:/x/herdr.sock"),
            Some(Endpoint::Local(PathBuf::from("C:/x/herdr.sock")))
        );
    }

    #[test]
    fn a_host_without_a_socket_is_not_an_endpoint_but_a_host() {
        assert_eq!(Endpoint::parse("ssh://dev@build-box.example.com"), None);
        assert_eq!(Endpoint::parse("ssh://dev@build-box.example.com/"), None);
        assert_eq!(
            Endpoint::parse_host("ssh://dev@build-box.example.com"),
            Some(h(Some("dev"), "build-box.example.com", None))
        );
        assert_eq!(
            Endpoint::parse_host("dev@build-box.example.com:2200"),
            Some(h(Some("dev"), "build-box.example.com", Some(2200)))
        );
        assert_eq!(Endpoint::parse_host("ssh://u@h/x.sock"), None);
        assert_eq!(Endpoint::parse_host(r"C:\x\herdr.sock"), None);
        assert_eq!(Endpoint::parse_host("/tmp/a@b.sock"), None);
        assert_eq!(Endpoint::parse_host("ssh://-oProxyCommand=x"), None);
    }

    #[test]
    fn dialog_fields_make_a_host_with_its_port() {
        assert_eq!(
            SshHost::from_fields(" dev ", "build-box.example.com", 22),
            Ok(h(Some("dev"), "build-box.example.com", None))
        );
        let p = SshHost::from_fields("dev", "build-box.example.com", 2222).unwrap();
        assert_eq!(p, h(Some("dev"), "build-box.example.com", Some(2222)));
        assert_eq!(p.key(), "dev@build-box.example.com:2222");
        assert_eq!(p.args(), ["-p", "2222", "dev@build-box.example.com"]);
        assert_eq!(Endpoint::parse_host(&p.key()), Some(p.clone()));
        let v6 = SshHost::from_fields("u", "fe80::1", 2200).unwrap();
        assert_eq!(v6.key(), "u@[fe80::1]:2200");
        assert!(SshHost::from_fields("dev", "build-box.example.com", 0).is_err());
        assert!(SshHost::from_fields("dev", "build-box.example.com", 65536).is_err());
        assert!(SshHost::from_fields("", "build-box.example.com", 22).is_err());
        assert!(SshHost::from_fields("dev", "", 22).is_err());
        assert!(SshHost::from_fields("dev", "-oProxyCommand=x", 22).is_err());
        assert!(SshHost::from_fields("a@b", "build-box.example.com", 22).is_err());
        assert!(SshHost::from_fields("dev", "box/x", 22).is_err());
        assert!(SshHost::from_fields("dev", "box:2222", 22).is_err());
    }

    #[test]
    fn list_script_covers_every_app_dir() {
        let s = list_script();
        for app in APP_DIRS {
            assert!(s.contains(&format!("\"$c/{app}\"")), "{s}");
        }
        assert!(s.contains("sessions/*/herdr.sock"));
    }

    /// `cargo test live_local -- --ignored --nocapture`: through this
    /// machine's bridge, many calls at once to the default Herdr and an
    /// event subscription that gets its acknowledgement.
    #[test]
    #[ignore]
    fn live_local_calls_and_subscription() {
        use std::io::{BufRead, BufReader, Write};
        let e = Endpoint::Local(crate::herdr::socket_path());
        let calls: Vec<_> = (0..12)
            .map(|_| {
                let e = e.clone();
                std::thread::spawn(move || {
                    crate::herdr::call(&e, "workspace.list", serde_json::json!({})).is_ok()
                })
            })
            .collect();
        let ok = calls
            .into_iter()
            .filter_map(|h| h.join().ok())
            .filter(|x| *x)
            .count();
        println!("{ok}/12 calls answered");
        assert_eq!(ok, 12);
        let mut s = e.connect(Some(Duration::from_secs(5))).unwrap();
        s.write_all(br#"{"id":"t","method":"events.subscribe","params":{"subscriptions":[{"type":"workspace.created"}]}}"#).unwrap();
        s.write_all(
            b"
",
        )
        .unwrap();
        let mut line = String::new();
        BufReader::new(s).read_line(&mut line).unwrap();
        println!("{line}");
        assert!(line.contains("\"result\""), "{line}");
    }

    /// `cargo test live_ssh -- --ignored --nocapture` with
    /// `DECK_TEST_SSH=user@host` set: lists the host's servers and pings
    /// each through a tunnel.
    #[test]
    #[ignore]
    fn live_ssh_list_and_ping() {
        let Some(target) = std::env::var("DECK_TEST_SSH").ok() else {
            return;
        };
        let host = Endpoint::parse_host(&target).expect("DECK_TEST_SSH is user@host");
        let sockets = list_remote(&host).expect("listing");
        println!("{sockets:?}");
        assert!(!sockets.is_empty());
        for socket in sockets {
            let e = Endpoint::Ssh {
                host: host.clone(),
                socket,
            };
            let started = Instant::now();
            let r = crate::herdr::call(&e, "ping", serde_json::json!({}));
            let first = started.elapsed();
            let again = Instant::now();
            let r2 = crate::herdr::call(&e, "ping", serde_json::json!({}));
            println!(
                "{} {r:?} first={first:?} again={:?} {}",
                e.address(),
                again.elapsed(),
                r2.is_ok()
            );
        }
        close_all();
    }
}
