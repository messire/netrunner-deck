//! The agent harness: Claude Code's configuration as it applies to each
//! repository (and the personal, machine-wide one in `~/.claude`). Deck lists
//! what is there — instructions, settings, skills, agents, commands, rules,
//! MCP servers — and reads the files for the viewer. The repository set comes
//! from the archive (the frontend sends the roots it knows); nothing here
//! decides which repositories exist.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

pub const EVENT: &str = "deck://harness";

/// Longest file the viewer gets; larger files are cut with a marker.
const MAX_READ: u64 = 512 * 1024;

#[derive(Debug, Clone, Deserialize)]
pub struct RepoRef {
    pub key: String,
    pub name: String,
    pub root: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Item {
    /// claude_md | settings | skill | agent | command | rule | mcp | hook
    pub kind: String,
    pub name: String,
    pub path: String,
    pub size: u64,
    pub mtime_ms: u64,
    /// true for `*.local.*` files (per machine, usually untracked)
    pub local: bool,
    /// frontmatter `description` (skills, agents, commands), when present
    pub description: Option<String>,
    /// frontmatter `tags`, lowercase; empty until written (by the author or by CLASSIFY)
    pub tags: Vec<String>,
    /// frontmatter `argument-hint` (what to type after `/name`), when present
    #[serde(default)]
    pub argument_hint: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HarnessRepo {
    pub key: String,
    pub name: String,
    pub root: String,
    pub personal: bool,
    pub exists: bool,
    pub items: Vec<Item>,
}

/// Roots the last scan covered; `read` serves files under these only.
#[derive(Default)]
pub struct Roots(pub Mutex<Vec<PathBuf>>);

/// Repos whose CLASSIFY run is in flight (one at a time per repo).
#[derive(Default)]
pub struct Classifying(pub Mutex<std::collections::BTreeSet<String>>);

// ---- frontmatter -------------------------------------------------------------

/// The `---` block at the top of a markdown file: `(frontmatter text, body)`;
/// `None` when the file has none.
fn split_frontmatter(text: &str) -> Option<(&str, &str)> {
    let rest = text.strip_prefix("---")?;
    let rest = rest
        .strip_prefix("\r\n")
        .or_else(|| rest.strip_prefix('\n'))?;
    let end = rest.find("\n---").map(|i| (i, i + 4))?;
    let fm = &rest[..end.0];
    let mut body = &rest[end.1..];
    body = body.strip_prefix('\r').unwrap_or(body);
    body = body.strip_prefix('\n').unwrap_or(body);
    Some((fm, body))
}

/// One top-level scalar of a (simple) YAML frontmatter, quotes stripped.
fn fm_scalar(fm: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}:");
    let v = fm
        .lines()
        .find(|l| l.starts_with(&prefix))?
        .get(prefix.len()..)?
        .trim();
    let v = v.trim_matches(|c| c == '"' || c == '\'');
    (!v.is_empty()).then(|| v.to_string())
}

/// `tags:` as `[a, b]`, `a, b`, or a `- a` list on the following lines; lowercased.
fn fm_tags(fm: &str) -> Vec<String> {
    let mut lines = fm.lines().peekable();
    while let Some(l) = lines.next() {
        if let Some(v) = l.strip_prefix("tags:") {
            let v = v.trim();
            let mut raw: Vec<String> = Vec::new();
            if v.is_empty() {
                while let Some(n) = lines.peek() {
                    let t = n.trim_start();
                    if let Some(item) = t.strip_prefix("- ") {
                        raw.push(item.to_string());
                        lines.next();
                    } else {
                        break;
                    }
                }
            } else {
                let inner = v.trim_start_matches('[').trim_end_matches(']');
                raw.extend(inner.split(',').map(str::to_string));
            }
            let mut out: Vec<String> = raw
                .iter()
                .map(|t| {
                    t.trim()
                        .trim_matches(|c| c == '"' || c == '\'')
                        .to_lowercase()
                })
                .filter(|t| !t.is_empty())
                .collect();
            out.dedup();
            return out;
        }
    }
    Vec::new()
}

/// `text` with `tags: [..]` set in its frontmatter (added when missing; a
/// frontmatter block is created when the file has none). Line endings kept.
pub fn with_tags(text: &str, tags: &[String]) -> String {
    let crlf = text.contains("\r\n");
    let nl = if crlf { "\r\n" } else { "\n" };
    let line = format!("tags: [{}]", tags.join(", "));
    match split_frontmatter(text) {
        Some((fm, body)) => {
            let mut out: Vec<String> = Vec::new();
            let mut done = false;
            let mut lines = fm.lines().peekable();
            while let Some(l) = lines.next() {
                if l.starts_with("tags:") {
                    out.push(line.clone());
                    done = true;
                    // drop a following `- item` list
                    while lines
                        .peek()
                        .is_some_and(|n| n.trim_start().starts_with("- "))
                    {
                        lines.next();
                    }
                } else {
                    out.push(l.trim_end_matches('\r').to_string());
                }
            }
            if !done {
                out.push(line);
            }
            format!("---{nl}{}{nl}---{nl}{body}", out.join(nl))
        }
        None => format!("---{nl}{line}{nl}---{nl}{text}"),
    }
}

type Meta = (Option<String>, Vec<String>, Option<String>);
/// `(mtime ms, len)` of a file when its frontmatter was parsed.
type Stamp = (u64, u64);
type MetaCache = Mutex<std::collections::HashMap<PathBuf, (Stamp, Meta)>>;

/// Frontmatter already parsed, by path and `(mtime, len)`: a rescan after an
/// archive refresh touches hundreds of files and almost none have changed.
fn meta_cache() -> &'static MetaCache {
    static CACHE: std::sync::OnceLock<MetaCache> = std::sync::OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

/// Description, tags and argument hint of a markdown harness file (empty for other kinds).
fn meta(path: &Path) -> Meta {
    if path.extension().and_then(|e| e.to_str()) != Some("md") {
        return (None, Vec::new(), None);
    }
    let stamp = std::fs::metadata(path)
        .ok()
        .map(|m| {
            let mtime = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            (mtime, m.len())
        })
        .unwrap_or((0, 0));
    if let Ok(cache) = meta_cache().lock() {
        if let Some((seen, meta)) = cache.get(path) {
            if *seen == stamp {
                return meta.clone();
            }
        }
    }
    let meta = meta_uncached(path);
    if let Ok(mut cache) = meta_cache().lock() {
        cache.insert(path.to_path_buf(), (stamp, meta.clone()));
    }
    meta
}

fn meta_uncached(path: &Path) -> Meta {
    let Ok(text) = std::fs::read_to_string(path) else {
        return (None, Vec::new(), None);
    };
    match split_frontmatter(&text) {
        Some((fm, _)) => (
            fm_scalar(fm, "description"),
            fm_tags(fm),
            fm_scalar(fm, "argument-hint").or_else(|| fm_scalar(fm, "argument_hint")),
        ),
        None => (None, Vec::new(), None),
    }
}

fn home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    let key = "USERPROFILE";
    #[cfg(not(windows))]
    let key = "HOME";
    std::env::var_os(key).map(PathBuf::from)
}

