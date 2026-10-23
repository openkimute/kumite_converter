//! atlas.py: a character's texture record drawn into one RGBA atlas (VRAM x * 4 by y), and texture page + CLUT
//! lookups from a polygon's UV into it.

use crate::disc::{py_slice, u32le};
use crate::tim::{self, Tim};
use anyhow::{Context, Result, bail};
use image::RgbaImage;
use std::collections::HashMap;

pub fn c15(c: u16) -> [u8; 4] {
    let (r, g, b) = ((c & 31) << 3, ((c >> 5) & 31) << 3, ((c >> 10) & 31) << 3);
    [r as u8, g as u8, b as u8, if c == 0 { 0 } else { 255 }]
}

/// Part 0 of a texture record: a TIM chain, or (the Tekken Force soldiers' shared record) a container of costume
/// sets {u32 n; n x (offset, size)}: set 0 holds the name picture and costume 0's TIMs, set k costume k's, all at the
/// same VRAM places. Returns the list of TIM chains (one for a plain chain).
pub fn costume_sets(part: &[u8]) -> Vec<&[u8]> {
    if part.len() >= 12 && u32le(part, 0) != 0x10 {
        let n = u32le(part, 0) as usize;
        if 1 < n && n <= 8 && 4 + 8 * n <= part.len() {
            let ents: Vec<(usize, usize)> =
                (0..n).map(|i| (u32le(part, 4 + 8 * i) as usize, u32le(part, 8 + 8 * i) as usize)).collect();
            if ents.iter().all(|&(o, z)| o + z <= part.len() && z > 0) {
                return ents.into_iter().map(|(o, z)| &part[o..o + z]).collect();
            }
        }
    }
    vec![part]
}

fn u16s(d: &[u8], n: usize) -> Result<Vec<u16>> {
    let b = d.get(..2 * n).context("a CLUT is too short")?;
    Ok(b.as_chunks::<2>().0.iter().map(|&c| u16::from_le_bytes(c)).collect())
}

/// The bytes of a character's texture record (5-part container) -> RGBA atlas (256 x 512 or wider) + clut -> TIM
/// map. A record of costume sets (see costume_sets) gives set 0 (the name picture, costume 0) with `costume`'s set
/// over it.
pub fn build_atlas(texrec: &[u8], costume: usize) -> Result<(RgbaImage, HashMap<u32, Tim<'_>>)> {
    if texrec.len() < 12 {
        bail!("a texture record is too short");
    }
    let (o, z) = (u32le(texrec, 4) as usize, u32le(texrec, 8) as usize);
    let sets = costume_sets(py_slice(texrec, o, o + z));
    let mut ts = tim::walk_tims(sets[0]);
    if costume != 0 && costume < sets.len() {
        ts.extend(tim::walk_tims(sets[costume]));
    }
    let w = ts.iter().map(|t| (t.img.0 as u32 + t.img.2 as u32) * 4).max().unwrap_or(0);
    let w = 512.max(w.div_ceil(256) * 256);
    let mut a = RgbaImage::new(w, 256);
    for t in &ts {
        let (x, y, tw, h, data) = t.img;
        let (x, y, tw, h) = (x as u32, y as u32, tw as usize, h as u32);
        let px = u16s(data, tw * h as usize).context("a TIM's pixels are short")?;
        let (pal, per, rep) = match (t.bpp, &t.clut) {
            (4, None) => continue,
            (4, Some(c)) => (u16s(c.4, 16)?, 4, 1),
            // 8bpp texel = 2 atlas columns
            (8, Some(c)) => (u16s(c.4, 256)?, 2, 2),
            (8, None) => bail!("an 8-bit TIM has no CLUT"),
            _ => continue,
        };
        if y + h > 256 || x * 4 + tw as u32 * 4 > w {
            bail!("a TIM lies outside the atlas");
        }
        for (k, &p) in px.iter().enumerate() {
            let (row, col) = (y + (k / tw) as u32, x * 4 + (k % tw) as u32 * 4);
            for s in 0..per {
                let i = if per == 4 { (p >> (4 * s)) & 15 } else { (p >> (8 * s)) & 255 };
                let c = c15(pal[i as usize]);
                for r in 0..rep {
                    a.put_pixel(col + (s * rep + r) as u32, row, image::Rgba(c));
                }
            }
        }
    }
    let bycl = ts
        .into_iter()
        .filter_map(|t| match t.clut {
            Some(c) if t.bpp == 4 => Some((((c.1 as u32) << 6) | (c.0 as u32 >> 4), t)),
            _ => None,
        })
        .collect();
    Ok((a, bycl))
}

pub fn atlas_uv(u: u32, v: u32, clutword: u16, bycl: &HashMap<u32, Tim>) -> Option<(u32, u32)> {
    if clutword & 0x8000 != 0 {
        // 8bpp, shared 256-colour CLUT, page 0
        return Some((u * 2 + 1, v));
    }
    let t = bycl.get(&(clutword as u32 & 0xff))?;
    let page = (t.img.0 as u32 / 64) * 64;
    Some((page * 4 + u, v))
}
