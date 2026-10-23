//! The browser's output (UNIVERSAL.md §6): one pack file, because OPFS creates files only asynchronously and the
//! converters write thousands. `assets.pack` holds the files' bytes, appended; `assets.idx` is JSON,
//! `{"version": 1, "files": {"path": [offset, length]}, "done": [finished steps]}`, rewritten at each checkpoint. A
//! file written again is appended again (the index points at the latest copy); removing drops paths from the index.

use crate::{Entry, Vfs};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, MutexGuard};

/// A file read and written at offsets (natively a std file; in the browser an OPFS sync access handle).
pub trait Store: Send + Sync {
    /// Fills `buf` from `off` (an error if the store ends before).
    fn read_at(&self, off: u64, buf: &mut [u8]) -> Result<()>;
    fn write_at(&self, off: u64, data: &[u8]) -> Result<()>;
    fn size(&self) -> Result<u64>;
    fn truncate(&self, len: u64) -> Result<()>;
    fn flush(&self) -> Result<()>;
}

/// A file on disk as a `Store`.
pub struct StdStore(Mutex<std::fs::File>);

impl StdStore {
    /// Opens or creates the file.
    pub fn open(path: &std::path::Path) -> Result<Self> {
        let f = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path);
        Ok(Self(Mutex::new(f.with_context(|| format!("cannot open {}", path.display()))?)))
    }
    fn file(&self) -> Result<MutexGuard<'_, std::fs::File>> {
        self.0.lock().map_err(|_| anyhow::anyhow!("a pack file's lock is poisoned"))
    }
}

impl Store for StdStore {
    fn read_at(&self, off: u64, buf: &mut [u8]) -> Result<()> {
        use std::io::{Read, Seek, SeekFrom};
        let mut f = self.file()?;
        f.seek(SeekFrom::Start(off))?;
        Ok(f.read_exact(buf)?)
    }
    fn write_at(&self, off: u64, data: &[u8]) -> Result<()> {
        use std::io::{Seek, SeekFrom, Write};
        let mut f = self.file()?;
        f.seek(SeekFrom::Start(off))?;
        Ok(f.write_all(data)?)
    }
    fn size(&self) -> Result<u64> {
        Ok(self.file()?.metadata()?.len())
    }
    fn truncate(&self, len: u64) -> Result<()> {
        Ok(self.file()?.set_len(len)?)
    }
    fn flush(&self) -> Result<()> {
        Ok(self.file()?.sync_data()?)
    }
}

/// `a//b/` as `a/b`: one name per path in the index.
fn clean(path: &str) -> String {
    path.split('/').filter(|s| !s.is_empty()).collect::<Vec<_>>().join("/")
}

/// The entries of folder `dir` among the paths of `files` (sorted by name, as `Vfs::list`).
pub fn list_paths<V>(files: &BTreeMap<String, V>, dir: &str) -> Vec<Entry> {
    let dir = clean(dir);
    let prefix = if dir.is_empty() { dir } else { format!("{dir}/") };
    let below = files.range(prefix.clone()..).take_while(|(k, _)| k.starts_with(&prefix));
    let entries: BTreeSet<Entry> = below
        .map(|(k, _)| match k[prefix.len()..].split_once('/') {
            Some((name, _)) => Entry { name: name.to_string(), is_dir: true },
            None => Entry { name: k[prefix.len()..].to_string(), is_dir: false },
        })
        .collect();
    entries.into_iter().collect()
}

/// A file of `files`, or a folder holding one.
pub fn path_exists<V>(files: &BTreeMap<String, V>, path: &str) -> bool {
    let path = clean(path);
    let prefix = format!("{path}/");
    path.is_empty()
        || files.contains_key(&path)
        || files.range(prefix.clone()..).next().is_some_and(|(k, _)| k.starts_with(&prefix))
}

struct Index {
    files: BTreeMap<String, (u64, u64)>,
    done: Vec<String>,
    /// where the next file goes
    end: u64,
}

/// A `Vfs` in a pack and its index.
pub struct PackVfs {
    pack: Box<dyn Store>,
    index: Box<dyn Store>,
    state: Mutex<Index>,
}

impl PackVfs {
    /// Opens a pack (empty stores: a new one). The bytes after the indexed files (an interrupted step's) are
    /// dropped; an index that does not read (cut off by an interrupted rewrite) or points past the pack starts the
    /// pack again.
    pub fn open(pack: Box<dyn Store>, index: Box<dyn Store>) -> Result<Self> {
        let mut bytes = vec![0; usize::try_from(index.size()?)?];
        index.read_at(0, &mut bytes)?;
        let (mut files, mut done) = parse_index(&bytes).unwrap_or_default();
        let mut end = files.values().map(|&(o, l)| o.saturating_add(l)).max().unwrap_or(0);
        if end > pack.size()? {
            (files, done, end) = Default::default();
        }
        pack.truncate(end)?;
        Ok(Self { pack, index, state: Mutex::new(Index { files, done, end }) })
    }

    fn state(&self) -> Result<MutexGuard<'_, Index>> {
        self.state.lock().map_err(|_| anyhow::anyhow!("the pack's lock is poisoned"))
    }
}

type Files = BTreeMap<String, (u64, u64)>;

fn parse_index(bytes: &[u8]) -> Option<(Files, Vec<String>)> {
    let v: Value = serde_json::from_slice(bytes).ok()?;
    if v["version"] != 1 {
        return None;
    }
    let mut files = BTreeMap::new();
    for (path, at) in v["files"].as_object()? {
        files.insert(path.clone(), (at[0].as_u64()?, at[1].as_u64()?));
    }
    let done = v["done"].as_array()?.iter().map(|s| s.as_str().map(str::to_string)).collect::<Option<_>>()?;
    Some((files, done))
}

