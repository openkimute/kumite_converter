//! export_stages.py: Tekken 3 (USA, SLUS-00402) stages (arenas) to glTF 2.0 binary (.glb).
//!
//! usage: t3_convert export_stages "Tekken_3_(USA)_(Track 1).bin" OUTDIR [stage numbers]
//!
//! Stages 0-14 are the arenas; 15-18 are the Tekken Force stages and 19 the Tekken Ball beach (texture
//! record only). Force stages get their scrolling backdrop as a textured wall (see FORCE_* below). Arena k uses:
//!   56 + k  geometry: header [?, ?, object count], then per object 7 words
//!           (vertex offset, vertex count, extra offset, polygon count, polygon offset, polygon count, 0);
//!           offsets are relative to byte 12. Vertices are 4 shorts (x, y, z, 0), y down, floor y = 0.
//!           Polygons are GPU-style packets: colour word (command in the top byte) followed by
//!           textured UV words and 4 vertex index bytes. 0x2C textured quad, 0x24 textured triangle
//!           (both 20 bytes), 0x28 flat quad (8 bytes).
//!   36 + k  container: part 0 = a chain of 4bpp TIM tiles (and their CLUTs) uploaded to VRAM, plus
//!           the 808-byte floor map: u32 128, then 10 x 10 entries (uv, tpage | flags, 0, clut) naming a
//!           64 x 64 texture tile (corner UVs: FLOOR_ROT / FLOOR_PERM).
//!           (Force records 51-54 and the Ball record 55 use the same container with more parts.)
//! The backdrop geometry (the "drum", 0x8006D014) is in units of 8 world units: the game draws it as a
//! world-oriented object 8 x its raw size, centred on the fighters' midpoint (x, z), with its base ring
//! STAGE_Y below the floor (EXE table 0x80025138, per stage), after the floor and without depth, its Y row
//! scaled 5/3 instead of the world's 7122/4096. It is exported ×8 as node "sky" (centred on the origin);
//! the scene's extras give drum_base_y (game units, y down) and the ratio of the Y scales. The floor is exported
//! as node "floor". The EXE's floor setup (0x80048640) uses 1800-unit tiles on a 10 x 10 map (Tekken Force: 2500
//! units, 10 x 3); world tile (i, j) shows map row (-j) mod H, column (i + W/2) mod W, so the map repeats.
//! Textures are resolved through an emulated VRAM (texture page + CLUT from the UV words), and the
//! regions polygons actually use are packed into one atlas. OUTDIR/index.json lists the files.

use crate::atlas::c15;
use crate::disc;
use crate::glb::{Glb, f32_bytes, f64_value, u32_bytes};
use crate::{pyjson, tim};
use anyhow::{Context, Result, bail};
use conv_io::{Ctx, DiscFile, StdDisc, StdFs, Vfs, join, print_line};
use image::{GenericImage, Rgba, RgbaImage};
use serde_json::{Map, Value, json};

pub const SCALE: f64 = 0.001;
/// copies of the floor map along each axis (odd: centred)
pub const FLOOR_REPEAT: i64 = 13;

/// A stage: name, texture record, backdrop record, floor tile size, map columns, map rows.
pub struct Stage {
    pub name: String,
    pub tex: usize,
    pub geo: Option<usize>,
    pub tile: i64,
    pub cols: usize,
    pub rows: usize,
}

pub fn stages() -> Vec<Stage> {
    // (an arena's name gets who lives there in `run`: gamedata_tables::arena_name)
    let arena = (0..15).map(|k| Stage {
        name: format!("Stage {}", k + 1),
        tex: 36 + k,
        geo: Some(56 + k),
        tile: 1800,
        cols: 10,
        rows: 10,
    });
    let force = (0..4).map(|k| Stage {
        name: format!("Tekken Force stage {}", k + 1),
        tex: 51 + k,
        geo: None,
        tile: 2500,
        cols: 10,
        rows: 3,
    });
    let ball = Stage { name: "Tekken Ball beach".into(), tex: 55, geo: None, tile: 1800, cols: 10, rows: 10 };
    arena.chain(force).chain([ball]).collect()
}

/// Floor corner UVs (EXE table 0x8009E800, u | v << 8), row = bits 14-15 of the tpage word; the 0x600 bits
/// permute the corners (0x200 mirror u, 0x400 mirror v). Corners: (x0,z1), (x1,z1), (x0,z0), (x1,z0).
const FLOOR_ROT: [[u16; 4]; 4] = [
    [0x0000, 0x003f, 0x3f00, 0x3f3f],
    [0x3f00, 0x0000, 0x3f3f, 0x003f],
    [0x3f3f, 0x3f00, 0x003f, 0x0000],
    [0x003f, 0x3f3f, 0x0000, 0x3f00],
];
/// FLOOR_PERM[(tpage & 0x600) >> 9]
const FLOOR_PERM: [[usize; 4]; 4] = [[0, 1, 2, 3], [1, 0, 3, 2], [2, 3, 0, 1], [3, 2, 1, 0]];
// Tekken Force backdrop (record 7, 0x800B4660 / 0x800B4914): a scrolling strip of 64x64 tiles, scroll =
// camera x / 20, bottom pinned to the floor. In 3D: a wall at z = 3000, panels 1280 wide and 736 tall,
// panel n spanning x = 1280n - 3680.
const FORCE_WALL_Z: f64 = 3000.0;
const FORCE_PANEL_W: i64 = 1280;
const FORCE_PANEL_H: f64 = 736.0;
const FORCE_PANEL_X0: i64 = -3680;
// Force events (part 3; record 7 handler 0x800B1294): 20-byte records {s32 x, s16 type, s16 dx, s16 0, s16 z,
// s16 0, s16 a, s16 b, s16 0}; x is the camera x that triggers it. Enemy kind -> soldier costume per stage:
// record 7 table 0x800B06E8; stream lists (type 2): 6-byte {kind, variant, 0} entries at record 7 + a*6, 0x63 ends.
const FORCE_OVL_BASE: usize = 0x800B0548;
const FORCE_COSTUME_TABLE: usize = 0x800B06E8;
/// character 21, costumes 0-3 = models 259, 263, 267, 271
const FORCE_SOLDIERS: [&str; 4] = ["CROW", "FALCON", "HAWK", "OWL"];
/// spacing used to lay out a spawn stream statically
const FORCE_STREAM_STEP: i64 = 1500;
/// pickup sprite size in game units (estimate)
const CHICKEN_W: f64 = 500.0;
const CHICKEN_H: f64 = 1000.0;
const BALL_RECORD: usize = 55;
/// 0x8006D014: the drum's translation is >> 3
const DRUM_UNIT: f64 = 8.0;

