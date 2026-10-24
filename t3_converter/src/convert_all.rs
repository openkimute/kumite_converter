//! convert_all: convert everything OpenKumite needs from your own disc image into standard formats, once.
//!
//! usage: t3_convert "Tekken_3_(USA)_(Track 1).bin" OUTDIR [--only characters,stages,...] [--jobs N]
//!        t3_convert --fingerprint OUTDIR     (only the manifest's fingerprint, again)
//!
//! OUTDIR layout (OpenKumite reads only this): docs/converter.md. `convert` runs the steps in `STEPS`
//! into a `Vfs`; `main` is the command line. The manifest names each step's module.

use crate::{
    export_effects, export_gamedata, export_gltf, export_media, export_pictures, export_shadows, export_stages,
    export_theater, pyjson,
};
use anyhow::{Context, Result, bail};
use conv_io::{Ctx, DiscFile, StdDisc, StdFs, Vfs, jobs_from, join, print_line};
use serde_json::{Map, Value, json};
use sha1::Digest;
use std::io::Read;

/// A step: (context, track 1, the CD-audio track 2 if there, its output folder).
type Step = fn(&Ctx, &dyn DiscFile, Option<&dyn DiscFile>, &str) -> Result<()>;

/// name -> (script, output folder, step), in their order
pub const STEPS: [(&str, &str, &str, Step); 8] = [
    ("characters", "export_gltf", "characters", |c, d, _, o| export_gltf::run(c, d, o, None, true, true)),
    ("stages", "export_stages", "stages", |c, d, _, o| export_stages::run(c, d, o, None)),
    ("pictures", "export_pictures", "pictures", |c, d, _, o| export_pictures::run(c, d, o)),
    // export_media's sfx/, music/ and index.json go to audio/, its video/ to video/
    ("media", "export_media", "", |c, d, t2, o| {
        let place = |p: &str| join(o, &if p == "video" { p.to_string() } else { format!("audio/{p}") });
        export_media::run(c, d, t2, &["sfx", "music", "video"], &place)
    }),
    ("gamedata", "export_gamedata", "", |c, d, _, o| export_gamedata::run(c, d, o)),
    // after pictures (its sheets, palettes) and gamedata (roster)
    ("effects", "export_effects", "pictures/effects", |c, d, _, o| export_effects::run(c, d, o)),
    ("theater", "export_theater", "", |c, d, _, o| export_theater::run(c, d, o)), // gamedata/theater.json, pictures/theater/
    ("shadows", "export_shadows", "", |c, d, _, o| export_shadows::run(c, d, o)), // gamedata/shadows.json
];

/// Converts `only` these steps (by name) into `ctx.out`, then writes manifest.json (`now`: when, for the steps'
/// "converted") and returns it.
pub fn convert(
    ctx: &Ctx,
    disc: &dyn DiscFile,
    track2: Option<&dyn DiscFile>,
    only: &[&str],
    now: &str,
) -> Result<Value> {
    let mut manifest = if ctx.out.exists("manifest.json") {
        read_manifest(ctx.out)?
    } else {
        Map::from_iter([("steps".into(), json!({}))])
    };
    manifest.insert("source".into(), source(disc)?);
    for &step in only {
        run_step(ctx, disc, track2, step)?;
        record_step(&mut manifest, step, now)?;
    }
    // (every step converted again: the data is this version's layout; else an older one's stays as it was)
    if STEPS.iter().all(|s| only.contains(&s.0)) {
        manifest.insert("format".into(), json!(FORMAT));
    }
    write_manifest(ctx.out, manifest)
}

/// The manifest's "source": the image, identified by its first 16 MB (enough to tell discs apart).
pub fn source(disc: &dyn DiscFile) -> Result<Value> {
    let mut head = Vec::new();
    disc.open()?.take(16 << 20).read_to_end(&mut head).with_context(|| format!("cannot read {}", disc.name()))?;
    let sha: String = sha1::Sha1::digest(&head).iter().map(|b| format!("{b:02x}")).collect();
    Ok(json!({"file": disc.name(), "sha1_16mb": sha}))
}

fn find_step(step: &str) -> Result<&'static (&'static str, &'static str, &'static str, Step)> {
    STEPS.iter().find(|s| s.0 == step).with_context(|| format!("unknown step {step:?}"))
}

/// Runs one step (by name) into `ctx.out`.
pub fn run_step(ctx: &Ctx, disc: &dyn DiscFile, track2: Option<&dyn DiscFile>, step: &str) -> Result<()> {
    let (_, script, dir, run) = find_step(step)?;
    ctx.log(&format!("[{step}] {script} -> {}", if dir.is_empty() { "." } else { dir }));
    run(ctx, disc, track2, dir).with_context(|| format!("[{step}] {script} failed"))?;
    ctx.log(&format!("[{step}] done"));
    Ok(())
}

