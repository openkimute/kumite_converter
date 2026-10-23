//! force_stage.py: make an installed Counter-Strike level (install.rs) an extra Tekken Force stage.
//!
//! `cs_levels force_stage ASSETS STAGE_ID BASE_STAGE [NAME]`
//!
//! Tekken Force runs along +x: the player starts at x = 0, the soldiers come in from ahead, the boss waits past the
//! clear (x ~ 30 m) in an arena of +-5 m. Its camera stands 7 m to the route's +z, 3.5 m up, and looks across the
//! route towards -z: a stage is worth seeing when the level opens up on that side.
//!
//! The level's route is a straight run of free floor (the installer's walk grid: floor within reach of one height,
//! nothing solid across the body) at least 1.75 m wide on either side of its line, along one of the level's axes in
//! either direction, as long as it can be (42 m, else down to 26 m: the script's distances are then scaled to fit).
//! Each run is scored by the view: how far the floor stays open (nothing solid between 0.5 m and 3.5 m above it)
//! beyond the run on the far side, and how much room there is towards the camera. Writes
//! ASSETS/gamedata/force.json "extra_stages" (replacing the level's entry):
//!   name         the stage card's two lines (the level's name)
//!   stage        the stage id (stages.json)
//!   name_colour  the card's name colour (the base stage's)
//!   base         the original stage (0..3) whose tables it uses: soldier costumes, health, classes, AI level, bonus
//!   route        origin [x, y, z] m (the route's x = 0 on the floor) and axis [x, z] (the route's +x in the level);
//!                the route's +z is the axis turned a quarter left (-axis z, axis x)
//!   scale        the script's distances against the original's (1: a 42 m route)
//!   script       the base stage's event script (its x and stream limits scaled), with a roast chicken (event
//!                type 4) every 4 m of the way on top
//!   bosses       how many bosses come in together at the end, drawn from boss_pool (character ids)

use crate::install::{
    CELL, Grid, NON_SOLID, Tri, V3, floor_heights, np_round, primitive, py_list, read_glb, surface_points, walk_grid,
};
use crate::pyjson;
use anyhow::{Context, Result, bail};
use conv_io::Ctx;
use serde_json::{Value, json};

const BEHIND: f64 = 6.0; // m of route behind the start
const LENGTHS: [i64; 5] = [42, 38, 34, 30, 26]; // m: the route lengths tried, longest first
const HALF: f64 = 1.75; // m: free half width the route needs (the body's room taken off)
const CLEAR: i64 = 30720; // the original scripts' clear (game units); the boss arena +-5 m after it
const VIEW: (f64, f64) = (0.5, 3.5); // m above the floor: what hides the view
const MIN_FAR: f64 = 10.0; // m of open view beyond the route that make it worth taking
const BOSS_POOL: [i64; 5] = [13, 14, 20, 23, 33]; // Heihachi, Ogre, True Ogre, Kazuya, Devil
// the route's +x in the level for each quarter turn of the grids (np.rot90 k): see `place`
const AXES: [(f64, f64); 4] = [(1.0, 0.0), (0.0, 1.0), (-1.0, 0.0), (0.0, -1.0)];

type M4 = [[f64; 4]; 4];

fn floats(v: Option<&Value>, default: &[f64]) -> Result<Vec<f64>> {
    match v {
        None => Ok(default.to_vec()),
        Some(v) => v.as_array().context("not a list")?.iter().map(|x| x.as_f64().context("not a number")).collect(),
    }
}

