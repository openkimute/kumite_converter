//! bsp2glb.py's `export` (the glb; not its OBJ): the same glTF, in the same order. Images first (PNG, the pixels as
//! Pillow's; the compressed bytes differ), one mesh per brush model with faces (worldspawn first), one primitive per
//! texture (by name), unlit materials (`{` textures alpha-masked), one sampler, the `info_player*` spawns as nodes,
//! the scene named after the map. Half-Life (x, y, z) inches become glTF (x, z, -y) x 0.0254 m.

use crate::bsp::{Bsp, numbers};
use crate::wad::{self, Miptex};
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::collections::HashMap;

const HU_TO_M: f64 = 0.0254;

/// `to_gltf_axes`
fn axes(v: [f64; 3]) -> [f64; 3] {
    [v[0], v[2], -v[1]]
}

/// 32-bit words, little-endian (`.astype('<f4').tobytes()`, `'<u4'`)
fn le(v: impl Iterator<Item = u32>) -> Vec<u8> {
    v.flat_map(u32::to_le_bytes).collect()
}

/// `Image.fromarray(a).save(f, 'PNG')` (RGB or RGBA)
fn png(px: &[u8], w: usize, h: usize, ch: usize) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut e = png::Encoder::new(&mut out, w as u32, h as u32);
    e.set_color(if ch == 4 { png::ColorType::Rgba } else { png::ColorType::Rgb });
    e.set_depth(png::BitDepth::Eight);
    let mut wr = e.write_header()?;
    wr.write_image_data(px)?;
    wr.finish()?;
    Ok(out)
}

