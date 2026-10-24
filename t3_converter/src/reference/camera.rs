//! camera: Tekken 3 (SLUS-00402) versus fight camera -- integer-exact reimplementation prototype.
//!
//! Reimplements (from the EXE disassembly, see docs/reference/tekken3/camera.md):
//!   0x8006465C  sample fighter positions        -> SAMP[2] = {x, y, z, side}
//!   0x80064810  yaw controller (slot 0)         -> yaw target, 2nd-order chase, pitch lag
//!   0x80064E10  framing solver ("camera key")   -> key slot {eye, pitch<<6, yaw<<6, H}
//!   0x80063D00 / 0x80064170  key blend (orbit-interpolation around a look target)
//!   0x80068394 / 0x800684B4  round-intro fly-in (slot 2) blended into slot 0
//!   0x80062B74  slot mixer, 0x80064080 apply -> RVIEW (pitch, yaw, eye) + projection H
//!
//! Units: world units (Y points DOWN, floor = 0, standing chest bone ~ -1425),
//! angles 4096 per turn; keys keep pitch/yaw shifted left by 6 ("<<6 units").
//!
//! `t3_convert camera` runs the sanity scenarios.

use crate::disc;
use anyhow::{Context, Result};
use std::sync::OnceLock;

fn off(a: u32) -> usize {
    (a - 0x80010000 + 0x800) as usize
}

pub struct Tables {
    /// sin, cos = SIN[i+1024]
    pub sin: Vec<i64>,
    /// atan helper bytes
    atn: Vec<i64>,
    intro_offs: Vec<[i64; 3]>,
}

/// The EXE's tables, read from the disc image (disc::bin()) by `FightCamera::new` (empty before).
static TABLES: OnceLock<Tables> = OnceLock::new();
static NO_TABLES: Tables = Tables { sin: Vec::new(), atn: Vec::new(), intro_offs: Vec::new() };

pub fn tables() -> &'static Tables {
    TABLES.get().unwrap_or(&NO_TABLES)
}

fn load_tables() -> Result<()> {
    if TABLES.get().is_none() {
        let t = load().with_context(|| {
            format!("the camera reads its tables from the disc image {} (set T3_BIN)", disc::bin().display())
        })?;
        let _ = TABLES.set(t);
    }
    Ok(())
}

fn load() -> Result<Tables> {
    let exe = disc::Iso::open(&conv_io::StdDisc(disc::bin()))?.read("TEKKEN3/SLUS_004.02")?;
    let h = |o: usize| -> Result<i64> {
        let b = exe.get(o..o + 2).context("the EXE is too short for the camera tables")?;
        Ok(i16::from_le_bytes([b[0], b[1]]) as i64)
    };
    let sin = (0..5120).map(|i| h(off(0x8001E8C4) + 2 * i)).collect::<Result<_>>()?;
    let atn = exe.get(off(0x800210C8)..off(0x800210C8) + 1025).context("the EXE is too short")?;
    let mut intro_offs = Vec::new();
    for i in 0..10 {
        let o = off(0x80098508) + 6 * i;
        intro_offs.push([h(o)?, h(o + 2)?, h(o + 4)?]);
    }
    Ok(Tables { sin, atn: atn.iter().map(|&b| b as i64).collect(), intro_offs })
}

// ---------------------------------------------------------------- C/MIPS integer helpers
/// MIPS div: truncate toward zero (a zero divisor, where the Python raises, gives 0)
pub fn tdiv(a: i64, b: i64) -> i64 {
    a.checked_div(b).unwrap_or(0)
}

/// "bgez; addiu (1<<n)-1; sra n" = divide by 2^n toward zero
pub fn sra_tz(a: i64, n: u32) -> i64 {
    (if a < 0 { a + (1 << n) - 1 } else { a }) >> n
}

/// "srl 31; addu; sra 1"
pub fn half(a: i64) -> i64 {
    tdiv(a, 2)
}

/// angle difference in <<6 units wrapped to [-0x20000, 0x20000)
pub fn wrap18(v: i64) -> i64 {
    let v = v & 0x3FFFF;
    if v & 0x20000 != 0 { v - 0x40000 } else { v }
}

