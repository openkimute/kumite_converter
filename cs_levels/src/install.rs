//! install.py: install a Counter-Strike level (an already converted glb: Half-Life units x 0.0254 m, Y up) as a
//! port arena for the CS LEVELS menu.
//!
//! `cs_levels install LEVEL.glb ASSETS [STAGE_ID]`
//!
//! Writes ASSETS/stages/stage_<id>.glb: the level under one root node that puts the fight spot at the origin, and
//! the scene extras the port reads:
//!   level         the level's name (the menu entry)
//!   walls         the fighters' walk grid: {origin [x, z] m, cell m, size [w, h], rows [hex, one bit a cell,
//!                 1 = blocked]} in the arena's coordinates; the fighters cannot enter a blocked cell
//!   camera_walls  the same over all the free floor: the camera stays over it
//! and registers it in ASSETS/gamedata/stages.json with "menu": "CS LEVELS" (off the game's own stage lists).
//!
//! The free floor: a grid laid at the floor under the level's spawn points (info_player_*); a cell is free where a
//! floor lies within reach of that height and no solid surface crosses the body's height above it, among the cells
//! reachable from a spawn (when the spawns' rooms are under 8 m across, as in the training maps: on the level's six
//! largest floors instead). The fighters' arena: the free cell farthest from any wall, its free square grown side
//! by side into a maximal free rectangle (convex: no wall ever stands between the two fighters). The fight starts
//! in its middle, the fight line (x) along its longer side.
//!
//! The floats follow numpy's operation for operation (the same order, no fused multiply-add), so the grids and the
//! files come out as the Python's.

use crate::pyjson;
use anyhow::{Context, Result, bail};
use conv_io::Ctx;
use serde_json::{Value, json};
use std::path::Path;

pub const CELL: f64 = 0.25; // m
/// stages.json `menu` of an installed level: the engine's menu that lists it, by its title.
pub const MENU: &str = "CS LEVELS";
const BODY: (f64, f64) = (0.25, 1.7); // m above the floor that a solid surface blocks
const STEP: f64 = 0.3; // m: floors this far from the fight floor still count as floor
const MIN_HALF: f64 = 4.0; // m: an arena narrower than twice this looks for a larger floor
const RADIUS: usize = 2; // cells a wall grows by (the fighters' bodies)
const FLOOR_REACH: f64 = 3.0; // m around the fight spot whose floor colours a borrowed shadow
pub const NON_SOLID: [&str; 5] = ["func_illusionary", "func_ladder", "func_water", "trigger", "info_"];

pub type V3 = [f64; 3];
pub type Tri = [V3; 3];

/// A bool grid [h, w], row-major (numpy's layout).
#[derive(Clone)]
pub struct Grid {
    pub h: usize,
    pub w: usize,
    pub v: Vec<bool>,
}

impl Grid {
    pub fn new(h: usize, w: usize) -> Grid {
        Grid { h, w, v: vec![false; h * w] }
    }
    pub fn at(&self, z: usize, x: usize) -> bool {
        self.v[z * self.w + x]
    }
    pub fn any(&self) -> bool {
        self.v.contains(&true)
    }
}

/// numpy's index: negative counts from the end; out of range is an error.
pub fn py_index(i: i64, n: usize) -> Result<usize> {
    let j = if i < 0 { i + n as i64 } else { i };
    if j < 0 || j >= n as i64 {
        bail!("index {i} is out of bounds for size {n}");
    }
    Ok(j as usize)
}

pub fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// np.cross (no fused multiply-add)
pub fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

/// np.linalg.norm of a 3-vector
pub fn norm(a: V3) -> f64 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}

/// np.round(x, decimals): x * 10**d, rint, / 10**d
pub fn np_round(x: f64, decimals: i32) -> f64 {
    let f = 10f64.powi(decimals);
    (x * f).round_ties_even() / f
}

/// A float list as Python prints it: [1.0, -2.5]
pub fn py_list(v: &[f64]) -> String {
    let s: Vec<String> = v.iter().map(|&x| pyjson::float_repr(x)).collect();
    format!("[{}]", s.join(", "))
}

fn u32le(d: &[u8], o: usize) -> Result<usize> {
    let b = d.get(o..o + 4).context("the glb is too short")?;
    Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
}

/// A glb's JSON and binary chunk.
pub fn read_glb(d: &[u8]) -> Result<(Value, Vec<u8>)> {
    let n = u32le(d, 12)?;
    let j =
        serde_json::from_slice(d.get(20..20 + n).context("the glb's JSON runs past it")?).context("bad glb JSON")?;
    let rest = d.get(20 + n..).unwrap_or(&[]);
    let blen = u32le(rest, 0)?;
    Ok((j, rest.get(8..).map_or(Vec::new(), |r| r[..blen.min(r.len())].to_vec())))
}

