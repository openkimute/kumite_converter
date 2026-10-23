//! Tekken 2 (USA v1.1, SLUS-00213) arenas to glTF 2.0 binary (.glb), in the port's stage layout.
//!
//! usage: t2_convert export_stages "Tekken 2 (USA) (v1.1) (Track 1).bin" OUTDIR [stage numbers]
//!
//! Stage k (0..22) is loaded from the block of the character whose home it is (character table 0x8005C504, byte
//! +6); its records are block +0 (TIMs: the panorama cells and the floor tiles, at their VRAM places) and +1, the
//! description (hrfld.c / hrbg.c; relocated by 0x80082074, kept at 0x800D2E18): u8 flag, 9 section offsets.
//!
//!   section 0  floor: u32 64 (tile texels), 10 x 10 x {u32 u | v << 8 | tpage << 16, u32 clut << 16}, s16 mode,
//!              s16 shade, s32 extra. The tpage's bits 14-15 turn the tile (corner UVs built by 0x80082790, as
//!              Tekken 3's 0x8009E800). Mode 0 (0x80082E60): 2500-unit tiles, 10 x 10 around the fighters' midpoint
//!              snapped to whole tiles (floor((m + 1250) / 2500)); world tile (i, j) shows map column (i + 5) mod 10,
//!              row (4 - j) mod 10; shade != 0 tints each vertex 128 + T[min(0x3FF, (sz - 10000) * 6710 >> 16)] *
//!              shade / 128 (T: 1024 bytes at 0x80056564, sz the view depth). Modes 1 and 2 (0x8008364C): 2000-unit
//!              tiles; vertex colour min(254, 0x8000000 / d1^2 + 0x8000000 / d2^2) (d: distance to each fighter),
//!              or min(192, 0x7A120000 / d^2) from their midpoint.
//!   section 1  panorama cells {u8 u, u8 v, u16 clut, u16 tpage}
//!   section 2  the rows' cell maps (s8 cell, < 0 none; 6144 / cell width entries a row)
//!   section 7  rows of 0x2C: {s32 distance (-1: none), s32 scroll, s16 x, s16 y, s16 w, s16 h, s32 map entries,
//!              s32 first cell, s32 map offset, 4 x s32 packet buffers}; every row has distance -1 and x 0
//!   section 8  {s16 rows, s16, u32, s16 scroll, s16 below, s16 sky_y, s16 sky_h, rgb sky, rgb sky2, s16 ground_y,
//!              rgb ground, s16 gy, s16 gh, rgb g1, rgb g2, s16 effect}
//!
//! The panorama (0x80084560) is drawn in screen space (512 x 240, H = 955, y scaled 5/8): row k's cells are 64 x 32
//! sprites, the column at the screen's left edge being (row x + scroll - 1.5 yaw) mod 6144 (yaw: 4096 a turn, larger
//! = turned left), the row's top at s5 + y - h, s5 = 120 - 597 tan(pitch) + below (the true horizon plus `below`).
//! 1.5 px per yaw unit is 978 px a radian, about H: so it is exported as a drum around the camera, 6144 px a turn
//! and 1 px = 1/597 high at unit radius, its bottom `below` px under the horizon. The flat fills drawn behind it
//! (sky colour above s5 + sky_y, a gradient to sky2 over sky_h; ground colour below s5 + ground_y, a gradient g1 to
//! g2 at gy over gh) are bands of the same drum.

use crate::export_characters::{self as t2, Disc, Vram, f32s, load_json, off, page_texels, title, write_json};
use crate::mips::{Cpu, no_hook};
use crate::np::{self, mv3};
use crate::ps1::{C, Glb, Rgba, SCALE, f64_value, floats, i16_, i32_, py_slice, u16_, u32_, walk_tims};
use crate::{pil, pyjson};
use anyhow::{Context, Result, bail};
use conv_io::{Ctx, DiscFile, StdDisc, StdFs, Vfs, join};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::f64::consts::PI;

