//! `deck.transfer.*`: COPY TO / MOVE TO another machine.
//!
//! The source server packs what was chosen into one file
//! ([`crate::pack`]) in the background (`export.start`), Deck reads it in
//! chunks (`export.read`) and writes them to the target server
//! (`import.start`, `import.write`), which unpacks it in the background
//! (`import.commit`). Deck only relays the bytes. A move then asks the
//! source to remove what arrived (`remove.start`), only after the target
//! said it is all there. Every long step is a job asked about with
//! `deck.transfer.status {job}`.
//!
//! What a repository transfer carries:
//! * the history as a git bundle (every ref, or the chosen branches and the
//!   tags), the remotes' addresses;
//! * per chosen worktree its branch, `git diff --binary HEAD` (staged and
//!   unstaged changes) and the untracked files that are not ignored, plus
//!   the repository's own harness (`.claude/`, `CLAUDE.md`,
//!   `CLAUDE.local.md`) even where it is ignored;
//! * the chosen Claude Code sessions: their transcripts and sidecar
//!   folders, put into `~/.claude/projects/<new folder>` on the target
//!   with the old paths inside rewritten to the new ones, so `claude
//!   --resume` finds them there.
//!
//! A harness transfer carries items of the personal `~/.claude` only
//! (instructions, skills, agents, commands, rules, output styles): never
//! credentials, accounts, settings or transcripts ([`harness_item_ok`]).

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::Engine as _;
use serde_json::{json, Value};

use crate::jobs::{Job, Jobs};
use crate::pack;
use crate::proc::{git_in, git_text};

/// Longest chunk one `export.read` hands out.
pub const MAX_CHUNK: u64 = 4 * 1024 * 1024;
const MANIFEST: &str = "manifest.json";

/// Where things are on this machine.
#[derive(Debug, Clone)]
pub struct Places {
    /// Packs and unpacked files in flight (`<server config dir>/transfer`).
    pub work: PathBuf,
    /// Claude Code's config home (`~/.claude`).
    pub claude_home: PathBuf,
    /// Where linked worktrees go (`worktrees.directory`, `~/.herdr/worktrees`).
    pub worktrees: PathBuf,
    /// A move only ever removes what is inside this folder (the server
    /// user's home on a server machine); `None`: anywhere.
    pub removable: Option<PathBuf>,
}

pub struct Transfer {
    jobs: Jobs,
    places: Places,
}

fn text<'a>(params: &'a Value, key: &str) -> crate::Result<&'a str> {
    params
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| format!("{key} is required"))
}

fn io_err(what: impl std::fmt::Display) -> impl Fn(std::io::Error) -> String {
    move |e| format!("{what}: {e}")
}

// ---- paths ------------------------------------------------------------------------

/// Claude Code's folder name for a working directory under `projects/`:
/// every character that is not a letter or digit becomes `-`.
pub fn project_dir_name(cwd: &str) -> String {
    cwd.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// Branch name → folder name, as deck-server names a new worktree.
pub fn branch_slug(branch: &str) -> String {
    let mut slug = String::new();
    let mut dash = false;
    for ch in branch.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            dash = false;
        } else if !dash {
            slug.push('-');
            dash = true;
        }
    }
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        "worktree".into()
    } else {
        slug
    }
}

/// Old paths of the source machine → new paths here; rewrites the strings
/// of a transcript so a session resumes in the new place.
#[derive(Debug, Clone, Default)]
pub struct PathMap {
    /// `(old spelling, new path)`, longest old first.
    pairs: Vec<(String, String)>,
}

impl PathMap {
    pub fn new(pairs: &[(String, String)]) -> Self {
        let mut out: Vec<(String, String)> = Vec::new();
        for (old, new) in pairs {
            let old = old.trim_end_matches(['/', '\\']);
            if old.is_empty() {
                continue;
            }
            let mut spellings = vec![
                old.to_string(),
                old.replace('\\', "/"),
                old.replace('/', "\\"),
            ];
            // C:\x and c:\x are the same folder.
            for s in spellings.clone() {
                let mut chars = s.chars();
                if let (Some(d), Some(':')) = (chars.next(), chars.next()) {
                    let flip = if d.is_ascii_uppercase() {
                        d.to_ascii_lowercase()
                    } else {
                        d.to_ascii_uppercase()
                    };
                    spellings.push(format!("{flip}{}", &s[1..]));
                }
            }
            for s in spellings {
                if !out.iter().any(|(o, _)| *o == s) {
                    out.push((s, new.clone()));
                }
            }
        }
        out.sort_by_key(|p| std::cmp::Reverse(p.0.len()));
        PathMap { pairs: out }
    }

    /// `s` with every old path replaced; `None` when nothing matched. A
    /// string that is a path under an old one also gets this machine's
    /// separators.
    pub fn rewrite(&self, s: &str) -> Option<String> {
        let mut out = String::with_capacity(s.len());
        let mut i = 0;
        let mut changed = false;
        let mut whole_path = false;
        'outer: while i < s.len() {
            for (old, new) in &self.pairs {
                if s[i..].starts_with(old.as_str()) {
                    let next = s[i + old.len()..].chars().next();
                    let boundary = match next {
                        None => true,
                        Some(c) => !(c.is_alphanumeric() || matches!(c, '-' | '_' | '.')),
                    };
                    if boundary {
                        if i == 0 {
                            whole_path = true;
                        }
                        out.push_str(new);
                        i += old.len();
                        changed = true;
                        continue 'outer;
                    }
                }
            }
            let c = s[i..].chars().next().unwrap_or_default();
            out.push(c);
            i += c.len_utf8().max(1);
        }
        if !changed {
            return None;
        }
        if whole_path && !out.contains(char::is_whitespace) {
            out = if cfg!(windows) {
                out.replace('/', "\\")
            } else {
                out.replace('\\', "/")
            };
        }
        Some(out)
    }

    fn walk(&self, v: &mut Value) -> bool {
        match v {
            Value::String(s) => match self.rewrite(s) {
                Some(n) => {
                    *s = n;
                    true
                }
                None => false,
            },
            Value::Array(a) => a.iter_mut().fold(false, |c, x| self.walk(x) | c),
            Value::Object(o) => o.values_mut().fold(false, |c, x| self.walk(x) | c),
            _ => false,
        }
    }

    /// One transcript line with its paths rewritten (a line that is not
    /// JSON is kept as it is).
    pub fn rewrite_line(&self, line: &str) -> String {
        match serde_json::from_str::<Value>(line) {
            Ok(mut v) => {
                if self.walk(&mut v) {
                    v.to_string()
                } else {
                    line.to_string()
                }
            }
            Err(_) => line.to_string(),
        }
    }

    pub fn rewrite_jsonl(&self, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        for line in text.split_inclusive('\n') {
            let (body, nl) = match line.strip_suffix('\n') {
                Some(b) => (b.strip_suffix('\r').unwrap_or(b), "\n"),
                None => (line, ""),
            };
            if body.trim().is_empty() {
                out.push_str(body);
            } else {
                out.push_str(&self.rewrite_line(body));
            }
            out.push_str(nl);
        }
        out
    }
}

