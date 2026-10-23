//! t3m.py: a character model (3DMK record): the 14-word row table and each row's vertex (D), polygon (B), texture
//! (C) and normal (A) blocks.

use crate::disc::py_slice;
use anyhow::{Context, Result};
use std::collections::BTreeMap;

pub fn u32(x: &[u8], o: usize) -> Result<u32> {
    let b = x.get(o..o + 4).context("the model record is too short")?;
    Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn i16at(x: &[u8], o: usize) -> Result<i16> {
    let b = x.get(o..o + 2).context("the model record is too short")?;
    Ok(i16::from_le_bytes([b[0], b[1]]))
}

/// The post-vertex weld lists (see EXE 0x80036ee4..0x80037164).
pub struct Weld {
    pub l1: Vec<u32>,
    pub l2a: Vec<u32>,
    pub l2b: Vec<u32>,
    pub l3a: Vec<u32>,
    pub l3b: Vec<u32>,
    pub l4: Vec<u32>,
}

/// A row's D block (word 12): Python's keys vref, verts, vend, H, weld, wend.
pub struct Verts {
    pub vref: [Vec<u8>; 2],
    pub verts: Vec<[i16; 3]>,
    pub vend: usize,
    pub h: u32,
    pub weld: Weld,
    pub wend: usize,
}

/// A row's polygon block (word 1): Python's keys tris, quads, gtris, gquads, pend.
pub struct Polys {
    pub tris: Vec<[u32; 2]>,
    pub quads: Vec<[u32; 2]>,
    pub gtris: Vec<[u32; 2]>,
    pub gquads: Vec<[u32; 3]>,
    pub pend: usize,
}

impl Polys {
    /// (name, each polygon's index word, corners) per list, in the order assemble.py and skin.py walk them.
    pub fn lists(&self) -> [(&'static str, Vec<u32>, usize); 4] {
        [
            ("tri", self.tris.iter().map(|e| e[0]).collect(), 3),
            ("quad", self.quads.iter().map(|e| e[0]).collect(), 4),
            ("gtri", self.gtris.iter().map(|e| e[0]).collect(), 3),
            ("gquad", self.gquads.iter().map(|e| e[0]).collect(), 4),
        ]
    }
}

/// A row's normal block (word 0): Python's keys nref, norms, nend.
pub struct Norms {
    pub nref: [Vec<u8>; 2],
    pub norms: u32,
    pub nend: usize,
}

pub struct Row {
    pub i: usize,
    /// The row's words, block pointers outside the record zeroed.
    pub w: [u32; 14],
    pub d: Option<Verts>,
    pub p: Option<Polys>,
    pub n: Option<Norms>,
}

fn refs(x: &[u8], p: &mut usize) -> Result<[Vec<u8>; 2]> {
    let mut out: [Vec<u8>; 2] = Default::default();
    for r in &mut out {
        let nr = u32(x, *p)? as usize;
        *p += 4;
        *r = py_slice(x, *p, *p + nr).to_vec();
        *p += (nr + 3) & !3;
    }
    Ok(out)
}

fn packed(x: &[u8], p: &mut usize, bits: u32) -> Result<Vec<u32>> {
    let n = u32(x, *p)?;
    *p += 4;
    let (mut vals, mut word) = (Vec::new(), 0u32);
    for _ in 0..n {
        if word == 0 {
            word = u32(x, *p)?;
            *p += 4;
        }
        vals.push(if bits == 2 { word & 3 } else { word & 0x1ff });
        word >>= bits;
    }
    Ok(vals)
}

fn words<const N: usize>(x: &[u8], p: &mut usize) -> Result<Vec<[u32; N]>> {
    let n = u32(x, *p)? as usize;
    *p += 4;
    let mut out = Vec::new();
    for _ in 0..n {
        let mut e = [0; N];
        for (k, v) in e.iter_mut().enumerate() {
            *v = u32(x, *p + 4 * k)?;
        }
        out.push(e);
        *p += 4 * N;
    }
    Ok(out)
}

/// The row table (the raw words) and each row's blocks.
pub fn parse(x: &[u8]) -> Result<(Vec<[u32; 14]>, Vec<Row>)> {
    let n = u32(x, 0)? as usize;
    let mut rows = Vec::new();
    for i in 0..n {
        let mut w = [0; 14];
        for (k, v) in w.iter_mut().enumerate() {
            *v = u32(x, 24 + 56 * i + 4 * k)?;
        }
        rows.push(w);
    }
    let lo = 24 + 56 * n;
    let mut out = Vec::new();
    for (i, w) in rows.iter().enumerate() {
        let mut w = *w;
        for k in [0, 1, 2, 12] {
            if !(w[k] as usize >= lo && (w[k] as usize) < x.len()) {
                w[k] = 0;
            }
        }
        let mut r = Row { i, w, d: None, p: None, n: None };
        if w[12] != 0 {
            let mut p = w[12] as usize + 4;
            let vref = refs(x, &mut p)?;
            let m = u32(x, p)? as usize;
            p += 4;
            let mut verts = Vec::new();
            for k in 0..m {
                verts.push([i16at(x, p + 8 * k)?, i16at(x, p + 8 * k + 2)?, i16at(x, p + 8 * k + 4)?]);
            }
            p += 8 * m;
            let (vend, h) = (p, u32(x, w[12] as usize)?);
            let weld = Weld {
                l1: packed(x, &mut p, 9)?,
                l2a: packed(x, &mut p, 9)?,
                l2b: packed(x, &mut p, 9)?,
                l3a: packed(x, &mut p, 9)?,
                l3b: packed(x, &mut p, 9)?,
                l4: packed(x, &mut p, 2)?,
            };
            r.d = Some(Verts { vref, verts, vend, h, weld, wend: p });
        }
        if w[1] != 0 {
            let mut p = w[1] as usize;
            let tris = words::<2>(x, &mut p)?;
            let quads = words::<2>(x, &mut p)?;
            let gtris = words::<2>(x, &mut p)?;
            let gquads = words::<3>(x, &mut p)?;
            r.p = Some(Polys { tris, quads, gtris, gquads, pend: p });
        }
        if w[0] != 0 {
            let mut p = w[0] as usize + 4;
            let nref = refs(x, &mut p)?;
            let m = u32(x, p)?;
            r.n = Some(Norms { nref, norms: m, nend: p + 4 + 8 * m as usize });
        }
        out.push(r);
    }
    Ok((rows, out))
}

/// `python3 t3m.py MODEL`: the row table, one line per row.
pub fn main(args: &[String]) -> Result<()> {
    let path = args.first().context("usage: t3m MODEL_RECORD")?;
    let x = std::fs::read(path).with_context(|| format!("cannot read {path}"))?;
    let signed = |v: u32| v as i32;
    for (r, w) in parse(&x)?.1.iter().map(|r| (r, r.w)) {
        let lens = |a: &[Vec<u8>; 2]| format!("[{}, {}]", a[0].len(), a[1].len());
        let d =
            r.d.as_ref().map_or(String::new(), |d| format!("V:{} ref{} vend={}", d.verts.len(), lens(&d.vref), d.vend));
        let n = r.n.as_ref().map_or(String::new(), |n| format!("N:{} ref{} nend={}", n.norms, lens(&n.nref), n.nend));
        let p = r.p.as_ref().map_or(String::new(), |p| {
            let c = w[2] as i64 + 8;
            format!(
                "T:{} Q:{} GT:{} GQ:{} pend={} C={} gap={}",
                p.tris.len(),
                p.quads.len(),
                p.gtris.len(),
                p.gquads.len(),
                p.pend,
                c,
                c - p.pend as i64
            )
        });
        let par = if w[6] < 1 << 31 { w[6] as i64 } else { -1 };
        println!("{:2} par={par:2} t=({},{},{}) {d} {n} {p}", r.i, signed(w[3]), signed(w[4]), signed(w[5]));
    }
    Ok(())
}

/// A bone's C block (texture info), Python's dict keys: per-polygon entries are byte strings (an attribute index,
/// then one UV index per corner), clamped to the block as Python slices.
pub struct Uv {
    pub attrs: Vec<u16>,
    pub uvs: Vec<(u8, u8)>,
    pub tri: Vec<Vec<u8>>,
    pub quad: Vec<Vec<u8>>,
    pub gtri: Vec<Vec<u8>>,
    pub gquad: Vec<Vec<u8>>,
    pub end: usize,
    pub cend: usize,
}

impl Uv {
    /// `u[name]`
    pub fn list(&self, name: &str) -> &[Vec<u8>] {
        match name {
            "tri" => &self.tri,
            "quad" => &self.quad,
            "gtri" => &self.gtri,
            _ => &self.gquad,
        }
    }
}

/// Parse a bone's C block (texture info). None when no polygon lists match the row's counts.
pub fn parse_uv(x: &[u8], r: &Row, cend: usize) -> Result<Option<Uv>> {
    let pl = r.p.as_ref().context("parse_uv on a row without polygons")?;
    let c0 = pl.pend;
    let blk = py_slice(x, c0, cend);
    let le16 = |o: usize| blk.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
    let nb = le16(0).context("a texture block is too short")? as usize;
    let attrs =
        (0..nb / 2).map(|k| le16(2 + 2 * k)).collect::<Option<Vec<_>>>().context("a texture block is too short")?;
    let (t, q, g, h) = (pl.tris.len(), pl.quads.len(), pl.gtris.len(), pl.gquads.len());
    let fits = |o: usize| -> bool {
        let mut p = o;
        for (n, size) in [(t, 4), (q, 5), (g, 4), (h, 5)] {
            if blk.get(p).map(|&b| b as usize) != Some(n) {
                return false;
            }
            p += 1 + size * n;
        }
        true
    };
    for o in (2 + nb..blk.len()).step_by(2) {
        if !fits(o) {
            continue;
        }
        let uvs = (2 + nb..o).step_by(2).map(|k| (blk[k], blk.get(k + 1).copied().unwrap_or(0))).collect();
        let mut q0 = o + 1;
        let mut take = |n: usize, size: usize, skip: usize| {
            let l: Vec<Vec<u8>> = (0..n).map(|k| py_slice(blk, q0 + size * k, q0 + size * k + size).to_vec()).collect();
            q0 += size * n + skip;
            l
        };
        let tri = take(t, 4, 1);
        let quad = take(q, 5, 1);
        let gtri = take(g, 4, 1);
        let gquad = take(h, 5, 0);
        return Ok(Some(Uv { attrs, uvs, tri, quad, gtri, gquad, end: c0 + q0, cend }));
    }
    Ok(None)
}

/// C block end = start of the next block (this row's D count word, or next row's A).
pub fn cends(x: &[u8], out: &[Row]) -> Result<BTreeMap<usize, usize>> {
    let mut res = BTreeMap::new();
    for r in out {
        let Some(p) = &r.p else { continue };
        let mut cands = Vec::new();
        if r.w[12] != 0 {
            cands.push(r.w[12] as usize + 4);
        }
        if let Some(j) = out[r.i + 1..].iter().find(|j| j.w[0] != 0) {
            cands.push(j.w[0] as usize + 4);
        }
        cands.push(x.len());
        let c = cands.into_iter().filter(|&c| c > p.pend).min().context("a texture block has no end")?;
        res.insert(r.i, c);
    }
    Ok(res)
}
