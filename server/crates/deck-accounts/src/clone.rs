//! `deck.git.clone.*`: clone a repository onto this machine for Deck's
//! OPEN REPOSITORY (a pasted git URL).
//!
//! The machine's own `git` does it with whatever credentials the machine
//! already has (ssh keys and agent, a credential helper), never asking
//! ([`crate::proc::git`]). A github.com address given over SSH that is
//! refused is tried once more over HTTPS, where the machine's GitHub CLI
//! sign-in ([`crate::github`], `gh auth setup-git`) answers for git.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use serde_json::{json, Value};

use crate::jobs::{Job, Jobs};

/// A git remote address Deck should offer to clone: `https://…`,
/// `http://…`, `ssh://…`, `git://…` or scp-like `user@host:path`.
pub fn is_git_url(text: &str) -> bool {
    let s = text.trim();
    if s.is_empty() || s.contains(char::is_whitespace) {
        return false;
    }
    for scheme in ["https://", "http://", "ssh://", "git://", "git+ssh://"] {
        if let Some(rest) = s.strip_prefix(scheme) {
            let mut parts = rest.splitn(2, '/');
            let host = parts.next().unwrap_or_default();
            let path = parts.next().unwrap_or_default().trim_matches('/');
            return !host.is_empty() && !path.is_empty();
        }
    }
    // scp-like: user@host:path (not a Windows drive like C:\x).
    match s.split_once(':') {
        Some((left, path)) => match left.split_once('@') {
            Some((user, host)) => {
                !user.is_empty()
                    && !host.is_empty()
                    && !left.contains(['/', '\\'])
                    && !path.trim_matches('/').is_empty()
            }
            None => false,
        },
        None => false,
    }
}

/// The folder name a clone of `url` gets: the last path segment without
/// `.git` (`https://github.com/o/atlas.git` → `atlas`).
pub fn repo_name(url: &str) -> Option<String> {
    let s = url.trim().trim_end_matches('/');
    let last = s.rsplit(['/', ':']).next()?;
    let name = last.strip_suffix(".git").unwrap_or(last);
    (!name.is_empty() && name != "." && name != "..").then(|| name.to_string())
}

/// The HTTPS address of a github.com repository in any of its forms
/// (`https://github.com/o/r`, `git@github.com:o/r.git`,
/// `ssh://git@github.com/o/r`); `None` for any other host.
pub fn github_https(url: &str) -> Option<String> {
    let s = url.trim().trim_end_matches('/');
    let path = if let Some(rest) = s
        .strip_prefix("https://")
        .or_else(|| s.strip_prefix("http://"))
        .or_else(|| s.strip_prefix("ssh://"))
        .or_else(|| s.strip_prefix("git://"))
    {
        let rest = rest.rsplit_once('@').map(|(_, r)| r).unwrap_or(rest);
        let (host, path) = rest.split_once('/')?;
        let host = host.split(':').next().unwrap_or(host);
        if !host.eq_ignore_ascii_case("github.com") && !host.eq_ignore_ascii_case("www.github.com")
        {
            return None;
        }
        path.to_string()
    } else {
        let (left, path) = s.split_once(':')?;
        let host = left.rsplit_once('@').map(|(_, h)| h).unwrap_or(left);
        if !host.eq_ignore_ascii_case("github.com") {
            return None;
        }
        path.trim_start_matches('/').to_string()
    };
    let path = path.strip_suffix(".git").unwrap_or(&path);
    let mut parts = path.split('/').filter(|p| !p.is_empty());
    let (owner, repo) = (parts.next()?, parts.next()?);
    Some(format!("https://github.com/{owner}/{repo}.git"))
}

