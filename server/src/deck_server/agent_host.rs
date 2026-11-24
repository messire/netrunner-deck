//! `deck-server agent`: Claude Code without a terminal, in a deck-server pane.
//!
//! Every deck-server pane runs this program directly, with no shell (the
//! server spawns its own executable as `agent`, see `pane.rs`
//! `set_agent_pane_program`; restore after a restart passes `--resume <id>`).
//! The host starts `claude -p` on the stream-json control protocol with
//! pipes, not the pane's terminal, and relays it to the server
//! (`src/api/agent.rs`): every output line goes up as a numbered event, and
//! what Deck asks for (a message, a permission answer, interrupt, model or
//! mode) comes down and is written to `claude`'s stdin. Nothing of Claude
//! Code's console UI exists here; Deck shows the session as a chat.
//!
//! The host also tells the pane what runs in it: it reports itself as agent
//! `deck-claude` (state idle / working / blocked, read from the stream) with
//! the command that resumes the session, so the tree, the sidebar status
//! and restore after a restart work as for any agent.
//!
//! If `claude` exits, it is started again on the same session (with a
//! growing pause when it keeps failing). If the server goes away (restart,
//! self-update) the host reconnects and replays the events the server may
//! have missed.

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use deck_agent::{proto, Backlog, Op, Status, Tracker};
use serde_json::{json, Value};

use crate::ipc::{connect_local_stream, LocalStream};

const USAGE: &str =
    "usage: deck-server agent [--resume SESSION_ID] [--model MODEL] [--permission-mode MODE]";

#[derive(Debug, Default, PartialEq, Eq)]
struct Options {
    resume: Option<String>,
    model: Option<String>,
    permission_mode: Option<String>,
}

fn parse_options(args: &[String]) -> Result<Options, String> {
    let mut options = Options::default();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut value = || {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{arg} needs a value\n{USAGE}"))
        };
        match arg.as_str() {
            "--resume" => options.resume = Some(value()?),
            "--model" => options.model = Some(value()?),
            "--permission-mode" => options.permission_mode = Some(value()?),
            other => return Err(format!("unknown argument {other}\n{USAGE}")),
        }
    }
    if let Some(mode) = &options.permission_mode {
        proto::set_permission_mode("check", mode)?;
    }
    Ok(options)
}

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or_default()
}

fn new_session_id() -> String {
    use sha2::{Digest, Sha256};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut hasher = Sha256::new();
    hasher.update(now_ns().to_le_bytes());
    hasher.update(std::process::id().to_le_bytes());
    hasher.update(COUNTER.fetch_add(1, Ordering::Relaxed).to_le_bytes());
    hasher.update(format!("{:p}", &hasher).as_bytes());
    let digest = hasher.finalize();
    let mut seed = [0u8; 16];
    seed.copy_from_slice(&digest[..16]);
    deck_agent::uuid_from_seed(seed)
}

/// What the event stream and the pane report have to agree on.
struct Stream {
    seq: u64,
    backlog: Backlog,
    tracker: Tracker,
    /// The server's event connection, when there is one.
    out: Option<LocalStream>,
}

struct Host {
    pane_id: String,
    socket: std::path::PathBuf,
    epoch: String,
    stream: Mutex<Stream>,
    stdin: Mutex<Option<ChildStdin>>,
    ops: AtomicU64,
    /// Set when a turn was ever sent, so the session exists on disk and a
    /// restart must `--resume` it rather than create it again.
    used: AtomicBool,
    session_id: Mutex<String>,
    reports: mpsc::Sender<Status>,
    /// Model / mode changes sent to `claude`, by control request id, with
    /// the note to emit when it confirms them.
    settings: Mutex<HashMap<String, Value>>,
    /// The running `claude`, so a resume or restart can stop it.
    child: Mutex<Option<Child>>,
    /// A restart was asked for while a turn ran: done at the next idle.
    restart_pending: AtomicBool,
    /// The current `claude` was stopped on purpose: start the next at once.
    restarting: AtomicBool,
}

