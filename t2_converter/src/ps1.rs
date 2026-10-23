//! PS1 formats shared by the Tekken 2 converter: the disc's ISO 9660 files, TIM pictures, the glTF writer, SPU ADPCM /
//! VAB banks, CD-XA streams and the LZ streams. (Copied from the Tekken 3 converter so the two stay separate.)

use crate::pyjson;
use anyhow::{Context, Result, bail};
use conv_io::{DiscFile, ReadSeek, Vfs, join};
use serde_json::{Map, Value, json};
use std::io::{Read, SeekFrom};

pub const SECTOR: u64 = 2352; // raw CD sector
pub const HDR: u64 = 24; // Mode 2 header + subheader
pub const RAW: u64 = 2352;
pub const XA_SAMPLES_PER_SECTOR: usize = 2016; // 37800 Hz stereo 4-bit: 18 groups x 112 sample pairs
pub const FPS: f64 = 60.0; // game frame rate; one motion frame per video frame
pub const SCALE: f64 = 0.001; // game units are ~1 mm

// ---- reading bytes (struct.unpack_from: a short buffer is an error) -------------------------------------------------
fn short(o: usize, n: usize, len: usize) -> anyhow::Error {
    anyhow::anyhow!("a read of {n} bytes at {o:#x} runs past the data ({len:#x} bytes)")
}
pub fn bytes<const N: usize>(d: &[u8], o: usize) -> Result<[u8; N]> {
    d.get(o..o.wrapping_add(N)).and_then(|b| b.try_into().ok()).ok_or_else(|| short(o, N, d.len()))
}
pub fn u8_(d: &[u8], o: usize) -> Result<u8> {
    d.get(o).copied().ok_or_else(|| short(o, 1, d.len()))
}
pub fn u16_(d: &[u8], o: usize) -> Result<u16> {
    Ok(u16::from_le_bytes(bytes(d, o)?))
}
pub fn i16_(d: &[u8], o: usize) -> Result<i16> {
    Ok(i16::from_le_bytes(bytes(d, o)?))
}
pub fn u32_(d: &[u8], o: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(bytes(d, o)?))
}
pub fn i32_(d: &[u8], o: usize) -> Result<i32> {
    Ok(i32::from_le_bytes(bytes(d, o)?))
}

/// Python's `b[a:b]`: clamped to the data, empty when reversed.
pub fn py_slice(d: &[u8], a: usize, b: usize) -> &[u8] {
    let a = a.min(d.len());
    let b = b.min(d.len());
    if a >= b { &[] } else { &d[a..b] }
}

/// Python's `round(x, nd)`: the correctly rounded decimal (half to even), as a float.
pub fn py_round(x: f64, nd: usize) -> f64 {
    format!("{x:.nd$}").parse().unwrap_or(x)
}

// ---- the disc --------------------------------------------------------------------------------------------------------
pub struct Iso {
    f: Box<dyn ReadSeek>,
    /// Every file, in the directory walk's order: path, first sector, size.
    pub files: Vec<(String, u32, u32)>,
}

impl Iso {
    pub fn open(disc: &dyn DiscFile) -> Result<Iso> {
        let f = disc.open().with_context(|| format!("cannot open the disc image {}", disc.name()))?;
        let mut iso = Iso { f, files: Vec::new() };
        let pvd = iso.sector(16)?;
        if pvd.get(1..6) != Some(b"CD001") {
            bail!("{} is not a raw (2352-byte sector) CD image with ISO 9660 data", disc.name());
        }
        let root = pvd[156..190].to_vec();
        iso.walk(u32_(&root, 2)?, u32_(&root, 10)?, String::new(), 0)?;
        Ok(iso)
    }

    /// One sector's 2048 bytes of user data (short at the end of the image).
    pub fn sector(&mut self, n: u64) -> Result<Vec<u8>> {
        self.at(n * SECTOR + HDR, 2048)
    }