pub fn write_glb(j: &Value, binary: &[u8]) -> Vec<u8> {
    let mut js = pyjson::compact(j).into_bytes();
    js.resize(js.len().div_ceil(4) * 4, b' ');
    let mut bin = binary.to_vec();
    bin.resize(bin.len().div_ceil(4) * 4, 0);
    let mut out = Vec::with_capacity(28 + js.len() + bin.len());
    for v in [0x46546C67, 2, 12 + 8 + js.len() as u32 + 8 + bin.len() as u32, js.len() as u32, 0x4E4F534A] {
        out.extend(u32::to_le_bytes(v));
    }
    out.extend(&js);
    out.extend(u32::to_le_bytes(bin.len() as u32));
    out.extend(u32::to_le_bytes(0x004E4942));
    out.extend(&bin);
    out
}

/// An accessor's elements as floats (numpy's astype(float)), and its components per element.
pub fn accessor(j: &Value, binary: &[u8], k: &Value) -> Result<(Vec<f64>, usize)> {
    let a = k.as_u64().and_then(|k| j["accessors"].get(k as usize)).context("an accessor is missing")?;
    let v =
        a["bufferView"].as_u64().and_then(|k| j["bufferViews"].get(k as usize)).context("a bufferView is missing")?;
    let comps = match a["type"].as_str() {
        Some("SCALAR") => 1,
        Some("VEC2") => 2,
        Some("VEC3") => 3,
        Some("VEC4") => 4,
        _ => bail!("an accessor's type is not SCALAR or VEC2..4"),
    };
    let size = match a["componentType"].as_u64() {
        Some(5126 | 5125) => 4,
        Some(5123) => 2,
        Some(5121) => 1,
        _ => bail!("an accessor's componentType is not float, u32, u16 or u8"),
    };
    let off = v["byteOffset"].as_u64().unwrap_or(0) as usize + a["byteOffset"].as_u64().unwrap_or(0) as usize;
    let n = a["count"].as_u64().context("an accessor without count")? as usize * comps;
    let d = binary.get(off..off + n * size).context("an accessor runs past the glb's data")?;
    let x = match a["componentType"].as_u64() {
        Some(5126) => d.as_chunks().0.iter().map(|&b| f32::from_le_bytes(b) as f64).collect(),
        Some(5125) => d.as_chunks().0.iter().map(|&b| u32::from_le_bytes(b) as f64).collect(),
        Some(5123) => d.as_chunks().0.iter().map(|&b| u16::from_le_bytes(b) as f64).collect(),
        _ => d.iter().map(|&b| b as f64).collect(),
    };
    Ok((x, comps))
}

/// The triangles of a primitive: POSITION (moved by `place`) indexed by its indices.
pub fn primitive(j: &Value, binary: &[u8], p: &Value, place: impl Fn(&[V3]) -> Vec<V3>) -> Result<Vec<Tri>> {
    let (pos, comps) = accessor(j, binary, &p["attributes"]["POSITION"])?;
    if comps != 3 {
        bail!("a POSITION accessor is not VEC3");
    }
    let pos = place(pos.as_chunks().0);
    let (idx, _) = accessor(j, binary, &p["indices"])?;
    if idx.len() % 3 != 0 {
        bail!("a primitive's index count is not a multiple of 3");
    }
    let get = |i: f64| pos.get(i as usize).copied().context("an index past the vertices");
    idx.as_chunks::<3>().0.iter().map(|t| Ok([get(t[0])?, get(t[1])?, get(t[2])?])).collect()
}

/// (name, triangles) per mesh node (the level's nodes carry no transforms of their own but translations, which
/// are applied).
fn triangles(j: &Value, binary: &[u8]) -> Result<Vec<(String, Vec<Tri>)>> {
    let mut out = Vec::new();
    for node in j["nodes"].as_array().context("the glb has no nodes")? {
        let Some(mesh) = node.get("mesh") else {
            continue;
        };
        let t: Vec<f64> = match node.get("translation") {
            Some(t) => t.as_array().into_iter().flatten().filter_map(Value::as_f64).collect(),
            None => vec![0.0; 3],
        };
        let &[tx, ty, tz] = t.as_slice() else { bail!("a node's translation is not 3 numbers") };
        let mesh = mesh.as_u64().and_then(|m| j["meshes"].get(m as usize)).context("a node's mesh is missing")?;
        for p in mesh["primitives"].as_array().into_iter().flatten() {
            let tris = primitive(j, binary, p, |pos| pos.iter().map(|q| [q[0] + tx, q[1] + ty, q[2] + tz]).collect())?;
            out.push((node["name"].as_str().unwrap_or("").to_string(), tris));
        }
    }
    Ok(out)
}