/// PS1 (y down) -> glTF (y up), same as the characters: numpy's `C @ v` (a sum from +0.0, so never -0.0).
fn c(v: [f64; 3]) -> [f64; 3] {
    [v[0] + 0.0, 0.0 - v[1], 0.0 - v[2]]
}

fn le<const N: usize>(d: &[u8], o: usize) -> Result<[u8; N]> {
    d.get(o..o + N).and_then(|b| b.try_into().ok()).context("a stage record is too short")
}
fn u16at(d: &[u8], o: usize) -> Result<u16> {
    Ok(u16::from_le_bytes(le(d, o)?))
}
fn i16at(d: &[u8], o: usize) -> Result<i16> {
    Ok(i16::from_le_bytes(le(d, o)?))
}
fn u32at(d: &[u8], o: usize) -> Result<usize> {
    Ok(u32::from_le_bytes(le(d, o)?) as usize)
}

pub fn packet_size(cmd: u32) -> usize {
    let (quad, tex, gour) = (cmd & 8 != 0, cmd & 4 != 0, cmd & 16 != 0);
    let mut s = 4 * if gour { if quad { 4 } else { 3 } } else { 1 }; // colour word(s)
    if tex {
        s += 12; // uv0|clut, uv1|tpage, uv2, uv3/pad
    }
    s + 4 // vertex indices
}

/// A textured polygon's UVs (one per corner), CLUT and tpage.
type Tex = (Vec<(u16, u16)>, u16, u16);

pub struct Poly {
    rgb: [u8; 3],
    /// textured polygons only
    tex: Option<Tex>,
    idx: Vec<u8>,
}

type Obj = (Vec<[i16; 3]>, Vec<Poly>);

pub fn parse_stage(d: &[u8]) -> Result<Vec<Obj>> {
    let n = u32at(d, 8)?;
    let mut objs = Vec::new();
    for k in 0..n {
        let w = |i: usize| u32at(d, 12 + 28 * k + 4 * i);
        let (vo, nv, npoly, po) = (w(0)?, w(1)?, w(3)?, w(4)?);
        let v = (0..nv)
            .map(|i| {
                let o = 12 + vo + 8 * i;
                Ok([i16at(d, o)?, i16at(d, o + 2)?, i16at(d, o + 4)?])
            })
            .collect::<Result<Vec<_>>>()?;
        let mut p = Vec::new();
        let mut q = 12 + po;
        for _ in 0..npoly {
            let col = u32at(d, q)? as u32;
            let cmd = col >> 24;
            let nvp = if cmd & 8 != 0 { 4 } else { 3 };
            let mut o = q + if cmd & 16 != 0 && nvp == 4 {
                16
            } else if cmd & 16 != 0 {
                12
            } else {
                4
            };
            let mut tex = None;
            if cmd & 4 != 0 {
                let h = (0..6).map(|i| u16at(d, o + 2 * i)).collect::<Result<Vec<_>>>()?;
                let uv = [h[0], h[2], h[4], h[5]][..nvp].iter().map(|&u| (u & 255, u >> 8)).collect();
                tex = Some((uv, h[1], h[3]));
                o += 12;
            }
            let idx = d.get(o..o + nvp).context("a stage record is too short")?.to_vec();
            p.push(Poly { rgb: [col as u8, (col >> 8) as u8, (col >> 16) as u8], tex, idx });
            q += packet_size(cmd);
        }
        objs.push((v, p));
    }
    Ok(objs)
}

pub struct FloorTile {
    uv: u16,
    tpage: u16,
    clut: u16,
}

/// The container part of 808 bytes (u32 128 + 10 x 10 entries); only the first `rows` rows are used.
pub fn parse_floor(texrec: &[u8], cols: usize, rows: usize) -> Result<Vec<FloorTile>> {
    let n = u32at(texrec, 0)?;
    for i in 0..n {
        let (o, size) = (u32at(texrec, 4 + 8 * i)?, u32at(texrec, 8 + 8 * i)?);
        if size == 808 {
            // u32 128, 100 entries, 4 trailing bytes
            return (0..cols * rows)
                .map(|i| {
                    let e = o + 4 + 8 * i;
                    Ok(FloorTile { uv: u16at(texrec, e)?, tpage: u16at(texrec, e + 2)?, clut: u16at(texrec, e + 6)? })
                })
                .collect();
        }
    }
    Ok(Vec::new())
}

/// The 1024 x 512 16-bit VRAM, row-major.
type Vram = Vec<u16>;