    /// `n` bytes at a byte offset of the image (short at its end).
    pub fn at(&mut self, offset: u64, n: u64) -> Result<Vec<u8>> {
        self.f.seek(SeekFrom::Start(offset))?;
        let mut buf = Vec::with_capacity(n as usize);
        (&mut self.f).take(n).read_to_end(&mut buf)?;
        Ok(buf)
    }

    fn walk(&mut self, lba: u32, size: u32, path: String, depth: u32) -> Result<()> {
        if depth > 16 {
            bail!("the disc's directories nest too deep");
        }
        let mut data = Vec::new();
        for i in 0..size.div_ceil(2048) {
            data.extend(self.sector(lba as u64 + i as u64)?);
        }
        let mut o = 0;
        while o < data.len() {
            let l = data[o] as usize;
            if l == 0 {
                o = (o / 2048 + 1) * 2048;
                continue;
            }
            let r = data.get(o..o + l).context("a directory record runs past its directory")?;
            o += l;
            if r.len() < 34 {
                bail!("a directory record is too short");
            }
            let (ext, sz) = (u32_(r, 2)?, u32_(r, 10)?);
            let name = r.get(33..33 + r[32] as usize).context("a directory record's name runs past it")?;
            if name == b"\0" || name == b"\x01" {
                continue;
            }
            let n = String::from_utf8_lossy(name);
            let n = n.split(';').next().unwrap_or("").to_string();
            if r[25] & 2 != 0 {
                self.walk(ext, sz, format!("{path}{n}/"), depth + 1)?;
            } else {
                self.files.push((format!("{path}{n}"), ext, sz));
            }
        }
        Ok(())
    }

    pub fn find(&self, name: &str) -> Result<(u32, u32)> {
        self.files
            .iter()
            .find(|f| f.0 == name)
            .map(|f| (f.1, f.2))
            .with_context(|| format!("{name} is not on the disc (is it Tekken 2 USA v1.1, SLUS-00213?)"))
    }

    pub fn read_file(&mut self, lba: u32, size: u32) -> Result<Vec<u8>> {
        let mut buf = Vec::with_capacity(size as usize + 2048);
        for i in 0..size.div_ceil(2048) {
            buf.extend(self.sector(lba as u64 + i as u64)?);
        }
        buf.truncate(size as usize);
        Ok(buf)
    }

    pub fn read(&mut self, name: &str) -> Result<Vec<u8>> {
        let (lba, size) = self.find(name)?;
        self.read_file(lba, size)
    }
}

// ---- TIM pictures ----------------------------------------------------------------------------------------------------
/// A TIM's CLUT or image block: x, y, w, h (VRAM units) and its words.
#[derive(Clone)]
pub struct Block {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
    pub px: Vec<u8>,
}

#[derive(Clone)]
pub struct Tim {
    pub off: usize,
    pub bpp: u32,
    pub clut: Option<Block>,
    pub img: Option<Block>,
}

fn block_at(p: &[u8], q: usize) -> Option<(u32, Block)> {
    let bl = u32_(p, q).ok()?;
    let [x, y, w, h] = [q + 4, q + 6, q + 8, q + 10].map(|o| u16_(p, o).unwrap_or(0));
    u16_(p, q + 10).ok()?;
    let end = (q as u64 + bl as u64).min(usize::MAX as u64) as usize;
    Some((bl, Block { x, y, w, h, px: py_slice(p, q + 12, end).to_vec() }))
}

/// Sequential TIM chain; skips leading non-TIM bytes by scanning.
pub fn walk_tims(p: &[u8]) -> Vec<Tim> {
    let mut out = Vec::new();
    let mut o = 0;
    while o + 8 <= p.len() {
        let fl = p[o + 4];
        if u32_(p, o).unwrap_or(0) != 0x10 || ![0, 1, 2, 3, 8, 9, 10, 11].contains(&fl) {
            o += 4;
            continue;
        }
        let mut q = o + 8;
        let mut t = Tim { off: o, bpp: [4, 8, 16, 24][(fl & 3) as usize], clut: None, img: None };
        if fl & 8 != 0 {
            let Some((bl, b)) = block_at(p, q) else { break };
            if bl as u64 != 12 + b.w as u64 * b.h as u64 * 2 {
                o += 4;
                continue;
            }
            t.clut = Some(b);
            q += bl as usize;
        }
        let Some((bl, b)) = block_at(p, q) else { break };
        if bl as u64 != 12 + b.w as u64 * b.h as u64 * 2 || b.w == 0 || b.h == 0 {
            o += 4;
            continue;
        }
        t.img = Some(b);
        q += bl as usize;
        out.push(t);
        o = q;
    }
    out
}