pub fn s16(v: i64) -> i64 {
    let v = v & 0xFFFF;
    if v & 0x8000 != 0 { v - 0x10000 } else { v }
}

/// sin-table index of an angle in <<6 units
fn idx6(a6: i64) -> usize {
    (sra_tz(a6, 6) & 0xFFF) as usize
}

fn sin6(a6: i64) -> i64 {
    tables().sin[idx6(a6)]
}

fn cos6(a6: i64) -> i64 {
    tables().sin[idx6(a6) + 1024]
}

/// 0x8004B39C: returns atan2(y, x), 0..4096
pub fn ratan2(x: i64, y: i64) -> i64 {
    let atn = |q: i64| tables().atn.get(q as usize).copied().unwrap_or(0) + (q >> 1);
    if x == 0 && y == 0 {
        return 0;
    }
    let r = if x >= 0 {
        if y >= 0 {
            if x >= y { atn(tdiv(y << 10, x)) } else { 0x400 - atn(tdiv(x << 10, y)) }
        } else {
            let ny = -y;
            if x >= ny { 0x1000 - atn(tdiv(ny << 10, x)) } else { atn(tdiv(x << 10, ny)) + 0xC00 }
        }
    } else {
        let nx = -x;
        if y >= 0 {
            if nx >= y { 0x800 - atn(tdiv(y << 10, nx)) } else { atn(tdiv(nx << 10, y)) + 0x400 }
        } else if nx < -y {
            0xC00 - atn(tdiv(x << 10, y))
        } else {
            atn(tdiv(y << 10, x)) + 0x800
        }
    };
    s16(r)
}

/// 0x8002A324 (Newton, unsigned)
pub fn isqrt(n: i64) -> i64 {
    let n = n & 0xFFFFFFFF;
    if n == 0 {
        return 0;
    }
    let (mut v, mut a) = (1i64, n);
    while v < a {
        v <<= 1;
        a >>= 1;
    }
    loop {
        a = v;
        v = (n / a + a) >> 1;
        if v >= a {
            return a;
        }
    }
}

// ---------------------------------------------------------------- data structures
/// 24-byte key slot (table 0x800A02A8)
#[derive(Clone, Copy)]
pub struct Key {
    pub x: i64,
    pub y: i64,
    pub z: i64,
    pub p: i64,
    pub w: i64,
    pub h: i64,
}

impl Default for Key {
    fn default() -> Key {
        Key { x: 0, y: 0, z: 0, p: 0, w: 0, h: 500 }
    }
}

/// 48-byte per-slot state (0x800A0328 + 0x30*slot)
#[derive(Clone, Copy, Default)]
struct SlotState {
    state: i64,      // +0x00 yaw-chase state (0 idle / 1 chasing)
    moving: i64,     // +0x02 eye moved > sqrt(1000)/frame (slot 0 only)
    count: i64,      // +0x08 settle counter
    yawvel: i64,     // +0x0C yaw velocity   (yaw<<6 += yawvel>>6 per frame)
    yawacc: i64,     // +0x10 yaw acceleration
    pitch: i64,      // +0x14 pitch lag (<<6), used as slot-0 pitch
    pitch_tgt: i64,  // +0x18
    pitch_step: i64, // +0x1C
    yawerr: i64,     // +0x20 |yaw error|
    vstep: i64,      // +0x24
    vpull: i64,      // +0x28 smoothed vertical pull-back
    hpull: i64,      // +0x2C smoothed horizontal pull-back (fit width)
}

/// RVIEW (0x80064080): pitch, yaw, eye and the projection distance.
#[derive(Clone, Copy)]
pub struct RView {
    pub pitch: i64,
    pub yaw: i64,
    pub eye: [i64; 3],
    pub h: i64,
}

