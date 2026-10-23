//! The output folder the user picked (Chrome and Edge): File System Access is async only, so each call suspends the
//! converter's stack until its promise settles (JSPI: these imports are `suspending`, the exports `jspi`).

use crate::js;
use anyhow::{Context, Result};
use conv_io::{Entry, Vfs};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(suspending, catch)]
    fn t3c_dir_read(path: &str) -> Result<Vec<u8>, JsValue>;
    #[wasm_bindgen(suspending, catch)]
    fn t3c_dir_write(path: &str, data: &[u8]) -> Result<(), JsValue>;
    #[wasm_bindgen(suspending)]
    fn t3c_dir_exists(path: &str) -> bool;
    /// the entries' names, a folder's with a '/' after it, one per line
    #[wasm_bindgen(suspending, catch)]
    fn t3c_dir_list(dir: &str) -> Result<String, JsValue>;
    #[wasm_bindgen(suspending, catch)]
    fn t3c_dir_remove(path: &str) -> Result<(), JsValue>;
}

/// As `StdFs` (no resuming: a run starts clean, as the command line's).
pub struct Dir;

impl Vfs for Dir {
    fn read(&self, path: &str) -> Result<Vec<u8>> {
        t3c_dir_read(path).map_err(js).with_context(|| format!("cannot read {path}"))
    }
    fn write(&self, path: &str, data: &[u8]) -> Result<()> {
        t3c_dir_write(path, data).map_err(js).with_context(|| format!("cannot write {path}"))
    }
    fn exists(&self, path: &str) -> bool {
        t3c_dir_exists(path)
    }
    fn list(&self, dir: &str) -> Result<Vec<Entry>> {
        let names = t3c_dir_list(dir).map_err(js).with_context(|| format!("cannot list {dir}"))?;
        let entry = |n: &str| match n.strip_suffix('/') {
            Some(name) => Entry { name: name.to_string(), is_dir: true },
            None => Entry { name: n.to_string(), is_dir: false },
        };
        let mut out: Vec<Entry> = names.lines().map(entry).collect();
        out.sort();
        Ok(out)
    }
    fn remove_all(&self, path: &str) -> Result<()> {
        t3c_dir_remove(path).map_err(js).with_context(|| format!("cannot remove {path}"))
    }
}