/// A personal harness item that may travel: a relative path under
/// `~/.claude` in one of the harness folders, never credentials,
/// accounts, settings, transcripts or caches.
pub fn harness_item_ok(rel: &str) -> bool {
    if pack::check_name(rel).is_err() {
        return false;
    }
    let mut parts = rel.split('/');
    let first = parts.next().unwrap_or_default();
    let rest: Vec<&str> = parts.collect();
    match first {
        "CLAUDE.md" => rest.is_empty(),
        "skills" | "agents" | "commands" | "rules" | "output-styles" => {
            !rest.is_empty()
                && !rest
                    .iter()
                    .any(|p| p.starts_with(".credentials") || p.ends_with(".env"))
        }
        _ => false,
    }
}

// ---- transcripts ----------------------------------------------------------------------

fn looks_like_session_id(s: &str) -> bool {
    s.len() == 36
        && s.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
        && s.matches('-').count() == 4
}

/// `projects/<dir>/<id>.jsonl` and its project folder.
fn find_transcript(projects: &Path, id: &str) -> Option<(PathBuf, PathBuf)> {
    let file = format!("{id}.jsonl");
    std::fs::read_dir(projects).ok()?.flatten().find_map(|d| {
        let p = d.path().join(&file);
        p.is_file().then(|| (p, d.path()))
    })
}

/// The first `cwd` a transcript records.
fn transcript_cwd(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    text.lines().find_map(|l| {
        serde_json::from_str::<Value>(l)
            .ok()?
            .get("cwd")?
            .as_str()
            .map(str::to_string)
    })
}

// ---- git ----------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct Worktree {
    pub path: String,
    pub branch: Option<String>,
}

/// `git worktree list --porcelain`: the main checkout first.
pub fn parse_worktrees(porcelain: &str) -> Vec<Worktree> {
    let mut out = Vec::new();
    for block in porcelain.split("\n\n") {
        let mut path = None;
        let mut branch = None;
        for line in block.lines() {
            if let Some(p) = line.strip_prefix("worktree ") {
                path = Some(p.to_string());
            } else if let Some(b) = line.strip_prefix("branch ") {
                branch = Some(b.strip_prefix("refs/heads/").unwrap_or(b).to_string());
            }
        }
        if let Some(path) = path {
            out.push(Worktree { path, branch });
        }
    }
    out
}