/// Part 2 of a Force / Ball record: u16 cols, u16 rows, cols*rows tile codes, cols*rows CLUT words (row-major),
/// drawn as `tile`-texel tiles; `uv(code)` gives a tile's place in its page.
fn panorama(texrec: &[u8], vr: &Vram, tile: u32, uv: fn(u16) -> (u32, u32)) -> Result<Option<RgbaImage>> {
    if u32at(texrec, 0)? < 3 {
        return Ok(None);
    }
    let o = u32at(texrec, 4 + 8 * 2)?;
    let (pc, pr) = (u16at(texrec, o)? as u32, u16at(texrec, o + 2)? as u32);
    let mut img = RgbaImage::new(pc * tile, pr * tile);
    for k in 0..pc * pr {
        let code = u16at(texrec, o + 4 + 2 * k as usize)?;
        let clut = u16at(texrec, o + 4 + 2 * (pc * pr + k) as usize)?;
        let (r, cl) = (k / pc, k % pc);
        let tp = ((code >> 8) & 0x10) | ((code >> 8) & 0xF);
        let (u, v) = uv(code);
        let t = texels(vr, tp as u32, clut as u32, u, v, u + tile - 1, v + tile - 1)?;
        img.copy_from(&t, cl * tile, r * tile)?;
    }
    Ok(Some(img))
}

/// Tekken Force part 2: 64x64 tiles; code: tpage = ((code >> 8) & 0x10) | ((code >> 8) & 0xF), u = (code & 3) *
/// 64, v = ((code >> 2) & 3) * 64.
fn force_panorama(texrec: &[u8], vr: &Vram) -> Result<Option<RgbaImage>> {
    panorama(texrec, vr, 64, |code| ((code as u32 & 3) * 64, ((code as u32 >> 2) & 3) * 64))
}

/// Tekken Ball's sky (record 55 part 2, drawn by record 8 0x800B5A48 as a 2D strip): u16 cols (32), u16 rows (8),
/// then the tile codes and CLUT words; 32x32 tiles: tpage as above, u = (code & 7) * 32, v = ((code >> 3) & 7) *
/// 32. A 1024 x 256 strip.
fn ball_panorama(texrec: &[u8], vr: &Vram) -> Result<Option<RgbaImage>> {
    let mut img = panorama(texrec, vr, 32, |code| ((code as u32 & 7) * 32, ((code as u32 >> 3) & 7) * 32))?;
    for p in img.iter_mut().flat_map(|i| i.pixels_mut()) {
        p.0[3] = 255;
    }
    Ok(img)
}

/// Python's `a // b`.
fn floor_div(a: i64, b: i64) -> i64 {
    let q = a / b;
    if a % b != 0 && (a < 0) != (b < 0) { q - 1 } else { q }
}

/// Placements for a Force stage: enemies (single spawns and spawn streams), pickups and markers.
fn force_events(texrec: &[u8], ovl: &[u8], stage: i64) -> Result<Vec<Map<String, Value>>> {
    if u32at(texrec, 0)? < 4 {
        return Ok(Vec::new());
    }
    let (o, size) = (u32at(texrec, 4 + 8 * 3)?, u32at(texrec, 8 + 8 * 3)?);
    let enemy = |x: i64, z: i64, kind: i16, var: i16, how: &str| -> Result<Map<String, Value>> {
        let at = FORCE_COSTUME_TABLE - FORCE_OVL_BASE + 2 * (stage & 3) as usize + (kind & 1) as usize;
        let costume = *ovl.get(at).context("record 7 is too short")? as usize;
        let soldier = FORCE_SOLDIERS.get(costume).context("a Force soldier costume is out of range")?;
        Ok(obj(json!({"type": "enemy", "x": x, "z": z, "kind": kind, "variant": (var as i64).rem_euclid(5),
                           "soldier": soldier, "model": 259 + 4 * costume, "spawn": how})))
    };
    let mut out = Vec::new();
    for k in 0..size / 20 {
        let e = o + 20 * k;
        let x = i32::from_le_bytes(le(texrec, e)?) as i64;
        let h = (0..8).map(|i| i16at(texrec, e + 4 + 2 * i)).collect::<Result<Vec<_>>>()?;
        let (typ, dx, z, a, b) = (h[0], h[1] as i64, h[3] as i64, h[5], h[6]);
        let v = match typ {
            7 => break,
            1 => Value::Object(enemy(x + dx, z, a, b, "single")?),
            2 => {
                // stream: spawns at camera x + 5000 while the camera is before b
                let mut q = usize::try_from(a as i64 * 6).context("a Force spawn stream is out of range")?;
                let mut lst = Vec::new();
                while q + 6 <= ovl.len() && i16at(ovl, q)? != 0x63 {
                    lst.push((i16at(ovl, q)?, i16at(ovl, q + 2)?));
                    q += 6;
                }
                let count = (lst.len() as i64).min(1.max(floor_div(b as i64 - x, FORCE_STREAM_STEP))).max(0);
                for (i, &(kd, vr)) in lst.iter().take(count as usize).enumerate() {
                    out.push(enemy(x + 5000 + i as i64 * FORCE_STREAM_STEP, 0, kd, vr, "stream")?);
                }
                continue;
            }
            4 => json!({"type": "item", "item": "chicken", "x": x + dx, "z": z}),
            3 => {
                let what = if a >= 0 { "wait for kills" } else { "wait until clear" };
                json!({"type": "marker", "what": what, "x": x, "z": 0, "count": a})
            }
            0 => {
                let what = ["scroll", "camera frozen", "arena lock"].get(a as usize).filter(|_| a >= 0);
                json!({"type": "marker", "what": what.unwrap_or(&"camera"), "x": x, "z": 0})
            }
            6 => json!({"type": "marker", "what": "stage clear", "x": x, "z": 0}),
            _ => continue,
        };
        out.push(obj(v));
    }
    Ok(out)
}

