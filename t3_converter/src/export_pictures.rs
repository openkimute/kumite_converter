//! export_pictures: Tekken 3 (USA, SLUS-00402) 2D pictures (portraits, menus, logos, HUD, Theater stills) to PNG.
//!
//! usage: t3_convert export_pictures "Tekken_3_(USA)_(Track 1).bin" OUTDIR
//!
//! Most 2D art is LZ-compressed (EXE routine 0x80031BFC, see docs/reference/tekken3/engine.md §4.4):
//!   flag byte, up to 7 flags LSB first (top set bit is a sentinel); 1 = literal byte,
//!   0 = big-endian u16 token: distance = low 11 bits (0 -> 2048), length = top 5 bits (0 -> 32);
//!   a flag byte of 0 ends the stream.
//! Compressed TIMs live in containers (u32 n; n x {offset, size}), in whole records (portraits 14-35),
//! or loose inside overlays. This scans each source for LZ streams that decode to a valid TIM,
//! drops duplicates, and when the pictures of one container tile together in VRAM (logo strips, panel
//! tiles) also writes the assembled picture. OUTDIR/index.json lists the sets for the viewer.

use crate::atlas::c15;
use crate::disc::{Game, py_slice, u32le};
use crate::export_stages::{load_vram, texels};
use crate::gamedata_core::Exe;
use crate::gamedata_tables::imm;
use crate::pyjson;
use crate::tim::{self, Block};
use anyhow::{Context, Result, bail};
use conv_io::{Ctx, DiscFile, StdDisc, StdFs, Vfs, join, print_line};
use image::{GenericImage, GrayImage, RgbaImage};
use serde_json::{Map, Value, json};
use std::collections::HashSet;

/// visible width of the game's 368x480 interlaced screen
pub const SCREEN_W: u32 = 368;
/// (set name, source, LZ?, UI?)  source = None for the EXE or a BNS record number; UI: the engine's screens draw
/// from its VRAM places
const SOURCES: [(&str, Option<usize>, bool, bool); 12] = [
    ("In-game HUD and fonts", None, true, true),
    ("Title and logos", Some(0), true, true),
    ("Character select", Some(9), true, true),
    ("Menus and options", Some(10), true, true),
    ("Records screen", Some(11), true, true),
    ("Lightning panel (red)", Some(12), true, true),
    ("Lightning panel (purple)", Some(13), true, true),
    ("Theater thumbnails", Some(301), true, true),
    ("Theater backgrounds", Some(301), false, false),
    ("Results and ending stills", Some(302), true, false),
    ("Tekken Ball rules card", Some(55), false, false),
    ("Tekken Force overlay", Some(7), false, true), // its three key icons (5 x 36 words, 4-bit; HUD 0x800B4440)
];
/// raw sources whose small pictures are kept (raw sets otherwise skip texture tiles under 100 x 100)
const RAW_SMALL: [usize; 1] = [7];
/// A Tekken Force pickup's quad: (name, CLUT word, u0, v0, u1, v1).
type Pickup = (&'static str, u32, u32, u32, u32, u32);

/// Tekken Force's roast chicken (record 7 0x800B3E90): drawn from the loaded stage's VRAM (records 51-54, Force
/// stages 1-4) with the quads record 7's 0x800B3CB4 sets up: a texture page, the chicken's CLUT and rows, its
/// shadow's from the top (the two TIMs under the chicken carry their own CLUTs; the game reads both with one).
fn force_pickups(r7: &[u8]) -> Result<([Pickup; 2], u32)> {
    let r7 = Exe::with_base(r7, 0x800B0548);
    let i = |a: i64| -> Result<u32> { Ok(imm(&r7, a)? as u32) };
    // (u0 and the shadow's v0: the code stores zero)
    let (corner, chicken_v) = (i(0x800B3CD8)?, [i(0x800B3D24)?, i(0x800B3D2C)?]);
    Ok((
        [
            ("chicken", i(0x800B3D1C)?, 0, chicken_v[0], corner, chicken_v[1]),
            ("shadow", i(0x800B3D94)?, 0, 0, corner, corner),
        ],
        i(0x800B3D18)?,
    ))
}

/// A TIM as valid_tim / tim.walk_tims give it: some carry only a CLUT.
pub struct Pic<'a> {
    pub bpp: u32,
    pub clut: Option<Block<'a>>,
    pub img: Option<Block<'a>>,
}

