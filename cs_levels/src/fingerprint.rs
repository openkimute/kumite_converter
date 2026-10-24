//! convert_all's `fingerprint`, `canonical_json`, `canonical_glb` and `write_manifest` (`convert_all
//! --fingerprint ASSETS`), which the scripts run after changing the assets; a copy: no import of Tekken 3's converter.

use crate::pyjson;
use anyhow::{Context, Result};
use conv_io::Vfs;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::Path;

/// A hash of everything a fight reads (online, two computers' fights stay alike only on the same data), in a
/// canonical form: two conversions of the same disc agree even where the bytes may not (the JSON's key order, the
/// PNG textures' compression, which differs between Pillow builds; the textures play no part in a fight).
pub fn fingerprint(out: &dyn Vfs) -> Result<String> {
    let mut h = Sha256::new();
    for sub in ["characters", "stages", "gamedata"] {
        // os.walk, its folders in sorted order (a symlinked folder is walked into: a converted tree has none)
        let mut dirs = vec![sub.to_string()];
        let mut i = 0;
        while let Some(d) = dirs.get(i).cloned() {
            i += 1;
            dirs.extend(out.list(&d)?.into_iter().filter(|e| e.is_dir).map(|e| format!("{d}/{}", e.name)));
        }
        dirs.sort();
        for d in dirs {
            // (list() is sorted by name)
            let files = out.list(&d)?.into_iter().filter(|e| !e.is_dir).map(|e| e.name);
            for name in files.filter(|n| n.ends_with(".glb") || n.ends_with(".json")) {
                let rel = format!("{d}/{name}");
                h.update(rel.as_bytes());
                h.update(b"\0");
                let data = out.read(&rel)?;
                h.update(if name.ends_with(".glb") {
                    canonical_glb(&data).with_context(|| format!("{rel}: not a glb"))?
                } else {
                    canonical_json(&serde_json::from_slice(&data).with_context(|| format!("{rel}: not JSON"))?)
                });
            }
        }
    }
    Ok(h.finalize().iter().take(16).map(|b| format!("{b:02x}")).collect())
}

pub fn canonical_json(obj: &Value) -> Vec<u8> {
    pyjson::to_string(obj, pyjson::CANONICAL).into_bytes()
}

fn u32at(d: &[u8], o: usize) -> Option<usize> {
    d.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
}

/// Python's `b[a:b]`, clamped.
fn slice(d: &[u8], a: usize, b: usize) -> &[u8] {
    d.get(a.min(d.len())..b.clamp(a.min(d.len()), d.len())).unwrap_or(&[])
}

/// A glb's JSON without where its data lies (buffers, views, images), then each accessor's elements.
pub fn canonical_glb(data: &[u8]) -> Result<Vec<u8>> {
    let jlen = u32at(data, 12).context("too short")?;
    let mut doc: Value = serde_json::from_slice(slice(data, 20, 20 + jlen))?;
    let rest = slice(data, 20 + jlen, usize::MAX);
    let blob = match u32at(rest, 0) {
        Some(n) if rest.len() >= 8 => slice(rest, 8, 8 + n),
        _ => &[],
    };
    let views = doc.get("bufferViews").and_then(Value::as_array).cloned().unwrap_or_default();
    let mut parts = Vec::new();
    for a in doc.get("accessors").and_then(Value::as_array).into_iter().flatten() {
        let Some(bv) = a.get("bufferView") else {
            continue;
        }; // (none the converter writes: zeros, no data of its own)
        let v = bv.as_u64().and_then(|k| views.get(k as usize)).context("an accessor's bufferView is missing")?;
        let size = match a.get("componentType").and_then(Value::as_u64) {
            Some(5120 | 5121) => 1,
            Some(5122 | 5123) => 2,
            Some(5125 | 5126) => 4,
            _ => anyhow::bail!("an accessor's componentType is unknown"),
        };
        let count = match a.get("type").and_then(Value::as_str) {
            Some("SCALAR") => 1,
            Some("VEC2") => 2,
            Some("VEC3") => 3,
            Some("VEC4" | "MAT2") => 4,
            Some("MAT3") => 9,
            Some("MAT4") => 16,
            _ => anyhow::bail!("an accessor's type is unknown"),
        };
        let num = |o: &Value, k: &str, d: usize| o.get(k).map_or(Some(d), |x| x.as_u64().map(|x| x as usize));
        let element = size * count;
        let n = num(a, "count", 0).context("an accessor's count")?;
        let stride = num(v, "byteStride", element).context("a view's byteStride")?;
        let at =
            num(v, "byteOffset", 0).context("a view's byteOffset")? + num(a, "byteOffset", 0).context("byteOffset")?;
        if stride == element {
            // packed: one slice
            parts.extend_from_slice(slice(blob, at, at + element * n));
        } else {
            for i in 0..n {
                parts.extend_from_slice(slice(blob, at + i * stride, at + i * stride + element));
            }
        }
    }
    if let Some(m) = doc.as_object_mut() {
        for key in ["buffers", "bufferViews", "images"] {
            m.shift_remove(key);
        }
    }
    let mut out = canonical_json(&doc);
    out.extend(parts);
    Ok(out)
}

/// manifest.json's `format` (t3_converter convert_all's).
const FORMAT: i64 = 3;

/// `convert_all --fingerprint OUT`: the manifest's fingerprint, again (returned).
pub fn write_manifest(out: &dyn Vfs) -> Result<String> {
    let mut manifest: Value = serde_json::from_slice(&out.read("manifest.json")?).context("manifest.json: not JSON")?;
    let fp = fingerprint(out)?;
    let m = manifest.as_object_mut().context("manifest.json is not an object")?;
    // (a refresh: an older conversion's format stays, only a full conversion sets it)
    m.entry("format").or_insert(FORMAT.into());
    m.insert("fingerprint".into(), fp.clone().into());
    out.write("manifest.json", pyjson::indent1(&manifest).as_bytes())?;
    Ok(fp)
}

/// The CLIs' last step: `write_manifest`, and its line.
pub fn print_manifest(out: &dyn Vfs, dir: &str) -> Result<()> {
    let fp = write_manifest(out)?;
    conv_io::print_line(&format!("manifest: {} {fp}", Path::new(dir).join("manifest.json").display()));
    Ok(())
}