fn node_matrix(n: &Value) -> Result<M4> {
    if let Some(m) = n.get("matrix") {
        let m = floats(Some(m), &[])?;
        if m.len() != 16 {
            bail!("a node's matrix is not 16 numbers");
        }
        return Ok(std::array::from_fn(|i| std::array::from_fn(|j| m[j * 4 + i])));
    }
    let &[x, y, z, w] = floats(n.get("rotation"), &[0.0, 0.0, 0.0, 1.0])?.as_slice() else { bail!("a bad rotation") };
    let r = [
        [1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y - z * w), 2.0 * (x * z + y * w)],
        [2.0 * (x * y + z * w), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z - x * w)],
        [2.0 * (x * z - y * w), 2.0 * (y * z + x * w), 1.0 - 2.0 * (x * x + y * y)],
    ];
    let &[sx, sy, sz] = floats(n.get("scale"), &[1.0; 3])?.as_slice() else { bail!("a bad scale") };
    let &[tx, ty, tz] = floats(n.get("translation"), &[0.0; 3])?.as_slice() else { bail!("a bad translation") };
    let (s, t) = ([sx, sy, sz], [tx, ty, tz]);
    let mut m = [[0.0; 4]; 4];
    for i in 0..3 {
        for j in 0..3 {
            m[i][j] = r[i][j] * s[j];
        }
        m[i][3] = t[i];
    }
    m[3][3] = 1.0;
    Ok(m)
}

/// a @ b of 4x4 matrices as numpy's BLAS (Accelerate, arm64) sums them.
/// shortcut: matches the reference machine's BLAS; another numpy build may differ in the last bit.
fn matmul(a: &M4, b: &M4) -> M4 {
    std::array::from_fn(|i| {
        std::array::from_fn(|j| {
            a[i][2].mul_add(b[2][j], a[i][0] * b[0][j]) + a[i][3].mul_add(b[3][j], a[i][1] * b[1][j])
        })
    })
}

/// pos @ m[:3, :3].T + m[:3, 3] as numpy's BLAS sums it: fused along each row, the last row of an odd count plain.
fn place(pos: &[V3], m: &M4) -> Vec<V3> {
    let odd_last = if pos.len() % 2 == 1 { pos.len() - 1 } else { usize::MAX };
    pos.iter()
        .enumerate()
        .map(|(i, p)| {
            std::array::from_fn(|j| {
                let r = &m[j];
                let s = if i == odd_last {
                    p[0] * r[0] + p[1] * r[1] + p[2] * r[2]
                } else {
                    p[2].mul_add(r[2], p[1].mul_add(r[1], p[0] * r[0]))
                };
                s + r[3]
            })
        })
        .collect()
}

/// (name, triangles) per mesh node, placed by the whole node hierarchy (the installed level hangs under its root
/// node).
fn triangles(j: &Value, binary: &[u8]) -> Result<Vec<(String, Vec<Tri>)>> {
    fn walk(
        j: &Value,
        binary: &[u8],
        k: &Value,
        parent: &M4,
        depth: u32,
        out: &mut Vec<(String, Vec<Tri>)>,
    ) -> Result<()> {
        if depth > 64 {
            bail!("the glb's nodes nest too deep (a cycle?)");
        }
        let n = k.as_u64().and_then(|k| j["nodes"].get(k as usize)).context("a node is missing")?;
        let m = matmul(parent, &node_matrix(n)?);
        if let Some(mesh) = n.get("mesh") {
            let mesh = mesh.as_u64().and_then(|i| j["meshes"].get(i as usize)).context("a node's mesh is missing")?;
            for p in mesh["primitives"].as_array().into_iter().flatten() {
                out.push((
                    n["name"].as_str().unwrap_or("").to_string(),
                    primitive(j, binary, p, |pos| place(pos, &m))?,
                ));
            }
        }
        for c in n["children"].as_array().into_iter().flatten() {
            walk(j, binary, c, &m, depth + 1, out)?;
        }
        Ok(())
    }
    let eye: M4 = std::array::from_fn(|i| std::array::from_fn(|j| if i == j { 1.0 } else { 0.0 }));
    let scene = j.get("scene").and_then(Value::as_u64).unwrap_or(0) as usize;
    let mut out = Vec::new();
    for k in j["scenes"].get(scene).context("the glb has no scene")?["nodes"].as_array().into_iter().flatten() {
        walk(j, binary, k, &eye, 0, &mut out)?;
    }
    Ok(out)
}

