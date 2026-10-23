//! Tekken 2 (USA v1.1, SLUS-00213) characters to glTF for the port's engine.
//!
//! usage: t2_convert export_characters "Tekken 2 (USA) (v1.1) (Track 1).bin" OUTDIR [block,...] [--jobs N]
//!
//! The disc (survey: TEKKEN2_ENGINE notes in this file):
//!   SLUS_002.13 holds the table of TEKKEN2.BNS records at file offset 0x960: 400 x {u32 0, u32 sector, u32 size}.
//!   Records 24..399 are 47 character blocks of 8 records (block b = character b // 2, costume b % 2, block 46 the
//!   extra one): +0 stage panorama TIMs, +1 ?, +2 model arc, +3 VAB header, +4 "nlz " data, +5 texture TIMs,
//!   +6 VAB body, +7 moves and motions.
//!   arc (EXE 0x8006C538): u32 n; n x {u32 offset, u32 size}.
//! Model (arc part 0, drawn by 0x8006EB38): 25 rows of 32 bytes {u32 uv block, u32 vertex block, u32 polygon
//!   block... } -- per row: i32 vertices, polygons, UVs offsets, 2 x 0, i32 tx, ty, tz (the bone's offset from its
//!   parent; z negated, and x, y negated for bones 2, 3, 7 when the game builds the matrix). Rigid: each bone has its
//!   own vertices.
//!     vertices: u32 shading, u32 n, n x {s16 x, y, z, pad} (shading 2: each followed by its normal)
//!     polygons: u32 nt, nt x {u8 a, b, c, pad, s16 nx, ny, nz, pad}; u32 nq, nq x {u8 a, b, c, d, normal}
//!               (shading 2: 4-byte entries, the indices only)
//!     UVs: u32 1, u32 nt, nt x 3 words, u32 nq, nq x 4 words; word = u | v << 8 | attribute << 16 (word 0's
//!          attribute the CLUT, word 1's the texture page, as in a GPU packet)
//!   Bone parents: EXE table 0x8004E964 (18 bones).
//! Moves and motions (+7): u8 relocated, u8, s16 move count, then 7 offsets: slot table (0x703 slots, u32: a move
//!   index, | 0x40000000 = the EXE's common table 0x80042864), moves (0x2C bytes: +0 the motion's offset in the
//!   record), ... Motions are decoded by the game's own routine 0x8006C410 (frame -> 57 s16: the root's translation
//!   and 18 x 3 angles, 65536 a turn -- the root's place is polar: direction, height, distance; bone k's matrix =
//!   Rz(-c) Ry(-b) Rx(a), EXE 0x80072234), run here through a small MIPS interpreter (mips.rs) on the original code.

use crate::mips::{Cpu, no_hook};
use crate::np::{I3, mm3, mv3};
use crate::ps1::{
    C, FPS, Glb, Iso, M3, Rgba, SCALE, V3, c15, floats, i16_, i32_, py_slice, quat, u8_, u16_, u32_, walk_tims, words,
};
use crate::{np, pil, pyjson};
use anyhow::{Context, Result, bail};
use conv_io::{Ctx, DiscFile, StdDisc, StdFs, Vfs, join};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};

pub const TOC: usize = 0x960; // the BNS table in SLUS_002.13 (file offset)
pub const TOC_COUNT: usize = 400;
pub const BLOCK0: usize = 24; // character blocks
pub const BLOCKS: usize = 47;
pub const EXE_LOAD: u32 = 0x80010000;
const PARENT_TABLE: u32 = 0x8004E964; // bone parents (s16 x 18)
const SIN_TABLE: u32 = 0x80056DA4; // 4096 s16 sines, cosine = +1024 entries
const POSE: u32 = 0x8006C410; // decode(motion, dest, frame): 57 s16 at dest
const CHAR_TABLE: u32 = 0x8005C504; // 25 x {u32 name, u8 id + 0x15, u8 id + 0x33, u8 stage, u8 order, name}
pub const BONES: usize = 18;
pub const BONE_NAMES: [&str; BONES] = [
    "root",
    "chest",
    "head",
    "clavicle_A",
    "upperarm_A",
    "forearm_A",
    "hand_A",
    "clavicle_B",
    "upperarm_B",
    "forearm_B",
    "hand_B",
    "pelvis",
    "thigh_A",
    "shin_A",
    "foot_A",
    "thigh_B",
    "shin_B",
    "foot_B",
];
const MOTION_BASE: u32 = 0x80180000; // where the moves record sits for the interpreter

pub struct Character {
    pub id: i64,
    pub name: String,
    pub stage: u8,
    pub order: u8,
}

pub struct Disc {
    pub exe: Vec<u8>,
    pub bns: Vec<u8>,
    toc: Vec<(u32, u32)>,
}

/// The EXE's file offset of a RAM address.
pub fn off(addr: u32) -> usize {
    addr.wrapping_sub(EXE_LOAD).wrapping_add(0x800) as usize
}

impl Disc {
    pub fn open(disc: &dyn DiscFile) -> Result<Disc> {
        let mut iso = Iso::open(disc)?;
        let exe = iso.read("SLUS_002.13")?;
        let bns = iso.read("TEKKEN2/TEKKEN2.BNS")?;
        let toc = (0..TOC_COUNT)
            .map(|i| Ok((u32_(&exe, TOC + 12 * i + 4)?, u32_(&exe, TOC + 12 * i + 8)?)))
            .collect::<Result<_>>()?;
        Ok(Disc { exe, bns, toc })
    }

    pub fn rec(&self, r: usize) -> Result<&[u8]> {
        let (s, z) = *self.toc.get(r).with_context(|| format!("no BNS record {r}"))?;
        let s = s as usize * 2048;
        Ok(py_slice(&self.bns, s, s + z as usize))
    }

    /// The EXE's bytes from a RAM address on (empty past its end).
    pub fn at(&self, addr: u32) -> &[u8] {
        py_slice(&self.exe, off(addr), usize::MAX)
    }

    pub fn u8(&self, addr: u32) -> Result<u8> {
        u8_(&self.exe, off(addr))
    }
    pub fn u16(&self, addr: u32) -> Result<u16> {
        u16_(&self.exe, off(addr))
    }
    pub fn i16(&self, addr: u32) -> Result<i16> {
        i16_(&self.exe, off(addr))
    }
    pub fn u32(&self, addr: u32) -> Result<u32> {
        u32_(&self.exe, off(addr))
    }
    pub fn i32(&self, addr: u32) -> Result<i32> {
        i32_(&self.exe, off(addr))
    }

