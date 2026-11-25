//! `deck.github.*`: this machine signs in to GitHub with the official
//! GitHub CLI (`gh`), so git on it — a clone from Deck, Claude's Bash in a
//! chat — reaches private repositories over HTTPS.
//!
//! * `gh` is the machine's own when it is on `PATH`; otherwise the server
//!   downloads the official release for its OS and architecture from
//!   GitHub (checksum verified) into its own data dir. No root, no package
//!   manager, nothing for the user to type.
//! * Sign-in is `gh auth login --web` run by the server without a
//!   terminal: it prints a one-time code, Deck shows it with the link, the
//!   user approves in any browser (as whichever GitHub user they are), `gh`
//!   finishes by itself. Then `gh auth setup-git` makes `gh` git's
//!   credential helper for github.com on this machine.
//! * The token is `gh`'s, kept where `gh` keeps it; Deck never sees it.
//!
//! Kept apart from Claude's accounts so the per-machine connections in
//! Deck's settings can grow next to it.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub const HOST: &str = "github.com";
pub const DEVICE_URL: &str = "https://github.com/login/device";
const RELEASES: &str = "https://api.github.com/repos/cli/cli/releases/latest";

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn quiet(program: &Path) -> Command {
    let mut c = Command::new(program);
    c.stdin(Stdio::null())
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_NO_UPDATE_NOTIFIER", "1")
        .env("NO_COLOR", "1")
        .env("GIT_TERMINAL_PROMPT", "0");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000);
    }
    c
}

fn text_of(o: &std::process::Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

// ---- the gh binary -------------------------------------------------------------

/// The release asset for this machine: `gh_<ver>_linux_amd64.tar.gz`, …
pub fn asset_name(version: &str, os: &str, arch: &str) -> Option<String> {
    let arch = match arch {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        "x86" => "386",
        "arm" => "armv6",
        _ => return None,
    };
    Some(match os {
        "linux" => format!("gh_{version}_linux_{arch}.tar.gz"),
        "macos" => format!("gh_{version}_macOS_{arch}.zip"),
        "windows" => format!("gh_{version}_windows_{arch}.zip"),
        _ => return None,
    })
}

/// The sha256 a `checksums.txt` gives for `file`.
pub fn checksum_of(checksums: &str, file: &str) -> Option<String> {
    checksums.lines().find_map(|l| {
        let mut parts = l.split_whitespace();
        let (sum, name) = (parts.next()?, parts.next()?);
        (name.trim_start_matches('*') == file).then(|| sum.to_lowercase())
    })
}

fn exe_name() -> &'static str {
    if cfg!(windows) {
        "gh.exe"
    } else {
        "gh"
    }
}

/// `gh` on `PATH` (or in `~/.local/bin`), if any.
fn on_path() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(exe_name()))
        .find(|p| p.is_file())
        .or_else(|| {
            let p = crate::home_dir()?
                .join(".local")
                .join("bin")
                .join(exe_name());
            p.is_file().then_some(p)
        })
}

fn curl(url: &str) -> crate::Result<Vec<u8>> {
    let mut c = Command::new(crate::http::find_curl());
    c.args(["-fsSL", "--max-time", "300", url])
        .stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000);
    }
    let out = c.output().map_err(|e| format!("cannot run curl: {e}"))?;
    if out.status.success() {
        Ok(out.stdout)
    } else {
        Err(format!(
            "download of {url} failed: {}",
            crate::proc::tail(&String::from_utf8_lossy(&out.stderr), 300)
        ))
    }
}