/// Cells a solid surface crosses within VIEW above the floor at `fy`.
fn opaque(solid: &[Tri], fy: f64, lo: [f64; 2], h: usize, w: usize) -> Grid {
    let mut g = Grid::new(h, w);
    for t in solid {
        let ys = t.map(|p| p[1]);
        if ys[0].max(ys[1]).max(ys[2]) >= fy + VIEW.0 && ys[0].min(ys[1]).min(ys[2]) <= fy + VIEW.1 {
            surface_points(t, 0.12, |p| {
                if p[1] > fy + VIEW.0 && p[1] < fy + VIEW.1 {
                    let cx = (((p[0] - lo[0]) / CELL).floor() as i64).clamp(0, w as i64 - 1) as usize;
                    let cz = (((p[2] - lo[1]) / CELL).floor() as i64).clamp(0, h as i64 - 1) as usize;
                    g.v[cz * w + cx] = true;
                }
            });
        }
    }
    g
}

/// np.rot90(g, k): a quarter turn counterclockwise, k times.
fn rot90(g: &Grid, k: usize) -> Grid {
    let (h, w) = (g.h, g.w);
    let (nh, nw) = if k % 2 == 1 { (w, h) } else { (h, w) };
    let mut out = Grid::new(nh, nw);
    for r in 0..nh {
        for c in 0..nw {
            out.v[r * nw + c] = match k {
                0 => g.at(r, c),
                1 => g.at(c, w - 1 - r),
                2 => g.at(h - 1 - r, w - 1 - c),
                _ => g.at(h - 1 - c, r),
            };
        }
    }
    out
}

/// Free runs `length` cells long, `half` cells free either side of their line, in the grids turned so the route
/// runs along +columns and the camera side is +rows: (score, far m, near m, row, column) each.
fn runs(free: &Grid, op: &Grid, length: usize, half: usize) -> Vec<(f64, f64, f64, usize, usize)> {
    let (h, w) = (free.h, free.w);
    let mut s = vec![0i64; (h + 1) * (w + 1)];
    for r in 0..h {
        for c in 0..w {
            s[(r + 1) * (w + 1) + c + 1] =
                free.at(r, c) as i64 + s[r * (w + 1) + c + 1] + s[(r + 1) * (w + 1) + c] - s[r * (w + 1) + c];
        }
    }
    let sum = |r: usize, c: usize| s[r * (w + 1) + c];
    // open view from each cell towards -rows (far) and +rows (near)
    let mut up = vec![0i64; h * w];
    let mut dn = vec![0i64; h * w];
    for r in 0..h {
        for c in 0..w {
            up[r * w + c] = if op.at(r, c) {
                0
            } else if r > 0 {
                up[(r - 1) * w + c] + 1
            } else {
                1
            };
        }
    }
    for r in (0..h).rev() {
        for c in 0..w {
            dn[r * w + c] = if op.at(r, c) {
                0
            } else if r + 1 < h {
                dn[(r + 1) * w + c] + 1
            } else {
                1
            };
        }
    }
    let mut out = Vec::new();
    if h < 2 * half + 2 || w < length {
        return out;
    }
    let cap = (7.0 / CELL) as i64;
    for r in half + 1..h - half - 1 {
        for c in 0..w - length {
            let full =
                sum(r + half + 1, c + length) - sum(r - half, c + length) - sum(r + half + 1, c) + sum(r - half, c);
            if full != ((2 * half + 1) * length) as i64 {
                continue;
            }
            let far = up[(r - half - 1) * w + c..][..length].iter().sum::<i64>() as f64 / length as f64 * CELL;
            let near = dn[(r + half + 1) * w + c..][..length].iter().map(|&v| v.min(cap)).sum::<i64>() as f64
                / length as f64
                * CELL;
            out.push((far.min(25.0) + 0.7 * near, far, near, r, c));
        }
    }
    out
}