/// The LZ stream at `pos`: (decoded bytes, end). None where Python returns None or raises IndexError.
pub fn lz(src: &[u8], mut pos: usize) -> Option<(Vec<u8>, usize)> {
    const LIMIT: usize = 1 << 21;
    let mut out = Vec::new();
    let mut flags = *src.get(pos)?;
    pos += 1;
    if flags == 0 {
        return None;
    }
    loop {
        while flags >= 2 {
            if flags & 1 != 0 {
                out.push(*src.get(pos)?);
                pos += 1;
            } else {
                let w = (*src.get(pos)? as usize) << 8 | *src.get(pos + 1)? as usize;
                pos += 2;
                let dist = match w & 0x7FF {
                    0 => 0x800,
                    d => d,
                };
                let n = match (w >> 11) & 0x1F {
                    0 => 0x20,
                    n => n,
                };
                let s = out.len().checked_sub(dist)?;
                for k in 0..n {
                    out.push(out[s + k]);
                }
            }
            flags >>= 1;
            if out.len() > LIMIT {
                return None;
            }
        }
        flags = *src.get(pos)?;
        pos += 1;
        if flags == 0 {
            return Some((out, pos));
        }
    }
}

/// A decoded LZ stream that is one TIM (as tim.walk_tims), else None.
/// Some TIMs are only a CLUT (palettes loaded on their own), and some carry a wrong image block length
/// (the select screen's chrome plates): the block size is taken from its width and height instead.
pub fn valid_tim(b: &[u8]) -> Option<Pic<'_>> {
    if b.len() < 20 || b[..4] != [0x10, 0, 0, 0] || ![0, 1, 2, 3, 8, 9, 10, 11].contains(&b[4]) {
        return None;
    }
    let fl = b[4];
    let mut q = 8;
    let mut t = Pic { bpp: [4, 8, 16, 24][(fl & 3) as usize], clut: None, img: None };
    let keys: &[bool] = if fl & 8 != 0 { &[true, false] } else { &[false] }; // is the block the CLUT?
    for &is_clut in keys {
        if q + 12 > b.len() {
            break;
        }
        let u16at = |o: usize| u16::from_le_bytes([b[o], b[o + 1]]);
        let (x, y, w, h) = (u16at(q + 4), u16at(q + 6), u16at(q + 8), u16at(q + 10));
        let n = 12 + w as usize * h as usize * 2;
        if w == 0 || h == 0 || x >= 1024 || y >= 512 || q + n > b.len() {
            return None;
        }
        let block = Some((x, y, w, h, &b[q + 12..q + n]));
        if is_clut {
            t.clut = block
        } else {
            t.img = block
        }
        q += n;
    }
    (t.img.is_some() || t.clut.is_some()).then_some(t)
}

impl<'a> From<tim::Tim<'a>> for Pic<'a> {
    fn from(t: tim::Tim<'a>) -> Pic<'a> {
        Pic { bpp: t.bpp, clut: t.clut, img: Some(t.img) }
    }
}

/// A block's 16-bit words (h x w); None when its data is short (numpy's reshape would raise).
pub fn words(b: &Block) -> Option<Vec<u16>> {
    let n = b.2 as usize * b.3 as usize;
    let px = b.4.get(..n * 2)?;
    Some(px.as_chunks::<2>().0.iter().map(|&c| u16::from_le_bytes(c)).collect())
}

/// A 4- or 8-bit picture's indices: (width in pixels, height, row-major indices).
pub fn indices(b: &Block, bpp: u32) -> Option<(u32, u32, Vec<u16>)> {
    let ws = words(b)?;
    let (w, h) = (b.2 as u32, b.3 as u32);
    match bpp {
        4 => Some((w * 4, h, ws.iter().flat_map(|v| [0, 4, 8, 12].map(|s| (v >> s) & 15)).collect())),
        8 => Some((w * 2, h, ws.iter().flat_map(|v| [v & 255, v >> 8]).collect())),
        _ => None,
    }
}

fn rgba_of(w: u32, h: u32, colours: impl Iterator<Item = u16>) -> Option<RgbaImage> {
    RgbaImage::from_raw(w, h, colours.flat_map(c15).collect())
}

