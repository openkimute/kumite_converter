//! attackpath.py: Tekken 3 (SLUS-00402) attack-trajectory decoder.
//!
//! Re-implements what the EXE does each active frame of a move:
//!   0x80041F6C  gate: record+0x2D <= actor+0x58 <= record+0x2E  -> 0x8006A3BC
//!   0x8006A3BC  build up to 2 attack segments at actor+0x1AC / +0x1C4 from the move's descriptor
//!   0x80069714  decode one packed 5-byte point (13/13/14-bit signed fields)
//!   0x8006A230  local point -> world (yaw rotation by actor+0x2C, + root, optional pivot turn)
//!   0x8006AA9C  (start of each frame) save root + both segment end points for the swept-start case
//!   0x800401xx  actor+0xE3 (segment count) = 2 if descriptor byte 2 != 0 else 1
//!
//! CLI: `t3_convert attackpath <record> <move-index> [...]` (the disc image: disc::bin()).

use crate::disc::{self, Game, py_slice, u32le};
use anyhow::{Context, Result, bail};

pub const EXE_BASE: u32 = 0x80010000 - 0x800;

/// 0x8006A3BC jump table 0x80024344: data offset of frame f (bytes after descriptor+6):
/// code -> (bytes per frame, extra bytes in front = "initial" points for frame -1).
/// Codes 8 and 11 (and >= 15) make 0x8006A3BC return without touching the segments.
fn stride(code: u32) -> Option<(i64, i64)> {
    Some(match code {
        0 => (10, 0),
        1 => (5, 5),
        2 => (10, 10),
        3 | 4 => (15, 5),
        5 => (20, 0),
        6 | 7 | 9 => (10, 5),
        10 | 12 | 13 | 14 => (15, 0),
        _ => return None,
    })
}

pub type Point = (i64, i64, i64);

fn u8at(buf: &[u8], o: usize) -> Result<u8> {
    buf.get(o).copied().context("a move descriptor runs past its data")
}

/// Engine sin/cos (table 0x8001E8C4, 4096 entries per turn, cos = +0x800 bytes) of 16-bit angle a.
pub fn sincos(exe: &[u8], a: i64) -> Result<(i64, i64)> {
    let i = ((a >> 3) & 0x1FFE) as usize;
    let base = (0x8001E8C4 - EXE_BASE) as usize;
    let h = |o: usize| -> Result<i64> {
        let b = exe.get(o..o + 2).context("the EXE is too short for the sine table")?;
        Ok(i16::from_le_bytes([b[0], b[1]]) as i64)
    };
    Ok((h(base + i)?, h(base + i + 0x800)?))
}

/// 0x80069714: 40-bit little-endian word -> x 13 bits, y 13 bits, z 14 bits, each biased.
pub fn decode_point(buf: &[u8], p: i64) -> Point {
    let b = if p < 0 { &[][..] } else { py_slice(buf, p as usize, p as usize + 5) };
    let v = b.iter().rev().fold(0i64, |v, &x| v << 8 | x as i64);
    ((v & 0x1FFF) - 0x1000, ((v >> 13) & 0x1FFF) - 0x1000, ((v >> 26) & 0x3FFF) - 0x2000)
}

/// MIPS "if (v<0) v+=0xfff; v>>=12" (round toward zero)
fn div4096(v: i64) -> i64 {
    if v < 0 { -((-v) >> 12) } else { v >> 12 }
}

/// 0x8006A230(actor, dst, P, O). ang = actor+0x2C, diff = actor+0x2C - actor+0x0E,
/// pivot = actor+0xF68..F70. With ang = diff = 0 and O = 0 this is the identity.
pub fn to_world(exe: &[u8], p: Point, o: Point, ang: i64, diff: i64, pivot: Point) -> Result<Point> {
    let (mut ox, oy, mut oz) = o;
    if diff != 0 {
        let (s2, c2) = sincos(exe, (-diff) & 0xFFFF)?;
        let (dx, dz) = (pivot.0 - ox, pivot.2 - oz);
        ox = pivot.0 - div4096(dx * c2 - dz * s2);
        oz = pivot.2 - div4096(dx * s2 + dz * c2);
    }
    let (s, c) = sincos(exe, (-ang) & 0xFFFF)?;
    let (x, y, z) = p;
    Ok((ox + div4096(x * c) - div4096(z * s), oy + y, oz + div4096(x * s) + div4096(z * c)))
}