/// Grid cell (row, column) of the grids turned by np.rot90 k back in the level's grid.
fn place_cell(k: usize, r: usize, c: usize, (h, w): (usize, usize)) -> (usize, usize) {
    [(r, c), (c, w - 1 - r), (h - 1 - r, w - 1 - c), (h - 1 - c, r)][k]
}

pub fn main(args: &[String]) -> Result<()> {
    let [assets, sid, base, ..] = args else { bail!("usage: cs_levels force_stage ASSETS STAGE_ID BASE_STAGE [NAME]") };
    let sid: i64 = sid.trim().parse().with_context(|| format!("not a stage id: {sid}"))?;
    let base: usize = base.trim().parse().with_context(|| format!("not a base stage: {base}"))?;
    let fs = conv_io::StdFs(assets.into());
    force_stage(&Ctx { out: &fs, log: &conv_io::print_line, jobs: 1 }, sid, base, args.get(3).map(String::as_str))?;
    // the files changed: the online fingerprint again
    crate::fingerprint::print_manifest(&fs, assets)
}

/// Makes the installed level `sid` an extra Tekken Force stage on the tables of stage `base` (its card's name
/// `name`, else the level's in capitals). The fingerprint is left to the caller.
pub fn force_stage(ctx: &Ctx, sid: i64, base: usize, name: Option<&str>) -> Result<()> {
    let sp: Value = serde_json::from_slice(&ctx.out.read("gamedata/stages.json")?).context("stages.json")?;
    let entry = sp["stages"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|s| s["id"].as_i64() == Some(sid))
        .with_context(|| format!("no stage {sid} in stages.json"))?;
    let name = match name {
        Some(n) => n.to_string(),
        None => entry["name"].as_str().context("the stage has no name")?.to_uppercase(),
    };
    let glb = format!("stages/{}", entry["glb"].as_str().context("the stage has no glb")?);
    let (j, binary) = read_glb(&ctx.out.read(&glb)?).with_context(|| format!("{glb}: not a glb"))?;
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
    let half = (HALF / CELL).round_ties_even() as usize;
    let mut grids = Vec::new();
    for fy in floor_heights(&solid) {
        let free = walk_grid(&solid, fy, lo, hi)?;
        let op = opaque(&solid, fy, lo, free.h, free.w);
        grids.push((fy, free, op));
    }
    // (score, far, near, metres, fy, k, r, c, shape)
    type Best = (f64, f64, f64, i64, f64, usize, usize, usize, (usize, usize));
    let mut best: Option<Best> = None;
    for metres in LENGTHS {
        let n = (metres as f64 / CELL) as usize;
        for (fy, free, op) in &grids {
            for k in 0..4 {
                for (score, far, near, r, c) in runs(&rot90(free, k), &rot90(op, k), n, half) {
                    if best.is_none_or(|b| score > b.0) {
                        best = Some((score, far, near, metres, *fy, k, r, c, (free.h, free.w)));
                    }
                }
            }
        }
        if best.is_some_and(|b| b.1 >= MIN_FAR) {
            break;
        }
    }
    let (_, far, near, metres, fy, k, r, c, shape) = best.context("no straight free run of floor long enough")?;
    let (row, col) = place_cell(k, r, c, shape);
    let (ax, az) = AXES[k];
    let start = [lo[0] + (col as f64 + 0.5) * CELL, lo[1] + (row as f64 + 0.5) * CELL];
    let origin = [start[0] + ax * BEHIND, fy, start[1] + az * BEHIND];
    let axis = [ax, az];
    // the script fits the route: the clear and the boss arena after it end 1 m short of its end
    let scale = f64::min(1.0, (metres as f64 - BEHIND - 6.0) * 1000.0 / CLEAR as f64);
    ctx.log(&format!(
        "{name}: route {metres} m along {}, open view {far:.1} m beyond it, {near:.1} m towards the camera, origin {} \
         (floor {fy:.2} m), script x {scale:.2}",
        py_list(&axis),
        py_list(&origin.map(|v| np_round(v, 2)))
    ));
    let fp = "gamedata/force.json";
    let mut force: Value = serde_json::from_slice(&ctx.out.read(fp)?).context("force.json")?;
    let b = force["stages"].get(base).with_context(|| format!("no Tekken Force stage {base}"))?.clone();
    let scaled =
        |v: &Value| -> Result<i64> { Ok((v.as_f64().context("a script number")? * scale).round_ties_even() as i64) };
    let mut script = Vec::new();
    for e in b["script"].as_array().context("the base stage has no script")? {
        let mut e = e.clone();
        e["x"] = json!(scaled(&e["x"])?);
        if e["type"].as_i64() == Some(2) {
            e["p1"] = json!(scaled(&e["p1"])?);
        }
        script.push(e);
    }
    let clear = (CLEAR as f64 * scale).round_ties_even() as i64;
    for (k, x) in (3072..clear - 2047).step_by(4096).enumerate() {
        script
            .push(json!({"x": x, "type": 4, "dx": 5000, "z": if k % 2 == 0 { 1100 } else { -1100 }, "p0": 0, "p1": 0}));
    }
    // in route order (the clear and the end stay last)
    let is_end = |e: &Value| matches!(e["type"].as_i64(), Some(6 | 7));
    let (end, mut script): (Vec<Value>, Vec<Value>) = script.into_iter().partition(is_end);
    script.sort_by_key(|e| e["x"].as_i64().unwrap_or(0));
    script.extend(end);
    let mut extra: Vec<Value> = force
        .get("extra_stages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|s| s["stage"].as_i64() != Some(sid))
        .cloned()
        .collect();
    let source = entry["source"].as_str().context("the stage has no source")?;
    extra.push(json!({"name": [name, ""], "stage": sid, "name_colour": b["name_colour"], "base": 3,
                      "script_from": base, "route": {"origin": origin.map(|v| np_round(v, 4)), "axis": axis,
                                                     "half_width": HALF, "length": metres, "view": np_round(far, 2)},
                      "scale": py_round(scale, 4),
                      "script": script, "bosses": 2, "boss_pool": BOSS_POOL,
                      "source": format!("cs_levels force_stage: {source}")}));
    force.as_object_mut().context("force.json is not an object")?.insert("extra_stages".into(), json!(extra));
    ctx.out.write(fp, pyjson::indent1(&force).as_bytes())
}

/// Python's round(x, n) of a float: correctly rounded (Rust's formatting rounds the exact value, ties to even).
fn py_round(x: f64, n: usize) -> f64 {
    format!("{x:.n$}").parse().unwrap_or(x)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rot90_as_numpy() {
        // [[1, 2, 3], [4, 5, 6]] turned: [[3, 6], [2, 5], [1, 4]]
        let g = Grid { h: 2, w: 3, v: vec![true, false, false, false, true, true] };
        let r = rot90(&g, 1);
        assert_eq!((r.h, r.w, r.v), (3, 2, vec![false, true, false, true, true, false]));
        for k in 0..4 {
            let r = rot90(&g, k);
            for (rr, cc) in [(0, 0), (r.h - 1, 0), (0, r.w - 1)] {
                let (z, x) = place_cell(k, rr, cc, (g.h, g.w));
                assert_eq!(r.at(rr, cc), g.at(z, x));
            }
        }
    }

    #[test]
    fn rounds_as_python() {
        assert_eq!(py_round(0.9765625, 4), 0.9766);
        assert_eq!(py_round(0.03125, 4), 0.0312);
        // numpy's (x * 100, rint, / 100), where Python's round(2.675, 2) is 2.67
        assert_eq!(np_round(2.675, 2), 2.68);
        assert_eq!(np_round(1.005, 2), 1.0);
        assert_eq!(np_round(-0.125, 2), -0.12);
    }
}