fn same_path(a: &str, b: &str) -> bool {
    let norm = |s: &str| {
        let s = s.trim_start_matches(r"\\?\").replace('\\', "/");
        let s = s.trim_end_matches('/').to_string();
        if cfg!(windows) {
            s.to_lowercase()
        } else {
            s
        }
    };
    norm(a) == norm(b)
}

fn nul_list(out: &[u8]) -> Vec<String> {
    out.split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect()
}

// ---- the service ------------------------------------------------------------------------

impl Transfer {
    pub fn new(places: Places) -> Self {
        // Packs left by a relay that never finished (Deck closed, a machine
        // went away) go after a day.
        if let Ok(entries) = std::fs::read_dir(&places.work) {
            for e in entries.flatten() {
                let old = e
                    .metadata()
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.elapsed().ok())
                    .is_some_and(|age| age > std::time::Duration::from_secs(24 * 3600));
                if old {
                    let _ = remove_any(&e.path());
                }
            }
        }
        Transfer {
            jobs: Jobs::default(),
            places,
        }
    }

    pub fn places(&self) -> &Places {
        &self.places
    }

    fn pack_path(&self, job: &str) -> PathBuf {
        self.places.work.join(format!("{job}.pack"))
    }

    fn staging(&self, job: &str) -> PathBuf {
        self.places.work.join(format!("{job}.d"))
    }

    fn spawn(
        &self,
        name: &str,
        job: Arc<Job>,
        work: impl FnOnce(&Job) -> crate::Result<()> + Send + 'static,
    ) -> crate::Result<()> {
        std::thread::Builder::new()
            .name(name.into())
            .spawn(move || match work(&job) {
                Ok(()) => job.finish(None),
                Err(e) => job.fail(e),
            })
            .map(|_| ())
            .map_err(|e| format!("cannot start {name}: {e}"))
    }

    /// `deck.transfer.status {job}`.
    pub fn status(&self, job: &str) -> crate::Result<Value> {
        Ok(self.jobs.get(job)?.progress().to_json())
    }

    /// `deck.transfer.cancel {job}`: stops it and removes its files here.
    pub fn cancel(&self, job: &str) -> crate::Result<Value> {
        let j = self.jobs.get(job)?;
        j.cancel();
        let _ = std::fs::remove_file(self.pack_path(job));
        let _ = std::fs::remove_dir_all(self.staging(job));
        Ok(j.progress().to_json())
    }

    /// `deck.transfer.finish {job}`: the relay is over; the pack goes.
    pub fn finish(&self, job: &str) -> crate::Result<Value> {
        let _ = std::fs::remove_file(self.pack_path(job));
        let _ = std::fs::remove_dir_all(self.staging(job));
        self.jobs.remove(job);
        Ok(json!({ "finished": true }))
    }

    // ---- export -------------------------------------------------------------------

    /// `deck.transfer.export.start {kind: "repo", repo, whole, worktrees:
    /// [{path, branch}], sessions: [{session_id, cwd}]}` or `{kind:
    /// "harness", items}` → `{job}`; `status` says `size` once packed.
    pub fn export_start(&self, params: &Value) -> crate::Result<Value> {
        std::fs::create_dir_all(&self.places.work)
            .map_err(io_err("cannot create the transfer folder"))?;
        let kind = text(params, "kind")?.to_string();
        let (id, job) = self.jobs.start("Packing")?;
        let out = self.pack_path(&id);
        let params = params.clone();
        let places = self.places.clone();
        self.spawn("deck-transfer-export", job, move |job| {
            let file = std::fs::File::create(&out).map_err(io_err("cannot create the pack"))?;
            let mut w = pack::Writer::new(std::io::BufWriter::new(file)).map_err(io_err("pack"))?;
            match kind.as_str() {
                "repo" => export_repo(job, &places, &params, &mut w)?,
                "harness" => export_harness(job, &places, &params, &mut w)?,
                other => return Err(format!("unknown transfer kind {other}")),
            }
            let size = w.written();
            w.finish().map_err(io_err("pack"))?;
            job.set_extra("size", json!(size));
            Ok(())
        })?;
        Ok(json!({ "job": id }))
    }

    /// `deck.transfer.export.read {job, offset, length}` → `{data (base64),
    /// eof, size}`.
    pub fn export_read(&self, params: &Value) -> crate::Result<Value> {
        let id = text(params, "job")?;
        let job = self.jobs.get(id)?;
        if job.is_running() {
            return Err("the pack is not ready yet".into());
        }
        let p = job.progress();
        if p.state != crate::jobs::State::Done {
            return Err(p
                .message
                .unwrap_or_else(|| "the export did not finish".into()));
        }
        let offset = params.get("offset").and_then(Value::as_u64).unwrap_or(0);
        let length = params
            .get("length")
            .and_then(Value::as_u64)
            .unwrap_or(MAX_CHUNK)
            .min(MAX_CHUNK);
        let mut file =
            std::fs::File::open(self.pack_path(id)).map_err(io_err("the pack is gone"))?;
        let size = file.metadata().map_err(io_err("pack"))?.len();
        file.seek(SeekFrom::Start(offset.min(size)))
            .map_err(io_err("pack"))?;
        let mut buf = Vec::with_capacity(length.min(size.saturating_sub(offset)) as usize);
        file.take(length)
            .read_to_end(&mut buf)
            .map_err(io_err("pack"))?;
        let end = offset + buf.len() as u64;
        Ok(json!({
            "data": base64::engine::general_purpose::STANDARD.encode(&buf),
            "eof": end >= size,
            "size": size,
        }))
    }

    // ---- import ---------------------------------------------------------------------

    /// `deck.transfer.import.start {kind, path?}` → `{job, path}`: `path`
    /// (a repository's new folder, `~/name`) must be free or empty.
    pub fn import_start(&self, params: &Value) -> crate::Result<Value> {
        let kind = text(params, "kind")?;
        let target = match kind {
            "repo" => Some(crate::place_of(text(params, "path")?)?),
            "harness" => None,
            other => return Err(format!("unknown transfer kind {other}")),
        };
        let worktrees = params
            .get("worktrees")
            .and_then(Value::as_str)
            .filter(|w| !w.is_empty())
            .map(PathBuf::from);
        let id = self.import_start_at(kind, target)?;
        if let (Some(w), Some(j)) = (worktrees, id.get("job").and_then(Value::as_str)) {
            self.jobs
                .get(j)?
                .set_extra("worktrees_dir", json!(w.to_string_lossy()));
        }
        Ok(id)
    }

    fn import_start_at(&self, kind: &str, target: Option<PathBuf>) -> crate::Result<Value> {
        if let Some(p) = &target {
            // The same repository may already be there: the branch joins it.
            if !is_repo_root(p) {
                free_folder(p)?;
            }
        }
        std::fs::create_dir_all(&self.places.work)
            .map_err(io_err("cannot create the transfer folder"))?;
        let (id, job) = self.jobs.start("Receiving")?;
        job.set_extra("kind", json!(kind));
        job.set_extra("received", json!(0));
        if let Some(t) = &target {
            job.set_extra("path", json!(t.to_string_lossy()));
        }
        std::fs::File::create(self.pack_path(&id)).map_err(io_err("cannot create the pack"))?;
        Ok(json!({ "job": id, "path": target.map(|t| t.to_string_lossy().into_owned()) }))
    }

    /// `deck.transfer.import.write {job, offset, data}` → `{size}`; chunks
    /// come in order (`offset` = what arrived so far).
    pub fn import_write(&self, params: &Value) -> crate::Result<Value> {
        let id = text(params, "job")?;
        let job = self.jobs.get(id)?;
        if !job.is_running() {
            return Err("this import is over".into());
        }
        let offset = params.get("offset").and_then(Value::as_u64).unwrap_or(0);
        let data = params
            .get("data")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|e| format!("chunk is not base64: {e}"))?;
        let path = self.pack_path(id);
        let size = std::fs::metadata(&path)
            .map_err(io_err("the pack is gone"))?
            .len();
        if offset != size {
            return Err(format!(
                "chunk at {offset}, but {size} bytes arrived so far"
            ));
        }
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .map_err(io_err("pack"))?;
        file.write_all(&bytes)
            .map_err(io_err("cannot write the pack"))?;
        let size = size + bytes.len() as u64;
        job.set_extra("received", json!(size));
        Ok(json!({ "size": size }))
    }

    /// `deck.transfer.import.commit {job, size, on_conflict?}`: everything
    /// arrived; it is unpacked in the background (`status`: `path`,
    /// `worktrees`, `sessions`, harness `copied` / `skipped` / `renamed`).
    pub fn import_commit(&self, params: &Value) -> crate::Result<Value> {
        let id = text(params, "job")?.to_string();
        let job = self.jobs.get(&id)?;
        let pack = self.pack_path(&id);
        let got = std::fs::metadata(&pack)
            .map_err(io_err("the pack is gone"))?
            .len();
        if let Some(want) = params.get("size").and_then(Value::as_u64) {
            if want != got {
                return Err(format!("{got} of {want} bytes arrived"));
            }
        }
        let progress = job.progress();
        let kind = progress
            .extra
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("repo")
            .to_string();
        let target = progress
            .extra
            .get("path")
            .and_then(Value::as_str)
            .map(PathBuf::from);
        let on_conflict = params
            .get("on_conflict")
            .and_then(Value::as_str)
            .unwrap_or("skip")
            .to_string();
        let mut places = self.places.clone();
        if let Some(w) = progress.extra.get("worktrees_dir").and_then(Value::as_str) {
            places.worktrees = PathBuf::from(w);
        }
        let staging = self.staging(&id);
        job.set_phase("Unpacking", Some(0));
        // The job thread ends the job; this one only starts it.
        std::thread::Builder::new()
            .name("deck-transfer-import".into())
            .spawn(move || {
                let r = match kind.as_str() {
                    "harness" => import_harness(&job, &places, &pack, &staging, &on_conflict),
                    _ => match target {
                        Some(t) => import_repo(&job, &places, &pack, &staging, &t),
                        None => Err("no target folder".into()),
                    },
                };
                let _ = std::fs::remove_dir_all(&staging);
                let _ = std::fs::remove_file(&pack);
                match r {
                    Ok(()) => job.finish(None),
                    Err(e) => job.fail(e),
                }
            })
            .map_err(|e| format!("cannot start the import: {e}"))?;
        Ok(json!({ "job": id }))
    }

    /// `deck.transfer.conflicts {items}` → `{existing}`: the harness items
    /// that already exist here (Deck asks overwrite / skip / keep both).
    pub fn conflicts(&self, params: &Value) -> crate::Result<Value> {
        let items = string_list(params, "items");
        let existing: Vec<&String> = items
            .iter()
            .filter(|rel| harness_item_ok(rel) && self.places.claude_home.join(rel).exists())
            .collect();
        Ok(json!({ "existing": existing }))
    }

    // ---- remove (after a move) --------------------------------------------------------------

    /// `deck.transfer.remove.start {kind: "repo", repo, whole, worktrees,
    /// sessions: [id]}` or `{kind: "harness", items}` → `{job}`.
    pub fn remove_start(&self, params: &Value) -> crate::Result<Value> {
        let kind = text(params, "kind")?.to_string();
        let params = params.clone();
        let places = self.places.clone();
        let (id, job) = self.jobs.start("Removing")?;
        self.spawn("deck-transfer-remove", job, move |job| {
            match kind.as_str() {
                "repo" => remove_repo(job, &places, &params),
                "harness" => {
                    for rel in string_list(&params, "items") {
                        if !harness_item_ok(&rel) {
                            return Err(format!("{rel} is not a harness item"));
                        }
                        remove_any(&places.claude_home.join(&rel))?;
                    }
                    Ok(())
                }
                other => Err(format!("unknown transfer kind {other}")),
            }
        })?;
        Ok(json!({ "job": id }))
    }
}

