//! The file a transfer carries from one machine to another: a flat list of
//! named byte runs (`DECKPACK1\n`, then per entry a `u32` name length, the
//! UTF-8 name, a `u64` length and the bytes, all little-endian). Names are
//! relative paths with `/`; a reader never lets one leave its folder.
//!
//! Small on purpose: no compression, no permissions beyond the executable
//! bit (a name ending in `\u{0}x`), so both sides need nothing but this.

use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};

pub const MAGIC: &[u8] = b"DECKPACK1\n";
/// Marks an executable file (Unix mode bit) at the end of its name.
const EXEC: &str = "\u{0}x";

pub struct Writer<W: Write> {
    out: W,
    written: u64,
}

impl<W: Write> Writer<W> {
    pub fn new(mut out: W) -> io::Result<Self> {
        out.write_all(MAGIC)?;
        Ok(Writer {
            out,
            written: MAGIC.len() as u64,
        })
    }

    fn head(&mut self, name: &str, len: u64) -> io::Result<()> {
        check_name(name).map_err(io::Error::other)?;
        let n = name.as_bytes();
        self.out.write_all(&(n.len() as u32).to_le_bytes())?;
        self.out.write_all(n)?;
        self.out.write_all(&len.to_le_bytes())?;
        self.written += 12 + n.len() as u64 + len;
        Ok(())
    }

    pub fn add_bytes(&mut self, name: &str, bytes: &[u8]) -> io::Result<()> {
        self.head(name, bytes.len() as u64)?;
        self.out.write_all(bytes)
    }

    /// The file at `path` as `name` (marked executable when it is).
    pub fn add_file(&mut self, name: &str, path: &Path) -> io::Result<()> {
        let mut file = std::fs::File::open(path)?;
        let meta = file.metadata()?;
        let name = if is_executable(&meta) {
            format!("{name}{EXEC}")
        } else {
            name.to_string()
        };
        self.head(&name, meta.len())?;
        let copied = io::copy(&mut (&mut file).take(meta.len()), &mut self.out)?;
        if copied != meta.len() {
            return Err(io::Error::other(format!(
                "{} changed while it was packed",
                path.display()
            )));
        }
        Ok(())
    }

    /// Every file under `dir` as `prefix/<relative path>`.
    pub fn add_dir(&mut self, prefix: &str, dir: &Path) -> io::Result<()> {
        for (rel, path) in files_under(dir)? {
            self.add_file(&format!("{prefix}/{rel}"), &path)?;
        }
        Ok(())
    }

    pub fn written(&self) -> u64 {
        self.written
    }

    pub fn finish(mut self) -> io::Result<W> {
        self.out.flush()?;
        Ok(self.out)
    }
}

#[cfg(unix)]
fn is_executable(meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o111 != 0
}
#[cfg(not(unix))]
fn is_executable(_: &std::fs::Metadata) -> bool {
    false
}

/// Files (not folders, not symlinks) under `dir`, `(relative path with /,
/// full path)`, in name order.
pub fn files_under(dir: &Path) -> io::Result<Vec<(String, PathBuf)>> {
    let mut out = Vec::new();
    let mut stack = vec![(String::new(), dir.to_path_buf())];
    while let Some((rel, path)) = stack.pop() {
        let mut entries: Vec<_> = std::fs::read_dir(&path)?.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let name = e.file_name().to_string_lossy().into_owned();
            let child_rel = if rel.is_empty() {
                name
            } else {
                format!("{rel}/{name}")
            };
            let kind = e.file_type()?;
            if kind.is_dir() {
                stack.push((child_rel, e.path()));
            } else if kind.is_file() {
                out.push((child_rel, e.path()));
            }
        }
    }
    out.sort();
    Ok(out)
}

/// A name is a relative path of plain parts.
pub fn check_name(name: &str) -> Result<(), String> {
    let plain = name.strip_suffix(EXEC).unwrap_or(name);
    if plain.is_empty() || plain.contains('\0') || plain.contains('\\') {
        return Err(format!("bad name in pack: {plain:?}"));
    }
    for part in plain.split('/') {
        if part.is_empty() || part == "." || part == ".." || part.contains(':') {
            return Err(format!("bad name in pack: {plain:?}"));
        }
    }
    Ok(())
}

/// `dir` joined with a checked pack name.
pub fn safe_join(dir: &Path, name: &str) -> Result<PathBuf, String> {
    check_name(name)?;
    let mut out = dir.to_path_buf();
    for part in Path::new(name).components() {
        match part {
            Component::Normal(p) => out.push(p),
            _ => return Err(format!("bad name in pack: {name:?}")),
        }
    }
    Ok(out)
}

/// Reads entries one after another.
pub struct Reader<R: Read> {
    input: R,
}

pub struct Entry {
    pub name: String,
    pub len: u64,
    pub executable: bool,
}

impl<R: Read> Reader<R> {
    pub fn new(mut input: R) -> io::Result<Self> {
        let mut magic = [0u8; MAGIC.len()];
        input.read_exact(&mut magic)?;
        if magic != MAGIC {
            return Err(io::Error::other("not a Deck transfer pack"));
        }
        Ok(Reader { input })
    }

