//! UPLINK on Deck's side: the log of what each pull brought in, kept in
//! Deck's `uplink.json`, each entry tagged with its server. The git work
//! itself (`deck.uplink.*`) runs on the server's machine
//! (`deck_accounts::uplink`).

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

pub use deck_accounts::uplink::{Sync, Update};

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