/// TIM -> RGBA (colour 0x0000 is transparent, as on the PS1).
pub fn render(t: &Pic) -> Option<RgbaImage> {
    let img = t.img.as_ref()?;
    if t.bpp == 16 {
        return rgba_of(img.2 as u32, img.3 as u32, words(img)?.into_iter());
    }
    let pal: Vec<u16> = match &t.clut {
        Some(c) => words(c)?,
        None => (0..=255u16).map(|i| i.wrapping_mul(0x421) / 8).collect(), // numpy uint16 arithmetic wraps
    };
    let (w, h, mut idx) = indices(img, t.bpp)?;
    if t.bpp == 8 && pal.len() >= 256 && idx.iter().all(|&i| i < 64) && h > 64 {
        // banded portraits: 6-bit pixels, and each 64-row band uses its own 64-colour palette
        for (k, i) in idx.iter_mut().enumerate() {
            *i += 64 * (k as u32 / w / 64).min(3) as u16;
        }
    }
    let last = pal.len().checked_sub(1)?;
    rgba_of(w, h, idx.into_iter().map(|i| pal[(i as usize).min(last)]))
}

/// Every LZ stream in d that decodes to a TIM: (offset, end, decoded bytes); `valid_tim` of the bytes is the TIM.
fn scan_lz_tims(d: &[u8]) -> Vec<(usize, usize, Vec<u8>)> {
    let mut found = Vec::new();
    let mut o = 0;
    while o + 8 < d.len() {
        // the TIM magic 10 00 00 00 starts with a literal 0x10, so the first flag bit is 1 and byte 1 is 0x10
        if d[o] & 1 != 0
            && d[o + 1] == 0x10
            && let Some((out, end)) = lz(d, o).filter(|(out, _)| valid_tim(out).is_some())
        {
            found.push((o, end, out));
            o = end;
            continue;
        }
        o += 1;
    }
    found
}

/// Place same-depth TIMs by VRAM position; return a picture if they tile a rectangle.
fn assemble(tims: &[&Pic]) -> Option<RgbaImage> {
    let bpp = tims.first()?.bpp;
    if tims.len() < 2 || tims.iter().any(|t| t.bpp != bpp) || ![4, 8, 16].contains(&bpp) {
        return None;
    }
    let k = 16 / bpp as i64; // pixels per VRAM word
    let boxes: Vec<[i64; 4]> = tims
        .iter()
        .map(|t| t.img.as_ref().map(|i| [i.0 as i64 * k, i.1 as i64, i.2 as i64 * k, i.3 as i64]))
        .collect::<Option<_>>()?;
    let x0 = boxes.iter().map(|b| b[0]).min()?;
    let y0 = boxes.iter().map(|b| b[1]).min()?;
    let x1 = boxes.iter().map(|b| b[0] + b[2]).max()?;
    let y1 = boxes.iter().map(|b| b[1] + b[3]).max()?;
    let area: i64 = boxes.iter().map(|b| b[2] * b[3]).sum();
    if area != (x1 - x0) * (y1 - y0) || (x1 - x0) * (y1 - y0) > 1024 * 1024 {
        return None;
    }
    let (cw, ch) = ((x1 - x0) as usize, (y1 - y0) as usize);
    let mut cover = vec![0u8; cw * ch];
    for b in &boxes {
        for y in b[1] - y0..b[1] - y0 + b[3] {
            for x in b[0] - x0..b[0] - x0 + b[2] {
                let c = &mut cover[y as usize * cw + x as usize];
                *c = c.saturating_add(1);
            }
        }
    }
    if cover.iter().any(|&c| c != 1) {
        return None;
    }
    let mut a = RgbaImage::new(cw as u32, ch as u32);
    for (t, b) in tims.iter().zip(&boxes) {
        a.copy_from(&render(t)?, (b[0] - x0) as u32, (b[1] - y0) as u32).ok()?;
    }
    Some(a)
}

