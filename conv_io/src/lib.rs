//! The converters' input and output, with no game in it: disc images to read (`DiscFile`), a tree of files to read
//! and write (`Vfs`: the output folder, or a game folder read only), and the progress log. The command-line tools use
//! the std implementations below; the browser build will give its own (File objects, OPFS).
//!
//! Paths in a `Vfs` are relative, `/`-separated (`gamedata/roster.json`).

use anyhow::{Context, Result};
use std::io::{Read, Seek};
use std::path::PathBuf;

mod pack;
pub use pack::{PackVfs, StdStore, Store, list_paths, path_exists};

pub trait ReadSeek: Read + Seek + Send {}
impl<T: Read + Seek + Send> ReadSeek for T {}

/// A disc image (or one of its tracks), opened afresh by each step that reads it.
pub trait DiscFile: Send + Sync {
    /// The file's name, for messages and the manifest (`Tekken_3_(USA)_(Track 1).bin`).
    fn name(&self) -> String;
    fn open(&self) -> Result<Box<dyn ReadSeek>>;
}

/// One entry of a folder listing.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Entry {
    pub name: String,
    pub is_dir: bool,
}

/// A tree of files.
pub trait Vfs: Send + Sync {
    fn read(&self, path: &str) -> Result<Vec<u8>>;
    /// Writes the whole file, creating its folders.
    fn write(&self, path: &str, data: &[u8]) -> Result<()>;
    fn exists(&self, path: &str) -> bool;
    /// A folder's entries, sorted by name; empty when it does not exist.
    fn list(&self, dir: &str) -> Result<Vec<Entry>>;
    /// Removes a file or a folder with everything in it; nothing when it does not exist.
    fn remove_all(&self, path: &str) -> Result<()>;
    /// The steps an earlier, interrupted conversion finished (a pack keeps them); `None`: no resuming (a folder).
    fn done(&self) -> Option<Vec<String>> {
        None
    }
    /// Records the finished steps and makes everything written so far last (a pack's index); a folder: nothing.
    fn checkpoint(&self, _done: &[String]) -> Result<()> {
        Ok(())
    }

    fn read_string(&self, path: &str) -> Result<String> {
        String::from_utf8(self.read(path)?).with_context(|| format!("{path} is not UTF-8 text"))
    }
}

/// `a/b` joined (an empty part left out).
pub fn join(a: &str, b: &str) -> String {
    match (a.trim_end_matches('/'), b.trim_start_matches('/')) {
        ("", b) => b.to_string(),
        (a, "") => a.to_string(),
        (a, b) => format!("{a}/{b}"),
    }
}

/// A disc image on disk.
pub struct StdDisc(pub PathBuf);

impl DiscFile for StdDisc {
    fn name(&self) -> String {
        self.0.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
    }
    fn open(&self) -> Result<Box<dyn ReadSeek>> {
        let f = std::fs::File::open(&self.0).with_context(|| format!("cannot open {}", self.0.display()))?;
        Ok(Box::new(std::io::BufReader::with_capacity(1 << 16, f)))
    }
}

/// A folder on disk as a `Vfs`.
pub struct StdFs(pub PathBuf);

impl StdFs {
    pub fn path(&self, p: &str) -> PathBuf {
        p.split('/').filter(|s| !s.is_empty()).fold(self.0.clone(), |acc, s| acc.join(s))
    }
}

impl Vfs for StdFs {
    fn read(&self, path: &str) -> Result<Vec<u8>> {
        let p = self.path(path);
        std::fs::read(&p).with_context(|| format!("cannot read {}", p.display()))
    }
    fn write(&self, path: &str, data: &[u8]) -> Result<()> {
        let p = self.path(path);
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
        }
        std::fs::write(&p, data).with_context(|| format!("cannot write {}", p.display()))
    }
    fn exists(&self, path: &str) -> bool {
        self.path(path).exists()
    }
    fn list(&self, dir: &str) -> Result<Vec<Entry>> {
        let p = self.path(dir);
        if !p.is_dir() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for e in std::fs::read_dir(&p).with_context(|| format!("cannot list {}", p.display()))? {
            let e = e?;
            // (a symlink to a folder counts as one: the viewers' links)
            let is_dir = e.path().is_dir();
            out.push(Entry { name: e.file_name().to_string_lossy().into_owned(), is_dir });
        }
        out.sort();
        Ok(out)
    }
    fn remove_all(&self, path: &str) -> Result<()> {
        let p = self.path(path);
        let r = if p.is_dir() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
        match r {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                Err(e).with_context(|| format!("cannot remove {}", p.display()))
            }
            _ => Ok(()),
        }
    }
}

/// A sub-folder of another `Vfs` as a `Vfs` of its own.
pub struct Sub<'a> {
    pub inner: &'a dyn Vfs,
    pub prefix: String,
}

impl Vfs for Sub<'_> {
    fn read(&self, path: &str) -> Result<Vec<u8>> {
        self.inner.read(&join(&self.prefix, path))
    }
    fn write(&self, path: &str, data: &[u8]) -> Result<()> {
        self.inner.write(&join(&self.prefix, path), data)
    }
    fn exists(&self, path: &str) -> bool {
        self.inner.exists(&join(&self.prefix, path))
    }
    fn list(&self, dir: &str) -> Result<Vec<Entry>> {
        self.inner.list(&join(&self.prefix, dir))
    }
    fn remove_all(&self, path: &str) -> Result<()> {
        self.inner.remove_all(&join(&self.prefix, path))
    }
}

/// What a conversion step works with: where it writes, how it reports, how many threads it may use.
pub struct Ctx<'a> {
    pub out: &'a dyn Vfs,
    pub log: &'a (dyn Fn(&str) + Sync),
    /// Threads for the steps that use them (1: inline, the browser's case); the output never depends on it.
    pub jobs: usize,
}

impl Ctx<'_> {
    pub fn log(&self, line: &str) {
        (self.log)(line)
    }
}

/// The log of the command-line tools: stdout, one line each (a closed stdout is no error).
pub fn print_line(line: &str) {
    use std::io::Write;
    let _ = writeln!(std::io::stdout(), "{line}");
}

/// The thread count of the command-line tools: `--jobs N` if given, else the machine's.
pub fn jobs_from(args: &[String]) -> usize {
    args.iter()
        .position(|a| a == "--jobs")
        .and_then(|i| args.get(i + 1))
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |n| n.get()))
        .max(1)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn std_fs_round_trip() {
        let root = std::env::temp_dir().join(format!("conv_io_test_{}", std::process::id()));
        let fs = StdFs(root.clone());
        fs.write("a/b/c.json", b"{}").unwrap();
        assert_eq!(fs.read("a/b/c.json").unwrap(), b"{}");
        assert!(fs.exists("a/b"));
        assert_eq!(fs.list("a").unwrap(), vec![Entry { name: "b".into(), is_dir: true }]);
        assert!(fs.list("nowhere").unwrap().is_empty());
        fs.remove_all("a").unwrap();
        fs.remove_all("a").unwrap();
        assert!(!fs.exists("a/b/c.json"));
        assert_eq!(join("x/", "/y"), "x/y");
        assert_eq!(join("", "y"), "y");
        std::fs::remove_dir_all(root).unwrap();
    }
}
