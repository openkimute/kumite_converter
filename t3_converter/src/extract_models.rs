//! extract_models.py: every Tekken 3 (USA, SLUS-00402) 3D character model + texture to OBJ/MTL/PNG.
//!
//! usage: t3_convert extract_models "Tekken_3_(USA)_(Track 1).bin" OUTDIR [--preview]
//!
//! Format notes: see docs/reference/tekken3/disc.md, section "3D models". The disc access (iso_files, read_file,
//! the BNS table) is in disc.rs.

use crate::assemble::{self, M3, rot};
use crate::{atlas, disc, texrender};
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::Path;

/// Improvised T-pose (no animation data is decoded): flip pelvis so legs point down, swing the clavicles out
/// sideways, stand the head (whose verts run along local +Z) up.
pub fn pose() -> HashMap<usize, M3> {
    HashMap::from([
        (3, rot('z', 180.0)),
        (11, rot('y', -90.0)),
        (15, rot('y', 90.0)),
        (19, rot('y', 90.0)),
        (20, rot('y', 90.0)),
    ])
}
/// Row 20 is an alternate head/face section textured from runtime-animated VRAM (facial frames); skipped.
pub const SKIP_SECTIONS: [usize; 1] = [20];

pub fn export_model(model: &[u8], tex: &[u8], outdir: &Path, name: &str) -> Result<(usize, usize)> {
    let (a, bycl) = atlas::build_atlas(tex, 0)?;
    let (h, w) = (a.height() as f64, a.width() as f64);
    let (verts, faces, _) = assemble::build(model, &pose())?;
    a.save(outdir.join(format!("{name}.png"))).context("cannot write a texture")?;
    let mut used: HashMap<usize, usize> = HashMap::new();
    let mut order = Vec::new(); // used's keys in insertion order
    let (mut vt, mut fl) = (Vec::new(), Vec::new());
    for f in &faces {
        if SKIP_SECTIONS.contains(&f.k) {
            continue;
        }
        let Some(ids) = f.ids.iter().map(|i| i.filter(|&i| verts[i].is_some())).collect::<Option<Vec<usize>>>() else {
            continue;
        };
        let mut fv = Vec::new();
        // PS1 quad -> polygon order
        let corners: &[usize] = if ids.len() == 4 { &[0, 1, 3, 2] } else { &[0, 1, 2] };
        for &j in corners {
            let i = ids[j];
            let n = *used.entry(i).or_insert_with(|| {
                order.push(i);
                order.len()
            });
            let uv = f.uv.as_ref().filter(|uv| !uv.is_empty());
            let at = match uv {
                Some(uv) => {
                    let (u, v) = *uv.get(j).context("a polygon has fewer UVs than corners")?;
                    atlas::atlas_uv(u as u32, v as u32, f.cl.unwrap_or(0), &bycl)
                }
                None => None,
            };
            match at {
                None => fv.push((n, None)),
                Some((x, y)) => {
                    vt.push((x as f64 / w, 1.0 - (y as f64 + 0.5) / h));
                    fv.push((n, Some(vt.len())));
                }
            }
        }
        fl.push(fv);
    }
    std::fs::write(
        outdir.join(format!("{name}.mtl")),
        format!("newmtl {name}\nKd 1 1 1\nmap_Kd {name}.png\nmap_d {name}.png\n"),
    )?;
    let mut o = format!("# Tekken 3 (PS1) {name}\nmtllib {name}.mtl\no {name}\n");
    for &i in &order {
        let p = verts[i].context("a used vertex is empty")?;
        let _ = writeln!(o, "v {:.4} {:.4} {:.4}", p[2].v / 1000.0, p[0].v / 1000.0, p[1].negated().v / 1000.0);
    }
    for (a, b) in &vt {
        let _ = writeln!(o, "vt {a:.5} {b:.5}");
    }
    let _ = writeln!(o, "usemtl {name}");
    for fv in &fl {
        let c: Vec<String> = fv.iter().map(|(v, t)| t.map_or(format!("{v}"), |t| format!("{v}/{t}"))).collect();
        let _ = writeln!(o, "f {}", c.join(" "));
    }
    std::fs::write(outdir.join(format!("{name}.obj")), o)?;
    Ok((used.len(), fl.len()))
}

pub fn main(args: &[String]) -> Result<()> {
    let pos: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    let [src, outdir, ..] = pos[..] else { anyhow::bail!("usage: extract_models BIN OUTDIR [--preview]") };
    let preview = args.iter().any(|a| a == "--preview");
    let outdir = Path::new(outdir);
    std::fs::create_dir_all(outdir).with_context(|| format!("cannot create {}", outdir.display()))?;
    let game = disc::Game::load(&conv_io::StdDisc(src.into()))?;
    let rec = &game.rec;
    let mut n = 0;
    for (i, r) in rec.iter().enumerate() {
        if disc::py_slice(r, 8, 12) != b"3DMK" {
            continue;
        }
        let name = format!("model_{i:03}");
        let tex = rec.get(i + 2).context("a model's texture record is past the table")?;
        let (v, fc) = export_model(r, tex, outdir, &name).context(name.clone())?;
        println!("{name}: {v} verts, {fc} faces  (textures: record {}, sound VAB: {}, anim?: {})", i + 2, i + 1, i + 3);
        if preview {
            let f = |s: &str| outdir.join(format!("{name}{s}"));
            texrender::render(&f(".obj"), &f(".png"), &f("_preview.png"))?;
        }
        n += 1;
    }
    println!("{n} models written to {}", outdir.display());
    Ok(())
}