/// An embedded picture as RGB rows: (width, height, pixels).
fn picture(j: &Value, binary: &[u8], image: usize) -> Result<(usize, usize, Vec<[u8; 3]>)> {
    let view =
        j["images"].get(image).and_then(|i| i["bufferView"].as_u64()).context("an image without a bufferView")?;
    let v = j["bufferViews"].get(view as usize).context("an image's bufferView is missing")?;
    let off = v["byteOffset"].as_u64().unwrap_or(0) as usize;
    let len = v["byteLength"].as_u64().context("a bufferView without byteLength")? as usize;
    let bytes = binary.get(off..off + len).context("an image runs past the glb's data")?;
    let mut d = png::Decoder::new(std::io::Cursor::new(bytes));
    d.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut r = d.read_info()?;
    let mut buf = vec![0; r.output_buffer_size().context("a picture too large")?];
    let info = r.next_frame(&mut buf)?;
    let ch = info.color_type.samples();
    let px =
        buf[..info.buffer_size()].chunks(ch).map(|c| if ch < 3 { [c[0]; 3] } else { [c[0], c[1], c[2]] }).collect();
    Ok((info.width as usize, info.height as usize, px))
}

/// The mean colour (0..255) of the upward faces whose centre `keep` takes, by area: each face's texel at its centre
/// times its vertex colour, as the stage is drawn.
pub fn floor_colour(j: &Value, binary: &[u8], keep: impl Fn(V3) -> bool) -> Result<Option<V3>> {
    let mut pictures = std::collections::HashMap::new();
    let (mut sum, mut weight) = ([0.0; 3], 0.0);
    for node in j["nodes"].as_array().context("the glb has no nodes")? {
        let Some(mesh) = node.get("mesh").and_then(Value::as_u64).and_then(|m| j["meshes"].get(m as usize)) else {
            continue;
        };
        let t: Vec<f64> = node["translation"].as_array().into_iter().flatten().filter_map(Value::as_f64).collect();
        let t = if let &[x, y, z] = t.as_slice() { [x, y, z] } else { [0.0; 3] };
        for p in mesh["primitives"].as_array().into_iter().flatten() {
            let tex = p["material"].as_u64().and_then(|m| j["materials"].get(m as usize));
            let Some(image) = tex
                .and_then(|m| m["pbrMetallicRoughness"]["baseColorTexture"]["index"].as_u64())
                .and_then(|k| j["textures"].get(k as usize))
                .and_then(|t| t["source"].as_u64())
            else {
                continue;
            };
            if let std::collections::hash_map::Entry::Vacant(e) = pictures.entry(image) {
                e.insert(picture(j, binary, image as usize)?);
            }
            let (w, h, px) = &pictures[&image];
            let (pos, _) = accessor(j, binary, &p["attributes"]["POSITION"])?;
            let (uv, _) = accessor(j, binary, &p["attributes"]["TEXCOORD_0"])?;
            // (a normalized u8 / u16 colour, or floats)
            let colour = match p["attributes"].get("COLOR_0") {
                Some(c) => {
                    let scale = match c.as_u64().and_then(|k| j["accessors"][k as usize]["componentType"].as_u64()) {
                        Some(5121) => 255.0,
                        Some(5123) => 65535.0,
                        _ => 1.0,
                    };
                    let (v, n) = accessor(j, binary, c)?;
                    Some((v.iter().map(|x| x / scale).collect::<Vec<_>>(), n))
                }
                None => None,
            };
            let (idx, _) = accessor(j, binary, &p["indices"])?;
            for f in idx.as_chunks::<3>().0 {
                let at = |k: usize, n: usize, v: &[f64]| -> Option<Vec<f64>> {
                    let i = f[k] as usize * n;
                    v.get(i..i + n).map(<[f64]>::to_vec)
                };
                let (Some(a), Some(b), Some(c)) = (at(0, 3, &pos), at(1, 3, &pos), at(2, 3, &pos)) else { continue };
                let [a, b, c] = [a, b, c].map(|q| [q[0] + t[0], q[1] + t[1], q[2] + t[2]]);
                let n = cross(sub(b, a), sub(c, a));
                let area = norm(n) / 2.0;
                let centre = [0, 1, 2].map(|k| (a[k] + b[k] + c[k]) / 3.0);
                if area <= 0.0 || n[1].abs() / (2.0 * area) < 0.7 || !keep(centre) {
                    continue;
                }
                let (Some(ua), Some(ub), Some(uc)) = (at(0, 2, &uv), at(1, 2, &uv), at(2, 2, &uv)) else { continue };
                let (u, v) = ((ua[0] + ub[0] + uc[0]) / 3.0, (ua[1] + ub[1] + uc[1]) / 3.0);
                let (x, y) = (((u - u.floor()) * *w as f64) as usize, ((v - v.floor()) * *h as f64) as usize);
                let Some(texel) = px.get(y.min(h - 1) * w + x.min(w - 1)) else { continue };
                let tint = match &colour {
                    Some((cv, n)) => {
                        let (Some(x), Some(y), Some(z)) = (at(0, *n, cv), at(1, *n, cv), at(2, *n, cv)) else {
                            continue;
                        };
                        [0, 1, 2].map(|k| (x[k] + y[k] + z[k]) / 3.0)
                    }
                    None => [1.0; 3],
                };
                for k in 0..3 {
                    sum[k] += texel[k] as f64 * tint[k] * area;
                }
                weight += area;
            }
        }
    }
    Ok((weight > 0.0).then(|| sum.map(|v| v / weight)))
}

