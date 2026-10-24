//! anim: port of Tekken 3 (SLUS-00402) animation keyframe decoder (EXE 0x80038B4C / 0x800387C8).
//!
//! The MIPS code's arithmetic is kept: values are i64 (Python ints) wrapped to 32 bits where the original's
//! registers wrap (`i32`).

use anyhow::{Context, Result};

pub const EXE_BASE: i64 = 0x80010000 - 0x800;

pub fn i32(v: i64) -> i64 {
    v as i32 as i64
}

pub fn s16(v: i64) -> i64 {
    v as i16 as i64
}

/// `x << k` as a Python int, then wrapped to 32 bits.
fn shl32(x: i64, k: u32) -> i64 {
    if k >= 64 { 0 } else { i32(x.wrapping_shl(k)) }
}

/// A little-endian u16 at a (possibly negative or out of range) offset.
pub fn u16at(m: &[u8], o: i64) -> Result<i64> {
    let o = usize::try_from(o).context("a motion read before its data")?;
    let b = m.get(o..o + 2).context("a motion runs past its data")?;
    Ok(u16::from_le_bytes([b[0], b[1]]) as i64)
}

pub struct Tables<'a> {
    pub exe: &'a [u8],
    pub chparam: Vec<u8>,
    pub w: i64,
}

