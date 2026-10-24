//! bsp2glb's textures: the miptex records (embedded in a BSP or in a WAD3 file) and their pictures.

use std::collections::HashMap;

/// A texture: its name (lower case), size, 8-bit pixels (the full-size mip) and 256-colour RGB palette.
pub struct Miptex {
    pub name: String,
    pub w: usize,
    pub h: usize,
    pub mip0: Vec<u8>,
    pub pal: Vec<u8>,
}

pub(crate) fn u32at(d: &[u8], o: usize) -> Option<u32> {
    d.get(o..o.checked_add(4)?).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

pub(crate) fn i32at(d: &[u8], o: usize) -> Option<i32> {
    u32at(d, o).map(|v| v as i32)
}

/// `cstr`: a NUL-terminated Latin-1 name, lower case.
pub fn cstr(raw: &[u8]) -> String {
    raw.split(|&b| b == 0).next().unwrap_or(&[]).iter().map(|&b| b as char).collect::<String>().to_lowercase()
}

/// `parse_miptex`: the miptex at `off`; None when its pixels live elsewhere (a WAD's) or it is cut short.
pub fn parse_miptex(buf: &[u8], off: usize) -> Option<Miptex> {
    let field = |k: usize| u32at(buf, off.checked_add(16 + 4 * k)?).map(|v| v as usize);
    let (w, h, o0, o3) = (field(0)?, field(1)?, field(2)?, field(5)?);
    if w == 0 || h == 0 || o0 == 0 {
        return None;
    }
    let mip0 = buf.get(off.checked_add(o0)?..)?.get(..w.checked_mul(h)?)?.to_vec();
    // the palette: after the last mip (an eighth of the size), past its colour count
    let pal_off = off.checked_add(o3)?.checked_add((w / 8) * (h / 8))?.checked_add(2)?;
    let pal = buf.get(pal_off..)?.get(..768)?.to_vec();
    Some(Miptex { name: cstr(buf.get(off..)?.get(..16)?), w, h, mip0, pal })
}

/// `load_wad_textures` for one WAD3 file: its miptex lumps added to `found`, a name found before kept.
pub fn add_textures(d: &[u8], found: &mut HashMap<String, Miptex>) {
    if d.get(..4) != Some(b"WAD3") {
        return;
    }
    let (Some(num), Some(ofs)) = (u32at(d, 4), u32at(d, 8)) else { return };
    for i in 0..num as usize {
        // (a directory entry: filepos, sizes, type, compression, name)
        let Some(entry) = d.get(ofs as usize..).and_then(|r| r.get(i * 32..i * 32 + 32)) else { return };
        let (Some(filepos), typ, name) = (u32at(entry, 0), entry[12], &entry[16..]) else { return };
        let key = cstr(name);
        if typ != 0x43 || found.contains_key(&key) {
            continue;
        }
        if let Some(t) = parse_miptex(d, filepos as usize) {
            found.insert(key, t);
        }
    }
}

/// `texture_image`: (RGB or, for a `{` texture, RGBA with palette index 255 transparent, channels, pixels).
pub fn image(t: &Miptex) -> (Vec<u8>, usize) {
    let alpha = t.name.starts_with('{');
    let ch = if alpha { 4 } else { 3 };
    let mut px = Vec::with_capacity(t.w * t.h * ch);
    for &i in &t.mip0 {
        let i = i as usize;
        px.extend_from_slice(&t.pal[3 * i..3 * i + 3]);
        if alpha {
            px.push(if i == 255 { 0 } else { 255 });
        }
    }
    (px, ch)
}

/// `placeholder_image`: 64 x 64 RGB, magenta and black squares.
pub fn placeholder() -> Vec<u8> {
    (0..64 * 64).flat_map(|k| if (k / 64 < 32) == (k % 64 < 32) { [255, 0, 255] } else { [0; 3] }).collect()
}