    pub fn characters(&self) -> Result<Vec<Character>> {
        let mut out = Vec::new();
        // each entry's name follows it; the next entry starts at the next word after the name
        let mut a = CHAR_TABLE;
        for _ in 0..25 {
            let (ptr, cid) = (self.u32(a)?, self.u8(a + 4)?);
            let (stage, order) = (self.u8(a + 6)?, self.u8(a + 7)?);
            let raw = self.at(ptr);
            let raw = raw.split(|b| *b == 0).next().unwrap_or(&[]);
            let name: String = raw.iter().map(|&b| if b < 0x80 { b as char } else { '\u{FFFD}' }).collect();
            out.push(Character { id: cid as i64 - 0x15, name, stage, order });
            a = (ptr.wrapping_add(raw.len() as u32 + 1 + 3)) & !3;
        }
        Ok(out)
    }

    /// {id: name} of the character table.
    pub fn names(&self) -> Result<HashMap<i64, String>> {
        Ok(self.characters()?.into_iter().map(|c| (c.id, c.name)).collect())
    }
}

pub fn arc(d: &[u8]) -> Result<Vec<&[u8]>> {
    let n = u32_(d, 0)? as usize;
    (0..n)
        .map(|k| {
            let (o, z) = (u32_(d, 4 + 8 * k)? as usize, u32_(d, 8 + 8 * k)? as usize);
            Ok(py_slice(d, o, o.saturating_add(z)))
        })
        .collect()
}

/// Python's `str.title()` (ASCII).
pub fn title(s: &str) -> String {
    let mut prev = false;
    s.chars()
        .map(|c| {
            let r = if prev { c.to_ascii_lowercase() } else { c.to_ascii_uppercase() };
            prev = c.is_alphabetic();
            r
        })
        .collect()
}

// ---- texture: the TIMs in VRAM, read through each face's page and CLUT -----------------------------------
pub struct Vram(pub Vec<u16>);

impl Vram {
    pub fn new() -> Vram {
        Vram(vec![0; 512 * 1024])
    }
    pub fn get(&self, y: usize, x: usize) -> u16 {
        self.0[y * 1024 + x]
    }
    /// `V[y:y + h, x:x + w] = words` (an error where numpy's shapes would not match).
    pub fn blit(&mut self, b: &crate::ps1::Block) -> Result<()> {
        let (x, y, w, h) = (b.x as usize, b.y as usize, b.w as usize, b.h as usize);
        let wd = words(&b.px);
        if x + w > 1024 || y + h > 512 || wd.len() != w * h {
            bail!("a TIM block ({x}, {y}, {w} x {h}) does not fit VRAM");
        }
        for r in 0..h {
            self.0[(y + r) * 1024 + x..(y + r) * 1024 + x + w].copy_from_slice(&wd[r * w..(r + 1) * w]);
        }
        Ok(())
    }
}

impl Default for Vram {
    fn default() -> Self {
        Self::new()
    }
}

/// The texture record's TIMs in two VRAM spaces: pictures and CLUTs. Their coordinates are relative (the game places
/// a character's pictures and palettes apart when it uploads them), so the CLUT rows at y 0.. would otherwise land on
/// the 8-bit pictures at (0, 0).
pub fn load_vram(texrec: &[u8]) -> Result<(Vram, Vram)> {
    let (mut v, mut p) = (Vram::new(), Vram::new());
    for t in walk_tims(texrec) {
        if let Some(b) = &t.img {
            v.blit(b)?;
        }
        if let Some(b) = &t.clut {
            p.blit(b)?;
        }
    }
    Ok((v, p))
}

/// The 256 x 256 texels of a texture page through a CLUT (RGBA).
pub fn page_texels(v: &Vram, pal: &Vram, tpage: u32, clut: u32) -> Rgba {
    let (px, py, depth) = (((tpage & 15) * 64) as usize, (((tpage >> 4) & 1) * 256) as usize, (tpage >> 7) & 3);
    let (cx, cy) = (((clut & 63) * 16) as usize, ((clut >> 6) & 511) as usize);
    let lut: Vec<[u8; 4]> = match depth {
        0 => (cx..cx + 16).map(|x| c15(pal.get(cy, x))).collect(),
        1 => (cx..(cx + 256).min(1024)).map(|x| c15(pal.get(cy, x))).collect(),
        _ => vec![],
    };
    let mut out = Rgba::new(256, 256);
    for y in 0..256 {
        for u in 0..256 {
            let c = match depth {
                0 => lut[(((v.get(py + y, (px + u / 4) % 1024) >> ((u % 4) * 4)) & 15) as usize).min(lut.len() - 1)],
                1 => lut[(((v.get(py + y, (px + u / 2) % 1024) >> ((u % 2) * 8)) & 255) as usize).min(lut.len() - 1)],
                _ => c15(v.get(py + y, (px + u) % 1024)),
            };
            out.set(u, y, c);
        }
    }
    out
}

// ---- model ---------------------------------------------------------------------------------------------------
/// A polygon: its corner indices and UV words.
pub type Face = (Vec<u8>, Vec<u32>);
/// A model row's vertices and polygons.
pub type Geom = (Vec<[i16; 3]>, Vec<Face>);

fn usz(v: i32) -> Result<usize> {
    usize::try_from(v).context("a negative offset in the model")
}

fn verts_at(p: &[u8], o: usize, n: usize, stride: usize) -> Result<Vec<[i16; 3]>> {
    (0..n).map(|j| Ok([i16_(p, o + stride * j)?, i16_(p, o + stride * j + 2)?, i16_(p, o + stride * j + 4)?])).collect()
}

/// Polygons (`es`-byte entries) and their UVs: the triangles then the quads.
fn faces_at(p: &[u8], po: usize, uo: usize, es: usize) -> Result<Vec<Face>> {
    let nt = u32_(p, po)? as usize;
    let q = po + 4 + es * nt;
    let nq = u32_(p, q)? as usize;
    let tris = (0..nt).map(|i| py_slice(p, po + 4 + es * i, po + 7 + es * i).to_vec());
    let quads = (0..nq).map(|i| py_slice(p, q + 4 + es * i, q + 8 + es * i).to_vec());
    let q = uo + 4;
    let ntu = u32_(p, q)? as usize;
    let tuv = (0..ntu).map(|i| (0..3).map(|k| u32_(p, q + 4 + 12 * i + 4 * k)).collect::<Result<Vec<_>>>());
    let tuv: Vec<Vec<u32>> = tuv.collect::<Result<_>>()?;
    let q = q + 4 + 12 * ntu;
    let nqu = u32_(p, q)? as usize;
    let quv: Vec<Vec<u32>> = (0..nqu)
        .map(|i| (0..4).map(|k| u32_(p, q + 4 + 16 * i + 4 * k)).collect::<Result<Vec<_>>>())
        .collect::<Result<_>>()?;
    Ok(tris.zip(tuv).chain(quads.zip(quv)).collect())
}

