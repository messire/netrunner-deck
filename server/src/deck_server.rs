//! `deck-server`: the headless Herdr server alone, for Netrunner Deck.
//!
//! Derived from Herdr (Apache-2.0, see LICENSE and NOTICE). It is the same
//! server and JSON API as `herdr server` (same PROTOCOL_VERSION), built
//! without the TUI client, the CLI and the self-updater, and it keeps its own
//! config/state/session/socket directories (`deck-server`, `deck-server-dev`
//! for debug builds) so it can run next to a Herdr install.
//!
//! Usage (like Herdr, the server runs in the background with no window):
//!
//! * `deck-server [--session NAME]` starts the server detached if it is not
//!   running and returns once its API answers;
//! * `deck-server stop [--session NAME]` stops it (`server.stop`);
//! * `deck-server status [--session NAME]` says whether it runs;
//! * `deck-server --foreground [--session NAME]` runs it in this console
//!   (debugging; this is also what the detached process runs).

// Modules shared with `herdr` carry code (and re-exports) only its TUI/CLI uses.
#![allow(dead_code, unused_imports)]

use std::io;

pub(crate) const HERDR_ENV_VAR: &str = "HERDR_ENV";
pub(crate) const HERDR_ENV_VALUE: &str = "1";

mod agent_resume;
mod agent_view_eval;
mod api;
mod app;
mod build_info;
#[path = "deck_server/cli.rs"]
mod cli;
#[path = "deck_server/client.rs"]
mod client;
mod config;
mod copy_mode;
mod detect;
mod events;
use ghostty_vt as ghostty;
mod handoff_runtime;
mod input;
mod integration;
mod ipc;
mod kitty_graphics;
mod layout;
mod logging;
mod metadata_tokens;
mod noninteractive_process;
mod pane;
use ghostty_vt::pane_graphics_files;
mod persist;
mod platform;
mod plugin_command;
mod plugin_installations;
mod plugin_paths;
mod popup_size;
mod product_announcements;
mod protocol;
mod pty;
mod raw_input;
mod release_notes;
mod render_prof;
mod render_signal;
mod selection;
#[cfg(unix)]
#[path = "deck_server/self_update.rs"]
mod self_update;
mod server;
mod session;
mod sound;
mod terminal;
mod terminal_notify;
mod terminal_theme;
mod thread_spawn;
mod ui;
#[path = "deck_server/update.rs"]
mod update;
mod workspace;
mod worktree;

/// Config, state, sessions and the API/client sockets live under this name.
const APP_DIR_NAME: &str = if cfg!(debug_assertions) {
    "deck-server-dev"
} else {
    "deck-server"
};

/// Herdr variables a parent Herdr pane exports. Inherited, they would point
/// this server at Herdr's own sockets, config or session.
const INHERITED_HERDR_VARS: &[&str] = &[
    api::SOCKET_PATH_ENV_VAR,
    server::socket_paths::CLIENT_SOCKET_PATH_ENV_VAR,
    config::CONFIG_PATH_ENV_VAR,
    session::SESSION_ENV_VAR,
    HERDR_ENV_VAR,
];

const USAGE: &str = "usage: deck-server [start|stop|status|update] [--session NAME] [--foreground]";

/// What the command line asks for, after `--session` was taken out.
#[derive(Debug, PartialEq, Eq)]
enum Action {
    Start,
    Foreground,
    Stop,
    Status,
    Update,
}

fn action_of(rest: &[String]) -> Result<Action, String> {
    let foreground = rest.iter().any(|a| a == "--foreground");
    let words: Vec<&str> = rest
        .iter()
        .map(String::as_str)
        .filter(|a| *a != "--foreground")
        .collect();
    match (words.as_slice(), foreground) {
        ([] | ["start"], false) => Ok(Action::Start),
        ([] | ["start"], true) => Ok(Action::Foreground),
        (["stop"], false) => Ok(Action::Stop),
        (["status"], false) => Ok(Action::Status),
        (["update"], false) => Ok(Action::Update),
        _ => Err(USAGE.to_string()),
    }
}

/// One JSON request to the running server's API; `None` when nothing answers.
fn request(method: &str) -> Option<String> {
    send_line(&format!(
        r#"{{"id":"deck-server","method":"{method}","params":{{}}}}"#
    ))
}

/// One request line to the API socket; the reply line, or `None`.
fn send_line(line: &str) -> Option<String> {
    use std::io::{BufRead, BufReader, Write};
    let path = api::socket_path();
    #[cfg(windows)]
    let stream = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(format!(r"\\.\pipe\{}", path.display()))
        .ok()?;
    #[cfg(not(windows))]
    let stream = std::os::unix::net::UnixStream::connect(&path).ok()?;
    let mut writer = stream.try_clone().ok()?;
    writeln!(writer, "{line}").ok()?;
    writer.flush().ok()?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).ok()?;
    Some(line)
}

fn running() -> bool {
    request("ping").is_some_and(|reply| reply.contains("\"result\""))
}