// ---------------------------------------------------------------- the camera
pub struct FightCamera {
    h: i64,
    hmin: i64,
    hmax: i64,
    keys: [Key; 5],
    st: [SlotState; 5],
    pub samp: [[i64; 4]; 2],
    g_vofs: i64,     // 0x80098374 (slot-1 vertical offset)
    w: [i64; 5],     // CAM+8.. weights
    wprev: [i64; 5], // CAM+0x12..
    frame: i64,
    intro: bool,
    intro_t_len: i64,
    intro_t: Option<i64>,
    intro_var: usize,
    intro_base: Key,
    pub rview: Option<RView>,
}

/// 5517: slot-0 framing distance (centre point depth)
const DIST: i64 = 0x158D;
/// projection distance; versus clamps H to [500, 500]
const H_DEFAULT: i64 = 500;

impl FightCamera {
    pub fn new(intro: bool, intro_variant: i64, first_round: bool) -> Result<FightCamera> {
        load_tables()?;
        let mut keys = [Key::default(); 5];
        // 0x80065ED8 init: slot 0 and 1 = eye(0,-1450,-8000), pitch 0, yaw 0, H
        for k in &mut keys[..2] {
            *k = Key { x: 0, y: -0x5AA, z: -0x1F40, p: 0, w: 0, h: H_DEFAULT };
        }
        Ok(FightCamera {
            h: H_DEFAULT,
            hmin: 500,
            hmax: 500,
            keys,
            st: [SlotState::default(); 5],
            samp: [[0; 4]; 2],
            g_vofs: 0,
            w: [0x1000, 0, 0, 0, 0],
            wprev: [0; 5],
            frame: 0,
            intro,
            intro_t_len: if first_round { 120 } else { 53 },
            intro_t: None,
            intro_var: intro_variant.rem_euclid(10) as usize,
            intro_base: Key::default(),
            rview: None,
        })
    }

    /// 0x8006465C (simplified: caller passes the sample values; sides -100 / 100)
    fn sample(&mut self, p1: [i64; 3], p2: [i64; 3]) {
        self.samp = [[p1[0], p1[1], p1[2], -100], [p2[0], p2[1], p2[2], 100]];
        let [a, b] = &mut self.samp;
        if a[0] == b[0] && a[2] == b[2] {
            // keep them apart
            a[0] -= 1;
            a[2] -= 1;
            b[0] += 1;
            b[2] += 1;
        }
    }

    /// 0x80064810 yaw controller -> (yaw<<6, snap)
    fn yaw_control(&mut self, snap: bool, slot: usize, frames_since_start: i64, move_cam: [i64; 2]) -> (i64, bool) {
        let key = self.keys[slot];
        let [a, b] = self.samp;
        let st = &mut self.st[slot];
        let mut ang = ratan2(b[0] - a[0], b[2] - a[2]);
        if a[3] >= b[3] {
            ang = (ang + 0x800) & 0xFFF;
        }
        let tgt = ang << 6;
        let d = wrap18(tgt - key.w);
        let sgn = d.signum();
        let d = d.abs();
        if snap {
            st.yawvel = 0;
            st.yawacc = 0;
            st.pitch = 0;
            st.pitch_tgt = 0;
            st.pitch_step = 0;
            return (tgt, true);
        }
        if slot != 0 || d > 0x1AAAA || frames_since_start < 200 {
            return (key.w, false); // frozen (side switch / round start)
        }
        let mut dyaw = 0;
        if st.state == 0 {
            st.count = if d >= 0xE38 { st.count + 1 } else { 0 };
            if st.count > 0 {
                st.state = 1;
                st.yawacc += sgn * 0x600;
                st.count = 0;
            }
        } else if st.state == 1 {
            if d < 0x38E4 {
                let w = st.yawvel;
                st.yawacc -= half(st.yawacc);
                st.yawvel = w - sra_tz(w * 14, 7);
                st.yawacc += sgn * 0x300;
                if d < 0xE39 {
                    st.yawacc -= half(st.yawacc);
                    st.count += 1;
                    if st.count >= 0x20 {
                        st.yawvel = 0;
                        st.state = 0;
                        st.count = 0;
                    }
                } else {
                    st.count = 0;
                }
            } else {
                st.yawacc += sgn * 0x600;
            }
            st.yawvel += st.yawacc;
            let lim = [(0x27, 0x200), (0x28, 0x180), (0x29, 0x80), (0x2A, 0x40)]
                .iter()
                .find(|(mc, _)| move_cam.contains(mc))
                .map_or(-1, |&(_, l)| l);
            if lim > 0 {
                let l = (lim * 56784) >> 8;
                if l < st.yawvel {
                    st.yawacc -= 0x600;
                    st.yawvel = l;
                } else if st.yawvel < -l {
                    st.yawacc += 0x600;
                    st.yawvel = -l;
                }
            } else if st.yawvel > 0xDDD0 {
                st.yawvel = 0xDDD0;
                st.yawacc -= 0x600;
            } else if st.yawvel < -0xDDD0 {
                st.yawvel = -0xDDD0;
                st.yawacc += 0x600;
            }
            dyaw = sra_tz(st.yawvel, 6);
        }
        // pitch lag (slot 0)
        st.yawerr = d;
        if d >= 0x38E3 {
            let v = (((d - 0x38E3) << 12) * 0x14923A73) >> 44;
            st.pitch_tgt = sra_tz(tables().sin[((sra_tz(v, 7) & 0x1FFE) >> 1) as usize] * 279616, 12);
        } else {
            st.pitch_tgt = 0;
        }
        if st.moving != 0 {
            let step = sra_tz(st.pitch_tgt - st.pitch, 4).clamp(-0x100, 0x100);
            st.pitch_step = step;
            st.pitch += step;
        } else {
            st.pitch_step = 0;
        }
        (key.w + dyaw, false)
    }