fn row(p: &[u8], k: usize) -> Result<[i32; 8]> {
    let mut r = [0; 8];
    for (i, v) in r.iter_mut().enumerate() {
        *v = i32_(p, 32 * k + 4 * i)?;
    }
    Ok(r)
}

/// One model row's (vertices, [(corner indices, UV words)]).
pub fn row_geometry(p: &[u8], r: &[i32; 8]) -> Result<Geom> {
    let (vo, po, uo) = (usz(r[0])?, usz(r[1])?, usz(r[2])?);
    // shading 1: flat (each polygon carries its normal, 12-byte entries); 2: smooth (each vertex followed by
    // its normal; 4-byte polygon entries of indices only)
    let (shading, n) = (u32_(p, vo)?, u32_(p, vo + 4)? as usize);
    let stride = if shading == 2 { 16 } else { 8 };
    let verts = verts_at(p, vo + 8, n, stride)?;
    let es = if shading == 1 { 12 } else { 4 };
    Ok((verts, faces_at(p, po, uo, es)?))
}

pub fn parse_model(p: &[u8]) -> Result<(Vec<(usize, Geom)>, Vec<V3>)> {
    let rows = (0..25).map(|k| row(p, k)).collect::<Result<Vec<_>>>()?;
    let mut bones = Vec::new();
    for (k, r) in rows[..BONES].iter().enumerate() {
        if r[0] != 0 {
            bones.push((k, row_geometry(p, r)?));
        }
    }
    let mut offsets: Vec<V3> = rows[..BONES].iter().map(|r| [r[5] as f64, r[6] as f64, -(r[7] as f64)]).collect();
    for k in [2, 3, 7] {
        offsets[k][0] *= -1.0;
        offsets[k][1] *= -1.0;
    }
    // the upper arms' offsets turn (0x8006ED64: ApplyMatrixLV of RotMatrixY(+-0x400) applied to a -90 degree turn
    // about X): bone 4 by Ry(+90) Rx, bone 8 by Ry(-90) Rx -- the shoulder joints sit out at the shoulders
    let mx = [[1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, -1.0, 0.0]];
    for (k, sgn) in [(4, 1.0), (8, -1.0)] {
        let ry = [[0.0, 0.0, sgn], [0.0, 1.0, 0.0], [-sgn, 0.0, 0.0]];
        offsets[k] = mv3(&mm3(&ry, &mx), &offsets[k]);
    }
    Ok((bones, offsets))
}

const CHAINS_MAGIC: i16 = -257;
const CHAIN_MAGIC: i16 = 0x6FFF;

/// A chain: the bone it hangs from, the link length (model units), each link's row geometry.
pub type Chain = (i16, i16, Vec<Geom>);

/// Model arc part 2: the hanging chains (Roger's / Alex's tail, Kunimitsu's ribbons, belt ends): s16 0xFEFF, chain
/// count, link count, the model row of each link (rows 18..20, beyond the 18 bones; up to 4), 0, 0, then 0x6FFF, the
/// bone they hang from, links a chain, link length (model units); the rest is the same in every model. The game
/// swings them (chain_sim.rs runs its routine).
pub fn parse_chains(model: &[u8], part2: &[u8]) -> Result<Vec<Chain>> {
    let h = (0..13).map(|i| i16_(part2, 2 * i)).collect::<Result<Vec<_>>>()?;
    if h[0] != CHAINS_MAGIC || h[9] != CHAIN_MAGIC || h[1] == 0 || h[11] == 0 {
        return Ok(vec![]);
    }
    let rows = &h[3..(3 + h[2].max(0) as usize).min(13)];
    let per = h[11];
    let mut out = Vec::new();
    for c in 0..h[1].max(0) as i64 {
        let (a, b) = (c * per as i64, (c + 1) * per as i64);
        // Python's slice of a tuple, negative bounds counting from the end
        let n = rows.len() as i64;
        let fix = |i: i64| if i < 0 { (i + n).max(0) } else { i.min(n) } as usize;
        let (a, b) = (fix(a), fix(b));
        let ks = if a < b { &rows[a..b] } else { &[][..] };
        let gs = ks.iter().map(|&k| row_geometry(model, &row(model, usz(k as i32)?)?)).collect::<Result<_>>()?;
        out.push((h[10], h[12], gs));
    }
    Ok(out)
}

/// Model arc part 1: the shadow hull (the same for every character), rows as part 0's; per bone {u32 n, n x (s16 x,
/// y, z, pad)}, {u32 nt, nt x (a, b, c, pad)}, {u32 nq, nq x (a, b, c, d)}, no UVs. 0x8006DBA8 makes flat POLY_F3 / F4
/// packets of the stage's shadow colour (table 0x800A1030 + 4) from it; the draw flattens it onto the floor through
/// the matrix 0x800D659C (export_stages: lighting.shadow).
pub fn parse_shadow(p: &[u8]) -> Result<Vec<(usize, Vec<[i16; 3]>, Vec<Vec<u8>>)>> {
    let mut out = Vec::new();
    for k in 0..BONES {
        let (vo, po) = (i32_(p, 32 * k)?, i32_(p, 32 * k + 4)?);
        if vo == 0 {
            continue;
        }
        let (vo, po) = (usz(vo)?, usz(po)?);
        let n = u32_(p, vo)? as usize;
        let verts = verts_at(p, vo + 4, n, 8)?;
        let nt = u32_(p, po)? as usize;
        let q = po + 4 + 4 * nt;
        let nq = u32_(p, q)? as usize;
        let mut faces: Vec<Vec<u8>> = (0..nt).map(|i| py_slice(p, po + 4 + 4 * i, po + 7 + 4 * i).to_vec()).collect();
        faces.extend((0..nq).map(|i| py_slice(p, q + 4 + 4 * i, q + 8 + 4 * i).to_vec()));
        out.push((k, verts, faces));
    }
    Ok(out)
}