impl Host {
    fn lock(&self) -> std::sync::MutexGuard<'_, Stream> {
        self.stream.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Numbers one event, keeps it for replay and sends it to the server.
    fn emit(&self, msg: Value) {
        let mut s = self.lock();
        s.seq += 1;
        let seq = s.seq;
        s.backlog.push(seq, &msg);
        let changed = s.tracker.observe(&msg);
        if changed == Some(Status::Working) {
            self.used.store(true, Ordering::Relaxed);
        }
        let line = deck_agent::event_line(seq, &msg);
        if let Some(out) = s.out.as_mut() {
            if write_line(out, &line).is_err() {
                s.out = None;
            }
        }
        drop(s);
        if let Some(status) = changed {
            let _ = self.reports.send(status);
            if status == Status::Idle && self.restart_pending.swap(false, Ordering::Relaxed) {
                self.stop_claude();
            }
        }
    }

    /// Stops the running `claude`; the main loop starts the next one at once.
    fn stop_claude(&self) {
        let mut child = self.child.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(child) = child.as_mut() {
            self.restarting.store(true, Ordering::Relaxed);
            let _ = child.kill();
        }
    }

    fn idle(&self) -> bool {
        self.lock().tracker.status() == Status::Idle
    }

    fn write_stdin(&self, lines: &[String]) -> io::Result<()> {
        let mut stdin = self.stdin.lock().unwrap_or_else(PoisonError::into_inner);
        let pipe = stdin
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "claude is not running"))?;
        for line in lines {
            pipe.write_all(line.as_bytes())?;
            pipe.write_all(b"\n")?;
        }
        pipe.flush()
    }

    fn apply(&self, op: Op) {
        match &op {
            Op::Resume { session_id } => {
                *self
                    .session_id
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner) = session_id.clone();
                self.used.store(true, Ordering::Relaxed);
                let _ = self.reports.send(Status::Idle);
                self.emit(proto::host_state(
                    "log",
                    Some(&format!("resuming session {session_id}")),
                ));
                self.stop_claude();
                return;
            }
            Op::Restart => {
                if self.idle() {
                    self.stop_claude();
                } else {
                    self.restart_pending.store(true, Ordering::Relaxed);
                }
                return;
            }
            _ => {}
        }
        if let Op::Respond { request_id, .. } = &op {
            // Another client answered first.
            if !self
                .lock()
                .tracker
                .open_requests()
                .iter()
                .any(|r| r == request_id)
            {
                return;
            }
        }
        let n = self.ops.fetch_add(1, Ordering::Relaxed);
        self.settings
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend(op.settings_requests(n));
        let result = op
            .stdin_lines(n)
            .and_then(|lines| self.write_stdin(&lines).map_err(|e| e.to_string()));
        match (result, &op) {
            (Ok(()), Op::Respond { request_id, .. }) => self.emit(proto::resolved(request_id)),
            (Ok(()), _) => {}
            (Err(err), _) => self.emit(proto::host_state(
                "log",
                Some(&format!("not delivered: {err}")),
            )),
        }
    }

    fn request_line(&self, method: &str) -> String {
        json!({
            "id": format!("deck-agent:{method}"),
            "method": method,
            "params": { "pane_id": self.pane_id, "epoch": self.epoch },
        })
        .to_string()
    }

    /// Opens a connection, sends `method` and reads its one-line reply
    /// (byte by byte, so nothing after it is taken from the stream).
    fn open(&self, method: &str) -> io::Result<LocalStream> {
        let mut stream = connect_local_stream(&self.socket)?;
        write_line(&mut stream, &self.request_line(method))?;
        let mut reply = Vec::new();
        let mut byte = [0u8; 1];
        loop {
            match stream.read(&mut byte)? {
                0 => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "server closed",
                    ))
                }
                _ if byte[0] == b'\n' => break,
                _ => reply.push(byte[0]),
            }
            if reply.len() > 64 * 1024 {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "reply too long"));
            }
        }
        let reply = String::from_utf8_lossy(&reply);
        if !reply.contains("\"result\"") {
            return Err(io::Error::other(format!(
                "{method} refused: {}",
                reply.trim()
            )));
        }
        Ok(stream)
    }
}