const FLOOR_ROT: [[u32; 4]; 4] = [
    [0x0000, 0x003f, 0x3f00, 0x3f3f],
    [0x3f00, 0x0000, 0x3f3f, 0x003f],
    [0x3f3f, 0x3f00, 0x003f, 0x0000],
    [0x003f, 0x3f3f, 0x0000, 0x3f00],
];
const TURN_PX: f64 = 6144.0; // panorama pixels a turn (1.5 px per 1/4096 turn)
const FOCAL_Y: f64 = 955.0 * 5.0 / 8.0; // screen lines per unit of tan (H = 0x3BB, y scaled 5/8)
const FADE_TABLE: u32 = 0x80056564;
const LIGHTS: u32 = 0x800A1030; // per stage (0x800D2CE8), 24 bytes: model colour, shadow colour, shadow angles, light
// angles, light colour, ambient << 4 (0x8006EB38, 0x8006E980, 0x80071338)
const ROTMATRIX: u32 = 0x8007BEB0;

fn px_rad() -> f64 {
    TURN_PX / (2.0 * PI)
}

/// Floor tile size by mode (0x80082E60 / 0x8008364C).
fn tile_of(mode: i16) -> i64 {
    if matches!(mode, 1 | 2) { 2000 } else { 2500 }
}

/// The stage's character lighting; the shadow direction v = RotMatrix(0, hi, lo) column 0 (0x8006F26C, run on the
/// interpreter), its matrix x' = x - y vx / vy, y' = 0, z' = z - y vz / vy (game axes, 0x800D659C).
pub(crate) fn lighting(disc: &Disc, stage: usize) -> Result<Value> {
    let f = py_slice(&disc.exe, off(LIGHTS + 24 * stage as u32), usize::MAX);
    if f.len() < 22 {
        bail!("the EXE is short of the stage lighting table");
    }
    let h = |o: usize| i16_(f, o).unwrap_or(0) as i64;
    let mut cpu = Cpu::new();
    cpu.map(t2::EXE_LOAD, py_slice(&disc.exe, 0x800, usize::MAX));
    let (sl, sh) = (h(8), h(10));
    let arg: Vec<u8> = [0, sh, sl, 0].iter().flat_map(|v| (*v as i16).to_le_bytes()).collect();
    cpu.map(0x80190000, &arg);
    cpu.call(ROTMATRIX, &[0x80190000, 0x80190100], &mut no_hook)?;
    let m: Vec<i64> = (0..9).map(|i| cpu.read(0x80190100 + 2 * i, 2) as u16 as i16 as i64).collect();
    let (vx, vy, vz) = (m[0], m[3], m[6]);
    if vy == 0 {
        bail!("stage {stage}'s shadow direction is level");
    }
    // C division (truncates)
    let (a, b) = ((((vx << 12) as f64) / vy as f64).trunc(), (((vz << 12) as f64) / vy as f64).trunc());
    Ok(json!({"model_rgb": f[0..3], "shadow_rgb": f[4..7], "shadow_angles": [sh, sl], "shadow_dir": [vx, vy, vz],
              "shadow": [a / 4096.0, b / 4096.0], "light_angles": [h(14), h(12)], "light_rgb": f[16..19],
              "ambient": u16_(f, 20)? >> 4, "unit": 128,
              "note": "shadow: game axes (y down) x -= y * shadow[0], z -= y * shadow[1]; glTF x += y * s0, z -= y * s1"}))
}

pub fn vram(rec0: &[u8]) -> Result<Vram> {
    let mut v = Vram::new();
    for t in walk_tims(rec0) {
        for b in [&t.img, &t.clut].into_iter().flatten() {
            v.blit(b)?;
        }
    }
    Ok(v)
}

/// A texture page through a CLUT, from one VRAM (pictures and palettes together), cached.
pub struct Pages {
    v: Vram,
    cache: HashMap<(u32, u32), Rgba>,
}