const WING_CLIPS: u32 = 0x800A14DC; // 8 x pointer to frame-number bytes, 0xFF-terminated (0x8006F9E8)
// 0x8006F9E8's transitions, per wing mode (fighter +0xE44, set by the moves at 0x80065290) and current clip:
// ('now', c) switches at once, ('then', c) plays the clip to its end and goes on with c; clip 0 in mode 0 follows
// the stance motion's frame (0x8006FA80)
const WING_MODES: [[(&str, u8); 8]; 4] = [
    [("then", 0), ("now", 2), ("then", 0), ("now", 2), ("now", 2), ("now", 2), ("now", 0), ("then", 0)],
    [("now", 1), ("then", 3), ("now", 3), ("then", 3), ("then", 3), ("then", 3), ("now", 1), ("now", 1)],
    [("now", 4), ("then", 5), ("now", 5), ("then", 5), ("then", 5), ("then", 5), ("now", 4), ("now", 4)],
    [("now", 6), ("now", 6), ("now", 6), ("now", 6), ("now", 6), ("now", 6), ("then", 7), ("then", 7)],
];

/// Devil's / Angel's wings: model row 18's polygons and UVs (shading 1; its vertex block is only the header), drawn
/// in the chest's frame (0x800727B8) with the vertices of one frame of the moves arc's part (costume & 1) + 1
/// (0x8006EEC8): {u32 36, 192 x u32 offset, 192 x 36 x (s16 x, y, z, pad)}.
pub fn parse_wings(model: &[u8], frames_rec: &[u8]) -> Result<Option<(Vec<Vec<[i16; 3]>>, Vec<Face>)>> {
    let r = row(model, 18)?;
    if r[0] == 0 {
        return Ok(None);
    }
    let n = u32_(frames_rec, 0)? as usize;
    let count = (u32_(frames_rec, 4)? as i64 - 4).div_euclid(4);
    let count = usize::try_from(count).context("the wing frames' table is empty")?;
    let frames =
        (0..count).map(|i| verts_at(frames_rec, u32_(frames_rec, 4 + 4 * i)? as usize, n, 8)).collect::<Result<_>>()?;
    Ok(Some((frames, faces_at(model, usz(r[1])?, usz(r[2])?, 12)?)))
}

pub struct Trig {
    sin: Vec<f64>,
}

impl Trig {
    pub fn new(disc: &Disc) -> Result<Trig> {
        Ok(Trig { sin: (0..4096).map(|i| Ok(disc.i16(SIN_TABLE + 2 * i)? as f64 / 4096.0)).collect::<Result<_>>()? })
    }

    pub fn sincos(&self, v: i64) -> (f64, f64) {
        let i = ((v & 0xFFFF) >> 4) as usize;
        (self.sin[i], self.sin[(i + 1024) & 4095])
    }

    pub fn rot(&self, a: i64, b: i64, c: i64) -> M3 {
        let (sx, cx) = self.sincos(a);
        let (sy, cy) = self.sincos(b);
        let (sz, cz) = self.sincos(c);
        [
            [cy * cz, sx * sy * cz - cx * sz, cx * sy * cz + sx * sz],
            [cy * sz, sx * sy * sz + cx * cz, cx * sy * sz - sx * cz],
            [-sy, sx * cy, cx * cy],
        ]
    }
}

/// The motion offsets the record's moves use, in move order (first use).
pub fn motions_of(moves: &[u8]) -> Result<Vec<usize>> {
    let n = i16_(moves, 2)?;
    let h2 = u32_(moves, 8)? as usize;
    let mut seen = Vec::new();
    for i in 0..n.max(0) as usize {
        let o = u32_(moves, h2 + 0x2C * i)? as usize;
        if o != 0 && !seen.contains(&o) && o < moves.len() {
            seen.push(o);
        }
    }
    Ok(seen)
}

/// The game's own motion decoder, on an interpreter loaded with one moves record (kept between motions, as the
/// Python's cache).
pub struct Decoder {
    cpu: Cpu,
}

impl Decoder {
    pub fn new(exe: &[u8], moves: &[u8]) -> Decoder {
        let mut cpu = Cpu::new();
        cpu.map(EXE_LOAD, py_slice(exe, 0x800, usize::MAX));
        cpu.map(MOTION_BASE, moves);
        Decoder { cpu }
    }

    /// All frames of the motion at offset o: 57 channels each.
    pub fn decode(&mut self, moves: &[u8], o: usize) -> Result<Vec<[i16; 57]>> {
        let n = moves.get(o).copied().unwrap_or(0);
        let mut out = Vec::new();
        for f in 0..n.max(1) as u32 {
            self.cpu.call(POSE, &[MOTION_BASE.wrapping_add(o as u32), 0x1F800072, f], &mut no_hook)?;
            let mut ch = [0i16; 57];
            for (i, c) in ch.iter_mut().enumerate() {
                *c = i16_(&self.cpu.scratch, 0x72 + 2 * i)?;
            }
            out.push(ch);
        }
        Ok(out)
    }
}

pub fn to_gl(r: &M3, t: &V3) -> (M3, V3) {
    (mm3(&mm3(&C, r), &C), mv3(&C, t).map(|v| SCALE * v))
}

/// glTF translation of a game offset (no rotation).
pub fn gl_t(t: &V3) -> V3 {
    mv3(&C, t).map(|v| SCALE * v)
}

/// Decoded frames (57 channels each) -> per-bone glTF rotations (quaternions, sign-continuous) and the root's glTF
/// translations.
pub fn clip_tracks(trig: &Trig, frames: &[[i16; 57]]) -> (Vec<Vec<[f64; 4]>>, Vec<V3>) {
    let mut rot: Vec<Vec<[f64; 4]>> = vec![Vec::new(); BONES];
    let mut root_t = Vec::new();
    for ch in frames {
        for (k, r) in rot.iter_mut().enumerate() {
            let a = &ch[3 + 3 * k..6 + 3 * k];
            let (rg, _) = to_gl(&trig.rot(a[0] as i64, -(a[1] as i64), -(a[2] as i64)), &[0.0; 3]);
            let mut q = quat(&rg);
            if let Some(last) = r.last()
                && np::dot4(&q, last) < 0.0
            {
                q = q.map(|v| -v);
            }
            r.push(q);
        }
        // the root's place (0x8006234C): polar on the floor, channel 0 the direction (relative to the facing,
        // 65536 a turn; any when the distance is 0), channel 2 the distance, channel 1 the height:
        // x = -d sin(dir + 0x8000), z = d cos(dir + 0x8000)
        let (s, c) = trig.sincos(ch[0] as i64 + 0x8000);
        let d = ch[2] as f64;
        root_t.push(gl_t(&[-d * s, -(ch[1] as f64), d * c]));
    }
    (rot, root_t)
}