fn string_list(params: &Value, key: &str) -> Vec<String> {
    params
        .get(key)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// A folder a repository may be put in: not there, or empty.
fn free_folder(p: &Path) -> crate::Result<()> {
    let taken = || {
        Err(format!(
            "{} is already taken on this machine — pick another name",
            p.display()
        ))
    };
    match std::fs::read_dir(p) {
        Err(_) if !p.exists() => Ok(()),
        Err(_) => taken(),
        Ok(mut entries) => {
            if entries.next().is_none() {
                Ok(())
            } else {
                taken()
            }
        }
    }
}

fn remove_any(p: &Path) -> crate::Result<()> {
    let r = match std::fs::symlink_metadata(p) {
        Err(_) => return Ok(()),
        Ok(m) if m.is_dir() => std::fs::remove_dir_all(p),
        Ok(_) => std::fs::remove_file(p),
    };
    r.map_err(io_err(format!("cannot remove {}", p.display())))
}

fn copy_tree(from: &Path, to: &Path) -> crate::Result<()> {
    if from.is_file() {
        if let Some(d) = to.parent() {
            std::fs::create_dir_all(d).map_err(io_err(d.display()))?;
        }
        std::fs::copy(from, to).map_err(io_err(to.display()))?;
        return Ok(());
    }
    for (rel, path) in pack::files_under(from).map_err(io_err(from.display()))? {
        let dest = pack::safe_join(to, &rel)?;
        if let Some(d) = dest.parent() {
            std::fs::create_dir_all(d).map_err(io_err(d.display()))?;
        }
        std::fs::copy(&path, &dest).map_err(io_err(dest.display()))?;
    }
    Ok(())
}

// ---- repository export ---------------------------------------------------------------------

fn export_repo<W: Write>(
    job: &Job,
    places: &Places,
    params: &Value,
    w: &mut pack::Writer<W>,
) -> crate::Result<()> {
    let repo = PathBuf::from(text(params, "repo")?);
    let whole = params
        .get("whole")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let top = git_text(&repo, &["rev-parse", "--show-toplevel"])
        .map_err(|e| format!("{} is not a git repository: {e}", repo.display()))?;
    let all = parse_worktrees(&git_text(&repo, &["worktree", "list", "--porcelain"])?);
    let main = all
        .first()
        .map(|w| w.path.clone())
        .unwrap_or_else(|| top.clone());
    let chosen: Vec<Worktree> = if whole {
        all.clone()
    } else {
        params
            .get("worktrees")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|w| {
                let path = w.get("path")?.as_str()?;
                // The branch is the worktree's own, as git says.
                let known = all.iter().find(|a| same_path(&a.path, path));
                Some(Worktree {
                    path: known
                        .map(|k| k.path.clone())
                        .unwrap_or_else(|| path.to_string()),
                    branch: known
                        .and_then(|k| k.branch.clone())
                        .or_else(|| w.get("branch")?.as_str().map(str::to_string)),
                })
            })
            .collect()
    };
    if chosen.is_empty() {
        return Err("nothing chosen to transfer".into());
    }
    for wt in &chosen {
        if wt.branch.is_none() {
            return Err(format!(
                "{} is not on a branch (detached HEAD) — check out a branch there first",
                wt.path
            ));
        }
    }
    let has_commits = git_in(&repo, &["rev-parse", "--verify", "-q", "HEAD"]).is_ok();
    // Root commits: the target recognises the same repository by them.
    let roots: Vec<String> = if has_commits {
        git_text(&repo, &["rev-list", "--max-parents=0", "--all"])
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    } else {
        Vec::new()
    };
    let name = Path::new(&main)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "repo".into());
    let remotes: Vec<Value> = git_text(&repo, &["config", "--get-regexp", r"^remote\..*\.url$"])
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let (key, url) = l.split_once(' ')?;
            let remote = key.strip_prefix("remote.")?.strip_suffix(".url")?;
            Some(json!({ "name": remote, "url": url }))
        })
        .collect();

    // History.
    let tmp_bundle = places.work.join(format!("bundle-{}", crate::random_id()?));
    let mut bundle = false;
    if has_commits {
        job.set_phase("Packing history", Some(5));
        let mut args: Vec<String> = vec![
            "bundle".into(),
            "create".into(),
            "-q".into(),
            tmp_bundle.to_string_lossy().into_owned(),
        ];
        if whole {
            args.push("--all".into());
        } else {
            for wt in &chosen {
                args.push(format!(
                    "refs/heads/{}",
                    wt.branch.as_deref().unwrap_or_default()
                ));
            }
            args.push("--tags".into());
        }
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        git_in(&repo, &refs)?;
        bundle = true;
    }

    // Sessions.
    let projects = places.claude_home.join("projects");
    let mut sessions = Vec::new();
    let mut session_files = Vec::new();
    for s in params
        .get("sessions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(id) = s
            .get("session_id")
            .and_then(Value::as_str)
            .filter(|id| looks_like_session_id(id))
        else {
            continue;
        };
        let Some((file, dir)) = find_transcript(&projects, id) else {
            continue;
        };
        let cwd = s
            .get("cwd")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| transcript_cwd(&file))
            .unwrap_or_else(|| main.clone());
        sessions.push(json!({ "id": id, "cwd": cwd }));
        session_files.push((id.to_string(), file, dir.join(id)));
    }

    // Changes per worktree.
    let worktrees: Vec<Value> = chosen
        .iter()
        .enumerate()
        .map(|(i, wt)| {
            json!({
                "index": i,
                "path": wt.path,
                "branch": wt.branch,
                "root": same_path(&wt.path, &main),
            })
        })
        .collect();
    let manifest = json!({
        "version": 1,
        "kind": "repo",
        "name": name,
        "source_root": main,
        "roots": roots,
        "whole": whole,
        "bundle": bundle,
        "remotes": remotes,
        "worktrees": worktrees,
        "sessions": sessions,
    });
    w.add_bytes(MANIFEST, manifest.to_string().as_bytes())
        .map_err(io_err("pack"))?;
    if bundle {
        w.add_file("repo.bundle", &tmp_bundle)
            .map_err(io_err("pack"))?;
        let _ = std::fs::remove_file(&tmp_bundle);
    }
    for (i, wt) in chosen.iter().enumerate() {
        if job.is_cancelled() {
            return Err("cancelled".into());
        }
        job.set_phase(
            &format!(
                "Packing changes in {}",
                wt.branch.as_deref().unwrap_or("worktree")
            ),
            Some(40 + (40 * i / chosen.len()) as u8),
        );
        let dir = Path::new(&wt.path);
        if has_commits {
            let diff = git_in(dir, &["diff", "--binary", "HEAD"])?.stdout;
            if !diff.is_empty() {
                w.add_bytes(&format!("wt/{i}/changes.patch"), &diff)
                    .map_err(io_err("pack"))?;
            }
        }
        let mut files =
            nul_list(&git_in(dir, &["ls-files", "--others", "--exclude-standard", "-z"])?.stdout);
        // The repository's own harness comes along even where it is ignored.
        let harness = git_in(
            dir,
            &[
                "ls-files",
                "--others",
                "--ignored",
                "--exclude-standard",
                "-z",
                "--",
                ".claude",
                "CLAUDE.md",
                "CLAUDE.local.md",
            ],
        )
        .map(|o| nul_list(&o.stdout))
        .unwrap_or_default();
        for f in harness {
            if !files.contains(&f) {
                files.push(f);
            }
        }
        for f in files {
            let path = dir.join(&f);
            if path.is_file() {
                w.add_file(&format!("wt/{i}/files/{f}"), &path)
                    .map_err(io_err(format!("cannot pack {}", path.display())))?;
            }
        }
    }
    job.set_phase("Packing sessions", Some(85));
    for (id, file, sidecar) in session_files {
        w.add_file(&format!("sessions/{id}.jsonl"), &file)
            .map_err(io_err("pack"))?;
        if sidecar.is_dir() {
            w.add_dir(&format!("sessions/{id}"), &sidecar)
                .map_err(io_err("pack"))?;
        }
    }
    Ok(())
}