impl Pages {
    pub fn new(v: Vram) -> Pages {
        Pages { v, cache: HashMap::new() }
    }
    pub fn get(&mut self, tpage: u32, clut: u32) -> &Rgba {
        let k = (tpage & 0x1ff, clut);
        let v = &self.v;
        self.cache.entry(k).or_insert_with(|| page_texels(v, v, k.0, clut))
    }
}

fn rgb(b: &[u8], o: usize) -> Result<Vec<f64>> {
    let c = b.get(o..o + 3).context("the background section is short")?;
    Ok(c.iter().map(|v| *v as f64 / 255.0).collect())
}

pub struct Row {
    pub x: i16,
    pub y: i16,
    pub w: i16,
    pub h: i16,
    /// cell -> (u, v, clut, tpage)
    pub cells: HashMap<i8, (u8, u8, u16, u16)>,
    pub map: Vec<i8>,
}

pub struct Info {
    pub flag: u8,
    pub texels: u32,
    pub floor: Vec<(u32, u32)>,
    pub mode: i16,
    pub shade: i16,
    pub extra: i32,
    pub rows: Vec<Row>,
    /// section 8, in the order the scene extras list it
    pub bg: Map<String, Value>,
}

pub fn describe(desc: &[u8]) -> Result<Info> {
    let sec = (0..9).map(|i| Ok(u32_(desc, 4 + 4 * i)? as usize)).collect::<Result<Vec<_>>>()?;
    let s0 = sec[0];
    let texels = u32_(desc, s0)?;
    let floor =
        (0..100).map(|n| Ok((u32_(desc, s0 + 4 + 8 * n)?, u32_(desc, s0 + 8 + 8 * n)?))).collect::<Result<_>>()?;
    let (mode, shade, extra) = (i16_(desc, s0 + 804)?, i16_(desc, s0 + 806)?, i32_(desc, s0 + 808)?);
    let s8 = py_slice(desc, sec[8], sec[8] + 44);
    let nrows = i16_(s8, 0)?;
    let mut rows = Vec::new();
    for k in 0..nrows.max(0) as usize {
        let o = sec[7] + 0x2C * k;
        let h16 = |a: usize| i16_(desc, o + a);
        let (x, y, w, h) = (h16(8)?, h16(10)?, h16(12)?, h16(14)?);
        let (entries, cell0, mapoff) = (i32_(desc, o + 16)?, i32_(desc, o + 20)?, i32_(desc, o + 24)?);
        let at = (sec[2] as i64 + mapoff as i64) as usize;
        let map: Vec<i8> = (0..entries.max(0) as usize)
            .map(|i| desc.get(at + i).map(|b| *b as i8).context("a row's cell map is short"))
            .collect::<Result<_>>()?;
        let mut cells = HashMap::new();
        for &c in map.iter().filter(|c| **c >= 0) {
            let a = (sec[1] as i64 + 6 * (cell0 as i64 + c as i64)) as usize;
            let b = desc.get(a..a + 6).context("a panorama cell is short")?;
            cells.insert(c, (b[0], b[1], u16::from_le_bytes([b[2], b[3]]), u16::from_le_bytes([b[4], b[5]])));
        }
        rows.push(Row { x, y, w, h, cells, map });
    }
    let h16 = |o: usize| i16_(s8, o);
    let mut bg = Map::new();
    bg.insert("rows".into(), json!(nrows));
    for (k, o) in [("scroll", 8), ("below", 0xA), ("sky_y", 0xC), ("sky_h", 0xE)] {
        bg.insert(k.into(), json!(h16(o)?));
    }
    bg.insert("sky".into(), floats(&rgb(s8, 0x10)?));
    bg.insert("sky2".into(), floats(&rgb(s8, 0x14)?));
    bg.insert("ground_y".into(), json!(h16(0x18)?));
    bg.insert("ground".into(), floats(&rgb(s8, 0x1A)?));
    bg.insert("gy".into(), json!(h16(0x1E)?));
    bg.insert("gh".into(), json!(h16(0x20)?));
    bg.insert("g1".into(), floats(&rgb(s8, 0x22)?));
    bg.insert("g2".into(), floats(&rgb(s8, 0x26)?));
    bg.insert("effect".into(), json!(h16(0x2A)?));
    Ok(Info {
        flag: *desc.first().context("the stage description is empty")?,
        texels,
        floor,
        mode,
        shade,
        extra,
        rows,
        bg,
    })
}

