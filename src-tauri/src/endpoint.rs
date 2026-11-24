//! Where a Herdr-protocol server is and how a byte stream to it is opened.
//!
//! Deck has one connection type for every server, local or remote: a stream
//! that carries Herdr's newline-delimited JSON API unchanged (one request
//! per connection, `events.subscribe` keeps its connection). Only *opening*
//! the stream depends on the address:
//!
//! * a **local** address is the server's socket path (on Windows the named
//!   pipe `\\.\pipe\<path>`, elsewhere a unix socket);
//! * an **ssh** address, `ssh://[user@]host[:port]/<absolute socket path>`,
//!   is the same socket on another machine. Deck keeps one
//!   `ssh host deck-server bridge` process per host (`bridge.rs`) and every
//!   connection to any socket there is a channel inside it: one request is
//!   one network round trip. A host without the bridge (only Herdr, or an
//!   older deck-server) is reached through `ssh -N -L` instead, one loopback
//!   TCP port forwarded to the remote socket per address (an SSH channel per
//!   connection, a few round trips each). Keys, agent, `~/.ssh/config` and
//!   `known_hosts` are the user's own OpenSSH setup; Deck stores only the
//!   address, in its local preferences.
//!
//! Everything above this module (calls, deadlines, the event stream, the
//! registry) sees only [`Endpoint`] and [`Stream`].

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// How long an SSH tunnel may take to come up (authentication included).
const TUNNEL_START: Duration = Duration::from_secs(20);
/// After a tunnel failed to start, wait this long before trying again so a
/// down host does not get an SSH attempt for every call.
const TUNNEL_RETRY: Duration = Duration::from_secs(10);
/// One remote listing of server sockets (discovery).
const LIST_TIMEOUT: Duration = Duration::from_secs(20);

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

    /// The address as shown and stored: the pipe name / socket path, or the
    /// `ssh://` URL.
    pub fn address(&self) -> String {
        match self {
            Endpoint::Local(path) => {
                let display = path.to_string_lossy();
                if cfg!(windows) {
                    format!(r"\\.\pipe\{display}")
                } else {
                    display.into_owned()
                }
            }
            Endpoint::Ssh { host, socket } => format!("ssh://{}{}", host.key(), socket),
        }
    }

    /// The local socket path; `None` for a remote server.
    pub fn local_path(&self) -> Option<&Path> {
        match self {
            Endpoint::Local(p) => Some(p),
            Endpoint::Ssh { .. } => None,
        }
    }

    pub fn is_remote(&self) -> bool {
        matches!(self, Endpoint::Ssh { .. })
    }

    /// Open one stream to the server. `read_timeout` bounds each read where
    /// the stream supports it (unix sockets, TCP); `None` blocks forever,
    /// which the long-lived event stream needs.
    pub fn connect(&self, read_timeout: Option<Duration>) -> io::Result<Stream> {
        match self {
            #[cfg(windows)]
            Endpoint::Local(_) => {
                let _ = read_timeout;
                std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(self.address())
                    .map(Stream::Local)
            }
            #[cfg(not(windows))]
            Endpoint::Local(path) => {
                let s = std::os::unix::net::UnixStream::connect(path)?;
                s.set_read_timeout(read_timeout).ok();
                Ok(Stream::Local(s))
            }
            Endpoint::Ssh { host, socket } => {
                if let Some(link) = host_link(host)? {
                    return Ok(Stream::Bridge(link.open(socket, read_timeout)?));
                }
                let port = tunnel_port(host, socket)?;
                let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
                let s = TcpStream::connect_timeout(&addr, Duration::from_secs(5))?;
                s.set_nodelay(true).ok();
                s.set_read_timeout(read_timeout).ok();
                Ok(Stream::Tcp(s))
            }
        }
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

/// A connected stream: the same bytes whatever carries them.
pub enum Stream {
    #[cfg(windows)]
    Local(std::fs::File),
    #[cfg(not(windows))]
    Local(std::os::unix::net::UnixStream),
    Tcp(TcpStream),
    Bridge(crate::bridge::Channel),
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Stream::Local(s) => s.read(buf),
            Stream::Tcp(s) => s.read(buf),
            Stream::Bridge(s) => s.read(buf),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Stream::Local(s) => s.write(buf),
            Stream::Tcp(s) => s.write(buf),
            Stream::Bridge(s) => s.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Stream::Local(s) => s.flush(),
            Stream::Tcp(s) => s.flush(),
            Stream::Bridge(s) => s.flush(),
        }
    }
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

