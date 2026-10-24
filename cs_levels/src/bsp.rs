//! bsp2glb's `load_bsp` and `build`: a GoldSrc (Half-Life, version 30) map's entities, embedded textures and
//! faces, the faces gathered into polygons per (brush model, texture). No lighting: the levels are unlit.

use crate::wad::{Miptex, cstr, i32at, parse_miptex, u32at};
use anyhow::{Context, Result, bail};
use std::collections::HashMap;

/// Faces with these textures are not drawn (sky, clip and trigger brushes, ...).
const SKIP_PREFIXES: [&str; 11] =
    ["sky", "aaatrigger", "clip", "null", "origin", "hint", "skip", "trigger", "bevel", "noclip", "noshow"];

const ENTITIES: usize = 0;
const PLANES: usize = 1;
const TEXTURES: usize = 2;
const VERTICES: usize = 3;
const TEXINFO: usize = 6;
const FACES: usize = 7;
const EDGES: usize = 12;
const SURFEDGES: usize = 13;
const MODELS: usize = 14;

/// An entity's keys (a key given twice: its last value, as a dict of them).
pub type Entity = HashMap<String, String>;

pub struct Bsp<'a> {
    lumps: Vec<&'a [u8]>,
    pub entities: Vec<Entity>,
    pub embedded: HashMap<String, Miptex>,
    /// Textures named but not embedded (in a WAD).
    pub external: usize,
}

/// One brush model's faces of one texture: their vertices (Half-Life units, the entity's origin added), the face
/// normals, UVs and triangle fans.
pub struct Group {
    pub model: usize,
    pub tex: String,
    pub pos: Vec<[f64; 3]>,
    pub nrm: Vec<[f64; 3]>,
    pub uv: Vec<[f64; 2]>,
    pub tri: Vec<[u32; 3]>,
}

fn f32at(d: &[u8], o: usize) -> Option<f64> {
    u32at(d, o).map(|v| f32::from_bits(v) as f64)
}

fn u16at(d: &[u8], o: usize) -> Option<usize> {
    d.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]]) as usize)
}

/// `[float(x) for x in s.split()]`
pub fn numbers(s: &str) -> Result<Vec<f64>> {
    s.split_whitespace().map(|x| x.parse().with_context(|| format!("not a number: {x}"))).collect()
}

/// Python's whitespace (`\s` of a str pattern).
fn py_space(c: char) -> bool {
    c.is_whitespace() || ('\x1c'..='\x1f').contains(&c)
}

/// `"([^"]*)"\s+"([^"]*)"` matched at `s[i]` (a quote): the key, the value and where the match ends.
fn pair(s: &[char], i: usize) -> Option<(String, String, usize)> {
    let quoted = |at: usize| -> Option<(String, usize)> {
        if s.get(at) != Some(&'"') {
            return None;
        }
        let end = at + 1 + s.get(at + 1..)?.iter().position(|&c| c == '"')?;
        Some((s[at + 1..end].iter().collect(), end + 1))
    };
    let (k, e) = quoted(i)?;
    let ws = s.get(e..)?.iter().take_while(|&&c| py_space(c)).count();
    if ws == 0 {
        return None;
    }
    let (v, e) = quoted(e + ws)?;
    Some((k, v, e))
}

/// `parse_entities`: each `{...}` block's `"key" "value"` pairs (the regexes' matches).
pub fn parse_entities(text: &str) -> Vec<Entity> {
    let s: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(open) = s.get(i..).and_then(|r| r.iter().position(|&c| c == '{')) {
        let open = i + open;
        let Some(close) = s[open..].iter().position(|&c| c == '}') else { break };
        let block = &s[open + 1..open + close];
        let mut ent = Entity::new();
        let mut k = 0;
        while k < block.len() {
            match (block[k] == '"').then(|| pair(block, k)).flatten() {
                Some((key, value, end)) => {
                    ent.insert(key, value);
                    k = end;
                }
                None => k += 1,
            }
        }
        out.push(ent);
        i = open + close + 1;
    }
    out
}

