//! `deck.secrets.*`: the one environment file Claude Code reads on this
//! machine, `<home>/.claude/.env`, for Deck's PREFERENCES › SECRETS.
//!
//! * `deck.secrets.read {}` → `{path, exists, text}`;
//! * `deck.secrets.write {text}` → `{path}`.
//!
//! There is no path parameter: nothing but that file can be read or written
//! through these. A write goes to a temporary file next to it and is renamed
//! over it, so a reader never sees half a file; on Unix the file is 0600.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

/// The largest file read or written.
pub const MAX_BYTES: usize = 256 * 1024;

/// `<home>/.claude/.env`.
pub fn env_path(home: &Path) -> PathBuf {
    home.join(".claude").join(".env")
}

fn home() -> crate::Result<PathBuf> {
    crate::home_dir().ok_or_else(|| "no home folder on this machine".to_string())
}

/// `deck.secrets.read` against `home`.
pub fn read_in(home: &Path) -> crate::Result<Value> {
    let path = env_path(home);
    let shown = path.to_string_lossy().to_string();
    match std::fs::metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(json!({ "path": shown, "exists": false, "text": "" }))
        }
        Err(e) => return Err(format!("{shown}: {e}")),
        Ok(m) if !m.is_file() => return Err(format!("{shown} is not a file")),
        Ok(m) if m.len() > MAX_BYTES as u64 => {
            return Err(format!("{shown} is larger than {} KB", MAX_BYTES / 1024))
        }
        Ok(_) => {}
    }
    let bytes = std::fs::read(&path).map_err(|e| format!("{shown}: {e}"))?;
    let text = String::from_utf8(bytes).map_err(|_| format!("{shown} is not UTF-8 text"))?;
    Ok(json!({ "path": shown, "exists": true, "text": text }))
}

/// `deck.secrets.write {text}` against `home`.
pub fn write_in(home: &Path, text: &str) -> crate::Result<Value> {
    if text.len() > MAX_BYTES {
        return Err(format!("the file may be at most {} KB", MAX_BYTES / 1024));
    }
    if text.contains('\0') {
        return Err("the text has a NUL byte".to_string());
    }
    let path = env_path(home);
    let shown = path.to_string_lossy().to_string();
    let dir = path.parent().expect("<home>/.claude");
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    if path.symlink_metadata().map(|m| m.is_dir()).unwrap_or(false) {
        return Err(format!("{shown} is a folder"));
    }
    let tmp = dir.join(format!(".env.deck-{}.tmp", std::process::id()));
    let written = (|| -> std::io::Result<()> {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut f = options.open(&tmp)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, &path)
    })();
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("{shown}: {e}"));
    }
    Ok(json!({ "path": shown }))
}

/// One `deck.secrets.<method>` request on this machine.
pub fn call(method: &str, params: &Value) -> crate::Result<Value> {
    match method {
        "read" => read_in(&home()?),
        "write" => {
            let text = params
                .get("text")
                .and_then(Value::as_str)
                .ok_or_else(|| "text is required".to_string())?;
            write_in(&home()?, text)
        }
        other => Err(format!("unknown method deck.secrets.{other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn home() -> PathBuf {
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "deck-secrets-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_missing_file_reads_as_empty() {
        let h = home();
        let r = read_in(&h).unwrap();
        assert_eq!(r["exists"], false);
        assert_eq!(r["text"], "");
        assert!(r["path"].as_str().unwrap().ends_with(".env"));
    }

    #[test]
    fn write_creates_the_folder_and_reads_back() {
        let h = home();
        assert!(!h.join(".claude").exists());
        let text = "# fake\nFAKE_KEY=not-a-secret\r\nexport OTHER=\"a b\"\n";
        let w = write_in(&h, text).unwrap();
        assert_eq!(w["path"], env_path(&h).to_string_lossy().as_ref());
        let r = read_in(&h).unwrap();
        assert_eq!(r["exists"], true);
        assert_eq!(r["text"], text);
        // Nothing else is left beside it.
        let names: Vec<_> = std::fs::read_dir(h.join(".claude"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from(".env")]);
    }

    #[test]
    fn write_replaces_the_whole_file() {
        let h = home();
        write_in(&h, "A=1\nB=2\nC=3\n").unwrap();
        write_in(&h, "A=9\n").unwrap();
        assert_eq!(read_in(&h).unwrap()["text"], "A=9\n");
    }

    #[test]
    fn nul_bytes_and_big_files_are_refused() {
        let h = home();
        assert!(write_in(&h, "A=1\0\n").unwrap_err().contains("NUL"));
        assert!(write_in(&h, &"x".repeat(MAX_BYTES + 1)).is_err());
        assert!(!env_path(&h).exists());
        std::fs::create_dir_all(h.join(".claude")).unwrap();
        std::fs::write(env_path(&h), "x".repeat(MAX_BYTES + 1)).unwrap();
        assert!(read_in(&h).is_err());
    }

    #[test]
    fn write_needs_text_and_methods_are_fixed() {
        assert!(call("write", &json!({})).unwrap_err().contains("text"));
        assert!(call("delete", &json!({})).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn the_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let h = home();
        write_in(&h, "A=1\n").unwrap();
        let mode = std::fs::metadata(env_path(&h))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
