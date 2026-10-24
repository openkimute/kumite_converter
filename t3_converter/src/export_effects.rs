//! export_effects: hit sparks and effects: bake the game's effect cells in their palettes into one RGBA atlas,
//! and write the effect tables (docs: effects research; EXE sprite system 0x80076884 / 0x80076E70, effect objects
//! 0x8006F4F4).
//!
//! usage: t3_convert export_effects "Tekken_3_(USA)_(Track 1).bin" OUTDIR/pictures/effects
//! Run after export_pictures (it reads ../exe/007_index.png, ../exe/008_index.png and ../cluts.json) and
//! export_gamedata (../../gamedata/roster.json).
//!
//! Writes atlas.png (32x32 cells, ATLAS_COLS per row) and effects.json:
//!   strips: name -> {cells: [atlas cell...], life, blend: add|half, palette, note}
//!     spark  : the hit spark (kind 3 variant 0): sheet 007 cells 0-26, CLUT (32,503), red
//!     zero   : zero-damage contact / op 9 (kind 3 / 1): 007 cells 27-47, CLUT (48,503), pink
//!     block  : blocked hit (kind 3 / 2): 007 cells 48-63 then 008 cells 0-4, CLUT (64,503), steel blue
//!     dust   : ground impact / op 0xA (kind 3 / 3): 008 cells 9-35, CLUT (80,503), grey, 50 % blend
//!     heavy_a: the heavy-hit particle (types 1 / 2): sheet 008 16 px cell at (128,144), one atlas cell per step of
//!              palette ramp A (256 + 16 i, 510), i = 0..15 (white -> yellow -> red -> black)
//!     heavy_b: the ki-charged one (types 12 / 13): 008 (128,128), ramp B (256 + 16 i, 509) (white -> blue -> black)
//!   characters: model record -> {cells, life, light: [r, g, b]}: each character's own spark (its texture record,
//!              part 1: life x 32x32 4-bit TIMs, coloured with the first one's CLUT) and its hit-light colour
//!   templates (0x80027950: per character id {life, light}), kind3_life (0x800279D4), ground_offsets (0x8001E818),
//!   constants (code).

use crate::atlas::c15;
use crate::disc::{Game, py_slice, u32le};
use crate::export_pictures::{indices, save_png, words};
use crate::gamedata_core::Exe;
use crate::pyjson;
use crate::tim;
use anyhow::{Context, Result, bail};
use conv_io::{Ctx, DiscFile, StdDisc, StdFs, Vfs, join, print_line};
use image::{GenericImage, GrayImage, RgbaImage, imageops};
use serde_json::{Map, Value, json};
use std::path::Path;

const ATLAS_COLS: u32 = 32;
const CELL: u32 = 32;

/// 15-bit PS1 palette words applied to an index array: colour 0 is transparent.
fn rgba(pal: &[u16], w: u32, h: u32, idx: impl Iterator<Item = u16>) -> Result<RgbaImage> {
    let last = pal.len().checked_sub(1).context("an empty palette")?;
    RgbaImage::from_raw(w, h, idx.flat_map(|i| c15(pal[(i as usize).min(last)])).collect()).context("a cell's size")
}

fn read_json(out: &dyn Vfs, p: &str) -> Result<Value> {
    serde_json::from_str(&out.read_string(p)?).with_context(|| format!("{p} is not JSON"))
}

/// A '/'-path's folder ("" at the top).
fn parent(p: &str) -> &str {
    p.rsplit_once('/').map_or("", |(d, _)| d)
}

pub fn main(args: &[String]) -> Result<()> {
    let [src, out, ..] = args else { bail!("usage: export_effects BIN OUTDIR/pictures/effects") };
    let out = Path::new(out);
    let root = out.parent().and_then(Path::parent).context("OUTDIR/pictures/effects has no OUTDIR")?;
    let rel = out.strip_prefix(root)?.to_str().context("OUTDIR is not UTF-8")?;
    run(&Ctx { out: &StdFs(root.into()), log: &print_line, jobs: 1 }, &StdDisc(src.into()), rel)
}