struct Tunnel {
    child: Child,
    port: u16,
    stderr: Arc<Mutex<String>>,
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[derive(Default)]
struct TunnelSlot {
    tunnel: Option<Tunnel>,
    /// Last failure and when, so callers fail fast while the host is down.
    failed: Option<(Instant, String)>,
}

type Slots = Mutex<HashMap<(SshHost, String), Arc<Mutex<TunnelSlot>>>>;

fn slots() -> &'static Slots {
    static SLOTS: std::sync::OnceLock<Slots> = std::sync::OnceLock::new();
    SLOTS.get_or_init(Default::default)
}

fn slot_of(host: &SshHost, socket: &str) -> Arc<Mutex<TunnelSlot>> {
    slots()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .entry((host.clone(), socket.to_string()))
        .or_default()
        .clone()
}

/// The loopback port forwarded to `socket` on `host`, starting the tunnel
/// when it is not running (or died).
fn tunnel_port(host: &SshHost, socket: &str) -> io::Result<u16> {
    let slot = slot_of(host, socket);
    let mut slot = slot.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(t) = slot.tunnel.as_mut() {
        if matches!(t.child.try_wait(), Ok(None)) {
            return Ok(t.port);
        }
        let why = last_line(&t.stderr);
        tracing::warn!(host = %host.key(), %why, "ssh tunnel exited; restarting");
        slot.tunnel = None;
    }
    if let Some((at, why)) = &slot.failed {
        if at.elapsed() < TUNNEL_RETRY {
            return Err(io::Error::new(io::ErrorKind::NotConnected, why.clone()));
        }
    }
    match start_tunnel(host, socket) {
        Ok(t) => {
            let port = t.port;
            slot.tunnel = Some(t);
            slot.failed = None;
            Ok(port)
        }
        Err(err) => {
            slot.failed = Some((Instant::now(), err.to_string()));
            Err(err)
        }
    }
}

fn last_line(buf: &Arc<Mutex<String>>) -> String {
    let text = buf.lock().unwrap_or_else(PoisonError::into_inner);
    text.lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim()
        .to_string()
}

fn free_port() -> io::Result<u16> {
    let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    Ok(l.local_addr()?.port())
}

fn start_tunnel(host: &SshHost, socket: &str) -> io::Result<Tunnel> {
    let port = free_port()?;
    let mut cmd = ssh_command(host);
    cmd.args([
        "-N",
        "-o",
        "ExitOnForwardFailure=yes",
        "-L",
        &format!("127.0.0.1:{port}:{socket}"),
    ]);
    cmd.stdout(Stdio::null()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| {
        io::Error::new(
            e.kind(),
            format!("cannot start ssh ({}): {e}", ssh_program().display()),
        )
    })?;
    let stderr = Arc::new(Mutex::new(String::new()));
    if let Some(err) = child.stderr.take() {
        let buf = stderr.clone();
        std::thread::Builder::new()
            .name("deck-ssh-stderr".into())
            .spawn(move || {
                for line in BufReader::new(err).lines().map_while(Result::ok) {
                    let mut b = buf.lock().unwrap_or_else(PoisonError::into_inner);
                    if b.len() > 8192 {
                        b.clear();
                    }
                    b.push_str(&line);
                    b.push('\n');
                }
            })
            .ok();
    }
    let mut tunnel = Tunnel {
        child,
        port,
        stderr,
    };
    // ssh listens only once the session is up: poll the port until it
    // accepts, the process gives up, or the deadline passes.
    let deadline = Instant::now() + TUNNEL_START;
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    loop {
        if let Ok(Some(status)) = tunnel.child.try_wait() {
            // Give the stderr reader a moment to collect ssh's last words.
            std::thread::sleep(Duration::from_millis(100));
            let why = last_line(&tunnel.stderr);
            let why = if why.is_empty() {
                format!("ssh exited ({status})")
            } else {
                why
            };
            return Err(io::Error::new(
                io::ErrorKind::ConnectionRefused,
                format!("ssh {}: {why}", host.key()),
            ));
        }
        if TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok() {
            tracing::info!(host = %host.key(), %socket, port, "ssh tunnel up");
            return Ok(tunnel);
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!(
                    "ssh {}: no connection within {}s",
                    host.key(),
                    TUNNEL_START.as_secs()
                ),
            ));
        }
        std::thread::sleep(Duration::from_millis(150));
    }
}

