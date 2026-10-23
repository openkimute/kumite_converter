//! assemble.py: a model's vertices in one rigid pose (each bone's D block placed by its row's offset, then the
//! engine's seam welding) and its textured faces.
//!
//! Numbers are `Num`: Python keeps a value an int until a float touches it, and an int zero negates to 0, not
//! -0.0 (extract_models writes `-y / 1000`), so each value carries whether it is still an int.

use crate::t3m;
use anyhow::{Context, Result};
use std::collections::{BTreeMap, HashMap};
use std::ops::{Add, Mul};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Num {
    pub v: f64,
    pub int: bool,
}

impl Num {
    pub const fn int(v: i64) -> Num {
        Num { v: v as f64, int: true }
    }
    pub const fn float(v: f64) -> Num {
        Num { v, int: false }
    }
    /// `c / 2` (true division: a float)
    pub fn half(self) -> Num {
        Num::float(self.v / 2.0)
    }
    /// `-x`
    pub fn negated(self) -> Num {
        Num { v: if self.int && self.v == 0.0 { 0.0 } else { -self.v }, int: self.int }
    }
}

impl Add for Num {
    type Output = Num;
    fn add(self, o: Num) -> Num {
        Num { v: self.v + o.v, int: self.int && o.int }
    }
}

impl Mul for Num {
    type Output = Num;
    fn mul(self, o: Num) -> Num {
        let int = self.int && o.int;
        let v = self.v * o.v;
        Num { v: if int && v == 0.0 { 0.0 } else { v }, int }
    }
}

pub type M3 = [[Num; 3]; 3];
pub type V3 = [Num; 3];

pub const I3: M3 = {
    let (o, z) = (Num::int(1), Num::int(0));
    [[o, z, z], [z, o, z], [z, z, o]]
};

pub fn s32(v: u32) -> i64 {
    v as i32 as i64
}

pub fn matmul(a: &M3, b: &M3) -> M3 {
    let mut r = [[Num::int(0); 3]; 3];
    for (i, row) in r.iter_mut().enumerate() {
        for (j, x) in row.iter_mut().enumerate() {
            *x = (0..3).fold(Num::int(0), |s, k| s + a[i][k] * b[k][j]);
        }
    }
    r
}

pub fn mv(a: &M3, v: &V3) -> V3 {
    let mut r = [Num::int(0); 3];
    for (i, x) in r.iter_mut().enumerate() {
        *x = (0..3).fold(Num::int(0), |s, k| s + a[i][k] * v[k]);
    }
    r
}

pub fn rot(axis: char, deg: f64) -> M3 {
    let (c, s) = (Num::float(deg.to_radians().cos()), Num::float(deg.to_radians().sin()));
    let (o, z) = (Num::int(1), Num::int(0));
    let ns = Num::float(-s.v);
    match axis {
        'x' => [[o, z, z], [z, c, ns], [z, s, c]],
        'y' => [[c, z, s], [z, o, z], [ns, z, c]],
        _ => [[c, ns, z], [s, c, z], [z, z, o]],
    }
}

/// One textured polygon: vertex ids (None: an empty scratch slot), the corners' (u, v), the CLUT word, the row and
/// the list name.
pub struct Face {
    pub ids: Vec<Option<usize>>,
    pub uv: Option<Vec<(u8, u8)>>,
    pub cl: Option<u16>,
    pub k: usize,
    pub name: &'static str,
}

/// Python's `a[i]` with a possibly negative index.
fn at(len: usize, i: i64) -> Result<usize> {
    let j = if i < 0 { i + len as i64 } else { i };
    if j < 0 || j >= len as i64 {
        anyhow::bail!("a weld index is out of range");
    }
    Ok(j as usize)
}

type Slot = Option<V3>;

fn add(a: Slot, b: Slot) -> Slot {
    Some([a?[0] + b?[0], a?[1] + b?[1], a?[2] + b?[2]])
}

fn half(v: Slot) -> Slot {
    v.map(|v| v.map(Num::half))
}