/// Screen strips stored in separate VRAM places (title logo, stills): try row-major grids whose
/// rows have equal total width and equal piece heights.
fn assemble_rows(tims: &[&Pic]) -> Option<RgbaImage> {
    let imgs: Vec<RgbaImage> = tims.iter().map(|t| render(t)).collect::<Option<_>>()?;
    let n = imgs.len();
    if n < 2 {
        return None;
    }
    for k in (2..=n).filter(|k| n.is_multiple_of(*k)) {
        let rows: Vec<&[RgbaImage]> = imgs.chunks(k).collect();
        if rows.iter().all(|r| r.iter().map(|i| i.width()).sum::<u32>() == SCREEN_W)
            && rows.iter().all(|r| r.iter().all(|i| i.height() == r[0].height()))
        {
            let mut a = RgbaImage::new(SCREEN_W, rows.iter().map(|r| r[0].height()).sum());
            let mut y = 0;
            for r in rows {
                let mut x = 0;
                for i in r {
                    a.copy_from(i, x, y).ok()?;
                    x += i.width();
                }
                y += r[0].height();
            }
            return Some(a);
        }
    }
    None
}

pub fn save_png<P: image::PixelWithColorType<Subpixel = u8>>(
    out: &dyn Vfs,
    path: &str,
    img: &image::ImageBuffer<P, Vec<u8>>,
) -> Result<()> {
    let mut png = std::io::Cursor::new(Vec::new());
    img.write_to(&mut png, image::ImageFormat::Png).with_context(|| format!("cannot encode {path}"))?;
    out.write(path, &png.into_inner())
}

fn index_png(b: &Block, bpp: u32) -> Option<GrayImage> {
    let (w, h, idx) = indices(b, bpp)?;
    GrayImage::from_raw(w, h, idx.into_iter().map(|i| i as u8).collect())
}

/// Writes a picture unless the same pixels were already written: its index.json entry.
fn save(
    ctx: &Ctx,
    out: &str,
    seen: &mut HashSet<Vec<u8>>,
    key: &str,
    n: &str,
    rgba: &RgbaImage,
    meta: Value,
) -> Result<Option<Map<String, Value>>> {
    if !seen.insert(rgba.as_raw().clone()) {
        return Ok(None);
    }
    let file = format!("{key}/{n}.png");
    save_png(ctx.out, &join(out, &file), rgba)?;
    let mut e = Map::new();
    e.insert("file".into(), json!(file));
    e.insert("w".into(), json!(rgba.width()));
    e.insert("h".into(), json!(rgba.height()));
    if let Value::Object(m) = meta {
        e.extend(m);
    }
    Ok(Some(e))
}

pub fn main(args: &[String]) -> Result<()> {
    let [src, out, ..] = args else { bail!("usage: export_pictures BIN OUTDIR") };
    run(&Ctx { out: &StdFs(out.into()), log: &print_line, jobs: 1 }, &StdDisc(src.into()), "")
}