fn write_line(stream: &mut LocalStream, line: &str) -> io::Result<()> {
    stream.write_all(line.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()
}

/// Keeps the event connection up: (re)connects and replays the buffer, so
/// a server that restarted gets the current turn again.
fn keep_events_linked(host: Arc<Host>) {
    loop {
        if host.lock().out.is_none() {
            match host.open("deck.agent.host") {
                Ok(mut stream) => {
                    let mut s = host.lock();
                    let replay = s.backlog.since(None);
                    let ok = replay.iter().all(|e| {
                        let msg: Value = serde_json::from_str(&e.msg).unwrap_or(Value::Null);
                        write_line(&mut stream, &deck_agent::event_line(e.seq, &msg)).is_ok()
                    });
                    if ok {
                        s.out = Some(stream);
                    }
                }
                Err(err) => note_link_error(&err),
            }
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

/// Reads operations from the server and applies them, reconnecting when
/// the server goes away (which also drops the event connection, so both
/// come back together).
fn keep_inbox_linked(host: Arc<Host>) {
    loop {
        match host.open("deck.agent.inbox") {
            Ok(stream) => {
                for line in BufReader::new(stream).lines() {
                    let Ok(line) = line else { break };
                    match Op::from_line(&line) {
                        Ok(op) => host.apply(op),
                        Err(err) => host.emit(proto::host_state(
                            "log",
                            Some(&format!("bad operation: {err}")),
                        )),
                    }
                }
                host.lock().out = None;
            }
            Err(err) => note_link_error(&err),
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

fn note_link_error(err: &io::Error) {
    static LAST: Mutex<Option<String>> = Mutex::new(None);
    let text = err.to_string();
    let mut last = LAST.lock().unwrap_or_else(PoisonError::into_inner);
    if last.as_deref() != Some(text.as_str()) {
        eprintln!("deck-server agent: server link: {text} (retrying)");
        *last = Some(text);
    }
}

/// Reports the pane's agent state to the server, the latest one first.
fn report_states(host: Arc<Host>, states: mpsc::Receiver<Status>) {
    while let Ok(mut status) = states.recv() {
        while let Ok(newer) = states.try_recv() {
            status = newer;
        }
        let session = host
            .session_id
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let mut params = json!({
            "pane_id": host.pane_id,
            "source": deck_agent::SOURCE,
            "agent": deck_agent::AGENT_LABEL,
            "state": status.as_str(),
            "seq": now_ns(),
        });
        // The session exists on disk only after its first turn: before
        // that there is nothing to archive or resume.
        if host.used.load(Ordering::Relaxed) {
            params["agent_session_id"] = json!(session);
            params["resume_argv"] = json!(["deck-server", "agent", "--resume", session]);
        }
        let request = json!({
            "id": format!("deck-agent:report:{}", now_ns()),
            "method": "pane.report_agent",
            "params": params,
        });
        if let Ok(mut stream) = connect_local_stream(&host.socket) {
            if write_line(&mut stream, &request.to_string()).is_ok() {
                let mut reply = String::new();
                let _ = BufReader::new(stream).read_line(&mut reply);
            }
        }
    }
}

/// The `claude` program: on Windows an npm install is `claude.cmd`.
fn spawn_claude(host: &Host, options: &Options, session: proto::Session<'_>) -> io::Result<Child> {
    let args = proto::claude_args(
        session,
        options.model.as_deref(),
        options.permission_mode.as_deref(),
    );
    let candidates: &[&str] = if cfg!(windows) {
        &["claude", "claude.cmd"]
    } else {
        &["claude"]
    };
    let mut last = None;
    for program in candidates {
        let mut command = crate::noninteractive_process::command(program);
        command
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Herdr's own Claude hooks would claim the pane for `herdr:claude`;
            // this host reports the pane itself.
            .env_remove(crate::HERDR_ENV_VAR);
        match command.spawn() {
            Ok(child) => return Ok(child),
            Err(err) if err.kind() == io::ErrorKind::NotFound => last = Some(err),
            Err(err) => return Err(err),
        }
    }
    let _ = host;
    Err(last.unwrap_or_else(|| io::Error::new(io::ErrorKind::NotFound, "claude not found")))
}

/// Runs `claude` once, relaying its output until it exits.
fn run_claude(host: &Arc<Host>, options: &Options, session: proto::Session<'_>) -> io::Result<i32> {
    let mut child = spawn_claude(host, options, session)?;
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    *host.stdin.lock().unwrap_or_else(PoisonError::into_inner) = child.stdin.take();
    *host.child.lock().unwrap_or_else(PoisonError::into_inner) = Some(child);
    let (proto::Session::New(id) | proto::Session::Resume(id)) = session;
    host.emit(proto::running(id));
    let _ = host.write_stdin(&[proto::initialize()]);

    let errors = {
        let host = Arc::clone(host);
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if !line.trim().is_empty() {
                    host.emit(proto::host_state("log", Some(&line)));
                }
            }
        })
    };
    for line in BufReader::new(stdout).lines() {
        let Ok(line) = line else { break };
        match serde_json::from_str::<Value>(&line) {
            Ok(msg) if msg.is_object() => {
                let confirmed = deck_agent::control_success_id(&msg).and_then(|id| {
                    host.settings
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .remove(id)
                });
                host.emit(msg);
                if let Some(note) = confirmed {
                    host.emit(note);
                }
            }
            _ if line.trim().is_empty() => {}
            _ => host.emit(proto::host_state("log", Some(&line))),
        }
    }
    *host.stdin.lock().unwrap_or_else(PoisonError::into_inner) = None;
    let child = host
        .child
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take();
    let Some(mut child) = child else {
        return Ok(-1);
    };
    let status = child.wait()?;
    let _ = errors.join();
    let code = status.code().unwrap_or(-1);
    host.emit(proto::host_state("exited", Some(&format!("code {code}"))));
    Ok(code)
}

/// `deck-server agent ...`; returns the process exit code.
pub(crate) fn run(args: &[String]) -> i32 {
    let options = match parse_options(args) {
        Ok(options) => options,
        Err(err) => {
            eprintln!("{err}");
            return 2;
        }
    };
    let (Ok(pane_id), Ok(socket)) = (
        std::env::var(crate::integration::HERDR_PANE_ID_ENV_VAR),
        std::env::var(crate::api::SOCKET_PATH_ENV_VAR),
    ) else {
        eprintln!("deck-server agent runs inside a deck-server pane (HERDR_PANE_ID and HERDR_SOCKET_PATH are not set)");
        return 2;
    };
    let resumed = options.resume.is_some();
    let session_id = options.resume.clone().unwrap_or_else(new_session_id);
    let (reports, states) = mpsc::channel();
    let host = Arc::new(Host {
        pane_id,
        socket: socket.into(),
        epoch: format!("{}-{}", std::process::id(), now_ns() / 1_000_000),
        stream: Mutex::new(Stream {
            seq: 0,
            backlog: Backlog::default(),
            tracker: Tracker::default(),
            out: None,
        }),
        stdin: Mutex::new(None),
        ops: AtomicU64::new(1),
        used: AtomicBool::new(resumed),
        session_id: Mutex::new(session_id.clone()),
        reports,
        settings: Mutex::new(HashMap::new()),
        child: Mutex::new(None),
        restart_pending: AtomicBool::new(false),
        restarting: AtomicBool::new(false),
    });
    println!("Claude Code runs here without a terminal; Netrunner Deck shows this pane as a chat.");
    println!("session {session_id}");
    let _ = session_id;
    for task in [keep_events_linked, keep_inbox_linked] {
        let host = Arc::clone(&host);
        std::thread::spawn(move || task(host));
    }
    {
        let host = Arc::clone(&host);
        std::thread::spawn(move || report_states(host, states));
    }
    // The pane shows as a Claude chat from the start, before the first turn.
    let _ = host.reports.send(Status::Idle);

    let mut pause = Duration::from_secs(1);
    loop {
        let started = Instant::now();
        let session_id = host
            .session_id
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let session = if host.used.load(Ordering::Relaxed) {
            proto::Session::Resume(&session_id)
        } else {
            proto::Session::New(&session_id)
        };
        if let Err(err) = run_claude(&host, &options, session) {
            host.emit(proto::host_state(
                "exited",
                Some(&format!("could not run claude: {err}")),
            ));
        }
        if host.restarting.swap(false, Ordering::Relaxed) {
            pause = Duration::from_secs(1);
            continue;
        }
        if started.elapsed() > Duration::from_secs(60) {
            pause = Duration::from_secs(1);
        }
        std::thread::sleep(pause);
        pause = (pause * 2).min(Duration::from_secs(30));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn options() {
        assert_eq!(parse_options(&[]).unwrap(), Options::default());
        let o = parse_options(&args(&[
            "--resume",
            "abc",
            "--model",
            "opus",
            "--permission-mode",
            "plan",
        ]))
        .unwrap();
        assert_eq!(o.resume.as_deref(), Some("abc"));
        assert_eq!(o.model.as_deref(), Some("opus"));
        assert!(parse_options(&args(&["--resume"])).is_err());
        assert!(parse_options(&args(&["--permission-mode", "yolo"])).is_err());
        assert!(parse_options(&args(&["--nope"])).is_err());
    }
}