/// The step's entry in the manifest's "steps" (`now`: when it was converted).
pub fn record_step(manifest: &mut Map<String, Value>, step: &str, now: &str) -> Result<()> {
    let script = find_step(step)?.1;
    let steps = manifest.entry("steps").or_insert_with(|| json!({}));
    steps
        .as_object_mut()
        .context("the manifest's steps is not an object")?
        .insert(step.into(), json!({"converted": now, "script": script}));
    Ok(())
}

/// The sub-folders of `base`/`rel` (relative to `base`, '/'-separated) and itself, as `os.walk` finds them.
fn walk(out: &dyn Vfs, base: &str, rel: &str, dirs: &mut Vec<String>) -> Result<()> {
    dirs.push(rel.to_string());
    for e in out.list(&join(base, rel))? {
        if e.is_dir {
            walk(out, base, &join(rel, &e.name), dirs)?;
        }
    }
    Ok(())
}

/// A hash of everything a fight reads (online, two computers' fights stay alike only on the same data), in a
/// canonical form: two conversions of the same disc agree even where the bytes may not (the JSON's key order, the
/// PNG textures' compression, which differs between Pillow builds; the textures play no part in a fight).
pub fn fingerprint(out: &dyn Vfs) -> Result<String> {
    let mut h = sha2::Sha256::new();
    for sub in ["characters", "stages", "gamedata"] {
        let mut dirs = Vec::new();
        walk(out, sub, "", &mut dirs)?;
        dirs.sort(); // sorted(os.walk(...)): by the folder's path
        for d in dirs {
            let dir = join(sub, &d);
            for e in out.list(&dir)? {
                if e.is_dir || !(e.name.ends_with(".glb") || e.name.ends_with(".json")) {
                    continue;
                }
                let rel = join(&dir, &e.name);
                h.update(rel.as_bytes());
                h.update(b"\0");
                let data = out.read(&rel)?;
                let canon = if e.name.ends_with(".glb") {
                    canonical_glb(&data)
                } else {
                    serde_json::from_slice(&data).map_err(anyhow::Error::from).map(|v| canonical_json(&v))
                };
                h.update(canon.with_context(|| format!("{rel} is not valid"))?);
            }
        }
    }
    Ok(h.finalize().iter().take(16).map(|b| format!("{b:02x}")).collect())
}

fn canonical_json(v: &Value) -> Vec<u8> {
    pyjson::to_string(v, pyjson::CANONICAL).into_bytes()
}

/// A glb's JSON without where its data lies (buffers, views, images), then each accessor's elements.
fn canonical_glb(data: &[u8]) -> Result<Vec<u8>> {
    let u32_at = |d: &[u8], o: usize| d.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let jlen = u32_at(data, 12).context("not a glb")? as usize;
    let mut doc: Value = serde_json::from_slice(crate::disc::py_slice(data, 20, 20 + jlen))?;
    let rest = crate::disc::py_slice(data, 20 + jlen, usize::MAX);
    let blob = match u32_at(rest, 0) {
        Some(n) => crate::disc::py_slice(rest, 8, 8 + n as usize),
        None => &[],
    };
    let views = doc.get("bufferViews").and_then(Value::as_array).cloned().unwrap_or_default();
    let int = |v: &Value, k: &str, default: Option<u64>| -> Result<usize> {
        match v.get(k) {
            Some(x) => x.as_u64().map(|x| x as usize).with_context(|| format!("{k} is not a count")),
            None => default.map(|x| x as usize).with_context(|| format!("no {k}")),
        }
    };
    let mut parts = Vec::new();
    for a in doc.get("accessors").and_then(Value::as_array).into_iter().flatten() {
        if a.get("bufferView").is_none() {
            continue; // (none the converter writes: zeros, no data of its own)
        }
        let v = views.get(int(a, "bufferView", None)?).context("an accessor's bufferView is missing")?;
        let size = match a.get("componentType").and_then(Value::as_u64) {
            Some(5120 | 5121) => 1,
            Some(5122 | 5123) => 2,
            Some(5125 | 5126) => 4,
            t => bail!("unknown componentType {t:?}"),
        };
        let count = match a.get("type").and_then(Value::as_str) {
            Some("SCALAR") => 1,
            Some("VEC2") => 2,
            Some("VEC3") => 3,
            Some("VEC4" | "MAT2") => 4,
            Some("MAT3") => 9,
            Some("MAT4") => 16,
            t => bail!("unknown accessor type {t:?}"),
        };
        let element = size * count;
        let stride = int(v, "byteStride", Some(element as u64))?;
        let at = int(v, "byteOffset", Some(0))? + int(a, "byteOffset", Some(0))?;
        let n = int(a, "count", None)?;
        if stride == element {
            // packed: one slice
            parts.extend_from_slice(crate::disc::py_slice(blob, at, at + element * n));
        } else {
            for i in 0..n {
                parts.extend_from_slice(crate::disc::py_slice(blob, at + i * stride, at + i * stride + element));
            }
        }
    }
    if let Some(m) = doc.as_object_mut() {
        for key in ["buffers", "bufferViews", "images"] {
            m.shift_remove(key);
        }
    }
    let mut c = canonical_json(&doc);
    c.extend(parts);
    Ok(c)
}