    /// The next entry's head; its bytes must be read with [`Reader::body`]
    /// (or skipped) before the next call.
    pub fn next_entry(&mut self) -> io::Result<Option<Entry>> {
        let mut len4 = [0u8; 4];
        match self.input.read_exact(&mut len4) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => return Err(e),
        }
        let n = u32::from_le_bytes(len4) as usize;
        if n > 64 * 1024 {
            return Err(io::Error::other("pack entry name too long"));
        }
        let mut name = vec![0u8; n];
        self.input.read_exact(&mut name)?;
        let name = String::from_utf8(name).map_err(io::Error::other)?;
        check_name(&name).map_err(io::Error::other)?;
        let mut len8 = [0u8; 8];
        self.input.read_exact(&mut len8)?;
        let (name, executable) = match name.strip_suffix(EXEC) {
            Some(n) => (n.to_string(), true),
            None => (name, false),
        };
        Ok(Some(Entry {
            name,
            len: u64::from_le_bytes(len8),
            executable,
        }))
    }

    /// The current entry's bytes.
    pub fn body(&mut self, entry: &Entry) -> io::Take<&mut R> {
        (&mut self.input).take(entry.len)
    }

    pub fn read_body(&mut self, entry: &Entry) -> io::Result<Vec<u8>> {
        let mut out = Vec::with_capacity(entry.len.min(1 << 24) as usize);
        self.body(entry).read_to_end(&mut out)?;
        if out.len() as u64 != entry.len {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "pack is cut short",
            ));
        }
        Ok(out)
    }

    /// Writes the current entry to `path` (folders made as needed).
    pub fn write_body_to(&mut self, entry: &Entry, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut file = std::fs::File::create(path)?;
        let copied = io::copy(&mut self.body(entry), &mut file)?;
        if copied != entry.len {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "pack is cut short",
            ));
        }
        #[cfg(unix)]
        if entry.executable {
            use std::os::unix::fs::PermissionsExt;
            let mut perm = file.metadata()?.permissions();
            perm.set_mode(perm.mode() | 0o755);
            std::fs::set_permissions(path, perm)?;
        }
        Ok(())
    }

    pub fn skip(&mut self, entry: &Entry) -> io::Result<()> {
        io::copy(&mut self.body(entry), &mut io::sink())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_round_trip() {
        let mut w = Writer::new(Vec::new()).unwrap();
        w.add_bytes("manifest.json", b"{}").unwrap();
        w.add_bytes("a/b/c.txt", b"hello").unwrap();
        w.add_bytes("empty", b"").unwrap();
        let len = w.written();
        let bytes = w.finish().unwrap();
        assert_eq!(bytes.len() as u64, len);
        let mut r = Reader::new(&bytes[..]).unwrap();
        let mut got = Vec::new();
        while let Some(e) = r.next_entry().unwrap() {
            let body = r.read_body(&e).unwrap();
            got.push((e.name, String::from_utf8(body).unwrap()));
        }
        assert_eq!(
            got,
            vec![
                ("manifest.json".to_string(), "{}".to_string()),
                ("a/b/c.txt".into(), "hello".into()),
                ("empty".into(), "".into()),
            ]
        );
    }

    #[test]
    fn names_cannot_leave_their_folder() {
        for bad in [
            "",
            "/etc/passwd",
            "../x",
            "a/../../x",
            "a//b",
            "C:/x",
            "a\\b",
            "./a",
        ] {
            assert!(check_name(bad).is_err(), "{bad}");
        }
        assert!(Writer::new(Vec::new())
            .unwrap()
            .add_bytes("../x", b"")
            .is_err());
        assert!(safe_join(Path::new("/t"), "a/b").unwrap().ends_with("a/b"));
        // A pack with a hostile name is refused on read.
        let mut bytes = MAGIC.to_vec();
        bytes.extend_from_slice(&4u32.to_le_bytes());
        bytes.extend_from_slice(b"../x");
        bytes.extend_from_slice(&0u64.to_le_bytes());
        assert!(Reader::new(&bytes[..]).unwrap().next_entry().is_err());
        assert!(Reader::new(&b"NOTAPACK!!"[..]).is_err());
    }

    #[test]
    fn a_folder_is_packed_file_by_file() {
        let dir = std::env::temp_dir().join(format!(
            "deck-pack-{}-{}",
            std::process::id(),
            crate::now_ms()
        ));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("one.md"), "1").unwrap();
        std::fs::write(dir.join("sub").join("two.md"), "22").unwrap();
        let mut w = Writer::new(Vec::new()).unwrap();
        w.add_dir("skills/x", &dir).unwrap();
        let bytes = w.finish().unwrap();
        let out = dir.with_extension("out");
        let mut r = Reader::new(&bytes[..]).unwrap();
        while let Some(e) = r.next_entry().unwrap() {
            let p = safe_join(&out, &e.name).unwrap();
            r.write_body_to(&e, &p).unwrap();
        }
        assert_eq!(
            std::fs::read_to_string(out.join("skills/x/one.md")).unwrap(),
            "1"
        );
        assert_eq!(
            std::fs::read_to_string(out.join("skills/x/sub/two.md")).unwrap(),
            "22"
        );
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&out);
    }
}