/// The LZ streams (0x8006C46C): Ok((the bytes, or None when it is no stream, the end)); Err when it reads past the
/// source (Python's IndexError).
#[allow(clippy::result_unit_err)]
pub fn lz(src: &[u8], pos: usize, limit: usize) -> Result<(Option<Vec<u8>>, usize), ()> {
    let at = |p: usize| src.get(p).copied().ok_or(());
    let mut pos = pos;
    let mut out: Vec<u8> = Vec::new();
    let mut flags = at(pos)? as u32;
    pos += 1;
    if flags == 0 {
        return Ok((None, pos));
    }
    loop {
        while flags >= 2 {
            if flags & 1 != 0 {
                out.push(at(pos)?);
                pos += 1;
            } else {
                let w = (at(pos)? as usize) << 8 | at(pos + 1)? as usize;
                pos += 2;
                let dist = match w & 0x7FF {
                    0 => 0x800,
                    d => d,
                };
                let n = match (w >> 11) & 0x1F {
                    0 => 0x20,
                    n => n,
                };
                if out.len() < dist {
                    return Ok((None, pos));
                }
                let s = out.len() - dist;
                for k in 0..n {
                    out.push(out[s + k]);
                }
            }
            flags >>= 1;
            if out.len() > limit {
                return Ok((None, pos));
            }
        }
        flags = at(pos)? as u32;
        pos += 1;
        if flags == 0 {
            return Ok((Some(out), pos));
        }
    }
}

pub const LZ_LIMIT: usize = 1 << 21;

/// A decoded LZ stream that is one TIM (as walk_tims'), else None. Some TIMs are only a CLUT (palettes loaded on
/// their own), and some carry a wrong image block length (the select screen's chrome plates): the block size is
/// taken from its width and height instead.
pub fn valid_tim(b: &[u8]) -> Option<Tim> {
    if b.len() < 20 || b[..4] != [0x10, 0, 0, 0] || ![0, 1, 2, 3, 8, 9, 10, 11].contains(&b[4]) {
        return None;
    }
    let fl = b[4];
    let mut q = 8;
    let mut t = Tim { off: 0, bpp: [4, 8, 16, 24][(fl & 3) as usize], clut: None, img: None };
    let keys: &[bool] = if fl & 8 != 0 { &[true, false] } else { &[false] };
    for &is_clut in keys {
        if q + 12 > b.len() {
            break;
        }
        let [x, y, w, h] = [4, 6, 8, 10].map(|k| u16_(b, q + k).unwrap_or(0));
        let n = 12 + w as usize * h as usize * 2;
        if w == 0 || h == 0 || x >= 1024 || y >= 512 || q + n > b.len() {
            return None;
        }
        let blk = Block { x, y, w, h, px: b[q + 12..q + n].to_vec() };
        if is_clut {
            t.clut = Some(blk);
        } else {
            t.img = Some(blk);
        }
        q += n;
    }
    if t.img.is_some() || t.clut.is_some() { Some(t) } else { None }
}

/// Every LZ stream in d that decodes to a TIM: (offset, end, tim).
pub fn scan_lz_tims(d: &[u8]) -> Vec<(usize, usize, Tim)> {
    let mut found = Vec::new();
    let mut o = 0;
    while o + 8 < d.len() {
        // the TIM magic 10 00 00 00 starts with a literal 0x10, so the first flag bit is 1 and byte 1 is 0x10
        if d[o] & 1 != 0
            && d[o + 1] == 0x10
            && let Ok((Some(out), end)) = lz(d, o, LZ_LIMIT)
            && let Some(t) = (!out.is_empty()).then(|| valid_tim(&out)).flatten()
        {
            found.push((o, end, t));
            o = end;
            continue;
        }
        o += 1;
    }
    found
}