/// The roast-chicken pickup: two stacked 64x64 tiles at VRAM (512,64) and (512,128) in every Force stage.
fn force_chicken(texrec: &[u8], vr: &Vram) -> Result<Option<RgbaImage>> {
    let ts = tim::walk_tims(texrec);
    // the last TIM at a place wins (a dict keyed by it)
    let tile = |x: u16, y: u16| ts.iter().rev().find(|t| t.clut.is_some() && (t.img.0, t.img.1) == (x, y));
    let (Some(a), Some(b)) = (tile(512, 64), tile(512, 128)) else { return Ok(None) };
    let rgba = |t: &tim::Tim| {
        let (x, y, w, h, _) = t.img;
        let (x, y, w, h) = (x as u32, y as u32, w as u32, h as u32);
        let cl = t.clut.map_or(0, |c| ((c.1 as u32) << 6) | (c.0 as u32 >> 4));
        let tp = (x / 64) | ((y / 256) << 4);
        texels(vr, tp, cl, (x % 64) * 4, y % 256, (x % 64) * 4 + w * 4 - 1, y % 256 + h - 1)
    };
    let (a, b) = (rgba(a)?, rgba(b)?);
    if a.width() != b.width() {
        bail!("the chicken's two tiles differ in width");
    }
    let mut img = RgbaImage::new(a.width(), a.height() + b.height());
    img.copy_from(&a, 0, 0)?;
    img.copy_from(&b, 0, a.height())?;
    Ok(Some(img))
}

pub fn load_vram(texrec: &[u8]) -> Result<Vram> {
    let mut vr = vec![0u16; 512 * 1024];
    for t in tim::walk_tims(texrec) {
        for (x, y, w, h, px) in [t.img].into_iter().chain(t.clut) {
            let (x, y, w, h) = (x as usize, y as usize, w as usize, h as usize);
            if x + w > 1024 || y + h > 512 || px.len() != 2 * w * h {
                bail!("a TIM lies outside VRAM or is short");
            }
            for (k, c) in px.as_chunks::<2>().0.iter().enumerate() {
                vr[(y + k / w) * 1024 + x + k % w] = u16::from_le_bytes(*c);
            }
        }
    }
    Ok(vr)
}

/// RGBA for the page rectangle [u0,u1] x [v0,v1] read through a CLUT, like the GPU.
pub fn texels(vr: &Vram, tpage: u32, clut: u32, u0: u32, v0: u32, u1: u32, v1: u32) -> Result<RgbaImage> {
    let (px, py, depth) = ((tpage as usize & 15) * 64, ((tpage as usize >> 4) & 1) * 256, (tpage >> 7) & 3);
    let (cx, cy) = ((clut as usize & 63) * 16, (clut as usize >> 6) & 511);
    let at = |x: usize, y: usize| {
        if x < 1024 && y < 512 { Ok(vr[y * 1024 + x]) } else { Err(anyhow::anyhow!("a texture reads outside VRAM")) }
    };
    if !(u0 <= u1 && u1 <= 255 && v0 <= v1 && v1 <= 255) {
        bail!("a texture rectangle [{u0}, {u1}] x [{v0}, {v1}] outside its page");
    }
    let mut img = RgbaImage::new(u1 - u0 + 1, v1 - v0 + 1);
    for v in v0..=v1 {
        for u in u0..=u1 {
            let (uu, vv) = (u as usize, py + v as usize);
            let c = match depth {
                0 => at(cx + ((at(px + uu / 4, vv)? >> ((uu % 4) * 4)) & 15) as usize, cy)?,
                1 => at(cx + ((at(px + uu / 2, vv)? >> ((uu % 2) * 8)) & 255) as usize, cy)?,
                _ => at(px + uu, vv)?,
            };
            img.put_pixel(u - u0, v - v0, Rgba(c15(c)));
        }
    }
    Ok(img)
}

/// (tpage, clut)
type Key = (u16, u16);
/// key -> (x, y, u0, v0)
type Place = (Key, [u32; 4]);