fn export_harness<W: Write>(
    job: &Job,
    places: &Places,
    params: &Value,
    w: &mut pack::Writer<W>,
) -> crate::Result<()> {
    let items = string_list(params, "items");
    if items.is_empty() {
        return Err("nothing chosen to transfer".into());
    }
    for rel in &items {
        if !harness_item_ok(rel) {
            return Err(format!("{rel} is not a harness item (only instructions, skills, agents, commands, rules and output styles travel)"));
        }
    }
    let present: Vec<&String> = items
        .iter()
        .filter(|rel| places.claude_home.join(rel).exists())
        .collect();
    w.add_bytes(
        MANIFEST,
        json!({ "version": 1, "kind": "harness", "items": present })
            .to_string()
            .as_bytes(),
    )
    .map_err(io_err("pack"))?;
    for (i, rel) in present.iter().enumerate() {
        job.set_phase(
            &format!("Packing {rel}"),
            Some((90 * i / present.len().max(1)) as u8),
        );
        let path = places.claude_home.join(rel);
        if path.is_dir() {
            w.add_dir(&format!("harness/{rel}"), &path)
                .map_err(io_err("pack"))?;
        } else {
            w.add_file(&format!("harness/{rel}"), &path)
                .map_err(io_err("pack"))?;
        }
    }
    Ok(())
}

// ---- import ----------------------------------------------------------------------------

/// Unpacks everything but the manifest into `staging`; the manifest.
fn unpack(job: &Job, pack_path: &Path, staging: &Path) -> crate::Result<Value> {
    let file = std::fs::File::open(pack_path).map_err(io_err("the pack is gone"))?;
    let total = file.metadata().map(|m| m.len()).unwrap_or(1).max(1);
    let mut r = pack::Reader::new(std::io::BufReader::new(file)).map_err(io_err("pack"))?;
    let first = r
        .next_entry()
        .map_err(io_err("pack"))?
        .ok_or("the pack is empty")?;
    if first.name != MANIFEST {
        return Err("the pack has no manifest".into());
    }
    let manifest: Value = serde_json::from_slice(&r.read_body(&first).map_err(io_err("pack"))?)
        .map_err(|e| format!("bad manifest: {e}"))?;
    let mut done = 0u64;
    while let Some(e) = r.next_entry().map_err(io_err("pack"))? {
        if job.is_cancelled() {
            return Err("cancelled".into());
        }
        let dest = pack::safe_join(staging, &e.name)?;
        r.write_body_to(&e, &dest)
            .map_err(io_err(format!("cannot unpack {}", e.name)))?;
        done += e.len;
        job.set_phase("Unpacking", Some((40 * done / total) as u8));
    }
    Ok(manifest)
}

