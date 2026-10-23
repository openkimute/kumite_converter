//! skel.py: pose (49 channels) -> per-row world matrices. Mirrors EXE 0x8003AD10 / 0x8003BCA8 / 0x8003AFCC.
//!
//! Works on the game's 18 matrix slots:
//!   0 root, 1 chest, 2 head, 3-6 arm A (clavicle, upper, fore, hand), 7-10 arm B,
//!   11 pelvis, 12-14 leg A (thigh, shin, foot), 15-17 leg B.
//! MESH_SLOT maps 3DMK rows to slots (EXE draw table 0x8001A074).
//! OFFSET_ROW gives the row whose joint translation feeds each slot. Joint translations are
//! stored with Z negated relative to the meshes (verified by fitting the bind pose of welded
//! seam vertices: residual ~0).

use crate::np::{self, I3, M3, V3, add, mm, mv, sub};
use crate::skin::Vert;
use anyhow::{Context, Result};
use std::collections::{BTreeSet, HashMap};
use std::f64::consts::PI;

/// (row, slot) in the Python dict's order
pub const MESH_SLOT: [(usize, usize); 19] = [
    (0, 0),
    (1, 1),
    (19, 2),
    (20, 2),
    (11, 3),
    (12, 4),
    (13, 5),
    (14, 6),
    (15, 7),
    (16, 8),
    (17, 9),
    (18, 10),
    (3, 11),
    (5, 12),
    (6, 13),
    (7, 14),
    (8, 15),
    (9, 16),
    (10, 17),
];
/// slot -> row; also SLOT_ROW ({s: r for r, s in MESH_SLOT.items() if r != 20}), which has the same values
pub const OFFSET_ROW: [usize; 18] = [0, 1, 19, 11, 12, 13, 14, 15, 16, 17, 18, 3, 5, 6, 7, 8, 9, 10];
/// slot -> parent slot (slot 0 has none)
pub const PARENT: [usize; 18] = [0, 0, 1, 1, 3, 4, 5, 1, 7, 8, 9, 0, 11, 12, 13, 11, 15, 16];
pub const ANGLE_SLOTS: [usize; 10] = [0, 1, 11, 2, 3, 7, 6, 10, 14, 17];
pub const M0: M3 = [[0.0, 0.0, -1.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]];
pub const MAIN_ROWS: [usize; 19] = [0, 1, 3, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20];

/// A world (or bind) transform: rotation, translation.
pub type Tr = (M3, V3);
/// Per row (index = row): the posed transforms.
pub type Pose = Vec<Option<Tr>>;

pub fn s32(v: u32) -> i64 {
    v as i32 as i64
}

pub fn ang(v: f64) -> f64 {
    v * 2.0 * PI / 65536.0
}

pub fn rot_zyx(ax: f64, ay: f64, az: f64) -> M3 {
    let (sx, cx) = np::sin_cos(ang(ax));
    let (sy, cy) = np::sin_cos(ang(ay));
    let (sz, cz) = np::sin_cos(ang(az));
    [
        [cy * cz, sx * sy * cz - cx * sz, cx * sy * cz + sx * sz],
        [cy * sz, sx * sy * sz + cx * cz, cx * sy * sz - sx * cz],
        [-sy, sx * cy, cx * cy],
    ]
}

#[allow(non_snake_case)]
pub fn Rz(t: f64) -> M3 {
    let (s, c) = np::sin_cos(t);
    [[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]]
}

#[allow(non_snake_case)]
pub fn Rx(t: f64) -> M3 {
    let (s, c) = np::sin_cos(t);
    [[1.0, 0.0, 0.0], [0.0, c, -s], [0.0, s, c]]
}

/// An accessory bone's neutral placement (see fit_extras): fitted bind transform, gravity-hung, or rigid.
#[derive(Clone)]
pub enum Mode {
    Fit(M3, V3),
    Hang(V3),
    Rigid(V3),
}