    /// 0x80064E10 framing solver
    fn frame_key(&mut self, pitch6: i64, yaw6: i64, flag: bool, dist: i64, slot: usize) {
        let h = self.h;
        let dist = if dist < 0x321 { 0x320 } else { dist };
        let key = self.keys[slot];
        let (s0, s5) = (sin6(-pitch6), cos6(-pitch6));
        let (s4, s3) = (sin6(-yaw6), cos6(-yaw6));
        let [a, b] = self.samp;
        let (mx, mz) = (half(a[0] + b[0]), half(a[2] + b[2]));
        let (ex, ez) = if flag {
            (mx + sra_tz(dist * cos6(yaw6 + 0x30000), 12), mz + sra_tz(dist * sin6(yaw6 + 0x30000), 12))
        } else {
            let dd = wrap18(yaw6 - key.w);
            let (c, s) = (cos6(dd), sin6(dd));
            let (ox, oz) = (key.x - mx, key.z - mz);
            (mx + sra_tz(c * ox - s * oz, 12), mz + sra_tz(s * ox + c * oz, 12))
        };
        let ey = key.y;
        let (mut xs, mut ys, mut zs, mut hds) = ([0i64; 2], [0i64; 2], [0i64; 2], [0i64; 2]);
        for (i, p) in self.samp.iter().enumerate() {
            let (dx, dz, dy) = (p[0] - ex, p[2] - ez, p[1] - ey);
            let hd = sra_tz(s4 * dx + s3 * dz, 12);
            xs[i] = sra_tz(s3 * dx - s4 * dz, 12);
            zs[i] = sra_tz(s5 * hd - s0 * dy, 12);
            ys[i] = sra_tz(s0 * hd + s5 * dy, 12);
            hds[i] = hd;
        }
        let (mut ddx, mut ddz) = (0i64, 0i64);
        // depth: centre point (mid horizontal depth, height -450) to depth = dist
        let ahd = half(hds[0] + hds[1]);
        let dyc = -0x1C2 - ey;
        let yc = sra_tz(s0 * ahd + s5 * dyc, 12);
        let a0 = dist - sra_tz(s5 * ahd - s0 * dyc, 12);
        let mut zc = dist;
        ddz -= a0;
        zs[0] += a0;
        zs[1] += a0;
        let st = &mut self.st[slot];
        // horizontal framing (150-unit margins, 280-px window, 48-px dead zone)
        let (l, r) = if xs[0] < xs[1] {
            xs[0] -= 150;
            xs[1] += 150;
            (0, 1)
        } else {
            xs[1] -= 150;
            xs[0] += 150;
            (1, 0)
        };
        let sxl = tdiv(xs[l] * h, zs[l]);
        let sxr = tdiv(xs[r] * h, zs[r]);
        if sxr - sxl >= 0x119 {
            let v1 = tdiv(h * (xs[r] - xs[l]) - 140 * (zs[r] - zs[l]), 280);
            let mut a1 = zs[l] - v1;
            if !flag {
                let prev = st.hpull;
                a1 = if prev < a1 { prev + sra_tz(a1 - prev, 5) } else { prev - sra_tz(prev - a1, 3) };
            }
            st.hpull = a1;
            ddz += a1;
            ddx = sra_tz(xs[l] - tdiv(-(140 * v1), h), 3);
            zs[0] -= a1;
            zs[1] -= a1;
            zc -= a1;
        } else {
            let mut decay = true;
            let t2 = tdiv(48 * zs[r], h);
            let a2 = tdiv(-48 * zs[l], h);
            if flag || (t2 - (xs[r] - xs[l])) < a2 {
                ddx = sra_tz(xs[l] + tdiv((xs[r] - xs[l]) * zs[l], zs[l] + zs[r]), 3);
                if flag {
                    st.hpull = 0;
                    decay = false;
                }
            } else if sxl < -0x30 {
                ddx += sra_tz(xs[l] - a2, 3);
            } else if sxr > 0x30 {
                ddx += sra_tz(xs[r] - t2, 3);
            }
            if decay {
                let prev = st.hpull;
                let n = if prev < 0 {
                    prev - sra_tz(prev, 5)
                } else if prev > 0 {
                    prev - (prev >> 3)
                } else {
                    0
                };
                st.hpull = n;
                ddz += n;
                zs[0] -= n;
                zs[1] -= n;
                zc -= n;
            }
        }
        // vertical framing
        let sy = [tdiv(ys[0] * h, zs[0]), tdiv(ys[1] * h, zs[1])];
        let top = if sy[0] < sy[1] { 0 } else { 1 };
        let (ytop, ztop) = (ys[top], zs[top]);
        let wy = sra_tz(self.samp[top][1] * s5, 12);
        let mut t1 = 100;
        if slot == 1 && pitch6 < 0 {
            t1 += 28;
        }
        let mut a2 = 0;
        if slot == 1 && pitch6 > 0 {
            if wy >= -1000 {
                a2 = tdiv((wy + 1000) * 368, 640);
            }
            if !flag {
                let pv = self.g_vofs;
                if pv < a2 && a2 - pv >= 0x1D {
                    a2 = pv + 0x1C;
                } else if a2 < pv && pv - a2 >= 0x1D {
                    a2 = pv - 0x1C;
                }
            }
            self.g_vofs = a2;
        }
        let mut a3 = sra_tz(s5 * (t1 - tdiv(450 * h, dist)), 12);
        let mut pull = (tdiv(-(112 * (ztop - zc)) - h * (ytop - yc), a3 + 0x70) - zc).max(0);
        if flag {
            pull = 0;
            st.vstep = 0;
        } else {
            let prev = st.vpull;
            if pull < prev {
                let dlt = (prev - pull).min(0x4B0);
                st.vstep = sra_tz(dlt * 120, 7) - dlt;
                pull = prev + st.vstep;
            } else if pull > prev {
                let dlt = (pull - prev).min(0x320);
                st.vstep = sra_tz(dlt, 3);
                pull = prev + st.vstep;
            }
        }
        st.vpull = pull;
        if pull > 0 {
            ddz -= pull;
            zc += pull;
        }
        if slot == 0 {
            a3 += sra_tz(tdiv(sra_tz(pull * s5, 12) * h, dist), 4);
        }
        let ddy = yc - tdiv(a3 * zc, h) + a2;
        // view-space delta -> world
        let (sp, cp) = (sin6(pitch6), cos6(pitch6));
        let hd = sra_tz(ddz * cp - ddy * sp, 12);
        let wdy = sra_tz(ddz * sp + ddy * cp, 12);
        let (sy_, cy_) = (sin6(yaw6), cos6(yaw6));
        let wdx = sra_tz(ddx * cy_ - hd * sy_, 12);
        let wdz = sra_tz(ddx * sy_ + hd * cy_, 12);
        let (nx, ny, nz) = (ex + wdx, ey + wdy, ez + wdz);
        if !flag && slot == 0 {
            let (dx, dz) = (nx - key.x, nz - key.z);
            self.st[0].moving = (dx * dx + dz * dz > 1000) as i64;
        }
        self.keys[slot] = Key { x: nx, y: ny, z: nz, p: pitch6, w: yaw6, h: key.h };
    }