/// Points spread over a triangle (at most `spacing` apart), as np.linspace / np.meshgrid lay them.
pub fn surface_points(tri: &Tri, spacing: f64, mut f: impl FnMut(V3)) {
    let [a, b, c] = *tri;
    let (ba, ca) = (sub(b, a), sub(c, a));
    let longest = norm(ba).max(norm(ca)).max(norm(sub(c, b)));
    let n = (longest / spacing).ceil() as usize + 1;
    let step = 1.0 / n as f64;
    let lin = |i: usize| if i == n { 1.0 } else { i as f64 * step };
    for r in 0..=n {
        let v = lin(r);
        for q in 0..=n {
            let u = lin(q);
            if u + v <= 1.0 {
                f([a[0] + u * ba[0] + v * ca[0], a[1] + u * ba[1] + v * ca[1], a[2] + u * ba[2] + v * ca[2]]);
            }
        }
    }
}

/// The highest surface under (x, top, z) facing up.
fn floor_under(tris: &[Tri], x: f64, z: f64, top: f64) -> Option<f64> {
    let mut best: Option<f64> = None;
    for &[a, b, c] in tris {
        let n = cross(sub(b, a), sub(c, a));
        if n[1].abs() < 1e-6 || n[1].abs() / norm(n) < 0.7 {
            continue;
        }
        // barycentric in xz
        let d = (b[2] - c[2]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[2] - c[2]);
        if d.abs() < 1e-12 {
            continue;
        }
        let l1 = ((b[2] - c[2]) * (x - c[0]) + (c[0] - b[0]) * (z - c[2])) / d;
        let l2 = ((c[2] - a[2]) * (x - c[0]) + (a[0] - c[0]) * (z - c[2])) / d;
        let l3 = 1.0 - l1 - l2;
        if l1.min(l2).min(l3) < -1e-6 {
            continue;
        }
        let y = l1 * a[1] + l2 * b[1] + l3 * c[1];
        if y <= top + 1e-6 && best.is_none_or(|b| y > b) {
            best = Some(y);
        }
    }
    best
}

/// The heights (m) of the level's largest floors: upward-facing area summed per 0.25 m, the n largest.
pub fn floor_heights(solid: &[Tri]) -> Vec<f64> {
    let mut area: Vec<(i64, f64)> = Vec::new(); // a dict: insertion order
    for t in solid {
        let nrm = cross(sub(t[1], t[0]), sub(t[2], t[0]));
        let a = norm(nrm) / 2.0;
        if a > 0.0 && nrm[1] / (2.0 * a) >= 0.7 {
            let k = (((t[0][1] + t[1][1]) + t[2][1]) / 3.0 / CELL).round_ties_even() as i64;
            match area.iter_mut().find(|e| e.0 == k) {
                Some(e) => e.1 += a,
                None => area.push((k, a)),
            }
        }
    }
    area.sort_by(|x, y| y.1.total_cmp(&x.1)); // stable, as sorted(reverse=True)
    area.iter().take(6).map(|e| e.0 as f64 * CELL).collect()
}

/// The cell (column, row) of a point; out of the grid is an error.
fn cell(p: V3, lo: [f64; 2], g: &Grid) -> Result<(usize, usize)> {
    Ok((py_index(((p[0] - lo[0]) / CELL).floor() as i64, g.w)?, py_index(((p[2] - lo[1]) / CELL).floor() as i64, g.h)?))
}

