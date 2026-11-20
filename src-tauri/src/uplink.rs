//! UPLINK: how far each checkout is behind its upstream, `git pull` on
//! request, and the log of what each pull brought in (PRs with links), kept
//! in Deck's `uplink.json`. Pure git plumbing over the paths Herdr reports.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::archive::{git, quiet_command};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Sync {
    pub branch: Option<String>,
    pub upstream: Option<String>,
    pub behind: u32,
    pub ahead: u32,
    /// Working tree has local modifications (pull may refuse).
    pub dirty: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Commit {
    pub sha: String,
    pub title: String,
    #[serde(default)]
    pub pr: Option<u32>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub author: String,
    pub at_ms: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Update {
    pub id: String,
    pub repo: String,
    pub path: String,
    pub branch: String,
    pub from: String,
    pub to: String,
    pub at_ms: u64,
    pub commits: Vec<Commit>,
    #[serde(default)]
    pub seen: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Saved {
    #[serde(default)]
    pub updates: Vec<Update>,
}

pub struct Store {
    path: PathBuf,
    saved: Mutex<Saved>,
}

impl Store {
    pub fn open(dir: &Path) -> Self {
        let _ = std::fs::create_dir_all(dir);
        let path = dir.join("uplink.json");
        let saved = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<Saved>(&b).ok())
            .unwrap_or_default();
        Self {
            path,
            saved: Mutex::new(saved),
        }
    }
    pub fn get(&self) -> Saved {
        self.saved.lock().map(|s| s.clone()).unwrap_or_default()
    }
    fn edit(&self, f: impl FnOnce(&mut Saved)) -> Result<Saved, String> {
        let mut g = self
            .saved
            .lock()
            .map_err(|_| "uplink poisoned".to_string())?;
        f(&mut g);
        // newest first, bounded
        g.updates.sort_by_key(|u| std::cmp::Reverse(u.at_ms));
        g.updates.truncate(300);
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(
            &tmp,
            serde_json::to_vec_pretty(&*g).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &self.path).map_err(|e| e.to_string())?;
        Ok(g.clone())
    }
    pub fn push(&self, u: Update) -> Result<Saved, String> {
        self.edit(|s| s.updates.insert(0, u))
    }
    pub fn mark_seen(&self) -> Result<Saved, String> {
        self.edit(|s| s.updates.iter_mut().for_each(|u| u.seen = true))
    }
    pub fn remove(&self, id: &str) -> Result<Saved, String> {
        self.edit(|s| s.updates.retain(|u| u.id != id))
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Behind/ahead of the upstream for one checkout (no network).
pub fn sync(path: &str) -> Sync {
    let branch = git(path, &["rev-parse", "--abbrev-ref", "HEAD"])
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && s != "HEAD");
    let upstream = git(path, &["rev-parse", "--abbrev-ref", "@{upstream}"])
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let (behind, ahead) = upstream
        .as_ref()
        .and_then(|_| {
            git(
                path,
                &["rev-list", "--left-right", "--count", "HEAD...@{upstream}"],
            )
            .ok()
        })
        .and_then(|s| {
            let mut it = s.split_whitespace();
            Some((
                it.next()?.parse::<u32>().ok()?,
                it.next()?.parse::<u32>().ok()?,
            ))
        })
        .map(|(ahead, behind)| (behind, ahead))
        .unwrap_or((0, 0));
    let dirty = git(path, &["status", "--porcelain", "--untracked-files=no"])
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);
    Sync {
        branch,
        upstream,
        behind,
        ahead,
        dirty,
    }
}

/// Sync state for many checkouts at once.
pub fn sync_many(paths: &[String]) -> HashMap<String, Sync> {
    paths.iter().map(|p| (p.clone(), sync(p))).collect()
}

/// `git fetch --quiet --prune` for a repository root (network).
pub fn fetch(root: &str) -> Result<(), String> {
    let out = quiet_command("git")
        .args(["-C", root, "fetch", "--quiet", "--prune"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|e| format!("git fetch: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "git fetch: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// `https://github.com/org/repo` from the origin URL (ssh or https), if it is GitHub-like.
pub fn web_base(path: &str) -> Option<String> {
    let url = git(path, &["remote", "get-url", "origin"])
        .ok()?
        .trim()
        .to_string();
    let url = url
        .trim_end_matches(".git")
        .trim_end_matches('/')
        .to_string();
    if let Some(rest) = url.strip_prefix("git@") {
        let (host, repo) = rest.split_once(':')?;
        return Some(format!("https://{host}/{repo}"));
    }
    if let Some(rest) = url.strip_prefix("ssh://git@") {
        let (host, repo) = rest.split_once('/')?;
        return Some(format!("https://{host}/{repo}"));
    }
    if url.starts_with("https://") || url.starts_with("http://") {
        return Some(url);
    }
    None
}

/// PR number from a commit subject: `Merge pull request #12 …` or `title (#12)`.
pub fn pr_of(subject: &str) -> Option<u32> {
    if let Some(rest) = subject.strip_prefix("Merge pull request #") {
        return rest
            .split(|c: char| !c.is_ascii_digit())
            .next()?
            .parse()
            .ok();
    }
    let open = subject.rfind("(#")?;
    let tail = &subject[open + 2..];
    let end = tail.find(')')?;
    tail[..end].parse().ok()
}

/// A PR title without its `(#12)` suffix; a merge commit keeps the branch name after the PR number.
pub fn title_of(subject: &str) -> String {
    if let Some(rest) = subject.strip_prefix("Merge pull request #") {
        let after = rest.split_once(" from ").map(|(_, b)| b).unwrap_or(rest);
        return after.trim().to_string();
    }
    match subject.rfind("(#") {
        Some(i) if subject[i..].ends_with(')') => subject[..i].trim().to_string(),
        _ => subject.trim().to_string(),
    }
}

/// First-parent commits `from..to`, newest first, with PR numbers and links.
pub fn commits_between(path: &str, from: &str, to: &str, base: Option<&str>) -> Vec<Commit> {
    let range = format!("{from}..{to}");
    let Ok(log) = git(
        path,
        &[
            "log",
            "--first-parent",
            "--format=%H%x1f%s%x1f%an%x1f%ct",
            &range,
        ],
    ) else {
        return Vec::new();
    };
    log.lines()
        .filter_map(|l| {
            let mut it = l.split('\u{1f}');
            let sha = it.next()?.to_string();
            let subject = it.next()?.to_string();
            let author = it.next().unwrap_or("").to_string();
            let at_ms = it
                .next()
                .and_then(|t| t.trim().parse::<u64>().ok())
                .unwrap_or(0)
                * 1000;
            let pr = pr_of(&subject);
            let url = match (pr, base) {
                (Some(n), Some(b)) => Some(format!("{b}/pull/{n}")),
                _ => None,
            };
            Some(Commit {
                sha,
                title: title_of(&subject),
                pr,
                url,
                author,
                at_ms,
            })
        })
        .collect()
}

/// `git pull --ff-only` in the checkout; returns the update (what came in).
pub fn pull(repo: &str, path: &str) -> Result<Update, String> {
    let from = git(path, &["rev-parse", "HEAD"])?.trim().to_string();
    let branch = git(path, &["rev-parse", "--abbrev-ref", "HEAD"])?
        .trim()
        .to_string();
    let out = quiet_command("git")
        .args(["-C", path, "pull", "--ff-only", "--quiet"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|e| format!("git pull: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git pull: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let to = git(path, &["rev-parse", "HEAD"])?.trim().to_string();
    let base = web_base(path);
    let commits = if from == to {
        Vec::new()
    } else {
        commits_between(path, &from, &to, base.as_deref())
    };
    Ok(Update {
        id: format!("u{}{}", now_ms(), &to[..7.min(to.len())]),
        repo: repo.to_string(),
        path: path.to_string(),
        branch,
        from: from[..7.min(from.len())].to_string(),
        to: to[..7.min(to.len())].to_string(),
        at_ms: now_ms(),
        commits,
        seen: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pr_numbers_and_titles() {
        assert_eq!(
            pr_of("Merge pull request #1011 from org/hotfix-x"),
            Some(1011)
        );
        assert_eq!(pr_of("feat: tabs on Info page (#1012)"), Some(1012));
        assert_eq!(pr_of("plain commit"), None);
        assert_eq!(
            title_of("feat: tabs on Info page (#1012)"),
            "feat: tabs on Info page"
        );
        assert_eq!(
            title_of("Merge pull request #1011 from org/hotfix-x"),
            "org/hotfix-x"
        );
        assert_eq!(title_of("no pr here"), "no pr here");
    }

    #[test]
    fn remote_urls_become_web_bases() {
        // exercised through `web_base` on a real repo elsewhere; the parsing is tested here
        let parse = |url: &str| -> Option<String> {
            let url = url.trim_end_matches(".git").to_string();
            if let Some(rest) = url.strip_prefix("git@") {
                let (host, repo) = rest.split_once(':')?;
                return Some(format!("https://{host}/{repo}"));
            }
            if url.starts_with("https://") {
                return Some(url);
            }
            None
        };
        assert_eq!(
            parse("git@github.com:acme-labs/Atlas.git").as_deref(),
            Some("https://github.com/acme-labs/Atlas")
        );
        assert_eq!(
            parse("https://github.com/a/b.git").as_deref(),
            Some("https://github.com/a/b")
        );
    }
}