    /// 0x80063D00 look target of a key (distance = mean eye->fighter distance)
    fn target(&self, k: &Key) -> (i64, [i64; 3]) {
        let (sp, cp) = (sin6(k.p), cos6(k.p));
        let yy = k.w + 0x10000;
        let (sy, cy) = (sin6(yy), cos6(yy));
        let mut d = 0;
        for p in &self.samp {
            d += isqrt((p[0] - k.x).pow(2) + (p[1] - k.y).pow(2) + (p[2] - k.z).pow(2));
        }
        d >>= 1;
        (
            d,
            [
                k.x + sra_tz(sra_tz(d * cy, 12) * cp, 12),
                k.y + sra_tz(d * sp, 12),
                k.z + sra_tz(sra_tz(d * sy, 12) * cp, 12),
            ],
        )
    }

    /// 0x80064170 blend(A, B, wA) -> slot 3 (applied)
    fn blend(&self, a: &Key, b: &Key, wa: i64) -> Key {
        let wb = 0x1000 - wa;
        let (da, ta) = self.target(a);
        let (db, tb) = self.target(b);
        let d = sra_tz(da * wa + db * wb, 12);
        let t = [0, 1, 2].map(|i| sra_tz(ta[i] * wa + tb[i] * wb, 12));
        let p = a.p + sra_tz(wrap18(b.p - a.p) * wb, 12);
        let yw = a.w + sra_tz(wrap18(b.w - a.w) * wb, 12);
        let (sp, cp) = (sin6(p), cos6(p));
        let i = ((sra_tz(yw, 6) + 0x400) & 0xFFF) as usize;
        let sin = &tables().sin;
        let e = [
            t[0] - sra_tz(sra_tz(d * sin[i + 1024], 12) * cp, 12),
            t[1] - sra_tz(d * sp, 12),
            t[2] - sra_tz(sra_tz(d * sin[i], 12) * cp, 12),
        ];
        let h = sra_tz(a.h * wa + b.h * wb, 12);
        lookat(e, t, if h > 0 { h } else { self.keys[3].h })
    }

