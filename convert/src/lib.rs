//! The universal converter: Tekken 3 (required), then the Counter-Strike levels and the Tekken 2 fighters (optional),
//! into one assets folder, the online fingerprint once at the end. It links the three converters as crates and runs
//! no outside program (UNIVERSAL.md).

use anyhow::{Context, Result};
use conv_io::{Ctx, DiscFile, Sub, Vfs};
use serde_json::{Value, json};
use sha1::Digest;
use std::io::Read;

/// A disc image with its CD-audio tracks (track 2, track 3: none where the disc has none).
pub struct Disc {
    pub track1: Box<dyn DiscFile>,
    pub cdda: [Option<Box<dyn DiscFile>>; 2],
}

pub struct Inputs<'a> {
    pub t3: Disc,
    pub t2: Option<Disc>,
    /// The Counter-Strike game folder (a Half-Life folder with cstrike/), read only.
    pub cs: Option<&'a dyn Vfs>,
}

/// Where the Tekken 2 movies the transplant needs are made (removed at the end).
const T2_SCRATCH: &str = ".tmp/assets_t2";

/// Converts everything into `ctx.out`; the manifest (written last). The steps: Tekken 3's, then "cs", then "t2". Into
/// a pack whose finished steps begin this run's (an interrupted conversion), those are not done again.
pub fn convert(ctx: &Ctx, inputs: &Inputs, now: &str) -> Result<Value> {
    use t3_converter::convert_all as t3c;
    let mut plan: Vec<&str> = t3c::STEPS.iter().map(|s| s.0).collect();
    plan.extend(inputs.cs.map(|_| "cs").into_iter().chain(inputs.t2.as_ref().map(|_| "t2")));
    let earlier = ctx.out.done();
    let resumes = |d: &Vec<String>| d.len() <= plan.len() && d.iter().zip(&plan).all(|(a, b)| a == b);
    let mut done = earlier.clone().filter(resumes).unwrap_or_default();
    if done.is_empty() {
        // a clean start: nothing of an earlier conversion (another set of extras) stays to be fingerprinted; a pack
        // starts empty
        let dirs: &[&str] = if earlier.is_some() { &[""] } else { &["characters", "stages", "gamedata"] };
        for dir in dirs {
            ctx.out.remove_all(dir)?;
        }
        // (before a pack's bytes are written over: its old index must not point at them)
        ctx.out.checkpoint(&done)?;
    }
    for path in ["manifest.json", ".tmp"] {
        ctx.out.remove_all(path)?;
    }
    let t3 = &inputs.t3;
    let mut manifest = serde_json::Map::from_iter([("steps".into(), json!({}))]);
    manifest.insert("source".into(), t3c::source(t3.track1.as_ref())?);
    for &step in &plan[..t3c::STEPS.len()] {
        if !done.iter().any(|d| d == step) {
            t3c::run_step(ctx, t3.track1.as_ref(), t3.cdda[0].as_deref(), step).context("Tekken 3")?;
            finish(ctx.out, &mut done, step)?;
        }
        t3c::record_step(&mut manifest, step, now)?;
    }
    // (where the manifest has always had them)
    manifest.insert("format".into(), json!(t3c::FORMAT));
    manifest.insert("fingerprint".into(), json!(""));
    let mut packs = serde_json::Map::new();
    if let Some(game) = inputs.cs {
        if !done.iter().any(|d| d == "cs") {
            ctx.log("[cs] the Counter-Strike levels");
            cs_levels::game::install_game(ctx, game).context("Counter-Strike")?;
            finish(ctx.out, &mut done, "cs")?;
        }
        packs.insert("cs".into(), json!(installed_levels(ctx.out)?));
    }
    if let Some(t2) = &inputs.t2 {
        if !done.iter().any(|d| d == "t2") {
            ctx.log("[t2] the Tekken 2 fighters");
            let scratch = Sub { inner: ctx.out, prefix: T2_SCRATCH.into() };
            let sub = Ctx { out: &scratch, log: ctx.log, jobs: ctx.jobs };
            let result = t2_converter::export_media::run(&sub, t2.track1.as_ref(), &t2.cdda, "media", &["video"])
                .and_then(|()| {
                    let videos = Sub { inner: ctx.out, prefix: format!("{T2_SCRATCH}/media/video") };
                    t2_converter::transplant::run(ctx, t2.track1.as_ref(), &videos, None)
                });
            ctx.out.remove_all(".tmp")?;
            result.context("Tekken 2")?;
            finish(ctx.out, &mut done, "t2")?;
        }
        packs.insert("t2".into(), json!({"file": t2.track1.name(), "sha1_16mb": sha1_16mb(t2.track1.as_ref())?}));
    }
    if !packs.is_empty() {
        manifest.insert("packs".into(), Value::Object(packs));
    }
    let manifest = t3c::write_manifest(ctx.out, manifest)?;
    ctx.out.checkpoint(&done)?;
    Ok(manifest)
}

/// A step finished: kept with the output (a pack's resume point).
fn finish(out: &dyn Vfs, done: &mut Vec<String>, step: &str) -> Result<()> {
    done.push(step.to_string());
    out.checkpoint(done)
}

/// The Counter-Strike levels in stages.json, by id.
fn installed_levels(out: &dyn Vfs) -> Result<Vec<String>> {
    let stages: Value = serde_json::from_slice(&out.read("gamedata/stages.json")?)?;
    Ok(stages["stages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|s| s["menu"] == cs_levels::install::MENU)
        .filter_map(|s| s["name"].as_str().map(str::to_string))
        .collect())
}

/// The first 16 MB's SHA-1 (tells disc images apart, as the Tekken 3 manifest's source does).
fn sha1_16mb(disc: &dyn DiscFile) -> Result<String> {
    let mut head = Vec::new();
    disc.open()?.take(16 << 20).read_to_end(&mut head).with_context(|| format!("cannot read {}", disc.name()))?;
    Ok(sha1::Sha1::digest(&head).iter().map(|b| format!("{b:02x}")).collect())
}