/// A character moveset BNS record (m+3) as stored on disc (unrelocated).
pub struct Moveset<'a> {
    pub rec: usize,
    pub d: &'a [u8],
    exe: &'a [u8],
    pub h: [u32; 16],
    pub count: u32,
}

impl<'a> Moveset<'a> {
    pub fn new(game: &'a Game, rec: usize) -> Result<Moveset<'a>> {
        let d = game.rec.get(rec).with_context(|| format!("no BNS record {rec}"))?;
        if d.len() < 64 {
            bail!("BNS record {rec} is too short for a moveset");
        }
        let h = std::array::from_fn(|i| u32le(d, 4 * i));
        Ok(Moveset { rec, d, exe: &game.exe, h, count: h[0] >> 16 })
    }

    pub fn move_(&self, i: usize) -> Result<[u32; 14]> {
        let o = self.h[2] as usize + 56 * i;
        let m = self.d.get(o..o + 56).with_context(|| format!("move {i} runs past the moveset"))?;
        Ok(std::array::from_fn(|k| u32le(m, 4 * k)))
    }

    pub fn active(&self, i: usize) -> Result<(u8, u8)> {
        let o = self.h[2] as usize + 56 * i;
        Ok((u8at(self.d, o + 0x2D)?, u8at(self.d, o + 0x2E)?))
    }

    /// -> (buffer, offset of descriptor) as 0x8002CE14 relocates record+0x28.
    pub fn descriptor(&self, i: usize) -> Result<Option<(&'a [u8], usize)>> {
        let v = self.move_(i)?[10];
        if v == 0 || v >= 0x7FF0 {
            // built-ins: table 0x80017C10 {u16 key, u32 ptr} x13
            let t = (0x80017C10 - EXE_BASE) as usize;
            for k in 0..13 {
                let e = self.exe.get(t + 8 * k..t + 8 * k + 8).context("the EXE is too short")?;
                if u16::from_le_bytes([e[0], e[1]]) as u32 == v {
                    let ptr = u32le(e, 4).checked_sub(EXE_BASE).context("a built-in descriptor below the EXE")?;
                    return Ok(Some((self.exe, ptr as usize)));
                }
            }
            return Ok(None);
        }
        let o = (self.h[10] as usize + v as usize).checked_sub(4).context("a descriptor before its record")?;
        Ok(Some((self.d, o)))
    }
}

pub struct Header {
    pub fmt: u32,
    pub code: u32,
    pub alt: u32,
}

pub fn header(buf: &[u8], o: usize) -> Result<Header> {
    let fmt = (u8at(buf, o + 4)? as u32) << 8 | u8at(buf, o + 5)? as u32;
    Ok(Header { fmt, code: fmt & 15, alt: (fmt & 0x7FFF) >> 4 })
}

pub type Segment = (Point, Point);
/// (segments [cap0, cap1], nseg = actor+0xE3, ends = what 0x8006AA9C saves for the next frame)
pub type Built = (Option<[Option<Segment>; 2]>, usize, Option<(Option<Point>, Option<Point>)>);

/// One call of 0x8006A3BC for frame offset f = actor+0x58 - record+0x2D.
/// mode = 0x800A05A8[side]: 1 when actor+0x18 (character id) is 14 (Ogre) or 15 (Mokujin).
#[allow(clippy::too_many_arguments)]
pub fn build(
    exe: &[u8],
    buf: &[u8],
    o: usize,
    f: i64,
    mode: u32,
    org: Point,
    oprev: Point,
    ang: i64,
    diff: i64,
    pivot: Point,
    prev_ends: Option<(Option<Point>, Option<Point>)>,
) -> Result<Built> {
    if u8at(buf, o)? >= 0x18 {
        return Ok((None, 0, None)); // projectile/object hit (0x80047BBC uses 0x800ADEAC/B0 objects)
    }
    let hd = header(buf, o)?;
    let code = hd.code;
    let mut alt = if mode == 1 { hd.alt } else { 0 };
    if mode == 2 && hd.fmt & 0x8000 != 0 {
        alt = (hd.fmt & 0x7FFE) >> 3; // dead code: mode 2 is never set
    }
    let Some((per, pre)) = stride(code) else {
        return Ok((None, 0, None));
    };
    let s2 = std::cell::Cell::new(o as i64 + alt as i64 + 6 + per * f + pre);
    let w = |p: Point, at: Point| to_world(exe, p, at, ang, diff, pivot);
    let nxt = || -> Result<Point> {
        let p = decode_point(buf, s2.get());
        s2.set(s2.get() + 5);
        w(p, org)
    };
    let first = f == 0 || prev_ends.is_none();
    let (pe0, pe1) = prev_ends.unwrap_or((None, None));
    let mut cap0: [Option<Point>; 2] = [None, None];
    let mut cap1: [Option<Point>; 2] = [None, None];
    // ---- first segment: table 0x80024384 ----
    if matches!(code, 1 | 4 | 6 | 7 | 2) {
        // 0x8006A6CC / 0x8006A6DC: swept point
        let back = if code == 2 { 10 } else { 5 };
        cap0[0] = if first { Some(w(decode_point(buf, s2.get() - back), oprev)?) } else { pe0 };
        cap0[1] = Some(nxt()?);
    } else {
        // 0x8006A678: two explicit points
        cap0[0] = Some(nxt()?);
        cap0[1] = Some(nxt()?);
    }
    // ---- second segment: table 0x800243C4 ----
    match code {
        2 | 3 => {
            // 0x8006A934 / 0x8006A944: second swept point
            let back = if code == 2 { 10 } else { 15 };
            cap1[0] = if first { Some(w(decode_point(buf, s2.get() - back), oprev)?) } else { pe1 };
            cap1[1] = Some(nxt()?);
        }
        4 | 5 => {
            // 0x8006A79C: two explicit points
            cap1[0] = Some(nxt()?);
            cap1[1] = Some(nxt()?);
        }
        6 | 10 => cap1 = [Some(nxt()?), cap0[1]], // 0x8006A7EC: new point -> cap0.end
        7 | 11 => cap1 = [cap0[1], Some(nxt()?)], // 0x8006A83C: cap0.end -> new point
        12 => cap1 = [Some(nxt()?), cap0[0]],     // 0x8006A87C: new point -> cap0.start
        13 => cap1 = [cap0[0], Some(nxt()?)],     // 0x8006A8A8: cap0.start -> new point
        14 => {
            // 0x8006A8E4: degenerate (point) at new point
            let p = Some(nxt()?);
            cap1 = [p, p];
        }
        9 => {
            // 0x8006A9E0: prev(?) -> cap0.start
            // f==0 re-decodes s2-5, which is cap0.end of THIS frame (looks like an EXE quirk)
            cap1[0] = if first { Some(w(decode_point(buf, s2.get() - 5), oprev)?) } else { pe1 };
            cap1[1] = cap0[0];
        }
        _ => {} // 0, 1, 8
    }
    let nseg = if u8at(buf, o + 2)? != 0 { 2 } else { 1 }; // 0x80040170..0x8004019C
    let seg = |c: [Option<Point>; 2]| c[0].zip(c[1]);
    let segs = [seg(cap0), if cap1[0].is_some() { seg(cap1) } else { None }];
    Ok((Some(segs), nseg, Some((cap0[1], cap1[1]))))
}

/// (start, end, radius)
pub type Capsule = (Point, Point, i64);

/// All active frames of move record i. roots/angles: optional per-active-frame actor root (actor+0..8) and
/// facing (actor+0x2C); default the actor stays at the origin facing 0. Gives (frame = actor+0x58,
/// [(start, end, radius), ...]) with only the segments the collision loop 0x80047CBC reads (actor+0xE3 of them).
/// radius is 0: the game tests a bare segment against each hurt sphere (0x8004770C).
pub fn capsules(
    m: &Moveset,
    i: usize,
    mode: u32,
    roots: Option<&[Point]>,
    angles: Option<&[i64]>,
) -> Result<Vec<(u32, Vec<Capsule>)>> {
    let mut out = Vec::new();
    let Some((buf, o)) = m.descriptor(i)? else {
        return Ok(out);
    };
    let (a0, a1) = m.active(i)?;
    let (mut prev, mut oprev) = (None, None);
    for (k, fr) in (a0 as u32..=a1 as u32).enumerate() {
        let org = match roots {
            Some(r) => *r.get(k).context("too few roots")?,
            None => (0, 0, 0),
        };
        let ang = match angles {
            Some(a) => *a.get(k).context("too few angles")?,
            None => 0,
        };
        let (segs, n, ends) =
            build(m.exe, buf, o, (fr - a0 as u32) as i64, mode, org, oprev.unwrap_or(org), ang, 0, (0, 0, 0), prev)?;
        prev = ends;
        oprev = Some(org);
        let segs = segs.map_or(Vec::new(), |s| s[..n].iter().flatten().map(|s| (s.0, s.1, 0)).collect());
        out.push((fr, segs));
    }
    Ok(out)
}

/// 0x8004770C (float version): attack segment a->b vs hurt sphere (centre c, radius r).
/// 1) per-axis box reject |mid - c| > |half extent| + r on x, z, y;
/// 2) clip the segment to the slab c.y - r .. c.y + r (only if a.y != b.y);
/// 3) 2-D (x,z) point-to-segment distance^2 <= r^2 (stored r^2 at sphere+0x10).
///
/// So the hurt volume is effectively a vertical cylinder of radius r and height 2r.
pub fn segment_hits_sphere(a: [f64; 3], b: [f64; 3], c: [f64; 3], r: f64) -> bool {
    for k in [0, 2, 1] {
        if (c[k] - (a[k] + b[k]) / 2.0).abs() > (b[k] - a[k]).abs() / 2.0 + r {
            return false;
        }
    }
    let (mut a, mut b) = (a, b);
    if a[1] != b[1] {
        let (lo, hi) = (c[1] - r, c[1] + r);
        let d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let (a0, b0) = (a, b);
        let at = |y: f64| {
            let t = (y - a0[1]) / d[1];
            [a0[0] + d[0] * t, y, a0[2] + d[2] * t]
        };
        if a0[1] < lo {
            a = at(lo);
        } else if a0[1] > hi {
            a = at(hi);
        }
        if b0[1] < lo {
            b = at(lo);
        } else if b0[1] > hi {
            b = at(hi);
        }
    }
    let (dx, dz) = (b[0] - a[0], b[2] - a[2]);
    let l = dx * dx + dz * dz;
    let t = if l == 0.0 { 0.0 } else { (((c[0] - a[0]) * dx + (c[2] - a[2]) * dz) / l).clamp(0.0, 1.0) };
    let (px, pz) = (a[0] + dx * t - c[0], a[2] + dz * t - c[2]);
    px * px + pz * pz <= r * r
}

/// skel::pose() space -> descriptor space
pub const POSE_TO_GAME: [[i64; 3]; 3] = [[-1, 0, 0], [0, 1, 0], [0, 0, -1]];

pub fn describe(m: &Moveset, i: usize) -> Result<String> {
    let Some((buf, o)) = m.descriptor(i)? else {
        return Ok("no descriptor".into());
    };
    let h = header(buf, o)?;
    let bytes: Vec<String> = py_slice(buf, o, o + 6).iter().map(|b| format!("{b:02x}")).collect();
    let (a0, a1) = m.active(i)?;
    Ok(format!(
        "desc {:#x} bytes {} code {} alt {:#x} active ({a0}, {a1})",
        m.move_(i)?[10],
        bytes.join(" "),
        h.code,
        h.alt
    ))
}

/// Python's `int(s, 0)`: decimal, or 0x / 0o / 0b.
fn parse_int(s: &str) -> Result<usize> {
    let t = s.trim().to_ascii_lowercase();
    let r = match t.get(..2) {
        Some("0x") => usize::from_str_radix(&t[2..], 16),
        Some("0o") => usize::from_str_radix(&t[2..], 8),
        Some("0b") => usize::from_str_radix(&t[2..], 2),
        _ => t.parse(),
    };
    r.with_context(|| format!("not a number: {s}"))
}

fn fmt_point(p: Point) -> String {
    format!("({}, {}, {})", p.0, p.1, p.2)
}

pub fn main(args: &[String]) -> Result<()> {
    let Some(rec) = args.first() else { bail!("usage: t3_convert attackpath <record> <move-index> [...]") };
    let game = Game::load(&conv_io::StdDisc(disc::bin()))?;
    let m = Moveset::new(&game, rec.parse().with_context(|| format!("not a record number: {rec}"))?)?;
    for a in &args[1..] {
        let i = parse_int(a)?;
        println!("move {i}: {}", describe(&m, i)?);
        for (fr, segs) in capsules(&m, i, 0, None, None)? {
            let s: Vec<String> =
                segs.iter().map(|s| format!("[{} -> {} r={}]", fmt_point(s.0), fmt_point(s.1), s.2)).collect();
            println!("  frame {fr:3}: {}", s.join("  "));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_points() {
        // x = 1, y = -1, z = 2 biased into 13/13/14 bits
        let v: u64 = 0x1001 | (0x0FFF << 13) | (0x2002 << 26);
        let b = v.to_le_bytes();
        assert_eq!(decode_point(&b, 0), (1, -1, 2));
        assert_eq!(decode_point(&b, -5), (-0x1000, -0x1000, -0x2000)); // Python's empty slice: 0
        assert_eq!((div4096(-4095), div4096(-4096), div4096(8191)), (0, -1, 1));
    }
}