fn plain(p: &Path) -> String {
    let s = p.to_string_lossy();
    s.strip_prefix(r"\\?\").unwrap_or(&s).replace('\\', "/")
}

fn item(kind: &str, name: &str, path: &Path) -> Option<Item> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    let mtime_ms = meta
        .modified()
        .ok()
        .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let file = path
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_default();
    let (description, tags, argument_hint) = self::meta(path);
    Some(Item {
        kind: kind.to_string(),
        name: name.to_string(),
        path: plain(path),
        size: meta.len(),
        mtime_ms,
        local: file.contains(".local."),
        description,
        tags,
        argument_hint,
    })
}

/// `dir/*/SKILL.md` → one item per skill directory.
fn skills(dir: &Path, out: &mut Vec<Item>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut found: Vec<Item> = rd
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            item("skill", &name, &e.path().join("SKILL.md"))
        })
        .collect();
    found.sort_by(|a, b| a.name.cmp(&b.name));
    out.append(&mut found);
}

/// `dir/**/*.md` → one item per file, named by its path inside `dir` without `.md`.
fn markdown_tree(kind: &str, dir: &Path, prefix: &str, out: &mut Vec<Item>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let path = e.path();
        let file = e.file_name().to_string_lossy().to_string();
        if path.is_dir() {
            markdown_tree(kind, &path, &format!("{prefix}{file}/"), out);
        } else if let Some(stem) = file.strip_suffix(".md") {
            if let Some(i) = item(kind, &format!("{prefix}{stem}"), &path) {
                out.push(i);
            }
        }
    }
}