/// One line of `git clone --progress` as `(phase, percent)`:
/// `Receiving objects:  45% (9/20)` → `("Receiving objects", Some(45))`.
pub fn parse_progress(line: &str) -> Option<(String, Option<u8>)> {
    let line = line.trim();
    let line = line.strip_prefix("remote:").map(str::trim).unwrap_or(line);
    if line.starts_with("Cloning into") {
        return Some(("Cloning".into(), None));
    }
    let (phase, rest) = line.split_once(':')?;
    let phase = phase.trim();
    if phase.is_empty() || phase.len() > 40 || !phase.chars().all(|c| c.is_alphabetic() || c == ' ')
    {
        return None;
    }
    let percent = rest.split_whitespace().find_map(|w| {
        w.strip_suffix('%')
            .and_then(|n| n.parse::<u8>().ok())
            .filter(|n| *n <= 100)
    });
    if percent.is_none() && !rest.trim().is_empty() && !rest.contains("done") {
        return None;
    }
    Some((phase.to_string(), percent))
}

/// Git's words for "these credentials (or none) were not accepted".
pub fn is_auth_error(stderr: &str) -> bool {
    let s = stderr.to_ascii_lowercase();
    [
        "authentication failed",
        "could not read username",
        "could not read password",
        "terminal prompts disabled",
        "permission denied (publickey",
        "repository not found",
        "invalid username or password",
        "returned error: 401",
        "returned error: 403",
        "access denied",
    ]
    .iter()
    .any(|p| s.contains(p))
}

/// Where a clone may go: nothing there yet, or an empty folder.
fn check_target(path: &Path) -> crate::Result<bool> {
    match std::fs::metadata(path) {
        Err(_) => Ok(false),
        Ok(m) if !m.is_dir() => Err(format!(
            "{} is a file — pick another folder",
            path.display()
        )),
        Ok(_) => {
            let empty = std::fs::read_dir(path)
                .map_err(|e| format!("cannot read {}: {e}", path.display()))?
                .next()
                .is_none();
            if empty {
                Ok(true)
            } else {
                Err(format!(
                    "{} is not empty — pick another folder (a clone goes into a new or empty one)",
                    path.display()
                ))
            }
        }
    }
}

/// `deck.git.clone.start {url, path}` → `{job, path}`: the clone runs in
/// the background.
pub fn start(jobs: &Jobs, url: &str, path: &str) -> crate::Result<Value> {
    let url = url.trim().to_string();
    if !is_git_url(&url) {
        return Err(format!("{url} is not a git address"));
    }
    let target = crate::place_of(path)?;
    if !target.is_absolute() {
        return Err(format!("{} is not an absolute path", target.display()));
    }
    let existed = check_target(&target)?;
    let (id, job) = jobs.start("Starting")?;
    job.set_extra("path", json!(target.to_string_lossy()));
    job.set_extra("url", json!(url));
    let thread_job = job.clone();
    let thread_target = target.clone();
    std::thread::Builder::new()
        .name("deck-git-clone".into())
        .spawn(move || run(&thread_job, &url, &thread_target, existed))
        .map_err(|e| format!("cannot start the clone: {e}"))?;
    Ok(json!({ "job": id, "path": target.to_string_lossy() }))
}

fn run(job: &Arc<Job>, url: &str, target: &Path, existed: bool) {
    let first = clone_once(job, url, target);
    let result = match first {
        // SSH refused: over HTTPS the machine's GitHub sign-in may answer.
        Err((text, true)) if !job.is_cancelled() => match github_https(url) {
            Some(https) if https != url => {
                job.set_phase("Trying HTTPS", None);
                clean_target(target, existed);
                clone_once(job, &https, target)
            }
            _ => Err((text, true)),
        },
        other => other,
    };
    match result {
        Ok(()) => job.finish(None),
        Err((text, auth)) => {
            clean_target(target, existed);
            job.set_extra("auth", json!(auth));
            job.set_extra("github", json!(github_https(url).is_some()));
            job.fail(text);
        }
    }
}

/// A failed or cancelled clone leaves nothing behind: a folder it made is
/// removed, an empty one it was given is emptied again.
fn clean_target(target: &Path, existed: bool) {
    if !target.join(".git").exists() && std::fs::metadata(target).is_err() {
        return;
    }
    if existed {
        if let Ok(entries) = std::fs::read_dir(target) {
            for e in entries.flatten() {
                let p = e.path();
                let _ = if p.is_dir() {
                    std::fs::remove_dir_all(&p)
                } else {
                    std::fs::remove_file(&p)
                };
            }
        }
    } else {
        let _ = std::fs::remove_dir_all(target);
    }
}

