//! deck-server keeps itself up to date, on Linux/macOS nodes and on Windows.
//!
//! A background thread of the running server checks an update source every
//! few minutes. The source is a URL or a local directory holding
//! `deck-server.json` (the commit the binaries were built from, and a name and
//! SHA-256 per CPU architecture) next to the binaries. By default it is the
//! `deck-server-latest` release of the repo, which CI refreshes on every push
//! to the deck-server branch.
//!
//! When the source names another commit than the one this binary was built
//! from, the new binary is downloaded, checked, put in place of the running
//! one and the server live-hands its panes over to it (`server.live_handoff`):
//! agents keep running, clients reconnect. On Windows, where a running
//! program cannot be replaced and there is no live handoff, the running
//! binary is moved aside (`deck-server.old.exe`), the new one put in its
//! place, and the server restarts on it without a window; its panes come
//! back from the session snapshot and resume their conversations.
//!
//! Optional `update.json` in the app config dir (`~/.config/deck-server`):
//! `{"enabled": true, "source": "https://… or /local/dir", "interval_minutes": 5}`.
//! Builds without `HERDR_BUILD_COMMIT` (local `cargo build`) never update.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;
use sha2::{Digest, Sha256};
use tracing::{info, warn};

pub const DEFAULT_SOURCE: &str =
    "https://github.com/messire/netrunner-deck/releases/download/deck-server-latest";
const MANIFEST: &str = "deck-server.json";
const SETTINGS: &str = "update.json";
const FIRST_CHECK: Duration = Duration::from_secs(60);
const DEFAULT_INTERVAL_MINUTES: u64 = 5;

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Settings {
    enabled: Option<bool>,
    source: Option<String>,
    interval_minutes: Option<u64>,
}