/// Pack every (tpage, clut, used UV rectangle) into one RGBA atlas. Returns atlas and key->(x, y, u0, v0).
pub fn build_atlas(vr: &Vram, objs: &[Obj], floor: &[FloorTile]) -> Result<(RgbaImage, Vec<Place>)> {
    let mut rects: Vec<(Key, [u32; 4])> = Vec::new(); // a dict: insertion order
    let mut add = |k: Key, lo: (u32, u32), hi: (u32, u32)| {
        let i = rects.iter().position(|r| r.0 == k).unwrap_or_else(|| {
            rects.push((k, [255, 255, 0, 0]));
            rects.len() - 1
        });
        let r = &mut rects[i].1;
        *r = [r[0].min(lo.0), r[1].min(lo.1), r[2].max(hi.0), r[3].max(hi.1)];
    };
    for t in floor {
        let (u, v) = ((t.uv & 255) as u32, (t.uv >> 8) as u32);
        add((t.tpage & 0x1ff, t.clut), (u, v), (255.min(u + 63), 255.min(v + 63)));
    }
    for p in objs.iter().flat_map(|o| &o.1) {
        let Some((uv, clut, tpage)) = &p.tex else { continue };
        let (us, vs) = (uv.iter().map(|e| e.0 as u32), uv.iter().map(|e| e.1 as u32));
        let lo = (us.clone().min().unwrap_or(255), vs.clone().min().unwrap_or(255));
        add((*tpage, *clut), lo, (us.max().unwrap_or(0), vs.max().unwrap_or(0)));
    }
    let tiles = rects
        .iter()
        .map(|(k, r)| texels(vr, k.0 as u32, k.1 as u32, r[0], r[1], r[2], r[3]))
        .collect::<Result<Vec<_>>>()?;
    let w = 1024;
    let (mut x, mut y, mut row) = (0, 0, 0);
    let mut place = vec![(0, 0); tiles.len()];
    let mut order: Vec<usize> = (0..tiles.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(tiles[i].height()));
    for &i in &order {
        let (tw, th) = tiles[i].dimensions();
        if x + tw + 2 > w {
            (x, y, row) = (0, y + row + 2, 0);
        }
        place[i] = (x + 1, y + 1);
        x += tw + 2;
        row = row.max(th);
    }
    let h = (y + row + 4).max(4).next_power_of_two();
    let mut a = RgbaImage::new(w, h);
    for yy in h - 2..h {
        for xx in w - 2..w {
            a.put_pixel(xx, yy, Rgba([255; 4])); // white texels for flat polygons
        }
    }
    let mut out = Vec::new();
    for &i in &order {
        let (t, (ax, ay)) = (&tiles[i], place[i]);
        let (tw, th) = t.dimensions();
        if ax + tw + 1 > w {
            bail!("an atlas tile is wider than the atlas");
        }
        a.copy_from(t, ax, ay)?;
        // 1-texel border copied from the edges so nearest/linear filtering never bleeds
        for u in 0..tw {
            a.put_pixel(ax + u, ay - 1, *t.get_pixel(u, 0));
            a.put_pixel(ax + u, ay + th, *t.get_pixel(u, th - 1));
        }
        for v in ay - 1..ay + th + 1 {
            a.put_pixel(ax - 1, v, *a.get_pixel(ax, v));
            a.put_pixel(ax + tw, v, *a.get_pixel(ax + tw - 1, v));
        }
        out.push((rects[i].0, [ax, ay, rects[i].1[0], rects[i].1[1]]));
    }
    Ok((a, out))
}

pub fn png(img: &RgbaImage) -> Result<Vec<u8>> {
    let mut b = std::io::Cursor::new(Vec::new());
    img.write_to(&mut b, image::ImageFormat::Png)?;
    Ok(b.into_inner())
}

/// A JSON object from `json!`.
fn obj(v: Value) -> Map<String, Value> {
    v.as_object().cloned().unwrap_or_default()
}