/// Everything Claude Code reads from one `.claude`-style directory.
fn claude_dir(dir: &Path, out: &mut Vec<Item>) {
    for f in ["settings.json", "settings.local.json"] {
        if let Some(i) = item("settings", f, &dir.join(f)) {
            out.push(i);
        }
    }
    skills(&dir.join("skills"), out);
    markdown_tree("agent", &dir.join("agents"), "", out);
    markdown_tree("command", &dir.join("commands"), "", out);
    markdown_tree("rule", &dir.join("rules"), "", out);
}

fn scan_repo(root: &Path) -> Vec<Item> {
    let mut out = Vec::new();
    for f in ["CLAUDE.md", "CLAUDE.local.md"] {
        if let Some(i) = item("claude_md", f, &root.join(f)) {
            out.push(i);
        }
    }
    if let Some(i) = item(
        "claude_md",
        ".claude/CLAUDE.md",
        &root.join(".claude").join("CLAUDE.md"),
    ) {
        out.push(i);
    }
    if let Some(i) = item("mcp", ".mcp.json", &root.join(".mcp.json")) {
        out.push(i);
    }
    claude_dir(&root.join(".claude"), &mut out);
    out
}

fn scan_personal(home: &Path) -> Vec<Item> {
    let dir = home.join(".claude");
    let mut out = Vec::new();
    if let Some(i) = item("claude_md", "CLAUDE.md", &dir.join("CLAUDE.md")) {
        out.push(i);
    }
    claude_dir(&dir, &mut out);
    out
}

/// Personal (`~/.claude`) first, then the repositories in the order given.
pub fn scan(repos: &[RepoRef]) -> (Vec<HarnessRepo>, Vec<PathBuf>) {
    let mut out = Vec::with_capacity(repos.len() + 1);
    let mut roots = Vec::with_capacity(repos.len() + 1);
    if let Some(home) = home_dir() {
        let dir = home.join(".claude");
        out.push(HarnessRepo {
            key: "personal".into(),
            name: "Personal".into(),
            root: plain(&dir),
            personal: true,
            exists: dir.is_dir(),
            items: scan_personal(&home),
        });
        roots.push(dir);
    }
    for r in repos {
        let root = PathBuf::from(&r.root);
        let exists = root.is_dir();
        out.push(HarnessRepo {
            key: r.key.clone(),
            name: r.name.clone(),
            root: plain(&root),
            personal: false,
            exists,
            items: if exists { scan_repo(&root) } else { Vec::new() },
        });
        roots.push(root);
    }
    (out, roots)
}

fn under(path: &Path, roots: &[PathBuf]) -> bool {
    let p = plain(path).to_lowercase();
    roots.iter().any(|r| {
        let r = plain(r).to_lowercase();
        p == r || p.starts_with(&format!("{r}/"))
    })
}

/// Whether `path` lies under one of the scanned roots (no `..`, case-insensitive prefix).
pub fn is_under_roots(path: &str, roots: &[PathBuf]) -> bool {
    let p = PathBuf::from(path);
    !p.components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
        && under(&p, roots)
}

/// Text of a harness file, cut at `MAX_READ`. Only files under a scanned root.
pub fn read(path: &str, roots: &[PathBuf]) -> Result<String, String> {
    let p = PathBuf::from(path);
    if p.components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
        || !under(&p, roots)
    {
        return Err("not a harness file".into());
    }
    let meta = std::fs::metadata(&p).map_err(|e| format!("{path}: {e}"))?;
    let bytes = std::fs::read(&p).map_err(|e| format!("{path}: {e}"))?;
    let cut = bytes.len() as u64 > MAX_READ;
    let slice = if cut {
        &bytes[..MAX_READ as usize]
    } else {
        &bytes[..]
    };
    let mut text = String::from_utf8_lossy(slice).to_string();
    if cut {
        text.push_str(&format!(
            "\n\n… cut at {} of {} bytes",
            MAX_READ,
            meta.len()
        ));
    }
    Ok(text)
}

// ---- CLASSIFY: tags from a shadow claude -p, written into the files ------------