impl Settings {
    fn load() -> Self {
        let path = crate::config::config_dir().join(SETTINGS);
        match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|err| {
                warn!(path = %path.display(), %err, "ignoring unreadable update settings");
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    fn source(&self) -> &str {
        self.source.as_deref().unwrap_or(DEFAULT_SOURCE)
    }

    fn interval(&self) -> Duration {
        let minutes = self
            .interval_minutes
            .unwrap_or(DEFAULT_INTERVAL_MINUTES)
            .max(1);
        Duration::from_secs(minutes * 60)
    }
}

#[derive(Debug, Deserialize)]
struct Manifest {
    commit: String,
    #[serde(default)]
    version: String,
    assets: HashMap<String, Asset>,
}

#[derive(Debug, Deserialize)]
struct Asset {
    name: String,
    sha256: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Disabled(&'static str),
    UpToDate(String),
    Updated { from: String, to: String },
}

/// The commit this binary was built from (set by CI), if any.
pub fn current_commit() -> Option<&'static str> {
    option_env!("HERDR_BUILD_COMMIT")
        .map(str::trim)
        .filter(|c| !c.is_empty())
}

/// Check now; on a newer commit install it at `exe` and call `handoff(exe)`.
pub fn check_and_apply(
    exe: &Path,
    handoff: &dyn Fn(&Path) -> Result<(), String>,
) -> Result<Outcome, String> {
    let settings = Settings::load();
    if settings.enabled == Some(false) {
        return Ok(Outcome::Disabled("disabled in update.json"));
    }
    let Some(current) = current_commit() else {
        return Ok(Outcome::Disabled("local build (no HERDR_BUILD_COMMIT)"));
    };
    let source = settings.source();
    let manifest: Manifest = serde_json::from_slice(&read_source(source, MANIFEST)?)
        .map_err(|err| format!("bad {MANIFEST} at {source}: {err}"))?;
    if manifest.commit == current {
        return Ok(Outcome::UpToDate(short(current)));
    }
    let key = asset_key(std::env::consts::OS, std::env::consts::ARCH);
    let asset = manifest
        .assets
        .get(&key)
        .ok_or_else(|| format!("{source} has no deck-server for {key}"))?;
    let bytes = read_source(source, &asset.name)?;
    let sum = hex(&Sha256::digest(&bytes));
    if !sum.eq_ignore_ascii_case(asset.sha256.trim()) {
        return Err(format!("{} checksum mismatch ({sum})", asset.name));
    }

    let staged = exe.with_file_name(if cfg!(windows) {
        ".deck-server.update.exe"
    } else {
        ".deck-server.update"
    });
    std::fs::write(&staged, &bytes).map_err(|e| format!("write {}: {e}", staged.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("chmod {}: {e}", staged.display()))?;
    }
    let probe = crate::noninteractive_process::command(&staged)
        .arg("--version")
        .output()
        .map_err(|e| format!("new binary does not run: {e}"))?;
    if !probe.status.success() || !String::from_utf8_lossy(&probe.stdout).contains("deck-server") {
        let _ = std::fs::remove_file(&staged);
        return Err("new binary failed its --version check".into());
    }
    #[cfg(windows)]
    {
        // A running program cannot be overwritten, but it can be renamed.
        let old = exe.with_file_name("deck-server.old.exe");
        let _ = std::fs::remove_file(&old);
        std::fs::rename(exe, &old).map_err(|e| format!("move {} aside: {e}", exe.display()))?;
        if let Err(e) = std::fs::rename(&staged, exe) {
            let _ = std::fs::rename(&old, exe);
            return Err(format!("replace {}: {e}", exe.display()));
        }
    }
    #[cfg(not(windows))]
    std::fs::rename(&staged, exe).map_err(|e| format!("replace {}: {e}", exe.display()))?;
    info!(
        from = current,
        to = %manifest.commit,
        version = %manifest.version,
        "deck-server binary updated, handing off"
    );
    handoff(exe)?;
    Ok(Outcome::Updated {
        from: short(current),
        to: short(&manifest.commit),
    })
}

/// Run the periodic check in the background of a running server.
pub fn spawn_background(exe: PathBuf, handoff: fn(&Path) -> Result<(), String>) {
    let spawned = std::thread::Builder::new()
        .name("deck-server-update".into())
        .spawn(move || {
            std::thread::sleep(FIRST_CHECK);
            loop {
                match check_and_apply(&exe, &handoff) {
                    Ok(Outcome::Disabled(why)) => {
                        info!(why, "deck-server self-update off");
                        return;
                    }
                    // The new server took over; this process is on its way out.
                    Ok(Outcome::Updated { .. }) => return,
                    Ok(Outcome::UpToDate(_)) => {}
                    Err(err) => warn!(%err, "deck-server update check failed"),
                }
                std::thread::sleep(Settings::load().interval());
            }
        });
    if let Err(err) = spawned {
        warn!(%err, "could not start the deck-server update thread");
    }
}

fn read_source(source: &str, name: &str) -> Result<Vec<u8>, String> {
    if source.starts_with("https://") || source.starts_with("http://") {
        let url = format!("{}/{name}", source.trim_end_matches('/'));
        let out = crate::noninteractive_process::curl_command()
            .args(["-fsSL", "--max-time", "300", &url])
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|e| format!("curl: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "GET {url}: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        Ok(out.stdout)
    } else {
        let dir = source.strip_prefix("file://").unwrap_or(source);
        let path = Path::new(dir).join(name);
        std::fs::read(&path).map_err(|e| format!("read {}: {e}", path.display()))
    }
}

/// The manifest key of this machine's build: the CPU architecture on Linux
/// (`x86_64`, `aarch64`), `windows-<arch>` on Windows.
pub fn asset_key(os: &str, arch: &str) -> String {
    if os == "windows" {
        format!("windows-{arch}")
    } else {
        arch.to_string()
    }
}

fn short(commit: &str) -> String {
    commit.chars().take(12).collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_parses() {
        let m: Manifest = serde_json::from_str(
            r#"{"commit":"abc","version":"0.9.5","assets":{"x86_64":{"name":"deck-server-linux-x86_64","sha256":"00"}}}"#,
        )
        .unwrap();
        assert_eq!(m.assets["x86_64"].name, "deck-server-linux-x86_64");
        assert_eq!(asset_key("linux", "x86_64"), "x86_64");
        assert_eq!(asset_key("windows", "x86_64"), "windows-x86_64");
    }

    #[test]
    fn local_dir_source_reads_files() {
        let dir = std::env::temp_dir().join(format!("deck-update-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("x"), b"hi").unwrap();
        assert_eq!(read_source(dir.to_str().unwrap(), "x").unwrap(), b"hi");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