/// Free cells (bool [h, w]) at a floor height: floor within STEP, nothing solid across the body.
pub fn walk_grid(solid: &[Tri], floor_y: f64, lo: [f64; 2], hi: [f64; 2]) -> Result<Grid> {
    let w = ((hi[0] - lo[0]) / CELL).ceil() as usize;
    let h = ((hi[1] - lo[1]) / CELL).ceil() as usize;
    let mut floor = Grid::new(h, w);
    let mut wall = Grid::new(h, w);
    for tri in solid {
        let ys = tri.map(|p| p[1]);
        let n = cross(sub(tri[1], tri[0]), sub(tri[2], tri[0]));
        let up = n[1].abs() / norm(n).max(1e-12) >= 0.7;
        if ys[0].max(ys[1]).max(ys[2]) < floor_y - STEP || ys[0].min(ys[1]).min(ys[2]) > floor_y + BODY.1 {
            continue;
        }
        let mut err = Ok(());
        surface_points(tri, 0.08, |p| {
            if up && (p[1] - floor_y).abs() <= STEP {
                match cell(p, lo, &floor) {
                    Ok((cx, cz)) => floor.v[cz * w + cx] = true,
                    Err(e) => err = Err(e),
                }
            }
            if p[1] > floor_y + BODY.0 && p[1] < floor_y + BODY.1 {
                match cell(p, lo, &wall) {
                    Ok((cx, cz)) => wall.v[cz * w + cx] = true,
                    Err(e) => err = Err(e),
                }
            }
        });
        err?;
    }
    for _ in 0..RADIUS {
        let mut g = wall.clone();
        for z in 0..h {
            for x in 0..w {
                g.v[z * w + x] |= (z > 0 && wall.at(z - 1, x))
                    || (z + 1 < h && wall.at(z + 1, x))
                    || (x > 0 && wall.at(z, x - 1))
                    || (x + 1 < w && wall.at(z, x + 1));
            }
        }
        wall = g;
    }
    floor.v.iter_mut().zip(&wall.v).for_each(|(f, w)| *f &= !w);
    Ok(floor)
}

fn reachable(free: &Grid, start: (usize, usize)) -> Grid {
    let mut seen = Grid::new(free.h, free.w);
    if !free.at(start.0, start.1) {
        return seen;
    }
    let mut stack = vec![start];
    seen.v[start.0 * free.w + start.1] = true;
    while let Some((z, x)) = stack.pop() {
        for (dz, dx) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            let (a, b) = (z as i64 + dz, x as i64 + dx);
            if a >= 0 && a < free.h as i64 && b >= 0 && b < free.w as i64 {
                let (a, b) = (a as usize, b as usize);
                if free.at(a, b) && !seen.at(a, b) {
                    seen.v[a * free.w + b] = true;
                    stack.push((a, b));
                }
            }
        }
    }
    seen
}

/// Cells to the nearest blocked cell (chessboard distance).
fn clearance(free: &Grid) -> Vec<i64> {
    let (h, w) = (free.h, free.w);
    let mut d: Vec<i64> = free.v.iter().map(|&f| if f { 1_000_000 } else { 0 }).collect();
    for z in 0..h {
        for x in 0..w {
            if d[z * w + x] != 0 {
                let up = if z > 0 { d[(z - 1) * w + x] + 1 } else { 1 };
                let left = if x > 0 { d[z * w + x - 1] + 1 } else { 1 };
                d[z * w + x] = d[z * w + x].min(up).min(left);
            }
        }
    }
    for z in (0..h).rev() {
        for x in (0..w).rev() {
            if d[z * w + x] != 0 {
                let down = if z + 1 < h { d[(z + 1) * w + x] + 1 } else { 1 };
                let right = if x + 1 < w { d[z * w + x + 1] + 1 } else { 1 };
                d[z * w + x] = d[z * w + x].min(down).min(right);
            }
        }
    }
    d
}

/// np.argmax as (row, column): the first largest.
fn argmax(c: &[i64], w: usize) -> (usize, usize) {
    let mut k = 0;
    for (i, &v) in c.iter().enumerate() {
        if v > c[k] {
            k = i;
        }
    }
    (k / w.max(1), k % w.max(1))
}

pub fn main(args: &[String]) -> Result<()> {
    let [src, assets, ..] = args else { bail!("usage: cs_levels install LEVEL.glb ASSETS [STAGE_ID]") };
    let sid: i64 = match args.get(2) {
        Some(s) => s.trim().parse().with_context(|| format!("not a stage id: {s}"))?,
        None => 40,
    };
    let src_name = Path::new(src).file_name().map_or(String::new(), |n| n.to_string_lossy().into_owned());
    let d = std::fs::read(src).with_context(|| format!("cannot read {src}"))?;
    let glb = read_glb(&d).with_context(|| format!("{src}: not a glb"))?;
    let fs = conv_io::StdFs(assets.into());
    install(&Ctx { out: &fs, log: &conv_io::print_line, jobs: 1 }, glb, &src_name, sid)?;
    // the files changed: the online fingerprint again
    crate::fingerprint::print_manifest(&fs, assets)
}