/// Downloads the official `gh` into `tools` (`<tools>/gh/bin/gh`).
fn install(tools: &Path) -> crate::Result<PathBuf> {
    let latest: Value = serde_json::from_slice(&curl(RELEASES)?)
        .map_err(|e| format!("GitHub's release list is not JSON: {e}"))?;
    let tag = latest
        .get("tag_name")
        .and_then(Value::as_str)
        .ok_or("no GitHub CLI release found")?;
    let version = tag.trim_start_matches('v');
    let (os, arch) = (std::env::consts::OS, std::env::consts::ARCH);
    let asset = asset_name(version, os, arch)
        .ok_or_else(|| format!("no GitHub CLI build for {os} {arch}"))?;
    let base = format!("https://github.com/cli/cli/releases/download/{tag}");
    let sums =
        String::from_utf8_lossy(&curl(&format!("{base}/gh_{version}_checksums.txt"))?).into_owned();
    let want = checksum_of(&sums, &asset)
        .ok_or_else(|| format!("{asset} is not in the release's checksums"))?;
    let bytes = curl(&format!("{base}/{asset}"))?;
    if format!("{:x}", Sha256::digest(&bytes)) != want {
        return Err(format!("{asset} does not match its checksum"));
    }
    let dir = tools.join("gh");
    let unpack = dir.join(format!("unpack-{version}"));
    let _ = std::fs::remove_dir_all(&unpack);
    std::fs::create_dir_all(&unpack)
        .map_err(|e| format!("cannot create {}: {e}", unpack.display()))?;
    let archive = unpack.join(&asset);
    std::fs::write(&archive, &bytes)
        .map_err(|e| format!("cannot write {}: {e}", archive.display()))?;
    // tar reads .tar.gz everywhere and .zip on Windows and macOS (bsdtar).
    let mut tar = Command::new("tar");
    tar.arg(if asset.ends_with(".zip") {
        "-xf"
    } else {
        "-xzf"
    })
    .arg(&archive)
    .arg("-C")
    .arg(&unpack)
    .stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        tar.creation_flags(0x0800_0000);
    }
    let out = tar.output().map_err(|e| format!("cannot run tar: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "cannot unpack {asset}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let found = crate::pack::files_under(&unpack)
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|(rel, _)| rel.ends_with(&format!("bin/{}", exe_name())))
        .map(|(_, p)| p)
        .ok_or("the GitHub CLI archive has no bin/gh")?;
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).map_err(|e| e.to_string())?;
    let dest = bin.join(exe_name());
    std::fs::copy(&found, &dest).map_err(|e| format!("cannot install gh: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755));
    }
    let _ = std::fs::remove_dir_all(&unpack);
    Ok(dest)
}

// ---- gh's output ---------------------------------------------------------------

/// The one-time code `gh auth login --web` prints (`ABCD-1234`).
pub fn device_code(text: &str) -> Option<String> {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
        .find(|w| {
            let b = w.as_bytes();
            b.len() == 9
                && b[4] == b'-'
                && b.iter()
                    .enumerate()
                    .all(|(i, c)| i == 4 || c.is_ascii_uppercase() || c.is_ascii_digit())
        })
        .map(str::to_string)
}

/// The account `gh auth status` reports for github.com.
pub fn status_user(text: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let l = l.trim();
        if !l.contains(HOST) || !l.to_lowercase().contains("logged in") {
            return None;
        }
        let after = l
            .split_once(" account ")
            .or_else(|| l.split_once(" as "))?
            .1;
        let user = after.split_whitespace().next()?;
        let user = user.trim_matches(|c: char| !(c.is_ascii_alphanumeric() || c == '-'));
        (!user.is_empty()).then(|| user.to_string())
    })
}

// ---- the service -------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
struct Login {
    state: &'static str,
    user_code: Option<String>,
    user: Option<String>,
    message: Option<String>,
}

type Logins = Arc<Mutex<HashMap<String, (Instant, Login)>>>;

fn set(logins: &Logins, id: &str, f: impl FnOnce(&mut Login)) {
    if let Some((_, l)) = lock(logins).get_mut(id) {
        f(l);
    }
}

pub struct GitHub {
    /// `<server data dir>/tools`: where a downloaded `gh` lives.
    tools: PathBuf,
    logins: Logins,
    installing: Arc<Mutex<()>>,
}

impl GitHub {
    pub fn new(tools: PathBuf) -> Self {
        GitHub {
            tools,
            logins: Arc::default(),
            installing: Arc::default(),
        }
    }

    /// The `gh` this server uses, if there is one yet.
    pub fn gh(&self) -> Option<PathBuf> {
        on_path().or_else(|| {
            let p = self.tools.join("gh").join("bin").join(exe_name());
            p.is_file().then_some(p)
        })
    }