impl<'a> Tables<'a> {
    pub fn new(exe: &'a [u8]) -> Result<Tables<'a>> {
        let o = (0x80096c24 - EXE_BASE) as usize;
        let chparam = exe.get(o..o + 49).context("the EXE is too short for the motion tables")?.to_vec();
        Ok(Tables { exe, chparam, w: 0x8009671c })
    }
    pub fn lhu(&self, addr: i64) -> Result<i64> {
        u16at(self.exe, addr - EXE_BASE)
    }
    pub fn lbu(&self, addr: i64) -> Result<i64> {
        let o = usize::try_from(addr - EXE_BASE).ok();
        Ok(*o.and_then(|o| self.exe.get(o)).context("a motion table read is outside the EXE")? as i64)
    }
}

pub fn curve(t_: &Tables, mem: &[u8], mut p: i64, t: i64, n: i64, k: u32) -> Result<i64> {
    let lh = |q: i64| u16at(mem, q).map(s16);
    let a2 = n - 1;
    let (mut t6, mut t7) = (0, 0);
    let mut t0 = lh(p)?;
    let mut t4 = lh(p + 2)?;
    let mut t1 = t0;
    p += 4;
    let mut t3 = shl32(t4 >> 5, k);
    t3 = i32(t3 + t1);
    t4 &= 0x1f;
    let mut t2 = 0;
    let mut first = true;
    loop {
        if !first {
            t6 = t7;
            t0 = t1;
            t1 = t2;
        }
        first = false;
        t7 = t7 + t4 + 1;
        t2 = t3;
        if t7 == a2 {
            break;
        }
        t4 = lh(p)?;
        p += 2;
        let t5 = shl32(t4 >> 5, k);
        t3 = i32(t3 + t5);
        let t5 = t - t7;
        t4 &= 0x1f;
        if t5 > 0 {
            continue;
        }
        break;
    }
    let (mut a1, mut a3) = (t - t6, t7 - t6);
    let t9 = t_.w;
    if a1 != 0 {
        if a3 - 16 > 0 {
            a1 = t_.lbu(t9 - (a3 << 5) - a1 + 0x420)?;
        } else {
            a1 = t_.lbu(t9 + (a3 << 5) + a1 - 0x21)?;
        }
        a1 <<= 1;
    }
    let (t8, t9) = (t9 + a1, t9 - a1);
    let v0;
    if t6 == 0 {
        a3 = t_.lhu(t9 + 0x300)?;
        let lo = i32(a3 * (t3 - t1));
        a1 = t_.lhu(t9 + 0x504)?;
        v0 = i32(lo + i32(a1 * (t2 - t1)));
    } else {
        let a0 = t_.lhu(t8 + 0x200)?;
        t0 -= t1;
        let lo = i32(a0 * t0);
        a3 = t_.lhu(t9 + 0x300)?;
        if t7 == a2 {
            a1 = t_.lhu(t9 + 0x504)?;
            a3 += 0x2aab;
            a1 += a3;
            v0 = i32(lo + i32(a1 * (t2 - t1)));
        } else {
            let v1 = lo;
            let lo = i32(a3 * (t3 - t1));
            a1 = t_.lhu(t9 + 0x402)?;
            let v0_ = i32(lo + v1);
            v0 = i32(v0_ + i32(a1 * (t2 - t1)));
        }
    }
    Ok(i32((v0 >> 16) + t1))
}

/// mem: the motion bank; base: offset of motion header. Returns 49 s16 pose channels.
pub fn decode(t_: &Tables, mem: &[u8], base: usize, frame: i64) -> Result<Vec<i64>> {
    let base = base as i64;
    let h = u16at(mem, base)?;
    let n = h & 0xff;
    let mut s0 = h >> 8;
    let s5 = frame.min(n - 1).max(0);
    let s7 = n < 10;
    let (mut s6, mut s1) = (base + 2, base + 0x32);
    let mut out = Vec::with_capacity(49);
    for ch in 0..49 {
        let a1 = if ch & 1 != 0 { n - (s5 + 1) } else { s5 };
        let v = if s0 == 0 {
            0
        } else if s0 == 1 {
            u16at(mem, s1)? << 1
        } else if s0 == 2 {
            let mut m = i32(s16(u16at(mem, s1 + 2)?) * a1);
            if !s7 {
                m >>= 5;
            }
            (u16at(mem, s1)? + m) << 1
        } else if s0 == n {
            u16at(mem, s1 + 2 * a1)? << 1
        } else {
            curve(t_, mem, s1, a1, n, t_.chparam[ch] as u32)? << 1
        };
        out.push(s16(v));
        s1 += 2 * s0;
        s0 = *usize::try_from(s6).ok().and_then(|o| mem.get(o)).context("a motion runs past its data")? as i64;
        s6 += 1;
    }
    Ok(out)
}

pub fn frames(mem: &[u8], base: usize) -> Result<usize> {
    Ok((u16at(mem, base as i64)? & 0xff) as usize)
}

/// Motions are [u16 prefix][u16 hdr][48 counts][keys] chained; returns header offsets.
pub fn find_motions(mem: &[u8], lo: usize, hi: usize) -> Result<Vec<usize>> {
    let ok = |o: usize| -> Result<Option<usize>> {
        if o + 50 > hi {
            return Ok(None);
        }
        let h = u16at(mem, o as i64)? as usize;
        let n = h & 0xff;
        let counts = crate::disc::py_slice(mem, o + 2, o + 50);
        let c = std::iter::once(h >> 8).chain(counts.iter().map(|&k| k as usize));
        if n < 2 || c.clone().any(|k| k > n) {
            return Ok(None);
        }
        let e = o + 50 + 2 * c.sum::<usize>();
        Ok((e <= hi).then_some(e))
    };
    let (mut res, mut o) = (Vec::new(), lo + 2);
    while o < hi {
        if let Some(e) = ok(o)? {
            res.push(o);
            o = e + 2;
            continue;
        }
        o += 2;
    }
    Ok(res)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// T3_BIN=<the disc image>: every motion of records 74, 186 and 279 at every frame (and one past each end)
    /// checksummed as anim gives it.
    #[test]
    fn decode_as_python() -> Result<()> {
        let Ok(bin) = std::env::var("T3_BIN") else {
            return Ok(());
        };
        let game = crate::disc::Game::load(&conv_io::StdDisc(bin.into()))?;
        let t_ = Tables::new(&game.exe)?;
        let want = [
            (74, 166, 27157214, 622594, 606758429620111840u64),
            (186, 158, 21825828, 626122, 1295479939416068732),
            (279, 363, 45593984, 847651, 1199475411031071114),
        ];
        for (r, count, sum, n, acc) in want {
            let m = &game.rec[r];
            let h = |k: usize| crate::disc::u32le(m, 4 * k) as usize;
            let mo = find_motions(m, h(7), h(8))?;
            let (mut a, mut k) = (0u64, 0);
            for &o in &mo {
                for f in -1..=frames(m, o)? as i64 {
                    for v in decode(&t_, m, o, f)? {
                        a = ((a as u128 * 31 + v.rem_euclid(1 << 61) as u128) % (1 << 61)) as u64;
                        k += 1;
                    }
                }
            }
            assert_eq!((mo.len(), mo.iter().sum::<usize>(), k, a), (count, sum, n, acc), "record {r}");
        }
        Ok(())
    }
}