fn import_repo(
    job: &Job,
    places: &Places,
    pack_path: &Path,
    staging: &Path,
    target: &Path,
) -> crate::Result<()> {
    let existing = is_repo_root(target);
    if !existing {
        free_folder(target)?;
    }
    let manifest = unpack(job, pack_path, staging)?;
    let made_target = !target.exists();
    let mut made: Vec<PathBuf> = Vec::new();
    let r = if existing {
        join_repo(job, places, &manifest, staging, target, &mut made)
    } else {
        place_repo(job, places, &manifest, staging, target, &mut made)
    };
    if r.is_err() {
        for p in made.iter().rev() {
            let _ = std::fs::remove_dir_all(p);
        }
        if made_target {
            let _ = std::fs::remove_dir_all(target);
        }
    }
    r
}

fn place_repo(
    job: &Job,
    places: &Places,
    manifest: &Value,
    staging: &Path,
    target: &Path,
    made: &mut Vec<PathBuf>,
) -> crate::Result<()> {
    job.set_phase("Restoring history", Some(45));
    std::fs::create_dir_all(target)
        .map_err(io_err(format!("cannot create {}", target.display())))?;
    git_in(target, &["init", "-q"])?;
    let bundle = staging.join("repo.bundle");
    let has_bundle = manifest
        .get("bundle")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        && bundle.is_file();
    if has_bundle {
        git_in(
            target,
            &[
                "fetch",
                "-q",
                "--update-head-ok",
                &bundle.to_string_lossy(),
                "refs/*:refs/*",
            ],
        )?;
    }
    for r in manifest
        .get("remotes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let (Some(n), Some(u)) = (
            r.get("name").and_then(Value::as_str),
            r.get("url").and_then(Value::as_str),
        ) {
            let _ = git_in(target, &["remote", "add", n, u]);
        }
    }
    let wts: Vec<&Value> = manifest
        .get("worktrees")
        .and_then(Value::as_array)
        .map(|a| a.iter().collect())
        .unwrap_or_default();
    let root_at = wts
        .iter()
        .position(|w| w.get("root").and_then(Value::as_bool) == Some(true))
        .unwrap_or(0);
    let name = manifest
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("repo");
    let mut pairs: Vec<(String, String)> = Vec::new();
    let mut placed: Vec<Value> = Vec::new();
    if let Some(src) = manifest.get("source_root").and_then(Value::as_str) {
        pairs.push((src.to_string(), target.to_string_lossy().into_owned()));
    }
    // The main checkout first, then the linked worktrees.
    let order: Vec<usize> = std::iter::once(root_at)
        .chain((0..wts.len()).filter(|i| *i != root_at))
        .collect();
    for (n, i) in order.into_iter().enumerate() {
        let Some(wt) = wts.get(i) else { continue };
        let branch = wt.get("branch").and_then(Value::as_str).unwrap_or("main");
        let index = wt.get("index").and_then(Value::as_u64).unwrap_or(i as u64);
        job.set_phase(
            &format!("Checking out {branch}"),
            Some(50 + (30 * n / wts.len().max(1)) as u8),
        );
        let dir = if n == 0 {
            if has_bundle {
                git_in(
                    target,
                    &["symbolic-ref", "HEAD", &format!("refs/heads/{branch}")],
                )?;
                git_in(target, &["reset", "-q", "--hard"])?;
            } else {
                git_in(
                    target,
                    &["symbolic-ref", "HEAD", &format!("refs/heads/{branch}")],
                )?;
            }
            target.to_path_buf()
        } else {
            let base = places.worktrees.join(name);
            let mut dir = base.join(branch_slug(branch));
            let mut k = 2;
            while dir.exists() {
                dir = base.join(format!("{}-{k}", branch_slug(branch)));
                k += 1;
            }
            if let Some(parent) = dir.parent() {
                std::fs::create_dir_all(parent).map_err(io_err(parent.display()))?;
            }
            git_in(
                target,
                &["worktree", "add", "-q", &dir.to_string_lossy(), branch],
            )?;
            made.push(dir.clone());
            dir
        };
        let patch = staging.join(format!("wt/{index}/changes.patch"));
        if patch.is_file() {
            git_in(
                &dir,
                &[
                    "apply",
                    "--binary",
                    "--whitespace=nowarn",
                    &patch.to_string_lossy(),
                ],
            )
            .map_err(|e| format!("the uncommitted changes of {branch} did not apply: {e}"))?;
        }
        let files = staging.join(format!("wt/{index}/files"));
        if files.is_dir() {
            copy_tree(&files, &dir)?;
        }
        if let Some(old) = wt.get("path").and_then(Value::as_str) {
            pairs.push((old.to_string(), dir.to_string_lossy().into_owned()));
        }
        placed.push(json!({ "path": dir.to_string_lossy(), "branch": branch, "root": n == 0 }));
    }
    job.set_extra("worktrees", Value::Array(placed));
    place_sessions(job, places, manifest, staging, target, &pairs)
}

/// The sessions of a pack under their new folders, paths inside rewritten.
fn place_sessions(
    job: &Job,
    places: &Places,
    manifest: &Value,
    staging: &Path,
    target: &Path,
    pairs: &[(String, String)],
) -> crate::Result<()> {
    job.set_phase("Placing sessions", Some(85));
    let map = PathMap::new(pairs);
    let projects = places.claude_home.join("projects");
    let mut sessions = Vec::new();
    for s in manifest
        .get("sessions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let (Some(id), Some(cwd)) = (
            s.get("id").and_then(Value::as_str),
            s.get("cwd").and_then(Value::as_str),
        ) else {
            continue;
        };
        if !looks_like_session_id(id) {
            continue;
        }
        let new_cwd = map
            .rewrite(cwd)
            .unwrap_or_else(|| target.to_string_lossy().into_owned());
        let dir = projects.join(project_dir_name(&new_cwd));
        std::fs::create_dir_all(&dir).map_err(io_err(dir.display()))?;
        let src = staging.join(format!("sessions/{id}.jsonl"));
        if let Ok(text) = std::fs::read_to_string(&src) {
            std::fs::write(dir.join(format!("{id}.jsonl")), map.rewrite_jsonl(&text))
                .map_err(io_err("cannot write the transcript"))?;
        }
        let side = staging.join(format!("sessions/{id}"));
        if side.is_dir() {
            for (rel, path) in pack::files_under(&side).map_err(io_err("sessions"))? {
                let dest = pack::safe_join(&dir.join(id), &rel)?;
                if let Some(d) = dest.parent() {
                    std::fs::create_dir_all(d).map_err(io_err(d.display()))?;
                }
                if rel.ends_with(".jsonl") {
                    let text = std::fs::read_to_string(&path).unwrap_or_default();
                    std::fs::write(&dest, map.rewrite_jsonl(&text))
                        .map_err(io_err(dest.display()))?;
                } else {
                    std::fs::copy(&path, &dest).map_err(io_err(dest.display()))?;
                }
            }
        }
        sessions.push(json!({ "id": id, "cwd": new_cwd }));
    }
    job.set_extra("sessions", Value::Array(sessions));
    Ok(())
}