#[allow(clippy::too_many_arguments)]
pub fn convert(
    geo: Option<&[u8]>,
    texrec: &[u8],
    vfs: &dyn Vfs,
    out_path: &str,
    name: &str,
    tile: i64,
    map_w: usize,
    map_h: usize,
    ovl: &[u8],
    stage: i64,
    drum_base_y: u16,
    ball: bool,
) -> Result<(usize, usize, (u32, u32))> {
    let objs = match geo {
        Some(g) if !g.is_empty() => parse_stage(g)?,
        _ => Vec::new(),
    };
    let floor = parse_floor(texrec, map_w, map_h)?;
    let vr = load_vram(texrec)?;
    let (a, place) = build_atlas(&vr, &objs, &floor)?;
    let (wf, hf) = (a.width() as f64, a.height() as f64);
    let find = |k: Key| place.iter().find(|p| p.0 == k).map(|p| p.1).context("an atlas key is missing");
    let (mut pos, mut uvs, mut cols, mut idx) = (Vec::new(), Vec::new(), Vec::new(), Vec::<u32>::new());
    let mut push = |p: [f64; 3], t: (f64, f64), c: [f64; 4]| {
        pos.push(p.map(|v| v as f32));
        uvs.push([t.0 as f32, t.1 as f32]);
        cols.push(c.map(|v| v as f32));
    };
    let mut n = 0u32; // len(pos)
    for (verts, polys) in &objs {
        for p in polys {
            if p.idx.iter().any(|&i| i as usize >= verts.len()) {
                continue;
            }
            let base = n;
            let (tuv, rgb): (Vec<(f64, f64)>, [f64; 3]) = match &p.tex {
                Some((uv, clut, tpage)) => {
                    let [ax, ay, u0, v0] = find((*tpage, *clut))?.map(|v| v as f64);
                    let tuv =
                        uv.iter().map(|&(u, v)| ((ax + u as f64 - u0 + 0.5) / wf, (ay + v as f64 - v0 + 0.5) / hf));
                    // texture * colour / 128
                    (tuv.collect(), p.rgb.map(|c| 1.0f64.min(c as f64 / 128.0)))
                }
                None => (vec![((wf - 1.0) / wf, (hf - 1.0) / hf); p.idx.len()], p.rgb.map(|c| c as f64 / 255.0)),
            };
            for (&i, &t) in p.idx.iter().zip(&tuv) {
                let v = verts[i as usize].map(|c| c as f64);
                push(c(v).map(|c| SCALE * DRUM_UNIT * c), t, [rgb[0], rgb[1], rgb[2], 1.0]);
                n += 1;
            }
            // PS1 winding -> glTF
            let tris: &[[u32; 3]] = if p.idx.len() == 4 { &[[0, 2, 1], [1, 2, 3]] } else { &[[0, 2, 1]] };
            idx.extend(tris.iter().flatten().map(|t| base + t));
        }
    }
    let n_sky = idx.len();
    // floor: one copy of the map (tiles i in [-W/2, W/2), j in (-H, 0]) at y = 0, instanced by nodes below
    let white = [1.0; 4];
    for (k, t) in floor.iter().enumerate() {
        let (row, col) = ((k / map_w) as i64, (k % map_w) as i64);
        let (i, jz) = (col - (map_w / 2) as i64, -row);
        let [ax, ay, u0, v0] = find((t.tpage & 0x1ff, t.clut))?.map(|v| v as i64);
        let (rot, perm) = (FLOOR_ROT[(t.tpage >> 14) as usize], FLOOR_PERM[((t.tpage & 0x600) >> 9) as usize]);
        let (bu, bv) = ((t.uv & 255) as i64, (t.uv >> 8) as i64);
        let base = n;
        for (q, (dx, dz)) in perm.iter().zip([(0, 1), (1, 1), (0, 0), (1, 0)]) {
            let (cu, cv) = (bu + (rot[*q] & 0xff) as i64 - u0, bv + (rot[*q] >> 8) as i64 - v0);
            let p = c([((i + dx) * tile) as f64, 0.0, ((jz + dz) * tile) as f64]);
            push(p.map(|v| SCALE * v), (((ax + cu) as f64 + 0.5) / wf, ((ay + cv) as f64 + 0.5) / hf), white);
            n += 1;
        }
        idx.extend([base, base + 1, base + 2, base + 1, base + 3, base + 2]);
    }
    let n_floor = idx.len();
    // Tekken Force backdrop wall (second texture: the panorama strip)
    let mut pano = if map_h == 3 { force_panorama(texrec, &vr)? } else { None };
    let strip = if ball { ball_panorama(texrec, &vr)? } else { None };
    let quad = [0, 2, 1, 1, 2, 3];
    if let Some(s) = &strip {
        // a unit drum (radius 1, y 0..1) the strip wraps twice around (2048 px per turn); the port centres it on
        // the camera and scales it each frame as 0x800B5A48 places the strip
        pano = Some(s.clone());
        for k in 0..64 {
            let base = n;
            let tau = 2.0 * std::f64::consts::PI;
            let (a0, a1) = (tau * k as f64 / 64.0, tau * (k + 1) as f64 / 64.0);
            // above the strip (y > 1) the texture's top row continues (v clamps): the sky fill over the strip
            for (a, uu) in [(a0, 2.0 * k as f64 / 64.0), (a1, 2.0 * (k + 1) as f64 / 64.0)] {
                for y in [0.0, 4.0] {
                    push([libm::sin(a), y, libm::cos(a)], (uu, 1.0 - y), white);
                    n += 1;
                }
            }
            idx.extend(quad.map(|q| base + q));
        }
    } else if let Some(pa) = &pano {
        let (pc, pr) = ((pa.width() / 64) as i64, (pa.height() / 64) as f64);
        if pc == 0 {
            bail!("the Force panorama has no columns");
        }
        for k in -27..52i64 {
            let (x0, cl, base) = (FORCE_PANEL_X0 + k * FORCE_PANEL_W, k.rem_euclid(pc), n);
            let uv = [(cl, 1), (cl + 1, 1), (cl, 0), (cl + 1, 0)];
            for ((dx, dy), (u, v)) in [(0, 0.0), (1, 0.0), (0, 1.0), (1, 1.0)].into_iter().zip(uv) {
                let p = c([(x0 + dx * FORCE_PANEL_W) as f64, -dy * pr * FORCE_PANEL_H, FORCE_WALL_Z]);
                push(p.map(|v| SCALE * v), (u as f64 / pc as f64, v as f64), white);
                n += 1;
            }
            idx.extend(quad.map(|q| base + q));
        }
    }
    let n_wall = idx.len();
    let events = if pano.is_some() && !ovl.is_empty() { force_events(texrec, ovl, stage)? } else { Vec::new() };
    let chicken = if events.is_empty() { None } else { force_chicken(texrec, &vr)? };
    if chicken.is_some() {
        // one upright quad at the origin; pickup nodes instance it
        let base = n;
        let uv = [(0.0, 1.0), (1.0, 1.0), (0.0, 0.0), (1.0, 0.0)];
        for ((dx, dy), t) in [(-1.0, 0.0), (1.0, 0.0), (-1.0, 1.0), (1.0, 1.0)].into_iter().zip(uv) {
            push(c([dx * CHICKEN_W / 2.0, -dy * CHICKEN_H, 0.0]).map(|v| SCALE * v), t, white);
            n += 1;
        }
        idx.extend(quad.map(|q| base + q));
    }
    if idx.iter().any(|&i| i >= n) {
        bail!("a polygon names a corner it does not have");
    }
    let nrm = normals(&pos, &idx);
    let mut g = Glb::default();
    let (mut lo, mut hi) = (vec![f64::INFINITY; 3], vec![f64::NEG_INFINITY; 3]);
    for p in &pos {
        for k in 0..3 {
            (lo[k], hi[k]) = (lo[k].min(p[k] as f64), hi[k].max(p[k] as f64));
        }
    }
    let flat = |v: &[[f32; 3]]| f32_bytes(v.as_flattened());
    let mut attrs = Map::new();
    attrs.insert(
        "POSITION".into(),
        json!(g.acc(&flat(&pos), pos.len(), 5126, "VEC3", Some(34962), Some((lo, hi)), false)),
    );
    attrs.insert("NORMAL".into(), json!(g.acc(&flat(&nrm), nrm.len(), 5126, "VEC3", Some(34962), None, false)));
    let tb = f32_bytes(uvs.as_flattened());
    attrs.insert("TEXCOORD_0".into(), json!(g.acc(&tb, uvs.len(), 5126, "VEC2", Some(34962), None, false)));
    let cb = f32_bytes(cols.as_flattened());
    attrs.insert("COLOR_0".into(), json!(g.acc(&cb, cols.len(), 5126, "VEC4", Some(34962), None, false)));
    let nearest = json!({"magFilter": 9728, "minFilter": 9728, "wrapS": 33071, "wrapT": 33071});
    let mut images = vec![json!({"bufferView": g.view(&png(&a)?, None), "mimeType": "image/png", "name": "atlas"})];
    let mut samplers = vec![nearest];
    let mut textures = vec![json!({"source": 0, "sampler": 0})];
    if let Some(p) = &pano {
        images.push(json!({"bufferView": g.view(&png(p)?, None), "mimeType": "image/png", "name": "panorama"}));
        samplers.push(json!({"magFilter": 9728, "minFilter": 9728, "wrapS": 10497, "wrapT": 33071}));
        textures.push(json!({"source": 1, "sampler": 1}));
    }
    if let Some(ch) = &chicken {
        images.push(json!({"bufferView": g.view(&png(ch)?, None), "mimeType": "image/png", "name": "chicken"}));
        textures.push(json!({"source": images.len() - 1, "sampler": 0}));
    }
    let materials: Vec<Value> = ["stage", "panorama", "chicken"][..textures.len()]
        .iter()
        .enumerate()
        .map(|(t, nm)| {
            json!({"name": nm, "doubleSided": true, "alphaMode": "MASK", "alphaCutoff": 0.5,
                   "pbrMetallicRoughness": {"baseColorTexture": {"index": t}, "metallicFactor": 0, "roughnessFactor": 1}})
        })
        .collect();
    g.j.insert("images".into(), images.into());
    g.j.insert("samplers".into(), samplers.into());
    g.j.insert("textures".into(), textures.into());
    g.j.insert("materials".into(), materials.into());
    let (mut meshes, mut nodes, mut roots) = (Vec::new(), Vec::<Value>::new(), Vec::new());
    let mut chicken_mesh = None;
    let wall_name = if strip.is_some() { "panorama_strip" } else { "backdrop_wall" };
    let parts = [
        ("sky", &idx[..n_sky], 0),
        ("floor", &idx[n_sky..n_floor], 0),
        (wall_name, &idx[n_floor..n_wall], 1),
        ("chicken", &idx[n_wall..], 2),
    ];
    for (part, ix, mat) in parts {
        if ix.is_empty() {
            continue;
        }
        let ia = g.acc(&u32_bytes(ix), ix.len(), 5125, "SCALAR", Some(34963), None, false);
        meshes.push(json!({"name": part, "primitives": [{"attributes": attrs, "indices": ia, "material": mat}]}));
        let m = meshes.len() - 1;
        if part == "chicken" {
            chicken_mesh = Some(m); // instanced by the pickup nodes below
            continue;
        }
        if part != "floor" {
            roots.push(nodes.len());
            nodes.push(json!({"name": part, "mesh": m}));
        } else {
            // the floor map repeats; each copy is a node sharing the one mesh
            let k = FLOOR_REPEAT / 2;
            let (sx, sz) = ((map_w as i64 * tile) as f64 * SCALE, (map_h as i64 * tile) as f64 * SCALE);
            let first = nodes.len() + 1;
            // Force: the corridor (x about -15000..48000, z -5000..2500), plus one copy behind the wall
            let (xs, zs) = if pano.is_some() { (-1..3, 0..2) } else { (-k..k + 1, -k..k + 1) };
            let copies: Vec<Value> = zs
                .flat_map(|z| xs.clone().map(move |x| (x, z)))
                .map(|(x, z)| {
                    json!({"name": format!("floor_{x}_{z}"), "mesh": m,
                           "translation": [x as f64 * sx, 0, (-z) as f64 * sz]})
                })
                .collect();
            roots.push(nodes.len());
            nodes.push(json!({"name": "floor", "children": (first..first + copies.len()).collect::<Vec<_>>()}));
            nodes.extend(copies);
        }
    }
    if !events.is_empty() {
        // Force placements: enemies (extras say which soldier), pickups, markers
        let mut kids = Vec::new();
        for (k, e) in events.into_iter().enumerate() {
            let (typ, x, z) =
                (e["type"].as_str().unwrap_or(""), e["x"].as_i64().unwrap_or(0), e["z"].as_i64().unwrap_or(0));
            let soldier =
                if typ == "enemy" { format!("_{}", e["soldier"].as_str().unwrap_or("")) } else { String::new() };
            let mut nd = obj(json!({"name": format!("{typ}_{k:03}{soldier}"),
                                    "translation": [x as f64 * SCALE, 0, (-z) as f64 * SCALE]}));
            let item = typ == "item";
            nd.insert("extras".into(), Value::Object(e));
            if item {
                nd.insert("mesh".into(), chicken_mesh.context("a Force pickup has no chicken picture")?.into());
            }
            kids.push(nodes.len());
            nodes.push(Value::Object(nd));
        }
        roots.push(nodes.len());
        nodes.push(json!({"name": "force_events", "children": kids}));
    }
    g.j.insert("meshes".into(), meshes.into());
    g.j.insert("nodes".into(), nodes.into());
    let mut extras = Map::new();
    if let Some(s) = &strip {
        extras.insert(
            "panorama_strip".into(),
            json!({"routine": "0x800B5A48 (record 8)", "width": s.width(),
            "height": s.height(), "turn_px": 2048, "bottom_ahead": 6656, "bottom_y": 256, "lateral_divisor": 0x60,
            "note": "drum centred on the camera; its bottom at the projection of a point bottom_ahead units in front \
                     of the camera at y = bottom_y (game units, y down); height px tall"}),
        );
    }
    if pano.is_none() || strip.is_some() {
        // the floor is laid around the fighters' midpoint (0x8004994C): its copies may shift by whole maps
        let period = [(map_w as i64 * tile) as f64 * SCALE, (map_h as i64 * tile) as f64 * SCALE];
        extras.insert("floor_period".into(), json!(period));
    }
    if !objs.is_empty() {
        extras.insert("drum_base_y".into(), json!(drum_base_y));
        extras.insert("drum_unit".into(), json!(DRUM_UNIT as i64));
        extras.insert("drum_y_scale".into(), f64_value((5.0 / 3.0) / (7122.0 / 4096.0)));
        extras.insert("drum_before_floor".into(), json!(stage + 15 == 11));
        extras.insert(
            "note".into(),
            json!(
                "centre the sky on the fighters' midpoint (x, z) at y = -drum_base_y / 1000 m; \
            draw it after the floor without depth (before it for stage 11); scale view-space y by drum_y_scale"
            ),
        );
    }
    // the full-screen fill behind everything (0x800482B0, linked at ordering-table entry 1023): sky blue on stage 11
    // (King's: its drum's and floor's transparent texels show it), black elsewhere
    let bg = if stage + 15 == 11 { [0x48, 0x68, 0xC8] } else { [0, 0, 0] };
    extras.insert("background_rgb".into(), json!(bg));
    g.j.insert("scenes".into(), json!([{"name": name, "nodes": roots, "extras": extras}]));
    g.j.insert("scene".into(), json!(0));
    vfs.write(out_path, &g.to_bytes())?;
    Ok((objs.len(), idx.len() / 3, (a.height(), a.width())))
}