// ---- bridge per host --------------------------------------------------------------

/// How long "this host has no bridge" is believed before trying it again
/// (its deck-server may update itself meanwhile).
const NO_BRIDGE_FOR: Duration = Duration::from_secs(600);

#[derive(Default)]
struct HostSlot {
    link: Option<Arc<crate::bridge::Link>>,
    no_bridge: Option<Instant>,
    failed: Option<(Instant, String)>,
}

type HostSlots = Mutex<HashMap<SshHost, Arc<Mutex<HostSlot>>>>;

fn host_slots() -> &'static HostSlots {
    static SLOTS: std::sync::OnceLock<HostSlots> = std::sync::OnceLock::new();
    SLOTS.get_or_init(Default::default)
}

/// The bridge to `host`, started when needed; `None` when the host has no
/// bridge (use the `-L` tunnel). An SSH failure is an error, remembered for
/// [`TUNNEL_RETRY`] so a down host is not dialled on every call.
fn host_link(host: &SshHost) -> io::Result<Option<Arc<crate::bridge::Link>>> {
    let slot = host_slots()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .entry(host.clone())
        .or_default()
        .clone();
    let mut slot = slot.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(link) = &slot.link {
        if link.alive() {
            return Ok(Some(link.clone()));
        }
        tracing::warn!(host = %host.key(), "ssh bridge ended; restarting");
        slot.link = None;
    }
    if slot
        .no_bridge
        .is_some_and(|at| at.elapsed() < NO_BRIDGE_FOR)
    {
        return Ok(None);
    }
    if let Some((at, why)) = &slot.failed {
        if at.elapsed() < TUNNEL_RETRY {
            return Err(io::Error::new(io::ErrorKind::NotConnected, why.clone()));
        }
    }
    match crate::bridge::start(ssh_command(host), host, TUNNEL_START) {
        Ok(link) => {
            slot.link = Some(link.clone());
            slot.failed = None;
            slot.no_bridge = None;
            Ok(Some(link))
        }
        Err(crate::bridge::StartError::NoBridge(why)) => {
            tracing::info!(host = %host.key(), %why, "no deck-server bridge; using ssh -L");
            slot.no_bridge = Some(Instant::now());
            Ok(None)
        }
        Err(crate::bridge::StartError::Ssh(err)) => {
            slot.failed = Some((Instant::now(), err.to_string()));
            Err(err)
        }
    }
}

/// Stop what carries a remote endpoint (the server was disconnected): its
/// `-L` tunnel, and the host's bridge when `host_in_use` is false (no other
/// connected server on that host). Open streams end, the event pump's too.
pub fn close(endpoint: &Endpoint, host_in_use: bool) {
    let Endpoint::Ssh { host, socket } = endpoint else {
        return;
    };
    let key = (host.clone(), socket.clone());
    let slot = slots()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(&key);
    if let Some(slot) = slot {
        slot.lock().unwrap_or_else(PoisonError::into_inner).tunnel = None;
    }
    if !host_in_use {
        let slot = host_slots()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(host);
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
}

/// Stop every tunnel and bridge (Deck is exiting; ssh children would outlive it).
pub fn close_all() {
    let all: Vec<_> = slots()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .drain()
        .map(|(_, v)| v)
        .collect();
    for slot in all {
        slot.lock().unwrap_or_else(PoisonError::into_inner).tunnel = None;
    }
    let hosts: Vec<_> = host_slots()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .drain()
        .map(|(_, v)| v)
        .collect();
    for slot in hosts {
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
            "ssh://dev@build-box.example.com/home/dev/.config/deck-server/herdr.sock"
        );
        assert!(e.is_remote());
        assert_eq!(e.local_path(), None);
        let p = Endpoint::parse("ssh://box:2222/s.sock").unwrap();
        assert_eq!(
            p,
            Endpoint::Ssh {
                host: h(None, "box", Some(2222)),
                socket: "/s.sock".into()
            }
        );
        assert_eq!(p.address(), "ssh://box:2222/s.sock");
        let v6 = Endpoint::parse("ssh://u@[fe80::1]:22/s").unwrap();
        assert_eq!(v6.address(), "ssh://u@[fe80::1]:22/s");
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
    fn list_script_covers_every_app_dir() {
        let s = list_script();
        for app in APP_DIRS {
            assert!(s.contains(&format!("\"$c/{app}\"")), "{s}");
        }
        assert!(s.contains("sessions/*/herdr.sock"));
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