/// One `git clone`; `Err((git's words, refused credentials))`.
fn clone_once(job: &Job, url: &str, target: &Path) -> Result<(), (String, bool)> {
    let mut command = crate::proc::git();
    command
        .args(["clone", "--progress", "--", url])
        .arg(target)
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|e| (format!("cannot run git: {e}"), false))?;
    let mut stderr = child.stderr.take();
    job.hold_child(child);
    let mut lines: Vec<String> = Vec::new();
    if let Some(err) = stderr.as_mut() {
        let mut buf = [0u8; 4096];
        let mut pending = Vec::new();
        loop {
            let n = match err.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            for &b in &buf[..n] {
                if b == b'\r' || b == b'\n' {
                    if !pending.is_empty() {
                        let line = String::from_utf8_lossy(&pending).into_owned();
                        pending.clear();
                        if let Some((phase, pct)) = parse_progress(&line) {
                            job.set_phase(&phase, pct);
                        } else {
                            lines.push(line);
                            if lines.len() > 40 {
                                lines.remove(0);
                            }
                        }
                    }
                } else {
                    pending.push(b);
                }
            }
        }
        if !pending.is_empty() {
            lines.push(String::from_utf8_lossy(&pending).into_owned());
        }
    }
    let status = job.take_child().and_then(|mut c| c.wait().ok());
    if job.is_cancelled() {
        return Err(("cancelled".into(), false));
    }
    match status {
        Some(s) if s.success() => Ok(()),
        _ => {
            let text = lines.join("\n");
            let auth = is_auth_error(&text);
            let words = crate::proc::tail(&text, 800);
            Err((
                if words.is_empty() {
                    "git clone failed".into()
                } else {
                    words
                },
                auth,
            ))
        }
    }
}

/// `deck.git.clone.status {job}`.
pub fn status(jobs: &Jobs, id: &str) -> crate::Result<Value> {
    Ok(jobs.get(id)?.progress().to_json())
}

/// `deck.git.clone.cancel {job}`: the clone stops and leaves nothing behind.
pub fn cancel(jobs: &Jobs, id: &str) -> crate::Result<Value> {
    let job = jobs.get(id)?;
    job.cancel();
    Ok(job.progress().to_json())
}