/// An RGBA picture (rows of w x 4 bytes).
#[derive(Clone, PartialEq)]
pub struct Rgba {
    pub w: usize,
    pub h: usize,
    pub px: Vec<u8>,
}

impl Rgba {
    pub fn new(w: usize, h: usize) -> Rgba {
        Rgba { w, h, px: vec![0; w * h * 4] }
    }
    pub fn at(&self, x: usize, y: usize) -> [u8; 4] {
        let o = (y * self.w + x) * 4;
        [self.px[o], self.px[o + 1], self.px[o + 2], self.px[o + 3]]
    }
    pub fn set(&mut self, x: usize, y: usize, c: [u8; 4]) {
        let o = (y * self.w + x) * 4;
        self.px[o..o + 4].copy_from_slice(&c);
    }
    /// numpy's `A[y0:y0 + h, x0:x0 + w] = src[sy:sy + h, sx:sx + w]` (both sides clipped to the pictures: a shape
    /// mismatch is an error, as numpy's).
    #[allow(clippy::too_many_arguments)]
    pub fn paste(&mut self, x0: usize, y0: usize, w: usize, h: usize, src: &Rgba, sx: usize, sy: usize) -> Result<()> {
        let dw = w.min(self.w.saturating_sub(x0));
        let dh = h.min(self.h.saturating_sub(y0));
        let sw = w.min(src.w.saturating_sub(sx));
        let shh = h.min(src.h.saturating_sub(sy));
        if (dw, dh) != (sw, shh) {
            bail!("cannot place a {sw} x {shh} picture into {dw} x {dh}");
        }
        for y in 0..dh {
            let d = ((y0 + y) * self.w + x0) * 4;
            let s = ((sy + y) * src.w + sx) * 4;
            self.px[d..d + dw * 4].copy_from_slice(&src.px[s..s + dw * 4]);
        }
        Ok(())
    }
}

/// A 15-bit colour as RGBA (colour 0x0000 is transparent, as on the PS1).
pub fn c15(c: u16) -> [u8; 4] {
    [((c & 31) << 3) as u8, (((c >> 5) & 31) << 3) as u8, (((c >> 10) & 31) << 3) as u8, if c == 0 { 0 } else { 255 }]
}

pub fn words(px: &[u8]) -> Vec<u16> {
    px.as_chunks::<2>().0.iter().map(|b| u16::from_le_bytes(*b)).collect()
}

/// TIM -> RGBA (None: a 24-bit one).
pub fn render(t: &Tim) -> Result<Option<Rgba>> {
    let img = t.img.as_ref().context("the TIM has no image")?;
    let (w, h) = (img.w as usize, img.h as usize);
    let wd = words(&img.px);
    if wd.len() != w * h {
        bail!("a TIM's image is short");
    }
    if t.bpp == 16 {
        let mut out = Rgba::new(w, h);
        for (i, &v) in wd.iter().enumerate() {
            out.px[i * 4..i * 4 + 4].copy_from_slice(&c15(v));
        }
        return Ok(Some(out));
    }
    let pal: Vec<u16> = match &t.clut {
        Some(c) => words(&c.px),
        // np.arange(256, dtype=np.uint16) * 0x421 // 8 (the product wraps at 16 bits)
        None => (0..256u32).map(|i| ((i * 0x421) & 0xFFFF) as u16 / 8).collect(),
    };
    if pal.is_empty() {
        bail!("a TIM's palette is empty");
    }
    let (pw, mut idx): (usize, Vec<usize>) = match t.bpp {
        4 => (w * 4, wd.iter().flat_map(|&v| [0, 4, 8, 12].map(|s| ((v >> s) & 15) as usize)).collect()),
        8 => (w * 2, wd.iter().flat_map(|&v| [(v & 255) as usize, (v >> 8) as usize]).collect()),
        _ => return Ok(None),
    };
    if t.bpp == 8 && pal.len() >= 256 && idx.iter().all(|&i| i < 64) && h > 64 {
        // banded portraits: 6-bit pixels, the picture in four bands of rows, each with its own 64 colours
        // (Tekken 2's 160 x 100 select faces: 25 rows a band)
        for y in 0..h {
            let band = 64 * (y / (h / 4)).min(3);
            for i in &mut idx[y * pw..(y + 1) * pw] {
                *i += band;
            }
        }
    }
    let mut out = Rgba::new(pw, h);
    for (k, &i) in idx.iter().enumerate() {
        out.px[k * 4..k * 4 + 4].copy_from_slice(&c15(pal[i.min(pal.len() - 1)]));
    }
    Ok(Some(out))
}

