//! `t3_convert SCRIPT ARGS...` runs one step (SCRIPT = the Python script's name without `.py`);
//! `t3_convert BIN OUTDIR [--only characters,stages,...] [--jobs N]` and `t3_convert --fingerprint OUTDIR`:
//! convert_all.py.

use anyhow::Result;
use t3_converter::reference::{attackpath, camera, camera_golden};
use t3_converter::*;

const SCRIPTS: &[(&str, Main)] = &[
    ("attackpath", attackpath::main),
    ("camera", camera::main),
    ("camera_golden", camera_golden::main),
    ("convert_all", convert_all::main),
    ("export_effects", export_effects::main),
    ("export_gamedata", export_gamedata::main),
    ("export_media", export_media::main),
    ("export_pictures", export_pictures::main),
    ("export_gltf", export_gltf::main),
    ("export_shadows", export_shadows::main),
    ("export_stages", export_stages::main),
    ("export_theater", export_theater::main),
    ("extract_models", extract_models::main),
    ("qa_glb", qa_glb::main),
    ("t3m", t3m::main),
    ("texrender", texrender::main),
];

fn run(args: &[String]) -> Result<()> {
    let first = args.first().map(String::as_str).unwrap_or("");
    if let Some((_, main)) = SCRIPTS.iter().find(|s| s.0 == first) {
        return main(&args[1..]);
    }
    // (a disc image's path, even a mistyped one: its error says it cannot be opened; step names have no dots)
    if first.starts_with("--") || first.contains(['.', '/']) {
        return convert_all::main(args);
    }
    let names: Vec<&str> = SCRIPTS.iter().map(|s| s.0).collect();
    anyhow::bail!(
        "unknown step {first:?}\nusage: t3_convert BIN OUTDIR [--only ...] [--jobs N] | t3_convert --fingerprint \
         OUTDIR | t3_convert STEP ARGS...\nsteps: {}",
        names.join(", ")
    )
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(e) = run(&args) {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}