    /// 0x80064080 apply key -> RVIEW + H
    fn apply(&mut self, k: Key) {
        self.keys[3] = k;
        self.h = self.hmin.max(self.hmax.min(k.h)); // 0x80063C64
        self.rview = Some(RView { pitch: sra_tz(k.p, 6), yaw: sra_tz(k.w, 6), eye: [k.x, k.y, k.z], h: self.h });
    }

    /// intro helpers (0x80068394 / 0x800684B4)
    fn intro_key(&mut self, t: i64) -> i64 {
        let base = self.intro_base;
        let f = tables().sin[((0xC00 + tdiv(t << 10, self.intro_t_len)) & 0xFFF) as usize] + 0x1000;
        let o = tables().intro_offs[self.intro_var];
        let eye = [base.x + sra_tz(o[0] * f, 12), base.y + sra_tz(o[1] * f, 12), base.z + sra_tz(o[2] * f, 12)];
        self.keys[2] = lookat(eye, [base.x, base.y, 0], base.h);
        let t = t - 1;
        self.intro_t = Some(t);
        if t > 0x10 { 0x1000 } else { t << 8 }
    }

    /// one frame of the versus camera director (phase 1 intro / phase 2 normal)
    pub fn step(&mut self, p1: [i64; 3], p2: [i64; 3], frames_since_start: i64, move_cam: [i64; 2]) -> Option<RView> {
        self.sample(p1, p2);
        if self.intro && self.intro_t.is_none() {
            // phase 0 -> 1
            self.slot0_update(true, frames_since_start, move_cam);
            self.intro_base = self.keys[0];
            self.intro_t = Some(self.intro_t_len);
            self.w = [0, 0, 0x1000, 0, 0];
            self.wprev = [0; 5];
        }
        match self.intro_t {
            Some(t) if self.intro && t > 0 => {
                let v = self.intro_key(t);
                self.w = [0x1000 - v, 0, v, 0, 0];
                if self.w[0] > 0 {
                    self.slot0_update(self.wprev[0] < 1, frames_since_start, move_cam);
                }
                self.mix();
                if self.w[2] <= 0 {
                    self.intro = false;
                }
            }
            _ => {
                self.w = [0x1000, 0, 0, 0, 0];
                self.slot0_update(self.wprev[0] < 1, frames_since_start, move_cam);
                self.mix();
            }
        }
        self.frame += 1;
        self.rview
    }

