//! The machine's own `git`, run without a window and without ever asking:
//! no terminal prompt, no credential manager dialog, ssh in batch mode. An
//! authentication failure comes back as an error instead of a prompt
//! nobody can see.

use std::path::Path;
use std::process::{Command, Output, Stdio};

/// `git` with the non-interactive environment.
pub fn git() -> Command {
    let mut command = Command::new("git");
    command
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .env("GIT_SSH_COMMAND", ssh_command(configured_ssh()))
        .env_remove("GIT_ASKPASS")
        .env_remove("SSH_ASKPASS")
        .stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// The ssh command the user set for git (`GIT_SSH_COMMAND`, else
/// `core.sshCommand` of the global or system config), read once.
fn configured_ssh() -> Option<String> {
    static CONFIGURED: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    CONFIGURED
        .get_or_init(|| {
            std::env::var("GIT_SSH_COMMAND")
                .ok()
                .filter(|s| !s.trim().is_empty())
                .or_else(|| {
                    let mut c = Command::new("git");
                    c.args(["config", "--get", "core.sshCommand"])
                        .stdin(Stdio::null());
                    #[cfg(windows)]
                    {
                        use std::os::windows::process::CommandExt;
                        c.creation_flags(0x0800_0000);
                    }
                    let out = c.output().ok()?;
                    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
                    (out.status.success() && !s.is_empty()).then_some(s)
                })
        })
        .clone()
}

/// ssh that never asks: batch mode, a first-seen host key accepted (as Deck
/// does), a changed one refused. A command the user set (with its own
/// agent or key) is kept, only put in batch mode.
pub fn ssh_command(existing: Option<String>) -> String {
    match existing.filter(|s| !s.trim().is_empty()) {
        Some(cmd) if cmd.contains("BatchMode") => cmd,
        Some(cmd) => format!("{cmd} -o BatchMode=yes"),
        None => "ssh -o BatchMode=yes -o StrictHostKeyChecking=accept-new".into(),
    }
}

/// `git -C dir args…` with `dir` trusted for this one command: a checkout
/// made by an elevated Herdr is owned by Administrators, and git refuses
/// such a "dubious ownership" repository (then `git diff` even falls back
/// to comparing two plain folders and prints its usage). Deck only ever
/// runs git in folders the user picked, so that one folder is trusted.
pub fn git_at(dir: &Path) -> Command {
    let mut command = git();
    command
        .arg("-c")
        .arg(format!("safe.directory={}", safe_directory(dir)))
        .arg("-C")
        .arg(dir);
    command
}

/// `dir` as `safe.directory` wants it: forward slashes, no `\\?\` prefix.
pub fn safe_directory(dir: &Path) -> String {
    let s = dir.to_string_lossy();
    let s = s.strip_prefix(r"\\?\").unwrap_or(&s);
    s.replace('\\', "/").trim_end_matches('/').to_string()
}

/// The words of a git failure worth showing: its `fatal:` / `error:` line
/// when there is one (never a usage dump), else the end of what it said.
pub fn git_error(stderr: &str) -> String {
    let lines = || stderr.lines().map(str::trim).filter(|l| !l.is_empty());
    lines()
        .find(|l| l.starts_with("fatal:") || l.starts_with("error:"))
        .map(str::to_string)
        .or_else(|| {
            lines()
                .next()
                .filter(|l| l.starts_with("usage:"))
                .map(|l| format!("git refused the command ({l})"))
        })
        .unwrap_or_else(|| tail(stderr, 600))
}

/// `git -C dir args…`, its output; a failure carries git's own words.
pub fn git_in(dir: &Path, args: &[&str]) -> crate::Result<Output> {
    let out = git_at(dir)
        .args(args)
        .output()
        .map_err(|e| format!("cannot run git: {e}"))?;
    if out.status.success() {
        Ok(out)
    } else {
        Err(format!(
            "git {} failed in {}: {}",
            args.first().copied().unwrap_or_default(),
            dir.display(),
            git_error(&String::from_utf8_lossy(&out.stderr))
        ))
    }
}

/// `git -C dir args…` stdout as text, trimmed.
pub fn git_text(dir: &Path, args: &[&str]) -> crate::Result<String> {
    git_in(dir, args).map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
}

/// The last `max` bytes of `text` (on a char boundary), trimmed.
pub fn tail(text: &str, max: usize) -> String {
    let t = text.trim();
    if t.len() <= max {
        return t.to_string();
    }
    let mut start = t.len() - max;
    while !t.is_char_boundary(start) {
        start += 1;
    }
    format!("…{}", &t[start..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_git_failure_shows_its_fatal_line_not_a_usage_dump() {
        let dump = "usage: git diff --no-index [<options>] <path> <path>\n\nDiff output format options\n    -p, --patch\n";
        assert!(git_error(dump).starts_with("git refused the command (usage: git diff"));
        let fatal = "warning: x\nfatal: detected dubious ownership in repository at 'C:/r'\nTo add an exception...";
        assert_eq!(
            git_error(fatal),
            "fatal: detected dubious ownership in repository at 'C:/r'"
        );
        assert_eq!(git_error("something else"), "something else");
        assert_eq!(
            safe_directory(Path::new(r"\\?\C:\Users\me\wt\")),
            "C:/Users/me/wt"
        );
    }

    #[test]
    fn ssh_never_asks() {
        assert_eq!(
            ssh_command(None),
            "ssh -o BatchMode=yes -o StrictHostKeyChecking=accept-new"
        );
        assert_eq!(ssh_command(Some("  ".into())), ssh_command(None));
        assert_eq!(
            ssh_command(Some("ssh -i k".into())),
            "ssh -i k -o BatchMode=yes"
        );
        assert_eq!(
            ssh_command(Some("ssh -o BatchMode=yes".into())),
            "ssh -o BatchMode=yes"
        );
    }

    #[test]
    fn tail_keeps_the_end() {
        assert_eq!(tail("  short ", 10), "short");
        assert_eq!(tail("abcdefghij", 4), "…ghij");
        assert_eq!(tail("ééé", 3), "…é");
    }
}