/// The level `name` as a glb (its JSON, and its binary padded to 4 bytes), the textures not in the BSP from `wads`
/// (a missing one: a placeholder and a warning on `log`).
pub fn build(name: &str, bsp: &Bsp, wads: &HashMap<String, Miptex>, log: &dyn Fn(&str)) -> Result<(Value, Vec<u8>)> {
    let get = |t: &str| bsp.embedded.get(t).or_else(|| wads.get(t));
    let (groups, mut missing) = bsp.groups(|t| get(t).map(|m| (m.w, m.h)))?;
    if !missing.is_empty() {
        missing.sort();
        log(&format!("{name}: textures missing (placeholder used): {}", missing.join(", ")));
    }
    let mut buf: Vec<u8> = Vec::new();
    let (mut views, mut accessors, mut meshes, mut nodes) = (vec![], vec![], vec![], vec![]);
    let (mut materials, mut textures, mut images) = (vec![], vec![], vec![]);
    let mut add_view = |buf: &mut Vec<u8>, data: &[u8], target: Option<u32>| {
        buf.resize(buf.len().div_ceil(4) * 4, 0);
        let mut v = json!({"buffer": 0, "byteOffset": buf.len(), "byteLength": data.len()});
        if let Some(t) = target {
            v["target"] = json!(t);
        }
        buf.extend_from_slice(data);
        views.push(v);
        views.len() - 1
    };
    // materials and textures, one per texture name
    let mut names: Vec<&str> = groups.iter().map(|g| g.tex.as_str()).collect();
    names.sort();
    names.dedup();
    let mut mat_index = HashMap::new();
    for &t in &names {
        let data = match get(t).filter(|_| t != "__missing__") {
            Some(m) => {
                let (px, ch) = wad::image(m);
                png(&px, m.w, m.h, ch)
            }
            None => png(&wad::placeholder(), 64, 64, 3),
        }
        .with_context(|| format!("{name}: texture {t}"))?;
        let view = add_view(&mut buf, &data, None);
        images.push(json!({"bufferView": view, "mimeType": "image/png", "name": t}));
        textures.push(json!({"source": images.len() - 1, "sampler": 0}));
        let mut mat = json!({
            "name": t,
            "pbrMetallicRoughness": {"baseColorTexture": {"index": textures.len() - 1}, "metallicFactor": 0.0,
                                     "roughnessFactor": 1.0},
            "doubleSided": false,
            "extensions": {"KHR_materials_unlit": {}},
        });
        if t.starts_with('{') {
            mat["alphaMode"] = json!("MASK");
            mat["alphaCutoff"] = json!(0.5);
        }
        materials.push(mat);
        mat_index.insert(t, materials.len() - 1);
    }
    // meshes per brush model (worldspawn, then the brush entities), one primitive per texture
    let model_entity = bsp.model_entities();
    for mi in 0..bsp.models() {
        let mut prims: Vec<_> = groups.iter().filter(|g| g.model == mi).collect();
        if prims.is_empty() {
            continue;
        }
        prims.sort_by(|a, b| a.tex.cmp(&b.tex));
        let label = match model_entity.get(&mi) {
            _ if mi == 0 => "worldspawn".to_string(),
            Some(e) => {
                let cls = e.get("classname").map_or("brush", String::as_str);
                e.get("targetname").map_or(cls.to_string(), |t| format!("{cls}_{t}"))
            }
            None => format!("model_{mi}"),
        };
        let mut primitives = Vec::new();
        for g in prims {
            let pos: Vec<[f32; 3]> = g.pos.iter().map(|&p| axes(p).map(|v| (v * HU_TO_M) as f32)).collect();
            let nrm: Vec<[f32; 3]> = g
                .nrm
                .iter()
                .map(|&n| {
                    let n = axes(n);
                    let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-9);
                    n.map(|v| (v / len) as f32)
                })
                .collect();
            let (mut mn, mut mx) = (pos.first().copied().unwrap_or_default(), pos.first().copied().unwrap_or_default());
            for p in &pos {
                for c in 0..3 {
                    // numpy's min and max: -0.0 below 0.0
                    if p[c].total_cmp(&mn[c]).is_lt() {
                        mn[c] = p[c];
                    }
                    if p[c].total_cmp(&mx[c]).is_gt() {
                        mx[c] = p[c];
                    }
                }
            }
            let vp = add_view(&mut buf, &le(pos.iter().flatten().map(|v| v.to_bits())), Some(34962));
            let vn = add_view(&mut buf, &le(nrm.iter().flatten().map(|v| v.to_bits())), Some(34962));
            let vt = add_view(&mut buf, &le(g.uv.iter().flatten().map(|&v| (v as f32).to_bits())), Some(34962));
            let vi = add_view(&mut buf, &le(g.tri.iter().flatten().copied()), Some(34963));
            let f = |a: [f32; 3]| a.map(f64::from);
            accessors.push(json!({"bufferView": vp, "componentType": 5126, "count": pos.len(), "type": "VEC3",
                                  "min": f(mn), "max": f(mx)}));
            accessors.push(json!({"bufferView": vn, "componentType": 5126, "count": nrm.len(), "type": "VEC3"}));
            accessors.push(json!({"bufferView": vt, "componentType": 5126, "count": g.uv.len(), "type": "VEC2"}));
            accessors.push(json!({"bufferView": vi, "componentType": 5125, "count": g.tri.len() * 3,
                                  "type": "SCALAR"}));
            let a = accessors.len();
            primitives.push(json!({"attributes": {"POSITION": a - 4, "NORMAL": a - 3, "TEXCOORD_0": a - 2},
                                   "indices": a - 1, "material": mat_index.get(g.tex.as_str())}));
        }
        meshes.push(json!({"name": label, "primitives": primitives}));
        nodes.push(json!({"name": label, "mesh": meshes.len() - 1}));
    }
    // the spawn points
    for e in &bsp.entities {
        let cls = e.get("classname").map_or("", String::as_str);
        if let (true, Some(o)) = (cls.starts_with("info_player"), e.get("origin")) {
            let o = numbers(o)?;
            let &[x, y, z, ..] = o.as_slice() else { anyhow::bail!("{name}: a spawn's origin is not 3 numbers") };
            nodes.push(json!({"name": cls, "translation": axes([x, y, z]).map(|v| v * HU_TO_M)}));
        }
    }
    let gltf = json!({
        "asset": {"version": "2.0", "generator": "bsp2glb (Half-Life CS map converter)"},
        "extensionsUsed": ["KHR_materials_unlit"],
        "scene": 0,
        "scenes": [{"name": name, "nodes": (0..nodes.len()).collect::<Vec<_>>()}],
        "nodes": nodes,
        "meshes": meshes,
        "accessors": accessors,
        "bufferViews": views,
        "buffers": [{"byteLength": buf.len()}],
        "materials": materials,
        "textures": textures,
        "images": images,
        "samplers": [{"magFilter": 9729, "minFilter": 9987, "wrapS": 10497, "wrapT": 10497}],
    });
    buf.resize(buf.len().div_ceil(4) * 4, 0);
    Ok((gltf, buf))
}