/// numpy's `np.round(np.clip(q.astype(float32), -1, 1) * 32767).astype(int16)`.
pub fn q16(q: &[f64; 4]) -> [i16; 4] {
    q.map(|v| ((v as f32).clamp(-1.0, 1.0) * 32767.0).round_ties_even() as i16)
}

pub fn f32s<const N: usize>(v: &[f64; N]) -> [f32; N] {
    v.map(|x| x as f32)
}

fn tri_list(n: usize) -> &'static [[usize; 3]] {
    if n == 4 { &[[0, 1, 2], [1, 3, 2]] } else { &[[0, 1, 2]] }
}

pub fn convert(disc: &Disc, block: usize, out: &dyn Vfs, out_path: &str) -> Result<(usize, usize, usize)> {
    let base = BLOCK0 + 8 * block;
    let model_parts = arc(disc.rec(base + 2)?)?;
    let model = *model_parts.first().context("the model arc is empty")?;
    let tex = disc.rec(base + 5)?;
    let mut moves = disc.rec(base + 7)?;
    let mut wings = None;
    if moves.get(..4) == Some(b"arc ") {
        // Devil / Angel: an "arc " container {magic, n, n x (offset, size)}: part 0 the moves and motions, parts 1
        // and 2 the wing frames of costume 0 (Devil) and 1 (Angel)
        let n = u32_(moves, 4)? as usize;
        let parts = (0..n)
            .map(|k| Ok((u32_(moves, 8 + 8 * k)? as usize, u32_(moves, 12 + 8 * k)? as usize)))
            .collect::<Result<Vec<_>>>()?;
        let (o, z) = *parts.get(1 + (block & 1)).context("the moves arc has no wing frames")?;
        wings = parse_wings(model, py_slice(moves, o, o + z))?;
        let (o, z) = parts[0];
        moves = py_slice(moves, o, o + z);
    }
    let (bones, offsets) = parse_model(model)?;
    let chains = match model_parts.get(2) {
        Some(p2) => parse_chains(model, p2)?,
        None => vec![],
    };
    // (chain, link, bone, length, geometry)
    let links: Vec<(usize, usize, i16, i16, &Geom)> = chains
        .iter()
        .enumerate()
        .flat_map(|(c, (bone, length, gs))| gs.iter().enumerate().map(move |(i, g)| (c, i, *bone, *length, g)))
        .collect();
    let parents = (0..BONES as u32).map(|k| disc.i16(PARENT_TABLE + 2 * k)).collect::<Result<Vec<_>>>()?;
    let trig = Trig::new(disc)?;

    // atlas: one cell per (page, CLUT) the faces use, cropped to the texels they touch
    let (vram, pal) = load_vram(tex)?;
    let mut used: BTreeMap<(u32, u32), [u32; 4]> = BTreeMap::new();
    let all_faces = bones
        .iter()
        .flat_map(|(_, g)| g.1.iter())
        .chain(wings.iter().flat_map(|w: &(Vec<Vec<[i16; 3]>>, Vec<Face>)| w.1.iter()))
        .chain(links.iter().flat_map(|l| l.4.1.iter()));
    for (_, uv) in all_faces {
        let key = (uv[1] >> 16, uv[0] >> 16);
        let b = used.entry(key).or_insert([255, 255, 0, 0]);
        for w in uv {
            let (u, v) = (w & 0xFF, (w >> 8) & 0xFF);
            *b = [b[0].min(u), b[1].min(v), b[2].max(u), b[3].max(v)];
        }
    }
    let mut cells: BTreeMap<(u32, u32), (usize, usize, usize, usize, usize)> = BTreeMap::new();
    let mut x = 0;
    for (key, &[u0, v0, u1, v1]) in &used {
        let (w, h) = ((u1 as i64 - u0 as i64 + 1) as usize, (v1 as i64 - v0 as i64 + 1) as usize);
        cells.insert(*key, (x, u0 as usize, v0 as usize, w, h));
        x += w + 1;
    }
    let aw = x.max(16);
    let ah = cells.values().map(|c| c.4).max().unwrap_or(16);
    let mut atlas = Rgba::new(aw, ah);
    for (&(tpage, clut), &(cx, u0, v0, w, h)) in &cells {
        atlas.paste(cx, 0, w, h, &page_texels(&vram, &pal, tpage, clut), u0, v0)?;
    }
    let (wf, hf) = (aw as f64, ah as f64);

    // bind pose: every angle zero (the bones laid along their offsets)
    let mut rest: Vec<(M3, V3)> = Vec::new();
    for k in 0..BONES {
        if parents[k] < 0 {
            rest.push((I3, [0.0; 3]));
        } else {
            let (pr, pt) = *rest.get(parents[k] as usize).context("a bone's parent comes after it")?;
            let o = mv3(&pr, &offsets[k]);
            rest.push((pr, [o[0] + pt[0], o[1] + pt[1], o[2] + pt[2]]));
        }
    }
    // the chains' links: joints BONES.. after the bones, each hanging on its bone (the first) or the link before,
    // laid along +x at rest (link length apart)
    for (n, &(_, i, bone, length, _)) in links.iter().enumerate() {
        let (pr, pt) =
            if i == 0 { *rest.get(bone as usize).context("a chain hangs from no bone")? } else { rest[BONES + n - 1] };
        let t = if i != 0 {
            let o = mv3(&pr, &[length as f64, 0.0, 0.0]);
            [pt[0] + o[0], pt[1] + o[1], pt[2] + o[2]]
        } else {
            pt
        };
        rest.push((pr, t));
    }
    // a vertex at rest: (v @ R.T) + T
    let placed = |v: &[i16; 3], (r, t): &(M3, V3)| -> V3 {
        let p = mv3(r, &v.map(|c| c as f64));
        [p[0] + t[0], p[1] + t[1], p[2] + t[2]]
    };
    let (mut pos, mut uvs, mut jts, mut idx): (Vec<[f32; 3]>, Vec<[f32; 2]>, Vec<[u8; 4]>, Vec<u32>) =
        Default::default();
    let parts = bones.iter().map(|(k, g)| (*k, g)).chain(links.iter().enumerate().map(|(n, l)| (BONES + n, l.4)));
    for (k, (verts, faces)) in parts {
        let rt = rest.get(k).context("a joint without a rest place")?;
        for (corners, uv) in faces {
            let (cx, u0, v0, _, _) = cells[&(uv[1] >> 16, uv[0] >> 16)];
            let mut ids = Vec::new();
            for (&vi, &wd) in corners.iter().zip(uv) {
                let (u, v) = ((wd & 0xFF) as usize, ((wd >> 8) & 0xFF) as usize);
                let p = placed(verts.get(vi as usize).context("a polygon's corner is no vertex")?, rt);
                pos.push(f32s(&gl_t(&p)));
                jts.push([k as u8, 0, 0, 0]);
                uvs.push([((cx + u - u0) as f64 + 0.5) / wf, ((v - v0) as f64 + 0.5) / hf].map(|f| f as f32));
                ids.push(pos.len() as u32 - 1);
            }
            // Tekken 2's front faces keep the GPU order (Tekken 3's are the other way round): quad 0 1 2 / 1 3 2
            for tri in tri_list(ids.len()) {
                for &t in tri {
                    idx.push(*ids.get(t).context("a polygon with fewer than 3 corners")?);
                }
            }
        }
    }
    // the shadow hull, after the model's vertices (its own primitive, untextured)
    let n_main = idx.len();
    let shadow = parse_shadow(model_parts.get(1).context("the model arc has no shadow hull")?)?;
    for (k, verts, faces) in &shadow {
        let rt = &rest[*k];
        let b = pos.len() as u32;
        for v in verts {
            pos.push(f32s(&gl_t(&placed(v, rt))));
            jts.push([*k as u8, 0, 0, 0]);
            uvs.push([((wf - 0.5) / wf) as f32, ((hf - 0.5) / hf) as f32]);
        }
        for f in faces {
            for tri in tri_list(f.len()) {
                for &t in tri {
                    idx.push(b + *f.get(t).context("a shadow polygon with fewer than 3 corners")? as u32);
                }
            }
        }
    }
    let nrm = normals(&pos, &idx)?;

    let mut g = Glb::new();
    let attrs = json!({
        "POSITION": g.acc_f32(&pos, Some(34962), true),
        "NORMAL": g.acc_f32(&nrm, Some(34962), false),
        "TEXCOORD_0": g.acc_f32(&uvs, Some(34962), false),
        "JOINTS_0": g.acc_u8x4(&jts, Some(34962)),
        "WEIGHTS_0": g.acc_f32(&vec![[1.0f32, 0.0, 0.0, 0.0]; pos.len()], Some(34962), false),
    });
    let image = g.view(&pil::png_bytes(&atlas)?, None);
    let j = &mut g.j;
    j.insert("images".into(), json!([{"bufferView": image, "mimeType": "image/png", "name": "atlas"}]));
    j.insert("samplers".into(), json!([{"magFilter": 9728, "minFilter": 9728, "wrapS": 33071, "wrapT": 33071}]));
    j.insert("textures".into(), json!([{"source": 0, "sampler": 0}]));
    j.insert(
        "materials".into(),
        json!([{"name": "atlas", "doubleSided": false, "alphaMode": "MASK", "alphaCutoff": 0.5,
                "pbrMetallicRoughness": {"baseColorTexture": {"index": 0}, "metallicFactor": 0, "roughnessFactor": 1}},
               {"name": "shadow", "doubleSided": true, "pbrMetallicRoughness": {"baseColorFactor": [0, 0, 0, 1],
                "metallicFactor": 0, "roughnessFactor": 1}}]),
    );
    let main_idx = g.acc_u32(&idx[..n_main], Some(34963));
    let shadow_idx = g.acc_u32(&idx[n_main..], Some(34963));
    let mut meshes = vec![
        json!({"name": format!("t2_block_{block:02}"), "primitives": [{"attributes": attrs, "indices": main_idx, "material": 0}]}),
        json!({"name": "shadow", "primitives": [{"attributes": attrs, "indices": shadow_idx, "material": 1}],
               "extras": {"note": "flatten onto the floor in world space along the stage's lighting.shadow (glTF: x += y a, z -= y b, y = 0) and fill with its shadow colour"}}),
    ];

    let mut nodes: Vec<Value> = Vec::new();
    for k in 0..BONES {
        let tl = if parents[k] >= 0 { offsets[k] } else { [0.0; 3] };
        let mut nd =
            json!({"name": BONE_NAMES[k], "translation": floats(&gl_t(&tl)), "rotation": [0.0, 0.0, 0.0, 1.0]});
        let kids: Vec<usize> = (0..BONES).filter(|&c| parents[c] == k as i16).collect();
        if !kids.is_empty() {
            nd["children"] = json!(kids);
        }
        nodes.push(nd);
    }
    let mut ibm: Vec<[f32; 16]> = Vec::new();
    let mut push_ibm = |(r, t): &(M3, V3)| -> Result<()> {
        let (r, t) = to_gl(r, t);
        let inv = np::inv4(&np::rt4(&r, &t)).context("a singular bind matrix")?;
        // .T.flatten(): column-major
        let mut m = [0f32; 16];
        for c in 0..4 {
            for rr in 0..4 {
                m[c * 4 + rr] = inv[rr][c] as f32;
            }
        }
        ibm.push(m);
        Ok(())
    };
    for rt in &rest[..BONES] {
        push_ibm(rt)?;
    }
    // the chains' joints after the bones' (node indices after the mesh nodes, appended below)
    let mut chain_nodes = Vec::new();
    for (n, &(c, i, bone, length, _)) in links.iter().enumerate() {
        push_ibm(&rest[BONES + n])?;
        let tg = gl_t(&if i != 0 { [length as f64, 0.0, 0.0] } else { [0.0; 3] });
        let parent = if i == 0 {
            BONE_NAMES.get(bone as usize).context("a chain hangs from no bone")?.to_string()
        } else {
            format!("chain{c}_{}", i - 1)
        };
        chain_nodes.push(
            json!({"name": format!("chain{c}_{i}"), "translation": floats(&tg), "rotation": [0.0, 0.0, 0.0, 1.0],
                                "extras": {"chain": c, "link": i, "length": SCALE * length as f64, "parent": parent}}),
        );
    }
    let ibm_acc = g.acc_f32(&ibm, None, false);
    let mut joints: Vec<usize> = (0..BONES).collect();
    nodes.push(json!({"name": "mesh", "mesh": 0, "skin": 0}));
    nodes.push(json!({"name": "shadow", "mesh": 1, "skin": 0}));
    let mut roots = vec![0, BONES, BONES + 1];
    if let Some((frames, faces)) = &wings {
        let (r, t) = rest[1];
        let (mut wp, mut wu, mut wi) = (Vec::new(), Vec::new(), Vec::new());
        for (corners, uv) in faces {
            let (cx, u0, v0, _, _) = cells[&(uv[1] >> 16, uv[0] >> 16)];
            let mut ids = Vec::new();
            for (&vi, &wd) in corners.iter().zip(uv) {
                wp.push(vi as usize);
                let (u, v) = ((wd & 0xFF) as usize, ((wd >> 8) & 0xFF) as usize);
                wu.push([((cx + u - u0) as f64 + 0.5) / wf, ((v - v0) as f64 + 0.5) / hf].map(|f| f as f32));
                ids.push(wp.len() as u32 - 1);
            }
            for tri in tri_list(ids.len()) {
                for &k in tri {
                    wi.push(*ids.get(k).context("a wing polygon with fewer than 3 corners")?);
                }
            }
        }
        let f: Vec<Vec<[f32; 3]>> = frames
            .iter()
            .map(|fr| {
                wp.iter()
                    .map(|&vi| Ok(f32s(&gl_t(&placed(fr.get(vi).context("a wing corner is no vertex")?, &(r, t))))))
                    .collect::<Result<Vec<_>>>()
            })
            .collect::<Result<_>>()?;
        let f0 = f.first().context("no wing frames")?;
        let wattrs = json!({
            "POSITION": g.acc_f32(f0, Some(34962), true),
            "TEXCOORD_0": g.acc_f32(&wu, Some(34962), false),
            "JOINTS_0": g.acc_u8x4(&vec![[1, 0, 0, 0]; wp.len()], Some(34962)),
            "WEIGHTS_0": g.acc_f32(&vec![[1.0f32, 0.0, 0.0, 0.0]; wp.len()], Some(34962), false),
        });
        let targets: Vec<Value> = f
            .iter()
            .map(|fr| {
                let d: Vec<[f32; 3]> =
                    fr.iter().zip(f0).map(|(a, b)| [a[0] - b[0], a[1] - b[1], a[2] - b[2]]).collect();
                json!({"POSITION": g.acc_f32(&d, None, true)})
            })
            .collect();
        let mut clips = Vec::new();
        for k in 0..8 {
            let o = off(disc.u32(WING_CLIPS + 4 * k)?);
            let tail = py_slice(&disc.exe, o, usize::MAX);
            let end = tail.iter().position(|b| *b == 0xFF).context("a wing clip without its end")?;
            clips.push(json!(tail[..end]));
        }
        let modes: Vec<Value> =
            WING_MODES.iter().map(|m| json!(m.iter().map(|(a, b)| json!([a, b])).collect::<Vec<_>>())).collect();
        let wing_idx = g.acc_u32(&wi, Some(34963));
        meshes.push(json!({"name": "wings", "primitives": [{"attributes": wattrs, "targets": targets, "material": 0, "indices": wing_idx}],
                           "weights": vec![0.0; f.len()],
                           "extras": {"frames": f.len(), "clips": clips, "modes": modes,
                                      "note": "morph target k = wing frame k (weight 1, the rest 0); a clip steps one frame number per game frame; the fighter's wing mode picks the clips"}}));
        roots.push(nodes.len());
        nodes.push(json!({"name": "wings", "mesh": meshes.len() - 1, "skin": 0}));
    }
    for nd in chain_nodes {
        let k = nodes.len();
        let parent_name = nd["extras"]["parent"].clone();
        nodes.push(nd);
        let parent = nodes
            .iter()
            .position(|x| x.get("name") == Some(&parent_name))
            .context("a chain link without its parent")?;
        match nodes[parent].get_mut("children").and_then(Value::as_array_mut) {
            Some(c) => c.push(json!(k)),
            None => nodes[parent]["children"] = json!([k]),
        }
        joints.push(k);
    }
    let j = &mut g.j;
    j.insert("meshes".into(), json!(meshes));
    j.insert("nodes".into(), json!(nodes));
    j.insert("skins".into(), json!([{"joints": joints, "skeleton": 0, "inverseBindMatrices": ibm_acc}]));
    j.insert("scenes".into(), json!([{"name": format!("t2_block_{block:02}"), "nodes": roots}]));
    j.insert("scene".into(), json!(0));

    // animations: motion_NNN in the order the moves first use them
    let mut anims = Vec::new();
    let mut times_of: HashMap<usize, usize> = HashMap::new();
    let mut dec = Decoder::new(&disc.exe, moves);
    for (mi, o) in motions_of(moves)?.into_iter().enumerate() {
        let frames = dec.decode(moves, o)?;
        let (rot, root_t) = clip_tracks(&trig, &frames);
        anims.push(animation(&mut g, &mut times_of, &format!("motion_{mi:03}"), frames.len(), &rot, &root_t));
    }
    g.j.insert("animations".into(), json!(anims));
    g.write(out, out_path)?;
    Ok((n_main, n_main / 3, anims.len()))
}