/// Area-weighted vertex normals as numpy computes them in float32: face cross products added per corner (all first
/// corners, then all second, then all third), then normalised.
pub fn normals(pos: &[[f32; 3]], idx: &[u32]) -> Vec<[f32; 3]> {
    let mut nrm = vec![[0f32; 3]; pos.len()];
    let tris = idx.as_chunks::<3>().0;
    let fns: Vec<[f32; 3]> = tris
        .iter()
        .map(|t| {
            let [p0, p1, p2] = t.map(|i| pos[i as usize]);
            let a = [p1[0] - p0[0], p1[1] - p0[1], p1[2] - p0[2]];
            let b = [p2[0] - p0[0], p2[1] - p0[1], p2[2] - p0[2]];
            [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
        })
        .collect();
    for cn in 0..3 {
        for (t, f) in tris.iter().zip(&fns) {
            let v = &mut nrm[t[cn] as usize];
            for k in 0..3 {
                v[k] += f[k];
            }
        }
    }
    for v in &mut nrm {
        let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-9);
        *v = v.map(|c| c / len);
    }
    nrm
}

pub fn main(args: &[String]) -> Result<()> {
    let [src, out, rest @ ..] = args else { bail!("usage: export_stages BIN OUTDIR [stage numbers]") };
    let n = stages().len();
    let only: Option<Vec<usize>> = match rest.first() {
        Some(s) => Some(
            s.split(',')
                .map(|v| v.trim().parse().ok().filter(|&k: &usize| k < n))
                .collect::<Option<_>>()
                .with_context(|| format!("stage numbers are 0..{}: {s}", n - 1))?,
        ),
        None => None,
    };
    run(&Ctx { out: &StdFs(out.into()), log: &print_line, jobs: 1 }, &StdDisc(src.into()), "", only)
}