    fn ensure_gh(tools: &Path, installing: &Mutex<()>) -> crate::Result<PathBuf> {
        let _one = lock(installing);
        if let Some(p) = on_path() {
            return Ok(p);
        }
        let p = tools.join("gh").join("bin").join(exe_name());
        if p.is_file() {
            return Ok(p);
        }
        install(tools)
    }

    /// `deck.github.status` → `{available, signed_in, user}`.
    pub fn status(&self) -> Value {
        let Some(gh) = self.gh() else {
            return json!({ "available": false, "signed_in": false, "user": null });
        };
        let (ok, text) = match quiet(&gh)
            .args(["auth", "status", "--hostname", HOST])
            .output()
        {
            Ok(o) => (o.status.success(), text_of(&o)),
            Err(_) => (false, String::new()),
        };
        json!({
            "available": true,
            "signed_in": ok,
            "user": if ok { status_user(&text) } else { None },
        })
    }

    /// `deck.github.logout`: `gh` forgets this machine's GitHub login.
    pub fn logout(&self) -> crate::Result<Value> {
        if let Some(gh) = self.gh() {
            let mut c = quiet(&gh);
            c.args(["auth", "logout", "--hostname", HOST]);
            if let Some(user) = self.status().get("user").and_then(Value::as_str) {
                c.args(["--user", user]);
            }
            let out = c.output().map_err(|e| format!("cannot run gh: {e}"))?;
            if !out.status.success() {
                return Err(format!(
                    "gh auth logout: {}",
                    crate::proc::tail(&text_of(&out), 300)
                ));
            }
        }
        Ok(self.status())
    }

    /// `deck.github.login.start` → `{login_id}`: gets `gh` if needed and
    /// starts `gh auth login --web`; `login.status` gives the code.
    pub fn login_start(&self) -> crate::Result<Value> {
        let id = crate::random_id()?;
        {
            let mut logins = lock(&self.logins);
            logins.retain(|_, (t, _)| t.elapsed() < Duration::from_secs(3600));
            let login = Login {
                state: "installing",
                ..Login::default()
            };
            logins.insert(id.clone(), (Instant::now(), login));
        }
        let (tools, installing, logins, login) = (
            self.tools.clone(),
            self.installing.clone(),
            self.logins.clone(),
            id.clone(),
        );
        std::thread::Builder::new()
            .name("deck-github-login".into())
            .spawn(move || {
                let r = run_login(&tools, &installing, &logins, &login);
                if let Err(m) = r {
                    set(&logins, &login, |l| {
                        l.state = "failed";
                        l.message = Some(m);
                    });
                }
            })
            .map_err(|e| format!("cannot start the sign-in: {e}"))?;
        Ok(json!({ "login_id": id }))
    }

    /// `deck.github.login.status {login_id}` → `{state: installing |
    /// starting | pending | done | failed, user_code, verification_uri,
    /// user, message}`.
    pub fn login_status(&self, login_id: &str) -> crate::Result<Value> {
        let logins = lock(&self.logins);
        let (_, l) = logins
            .get(login_id)
            .ok_or("this sign-in is not known on the machine any more — start it again")?;
        Ok(json!({
            "state": l.state,
            "user_code": l.user_code,
            "verification_uri": l.user_code.as_ref().map(|_| DEVICE_URL),
            "user": l.user,
            "message": l.message,
        }))
    }
}