const CLASSIFY_PROMPT: &str = "You tag Claude Code skills, agents, commands and rules by what they are for. \
Input: numbered items, each `N. name — description` (and sometimes the first lines of the file). \
Reply with ONLY a JSON object mapping each number (as a string) to an array of 1 to 3 tags. \
Tags are lowercase single words or hyphenated words, reused consistently across items; prefer these when they fit: \
analytics, sql, csharp, dotnet, python, typescript, frontend, backend, architecture, devops, ci, testing, docs, git, \
incident, monitoring, security, data, api, process, review, planning, research, writing, slack, clickup, jira. \
Invent a new tag only when none fits. No prose, no markdown fences.";

/// Items CLASSIFY may tag: markdown files that carry instructions.
fn taggable(i: &Item) -> bool {
    matches!(i.kind.as_str(), "skill" | "agent" | "command" | "rule")
        && i.path.ends_with(".md")
        && i.tags.is_empty()
}

fn classify_input(items: &[&Item]) -> String {
    let mut s = String::new();
    for (n, i) in items.iter().enumerate() {
        let desc = i.description.clone().unwrap_or_else(|| {
            // no description: the first lines of the body stand in
            std::fs::read_to_string(&i.path)
                .ok()
                .map(|t| {
                    let body = split_frontmatter(&t)
                        .map(|(_, b)| b.to_string())
                        .unwrap_or(t);
                    body.lines()
                        .filter(|l| !l.trim().is_empty())
                        .take(4)
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .unwrap_or_default()
        });
        let desc: String = desc.chars().take(300).collect();
        s.push_str(&format!(
            "{}. {} — {}\n",
            n + 1,
            i.name,
            desc.replace('\n', " ")
        ));
    }
    s
}

fn parse_classification(reply: &str) -> Result<BTreeMap<usize, Vec<String>>, String> {
    let start = reply.find('{').ok_or("no JSON in reply")?;
    let end = reply.rfind('}').ok_or("no JSON in reply")?;
    let v: serde_json::Value =
        serde_json::from_str(&reply[start..=end]).map_err(|e| format!("bad JSON: {e}"))?;
    let obj = v.as_object().ok_or("reply is not an object")?;
    let mut out = BTreeMap::new();
    for (k, tags) in obj {
        let Ok(n) = k.trim().parse::<usize>() else {
            continue;
        };
        let list: Vec<String> = tags
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|t| t.as_str())
                    .map(|t| t.trim().to_lowercase().replace(' ', "-"))
                    .filter(|t| !t.is_empty() && t.len() <= 24)
                    .take(3)
                    .collect()
            })
            .unwrap_or_default();
        if !list.is_empty() {
            out.insert(n, list);
        }
    }
    Ok(out)
}