    /// 0x80066084(P1, P2, snap, 0x158D)
    fn slot0_update(&mut self, snap: bool, fss: i64, mc: [i64; 2]) {
        let (yaw, flag) = self.yaw_control(snap, 0, fss, mc);
        self.frame_key(self.st[0].pitch, yaw, flag, DIST, 0);
    }

    /// 0x80062B74
    fn mix(&mut self) {
        let m: i64 = (0..5).filter(|&i| self.w[i] > 0).map(|i| 1 << i).sum();
        let k = self.keys;
        match m {
            1 => self.apply(k[0]),
            2 => self.apply(k[1]),
            4 => self.apply(k[2]),
            3 => self.apply(self.blend(&k[0], &k[1], self.w[0])),
            5 => self.apply(self.blend(&k[0], &k[2], self.w[0])),
            0x10 => self.apply(k[4]),
            0x11 => {
                let ww = tables().sin[((half(self.w[0]) - 0x400) & 0xFFF) as usize];
                self.apply(self.blend(&k[0], &k[4], half(ww) + 0x800));
            }
            _ => {}
        }
        self.wprev = self.w;
    }
}

/// 0x80063EEC key from eye/target
fn lookat(e: [i64; 3], t: [i64; 3], h: i64) -> Key {
    let (dx, dy, dz) = (t[0] - e[0], t[1] - e[1], t[2] - e[2]);
    let hd = isqrt(dx * dx + dz * dz);
    Key { x: e[0], y: e[1], z: e[2], p: s16(ratan2(hd, dy)) << 6, w: s16(ratan2(dz, -dx)) << 6, h }
}

// ---------------------------------------------------------------- sanity scenarios
/// sample Y of a standing fighter: chest bone (-1425) - 290
pub const STAND_Y: i64 = -0x591 - 0x122;

fn run(frames: i64, traj: impl Fn(i64) -> ([i64; 3], [i64; 3]), intro: bool) -> Result<(FightCamera, Vec<RView>)> {
    let mut cam = FightCamera::new(intro, 0, true)?;
    let mut out = Vec::new();
    for f in 0..frames {
        let (p1, p2) = traj(f);
        out.push(cam.step(p1, p2, 1000 + f, [0, 0]).context("the camera mixed no slot")?);
    }
    Ok((cam, out))
}

fn fmt(r: &RView) -> String {
    let e = r.eye;
    format!("eye=({:6},{:6},{:6}) pitch={:5} yaw={:5} H={}", e[0], e[1], e[2], r.pitch, r.yaw, r.h)
}

/// project world point with RVIEW (pinhole, ignores the 1.739 Y-row aspect)
fn screen_x(r: &RView, p: &[i64; 4]) -> String {
    use std::f64::consts::PI;
    let e = r.eye;
    let yw = r.yaw as f64 * 2.0 * PI / 4096.0;
    let pt = r.pitch as f64 * 2.0 * PI / 4096.0;
    let (dx, dy, dz) = ((p[0] - e[0]) as f64, (p[1] - e[1]) as f64, (p[2] - e[2]) as f64);
    let hd = -yw.sin() * dx + yw.cos() * dz;
    let x = yw.cos() * dx + yw.sin() * dz;
    let z = pt.cos() * hd + pt.sin() * dy;
    let y = -pt.sin() * hd + pt.cos() * dy;
    let h = r.h as f64;
    format!("({}, {}, {})", (x * h / z).round_ties_even(), (y * h / z).round_ties_even(), z.round_ties_even())
}