/// The times accessor of an N-frame clip (one per N).
pub fn times(g: &mut Glb, times_of: &mut HashMap<usize, usize>, n: usize) -> usize {
    *times_of.entry(n).or_insert_with(|| {
        let t: Vec<[f32; 1]> = (0..n).map(|i| [i as f32 / FPS as f32]).collect();
        g.acc_f32(&t, None, true)
    })
}

/// A clip: each bone's rotation track (int16 quaternions), then the root's translation.
fn animation(
    g: &mut Glb,
    times_of: &mut HashMap<usize, usize>,
    name: &str,
    n: usize,
    rot: &[Vec<[f64; 4]>],
    root_t: &[V3],
) -> Value {
    let t = times(g, times_of, n);
    let (mut samplers, mut channels) = (Vec::new(), Vec::new());
    for (k, r) in rot.iter().enumerate() {
        let q: Vec<[i16; 4]> = r.iter().map(q16).collect();
        samplers.push(json!({"input": t, "output": g.acc_q16(&q), "interpolation": "LINEAR"}));
        channels.push(json!({"sampler": samplers.len() - 1, "target": {"node": k, "path": "rotation"}}));
    }
    let rt: Vec<[f32; 3]> = root_t.iter().map(f32s).collect();
    samplers.push(json!({"input": t, "output": g.acc_f32(&rt, None, false), "interpolation": "LINEAR"}));
    channels.push(json!({"sampler": samplers.len() - 1, "target": {"node": 0, "path": "translation"}}));
    json!({"name": name, "samplers": samplers, "channels": channels})
}