/// Tag the untagged items of one repo in a background thread: one shadow
/// `claude -p` per repo, tags written into each file's frontmatter, then
/// `deck://harness` so the view rescans. Returns false when already running.
pub fn classify(app: AppHandle, key: String, items: Vec<Item>) -> bool {
    {
        let state = app.state::<Classifying>();
        let Ok(mut set) = state.0.lock() else {
            return false;
        };
        if !set.insert(key.clone()) {
            return false;
        }
    }
    std::thread::spawn(move || {
        let result = (|| -> Result<usize, String> {
            let todo: Vec<&Item> = items.iter().filter(|i| taggable(i)).collect();
            if todo.is_empty() {
                return Ok(0);
            }
            let workdir = crate::archive::shadow_workdir(&app).ok_or("no app data dir")?;
            let mut written = 0;
            // ≤ 40 items per call keeps the reply well inside one answer.
            for chunk in todo.chunks(40) {
                let reply = crate::archive::shadow_prompt(
                    &workdir,
                    CLASSIFY_PROMPT,
                    &classify_input(chunk),
                    20_000,
                )?;
                let map = parse_classification(&reply)?;
                for (n, tags) in map {
                    let Some(item) = chunk.get(n.wrapping_sub(1)) else {
                        continue;
                    };
                    let path = PathBuf::from(&item.path);
                    let Ok(text) = std::fs::read_to_string(&path) else {
                        continue;
                    };
                    if std::fs::write(&path, with_tags(&text, &tags)).is_ok() {
                        written += 1;
                    }
                }
            }
            Ok(written)
        })();
        if let Ok(mut set) = app.state::<Classifying>().0.lock() {
            set.remove(&key);
        }
        let payload = match result {
            Ok(n) => serde_json::json!({ "key": key, "written": n }),
            Err(e) => serde_json::json!({ "key": key, "error": e }),
        };
        let _ = app.emit(EVENT, payload);
    });
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_lists_instructions_settings_skills_agents_commands_rules_and_mcp() {
        let dir = std::env::temp_dir().join(format!("deck-harness-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let c = dir.join(".claude");
        std::fs::create_dir_all(c.join("skills").join("deploy")).unwrap();
        std::fs::create_dir_all(c.join("agents")).unwrap();
        std::fs::create_dir_all(c.join("commands").join("git")).unwrap();
        std::fs::create_dir_all(c.join("rules")).unwrap();
        std::fs::write(dir.join("CLAUDE.md"), "# repo").unwrap();
        std::fs::write(dir.join(".mcp.json"), "{}").unwrap();
        std::fs::write(c.join("settings.json"), "{}").unwrap();
        std::fs::write(c.join("settings.local.json"), "{}").unwrap();
        std::fs::write(c.join("skills").join("deploy").join("SKILL.md"), "s").unwrap();
        std::fs::write(c.join("agents").join("reviewer.md"), "a").unwrap();
        std::fs::write(
            c.join("commands").join("git").join("sync.md"),
            "---\ndescription: Sync\nargument-hint: [branch]\n---\nc",
        )
        .unwrap();
        std::fs::write(c.join("rules").join("style.md"), "r").unwrap();
        let items = scan_repo(&dir);
        let got: Vec<(String, String, bool)> = items
            .iter()
            .map(|i| (i.kind.clone(), i.name.clone(), i.local))
            .collect();
        assert_eq!(
            got,
            vec![
                ("claude_md".into(), "CLAUDE.md".into(), false),
                ("mcp".into(), ".mcp.json".into(), false),
                ("settings".into(), "settings.json".into(), false),
                ("settings".into(), "settings.local.json".into(), true),
                ("skill".into(), "deploy".into(), false),
                ("agent".into(), "reviewer".into(), false),
                ("command".into(), "git/sync".into(), false),
                ("rule".into(), "style".into(), false),
            ]
        );
        let roots = vec![dir.clone()];
        assert_eq!(read(&items[0].path, &roots).unwrap(), "# repo");
        let sync = items.iter().find(|i| i.name == "git/sync").unwrap();
        assert_eq!(sync.description.as_deref(), Some("Sync"));
        assert_eq!(sync.argument_hint.as_deref(), Some("[branch]"));
        assert!(items[0].tags.is_empty());
        assert!(read(&plain(&dir.join("..").join("x")), &roots).is_err());
        assert!(read("C:/Windows/win.ini", &roots).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn frontmatter_tags_are_read_and_written_in_place() {
        let fm =
            "---\nname: deploy\ndescription: \"Ship it\"\ntags: [SQL, Analytics]\n---\n# body\n";
        let (f, body) = split_frontmatter(fm).unwrap();
        assert_eq!(fm_scalar(f, "description").as_deref(), Some("Ship it"));
        assert_eq!(fm_tags(f), vec!["sql", "analytics"]);
        assert_eq!(body, "# body\n");
        let listed = "---\ntags:\n  - a\n  - b\nname: x\n---\nbody";
        assert_eq!(
            fm_tags(split_frontmatter(listed).unwrap().0),
            vec!["a", "b"]
        );
        let t = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            with_tags(listed, &t(&["c"])),
            "---\ntags: [c]\nname: x\n---\nbody"
        );
        assert_eq!(
            with_tags("---\nname: x\n---\nbody", &t(&["c", "d"])),
            "---\nname: x\ntags: [c, d]\n---\nbody"
        );
        assert_eq!(
            with_tags("# plain\n", &t(&["c"])),
            "---\ntags: [c]\n---\n# plain\n"
        );
        assert_eq!(
            with_tags("---\r\nname: x\r\n---\r\nbody", &t(&["c"])),
            "---\r\nname: x\r\ntags: [c]\r\n---\r\nbody"
        );
    }

    #[test]
    fn classification_reply_is_parsed_leniently() {
        let m = parse_classification("Here you go:\n{\"1\": [\"SQL\", \"Data Model\"], \"2\": [], \"x\": [\"a\"], \"3\": [\"a\",\"b\",\"c\",\"d\"]}").unwrap();
        assert_eq!(
            m.get(&1).unwrap(),
            &vec!["sql".to_string(), "data-model".to_string()]
        );
        assert!(!m.contains_key(&2));
        assert_eq!(m.get(&3).unwrap().len(), 3);
        assert!(parse_classification("nope").is_err());
    }
}