/// Installs a level (its glb's JSON and binary; `src_name` its file name, LEVEL.glb) as stages/stage_<sid>.glb of
/// the assets and registers it in stages.json. The fingerprint is left to the caller (once after all the changes).
pub fn install(ctx: &Ctx, (mut j, binary): (Value, Vec<u8>), src_name: &str, sid: i64) -> Result<()> {
    let scene = j.get("scene").and_then(Value::as_u64).unwrap_or(0) as usize;
    let name = match j["scenes"].get(scene).context("the glb has no scene")?["name"].as_str() {
        Some(n) if !n.is_empty() => n.to_string(),
        _ => Path::new(src_name).file_stem().map_or(String::new(), |n| n.to_string_lossy().into_owned()),
    };
    let tris = triangles(&j, &binary)?;
    let solid: Vec<Tri> =
        tris.iter().filter(|(n, _)| !NON_SOLID.iter().any(|p| n.starts_with(p))).flat_map(|t| t.1.clone()).collect();
    if solid.is_empty() {
        bail!("the level has no solid triangles");
    }
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for p in tris.iter().flat_map(|t| t.1.iter().flatten()) {
        lo = [lo[0].min(p[0]), lo[1].min(p[2])];
        hi = [hi[0].max(p[0]), hi[1].max(p[2])];
    }
    let (lo, hi) = ([lo[0] - CELL, lo[1] - CELL], [hi[0] + CELL, hi[1] + CELL]);
    let mut spawns = Vec::new();
    for n in j["nodes"].as_array().into_iter().flatten() {
        if n["name"].as_str().is_some_and(|s| s.starts_with("info_player"))
            && let Some(t) = n.get("translation")
        {
            let t: Vec<f64> = t.as_array().into_iter().flatten().filter_map(Value::as_f64).collect();
            let &[x, y, z] = t.as_slice() else { bail!("a spawn's translation is not 3 numbers") };
            spawns.push([x, y, z]);
        }
    }
    if spawns.is_empty() {
        bail!("no info_player_* spawn nodes");
    }
    // the spawns' floors, each with the free cells reachable from it
    let mut best: Option<(i64, f64, Grid, usize, usize)> = None;
    let mut grids: Vec<(f64, Grid)> = Vec::new();
    for s in &spawns {
        let Some(fy) = floor_under(&solid, s[0], s[2], s[1]) else {
            continue;
        };
        let key = np_round(fy, 2);
        if !grids.iter().any(|g| g.0 == key) {
            grids.push((key, walk_grid(&solid, fy, lo, hi)?));
        }
        let free = &grids.iter().find(|g| g.0 == key).context("a grid")?.1;
        let at = (py_index(((s[2] - lo[1]) / CELL) as i64, free.h)?, py_index(((s[0] - lo[0]) / CELL) as i64, free.w)?);
        let area = reachable(free, at);
        if !area.any() {
            continue;
        }
        let c = clearance(&area);
        let (z, x) = argmax(&c, area.w);
        if best.as_ref().is_none_or(|b| c[z * area.w + x] > b.0) {
            best = Some((c[z * area.w + x], fy, area, z, x));
        }
    }
    // small rooms there (the training maps): the most open place on the level's largest floors instead
    if best.as_ref().is_none_or(|b| b.0 as f64 * CELL < MIN_HALF) {
        for fy in floor_heights(&solid) {
            let free = walk_grid(&solid, fy, lo, hi)?;
            if !free.any() {
                continue;
            }
            let c = clearance(&free);
            let (z, x) = argmax(&c, free.w);
            if best.as_ref().is_none_or(|b| c[z * free.w + x] > b.0) {
                best = Some((c[z * free.w + x], fy, reachable(&free, (z, x)), z, x));
            }
        }
    }
    let (clear, fy, area, z, x) = best.context("no free floor on the level")?;
    // the fighters' arena: the free square around that cell grown side by side while a whole new edge is free (a
    // maximal free rectangle: convex, so no wall ever stands between the two fighters); the camera keeps the whole
    // free area
    let (z, x) = (z as i64, x as i64);
    let mut r = [z - clear + 1, z + clear - 1, x - clear + 1, x + clear - 1]; // z0, z1, x0, x1 (inclusive)
    let inside = |z: i64, x: i64| z >= 0 && x >= 0 && z < area.h as i64 && x < area.w as i64;
    let free_at = |z: i64, x: i64| inside(z, x) && area.at(z as usize, x as usize);
    let mut grown = true;
    while grown {
        grown = false;
        for side in 0..4 {
            let [z0, z1, x0, x1] = r;
            let edge = match side {
                0 => (z0 > 0).then(|| (x0..=x1).all(|x| free_at(z0 - 1, x))),
                1 => (z1 + 1 < area.h as i64).then(|| (x0..=x1).all(|x| free_at(z1 + 1, x))),
                2 => (x0 > 0).then(|| (z0..=z1).all(|z| free_at(z, x0 - 1))),
                _ => (x1 + 1 < area.w as i64).then(|| (z0..=z1).all(|z| free_at(z, x1 + 1))),
            };
            if edge == Some(true) {
                r[side] += if side == 0 || side == 2 { -1 } else { 1 };
                grown = true;
            }
        }
    }
    let [z0, z1, x0, x1] = r;
    let mut arena = Grid::new(area.h, area.w);
    for z in z0..=z1 {
        for x in (x0..=x1).filter(|&x| inside(z, x)) {
            arena.v[z as usize * area.w + x as usize] = true;
        }
    }
    // the fight starts in its middle, the line along its longer side
    let turn = z1 - z0 > x1 - x0;
    let spot = [lo[0] + (x0 + x1 + 1) as f64 / 2.0 * CELL, fy, lo[1] + (z0 + z1 + 1) as f64 / 2.0 * CELL];
    let cells = area.v.iter().filter(|&&f| f).count();
    ctx.log(&format!(
        "{name}: arena {:.2} x {:.2} m around {} (floor {fy:.2} m), line along {}; camera area {:.0} m2",
        (x1 - x0 + 1) as f64 * CELL,
        (z1 - z0 + 1) as f64 * CELL,
        py_list(&spot.map(|v| np_round(v, 2))),
        if turn { "z" } else { "x" },
        cells as f64 * CELL * CELL
    ));
    // the arena: rotate (a quarter turn when the line runs along z) and move the spot to the origin
    // (a +90 degree turn about y takes (x, z) to (z, -x); its inverse maps arena points back for the grid)
    let q = if turn {
        let a = std::f64::consts::PI / 4.0;
        [0.0, libm::sin(a), 0.0, libm::cos(a)]
    } else {
        [0.0, 0.0, 0.0, 1.0]
    };
    let rot = |p: V3| if turn { [p[2], p[1], -p[0]] } else { p };
    let sc = j["scenes"].get_mut(scene).context("the glb has no scene")?;
    let roots = sc.get("nodes").cloned().context("the glb's scene has no nodes")?;
    let nodes = j["nodes"].as_array_mut().context("the glb has no nodes")?;
    nodes.push(json!({"name": "level", "children": roots, "rotation": q, "translation": rot(spot).map(|v| -v)}));
    let level = nodes.len() - 1;
    j["scenes"][scene]["nodes"] = json!([level]);
    // the walk grid in arena coordinates: resample the level grid through the inverse placement
    let corners =
        [[lo[0], 0.0, lo[1]], [hi[0], 0.0, lo[1]], [lo[0], 0.0, hi[1]], [hi[0], 0.0, hi[1]]].map(|c| rot(sub(c, spot)));
    let alo = [0, 2].map(|k| corners.iter().map(|c| c[k]).fold(f64::INFINITY, f64::min));
    let ahi = [0, 2].map(|k| corners.iter().map(|c| c[k]).fold(f64::NEG_INFINITY, f64::max));
    let w = ((ahi[0] - alo[0]) / CELL).ceil() as usize;
    let h = ((ahi[1] - alo[1]) / CELL).ceil() as usize;
    // the level cell under each arena cell, None outside the level grid
    let mut under = Vec::with_capacity(w * h);
    for r in 0..h {
        for c in 0..w {
            let (gx, gz) = (alo[0] + (c as f64 + 0.5) * CELL, alo[1] + (r as f64 + 0.5) * CELL);
            let back = if turn { [-gz + spot[0], spot[2] + gx] } else { [gx + spot[0], gz + spot[2]] };
            let lx = ((back[0] - lo[0]) / CELL).floor() as i64;
            let lz = ((back[1] - lo[1]) / CELL).floor() as i64;
            let inside = lx >= 0 && lx < area.w as i64 && lz >= 0 && lz < area.h as i64;
            under.push(inside.then(|| lz as usize * area.w + lx as usize));
        }
    }
    let resample = |grid: &Grid| -> Vec<String> {
        under
            .chunks(w.max(1))
            .map(|row| {
                let mut bytes = vec![0u8; row.len().div_ceil(8)];
                for (i, u) in row.iter().enumerate() {
                    if !u.is_some_and(|k| grid.v[k]) {
                        bytes[i / 8] |= 1 << (i % 8); // np.packbits(~row, bitorder='little')
                    }
                }
                bytes.iter().map(|b| format!("{b:02x}")).collect()
            })
            .collect()
    };
    let (rows, camera_rows) = (resample(&arena), resample(&area));
    let origin = alo.map(|v| np_round(v, 4));
    let sc = j["scenes"][scene].as_object_mut().context("a scene is not an object")?;
    let extras = sc.entry("extras").or_insert_with(|| json!({}));
    let extras = extras.as_object_mut().context("the scene's extras are not an object")?;
    extras.insert("level".into(), json!(name));
    extras.insert("source".into(), json!(src_name));
    extras.insert("walls".into(), json!({"origin": origin, "cell": CELL, "size": [w, h], "rows": rows}));
    extras.insert("camera_walls".into(), json!({"origin": origin, "cell": CELL, "size": [w, h], "rows": camera_rows}));
    extras.insert(
        "note".into(),
        json!(
            "Counter-Strike level: the fight spot at the origin; walls = the fighters' walk grid (a free rectangle), \
             camera_walls = all the free floor the camera may stand over (bit set = blocked, x along a row from \
             origin[0], rows along z from origin[1])"
        ),
    );
    ctx.out.write(&format!("stages/stage_{sid:02}.glb"), &write_glb(&j, &binary))?;
    let sp = "gamedata/stages.json";
    let mut stages: Value = serde_json::from_slice(&ctx.out.read(sp)?).context("stages.json")?;
    let list = stages["stages"].as_array_mut().context("stages.json has no stage list")?;
    list.retain(|s| s["id"].as_i64() != Some(sid));
    let mut entry = json!({"id": sid, "name": name, "glb": format!("stage_{sid:02}.glb"),
                           "source": format!("Counter-Strike level {src_name}"), "menu": MENU,
                           "home_characters": [], "home_names": []});
    // a level has no light of its own: the fighters' shadow of the first stage with a light table, its direction (its
    // shadow angles [b, a] in 4096ths of a turn: towards the light (cos a cos b, sin b, -sin a cos b), y up); its
    // colour this level's floor around the fight spot darkened as that stage's shadow darkens its floor around its
    // origin
    if let Some((base, l)) = list.iter().find_map(|s| Some((s, s.get("light")?))) {
        let turn = |v: &Value| v.as_f64().unwrap_or(0.0) * std::f64::consts::TAU / 4096.0;
        let (b, a) = (turn(&l["shadow_angles"][0]), turn(&l["shadow_angles"][1]));
        let toward =
            [libm::cos(a) * libm::cos(b), libm::sin(b), -(libm::sin(a) * libm::cos(b))].map(|v| np_round(v, 6));
        let shade: Vec<f64> = (0..3).map(|k| l["unknown_10"][k].as_f64().unwrap_or(0.0)).collect();
        let glb = format!("stages/{}", base["glb"].as_str().context("the light table's stage has no glb")?);
        let near = |p: V3, c: V3| (p[0] - c[0]).abs() < FLOOR_REACH && (p[2] - c[2]).abs() < FLOOR_REACH;
        // (level faces: the stages' floors are wound either way, so the height band keeps ceilings and undersides out;
        // a floor that cannot be read takes the fallback)
        let measured = |r: Result<Option<V3>>, of: &str| {
            r.unwrap_or_else(|e| {
                ctx.log(&format!("{name}: no floor colour from {of}: {e:#}"));
                None
            })
        };
        let base = ctx
            .out
            .read(&glb)
            .and_then(|d| read_glb(&d))
            .and_then(|(bj, bbin)| floor_colour(&bj, &bbin, |p| near(p, [0.0; 3]) && p[1].abs() < STEP));
        let base_floor = measured(base, &glb);
        let here = measured(floor_colour(&j, &binary, |p| near(p, spot) && (p[1] - spot[1]).abs() < STEP), "the level");
        // (darkened by the luma ratio, Rec. 601: the floor's own hue kept)
        let luma = |c: &[f64]| 0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2];
        let colour: Vec<i64> = match (base_floor, here) {
            (Some(f0), Some(f)) => {
                let k = luma(&shade) / luma(&f0).max(1.0);
                f.iter().map(|v| (v * k).clamp(0.0, 255.0).round() as i64).collect()
            }
            _ => shade.iter().map(|&v| v as i64).collect(),
        };
        entry["shadow"] = json!({"toward": toward, "colour": colour,
                                 "source": format!("direction: {glb}'s light table (the level has none); colour: this level's floor within {FLOOR_REACH} m of the fight spot darkened by the luma ratio of {glb}'s shadow colour (unknown_10) to its floor's within {FLOOR_REACH} m of its origin (floors: the mean texel x vertex colour of the upward faces)")});
    }
    list.push(entry);
    list.sort_by_key(|s| s["id"].as_i64().unwrap_or(0));
    ctx.out.write(sp, pyjson::indent1(&stages).as_bytes())?;
    ctx.log(&format!("stage_{sid:02}.glb"));
    Ok(())
}
