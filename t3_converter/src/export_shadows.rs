//! export_shadows.py: export the fighters' shadow meshes to gamedata/shadows.json.
//!
//! usage: t3_convert export_shadows "Tekken_3_(USA)_(Track 1).bin" OUTDIR
//!
//! A fighter's shadow (0x8003AFCC / 0x8003B4F8) is not its visible model: it is a small mesh of its own, section 3 of
//! the model's texture record (record m + 2; 0x80036510). Each frame its vertices go through the bone matrices and the
//! shadow matrix (0x80035F2C: the stage's light direction, flattened onto the floor y = 0), and the faces are drawn as
//! opaque flat polygons in the stage's shadow colour (the light table's +0x10) at ordering-table entry 1007: after the
//! floor, before the fighters, with no depth test. Most fighters share one mesh; a few have their own (Yoshimitsu, the
//! bears and True Ogre, Mokujin, Gon); the Tekken Force soldiers have none.
//!
//! Section layout: +0 / +4 the packet buffers (set at run time), +8 the face list's offset, +0xC triangles, +0x10
//! quads, +0x14 18 offsets (the engine's bone slots 0..17), each {u32 n; n x (s16 x, y, z, pad)} in the slot's local
//! frame. Face list: u32 count then one word per triangle (indices (w >> 2) & 0xFF, (w >> 10) & 0xFF,
//! (w >> 20) & 0xFF), u32 count then one word per quad (indices in bytes 0..3). Indices count each slot's vertices
//! padded to a multiple of 3 (RTPT transforms three at a time).
//!
//! SCHEMA
//!   meshes: [{models: [model record...], joints: [glTF node name...],
//!             vertices: [[joint index, x, y, z]...] (metres, the joint's local axes, glTF's: y up),
//!             triangles: [[a, b, c]...], quads: [[a, b, c, d]...] (indices into vertices)}]

use crate::disc::{self, py_slice, u32le};
use crate::export_gltf::{SCALE, row_name};
use crate::{pyjson, skel};
use anyhow::{Context, Result};
use conv_io::{Ctx, DiscFile, StdDisc, StdFs, join, print_line};
use serde_json::{Value, json};

pub const SLOTS: usize = 18;

/// A shadow mesh: joints, vertices [joint, x, y, z], triangles, quads.
#[derive(PartialEq)]
pub struct Shadow {
    pub joints: Vec<String>,
    pub vertices: Vec<(usize, [f64; 3])>,
    pub triangles: Vec<Vec<usize>>,
    pub quads: Vec<Vec<usize>>,
}

/// Python's `round(x, 5)` (exact for the thousandths these values are)
fn round5(x: f64) -> f64 {
    format!("{x:.5}").parse().unwrap_or(x)
}

fn u32at(s: &[u8], o: usize) -> Result<usize> {
    s.get(o..o + 4).context("a shadow mesh runs past its section")?;
    Ok(u32le(s, o) as usize)
}

/// The shadow mesh in a texture record (None when it has none).
pub fn shadow_mesh(tex: &[u8]) -> Result<Option<Shadow>> {
    let n = u32at(tex, 0)?;
    if !(4..=16).contains(&n) {
        return Ok(None);
    }
    let (off, size) = (u32at(tex, 4 + 8 * 3)?, u32at(tex, 8 + 8 * 3)?);
    if size == 0 {
        return Ok(None);
    }
    let s = py_slice(tex, off, off.saturating_add(size));
    let face_off = u32at(s, 8)?;
    let (mut joints, mut vertices, mut padded) = (Vec::<String>::new(), Vec::new(), Vec::new());
    for slot in 0..SLOTS {
        let at = u32at(s, 0x14 + 4 * slot)?;
        let count = u32at(s, at)?;
        let name = row_name(skel::OFFSET_ROW[slot]);
        if count != 0 && !joints.contains(&name) {
            joints.push(name.clone());
        }
        for k in 0..count.div_ceil(3) * 3 {
            if k < count {
                let b = s.get(at + 4 + 8 * k..at + 10 + 8 * k).context("a shadow vertex runs past its section")?;
                let [x, y, z] = [0, 2, 4].map(|o| i16::from_le_bytes([b[o], b[o + 1]]) as f64);
                // PS1 axes (y down, z forward) to glTF's (y up): 180 degrees about x
                padded.push(Some(vertices.len()));
                let j = joints.iter().position(|j| *j == name).context("a shadow vertex without a joint")?;
                vertices.push((j, [round5(SCALE * x), round5(-SCALE * y), round5(-SCALE * z)]));
            } else {
                padded.push(None);
            }
        }
    }
    let words = |at: usize| -> Result<Vec<u32>> {
        let n = u32at(s, at)?;
        (0..n).map(|k| u32at(s, at + 4 + 4 * k).map(|w| w as u32)).collect()
    };
    let w3 = words(face_off)?;
    let w4 = words(face_off + 4 + 4 * w3.len())?;
    let face = |ids: Vec<u32>| -> Option<Vec<usize>> {
        ids.iter().map(|&i| padded.get(i as usize).copied().flatten()).collect()
    };
    let triangles =
        w3.iter().filter_map(|&w| face(vec![(w >> 2) & 0xFF, (w >> 10) & 0xFF, (w >> 20) & 0xFF])).collect();
    let quads = w4.iter().filter_map(|&w| face((0..4).map(|k| (w >> (8 * k)) & 0xFF).collect())).collect();
    Ok(Some(Shadow { joints, vertices, triangles, quads }))
}

pub fn shadows(rec: &[Vec<u8>]) -> Result<Value> {
    let mut meshes: Vec<(Vec<usize>, Shadow)> = Vec::new();
    for (i, r) in rec.iter().enumerate() {
        if r.get(8..12) != Some(b"3DMK") || i + 2 >= rec.len() {
            continue;
        }
        let Some(m) = shadow_mesh(&rec[i + 2])? else { continue };
        match meshes.iter_mut().find(|e| e.1 == m) {
            Some(e) => e.0.push(i),
            None => meshes.push((vec![i], m)),
        }
    }
    let meshes: Vec<Value> = meshes
        .into_iter()
        .map(|(models, m)| {
            let v: Vec<Value> = m.vertices.iter().map(|(j, p)| json!([j, p[0], p[1], p[2]])).collect();
            json!({"models": models, "joints": m.joints, "vertices": v, "triangles": m.triangles, "quads": m.quads})
        })
        .collect();
    Ok(json!({"source": "section 3 of each model's texture record (0x80036510); drawn by 0x8003AFCC / 0x8003B4F8",
              "meshes": meshes}))
}

pub fn main(args: &[String]) -> Result<()> {
    let [src, out, ..] = args else { anyhow::bail!("usage: export_shadows BIN OUTDIR") };
    run(&Ctx { out: &StdFs(out.into()), log: &print_line, jobs: 1 }, &StdDisc(src.into()), "")
}

/// Writes `out`/gamedata/shadows.json.
pub fn run(ctx: &Ctx, disc: &dyn DiscFile, out: &str) -> Result<()> {
    let game = disc::Game::load(disc)?;
    let data = shadows(&game.rec)?;
    ctx.out.write(&join(out, "gamedata/shadows.json"), pyjson::compact(&data).as_bytes())?;
    for m in data["meshes"].as_array().into_iter().flatten() {
        let len = |k: &str| m[k].as_array().map_or(0, Vec::len);
        ctx.log(&format!(
            "models {}: {} vertices, {} triangles, {} quads",
            pyjson::to_string(&m["models"], pyjson::DEFAULT),
            len("vertices"),
            len("triangles"),
            len("quads")
        ));
    }
    Ok(())
}