#[allow(clippy::type_complexity)]
pub fn build(x: &[u8], pose: &HashMap<usize, M3>) -> Result<(Vec<Slot>, Vec<Face>, BTreeMap<usize, (M3, V3)>)> {
    let (rows, out) = t3m::parse(x)?;
    let ce = t3m::cends(x, &out)?;
    let n = rows.len();
    // parent field indexes this list (rows 2,4 = vertex spacers, 20 = alt head)
    let bones: Vec<usize> = (0..n).filter(|i| ![2, 4, 20].contains(i)).collect();
    let mut todo: Vec<usize> = (0..n).filter(|&i| rows[i][10] == 1).collect();
    let mut world: BTreeMap<usize, (M3, V3)> = BTreeMap::new();
    while !todo.is_empty() {
        let mut progressed = false;
        for i in todo.clone() {
            let w = &rows[i];
            let p = s32(w[6]);
            let t = [Num::int(s32(w[3])), Num::int(s32(w[4])), Num::int(s32(w[5]))];
            let rl = pose.get(&i).copied().unwrap_or(I3);
            if p < 0 {
                world.insert(i, (rl, [Num::int(0); 3]));
            } else {
                let pr = *bones.get(p as usize).context("a bone's parent is out of range")?;
                let Some((pm, pt)) = world.get(&pr).copied() else {
                    continue;
                };
                let d = mv(&pm, &t);
                world.insert(i, (matmul(&pm, &rl), [pt[0] + d[0], pt[1] + d[1], pt[2] + d[2]]));
            }
            todo.retain(|&j| j != i);
            progressed = true;
        }
        if !progressed {
            anyhow::bail!("unresolved bones {todo:?}");
        }
    }
    let mut scratch: Vec<Slot> = vec![None; 200];
    let mut g: Vec<Slot> = vec![None; 200];
    let (mut verts, mut faces) = (Vec::new(), Vec::new());
    for k in 1..n {
        let Some(dsrc) = &out[k - 1].d else { continue };
        let Some((r, t)) = world.get(&k) else {
            continue;
        };
        let mut buf = Vec::new();
        for (li, lst) in dsrc.vref.iter().enumerate() {
            let src = if li == 0 { &scratch } else { &g };
            for &rf in lst {
                buf.push(src[at(200, rf as i64 / 2 - 1)?]);
            }
        }
        for v in &dsrc.verts {
            let p = mv(r, &v.map(|c| Num::int(c as i64)));
            buf.push(Some([p[0] + t[0], p[1] + t[1], p[2] + t[2]]));
        }
        for (j, v) in buf.iter().enumerate() {
            *scratch.get_mut(j).context("a bone has too many vertices")? = *v;
        }
        // weld lists, walking own vertices from slot H/2-1
        let wl = &dsrc.weld;
        let mut c = dsrc.h as i64 / 2 - 1;
        let idx = |e: u32| (e & 0xff) as i64 / 2 - 1;
        for &e in &wl.l1 {
            let (i, ci) = (at(200, idx(e))?, at(200, c)?);
            let cur = if e & 0x100 != 0 { half(scratch[ci]) } else { scratch[ci] };
            let cur = add(cur, g[i]);
            g[i] = cur;
            scratch[ci] = cur;
            c += 1;
        }
        for (from_g, l) in [(true, &wl.l2a), (false, &wl.l2b)] {
            for &e in l {
                let (i, ci) = (at(200, idx(e))?, at(200, c)?);
                let cur = if e & 0x100 != 0 { half(scratch[ci]) } else { scratch[ci] };
                scratch[ci] = add(if from_g { g[i] } else { scratch[i] }, cur);
                c += 1;
            }
        }
        for (to_g, l) in [(true, &wl.l3a), (false, &wl.l3b)] {
            for &e in l {
                let (i, ci) = (at(200, idx(e))?, at(200, c)?);
                if e & 0x100 != 0 {
                    scratch[ci] = half(scratch[ci]);
                }
                if to_g {
                    g[i] = scratch[ci]
                } else {
                    scratch[i] = scratch[ci]
                }
                c += 1;
            }
        }
        for &e in &wl.l4 {
            if e & 3 != 1 {
                let ci = at(200, c)?;
                scratch[ci] = half(scratch[ci]);
            }
            c += 1;
        }
        let base = verts.len();
        let snap: Vec<Slot> = scratch[..((buf.len() as i64).max(c) + 8).clamp(0, 200) as usize].to_vec();
        verts.extend(snap.iter().copied());
        let row = &out[k];
        let Some(pl) = &row.p else { continue };
        let u = match ce.get(&k) {
            Some(&e) => t3m::parse_uv(x, row, e)?,
            None => None,
        };
        for (name, l, nv) in pl.lists() {
            for (j, pw) in l.into_iter().enumerate() {
                let idx: Vec<usize> = [2, 9, 16, 25][..nv].iter().map(|s| ((pw >> s) & 0x7f) as usize).collect();
                let ids = idx.iter().map(|&i| snap.get(i).copied().flatten().map(|_| base + i)).collect();
                let (mut uv, mut cl) = (None, None);
                if let Some(u) = &u {
                    let ent = u.list(name).get(j).context("a texture entry is missing")?;
                    let a = *ent.first().context("a texture entry is empty")?;
                    cl = Some(*u.attrs.get(a as usize).context("a texture attribute is out of range")?);
                    uv = Some(
                        ent[1..]
                            .iter()
                            .map(|&q| u.uvs.get(q as usize).copied().context("a UV index is out of range"))
                            .collect::<Result<_>>()?,
                    );
                }
                faces.push(Face { ids, uv, cl, k, name });
            }
        }
    }
    Ok((verts, faces, world))
}