#[derive(Clone)]
pub struct Extra {
    pub mode: Mode,
    pub r#ref: usize,
}

impl Extra {
    pub fn t(&self) -> V3 {
        match self.mode {
            Mode::Fit(_, t) | Mode::Hang(t) | Mode::Rigid(t) => t,
        }
    }
}

/// row -> extra, in the Python dict's insertion order
pub type Extras = Vec<(usize, Extra)>;

pub fn get_extra(e: &Extras, r: usize) -> Option<&Extra> {
    e.iter().find(|x| x.0 == r).map(|x| &x.1)
}

pub struct Skeleton {
    pub rows: Vec<[u32; 14]>,
    pub t: Vec<V3>,
    pub parent: Vec<Option<usize>>,
    pub extras: Extras,
}

impl Skeleton {
    pub fn new(model: &[u8], extras: Extras) -> Result<Skeleton> {
        let (rows, _) = crate::t3m::parse(model)?;
        let n = rows.len();
        let t = rows.iter().map(|r| [s32(r[3]) as f64, s32(r[4]) as f64, -s32(r[5]) as f64]).collect();
        let bones: Vec<usize> = (0..n).filter(|i| ![2, 4, 20].contains(i)).collect();
        let parent = rows
            .iter()
            .map(|r| match usize::try_from(s32(r[6])) {
                Ok(p) => bones.get(p).copied().map(Some).context("a bone's parent is out of range"),
                Err(_) => Ok(None),
            })
            .collect::<Result<_>>()?;
        Ok(Skeleton { rows, t, parent, extras })
    }