/// A git repository's top folder (it has `.git`).
fn is_repo_root(p: &Path) -> bool {
    p.join(".git").exists()
}

/// A new folder for a worktree of `branch` under `<worktrees>/<repo>/`.
fn worktree_dir(places: &Places, name: &str, branch: &str) -> crate::Result<PathBuf> {
    let base = places.worktrees.join(name);
    let mut dir = base.join(branch_slug(branch));
    let mut k = 2;
    while dir.exists() {
        dir = base.join(format!("{}-{k}", branch_slug(branch)));
        k += 1;
    }
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent).map_err(io_err(parent.display()))?;
    }
    Ok(dir)
}

fn apply_changes(staging: &Path, index: u64, dir: &Path, branch: &str) -> crate::Result<()> {
    let patch = staging.join(format!("wt/{index}/changes.patch"));
    if patch.is_file() {
        git_in(
            dir,
            &[
                "apply",
                "--binary",
                "--whitespace=nowarn",
                &patch.to_string_lossy(),
            ],
        )
        .map_err(|e| format!("the uncommitted changes of {branch} did not apply: {e}"))?;
    }
    let files = staging.join(format!("wt/{index}/files"));
    if files.is_dir() {
        copy_tree(&files, dir)?;
    }
    Ok(())
}

/// The same repository is already at `target`: its history gets what was
/// sent, each branch becomes a worktree of it in the worktrees folder (the
/// one the server that shows it uses, so Herdr lists it in its own "open
/// worktree"), unless the branch is already checked out there at the same
/// commit. Nothing of what was there is overwritten or removed.
fn join_repo(
    job: &Job,
    places: &Places,
    manifest: &Value,
    staging: &Path,
    target: &Path,
    made: &mut Vec<PathBuf>,
) -> crate::Result<()> {
    job.set_phase("Joining the repository there", Some(45));
    let theirs: Vec<String> = git_text(target, &["rev-list", "--max-parents=0", "--all"])
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect();
    let ours = string_list(manifest, "roots");
    let urls = |v: &[Value]| -> Vec<String> {
        v.iter()
            .filter_map(|r| r.get("url").and_then(Value::as_str))
            .map(|u| u.trim_end_matches(".git").to_lowercase())
            .collect()
    };
    let sent_urls = urls(
        manifest
            .get("remotes")
            .and_then(Value::as_array)
            .map(|a| a.as_slice())
            .unwrap_or_default(),
    );
    let their_urls: Vec<String> =
        git_text(target, &["config", "--get-regexp", r"^remote\..*\.url$"])
            .unwrap_or_default()
            .lines()
            .filter_map(|l| {
                l.split_once(' ')
                    .map(|(_, u)| u.trim_end_matches(".git").to_lowercase())
            })
            .collect();
    let same =
        ours.iter().any(|r| theirs.contains(r)) || sent_urls.iter().any(|u| their_urls.contains(u));
    if !same {
        return Err(format!(
            "{} is another repository — pick another name",
            target.display()
        ));
    }
    let bundle = staging.join("repo.bundle");
    if !bundle.is_file() {
        return Err("nothing was sent to add to the repository there (it has no commits)".into());
    }
    git_in(
        target,
        &[
            "fetch",
            "-q",
            &bundle.to_string_lossy(),
            "+refs/heads/*:refs/deck-transfer/*",
        ],
    )?;
    for r in manifest
        .get("remotes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let (Some(n), Some(u)) = (
            r.get("name").and_then(Value::as_str),
            r.get("url").and_then(Value::as_str),
        ) {
            let _ = git_in(target, &["remote", "add", n, u]);
        }
    }
    let checkouts = parse_worktrees(&git_text(target, &["worktree", "list", "--porcelain"])?);
    let name = manifest
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("repo");
    let mut pairs: Vec<(String, String)> = Vec::new();
    let mut placed: Vec<Value> = Vec::new();
    if let Some(src) = manifest.get("source_root").and_then(Value::as_str) {
        pairs.push((src.to_string(), target.to_string_lossy().into_owned()));
    }
    let wts: Vec<&Value> = manifest
        .get("worktrees")
        .and_then(Value::as_array)
        .map(|a| a.iter().collect())
        .unwrap_or_default();
    let result = (|| -> crate::Result<()> {
        for (i, wt) in wts.iter().enumerate() {
            let branch = wt.get("branch").and_then(Value::as_str).unwrap_or("main");
            let index = wt.get("index").and_then(Value::as_u64).unwrap_or(i as u64);
            job.set_phase(
                &format!("Adding {branch}"),
                Some(50 + (30 * i / wts.len().max(1)) as u8),
            );
            let sent = git_text(
                target,
                &["rev-parse", &format!("refs/deck-transfer/{branch}")],
            )
            .map_err(|_| format!("{branch} was not in what was sent"))?;
            let local = git_text(
                target,
                &[
                    "rev-parse",
                    "--verify",
                    "-q",
                    &format!("refs/heads/{branch}"),
                ],
            )
            .ok();
            let out = checkouts
                .iter()
                .find(|c| c.branch.as_deref() == Some(branch));
            match (&local, out) {
                (None, _) => {
                    git_in(target, &["branch", branch, &sent])?;
                }
                (Some(l), _) if *l == sent => {}
                (Some(l), None)
                    if git_in(target, &["merge-base", "--is-ancestor", l, &sent]).is_ok() =>
                {
                    git_in(target, &["branch", "-f", branch, &sent])?;
                }
                (Some(_), Some(c)) => {
                    return Err(format!(
                        "{branch} is checked out at {} there with other commits — commit or switch it there first",
                        c.path
                    ))
                }
                (Some(_), None) => {
                    return Err(format!(
                        "{branch} has other commits there than the ones sent — rename or merge it there first"
                    ))
                }
            }
            let dir = match out {
                Some(c) => {
                    let dirty = !git_text(Path::new(&c.path), &["status", "--porcelain"])
                        .unwrap_or_default()
                        .is_empty();
                    let sends_changes = staging.join(format!("wt/{index}/changes.patch")).is_file()
                        || staging.join(format!("wt/{index}/files")).is_dir();
                    if dirty && sends_changes {
                        return Err(format!(
                            "{branch} is checked out at {} there with its own uncommitted changes",
                            c.path
                        ));
                    }
                    PathBuf::from(&c.path)
                }
                None => {
                    let dir = worktree_dir(places, name, branch)?;
                    git_in(
                        target,
                        &["worktree", "add", "-q", &dir.to_string_lossy(), branch],
                    )?;
                    made.push(dir.clone());
                    dir
                }
            };
            apply_changes(staging, index, &dir, branch)?;
            if let Some(old) = wt.get("path").and_then(Value::as_str) {
                pairs.push((old.to_string(), dir.to_string_lossy().into_owned()));
            }
            placed.push(json!({ "path": dir.to_string_lossy(), "branch": branch, "root": same_path(&dir.to_string_lossy(), &target.to_string_lossy()) }));
        }
        Ok(())
    })();
    // The fetched refs were only a way in.
    for r in git_text(
        target,
        &["for-each-ref", "--format=%(refname)", "refs/deck-transfer"],
    )
    .unwrap_or_default()
    .lines()
    {
        let _ = git_in(target, &["update-ref", "-d", r]);
    }
    result?;
    job.set_extra("worktrees", Value::Array(placed));
    job.set_extra("joined", json!(true));
    place_sessions(job, places, manifest, staging, target, &pairs)
}

