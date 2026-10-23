//! The converter in the browser (UNIVERSAL.md §6): web/convert_worker.js runs it in a Web Worker, one thread,
//! and gives it functions (`t3c_*` on the worker's global scope): the picked files by id (read with FileReaderSync),
//! the output and the log. The output is the pack and its index (OPFS sync access handles: store 0 and 1), or in
//! Chrome and Edge a folder the user picked (`dir`, built with the `jspi` feature: build.sh makes both builds, the
//! worker loads this one where the browser has JSPI; the other needs no exnref, which Safari before 18.4 lacks).
// (wasm-bindgen 0.2.129 marks JSPI experimental, by deprecation warnings its macros raise)
#![allow(deprecated)]

#[cfg(feature = "jspi")]
mod dir;

use anyhow::{Context, Result, anyhow, bail};
use conv_io::{Ctx, DiscFile, Entry, PackVfs, ReadSeek, Store, Vfs};
use convert::{Disc, Inputs};
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    fn t3c_input_name(id: u32) -> String;
    fn t3c_input_size(id: u32) -> f64;
    /// fills `buf` from `at` of the file
    #[wasm_bindgen(catch)]
    fn t3c_input_read(id: u32, at: f64, buf: &mut [u8]) -> Result<(), JsValue>;
    #[wasm_bindgen(catch)]
    fn t3c_store_read(store: u32, at: f64, buf: &mut [u8]) -> Result<(), JsValue>;
    #[wasm_bindgen(catch)]
    fn t3c_store_write(store: u32, at: f64, data: &[u8]) -> Result<(), JsValue>;
    #[wasm_bindgen(catch)]
    fn t3c_store_size(store: u32) -> Result<f64, JsValue>;
    #[wasm_bindgen(catch)]
    fn t3c_store_truncate(store: u32, len: f64) -> Result<(), JsValue>;
    #[wasm_bindgen(catch)]
    fn t3c_store_flush(store: u32) -> Result<(), JsValue>;
    fn t3c_log(line: &str);
    fn t3c_panic(message: &str);
}

/// A JavaScript exception as an error (the worker throws "Name: message" strings).
fn js(e: JsValue) -> anyhow::Error {
    anyhow!(e.as_string().unwrap_or_else(|| format!("{e:?}")))
}

/// A picked file.
struct Input(u32);

impl DiscFile for Input {
    fn name(&self) -> String {
        t3c_input_name(self.0)
    }
    fn open(&self) -> Result<Box<dyn ReadSeek>> {
        let reader = Reader { id: self.0, pos: 0, size: t3c_input_size(self.0) as u64 };
        Ok(Box::new(std::io::BufReader::with_capacity(1 << 16, reader)))
    }
}

struct Reader {
    id: u32,
    pos: u64,
    size: u64,
}

impl Read for Reader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = buf.len().min(self.size.saturating_sub(self.pos) as usize);
        let Some(part) = buf.get_mut(..n).filter(|_| n > 0) else { return Ok(0) };
        t3c_input_read(self.id, self.pos as f64, part).map_err(|e| std::io::Error::other(js(e)))?;
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for Reader {
    fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
        let pos = match to {
            SeekFrom::Start(p) => Some(p),
            SeekFrom::End(d) => self.size.checked_add_signed(d),
            SeekFrom::Current(d) => self.pos.checked_add_signed(d),
        };
        self.pos = pos.ok_or_else(|| std::io::Error::other("a seek before the start of the file"))?;
        Ok(self.pos)
    }
}

/// The picked Counter-Strike folder: its files by path (below the picked folder), read only.
struct Folder(BTreeMap<String, u32>);

impl Vfs for Folder {
    fn read(&self, path: &str) -> Result<Vec<u8>> {
        let &id = self.0.get(path).with_context(|| format!("no {path} in the Counter-Strike folder"))?;
        let mut buf = vec![0; t3c_input_size(id) as usize];
        t3c_input_read(id, 0.0, &mut buf).map_err(js).with_context(|| format!("cannot read {path}"))?;
        Ok(buf)
    }
    fn write(&self, path: &str, _: &[u8]) -> Result<()> {
        bail!("cannot write {path}: the Counter-Strike folder is read only")
    }
    fn exists(&self, path: &str) -> bool {
        conv_io::path_exists(&self.0, path)
    }
    fn list(&self, dir: &str) -> Result<Vec<Entry>> {
        Ok(conv_io::list_paths(&self.0, dir))
    }
    fn remove_all(&self, path: &str) -> Result<()> {
        bail!("cannot remove {path}: the Counter-Strike folder is read only")
    }
}