/// Where a clone of `url` goes by default: `~/{name}`.
pub fn default_path(url: &str) -> Option<PathBuf> {
    repo_name(url).map(|n| PathBuf::from("~").join(n))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_urls_are_recognised() {
        for url in [
            "https://github.com/o/atlas.git",
            "https://gitlab.example.com/group/sub/atlas",
            "git@github.com:o/atlas.git",
            "ssh://git@example.com:2222/o/atlas.git",
            "git://example.com/atlas",
            "deploy@example.com:repos/atlas",
        ] {
            assert!(is_git_url(url), "{url}");
        }
        for not in [
            "",
            "atlas",
            "~/atlas",
            "C:\\Workspace\\atlas",
            "/home/me/atlas",
            "https://github.com",
            "git@github.com:",
            "two words@x:y",
        ] {
            assert!(!is_git_url(not), "{not}");
        }
    }

    #[test]
    fn the_folder_is_named_after_the_repository() {
        assert_eq!(
            repo_name("https://github.com/o/atlas.git").as_deref(),
            Some("atlas")
        );
        assert_eq!(
            repo_name("git@github.com:o/atlas.git").as_deref(),
            Some("atlas")
        );
        assert_eq!(
            repo_name("https://example.com/a/notes/").as_deref(),
            Some("notes")
        );
        assert_eq!(repo_name("me@example.com:atlas").as_deref(), Some("atlas"));
        assert_eq!(
            default_path("https://x.example/o/r.git"),
            Some(PathBuf::from("~").join("r"))
        );
    }

    #[test]
    fn github_addresses_have_an_https_form() {
        let want = Some("https://github.com/o/atlas.git".to_string());
        assert_eq!(github_https("https://github.com/o/atlas"), want);
        assert_eq!(github_https("https://github.com/o/atlas.git/"), want);
        assert_eq!(github_https("git@github.com:o/atlas.git"), want);
        assert_eq!(github_https("ssh://git@github.com/o/atlas.git"), want);
        assert_eq!(
            github_https("https://user@github.com/o/atlas/tree/main"),
            want
        );
        assert_eq!(github_https("https://gitlab.com/o/atlas"), None);
        assert_eq!(github_https("git@example.com:o/atlas"), None);
        assert_eq!(github_https("https://github.com/o"), None);
    }

    #[test]
    fn progress_lines_give_phase_and_percent() {
        assert_eq!(
            parse_progress("Receiving objects:  45% (9/20), 1.20 MiB | 1.00 MiB/s"),
            Some(("Receiving objects".into(), Some(45)))
        );
        assert_eq!(
            parse_progress("remote: Counting objects: 100% (20/20), done."),
            Some(("Counting objects".into(), Some(100)))
        );
        assert_eq!(
            parse_progress("Resolving deltas:   3% (1/30)"),
            Some(("Resolving deltas".into(), Some(3)))
        );
        assert_eq!(
            parse_progress("Cloning into '/home/me/atlas'..."),
            Some(("Cloning".into(), None))
        );
        assert_eq!(parse_progress("fatal: repository 'x' not found"), None);
        assert_eq!(
            parse_progress("remote: Enumerating objects: 20, done."),
            Some(("Enumerating objects".into(), None))
        );
    }

    #[test]
    fn refused_credentials_are_recognised() {
        assert!(is_auth_error(
            "fatal: could not read Username for 'https://github.com': terminal prompts disabled"
        ));
        assert!(is_auth_error("git@github.com: Permission denied (publickey).\r\nfatal: Could not read from remote repository."));
        assert!(is_auth_error("remote: Repository not found.\nfatal: repository 'https://github.com/o/p.git/' not found"));
        assert!(is_auth_error(
            "fatal: Authentication failed for 'https://example.com/x.git/'"
        ));
        assert!(!is_auth_error(
            "fatal: unable to access 'https://x/': Could not resolve host: x"
        ));
        assert!(!is_auth_error(
            "fatal: destination path 'atlas' already exists and is not an empty directory."
        ));
    }

    #[test]
    fn a_clone_goes_into_a_new_or_empty_folder_only() {
        let dir = std::env::temp_dir().join(format!(
            "deck-clone-target-{}-{}",
            std::process::id(),
            crate::now_ms()
        ));
        assert_eq!(check_target(&dir), Ok(false));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(check_target(&dir), Ok(true));
        std::fs::write(dir.join("x"), "x").unwrap();
        assert!(check_target(&dir).unwrap_err().contains("is not empty"));
        assert!(check_target(&dir.join("x"))
            .unwrap_err()
            .contains("is a file"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A real clone of a local repository through the job: progress, then done.
    #[test]
    fn a_local_repository_is_cloned_in_the_background() {
        let root = std::env::temp_dir().join(format!(
            "deck-clone-real-{}-{}",
            std::process::id(),
            crate::now_ms()
        ));
        let src = root.join("src");
        std::fs::create_dir_all(&src).unwrap();
        let git = |dir: &Path, args: &[&str]| crate::proc::git_in(dir, args).unwrap();
        git(&src, &["init", "-q", "-b", "main"]);
        std::fs::write(src.join("a.txt"), "a").unwrap();
        git(&src, &["add", "."]);
        git(
            &src,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@example.com",
                "commit",
                "-q",
                "-m",
                "a",
            ],
        );
        let jobs = Jobs::default();
        let target = root.join("dst");
        // A file:// address stands in for a remote one.
        let url = format!(
            "file:///{}",
            src.to_string_lossy()
                .replace('\\', "/")
                .trim_start_matches('/')
        );
        let job = crate::jobs::Jobs::start(&jobs, "x").unwrap().1;
        let r = clone_once(&job, &url, &target);
        assert_eq!(r, Ok(()));
        assert!(target.join("a.txt").is_file());
        assert!(check_target(&target).unwrap_err().contains("is not empty"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