/// Writes the stages (`only` these numbers, or all) and index.json under `out`.
pub fn run(ctx: &Ctx, disc: &dyn DiscFile, out: &str, only: Option<Vec<usize>>) -> Result<()> {
    let all = stages();
    let only = only.unwrap_or_else(|| (0..all.len()).collect());
    let game = disc::Game::load(disc)?;
    let exe = crate::gamedata_core::Exe::new(&game.exe);
    let rec = |i: usize| game.rec.get(i).map(Vec::as_slice).context("a stage record is past the BNS table");
    let mut index = Vec::new();
    for k in only {
        let st = &all[k];
        let name = if k < 15 { crate::gamedata_tables::arena_name(&exe, k as i64)? } else { st.name.clone() };
        let fname = format!("stage_{k:02}.glb");
        // STAGE_Y
        let base_y = if k < 15 { u16at(game.at(0x80025138 + 4 * k as u32), 0)? } else { 0 };
        let geo = st.geo.map(rec).transpose()?;
        let (no, nt, (ah, aw)) = convert(
            geo,
            rec(st.tex)?,
            ctx.out,
            &join(out, &fname),
            &name,
            st.tile,
            st.cols,
            st.rows,
            rec(7)?,
            k as i64 - 15,
            base_y,
            st.tex == BALL_RECORD,
        )
        .with_context(|| fname.clone())?;
        index.push(json!({"id": k, "name": name, "file": fname, "backdrop": st.geo.is_some()}));
        ctx.log(&format!("{fname} objects {no} tris {nt} atlas {aw}x{ah}"));
    }
    ctx.out.write(&join(out, "index.json"), pyjson::indent1(&Value::Array(index)).as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_semantics() {
        assert_eq!(packet_size(0x2C), 20);
        assert_eq!(packet_size(0x24), 20);
        assert_eq!(packet_size(0x28), 8);
        assert_eq!(packet_size(0x3C), 32);
        assert_eq!(floor_div(-1, 1500), -1);
        assert_eq!(floor_div(3000, 1500), 2);
        assert_eq!(c([-5.0, 0.0, 3.0]).map(f64::to_bits), [(-5.0f64).to_bits(), 0, (-3.0f64).to_bits()]);
    }
}