impl<'a> Bsp<'a> {
    /// `load_bsp` and the entities and texture lumps of `build`.
    pub fn read(data: &'a [u8]) -> Result<Bsp<'a>> {
        let version = i32at(data, 0).context("not a BSP file")?;
        if version != 30 {
            bail!("unsupported BSP version {version} (expected 30 / GoldSrc)");
        }
        let mut lumps = Vec::new();
        for i in 0..15 {
            let (off, len) = (u32at(data, 4 + i * 8), u32at(data, 8 + i * 8));
            let lump = off.zip(len).and_then(|(o, n)| data.get(o as usize..)?.get(..n as usize));
            lumps.push(lump.with_context(|| format!("the BSP's lump {i} runs past its end"))?);
        }
        let entities = parse_entities(&lumps[ENTITIES].iter().map(|&b| b as char).collect::<String>());
        let tex = lumps[TEXTURES];
        let mut embedded = HashMap::new();
        let mut external = 0;
        for i in 0..i32at(tex, 0).unwrap_or(0).max(0) as usize {
            let ofs = i32at(tex, 4 + i * 4).context("the BSP's texture table runs past its lump")?;
            let Ok(ofs) = usize::try_from(ofs) else { continue };
            match parse_miptex(tex, ofs) {
                Some(t) => {
                    embedded.insert(t.name.clone(), t);
                }
                None => external += 1,
            }
        }
        Ok(Bsp { lumps, entities, embedded, external })
    }

    /// The WAD files worldspawn names (`"\halflife\valve\decals.wad;..."`), by their file names.
    pub fn wads(&self) -> Vec<String> {
        let refs = self.entities.first().and_then(|w| w.get("wad")).map_or("", String::as_str);
        refs.split(';')
            .filter(|p| !p.is_empty())
            .filter_map(|p| p.replace('\\', "/").rsplit('/').next().map(str::to_string))
            .filter(|n| !n.is_empty())
            .collect()
    }

    /// Brush model -> its entity (the last to name it).
    pub fn model_entities(&self) -> HashMap<usize, &Entity> {
        let mut out = HashMap::new();
        for ent in self.entities.iter().skip(1) {
            if let Some(m) = ent.get("model").and_then(|m| m.strip_prefix('*'))
                && let Ok(m) = m.trim().parse::<usize>()
            {
                out.insert(m, ent);
            }
        }
        out
    }

    /// The number of brush models.
    pub fn models(&self) -> usize {
        self.lumps[MODELS].len() / 64
    }

    /// `miptex_for_index`: a texinfo's texture name (None for none).
    fn texture_name(&self, miptex: i32) -> Result<Option<String>> {
        let Ok(miptex) = usize::try_from(miptex) else { return Ok(None) };
        let tex = self.lumps[TEXTURES];
        let ofs = i32at(tex, 4 + miptex * 4).context("a texinfo's texture is past the texture table")?;
        // tex_buf[ofs:ofs + 16]
        let raw = usize::try_from(ofs).ok().and_then(|o| tex.get(o..)).unwrap_or(&[]);
        Ok(Some(cstr(&raw[..raw.len().min(16)])))
    }

    /// The polygons per (brush model, texture), in the order first met, and the textures missing (`size` gives a
    /// texture's size, None when it is missing).
    pub fn groups(&self, size: impl Fn(&str) -> Option<(usize, usize)>) -> Result<(Vec<Group>, Vec<String>)> {
        let [planes, verts, texinfo, faces, edges, surfedges, models] =
            [PLANES, VERTICES, TEXINFO, FACES, EDGES, SURFEDGES, MODELS].map(|k| self.lumps[k]);
        let model_entity = self.model_entities();
        let mut groups: Vec<Group> = Vec::new();
        let mut at: HashMap<(usize, String), usize> = HashMap::new();
        let mut missing = Vec::new();
        for mi in 0..models.len() / 64 {
            let mut origin = [0.0; 3];
            if let Some(o) = model_entity.get(&mi).and_then(|e| e.get("origin")) {
                origin = numbers(o)?.try_into().ok().context("an origin is not 3 numbers")?;
            }
            let (Some(first), Some(n)) = (i32at(models, mi * 64 + 56), i32at(models, mi * 64 + 60)) else {
                bail!("a bad model")
            };
            let first = first.max(0) as usize;
            for fi in first..first + n.max(0) as usize {
                let f = fi * 20;
                let (Some(pl), Some(side), Some(fe), Some(ne), Some(ti)) = (
                    u16at(faces, f),
                    u16at(faces, f + 2),
                    i32at(faces, f + 4),
                    u16at(faces, f + 8),
                    u16at(faces, f + 10),
                ) else {
                    bail!("face {fi} is past the faces");
                };
                let t = ti * 40;
                let vecs: Vec<f64> = (0..8).filter_map(|k| f32at(texinfo, t + 4 * k)).collect();
                let (Ok(&[s0, s1, s2, s3, t0, t1, t2, t3]), Some(miptex)) =
                    (<&[f64; 8]>::try_from(vecs.as_slice()), i32at(texinfo, t + 32))
                else {
                    bail!("face {fi}'s texinfo is past the texinfos");
                };
                let tex = self.texture_name(miptex)?.unwrap_or_else(|| "__missing__".into());
                if SKIP_PREFIXES.iter().any(|p| tex.starts_with(p)) {
                    continue;
                }
                let (w, h) = size(&tex).unwrap_or_else(|| {
                    if !missing.contains(&tex) {
                        missing.push(tex.clone());
                    }
                    (64, 64)
                });
                let mut p = Vec::with_capacity(ne);
                for k in 0..ne {
                    let se = usize::try_from(fe).ok().and_then(|fe| i32at(surfedges, 4 * (fe + k)));
                    let se = se.with_context(|| format!("face {fi}'s edges are past the surfedges"))?;
                    let v = if se >= 0 {
                        u16at(edges, se as usize * 4)
                    } else {
                        u16at(edges, se.unsigned_abs() as usize * 4 + 2)
                    };
                    let v = v.with_context(|| format!("face {fi}'s edge is past the edges"))?;
                    let xyz = [0, 1, 2].map(|c| f32at(verts, v * 12 + c * 4));
                    let [Some(x), Some(y), Some(z)] = xyz else { bail!("face {fi}'s vertex is past the vertices") };
                    p.push([x, y, z]);
                }
                // the texture projection in Half-Life space (P @ vecs[0, :3] + vecs[0, 3], summed left to right)
                let mut uv: Vec<[f64; 2]> = p
                    .iter()
                    .map(|q| {
                        let s = q[0] * s0 + q[1] * s1 + q[2] * s2 + s3;
                        let t = q[0] * t0 + q[1] * t1 + q[2] * t2 + t3;
                        [s / w as f64, t / h as f64]
                    })
                    .collect();
                let nf: Vec<f64> = (0..3).filter_map(|c| f32at(planes, pl * 20 + c * 4)).collect();
                let &[n0, n1, n2] = nf.as_slice() else { bail!("face {fi}'s plane is past the planes") };
                let n = if side != 0 { [-n0, -n1, -n2] } else { [n0, n1, n2] };
                // the winding made to agree with the face's normal (Newell's normal against the plane's)
                let mut newell = [0.0; 3];
                for k in 0..p.len() {
                    let c = crate::install::cross(p[k], p[(k + 1) % p.len()]);
                    newell = [newell[0] + c[0], newell[1] + c[1], newell[2] + c[2]];
                }
                if newell[0] * n[0] + newell[1] * n[1] + newell[2] * n[2] < 0.0 {
                    p.reverse();
                    uv.reverse();
                }
                let key = (mi, tex.clone());
                let g = match at.get(&key) {
                    Some(&g) => g,
                    None => {
                        at.insert(key, groups.len());
                        groups.push(Group { model: mi, tex, pos: vec![], nrm: vec![], uv: vec![], tri: vec![] });
                        groups.len() - 1
                    }
                };
                let g = &mut groups[g];
                let base = g.pos.len() as u32;
                for j in 1..p.len().saturating_sub(1) as u32 {
                    g.tri.push([base, base + j, base + j + 1]);
                }
                g.pos.extend(p.iter().map(|q| [q[0] + origin[0], q[1] + origin[1], q[2] + origin[2]]));
                g.nrm.extend(std::iter::repeat_n(n, p.len()));
                g.uv.extend(uv);
            }
        }
        Ok((groups, missing))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entities_as_the_regexes() {
        // re.findall's matches, a failed match retried one character on
        let e = parse_entities(
            "{\n\"classname\" \"worldspawn\"\n\"wad\" \"\\a\\b.wad;c.wad\"\n}\n{ \"a\" x \"b\" \"c\" }{\"k\"\"v\" \"k\"\t\"w\"} { \"no\" \"end\" ",
        );
        let ent = |kv: &[(&str, &str)]| kv.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<Entity>();
        assert_eq!(
            e,
            [ent(&[("classname", "worldspawn"), ("wad", "\\a\\b.wad;c.wad")]), ent(&[("b", "c")]), ent(&[("v", "k")])]
        );
    }
}