/// Writes the pictures under `out`.
pub fn run(ctx: &Ctx, disc: &dyn DiscFile, out: &str) -> Result<()> {
    let g = Game::load(disc)?;
    let rec = |i: usize| g.rec.get(i).map_or(&[][..], |r| &r[..]);
    let mut seen = HashSet::new();
    let mut sets = Vec::new();
    let mut cluts = Map::new();

    // large portraits: one compressed TIM per record 14-35, the characters' (named as the EXE has them) and a
    // silhouette
    let exe = crate::gamedata_core::Exe::new(&g.exe);
    let mut imgs = Vec::new();
    for i in 0..22 {
        let name = if i < 21 { crate::gamedata_tables::char_name(&exe, i as i64)? } else { "Silhouette".to_string() };
        let (d, _) = lz(rec(14 + i), 0).with_context(|| format!("record {}: no LZ stream", 14 + i))?;
        let t = valid_tim(&d).with_context(|| format!("record {}: not a TIM", 14 + i))?;
        let rgba = render(&t).with_context(|| format!("record {}: no picture", 14 + i))?;
        if let Some(e) =
            save(ctx, out, &mut seen, "portraits", &format!("{i:02}"), &rgba, json!({"name": name, "bpp": t.bpp}))?
        {
            imgs.push(Value::Object(e));
        }
    }
    sets.push(json!({"name": "Large portraits", "source": "records 14-35", "images": imgs}));

    // fight HUD names: each character's texture record (model record + 2) holds a 4-bit picture of its name at
    // texture-relative (80, 0) without a CLUT; the HUD colours it per costume (EXE 0x8004E8B4)
    let mut imgs = Vec::new();
    for m in (71..276).step_by(4) {
        let t = rec(m + 2);
        if t.len() < 12 {
            continue;
        }
        let (o, z) = (u32le(t, 4) as usize, u32le(t, 8) as usize);
        let tims = tim::walk_tims(py_slice(t, o, o.saturating_add(z)));
        let Some(name) = tims.into_iter().find(|x| x.bpp == 4 && x.img.0 == 80 && x.img.1 == 0) else { continue };
        let name = Pic::from(name);
        let Some(b) = name.img.as_ref() else { continue };
        let what = || format!("model {m}: a short name picture");
        save_png(ctx.out, &join(out, &format!("names/model_{m:03}_index.png")), &index_png(b, 4).with_context(what)?)?;
        save_png(ctx.out, &join(out, &format!("names/model_{m:03}.png")), &render(&name).with_context(what)?)?;
        imgs.push(
            json!({"file": format!("names/model_{m:03}.png"), "index_map": format!("names/model_{m:03}_index.png"),
                         "w": b.2 as u32 * 4, "h": b.3, "bpp": 4, "model_record": m,
                         "note": "no CLUT: the fight HUD colours it per costume"}),
        );
    }
    sets.push(json!({"name": "Fight names", "source": "character texture records (model + 2)", "images": imgs}));

    for (name, source, packed, ui) in SOURCES {
        let d = source.map_or(&g.exe[..], rec);
        let key = source.map_or("exe".to_string(), |s| format!("r{s:03}")) + if packed { "" } else { "_raw" };
        let scanned = if packed { scan_lz_tims(d) } else { Vec::new() };
        let groups: Vec<Vec<Pic>> = if packed {
            let found: Vec<(usize, usize, Pic)> =
                scanned.iter().filter_map(|(o, end, b)| valid_tim(b).map(|t| (*o, *end, t))).collect();
            // every palette of a source with its VRAM place, so the game can colour index maps with the CLUT
            // the original code picks (fonts, select-screen pieces, cursors); colours as 15-bit words
            for (_, _, t) in &found {
                let Some(c) = &t.clut else { continue };
                let ws = words(c).context("a CLUT runs short")?;
                let entry = cluts.entry(key.clone()).or_insert_with(|| json!({}));
                for (r, row) in ws.chunks(c.2 as usize).enumerate() {
                    entry[format!("{},{}", c.0, c.1 as usize + r)] = json!(row);
                }
            }
            // split into containers (runs listed by an offset table just before them); CLUT-only TIMs carry no picture
            let mut groups: Vec<Vec<Pic>> = Vec::new();
            let mut last_end: Option<usize> = None;
            for (o, end, t) in found.into_iter().filter(|f| f.2.img.is_some()) {
                if last_end.is_none_or(|e| o as i64 - e as i64 > 8) {
                    groups.push(Vec::new());
                }
                last_end = Some(end);
                if let Some(g) = groups.last_mut() {
                    g.push(t);
                }
            }
            groups
        } else {
            vec![tim::walk_tims(d).into_iter().filter(|t| t.clut.is_some()).map(Pic::from).collect()]
        };
        let mut imgs = Vec::new();
        let mut n = 0;
        for g in &groups {
            let mut tims: Vec<&Pic> = g.iter().collect();
            let mut comp = if packed { assemble(&tims) } else { None };
            let mut rest = Vec::new();
            if comp.is_none() && packed && tims.len() <= 13 {
                let l = tims.len().saturating_sub(1);
                for (part, left) in [(&tims[..], &tims[..0]), (&tims[..l], &tims[l..]), (&tims[1..], &tims[..1])] {
                    comp = assemble_rows(part);
                    if comp.is_some() {
                        rest = left.to_vec();
                        break;
                    }
                }
            }
            if let Some(c) = comp.filter(|c| c.width() * c.height() >= 128 * 64) {
                let what = if c.width() == SCREEN_W { "full screen" } else { "assembled" };
                if let Some(e) = save(
                    ctx,
                    out,
                    &mut seen,
                    &key,
                    &format!("{n:03}_full"),
                    &c,
                    json!({"name": what, "bpp": tims[0].bpp}),
                )? {
                    imgs.push(Value::Object(e));
                    n += 1;
                }
                tims = rest; // the pieces add nothing on their own
            }
            for t in tims {
                let Some(rgba) = render(t) else { continue };
                let area = rgba.width() * rgba.height();
                if area < 16 * 16 || !rgba.pixels().any(|p| p[3] != 0) {
                    continue;
                }
                if !packed && !source.is_some_and(|s| RAW_SMALL.contains(&s)) && area < 100 * 100 {
                    continue; // raw sets: skip texture tiles
                }
                let Some(b) = t.img.as_ref() else { continue };
                let Some(mut e) = save(
                    ctx,
                    out,
                    &mut seen,
                    &key,
                    &format!("{n:03}"),
                    &rgba,
                    json!({"bpp": t.bpp, "vram": [b.0, b.1]}),
                )?
                else {
                    continue;
                };
                if t.bpp == 4 || t.bpp == 8 {
                    // the raw 4/8-bit indices as a grayscale picture (value = index): the game often draws a picture
                    // with another CLUT than its own (fonts, cursors, tinted backgrounds); see cluts.json
                    let idx = index_png(b, t.bpp).context("a picture runs short")?;
                    save_png(ctx.out, &join(out, &format!("{key}/{n:03}_index.png")), &idx)?;
                    e.insert("index_map".into(), json!(format!("{key}/{n:03}_index.png")));
                    match &t.clut {
                        Some(c) => e.insert("clut".into(), json!([c.0, c.1])),
                        None => e.insert(
                            "note".into(),
                            json!("no CLUT: colours are a placeholder ramp; index_map holds the indices"),
                        ),
                    };
                }
                imgs.push(Value::Object(e));
                n += 1;
            }
        }
        ctx.log(&format!("{name:<32} {} pictures", imgs.len()));
        if !imgs.is_empty() {
            let source = source.map_or("EXE".to_string(), |s| format!("record {s}"));
            sets.push(json!({"name": name, "source": source, "ui": ui, "images": imgs}));
        }
    }
    // Tekken Force's pickups, per stage, read from that stage's VRAM as the game draws them
    let mut imgs = Vec::new();
    let (pickups, tpage) = force_pickups(rec(7))?;
    for k in 0..4 {
        let vram = load_vram(rec(51 + k)).with_context(|| format!("record {}", 51 + k))?;
        for (what, clut, u0, v0, u1, v1) in pickups {
            let (cx, cy, px) = (16 * (clut & 0x3F), clut >> 6, 64 * (tpage & 15));
            let rgba = texels(&vram, tpage, clut, u0, v0, u1, v1)
                .with_context(|| format!("record {}: its VRAM does not hold the pickups", 51 + k))?;
            let file = format!("force/{what}_{k}.png");
            save_png(ctx.out, &join(out, &file), &rgba)?;
            imgs.push(json!({"file": file, "w": rgba.width(), "h": rgba.height(), "force_stage": k, "record": 51 + k,
                             "name": what, "note": format!("record 7 0x800B3E90: tpage x {px}, CLUT ({cx}, {cy}), rows {v0}..{v1}")}));
        }
    }
    ctx.log(&format!("{:<32} {} pictures", "Tekken Force pickups", imgs.len()));
    sets.push(
        json!({"name": "Tekken Force pickups", "source": "records 51-54 (stage VRAM)", "ui": true, "images": imgs}),
    );
    let total: usize = sets.iter().map(|s| s["images"].as_array().map_or(0, Vec::len)).sum();
    let sets = Value::Array(sets);
    ctx.out.write(&join(out, "index.json"), pyjson::indent1(&sets).as_bytes())?;
    let cl = json!({"note": "CLUT rows by source and VRAM \"x,y\": 15-bit PS1 colours (bit 15 = semi-transparent, \
                             0 = transparent)", "cluts": cluts});
    ctx.out.write(&join(out, "cluts.json"), pyjson::to_string(&cl, pyjson::DEFAULT).as_bytes())?;
    ctx.log(&format!("total {total}"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lz_literals_and_a_back_reference() {
        // flags 0b1111 (sentinel + 3 literals) 'a' 'b' 'c'; then flags 0b10: a token, distance 3, length 3
        let src = [0x0F, b'a', b'b', b'c', 0x02, 0x18, 0x03, 0x00];
        assert_eq!(lz(&src, 0), Some((b"abcabc".to_vec(), 8)));
        // a distance before the start fails
        assert_eq!(lz(&[0x02, 0x08, 0x01], 0), None);
    }
}