fn fighter(x: i64) -> [i64; 3] {
    [x, STAND_Y, 0]
}

pub fn main(_args: &[String]) -> Result<()> {
    for sep in [1500i64, 3000, 6000] {
        let (cam, out) = run(240, |_| (fighter((-sep).div_euclid(2)), fighter(sep / 2)), false)?;
        let r = out.last().context("no frames")?;
        println!("separation {sep:5}  frame 240: {}", fmt(r));
        for (i, p) in cam.samp.iter().enumerate() {
            println!("    P{} screen(x,y,z)={}", i + 1, screen_x(r, p));
        }
    }
    // one fighter jumps (parabola, apex 1200 units above stance, 40 frames), 3000 apart
    let jump = |f: i64| {
        let mut h = 0;
        if (60..100).contains(&f) {
            let t = (f - 60) as f64 / 40.0;
            h = (4800.0 * t * (1.0 - t)) as i64;
        }
        (fighter(-1500), [1500, STAND_Y - h, 0])
    };
    let (cam, out) = run(140, jump, false)?;
    println!("jump (P2, apex 1200 up at f=80), 3000 apart:");
    for f in [59usize, 70, 80, 90, 100, 110, 139] {
        let p2 = if f == 139 { screen_x(&out[f], &cam.samp[1]) } else { String::new() };
        println!("   f={f:3} {}  P2 screen={p2}", fmt(&out[f]));
    }
    // separation grows 1500 -> 6000 over 90 frames (backdash), then holds
    let (_, out) = run(
        200,
        |f| {
            let s = 1500 + f.min(90) * 50;
            (fighter((-s).div_euclid(2)), fighter(s / 2))
        },
        false,
    )?;
    println!("separation 1500->6000 over 90 frames:");
    for f in [0usize, 30, 60, 90, 120, 199] {
        println!("   f={f:3} {}", fmt(&out[f]));
    }
    // sidestep: P2 circles 45 degrees around P1 over 60 frames at distance 2000
    let side = |f: i64| {
        let a = (f.min(60) as f64 * 0.75).to_radians();
        (fighter(0), [(2000.0 * a.cos()) as i64, STAND_Y, (2000.0 * a.sin()) as i64])
    };
    let (_, out) = run(200, side, false)?;
    println!("sidestep: axis rotates 45 deg (yaw 512 units) in 60 frames:");
    for f in [0usize, 20, 40, 60, 80, 100, 140, 199] {
        println!("   f={f:3} {}", fmt(&out[f]));
    }
    // round intro (variant 0, first round, 120 frames)
    let (_, out) = run(130, |_| (fighter(-1000), fighter(1000)), true)?;
    println!("round intro variant 0 (offset (0,0,-7500)), fighters at x=+-1000:");
    for f in [0usize, 40, 80, 100, 104, 110, 119, 129] {
        println!("   f={f:3} {}", fmt(&out[f]));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mips_integer_helpers() {
        assert_eq!((tdiv(-7, 2), tdiv(7, -2), tdiv(5, 0)), (-3, -3, 0));
        assert_eq!((sra_tz(-1, 6), sra_tz(-64, 6), sra_tz(-65, 6), sra_tz(127, 6)), (0, -1, -1, 1));
        assert_eq!((wrap18(0x20000), wrap18(-1), wrap18(0x3FFFF + 2)), (-0x20000, -1, 1));
        assert_eq!((isqrt(0), isqrt(1), isqrt(99), isqrt(100), isqrt(1 << 31)), (0, 1, 9, 10, 46340));
        assert_eq!(
            (ratan2(0, 0), ratan2(100, 0), ratan2(0, 100), ratan2(-100, 0), ratan2(0, -100)),
            (0, 0, 0x400, 0x800, 0xC00)
        );
    }
}