    fn row_t(&self, r: usize) -> Result<V3> {
        self.t.get(r).copied().with_context(|| format!("the model has no row {r}"))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn ik(&self, x: &Tr, target: &[i64], off: V3, a: f64, b: f64, twist: i64, knee: bool) -> (M3, M3) {
        let (r, t) = x;
        let tg = [target[0] as f64, target[1] as f64, target[2] as f64];
        let d = sub(&mv(r, true, &sub(&tg, t)), &off);
        let l = np::norm(&d);
        if l < 1e-6 {
            return (I3, I3);
        }
        let (nx, ny, nz) = (d[0] / l, d[1] / l, d[2] / l);
        let rmin = if nx > -0.9999 {
            let k = 1.0 / (1.0 + nx);
            [[nx, -ny, -nz], [ny, 1.0 - ny * ny * k, -ny * nz * k], [nz, -ny * nz * k, 1.0 - nz * nz * k]]
        } else {
            [[-1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, 1.0]]
        };
        let u = mm(&rmin, false, &Rx(ang(twist as f64)), false);
        if l >= a + b - 1e-6 {
            return (u, I3);
        }
        let al = libm::acos(((a * a + l * l - b * b) / (2.0 * a * l)).clamp(-1.0, 1.0));
        let th = if knee { -al } else { al };
        let u = mm(&u, false, &Rz(th), false);
        let (s, c) = np::sin_cos(-th);
        (u, Rz(libm::atan2(l * s, l * c - a)))
    }

    /// P: 49 channels. Returns per row world transforms (character space, y down).
    /// springs: a Springs state to advance one frame (accessory links); else they hold neutral.
    pub fn pose(&self, p: &[i64], springs: Option<&mut Springs>) -> Result<Pose> {
        if p.len() < 49 {
            anyhow::bail!("a pose has fewer than 49 channels");
        }
        let t = |s: usize| self.row_t(OFFSET_ROW[s]);
        let mut l = [I3; 18];
        for (i, &s) in ANGLE_SLOTS.iter().enumerate() {
            l[s] = rot_zyx(p[19 + 3 * i] as f64, p[20 + 3 * i] as f64, p[21 + 3 * i] as f64);
        }
        for s in [2, 3, 7] {
            l[s] = mm(&l[s], false, &M0, false);
        }
        let comp = |x: &Tr, rl: &M3, tl: &V3| (mm(&x.0, false, rl, false), add(&mv(&x.0, false, tl), &x.1));
        let mut w: [Option<Tr>; 18] = Default::default();
        w[0] = Some((l[0], [p[0] as f64, p[1] as f64, p[2] as f64]));
        let wp = |w: &[Option<Tr>; 18], s: usize| w[PARENT[s]].context("a slot's parent is not posed");
        for s in [1, 11, 3, 7] {
            w[s] = Some(comp(&wp(&w, s)?, &l[s], &t(s)?));
        }
        let w3 = w[3].context("slot 3")?;
        let w7 = w[7].context("slot 7")?;
        let w11 = w[11].context("slot 11")?;
        (l[4], l[5]) = self.ik(&w3, &p[3..6], t(4)?, t(5)?[0], t(6)?[0], p[15], false);
        (l[8], l[9]) = self.ik(&w7, &p[6..9], t(8)?, t(9)?[0], t(10)?[0], p[16], false);
        (l[12], l[13]) = self.ik(&w11, &p[9..12], t(12)?, t(13)?[0], t(14)?[0], p[17], true);
        (l[15], l[16]) = self.ik(&w11, &p[12..15], t(15)?, t(16)?[0], t(17)?[0], p[18], true);
        for s in [2, 4, 5, 6, 8, 9, 10, 12, 13, 14, 15, 16, 17] {
            w[s] = Some(comp(&wp(&w, s)?, &l[s], &t(s)?));
        }
        let n = self.rows.len();
        let mut out: Pose = vec![None; n.max(21)];
        for (r, s) in MESH_SLOT {
            if r < n {
                out[r] = w[s];
            }
        }
        if let Some(sp) = springs {
            sp.step(self, &mut out)?;
        }
        // accessory bones (see fit_extras): fitted bind transform, gravity-hung, or rigid
        for _ in 0..8 {
            for (r, e) in &self.extras {
                if out.get(*r).is_none_or(|o| o.is_some()) {
                    continue;
                }
                let Some((rr, tt)) = out.get(e.r#ref).copied().flatten() else { continue };
                out[*r] = Some(match &e.mode {
                    Mode::Fit(er, et) => (mm(&rr, false, er, false), add(&mv(&rr, false, et), &tt)),
                    Mode::Rigid(et) => (rr, add(&mv(&rr, false, et), &tt)),
                    // hang: rotate the bone so its +X axis points down (+Y is down in PS1 space)
                    Mode::Hang(et) => (hang(&rr), add(&mv(&rr, false, et), &tt)),
                });
            }
        }
        for _ in 0..6 {
            // remaining chain links follow their parent rigidly
            for r in 0..n {
                if out[r].is_some() {
                    continue;
                }
                if let Some((rr, tt)) = self.parent[r].and_then(|p| out[p]) {
                    out[r] = Some((rr, add(&mv(&rr, false, &self.t[r]), &tt)));
                }
            }
        }
        Ok(out)
    }
}

/// Minimal rotation taking the bone's +X axis to straight down, applied on top of R.
fn hang(r: &M3) -> M3 {
    let x = [r[0][0], r[1][0], r[2][0]];
    let dn = [0.0, 1.0, 0.0];
    let c = np::dot(&x, &dn);
    let ax = [x[1] * dn[2] - x[2] * dn[1], x[2] * dn[0] - x[0] * dn[2], x[0] * dn[1] - x[1] * dn[0]];
    let s = np::norm(&ax);
    if s < 1e-6 {
        return if c > 0.0 { *r } else { mm(&[[-1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, 1.0]], false, r, false) };
    }
    let u = [ax[0] / s, ax[1] / s, ax[2] / s];
    let k = [[0.0, -u[2], u[1]], [u[2], 0.0, -u[0]], [-u[1], u[0], 0.0]];
    let kk = mm(&k, false, &k, false);
    let q: M3 = std::array::from_fn(|i| std::array::from_fn(|j| (I3[i][j] + s * k[i][j]) + (1.0 - c) * kk[i][j]));
    mm(&q, false, r, false)
}

/// A row of fit_extras's input: [parent, tx, ty, tz, has_geo, ax, ay, az].
pub type ExtraRow = [i64; 8];

/// Neutral transforms for accessory bones (hair, tails, dress panels, coats...).
/// rows: [parent, tx, ty, tz, has_geo, ax, ay, az]; verts: skin::build() output.
/// Rows 2 and 4 (extra sections such as jackets) ride on the chest and pelvis. Accessory slots
/// (rows 21+) carry their neutral Euler angles in row words 7-9; the game loads them into the bone's
/// local matrix (EXE 0x80037C5C) and runs a spring simulation around them (0x80037CBC, see
/// Springs). Remaining extras are placed by fitting welded seam vertices (Kabsch), else hang/rigid.
pub fn fit_extras(rows: &[ExtraRow], verts: &[Vert]) -> Result<Extras> {
    let pairs = weld_pairs(verts);
    let n = rows.len();
    let bones: Vec<usize> = (0..n).filter(|i| ![2, 4, 20].contains(i)).collect();
    let bone = |p: i64| bones.get(p as usize).copied().context("an accessory's parent is out of range");
    let extra: Vec<usize> = (0..n).filter(|&i| rows[i][4] != 0 && !MAIN_ROWS.contains(&i)).collect();
    // a set of small ints: Python iterates it in ascending order
    let mut placed: BTreeSet<usize> = MAIN_ROWS.into_iter().collect();
    let mut out: Extras = Vec::new();
    let has = |out: &Extras, r: usize| out.iter().any(|e| e.0 == r);
    // Rows 2/4/20 are drawn as secondary meshes of the section before them (chest, pelvis, head),
    // in that bone's local frame.
    for (r, rf) in [(2, 1), (4, 3)] {
        if extra.contains(&r) {
            out.push((r, Extra { mode: Mode::Fit(I3, [0.0; 3]), r#ref: rf }));
            placed.insert(r);
        }
    }
    for &r in &extra {
        if r >= 21 && rows[r][0] >= 0 {
            let s16 = |v: i64| if v >= 32768 { v - 65536 } else { v } as f64;
            let rr = rot_zyx(s16(rows[r][5]), s16(rows[r][6]), s16(rows[r][7]));
            let t = [rows[r][1] as f64, rows[r][2] as f64, -rows[r][3] as f64];
            out.push((r, Extra { mode: Mode::Fit(rr, t), r#ref: bone(rows[r][0])? }));
        }
    }
    for _ in 0..8 {
        for &r in &extra {
            if has(&out, r) {
                continue;
            }
            let mut best: Option<(usize, Fit)> = None;
            for &q in &placed {
                if let Some(f) = pairs.get(&(q, r)).and_then(|p| kabsch(p))
                    && f.2 < 8.0
                    && best.as_ref().is_none_or(|b| f.3 > b.1.3)
                {
                    best = Some((q, f));
                }
            }
            if let Some((q, f)) = best {
                out.push((r, Extra { mode: Mode::Fit(f.0, f.1), r#ref: q }));
                placed.insert(r);
            }
        }
    }
    for &r in &extra {
        if has(&out, r) {
            continue;
        }
        let p = rows[r][0];
        let par = if p >= 0 { bone(p)? } else { 0 };
        let t = [rows[r][1] as f64, rows[r][2] as f64, -rows[r][3] as f64];
        out.push((r, Extra { mode: if par != 14 && par != 18 { Mode::Hang(t) } else { Mode::Rigid(t) }, r#ref: par }));
    }
    Ok(out)
}

/// A local position pair: (point in bone A's frame, the same point in bone B's frame).
pub type Pair = (V3, V3);

/// {(rowA,rowB): [(pA_local, pB_local), ...]} for vertices blended between two bones.
pub fn weld_pairs(verts: &[Vert]) -> HashMap<(usize, usize), Vec<Pair>> {
    let mut pairs: HashMap<(usize, usize), Vec<Pair>> = HashMap::new();
    for v in verts.iter().flatten() {
        let mut d: Vec<(usize, V3)> = Vec::new();
        for (r, _, p) in v.iter() {
            if !d.iter().any(|e| e.0 == *r) {
                d.push((*r, p.map(|c| c as f64)));
            }
        }
        for a in &d {
            for b in &d {
                if a.0 != b.0 {
                    pairs.entry((a.0, b.0)).or_default().push((a.1, b.1));
                }
            }
        }
    }
    pairs
}

/// kabsch's result: R, t, max residual, n
pub type Fit = (M3, V3, f64, usize);

/// Fit pA = R pB + t. None if fewer than 3 pairs, or all on one line.
pub fn kabsch(p: &[Pair]) -> Option<Fit> {
    let n = p.len();
    if n < 3 {
        return None;
    }
    let mean = |k: usize| -> V3 {
        let mut s = [0.0; 3];
        for (i, e) in p.iter().enumerate() {
            let v = if k == 0 { e.0 } else { e.1 };
            s = if i == 0 { v } else { add(&s, &v) };
        }
        s.map(|c| c / n as f64)
    };
    let (ca, cb) = (mean(0), mean(1));
    let a0: Vec<f64> = p.iter().flat_map(|e| sub(&e.0, &ca)).collect();
    let b0: Vec<f64> = p.iter().flat_map(|e| sub(&e.1, &cb)).collect();
    let h = np::gemm(&b0, true, &a0, false, 3, 3, n);
    let (u, s, vt) = np::svd(&[[h[0], h[1], h[2]], [h[3], h[4], h[5]], [h[6], h[7], h[8]]]);
    // pairs on one line (often two points repeated) leave the turn about it free: the SVD's pick, not a fit
    if s[1] <= s[0] * 1e-9 {
        return None;
    }
    let m = mm(&vt, true, &u, true);
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    let sign = if det > 0.0 {
        1.0
    } else if det < 0.0 {
        -1.0
    } else {
        0.0
    };
    let d = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, sign]];
    let r = mm(&mm(&vt, true, &d, false), false, &u, true);
    let t = sub(&ca, &mv(&r, false, &cb));
    let bs: Vec<f64> = p.iter().flat_map(|e| e.1).collect();
    let rb = np::gemm(&bs, false, &[r[0], r[1], r[2]].concat(), true, n, 3, 3);
    let res = p
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let q = sub(&e.0, &add(&[rb[3 * i], rb[3 * i + 1], rb[3 * i + 2]], &t));
            (q[0] * q[0] + q[1] * q[1] + q[2] * q[2]).sqrt()
        })
        .fold(f64::NEG_INFINITY, f64::max);
    Some((r, t, res, n))
}

// ---- accessory springs (hair, tails, coat tails, skirts) -------------------------------------
// Port of EXE 0x80037CBC, run by the pose solver (0x8003B418) once per frame for slots 18-21,
// which hold rows 21-24. Each link keeps its tip from the previous frame; the new direction is a
// blend of that lagging direction, gravity and the neutral (rest) direction, turned into two
// angles on top of the neutral rotation. Parameters are per character (index = player id, which
// is the model's position in the BNS archive: (record - 71) / 4).
pub const EXE_BASE: i64 = 0x80010000 - 0x800; // PS-X EXE: file offset = address - EXE_BASE
pub const SPRING_TABLE: i64 = 0x8009623C;
pub const SPRING_NONE: u32 = 0x80095F48;
pub const SPRING_SIGNS: i64 = 0x80095D20;

/// 0x80095D10 / 0x80095D14: (hi, lo)
fn spring_clamp(c: i64) -> Option<(f64, f64)> {
    match c {
        26 => Some((4096.0, -4096.0)),
        36 | 41 => Some((8192.0, -8192.0)),
        _ => None,
    }
}

/// 4096 units per turn
pub fn ratan2(y: f64, x: f64) -> f64 {
    libm::atan2(y, x) * 4096.0 / (2.0 * PI)
}

/// Python's `math.hypot(x, y)` (3.12: a correctly rounded vector norm, not the C library's hypot).
pub fn hypot(x: f64, y: f64) -> f64 {
    let (x, y) = (x.abs(), y.abs());
    let max = x.max(y);
    if max.is_infinite() || max == 0.0 {
        return max;
    }
    if x.is_nan() || y.is_nan() {
        return f64::NAN;
    }
    let max_e = frexp_exp(max);
    if max_e < -1023 {
        return f64::MIN_POSITIVE * hypot(x / f64::MIN_POSITIVE, y / f64::MIN_POSITIVE);
    }
    let scale = 2f64.powi(-max_e);
    let (mut csum, mut frac1, mut frac2) = (1.0f64, 0.0f64, 0.0f64);
    let dl_mul = |a: f64, b: f64| {
        let z = a * b;
        (z, a.mul_add(b, -z))
    };
    let dl_fast_sum = |a: f64, b: f64| {
        let x = a + b;
        (x, b - (x - a))
    };
    for v in [x, y] {
        let v = v * scale;
        let pr = dl_mul(v, v);
        let sm = dl_fast_sum(csum, pr.0);
        csum = sm.0;
        frac1 += pr.1;
        frac2 += sm.1;
    }
    let mut h = (csum - 1.0 + (frac1 + frac2)).sqrt();
    let pr = dl_mul(-h, h);
    let sm = dl_fast_sum(csum, pr.0);
    csum = sm.0;
    frac1 += pr.1;
    frac2 += sm.1;
    let x = csum - 1.0 + (frac1 + frac2);
    h += x / (2.0 * h);
    h / scale
}

/// frexp's exponent: x = m * 2^e with 0.5 <= m < 1
fn frexp_exp(x: f64) -> i32 {
    let b = x.to_bits();
    let e = ((b >> 52) & 0x7ff) as i32;
    if e == 0 {
        // subnormal
        frexp_exp(x * 2f64.powi(64)) - 64
    } else {
        e - 1022
    }
}

#[derive(Clone)]
pub struct Spring {
    pub typ: i64,
    pub w_lag: i64,
    pub w_grav: i64,
    pub len: i64,
    pub grav: i64,
    pub collide: bool,
    pub coll_slot: i64,
    pub coll_point: V3,
    pub coll_cmp: i64,
    pub signs: Vec<[i64; 6]>,
}

/// Spring parameters for accessory links 0-3 of a character (None = no spring).
pub fn spring_params(exe: &[u8], char: i64) -> Result<Vec<Option<Spring>>> {
    let rd = |a: i64, n: usize| -> Result<&[u8]> {
        usize::try_from(a - EXE_BASE)
            .ok()
            .and_then(|o| exe.get(o..o + n))
            .context("a spring table read is outside the EXE")
    };
    let i32s = |b: &[u8]| b.as_chunks::<4>().0.iter().map(|c| i32::from_le_bytes(*c) as i64).collect::<Vec<_>>();
    let signs = (0..10)
        .map(|i| -> Result<[i64; 6]> {
            let v = i32s(rd(SPRING_SIGNS + 24 * i, 24)?);
            Ok([v[0], v[1], v[2], v[3], v[4], v[5]])
        })
        .collect::<Result<Vec<_>>>()?;
    let mut out = Vec::new();
    for c in rd(SPRING_TABLE + 24 * char, 16)?.as_chunks::<4>().0 {
        let p = u32::from_le_bytes(*c);
        if p == SPRING_NONE {
            out.push(None);
            continue;
        }
        let v: Vec<i64> = rd(p as i64, 24)?.as_chunks::<2>().0.iter().map(|c| i16::from_le_bytes(*c) as i64).collect();
        out.push(Some(Spring {
            typ: v[0],
            w_lag: v[2],
            w_grav: v[3],
            len: v[4],
            grav: v[5],
            collide: v[6] == 1,
            coll_slot: v[7],
            coll_point: [v[8] as f64, v[9] as f64, v[10] as f64],
            coll_cmp: v[11],
            signs: signs.clone(),
        }));
    }
    Ok(out)
}

/// Python's `l[i]` for a possibly negative i
fn py_get<T>(l: &[T], i: i64) -> Result<&T> {
    let k = if i < 0 { i + l.len() as i64 } else { i };
    usize::try_from(k).ok().and_then(|k| l.get(k)).context("a spring parameter indexes past its table")
}

/// Per-clip simulation state. step() runs after the main skeleton is posed.
pub struct Springs {
    pub char: i64,
    /// (link, row, params, neutral rotation)
    pub links: Vec<(usize, usize, Option<Spring>, M3)>,
    pub prev: Vec<Option<Tr>>,
}

impl Springs {
    pub fn new(sk: &Skeleton, params: &[Option<Spring>], char: i64) -> Springs {
        let mut links = Vec::new();
        for (s, p) in params.iter().enumerate() {
            let r = 21 + s;
            let Some(row) = sk.rows.get(r) else { continue };
            if row[10] == 0 {
                continue;
            }
            let a = |v: u32| {
                let v = (v & 0xffff) as i64;
                (if v >= 32768 { v - 65536 } else { v }) as f64
            };
            links.push((s, r, p.clone(), rot_zyx(a(row[7]), a(row[8]), a(row[9]))));
        }
        Springs { char, links, prev: vec![None; sk.rows.len()] }
    }

    fn angles(v: &V3, e: &[i64; 6]) -> (f64, f64) {
        let yaw = ratan2(e[0] as f64 * v[2], e[1] as f64 * v[0]) * e[4] as f64;
        let pitch = ratan2(e[2] as f64 * v[1], e[3] as f64 * hypot(v[0], v[2])) * e[5] as f64;
        (yaw, pitch)
    }

    pub fn step(&mut self, sk: &Skeleton, out: &mut Pose) -> Result<()> {
        let c = self.char;
        let get = |out: &Pose, r: usize| out.get(r).copied().flatten();
        for (s, r, p, rn) in &self.links {
            let (s, r) = (*s, *r);
            let Some((pr, pt)) = sk.parent[r].and_then(|q| get(out, q)) else { continue };
            let t = sk.t[r];
            let live = p.is_some() && !((c == 32 || c == 33) && s < 2) && self.prev[r].is_some();
            let (Some(p), true) = (p, live) else {
                // first frame / no spring: neutral pose
                out[r] = Some((mm(&pr, false, rn, false), add(&mv(&pr, false, &t), &pt)));
                self.prev[r] = out[r];
                continue;
            };
            let b = mm(&pr, false, rn, false); // neutral frame; the link points along +X
            let (p_r, p_t) = self.prev[r].context("a spring without its previous frame")?;
            let mut tip = add(&mv(&p_r, false, &[p.len as f64, 0.0, 0.0]), &p_t);
            let pivot = add(&mv(&pr, false, &t), &pt);
            let airborne = tip[1] < 0.0; // y is down; the floor is y = 0
            if !airborne {
                tip[1] = 0.0;
            }
            let d = mv(&b, true, &sub(&tip, &pivot));
            let n = np::norm(&d);
            let a = if n > 1e-9 { d.map(|x| x / n * 4096.0) } else { [4096.0, 0.0, 0.0] };
            let k = p.grav;
            let g = if airborne {
                mv(&b, true, &[(k >> 4) as f64, ((256 - k) << 4) as f64, (k >> 4) as f64])
            } else {
                [0.0; 3]
            };
            let mut v: V3 = std::array::from_fn(|i| (a[i] * p.w_lag as f64 + g[i] * p.w_grav as f64) / 256.0);
            v[0] += ((256 - p.w_lag - p.w_grav) * 16) as f64;
            let (mut typ, mut flag, mut lim) = (p.typ, false, None);
            if p.collide {
                // keep the link off a thigh (0x80038430)
                let slot = usize::try_from(p.coll_slot).ok().and_then(|s| OFFSET_ROW.get(s));
                let (lr, lt) = slot.and_then(|&q| get(out, q)).context("a spring collides with an unposed bone")?;
                let q = mv(&b, true, &sub(&add(&mv(&lr, false, &p.coll_point), &lt), &pivot));
                let mut lt = typ;
                if c == 38 && q[0] < 0.0 {
                    (lt, flag) = (8, true);
                }
                lim = Some(Self::angles(&q, py_get(&p.signs, lt)?));
            } else if (c == 36 || c == 41) && s == 0 {
                // front dress panel aims between the knees (0x8003866c)
                let ka = get(out, OFFSET_ROW[13]).context("knee A")?.1;
                let kb = get(out, OFFSET_ROW[16]).context("knee B")?.1;
                let dk = sub(&kb, &ka);
                let l = np::norm(&dk);
                if l * l >= 16384.0 {
                    let mut pt = add(&dk.map(|x| x * 128.0 / l), &ka);
                    pt[2] += 64.0;
                    let w = mv(&b, true, &sub(&pt, &p_t));
                    let nw = np::norm(&w);
                    v = w.map(|x| x / nw * 4096.0);
                }
            }
            if c == 38 && flag {
                typ = 2;
            }
            let (mut yaw, mut pitch) = Self::angles(&v, py_get(&p.signs, typ)?);
            if let Some(lim) = lim {
                let e = py_get(&p.signs, p.coll_cmp)?;
                let (a, b) = (e[5] as f64, e[4] as f64);
                if a * lim.0 + b * lim.1 < a * yaw + b * pitch {
                    (yaw, pitch) = lim;
                }
            }
            (yaw, pitch) = (yaw * 16.0, pitch * 16.0); // -> 65536 units per turn
            if [(14, 0), (27, 2), (38, 3)].contains(&(c, s)) || (c == 15 && (s == 1 || s == 2)) {
                yaw = 0.0;
            }
            if c == 38 && s == 0 && pitch < 2048.0 {
                pitch = 2048.0;
            }
            if let Some((hi, lo)) = spring_clamp(c)
                && (c == 26 || s < 4)
            {
                yaw = yaw.max(lo).min(hi);
                pitch = pitch.max(lo).min(hi);
            }
            out[r] = Some((mm(&b, false, &rot_zyx(0.0, yaw, pitch), false), pivot));
            self.prev[r] = out[r];
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// math.hypot of Python 3.12 (values from `python3 -c 'import math; print(math.hypot(x, y).hex())'`)
    #[test]
    fn hypot_as_python() {
        for (x, y, want) in
            [(1e-310, 3e-310, 0x00003a365ff2ea11), (3.7, -1.1, 0x400ee162d851db37), (0.1, 0.2, 0x3fcc9f25c5bfedda)]
        {
            assert_eq!(hypot(x, y).to_bits(), want, "{x} {y}");
        }
    }

    /// two seam points, one repeated: the rotation about their line is anyone's guess, so no fit
    #[test]
    fn kabsch_on_a_line_is_no_fit() {
        let (a, b) = ([73.0, -6.0, -74.0], [39.0, 54.0, 97.0]);
        assert!(kabsch(&[(a, a), (b, b), (a, a)]).is_none());
        assert!(kabsch(&[(a, a), (b, b), ([1.0, 2.0, 3.0], [1.0, 2.0, 3.0])]).is_some());
    }

    /// sin(ang(-5898)), the same on every platform (Python's math.sin on macOS gives one bit more)
    #[test]
    fn sin_golden() {
        let v = std::hint::black_box(-5898.0);
        assert_eq!(rot_zyx(0.0, v, 0.0)[2][0].to_bits(), 0x3fe125557dd41c96);
    }
}