/// Writes `out` (pictures/effects) from ../cluts.json, ../exe/*_index.png and ../../gamedata/roster.json.
pub fn run(ctx: &Ctx, disc: &dyn DiscFile, out: &str) -> Result<()> {
    let pics = parent(out);
    let g = Game::load(disc)?;
    let exe = Exe::new(&g.exe);
    let cluts = read_json(ctx.out, &join(pics, "cluts.json"))?;
    let cluts = cluts["cluts"]["exe"].as_object().context("cluts.json has no EXE palettes")?;
    let sheets: Vec<GrayImage> = [7, 8]
        .iter()
        .map(|n| {
            let p = join(pics, &format!("exe/{n:03}_index.png"));
            let png = image::load_from_memory_with_format(&ctx.out.read(&p)?, image::ImageFormat::Png);
            Ok(png.with_context(|| format!("cannot read {p}"))?.into_luma8())
        })
        .collect::<Result<_>>()?;
    let pal_of = |key: &str| -> Result<Vec<u16>> {
        let row =
            cluts.get(key).and_then(Value::as_array).with_context(|| format!("no palette {key} in cluts.json"))?;
        row.iter().map(|v| v.as_u64().map(|v| v as u16).context("a palette colour is not a number")).collect()
    };
    // numpy's clamped slice sheet[y:y + h, x:x + w]
    let crop = |s: usize, x: u32, y: u32, w: u32, h: u32| {
        let sh = &sheets[s];
        let (w, h) = (w.min(sh.width().saturating_sub(x)), h.min(sh.height().saturating_sub(y)));
        imageops::crop_imm(sh, x, y, w, h).to_image()
    };

    let mut cells: Vec<RgbaImage> = Vec::new(); // 32x32 RGBA tiles in atlas order
    let mut strips = Map::new();

    // kind 3 lives (templates 0x800279D4, 6 bytes each; variant 2 uses variant 1's)
    let k3 = (0..4).map(|v| exe.u16(0x800279D4 + 6 * v)).collect::<Result<Vec<_>>>()?;
    let mut strip =
        |name: &str, parts: &[(usize, std::ops::Range<u32>)], clut: &str, life, blend, note| -> Result<()> {
            let pal = pal_of(clut)?;
            let mut ids = Vec::new();
            for (s, ks) in parts {
                for k in ks.clone() {
                    let c = crop(*s, k % 8 * 32, k / 8 * 32, 32, 32);
                    cells.push(rgba(&pal, c.width(), c.height(), c.pixels().map(|p| p[0] as u16))?);
                    ids.push(cells.len() - 1);
                }
            }
            strips.insert(
                name.into(),
                json!({"cells": ids, "life": life, "blend": blend, "palette": clut, "note": note}),
            );
            Ok(())
        };
    strip("spark", &[(0, 0..27)], "32,503", k3[0], "add", "hit spark (kind 3/0): every damaging hit")?;
    strip("zero", &[(0, 27..48)], "48,503", k3[1], "add", "zero-damage contact, op 9 (kind 3/1)")?;
    strip("block", &[(0, 48..64), (1, 0..5)], "64,503", k3[1], "add", "blocked hit (kind 3/2)")?;
    strip("dust", &[(1, 9..36)], "80,503", k3[3], "half", "ground impact, op 0xA (kind 3/3): 50 % blend")?;

    // the heavy-hit particles: a 16 px cell of sheet 008 in each palette of a ramp (16 sub-palettes per row)
    for (name, (u, v), row) in [("heavy_a", (128, 144), "256,510"), ("heavy_b", (128, 128), "256,509")] {
        let ramp = pal_of(row)?;
        let small = crop(1, u, v, 16, 16);
        let mut ids = Vec::new();
        for i in 0..16 {
            let sub = &ramp[(16 * i).min(ramp.len())..(16 * i + 16).min(ramp.len())];
            let c = rgba(sub, small.width(), small.height(), small.pixels().map(|p| p[0] as u16))?;
            let big = imageops::resize(&c, c.width() * 2, c.height() * 2, imageops::FilterType::Nearest);
            if big.dimensions() != (CELL, CELL) {
                bail!("sheet 008's heavy-hit cell at ({u}, {v}) is not 16 x 16");
            }
            cells.push(big);
            ids.push(cells.len() - 1);
        }
        strips.insert(
            name.into(),
            json!({"cells": ids, "life": 0, "blend": "add", "palette": format!("{row} + 16 i"),
                                          "note": "one cell per ramp step i = age * 16 / life (16 px cell scaled x2)"}),
        );
    }

    // per character id: spark life and hit-light colour (0x80027950, 6 bytes: u16 life, r, g, b)
    let templates = (0..22)
        .map(|c| {
            let a = 0x80027950 + 6 * c;
            Ok(json!({"life": exe.u16(a)?, "light": [exe.u8(a + 2)?, exe.u8(a + 3)?, exe.u8(a + 4)?]}))
        })
        .collect::<Result<Vec<Value>>>()?;
    // each character model's own spark frames (texture record = model record + 2, part 1)
    let roster = read_json(ctx.out, &join(parent(pics), "gamedata/roster.json"))?;
    let mut char_of: Vec<(i64, Value)> = Vec::new(); // the first selection of a model (later rows reuse models)
    for e in roster["selections"]["entries"].as_array().context("roster.json has no selections")? {
        if let Some(m) = e["model_record"].as_i64().filter(|&m| m != 0 && !char_of.iter().any(|c| c.0 == m)) {
            char_of.push((m, e["character"].clone()));
        }
    }
    let mut characters = Map::new();
    for m in (71..276).step_by(4) {
        let t = g.rec.get(m + 2).map_or(&[][..], |r| &r[..]);
        if t.len() < 20 {
            continue;
        }
        let (o, z) = (u32le(t, 12) as usize, u32le(t, 16) as usize);
        let frames: Vec<_> = tim::walk_tims(py_slice(t, o, o.saturating_add(z)))
            .into_iter()
            .filter(|x| x.bpp == 4 && (x.img.2, x.img.3) == (8, 32))
            .collect();
        let Some(clut) = frames.first().and_then(|f| f.clut.as_ref()) else { continue };
        let pal = words(clut).context("a spark's CLUT runs short")?;
        let pal = &pal[..pal.len().min(16)];
        let mut ids = Vec::new();
        for fr in &frames {
            let (w, h, idx) = indices(&fr.img, 4).context("a spark frame runs short")?;
            cells.push(rgba(pal, w, h, idx.into_iter())?);
            ids.push(cells.len() - 1);
        }
        let c = char_of.iter().find(|c| c.0 == m as i64).map_or(Value::Null, |c| c.1.clone());
        let tpl = c.as_i64().and_then(|c| usize::try_from(c).ok()).and_then(|c| templates.get(c));
        let (life, light) = match tpl {
            Some(t) => (t["life"].clone(), t["light"].clone()),
            None => (json!(frames.len()), json!([255, 255, 255])),
        };
        characters.insert(m.to_string(), json!({"character": c, "cells": ids, "life": life, "light": light}));
    }

    let rows = (cells.len() as u32).div_ceil(ATLAS_COLS);
    let mut atlas = RgbaImage::new(ATLAS_COLS * CELL, rows * CELL);
    for (k, tile) in cells.iter().enumerate() {
        let (r, c) = (k as u32 / ATLAS_COLS, k as u32 % ATLAS_COLS);
        atlas.copy_from(tile, c * CELL, r * CELL)?;
    }
    save_png(ctx.out, &join(out, "atlas.png"), &atlas)?;
    let ground = (0..6)
        .map(|k| (0..3).map(|j| exe.s16(0x8001E818 + 8 * k + 2 * j)).collect::<Result<Vec<_>>>())
        .collect::<Result<Vec<_>>>()?;
    let data = json!({
        "atlas": {"file": "atlas.png", "cell": CELL, "cols": ATLAS_COLS, "rows": rows},
        "strips": strips,
        "characters": characters,
        "templates": templates,
        "kind3_life": k3,
        "ground_offsets": ground,
        "constants": {
            "hit_slots": 3, "ground_groups": 3, "size_mm": 188.6, "drag": [118, 128], "speed_mm": 50,
            "min_height_mm": 250, "fade_frames": 10, "fade_from": 128, "fade_to": 32, "ground_cadence": 2,
            "ground_y_mm": 250, "heavy_damage": 21, "heavy_counts": [6, 20], "hit_light_frames": 48,
            "heavy_quad_mm": [110, 70], "rng": "PsyQ rand: seed = seed * 0x41C64E6D + 12345; (seed >> 16) & 0x7FFF",
            "note": "code constants of 0x80076884 / 0x80076E70 / 0x8004A990 / 0x8006F4F4 (effects research)"},
    });
    ctx.out.write(&join(out, "effects.json"), pyjson::indent1(&data).as_bytes())?;
    ctx.log(&format!("effects: {} cells ({rows} rows), {} character sparks", cells.len(), characters.len()));
    Ok(())
}
