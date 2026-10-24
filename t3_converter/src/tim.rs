//! tim: a sequential chain of PS1 TIM images.

use crate::disc::u32le;

/// A TIM block: VRAM x, y, width (in 16-bit units), height, and its data (clamped to the input, as Python slices).
pub type Block<'a> = (u16, u16, u16, u16, &'a [u8]);

pub struct Tim<'a> {
    pub off: usize,
    pub bpp: u32,
    pub clut: Option<Block<'a>>,
    pub img: Block<'a>,
}

fn block(p: &[u8], q: usize) -> Option<(u32, Block<'_>)> {
    let h = p.get(q..q + 12)?;
    let u16at = |o: usize| u16::from_le_bytes([h[o], h[o + 1]]);
    let bl = u32le(h, 0);
    Some((bl, (u16at(4), u16at(6), u16at(8), u16at(10), crate::disc::py_slice(p, q + 12, q + bl as usize))))
}

/// Sequential TIM chain. Skips leading non-TIM bytes by scanning.
pub fn walk_tims(p: &[u8]) -> Vec<Tim<'_>> {
    let mut out = Vec::new();
    let mut o = 0;
    while o + 8 <= p.len() {
        let fl = p[o + 4];
        if u32le(p, o) != 0x10 || ![0, 1, 2, 3, 8, 9, 10, 11].contains(&fl) {
            o += 4;
            continue;
        }
        let mut q = o + 8;
        let bpp = [4, 8, 16, 24][(fl & 3) as usize];
        let size = |b: &Block| 12 + b.2 as u64 * b.3 as u64 * 2;
        let mut clut = None;
        if fl & 8 != 0 {
            // a short header is Python's struct.error: the walk ends
            let Some((bl, c)) = block(p, q) else { break };
            if bl as u64 != size(&c) {
                o += 4;
                continue;
            }
            clut = Some(c);
            q += bl as usize;
        }
        let Some((bl, img)) = block(p, q) else { break };
        if bl as u64 != size(&img) || img.2 == 0 || img.3 == 0 {
            o += 4;
            continue;
        }
        q += bl as usize;
        out.push(Tim { off: o, bpp, clut, img });
        o = q;
    }
    out
}