/// Start `deck-server --foreground` detached (no console, outlives this
/// process), the way `herdr` starts its server, and wait until it answers.
fn start_detached(session: Option<&str>) -> io::Result<()> {
    let exe = std::env::current_exe()?;
    let mut command = std::process::Command::new(exe);
    command
        .arg("--foreground")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    if let Some(name) = session {
        command.args(["--session", name]);
    }
    platform::detach_server_daemon_command(&mut command);
    let pid = platform::launch_server_daemon_command(&mut command)?;
    for _ in 0..100 {
        if running() {
            println!(
                "deck-server started (pid {pid}) at {}",
                api::socket_path().display()
            );
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    Err(io::Error::other(format!(
        "deck-server (pid {pid}) did not answer at {} within 10 s",
        api::socket_path().display()
    )))
}

/// Background self-update of a running server (Unix only; Deck ships Windows').
fn start_self_update() {
    #[cfg(unix)]
    match std::env::current_exe() {
        Ok(exe) => self_update::spawn_background(exe, live_handoff),
        Err(err) => tracing::warn!(%err, "self-update off: no executable path"),
    }
}

/// Ask the running server to hand its panes over to the binary at `exe`.
fn live_handoff(exe: &std::path::Path) -> Result<(), String> {
    let line = serde_json::json!({
        "id": "deck-server:update",
        "method": "server.live_handoff",
        "params": { "import_exe": exe },
    })
    .to_string();
    match send_line(&line) {
        Some(reply) if reply.contains("\"result\"") => Ok(()),
        Some(reply) => Err(format!("live handoff refused: {}", reply.trim())),
        // Nothing running: the new binary is simply used on the next start.
        None => Ok(()),
    }
}

/// `deck-server update`: check now and, if newer, install and hand off.
fn update_now() -> io::Result<()> {
    #[cfg(unix)]
    {
        let exe = std::env::current_exe()?;
        match self_update::check_and_apply(&exe, &live_handoff) {
            Ok(self_update::Outcome::Disabled(why)) => println!("self-update off: {why}"),
            Ok(self_update::Outcome::UpToDate(c)) => println!("deck-server is up to date ({c})"),
            Ok(self_update::Outcome::Updated { from, to }) => {
                println!("deck-server updated {from} -> {to}")
            }
            Err(err) => return Err(io::Error::other(err)),
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        println!("deck-server on Windows is updated with Netrunner Deck");
        Ok(())
    }
}

/// The `--session` value, if given (for passing it on to the detached server).
fn session_arg(args: &[String]) -> Option<&str> {
    args.windows(2)
        .find(|w| w[0] == "--session")
        .map(|w| w[1].as_str())
}

/// Agent hooks in deck-server panes run `$HERDR_BIN_PATH pane report-agent-session ...`
/// (and the other report leaves), with `HERDR_SOCKET_PATH` set to this server.
/// Forward those to the API like `herdr` does; `None` = not a hook call.
fn hook_call(args: &[String]) -> Option<i32> {
    if args.get(1).map(String::as_str) != Some("pane") {
        return None;
    }
    let leaf = args.get(2)?;
    let request = match cli::pane::hook_request(leaf, args.get(3..).unwrap_or_default())? {
        Ok(request) => request,
        Err(err) => {
            eprintln!("deck-server pane {leaf}: {err}");
            return Some(2);
        }
    };
    let Ok(line) = serde_json::to_string(&request) else {
        return Some(1);
    };
    match send_line(&line) {
        Some(reply) if reply.contains("\"result\"") => Some(0),
        Some(reply) => {
            eprintln!("deck-server pane {leaf}: {}", reply.trim());
            Some(1)
        }
        None => {
            eprintln!(
                "deck-server pane {leaf}: server not reachable at {}",
                api::socket_path().display()
            );
            Some(1)
        }
    }
}

fn main() -> io::Result<()> {
    // Before the env is cleaned: a hook call targets the server in HERDR_SOCKET_PATH.
    let raw: Vec<String> = std::env::args().collect();
    if let Some(code) = hook_call(&raw) {
        std::process::exit(code);
    }
    // A live handoff (self-update) starts the new binary as
    // `deck-server server --handoff-import SOCKET TOKEN`, with the session in its env.
    if raw.get(1).map(String::as_str) == Some("server")
        && raw.get(2).map(String::as_str) == Some("--handoff-import")
    {
        config::set_app_dir_name(APP_DIR_NAME);
        start_self_update();
        return server::headless::run_server();
    }
    for var in INHERITED_HERDR_VARS {
        std::env::remove_var(var);
    }
    config::set_app_dir_name(APP_DIR_NAME);
    let args: Vec<String> = std::env::args().collect();
    if args
        .iter()
        .skip(1)
        .any(|a| a == "--help" || a == "-h" || a == "--version" || a == "-V")
    {
        println!(
            "deck-server {} {} (herdr protocol {})\n{USAGE}\napi socket: {}",
            env!("CARGO_PKG_VERSION"),
            option_env!("HERDR_BUILD_COMMIT").unwrap_or("local"),
            protocol::PROTOCOL_VERSION,
            api::socket_path().display()
        );
        return Ok(());
    }
    // `--session NAME` selects `sessions/NAME` under the app dir, as in Herdr.
    let rest = match session::configure_from_args(&args) {
        Ok(cleaned) => cleaned,
        Err(err) => {
            eprintln!("error: {err}");
            std::process::exit(2);
        }
    };
    let action = match action_of(rest.get(1..).unwrap_or_default()) {
        Ok(action) => action,
        Err(usage) => {
            eprintln!("{usage}");
            std::process::exit(2);
        }
    };
    let socket = api::socket_path();
    match action {
        Action::Foreground => {
            start_self_update();
            server::headless::run_server()
        }
        Action::Update => update_now(),
        Action::Start if running() => {
            println!("deck-server is already running at {}", socket.display());
            Ok(())
        }
        Action::Start => start_detached(session_arg(&args)),
        Action::Status => {
            if running() {
                println!("running at {}", socket.display());
            } else {
                println!("not running ({})", socket.display());
                std::process::exit(1);
            }
            Ok(())
        }
        Action::Stop => {
            if !running() {
                println!("deck-server is not running ({})", socket.display());
                return Ok(());
            }
            let _ = request("server.stop");
            for _ in 0..50 {
                if !running() {
                    println!("deck-server stopped");
                    return Ok(());
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(io::Error::other("deck-server did not stop within 5 s"))
        }
    }
}