/// A free name next to `dest`: `name-2`, `name-2.md`, …
fn keep_both_name(dest: &Path) -> PathBuf {
    let parent = dest.parent().map(Path::to_path_buf).unwrap_or_default();
    let file = dest
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (stem, ext) = match file.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() && !dest.is_dir() => (s.to_string(), format!(".{e}")),
        _ => (file.clone(), String::new()),
    };
    (2..)
        .map(|k| parent.join(format!("{stem}-{k}{ext}")))
        .find(|p| !p.exists())
        .unwrap_or_else(|| dest.to_path_buf())
}

fn import_harness(
    job: &Job,
    places: &Places,
    pack_path: &Path,
    staging: &Path,
    on_conflict: &str,
) -> crate::Result<()> {
    let manifest = unpack(job, pack_path, staging)?;
    let mut copied = Vec::new();
    let mut skipped = Vec::new();
    let mut renamed = Vec::new();
    for rel in string_list(&manifest, "items") {
        if !harness_item_ok(&rel) {
            skipped.push(rel);
            continue;
        }
        let src = staging.join("harness").join(&rel);
        if !src.exists() {
            continue;
        }
        let mut dest = places.claude_home.join(&rel);
        if dest.exists() {
            match on_conflict {
                "overwrite" => remove_any(&dest)?,
                "keep_both" => {
                    dest = keep_both_name(&dest);
                    renamed.push(json!({ "item": rel, "as": dest.strip_prefix(&places.claude_home).unwrap_or(&dest).to_string_lossy().replace('\\', "/") }));
                }
                _ => {
                    skipped.push(rel);
                    continue;
                }
            }
        }
        copy_tree(&src, &dest)?;
        copied.push(rel);
    }
    job.set_extra("copied", json!(copied));
    job.set_extra("skipped", json!(skipped));
    job.set_extra("renamed", json!(renamed));
    Ok(())
}

// ---- removal after a move ----------------------------------------------------------------

fn remove_repo(job: &Job, places: &Places, params: &Value) -> crate::Result<()> {
    let repo = PathBuf::from(text(params, "repo")?);
    let whole = params
        .get("whole")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let projects = places.claude_home.join("projects");
    for s in params
        .get("sessions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(id) = s
            .as_str()
            .or_else(|| s.get("session_id")?.as_str())
            .filter(|id| looks_like_session_id(id))
        else {
            continue;
        };
        if let Some((file, dir)) = find_transcript(&projects, id) {
            remove_any(&file)?;
            remove_any(&dir.join(id))?;
        }
    }
    if !repo.exists() {
        return Ok(());
    }
    let top = git_text(&repo, &["rev-parse", "--show-toplevel"])
        .map_err(|e| format!("{} is not a git repository: {e}", repo.display()))?;
    let all = parse_worktrees(&git_text(&repo, &["worktree", "list", "--porcelain"])?);
    let main = all.first().map(|w| w.path.clone()).unwrap_or(top);
    let main_path = PathBuf::from(&main);
    if let Some(home) = crate::home_dir() {
        if same_path(&main, &home.to_string_lossy()) {
            return Err("refusing to remove the home folder".into());
        }
    }
    if let Some(inside) = &places.removable {
        let real = std::fs::canonicalize(&main_path).unwrap_or_else(|_| main_path.clone());
        let base = std::fs::canonicalize(inside).unwrap_or_else(|_| inside.clone());
        if !real.starts_with(&base) || real == base {
            return Err(format!(
                "refusing to remove {main}: it is not inside {}",
                inside.display()
            ));
        }
    }
    if whole {
        job.set_phase("Removing the repository", Some(30));
        for wt in all.iter().skip(1) {
            let _ = git_in(&main_path, &["worktree", "remove", "--force", &wt.path]);
            let _ = remove_any(Path::new(&wt.path));
        }
        return remove_any(&main_path);
    }
    for w in params
        .get("worktrees")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(path) = w.get("path").and_then(Value::as_str) else {
            continue;
        };
        let Some(known) = all.iter().skip(1).find(|a| same_path(&a.path, path)) else {
            // The main checkout stays: it is the repository itself.
            continue;
        };
        job.set_phase(
            &format!("Removing {}", known.branch.as_deref().unwrap_or(path)),
            Some(50),
        );
        git_in(&main_path, &["worktree", "remove", "--force", &known.path])?;
        if let Some(b) = &known.branch {
            let _ = git_in(&main_path, &["branch", "-D", b]);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