// ---- glTF ------------------------------------------------------------------------------------------------------------
pub type M3 = [[f64; 3]; 3];
pub type V3 = [f64; 3];

/// PS1 (y down, z forward) -> glTF (y up): 180 deg about X.
pub const C: M3 = [[1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, -1.0]];

/// Rotation matrix -> unit quaternion (x, y, z, w).
pub fn quat(m: &M3) -> [f64; 4] {
    let tr = m[0][0] + m[1][1] + m[2][2];
    let (x, y, z, w);
    if tr > 0.0 {
        let s = 0.5 / (tr + 1.0).sqrt();
        w = 0.25 / s;
        x = (m[2][1] - m[1][2]) * s;
        y = (m[0][2] - m[2][0]) * s;
        z = (m[1][0] - m[0][1]) * s;
    } else if m[0][0] > m[1][1] && m[0][0] > m[2][2] {
        let s = 2.0 * (1.0 + m[0][0] - m[1][1] - m[2][2]).sqrt();
        w = (m[2][1] - m[1][2]) / s;
        x = 0.25 * s;
        y = (m[0][1] + m[1][0]) / s;
        z = (m[0][2] + m[2][0]) / s;
    } else if m[1][1] > m[2][2] {
        let s = 2.0 * (1.0 + m[1][1] - m[0][0] - m[2][2]).sqrt();
        w = (m[0][2] - m[2][0]) / s;
        x = (m[0][1] + m[1][0]) / s;
        y = 0.25 * s;
        z = (m[1][2] + m[2][1]) / s;
    } else {
        let s = 2.0 * (1.0 + m[2][2] - m[0][0] - m[1][1]).sqrt();
        w = (m[1][0] - m[0][1]) / s;
        x = (m[0][2] + m[2][0]) / s;
        y = (m[1][2] + m[2][1]) / s;
        z = 0.25 * s;
    }
    let q = [x, y, z, w];
    let n = norm4(&q);
    q.map(|v| v / n)
}