/// Smooth normals in float32 (numpy's cross, add.at and norm).
fn normals(pos: &[[f32; 3]], idx: &[u32]) -> Result<Vec<[f32; 3]>> {
    let mut nrm = vec![[0f32; 3]; pos.len()];
    let tris: Vec<[usize; 3]> = idx.as_chunks::<3>().0.iter().map(|t| t.map(|i| i as usize)).collect();
    let mut fns = Vec::with_capacity(tris.len());
    for t in &tris {
        let [p0, p1, p2] = t.map(|i| pos[i]);
        let a = [p1[0] - p0[0], p1[1] - p0[1], p1[2] - p0[2]];
        let b = [p2[0] - p0[0], p2[1] - p0[1], p2[2] - p0[2]];
        fns.push([a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]);
    }
    for c in 0..3 {
        for (t, f) in tris.iter().zip(&fns) {
            let n = &mut nrm[t[c]];
            for i in 0..3 {
                n[i] += f[i];
            }
        }
    }
    for n in &mut nrm {
        let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-9);
        *n = n.map(|v| v / l);
    }
    Ok(nrm)
}

/// Block b: character b // 2 (the EXE's table order), costume b % 2 + 1; Devil's second is Angel, Roger's second
/// costume is Alex, and block 46 is a third Kazuya (its stage records match block 0's). (From the models; the select
/// screen's own mapping is not traced yet.)
pub fn block_name(block: usize, names: &HashMap<i64, String>) -> String {
    let get = |k: i64, d: &str| names.get(&k).cloned().unwrap_or_else(|| d.to_string());
    match block {
        35 => title(&get(24, "Angel")), // Devil's second slot (the ending table, record 20)
        45 => title(&get(23, "Alex")),
        46 => format!("{} (costume 3)", title(&get(0, "Kazuya"))),
        _ => format!("{} (costume {})", title(&get(block as i64 / 2, &format!("block {block}"))), block % 2 + 1),
    }
}