fn gcd(a: usize, b: usize) -> usize {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// One period of the panorama: rows stacked from the top (y - h) to the bottom line (y = 0 at s5).
pub fn strip(rows: &[Row], pages: &mut Pages) -> Result<Option<(Rgba, i64, i64)>> {
    let Some(r0) = rows.first() else {
        return Ok(None);
    };
    let mut period = 1;
    for r in rows {
        let m = &r.map;
        let p = (1..=m.len())
            .find(|&p| m.len().is_multiple_of(p) && (0..m.len()).all(|i| m[i] == m[i % p]))
            .context("a panorama row with an empty cell map")?;
        period = period * p / gcd(period, p);
    }
    let w = r0.w as usize;
    let top = rows.iter().map(|r| r.y as i64 - r.h as i64).min().unwrap_or(0);
    let bottom = rows.iter().map(|r| r.y as i64).max().unwrap_or(0);
    let mut out = Rgba::new(period * w, usize::try_from(bottom - top).context("the panorama has no height")?);
    for r in rows {
        let y0 = (r.y as i64 - r.h as i64 - top) as usize;
        for c in 0..period {
            let n = r.map[c % r.map.len()];
            if n < 0 {
                continue;
            }
            let &(u, v, clut, tpage) = r.cells.get(&n).context("a panorama cell is missing")?;
            if r.w as usize != w {
                bail!("panorama rows of different widths");
            }
            let p = pages.get(tpage as u32, clut as u32);
            out.paste(c * w, y0, w, r.h as usize, p, u as usize, v as usize)?;
        }
    }
    Ok(Some((out, top, bottom)))
}

pub fn floor_atlas(info: &Info, pages: &mut Pages) -> Result<(Rgba, HashMap<(u32, u32, u32), (usize, usize)>)> {
    let keys: BTreeSet<(u32, u32, u32)> =
        info.floor.iter().map(|&(e0, e1)| (e0 & 0xffff, (e0 >> 16) & 0x1ff, e1 >> 16)).collect();
    let n = info.texels as usize;
    let cols = 8;
    let rows = keys.len().div_ceil(cols);
    let mut a = Rgba::new(cols * n, rows * n);
    let mut place = HashMap::new();
    for (i, &(uv, tp, clut)) in keys.iter().enumerate() {
        let (u, v) = ((uv & 255) as usize, (uv >> 8) as usize);
        let (ax, ay) = ((i % cols) * n, (i / cols) * n);
        a.paste(ax, ay, n, n, pages.get(tp, clut), u, v)?;
        place.insert((uv, tp, clut), (ax, ay));
    }
    Ok((a, place))
}

fn floor_div(a: i64, b: i64) -> i64 {
    a.div_euclid(b) - if b < 0 && a.rem_euclid(b) != 0 { 1 } else { 0 }
}

/// port: OpenKumite's layout instead (a camera-centred 'sky' drawn first, 3 x 3 copies of the floor map clipped
/// by the engine to the 10 x 10 tile patch, the floor lighting baked, a white texel for the fills).
pub fn convert(
    disc: &Disc,
    stage: usize,
    block: usize,
    out: &dyn Vfs,
    out_path: &str,
    name: &str,
    port: bool,
) -> Result<(usize, usize)> {
    let base = t2::BLOCK0 + 8 * block;
    let info = describe(disc.rec(base + 1)?)?;
    let mut pages = Pages::new(vram(disc.rec(base)?)?);
    let tile = tile_of(info.mode);
    let n = info.texels as i64;
    let (mut atlas, place) = floor_atlas(&info, &mut pages)?;
    if port {
        // a white 2 x 2 block below the tiles: the fills' texel
        let w = atlas.w;
        atlas.h += 2;
        atlas.px.resize(atlas.w * atlas.h * 4, 0);
        for y in atlas.h - 2..atlas.h {
            for x in 0..2.min(w) {
                atlas.set(x, y, [255; 4]);
            }
        }
    }
    let (wf, hf) = (atlas.w as f64, atlas.h as f64);
    let fade = py_slice(&disc.exe, off(FADE_TABLE), off(FADE_TABLE) + 1024).to_vec();
    let (mut pos, mut uvs, mut cols, mut idx): (Vec<[f64; 3]>, Vec<[f64; 2]>, Vec<[f64; 4]>, Vec<u32>) =
        Default::default();
    // the floor: one copy of the map, world tiles i, j in [-5, 5) (the patch the game lays around a midpoint at 0)
    let span: Vec<i64> = if port { (-15..15).collect() } else { (-5..5).collect() };
    for &j in &span {
        for &i in &span {
            let (e0, e1) = info.floor[((4 - j).rem_euclid(10) * 10 + (i + 5).rem_euclid(10)) as usize];
            let key = (e0 & 0xffff, (e0 >> 16) & 0x1ff, e1 >> 16);
            let (ax, ay) = place[&key];
            let rot = FLOOR_ROT[(e0 >> 30) as usize];
            let b = pos.len() as u32;
            // corners as the packet's: points p + 11, p + 12, p, p + 1 of the 11 x 11 grid = (x0,z1) (x1,z1) (x0,z0) (x1,z0)
            for (q, (dx, dz)) in [(0, 1), (1, 1), (0, 0), (1, 0)].into_iter().enumerate() {
                let (cu, cv) =
                    (floor_div((rot[q] & 0xff) as i64 * (n - 1), 63), floor_div((rot[q] >> 8) as i64 * (n - 1), 63));
                pos.push(mv3(&C, &[((i + dx) * tile) as f64, 0.0, ((j + dz) * tile) as f64]).map(|v| SCALE * v));
                // the port lights the floor itself (extras floor_light)
                uvs.push([(ax as f64 + cu as f64 + 0.5) / wf, (ay as f64 + cv as f64 + 0.5) / hf]);
                cols.push([1.0; 4]);
            }
            idx.extend([b, b + 2, b + 1, b + 1, b + 2, b + 3]);
        }
    }
    let n_floor = idx.len();
    // the panorama drum (unit radius, centred on the camera at eye height): vertex at left-turn angle a looks
    // along (-sin a, 0, -cos a) (glTF) and shows strip column scroll + x + 256 - a * PX_RAD (the screen centre's
    // at yaw a; the rows' scroll starts at their x plus section 8's, 0x80084060)
    let strip = strip(&info.rows, &mut pages)?;
    let bgi = |k: &str| info.bg[k].as_i64().unwrap_or(0);
    let below = bgi("below");
    let seg = 96;
    let ring = |a: f64, y: f64| {
        let (s, c) = np::sin_cos(a);
        [-s, y, -c]
    };
    let yv = |py: i64| (-below - py) as f64 / FOCAL_Y; // py: screen lines down from s5; up is +y
    let angles = |k: usize| [2.0 * PI * k as f64 / seg as f64, 2.0 * PI * (k + 1) as f64 / seg as f64];
    if let Some((s, top, bottom)) = &strip {
        let sw = s.w as f64;
        for k in 0..seg {
            let b = pos.len() as u32;
            for a in angles(k) {
                let u = ((bgi("scroll") + info.rows[0].x as i64 + 256) as f64 - a * px_rad()) / sw;
                for (py, v) in [(*top, 0.0), (*bottom, 1.0)] {
                    pos.push(ring(a, yv(py)));
                    uvs.push([u, v]);
                    cols.push([1.0; 4]);
                }
            }
            idx.extend([b, b + 1, b + 2, b + 2, b + 1, b + 3]);
        }
    }
    let n_pano = idx.len();
    // the flat fills behind it, as bands (top line, bottom line, top colour, bottom colour), drawn in this order
    let far = 4000;
    let colour = |k: &str| -> Vec<f64> {
        info.bg[k].as_array().map_or(vec![], |a| a.iter().filter_map(Value::as_f64).collect())
    };
    let mut bands = vec![(-far, bgi("sky_y"), colour("sky"), colour("sky"))];
    if bgi("sky_h") > 0 {
        bands.push((bgi("sky_y"), bgi("sky_y") + bgi("sky_h"), colour("sky"), colour("sky2")));
    }
    bands.push((bgi("ground_y"), far, colour("ground"), colour("ground")));
    if bgi("gh") > 0 {
        bands.push((bgi("gy"), bgi("gy") + bgi("gh"), colour("g1"), colour("g2")));
    }
    let mut fill_ranges = Vec::new();
    for (y0, y1, c0, c1) in &bands {
        let start = idx.len();
        for k in 0..seg {
            let b = pos.len() as u32;
            for a in angles(k) {
                for (py, c) in [(*y0, c0), (*y1, c1)] {
                    let white = if port { [0.5 / wf, (hf - 1.5) / hf] } else { [(wf - 0.5) / wf, (hf - 0.5) / hf] };
                    pos.push(ring(a, yv(py)));
                    uvs.push(white);
                    let c = c.get(..3).context("a fill colour is short")?;
                    cols.push([c[0], c[1], c[2], 1.0]);
                }
            }
            idx.extend([b, b + 1, b + 2, b + 2, b + 1, b + 3]);
        }
        fill_ranges.push((start, idx.len()));
    }
    let mut g = Glb::new();
    let pos32: Vec<[f32; 3]> = pos.iter().map(f32s).collect();
    let attrs = json!({
        "POSITION": g.acc_f32(&pos32, Some(34962), true),
        "TEXCOORD_0": g.acc_f32(&uvs.iter().map(f32s).collect::<Vec<_>>(), Some(34962), false),
        "COLOR_0": g.acc_f32(&cols.iter().map(f32s).collect::<Vec<_>>(), Some(34962), false),
    });
    let (mut images, mut samplers, mut textures) = (vec![], vec![], vec![]);
    for (im, nm, wrap) in [(Some(&atlas), "floor", 33071), (strip.as_ref().map(|s| &s.0), "panorama", 10497)] {
        let Some(im) = im else { continue };
        images.push(json!({"bufferView": g.view(&pil::png_bytes(im)?, None), "mimeType": "image/png", "name": nm}));
        samplers.push(json!({"magFilter": 9728, "minFilter": 9728, "wrapS": wrap, "wrapT": 33071}));
        textures.push(json!({"source": images.len() - 1, "sampler": samplers.len() - 1}));
    }
    g.j.insert("images".into(), json!(images));
    g.j.insert("samplers".into(), json!(samplers));
    g.j.insert("textures".into(), json!(textures));
    let mat = |nm: &str, t: usize| {
        json!({"name": nm, "doubleSided": true, "alphaMode": "MASK", "alphaCutoff": 0.5,
               "pbrMetallicRoughness": {"baseColorTexture": {"index": t}, "metallicFactor": 0, "roughnessFactor": 1}})
    };
    let mut materials = vec![mat("floor", 0)];
    if strip.is_some() {
        materials.push(mat("panorama", 1));
    }
    materials.push(json!({"name": "fill", "doubleSided": true,
                          "pbrMetallicRoughness": {"baseColorTexture": {"index": 0}, "metallicFactor": 0, "roughnessFactor": 1}}));
    let fill_mat = materials.len() - 1;
    g.j.insert("materials".into(), json!(materials));
    let width = strip.as_ref().map_or(0, |s| s.0.w);
    let tris = idx.len() / 3;
    let step = |s: &[u8], k: usize| s.iter().step_by(k).copied().collect::<Vec<u8>>();
    if port {
        let mut prims: Vec<Value> = fill_ranges
            .iter()
            .map(|&(a, b)| json!({"attributes": attrs, "indices": g.acc_u32(&idx[a..b], Some(34963)), "material": fill_mat}))
            .collect();
        if strip.is_some() {
            prims.push(
                json!({"attributes": attrs, "indices": g.acc_u32(&idx[n_floor..n_pano], Some(34963)), "material": 1}),
            );
        }
        let floor = g.acc_u32(&idx[..n_floor], Some(34963));
        let light = json!({"mode": info.mode, "shade": info.shade, "table": step(&fade, 32)});
        let j = &mut g.j;
        j.insert(
            "meshes".into(),
            json!([{"name": "floor", "primitives": [{"attributes": attrs, "indices": floor, "material": 0}]},
                                          {"name": "sky", "primitives": prims}]),
        );
        j.insert("nodes".into(), json!([{"name": "floor", "mesh": 0}, {"name": "sky", "mesh": 1}]));
        let period = 10.0 * tile as f64 * SCALE;
        j.insert("scenes".into(), json!([{"name": name, "nodes": [0, 1], "extras": {
            "game": "tekken2", "camera_drum": true, "drum_before_floor": true, "drum_y_scale": 1.0,
            "floor_period": [period, period], "floor_patch": [tile as f64 * SCALE, 10], "floor_light": light,
            "note": "sky: a unit drum centred on the camera, drawn first without depth; floor: shown on the 10 x 10 tile patch around the fighters' midpoint snapped to whole tiles (the copies repeat every floor_period)"}}]));
        j.insert("scene".into(), json!(0));
        g.write(out, out_path)?;
        return Ok((prims.len(), tris));
    }
    let mut parts = vec![("floor".to_string(), 0, n_floor, 0)];
    if strip.is_some() {
        parts.push(("panorama_strip".into(), n_floor, n_pano, 1));
    }
    parts.extend(fill_ranges.iter().enumerate().map(|(k, &(a, b))| (format!("fill_{k}"), a, b, fill_mat)));
    let (mut meshes, mut nodes) = (vec![], vec![]);
    for (nm, a, b, m) in &parts {
        meshes.push(json!({"name": nm, "primitives": [{"attributes": attrs, "indices": g.acc_u32(&idx[*a..*b], Some(34963)), "material": m}]}));
        nodes.push(json!({"name": nm, "mesh": meshes.len() - 1}));
    }
    let sky: Vec<usize> = parts.iter().enumerate().filter(|(_, p)| p.0 != "floor").map(|(k, _)| k).collect();
    nodes.push(json!({"name": "sky_drum", "children": sky,
                      "extras": {"note": "unit drum: centre it on the camera at eye height, draw the fills then the strip before the floor without depth"}}));
    let period = 10.0 * tile as f64 * SCALE;
    let (top, bottom) = strip.as_ref().map_or((0, 0), |s| (s.1, s.2));
    let extras = json!({
        "game": "tekken2", "stage": stage, "block": block, "flag": info.flag,
        "floor_period": [period, period],
        "floor_patch": {"tile": tile as f64 * SCALE, "tiles": 10, "snap": "centre = round(midpoint / tile) * tile; the patch spans centre +- 5 tiles; this mesh is the patch for centre 0"},
        "floor_mode": info.mode, "floor_shade": info.shade, "floor_extra": info.extra,
        "floor_fade": {"sz0": 10000, "scale": f64_value(6710.0 / 65536.0), "table": step(&fade, 8), "table_step": 8,
                       "note": "mode 0, shade != 0: colour 128 + T[min(1023, (view depth - sz0) * scale)] * shade / 128"},
        "floor_light": {"mode1": "min(254, 0x8000000 / d1^2 + 0x8000000 / d2^2), d to each fighter (game units)",
                        "mode2": "min(192, 0x7A120000 / d^2), d to the fighters' midpoint", "unit": 128},
        "panorama": {"turn_px": TURN_PX as i64, "px_per_rad": px_rad(), "focal_y": FOCAL_Y, "below": below,
                     "top": top, "bottom": bottom, "scroll": bgi("scroll"), "width": width,
                     "rows": bgi("rows"), "effect": bgi("effect")},
        "fills": info.bg, "lighting": lighting(disc, stage)?});
    let j = &mut g.j;
    j.insert("meshes".into(), json!(meshes));
    let last = nodes.len() - 1;
    j.insert("nodes".into(), json!(nodes));
    j.insert("scenes".into(), json!([{"name": name, "nodes": [0, last]}]));
    j.insert("scene".into(), json!(0));
    j["scenes"][0]["extras"] = extras;
    g.write(out, out_path)?;
    Ok((width, tris))
}

/// (stage, block, name): each stage from the first character whose home it is.
pub fn stages(disc: &Disc) -> Result<Vec<(usize, usize, String)>> {
    let chars = disc.characters()?;
    let mut out = Vec::new();
    for k in 0..23 {
        let homes: Vec<_> = chars.iter().filter(|c| c.stage as usize == k).collect();
        let Some(first) = homes.first() else { continue };
        let block = if first.id == 23 { 45 } else { 2 * first.id };
        if block < 0 || block as usize >= t2::BLOCKS {
            continue;
        }
        let names: Vec<String> = homes.iter().map(|c| title(&c.name)).collect();
        out.push((k, block as usize, format!("Stage {} ({})", k + 1, names.join(", "))));
    }
    Ok(out)
}

pub fn main(args: &[String]) -> Result<()> {
    let [src, out, rest @ ..] = args else { bail!("usage: t2_convert export_stages BIN OUTDIR [stage,...]") };
    let only: Option<BTreeSet<usize>> = match rest.first() {
        Some(s) => Some(
            s.split(',').map(|v| v.trim().parse().with_context(|| format!("bad stage {v}"))).collect::<Result<_>>()?,
        ),
        None => None,
    };
    let ctx = Ctx { out: &StdFs(out.into()), log: &conv_io::print_line, jobs: 1 };
    run(&ctx, &StdDisc(src.into()), "", only.as_ref())
}

/// The arenas (`only` those) as glTF in `dir` of the output, with index.json.
pub fn run(ctx: &Ctx, src: &dyn DiscFile, dir: &str, only: Option<&BTreeSet<usize>>) -> Result<()> {
    let disc = Disc::open(src)?;
    let mut index = Vec::new();
    for (k, block, name) in stages(&disc)? {
        if only.is_some_and(|o| !o.contains(&k)) {
            continue;
        }
        let file = format!("t2_stage_{k:02}.glb");
        let (w, nt) =
            convert(&disc, k, block, ctx.out, &join(dir, &file), &name, false).with_context(|| format!("stage {k}"))?;
        ctx.log(&format!("{file} {name} block {block} panorama {w} tris {nt}"));
        index.push(json!({"id": k, "name": name, "file": file, "block": block}));
    }
    let ipath = join(dir, "index.json");
    let mut prev: BTreeMap<i64, Value> = BTreeMap::new();
    if ctx.out.exists(&ipath) {
        for e in load_json(ctx.out, &ipath)?.as_array().context("index.json is not a list")? {
            prev.insert(e["id"].as_i64().context("an index.json entry without an id")?, e.clone());
        }
    }
    for e in index {
        prev.insert(e["id"].as_i64().unwrap_or(0), e);
    }
    write_json(ctx.out, &ipath, &json!(prev.into_values().collect::<Vec<_>>()), pyjson::INDENT1)
}
