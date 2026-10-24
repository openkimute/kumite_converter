//! skin: build a weighted mesh: each vertex = sum_i w_i * (M_row_i * p_i). Emulates the engine's seam welding.

use crate::t3m;
use anyhow::{Context, Result};

/// A vertex: its (row, weight, local position) influences; None = an empty scratch slot.
pub type Vert = Option<Vec<(usize, f64, [i16; 3])>>;

/// A polygon: vertex ids (None = not built), its UVs and CLUT attribute (None without a texture block), row, list
/// name, double-sided.
pub struct Face {
    pub ids: Vec<Option<usize>>,
    pub uv: Option<Vec<(u8, u8)>>,
    pub attr: Option<u16>,
    pub k: usize,
    pub name: &'static str,
    pub ds: bool,
}

const SLOTS: usize = 200;

/// Python's `l[i]` index into a 200-entry scratch list (negative counts from the end)
fn ix(i: i64) -> Result<usize> {
    let k = if i < 0 { i + SLOTS as i64 } else { i };
    usize::try_from(k).ok().filter(|&k| k < SLOTS).context("a weld list indexes past the vertex scratch")
}

fn half(v: &Vert) -> Vert {
    v.as_ref().map(|v| v.iter().map(|&(r, w, p)| (r, w / 2.0, p)).collect())
}

fn add(a: &Vert, b: &Vert) -> Vert {
    Some([a.as_ref()?.as_slice(), b.as_ref()?.as_slice()].concat())
}

pub fn build(x: &[u8]) -> Result<(Vec<Vert>, Vec<Face>)> {
    let (rows, out) = t3m::parse(x)?;
    let ce = t3m::cends(x, &out)?;
    let n = rows.len();
    let mut scratch: Vec<Vert> = vec![None; SLOTS];
    let mut g: Vec<Vert> = vec![None; SLOTS];
    let (mut verts, mut faces) = (Vec::new(), Vec::new());
    for k in 1..n {
        let Some(d) = &out[k - 1].d else { continue };
        if rows[k][10] == 0 {
            continue;
        }
        let mut buf: Vec<Vert> = Vec::new();
        for (li, lst) in d.vref.iter().enumerate() {
            let src = if li == 0 { &scratch } else { &g };
            for &r in lst {
                buf.push(src[ix(r as i64 / 2 - 1)?].clone());
            }
        }
        for v in &d.verts {
            buf.push(Some(vec![(k, 1.0, *v)]));
        }
        if buf.len() > SLOTS {
            anyhow::bail!("a section has more vertices than the scratch holds");
        }
        for (j, v) in buf.iter().enumerate() {
            scratch[j] = v.clone();
        }
        let w = &d.weld;
        let mut c = d.h as i64 / 2 - 1;
        let e_i = |e: u32| ix((e & 0xff) as i64 / 2 - 1);
        for &e in &w.l1 {
            let (i, ci) = (e_i(e)?, ix(c)?);
            let cur = if e & 0x100 != 0 { half(&scratch[ci]) } else { scratch[ci].clone() };
            let cur = add(&cur, &g[i]);
            g[i] = cur.clone();
            scratch[ci] = cur;
            c += 1;
        }
        for (from_g, l) in [(true, &w.l2a), (false, &w.l2b)] {
            for &e in l {
                let (i, ci) = (e_i(e)?, ix(c)?);
                let cur = if e & 0x100 != 0 { half(&scratch[ci]) } else { scratch[ci].clone() };
                scratch[ci] = add(if from_g { &g[i] } else { &scratch[i] }, &cur);
                c += 1;
            }
        }
        for (to_g, l) in [(true, &w.l3a), (false, &w.l3b)] {
            for &e in l {
                let (i, ci) = (e_i(e)?, ix(c)?);
                if e & 0x100 != 0 {
                    scratch[ci] = half(&scratch[ci]);
                }
                let v = scratch[ci].clone();
                if to_g {
                    g[i] = v
                } else {
                    scratch[i] = v
                }
                c += 1;
            }
        }
        for &e in &w.l4 {
            if e & 3 != 1 {
                let ci = ix(c)?;
                scratch[ci] = half(&scratch[ci]);
            }
            c += 1;
        }
        let base = verts.len();
        let snap_len = ((buf.len() as i64).max(c) + 8).clamp(0, SLOTS as i64) as usize;
        let snap = &scratch[..snap_len];
        verts.extend_from_slice(snap);
        let r = &out[k];
        let Some(pl) = &r.p else { continue };
        let u = match ce.get(&k) {
            Some(&cend) => t3m::parse_uv(x, r, cend)?,
            None => None,
        };
        for (name, l, nv) in pl.lists() {
            for (j, &p) in l.iter().enumerate() {
                let mut idx = vec![(p >> 2) & 0x7f, (p >> 9) & 0x7f, (p >> 16) & 0x7f];
                if nv == 4 {
                    idx.push((p >> 25) & 0x7f);
                }
                let ids = idx
                    .iter()
                    .map(|&i| (snap.get(i as usize).is_some_and(|v| v.is_some())).then_some(base + i as usize));
                let (uv, attr) = match &u {
                    Some(u) => {
                        let ent = u.list(name).get(j).context("a polygon has no texture entry")?;
                        let a =
                            *ent.first().and_then(|&a| u.attrs.get(a as usize)).context("a bad texture attribute")?;
                        let uv = ent[1..]
                            .iter()
                            .map(|&q| u.uvs.get(q as usize).copied())
                            .collect::<Option<Vec<_>>>()
                            .context("a polygon's UV index is out of range")?;
                        (Some(uv), Some(a))
                    }
                    None => (None, None),
                };
                // bit 24 of the index word = double-sided (EXE 0x800373F4: one-sided polys are culled by NCLIP)
                faces.push(Face { ids: ids.collect(), uv, attr, k, name, ds: p & 0x01000000 != 0 });
            }
        }
    }
    Ok((verts, faces))
}