fn run_login(tools: &Path, installing: &Mutex<()>, logins: &Logins, id: &str) -> crate::Result<()> {
    let gh = GitHub::ensure_gh(tools, installing)
        .map_err(|e| format!("cannot get the GitHub CLI on this machine: {e}"))?;
    set(logins, id, |l| l.state = "starting");
    let mut c = quiet(&gh);
    c.args([
        "auth",
        "login",
        "--hostname",
        HOST,
        "--git-protocol",
        "https",
        "--web",
    ])
    // No browser on the server: Deck opens the link where the user is.
    .env(
        "GH_BROWSER",
        if cfg!(windows) { "cmd /c rem" } else { "true" },
    )
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    let mut child = c.spawn().map_err(|e| format!("cannot run gh: {e}"))?;
    let seen = Arc::new(Mutex::new(String::new()));
    let streams: Vec<Box<dyn Read + Send>> = [
        child
            .stdout
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
        child
            .stderr
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
    ]
    .into_iter()
    .flatten()
    .collect();
    let readers: Vec<_> = streams
        .into_iter()
        .map(|mut stream| {
            let (seen, logins, id) = (seen.clone(), logins.clone(), id.to_string());
            std::thread::spawn(move || {
                let mut buf = [0u8; 1024];
                while let Ok(n) = stream.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    let code = {
                        let mut s = lock(&seen);
                        s.push_str(&String::from_utf8_lossy(&buf[..n]));
                        device_code(&s)
                    };
                    if let Some(code) = code {
                        set(&logins, &id, |l| {
                            if l.user_code.is_none() {
                                l.user_code = Some(code);
                                l.state = "pending";
                            }
                        });
                    }
                }
            })
        })
        .collect();
    let status = child.wait();
    for r in readers {
        let _ = r.join();
    }
    let text = lock(&seen).clone();
    if !matches!(status, Ok(s) if s.success()) {
        return Err(format!("gh auth login: {}", crate::proc::tail(&text, 400)));
    }
    // git on this machine asks gh for github.com credentials from now on.
    let _ = quiet(&gh)
        .args(["auth", "setup-git", "--hostname", HOST])
        .output();
    let user = quiet(&gh)
        .args(["auth", "status", "--hostname", HOST])
        .output()
        .ok()
        .and_then(|o| status_user(&text_of(&o)));
    set(logins, id, |l| {
        l.state = "done";
        l.user = user;
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_release_asset_fits_the_machine() {
        let a = |os, arch| asset_name("2.83.0", os, arch);
        assert_eq!(
            a("linux", "x86_64").as_deref(),
            Some("gh_2.83.0_linux_amd64.tar.gz")
        );
        assert_eq!(
            a("linux", "aarch64").as_deref(),
            Some("gh_2.83.0_linux_arm64.tar.gz")
        );
        assert_eq!(
            a("windows", "x86_64").as_deref(),
            Some("gh_2.83.0_windows_amd64.zip")
        );
        assert_eq!(
            a("macos", "aarch64").as_deref(),
            Some("gh_2.83.0_macOS_arm64.zip")
        );
        assert_eq!(a("freebsd", "x86_64"), None);
        assert_eq!(a("linux", "riscv64"), None);
    }

    #[test]
    fn checksums_are_read_per_file() {
        let sums = "abc123  gh_2.83.0_linux_amd64.tar.gz\nDEF456 *gh_2.83.0_windows_amd64.zip\n";
        assert_eq!(
            checksum_of(sums, "gh_2.83.0_linux_amd64.tar.gz").as_deref(),
            Some("abc123")
        );
        assert_eq!(
            checksum_of(sums, "gh_2.83.0_windows_amd64.zip").as_deref(),
            Some("def456")
        );
        assert_eq!(checksum_of(sums, "gh_2.83.0_linux_arm64.tar.gz"), None);
    }

    #[test]
    fn the_one_time_code_is_found_in_ghs_output() {
        let out = "! First copy your one-time code: 3A1F-B2C9\nOpen this URL to continue in your web browser: https://github.com/login/device\n";
        assert_eq!(device_code(out).as_deref(), Some("3A1F-B2C9"));
        assert_eq!(
            device_code("- gh config set -h github.com git_protocol https"),
            None
        );
        assert_eq!(device_code("wait…"), None);
    }

    #[test]
    fn the_signed_in_user_is_read_from_auth_status() {
        let new = "github.com\n  ✓ Logged in to github.com account smirnov-plurio (keyring)\n  - Active account: true\n";
        assert_eq!(status_user(new).as_deref(), Some("smirnov-plurio"));
        let old =
            "github.com\n  ✓ Logged in to github.com as octo (/home/me/.config/gh/hosts.yml)\n";
        assert_eq!(status_user(old).as_deref(), Some("octo"));
        assert_eq!(
            status_user("You are not logged into any GitHub hosts."),
            None
        );
    }

    #[test]
    fn an_unknown_sign_in_says_so() {
        let gh = GitHub::new(std::env::temp_dir().join("deck-gh-none"));
        assert!(gh
            .login_status("nope")
            .unwrap_err()
            .contains("start it again"));
    }
}