/// An OPFS file of the pack.
struct Opfs(u32);

impl Store for Opfs {
    fn read_at(&self, off: u64, buf: &mut [u8]) -> Result<()> {
        t3c_store_read(self.0, off as f64, buf).map_err(js)
    }
    fn write_at(&self, off: u64, data: &[u8]) -> Result<()> {
        t3c_store_write(self.0, off as f64, data).map_err(js)
    }
    fn size(&self) -> Result<u64> {
        Ok(t3c_store_size(self.0).map_err(js)? as u64)
    }
    fn truncate(&self, len: u64) -> Result<()> {
        t3c_store_truncate(self.0, len as f64).map_err(js)
    }
    fn flush(&self) -> Result<()> {
        t3c_store_flush(self.0).map_err(js)
    }
}

/// The output: the folder, else the pack.
fn out(dir: bool) -> Result<Box<dyn Vfs>> {
    if dir {
        #[cfg(feature = "jspi")]
        return Ok(Box::new(dir::Dir));
        #[cfg(not(feature = "jspi"))]
        bail!("this build of the converter writes no folder (built without JSPI)");
    }
    Ok(Box::new(PackVfs::open(Box::new(Opfs(0)), Box::new(Opfs(1)))?))
}

#[wasm_bindgen(start)]
fn start() {
    std::panic::set_hook(Box::new(|info| t3c_panic(&info.to_string())));
}

/// Converts: `options` is JSON, `{"t3": [track 1, track 2?], "t2": [track 1, 2?, 3?] | null,
/// "cs": [[path, id], ...] | null, "now": "YYYY-MM-DD HH:MM:SS", "dir": true (into the folder; else the pack)}`
/// (file ids). The fingerprint.
#[cfg_attr(feature = "jspi", wasm_bindgen(jspi))]
#[cfg_attr(not(feature = "jspi"), wasm_bindgen)]
pub fn convert(options: &str) -> Result<String, String> {
    run(options).map_err(|e| format!("{e:#}"))
}

/// The output's fingerprint, computed again from its files (a check of a finished conversion).
#[cfg_attr(feature = "jspi", wasm_bindgen(jspi))]
#[cfg_attr(not(feature = "jspi"), wasm_bindgen)]
pub fn fingerprint(dir: bool) -> Result<String, String> {
    out(dir).and_then(|o| t3_converter::convert_all::fingerprint(o.as_ref())).map_err(|e| format!("{e:#}"))
}

fn disc(ids: &Value) -> Result<Disc> {
    let id = |i: usize| ids.get(i).and_then(Value::as_u64).map(|id| Box::new(Input(id as u32)) as Box<dyn DiscFile>);
    Ok(Disc { track1: id(0).context("no track 1")?, cdda: [id(1), id(2)] })
}

fn run(options: &str) -> Result<String> {
    let o: Value = serde_json::from_str(options)?;
    let file = |f: &Value| Some((f.get(0)?.as_str()?.to_string(), f.get(1)?.as_u64()? as u32));
    let cs = o["cs"].as_array().map(|files| Folder(files.iter().filter_map(file).collect()));
    let inputs = Inputs {
        t3: disc(&o["t3"])?,
        t2: if o["t2"].is_array() { Some(disc(&o["t2"])?) } else { None },
        cs: cs.as_ref().map(|c| c as &dyn Vfs),
    };
    let out = out(o["dir"] == true)?;
    let log = |line: &str| t3c_log(line);
    let ctx = Ctx { out: out.as_ref(), log: &log, jobs: 1 };
    let manifest = convert::convert(&ctx, &inputs, o["now"].as_str().unwrap_or_default())?;
    Ok(manifest["fingerprint"].as_str().unwrap_or_default().to_string())
}