/// np.linalg.norm of a 4-vector.
pub fn norm4(q: &[f64; 4]) -> f64 {
    (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt()
}

/// Minimal glTF 2.0 binary writer (GLB).
pub struct Glb {
    pub bin: Vec<u8>,
    /// The document; its `bufferViews` and `accessors` keep their place and are filled from the lists below on
    /// writing.
    pub j: Map<String, Value>,
    pub views: Vec<Value>,
    pub accessors: Vec<Value>,
}

impl Default for Glb {
    fn default() -> Self {
        Self::new()
    }
}

impl Glb {
    pub fn new() -> Glb {
        let mut j = Map::new();
        j.insert("asset".into(), json!({"version": "2.0", "generator": "t2_extractor"}));
        j.insert("bufferViews".into(), json!([]));
        j.insert("accessors".into(), json!([]));
        Glb { bin: Vec::new(), j, views: Vec::new(), accessors: Vec::new() }
    }

    pub fn view(&mut self, data: &[u8], target: Option<u32>) -> usize {
        while !self.bin.len().is_multiple_of(4) {
            self.bin.push(0);
        }
        let mut v = Map::new();
        v.insert("buffer".into(), json!(0));
        v.insert("byteOffset".into(), json!(self.bin.len()));
        v.insert("byteLength".into(), json!(data.len()));
        if let Some(t) = target.filter(|t| *t != 0) {
            v.insert("target".into(), json!(t));
        }
        self.bin.extend_from_slice(data);
        self.views.push(Value::Object(v));
        self.views.len() - 1
    }

    /// An accessor over `count` elements of `bytes` (little-endian, packed). `minmax`: per-component min and max.
    #[allow(clippy::too_many_arguments)]
    pub fn acc(
        &mut self,
        bytes: &[u8],
        count: usize,
        ctype: u32,
        typ: &str,
        target: Option<u32>,
        minmax: Option<(Vec<f64>, Vec<f64>)>,
        normalized: bool,
    ) -> usize {
        let view = self.view(bytes, target);
        let mut a = Map::new();
        a.insert("bufferView".into(), json!(view));
        a.insert("componentType".into(), json!(ctype));
        a.insert("count".into(), json!(count));
        a.insert("type".into(), json!(typ));
        if normalized {
            a.insert("normalized".into(), json!(true));
        }
        if let Some((min, max)) = minmax {
            a.insert("min".into(), Value::Array(min.into_iter().map(f64_value).collect()));
            a.insert("max".into(), Value::Array(max.into_iter().map(f64_value).collect()));
        }
        self.accessors.push(Value::Object(a));
        self.accessors.len() - 1
    }

    /// float32 rows of N components (VEC2 / VEC3 / VEC4 / MAT4), with their min / max when asked.
    pub fn acc_f32<const N: usize>(&mut self, rows: &[[f32; N]], target: Option<u32>, minmax: bool) -> usize {
        let typ = match N {
            1 => "SCALAR",
            2 => "VEC2",
            3 => "VEC3",
            4 => "VEC4",
            _ => "MAT4",
        };
        let mm = minmax.then(|| {
            let mut lo = vec![f64::INFINITY; N];
            let mut hi = vec![f64::NEG_INFINITY; N];
            for r in rows {
                for c in 0..N {
                    lo[c] = lo[c].min(r[c] as f64);
                    hi[c] = hi[c].max(r[c] as f64);
                }
            }
            (lo, hi)
        });
        let b: Vec<u8> = rows.iter().flat_map(|r| r.iter().flat_map(|x| x.to_le_bytes())).collect();
        self.acc(&b, rows.len(), 5126, typ, target, mm, false)
    }

    /// int16 quaternions (normalized VEC4).
    pub fn acc_q16(&mut self, rows: &[[i16; 4]]) -> usize {
        let b: Vec<u8> = rows.iter().flat_map(|r| r.iter().flat_map(|x| x.to_le_bytes())).collect();
        self.acc(&b, rows.len(), 5122, "VEC4", None, None, true)
    }

    pub fn acc_u8x4(&mut self, rows: &[[u8; 4]], target: Option<u32>) -> usize {
        self.acc(&rows.concat(), rows.len(), 5121, "VEC4", target, None, false)
    }

    pub fn acc_u32(&mut self, idx: &[u32], target: Option<u32>) -> usize {
        let b: Vec<u8> = idx.iter().flat_map(|x| x.to_le_bytes()).collect();
        self.acc(&b, idx.len(), 5125, "SCALAR", target, None, false)
    }

    pub fn to_bytes(&mut self) -> Vec<u8> {
        while !self.bin.len().is_multiple_of(4) {
            self.bin.push(0);
        }
        self.j.insert("bufferViews".into(), Value::Array(self.views.clone()));
        self.j.insert("accessors".into(), Value::Array(self.accessors.clone()));
        self.j.insert("buffers".into(), json!([{"byteLength": self.bin.len()}]));
        let mut js = pyjson::compact(&Value::Object(self.j.clone())).into_bytes();
        while !js.len().is_multiple_of(4) {
            js.push(b' ');
        }
        let mut out = Vec::with_capacity(28 + js.len() + self.bin.len());
        out.extend(0x46546C67u32.to_le_bytes());
        out.extend(2u32.to_le_bytes());
        out.extend(((12 + 8 + js.len() + 8 + self.bin.len()) as u32).to_le_bytes());
        out.extend((js.len() as u32).to_le_bytes());
        out.extend(0x4E4F534Au32.to_le_bytes());
        out.extend(js);
        out.extend((self.bin.len() as u32).to_le_bytes());
        out.extend(0x004E4942u32.to_le_bytes());
        out.extend(&self.bin);
        out
    }

    pub fn write(&mut self, out: &dyn Vfs, path: &str) -> Result<()> {
        out.write(path, &self.to_bytes())
    }
}

/// A float as a JSON number (a non-finite one as null).
pub fn f64_value(f: f64) -> Value {
    serde_json::Number::from_f64(f).map_or(Value::Null, Value::Number)
}

/// A list of floats as JSON.
pub fn floats(v: &[f64]) -> Value {
    Value::Array(v.iter().map(|&f| f64_value(f)).collect())
}

// ---- sound -----------------------------------------------------------------------------------------------------------
const ADPCM_K: [(i32, i32); 5] = [(0, 0), (60, 0), (115, -52), (98, -55), (122, -60)];

/// PS1 SPU ADPCM (16-byte blocks: shift/filter, flags, 14 data bytes) -> int16 samples.
pub fn spu_adpcm(b: &[u8]) -> Vec<i16> {
    let mut out = Vec::new();
    let (mut s1, mut s2) = (0i32, 0i32);
    let mut o = 0;
    while o + 16 <= b.len() {
        let (sh, f, flag) = (b[o] & 15, ((b[o] >> 4) & 7).min(4), b[o + 1]);
        let (k0, k1) = ADPCM_K[f as usize];
        for &byte in &b[o + 2..o + 16] {
            for nib in [byte & 15, byte >> 4] {
                let mut v = (nib as i32) << 12;
                if v & 0x8000 != 0 {
                    v -= 0x10000;
                }
                let s = ((v >> sh) + (s1 * k0 + s2 * k1 + 32).div_euclid(64)).clamp(-32768, 32767);
                out.push(s as i16);
                s2 = s1;
                s1 = s;
            }
        }
        if flag & 1 != 0 && o != 0 && flag & 2 == 0 {
            break; // end of a one-shot sample
        }
        o += 16;
    }
    out
}

/// A pBAV v7 tone: (program, min key, centre, fine, vag index, max key).
pub type Tone = (usize, u8, u8, u8, i16, u8);

/// pBAV v7 header -> (tones, sample sizes, header length).
pub fn parse_vab_header(d: &[u8]) -> Result<(Vec<Tone>, Vec<usize>, usize)> {
    let (ps, vs) = (u16_(d, 18)? as usize, u16_(d, 22)? as usize);
    let o = 32 + 2048;
    let mut tones = Vec::new();
    for p in 0..ps {
        for t in 0..16 {
            let a = o + (p * 16 + t) * 32;
            let vag = i16_(d, a + 22)?;
            if vag > 0 {
                tones.push((p, u8_(d, a + 6)?, u8_(d, a + 4)?, u8_(d, a + 5)?, vag, u8_(d, a + 7)?));
            }
        }
    }
    let at = o + ps * 512;
    bytes::<512>(d, at)?;
    let sizes = (1..(vs + 1).min(256)).map(|i| u16_(d, at + 2 * i).map(|s| s as usize * 8)).collect::<Result<_>>()?;
    Ok((tones, sizes, at + 512))
}

/// A mono 16-bit WAV (Python's wave module).
pub fn wav_bytes(x: &[i16], rate: u32) -> Vec<u8> {
    let n = x.len() * 2;
    let mut d = Vec::with_capacity(44 + n);
    d.extend(b"RIFF");
    d.extend((36 + n as u32).to_le_bytes());
    d.extend(b"WAVEfmt ");
    d.extend(16u32.to_le_bytes());
    d.extend(1u16.to_le_bytes());
    d.extend(1u16.to_le_bytes());
    d.extend(rate.to_le_bytes());
    d.extend((rate * 2).to_le_bytes());
    d.extend(2u16.to_le_bytes());
    d.extend(16u16.to_le_bytes());
    d.extend(b"data");
    d.extend((n as u32).to_le_bytes());
    d.extend(x.iter().flat_map(|s| s.to_le_bytes()));
    d
}

/// A VAB bank's samples as WAVs in outdir/key/NN.wav; the bank's index entry.
pub fn export_bank(name: &str, header: &[u8], body: &[u8], out: &dyn Vfs, outdir: &str, key: &str) -> Result<Value> {
    let (tones, sizes, _) = parse_vab_header(header)?;
    let mut offs = vec![0usize];
    for s in &sizes {
        offs.push(offs[offs.len() - 1] + s);
    }
    let mut rate_of: Vec<(i16, f64)> = Vec::new();
    let mut keys_of: Vec<(i16, Vec<Value>)> = Vec::new();
    for &(p, k, centre, fine, vag, kmax) in &tones {
        if !rate_of.iter().any(|r| r.0 == vag) {
            rate_of.push((vag, 44100.0 * libm::pow(2.0, (k as f64 - centre as f64 + fine as f64 / 128.0) / 12.0)));
        }
        let e = json!([p, k, kmax]);
        match keys_of.iter_mut().find(|r| r.0 == vag) {
            Some(r) => r.1.push(e),
            None => keys_of.push((vag, vec![e])),
        }
    }
    let mut samples = Vec::new();
    for i in 0..sizes.len() {
        let x = spu_adpcm(py_slice(body, offs[i], offs[i + 1]));
        if x.len() < 32 || x.iter().map(|s| s.wrapping_abs()).max().unwrap_or(0) == 0 {
            continue;
        }
        let vag = (i + 1) as i16;
        let rate = rate_of.iter().find(|r| r.0 == vag).map_or(22050.0, |r| r.1);
        let fname = format!("{key}/{:02}.wav", i + 1);
        let r = rate.round_ties_even();
        out.write(&join(outdir, &fname), &wav_bytes(&x, r as u32))?;
        // tones: [program, min key, max key] that play this sample (SePlay keys note = tone + 60)
        let tones = keys_of.iter().find(|r| r.0 == vag).map_or(vec![], |r| r.1.clone());
        samples.push(json!({"file": fname, "rate": r as i64, "seconds": py_round(x.len() as f64 / rate, 2),
                            "tones": tones}));
    }
    Ok(json!({"name": name, "samples": samples}))
}

/// STR movies: each starts at a video sector with frame 1, chunk 0; ends where the next starts.
pub fn find_movies(iso: &mut Iso, xas_lba: u32, nsec: u32) -> Result<Vec<(u32, u32)>> {
    let mut starts = Vec::new();
    let mut last = None;
    for i in 0..nsec {
        let h = iso.at((xas_lba + i) as u64 * RAW + 16, 24)?;
        if h.len() < 24 || h[2] & 4 != 0 || h[8..12] != [0x60, 0x01, 0x01, 0x80] {
            continue;
        }
        let (chunk, frame) = (u16_(&h, 12)?, u32_(&h, 16)?);
        if chunk == 0 && frame == 1 {
            starts.push(i);
        }
        last = Some(i);
    }
    Ok(starts.iter().enumerate().map(|(j, &s)| (s, starts.get(j + 1).map_or(last.unwrap_or(0), |n| n - 1))).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_as_python() {
        assert_eq!(py_round(0.125, 2), 0.12);
        assert_eq!(py_round(0.375, 2), 0.38);
        assert_eq!(py_round(2.675, 2), 2.67);
        assert_eq!(py_round(1.05, 1), 1.1);
    }

    #[test]
    fn lz_copies_back() {
        // flags 0b10111: three literals, a copy of 4 bytes 3 back, the end marker; then flags 0 ends
        let src = [0x17, b'a', b'b', b'c', 0x20, 0x03, 0];
        let (out, end) = lz(&src, 0, LZ_LIMIT).unwrap_or((None, 0));
        assert_eq!(out.as_deref(), Some(&b"abcabca"[..]));
        assert_eq!(end, 7);
    }
}