impl Vfs for PackVfs {
    fn read(&self, path: &str) -> Result<Vec<u8>> {
        let path = clean(path);
        let Some(&(off, len)) = self.state()?.files.get(&path) else { bail!("cannot read {path}: not in the pack") };
        let mut buf = vec![0; usize::try_from(len)?];
        self.pack.read_at(off, &mut buf).with_context(|| format!("cannot read {path} from the pack"))?;
        Ok(buf)
    }
    fn write(&self, path: &str, data: &[u8]) -> Result<()> {
        let path = clean(path);
        let mut s = self.state()?;
        self.pack.write_at(s.end, data).with_context(|| format!("cannot write {path} into the pack"))?;
        let at = (s.end, data.len() as u64);
        s.end += at.1;
        s.files.insert(path, at);
        Ok(())
    }
    fn exists(&self, path: &str) -> bool {
        self.state().is_ok_and(|s| path_exists(&s.files, path))
    }
    fn list(&self, dir: &str) -> Result<Vec<Entry>> {
        Ok(list_paths(&self.state()?.files, dir))
    }
    /// ("": everything, and the pack starts again empty)
    fn remove_all(&self, path: &str) -> Result<()> {
        let path = clean(path);
        let mut s = self.state()?;
        let prefix = format!("{path}/");
        s.files.retain(|k, _| !path.is_empty() && *k != path && !k.starts_with(&prefix));
        if path.is_empty() {
            s.done.clear();
            s.end = 0;
            self.pack.truncate(0)?;
        }
        Ok(())
    }
    fn done(&self) -> Option<Vec<String>> {
        self.state().ok().map(|s| s.done.clone())
    }
    /// The pack's bytes first, then the index that points at them.
    fn checkpoint(&self, done: &[String]) -> Result<()> {
        let mut s = self.state()?;
        s.done = done.to_vec();
        let files: serde_json::Map<String, Value> =
            s.files.iter().map(|(k, &(o, l))| (k.clone(), json!([o, l]))).collect();
        let index = serde_json::to_vec(&json!({"version": 1, "files": files, "done": s.done}))?;
        self.pack.flush().context("cannot write the pack")?;
        self.index.write_at(0, &index).context("cannot write the pack's index")?;
        self.index.truncate(index.len() as u64)?;
        self.index.flush().context("cannot write the pack's index")
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[derive(Clone, Default)]
    struct Mem(Arc<Mutex<Vec<u8>>>);

    impl Store for Mem {
        fn read_at(&self, off: u64, buf: &mut [u8]) -> Result<()> {
            let v = self.0.lock().unwrap();
            buf.copy_from_slice(v.get(off as usize..off as usize + buf.len()).context("past the end")?);
            Ok(())
        }
        fn write_at(&self, off: u64, data: &[u8]) -> Result<()> {
            let mut v = self.0.lock().unwrap();
            let end = off as usize + data.len();
            if v.len() < end {
                v.resize(end, 0);
            }
            v[off as usize..end].copy_from_slice(data);
            Ok(())
        }
        fn size(&self) -> Result<u64> {
            Ok(self.0.lock().unwrap().len() as u64)
        }
        fn truncate(&self, len: u64) -> Result<()> {
            self.0.lock().unwrap().resize(len as usize, 0);
            Ok(())
        }
        fn flush(&self) -> Result<()> {
            Ok(())
        }
    }

    #[test]
    fn pack_round_trip() {
        let (pack, index) = (Mem::default(), Mem::default());
        let open = || PackVfs::open(Box::new(pack.clone()), Box::new(index.clone())).unwrap();
        let fs = open();
        assert_eq!(fs.done(), Some(vec![]));
        fs.write("a/b/c.json", b"{}").unwrap();
        fs.write("a/b.json", b"1").unwrap();
        fs.write("a//b.json", b"22").unwrap();
        fs.write("z", b"").unwrap();
        assert_eq!(fs.read("a/b.json").unwrap(), b"22");
        assert!(fs.exists("a/b") && fs.exists("a/b/c.json") && !fs.exists("a/c") && !fs.exists("a/b/c"));
        let e = |name: &str, is_dir| Entry { name: name.into(), is_dir };
        assert_eq!(fs.list("a").unwrap(), vec![e("b", true), e("b.json", false)]);
        assert_eq!(fs.list("").unwrap(), vec![e("a", true), e("z", false)]);
        assert!(fs.list("nowhere").unwrap().is_empty());
        fs.checkpoint(&["one".into()]).unwrap();
        let checkpointed = pack.size().unwrap();
        fs.write("lost", b"not checkpointed").unwrap();

        // reopened: as at the checkpoint, new files after the old bytes
        let fs = open();
        assert_eq!(fs.done(), Some(vec!["one".to_string()]));
        assert!(!fs.exists("lost") && pack.size().unwrap() == checkpointed);
        fs.write("new", b"n").unwrap();
        assert_eq!(fs.read("a/b/c.json").unwrap(), b"{}");
        assert_eq!(fs.read("new").unwrap(), b"n");
        fs.remove_all("a/b").unwrap();
        assert!(!fs.exists("a/b/c.json") && fs.exists("a/b.json"));
        fs.remove_all("").unwrap();
        assert!(fs.list("").unwrap().is_empty() && pack.size().unwrap() == 0);

        // an index cut off: a new pack
        index.truncate(5).unwrap();
        assert!(!open().exists("a"));
    }
}
