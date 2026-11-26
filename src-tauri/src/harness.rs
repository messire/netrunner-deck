//! HARNESS on Deck's side: the scan and the files come from each server's
//! machine (`deck_accounts::harness`, asked as `deck.harness.*`); CLASSIFY
//! (tags from a shadow `claude -p`, written into the files) runs here for
//! the files of this computer.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

use tauri::{AppHandle, Emitter, Manager};

pub use deck_accounts::harness::*;

pub const EVENT: &str = "deck://harness";

/// Roots the last scan of each server covered: its files are read and
/// (on this computer) CLASSIFY writes under these only.
#[derive(Default)]
pub struct Roots(pub Mutex<std::collections::HashMap<String, Vec<PathBuf>>>);

/// Repos whose CLASSIFY run is in flight (one at a time per repo).
#[derive(Default)]
pub struct Classifying(pub Mutex<std::collections::BTreeSet<String>>);

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