/// Read a JSON file written by the Python tools (keys in their order).
pub fn load_json(fs: &dyn Vfs, path: &str) -> Result<Value> {
    serde_json::from_slice(&fs.read(path)?).with_context(|| format!("{path} is not JSON"))
}

pub fn write_json(fs: &dyn Vfs, path: &str, v: &Value, style: pyjson::Style) -> Result<()> {
    fs.write(path, pyjson::to_string(v, style).as_bytes())
}

/// `--jobs N` of a command line (conv_io::jobs_from) and the arguments without it.
pub fn take_jobs(args: &[String]) -> (usize, Vec<String>) {
    let mut rest = args.to_vec();
    if let Some(k) = rest.iter().position(|a| a == "--jobs") {
        rest.drain(k..(k + 2).min(rest.len()));
    }
    (conv_io::jobs_from(args), rest)
}

pub fn main(args: &[String]) -> Result<()> {
    let (jobs, args) = take_jobs(args);
    let [src, out, rest @ ..] = &args[..] else {
        bail!("usage: t2_convert export_characters BIN OUTDIR [block,...] [--jobs N]")
    };
    let only: Option<Vec<usize>> = match rest.first() {
        Some(s) => Some(
            s.split(',').map(|v| v.trim().parse().with_context(|| format!("bad block {v}"))).collect::<Result<_>>()?,
        ),
        None => None,
    };
    let ctx = Ctx { out: &StdFs(out.into()), log: &conv_io::print_line, jobs };
    run(&ctx, &StdDisc(src.into()), "", only.as_deref())
}

/// The character blocks (`only` those) as glTF in `dir` of the output, with index.json and characters.json.
pub fn run(ctx: &Ctx, src: &dyn DiscFile, dir: &str, only: Option<&[usize]>) -> Result<()> {
    let all: Vec<usize> = (0..BLOCKS).collect();
    let only = only.unwrap_or(&all);
    let disc = Disc::open(src)?;
    let chars = disc.characters()?;
    let next = std::sync::atomic::AtomicUsize::new(0);
    let done = std::sync::Mutex::new(Vec::new());
    let work = || {
        loop {
            let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let Some(&b) = only.get(i) else { break };
            let file = format!("t2_block_{b:02}.glb");
            let r = convert(&disc, b, ctx.out, &join(dir, &file)).with_context(|| format!("block {b}"));
            if let Ok((nv, nt, na)) = &r {
                ctx.log(&format!("{file} verts {nv} tris {nt} animations {na}"));
            }
            let failed = r.is_err();
            if let Ok(mut d) = done.lock() {
                d.push((b, file, r.map(|x| x.2)));
            }
            if failed {
                next.store(usize::MAX / 2, std::sync::atomic::Ordering::Relaxed);
            }
        }
    };
    if ctx.jobs <= 1 {
        work();
    } else {
        std::thread::scope(|s| {
            for _ in 0..ctx.jobs {
                s.spawn(work);
            }
        });
    }
    let done = done.into_inner().map_err(|_| anyhow::anyhow!("a worker failed"))?;
    let mut index = Vec::new();
    for (b, file, r) in done {
        index.push((b, file, r?));
    }
    // index.json: the viewer's list (id, name, file); characters.json: the EXE's character table
    let names: HashMap<i64, String> = chars.iter().map(|c| (c.id, c.name.clone())).collect();
    let ipath = join(dir, "index.json");
    let mut prev: BTreeMap<i64, Value> = BTreeMap::new();
    if ctx.out.exists(&ipath) {
        for e in load_json(ctx.out, &ipath)?.as_array().context("index.json is not a list")? {
            prev.insert(e["id"].as_i64().context("an index.json entry without an id")?, e.clone());
        }
    }
    for (b, file, na) in index {
        prev.insert(b as i64, json!({"id": b, "name": block_name(b, &names), "file": file, "motions": na}));
    }
    write_json(ctx.out, &ipath, &json!(prev.into_values().collect::<Vec<_>>()), pyjson::INDENT1)?;
    let cj: Vec<Value> =
        chars.iter().map(|c| json!({"id": c.id, "name": c.name, "stage": c.stage, "order": c.order})).collect();
    write_json(ctx.out, &join(dir, "characters.json"), &json!(cj), pyjson::INDENT1)
}