/// manifest.json's `format`: the converted data's layout (2: the engine reads every fact of the games from it; 3:
/// its keys name no game; 4: the rules of particular moves, movesets and sounds are data; 5: the screens' places,
/// sizes and pictures are data too; 6: the modes' rules, the select's ball and the HUD's rules once each; 7: the
/// result screens' faces and log, the VS panel's tab; 8: the half-size face's slash, the team log's face flags). A
/// full conversion sets it; a refresh keeps the one there (an older conversion stays one).
pub const FORMAT: i64 = 8;

/// Adds the format (when there is none) and the fingerprint, writes manifest.json; the manifest.
pub fn write_manifest(out: &dyn Vfs, mut manifest: Map<String, Value>) -> Result<Value> {
    manifest.entry("format").or_insert(json!(FORMAT));
    manifest.insert("fingerprint".into(), json!(fingerprint(out)?));
    let manifest = Value::Object(manifest);
    out.write("manifest.json", pyjson::indent1(&manifest).as_bytes())?;
    Ok(manifest)
}

pub fn read_manifest(out: &dyn Vfs) -> Result<Map<String, Value>> {
    serde_json::from_slice(&out.read("manifest.json")?).context("manifest.json is not a JSON object")
}

pub fn main(args: &[String]) -> Result<()> {
    let (out, manifest) = if let [flag, out, ..] = args
        && flag == "--fingerprint"
    {
        let fs = StdFs(out.into());
        (out, write_manifest(&fs, read_manifest(&fs)?)?)
    } else {
        let [src, out, ..] = args else {
            bail!(
                "usage: t3_convert \"Tekken_3_(USA)_(Track 1).bin\" OUTDIR [--only characters,stages,...] [--jobs N]\n       \
                 t3_convert --fingerprint OUTDIR     (only the manifest's fingerprint, again)"
            )
        };
        let only: Vec<&str> = match args.iter().position(|a| a == "--only") {
            Some(i) => args.get(i + 1).context("--only needs a list of steps")?.split(',').collect(),
            None => STEPS.iter().map(|s| s.0).collect(),
        };
        let track2 = StdDisc(src.replace("Track 1", "Track 2").into());
        let track2 = track2.0.exists().then_some(&track2 as &dyn DiscFile);
        // each step's time, from the end of the one before (the library has no clock)
        let start = std::sync::Mutex::new(std::time::Instant::now());
        let log = |line: &str| match start.lock() {
            Ok(mut t) if line.starts_with('[') && line.ends_with("] done") => {
                print_line(&format!("{line} in {:.0} s", t.elapsed().as_secs_f64()));
                *t = std::time::Instant::now();
            }
            _ => print_line(line),
        };
        let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        let ctx = Ctx { out: &StdFs(out.into()), log: &log, jobs: jobs_from(args) };
        (out, convert(&ctx, &StdDisc(src.into()), track2, &only, &now)?)
    };
    let mpath = std::path::Path::new(out).join("manifest.json");
    print_line(&format!("manifest: {} {}", mpath.display(), manifest["fingerprint"].as_str().unwrap_or("")));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_json_as_python() {
        // json.dumps(json.loads(s), sort_keys=True, separators=(',', ':')); shortcut: an integer "-0" reads as
        // -0.0 here (Python: 0), the converters never write one
        let v: Value = serde_json::from_str("{\"b\": [1E5, 1e-05, -0.0, 12345678901234567890, 7], \"a\": \"\u{e9}\"}")
            .unwrap_or_default();
        assert_eq!(
            String::from_utf8_lossy(&canonical_json(&v)),
            "{\"a\":\"\\u00e9\",\"b\":[100000.0,1e-05,-0.0,12345678901234567890,7]}"
        );
    }

    /// `--fingerprint` on an older conversion keeps its format (the engine still asks for a new conversion).
    #[test]
    fn a_refresh_keeps_the_format() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("t3_manifest_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        let out = StdFs(dir.clone());
        let old = Map::from_iter([("format".into(), json!(FORMAT - 1))]);
        assert_eq!(write_manifest(&out, old)?["format"], json!(FORMAT - 1));
        assert_eq!(write_manifest(&out, Map::new())?["format"], json!(FORMAT));
        std::fs::remove_dir_all(&dir)?;
        Ok(())
    }

    /// The fingerprint of a converted folder against its manifest's, without writing anything:
    /// `T3_FINGERPRINT_DIR=... cargo test --release -- --ignored fingerprint_of`.
    #[test]
    #[ignore]
    fn fingerprint_of() -> Result<()> {
        let out = StdFs(std::env::var("T3_FINGERPRINT_DIR").context("set T3_FINGERPRINT_DIR")?.into());
        let m = read_manifest(&out)?;
        assert_eq!(Some(fingerprint(&out)?.as_str()), m.get("fingerprint").and_then(Value::as_str));
        Ok(())
    }
}
