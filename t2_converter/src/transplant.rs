//! Bring Tekken 2 fighters into OpenKumite: their model and motions rebuilt on the Tekken 3 skeleton.
//!
//! usage: t2_convert transplant "Tekken 2 (USA) (v1.1) (Track 1).bin" T3_ASSETS [block,...]
//!
//! Writes T3_ASSETS/characters/model_9NN.glb and gamedata/movesets/moveset_t2_<name>.json for every Tekken 2 fighter
//! (costume block b -> model 900 + b), and registers them in movesets/index.json and roster.json. The move sets:
//! transplant_moves.rs.
//!
//! The two games' skeletons have the same bones (18; Tekken 3 adds head_b, an unanimated child of the head) and the
//! same sides (A at +z), but each bone's local axes differ by a fixed turn D (Tekken 3 lays every bone along its
//! +x). D was measured by comparing the stance poses of the fighters both games have (Paul, Law, Lei, King,
//! Yoshimitsu, Nina, Kuma, Anna, Heihachi, Jin / Kazuya): the averaged turns snap to whole quarter turns (King's pair
//! within 1 degree), and with them every Tekken 2 child offset lies along its parent's +x, as in Tekken 3.
//!
//! With W3 = W2 · D per bone (the same body, Tekken 3's axes):
//!   rest translation  t3 = D_parent^T t2;  rest rotation  D_parent^T D_bone (Tekken 2's rest is all zero angles)
//!   motion rotation   L3 = D_parent^T L2 D_bone;  the root's translation is unchanged (D_root = I)
//!   skin              the vertices stay put; inverse bind = (B2 D)^-1
//! So Tekken 3's motions play on these fighters by joint name (throws, shared reactions), and theirs on Tekken 3's.

use crate::chain_sim::{Bones, ChainSim};
use crate::export_characters::{self as t2, BONE_NAMES, Disc, f32s, load_json, off, q16, title, write_json};
use crate::export_media as em;
use crate::np::{self, I3, M4, mm3, mm4, mv3, t3};
use crate::ps1::{
    C, Glb, Iso, M3, Rgba, SCALE, V3, export_bank, floats, i16_, py_round, quat, render, scan_lz_tims, u8_, u16_, u32_,
    walk_tims,
};
use crate::transplant_moves as tm;
use crate::{pil, pyjson};
use anyhow::{Context, Result, bail};
use conv_io::{Ctx, DiscFile, StdDisc, StdFs, Vfs, join};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap};

const T3_ORDER: [&str; 19] = [
    "root",
    "chest",
    "pelvis",
    "thigh_A",
    "shin_A",
    "foot_A",
    "thigh_B",
    "shin_B",
    "foot_B",
    "clavicle_A",
    "upperarm_A",
    "forearm_A",
    "hand_A",
    "clavicle_B",
    "upperarm_B",
    "forearm_B",
    "hand_B",
    "head",
    "head_b",
];

pub fn t3_parent(n: &str) -> Option<&'static str> {
    Some(match n {
        "chest" | "pelvis" => "root",
        "thigh_A" | "thigh_B" => "pelvis",
        "shin_A" => "thigh_A",
        "foot_A" => "shin_A",
        "shin_B" => "thigh_B",
        "foot_B" => "shin_B",
        "clavicle_A" | "clavicle_B" | "head" => "chest",
        "upperarm_A" => "clavicle_A",
        "forearm_A" => "upperarm_A",
        "hand_A" => "forearm_A",
        "upperarm_B" => "clavicle_B",
        "forearm_B" => "upperarm_B",
        "hand_B" => "forearm_B",
        "head_b" => "head",
        _ => return None,
    })
}

/// Each bone's turn D (identity for the rest; head_b as the head).
fn d_of(n: &str) -> M3 {
    match n {
        "chest" => [[-1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, 1.0]],
        "head" | "head_b" => [[0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        "clavicle_A" => [[0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]],
        "clavicle_B" => [[0.0, -1.0, 0.0], [0.0, 0.0, 1.0], [-1.0, 0.0, 0.0]],
        "foot_A" | "foot_B" => [[0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
        _ => I3,
    }
}

/// Tekken 2 bone index -> joint name (the same names in both)
pub const T2_TO_T3: [&str; 18] = BONE_NAMES;

pub fn qmat(q: &[f64; 4]) -> M3 {
    let n = np::dot4(q, q).sqrt();
    let [x, y, z, w] = q.map(|v| v / n);
    [
        [1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y - z * w), 2.0 * (x * z + y * w)],
        [2.0 * (x * y + z * w), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z - x * w)],
        [2.0 * (x * z - y * w), 2.0 * (y * z + x * w), 1.0 - 2.0 * (x * x + y * y)],
    ]
}

fn vec4(v: &[f64]) -> [f64; 4] {
    [0, 1, 2, 3].map(|i| v.get(i).copied().unwrap_or(0.0))
}
fn vec3(v: &[f64]) -> V3 {
    [0, 1, 2].map(|i| v.get(i).copied().unwrap_or(0.0))
}
fn json_f(v: &Value) -> Vec<f64> {
    v.as_array().map_or(vec![], |a| a.iter().filter_map(Value::as_f64).collect())
}
fn int(v: &Value) -> i64 {
    v.as_i64().unwrap_or(0)
}
fn arr(v: &Value) -> &[Value] {
    v.as_array().map_or(&[], Vec::as_slice)
}
fn obj(v: &mut Value) -> Result<&mut Map<String, Value>> {
    v.as_object_mut().context("expected a JSON object")
}
fn list(v: &mut Value) -> Result<&mut Vec<Value>> {
    v.as_array_mut().context("expected a JSON list")
}

/// A glb read back: its document, binary chunk, accessors as rows of floats (normalized ones / 32767).
pub struct GlbIn {
    pub j: Value,
    bin: Vec<u8>,
}

impl GlbIn {
    pub fn read(fs: &dyn Vfs, path: &str) -> Result<GlbIn> {
        let b = fs.read(path)?;
        let jl = u32_(&b, 12)? as usize;
        let j = serde_json::from_slice(b.get(20..20 + jl).context("a glb's JSON is short")?)
            .with_context(|| format!("{path} has no glTF JSON"))?;
        Ok(GlbIn { j, bin: b.get(20 + jl + 8..).unwrap_or(&[]).to_vec() })
    }

    pub fn acc(&self, k: &Value) -> Result<Vec<Vec<f64>>> {
        let a = &self.j["accessors"][k.as_u64().context("an accessor index")? as usize];
        let v = &self.j["bufferViews"][int(&a["bufferView"]) as usize];
        let (size, scale) = match int(&a["componentType"]) {
            5126 => (4, 1.0),
            5122 => (2, 32767.0),
            5121 => (1, 1.0),
            5125 => (4, 1.0),
            t => bail!("component type {t}"),
        };
        let n = match a["type"].as_str() {
            Some("SCALAR") => 1,
            Some("VEC2") => 2,
            Some("VEC3") => 3,
            Some("VEC4") => 4,
            Some("MAT4") => 16,
            t => bail!("accessor type {t:?}"),
        };
        let count = int(&a["count"]) as usize;
        let o = int(&v["byteOffset"]) as usize + int(&a["byteOffset"]) as usize;
        let data = self.bin.get(o..o + count * n * size).context("an accessor runs past the glb")?;
        let ct = int(&a["componentType"]);
        let norm = a["normalized"].as_bool().unwrap_or(false);
        let vals = data.chunks_exact(size).map(|c| {
            let x = match ct {
                5126 => f32::from_le_bytes([c[0], c[1], c[2], c[3]]) as f64,
                5122 => i16::from_le_bytes([c[0], c[1]]) as f64,
                5121 => c[0] as f64,
                _ => u32::from_le_bytes([c[0], c[1], c[2], c[3]]) as f64,
            };
            if norm { x / scale } else { x }
        });
        let vals: Vec<f64> = vals.collect();
        Ok(vals.chunks_exact(n).map(<[f64]>::to_vec).collect())
    }

    pub fn image(&self, k: usize) -> Result<&[u8]> {
        let v = &self.j["bufferViews"][int(&self.j["images"][k]["bufferView"]) as usize];
        let o = int(&v["byteOffset"]) as usize;
        self.bin.get(o..o + int(&v["byteLength"]) as usize).context("an image runs past the glb")
    }
}

/// Wing modes for the wing clips (Devil, Angel): the stance clip, the block reactions' clips, the moves' air windows.
#[derive(Default)]
pub struct WingInfo {
    pub stance: Option<String>,
    pub guard: BTreeSet<String>,
    pub air: HashMap<Option<String>, (u8, u8)>,
}

/// A clip: name, {bone: quaternions}, root translations.
pub type Clip = (String, BTreeMap<String, Vec<[f64; 4]>>, Vec<V3>);

/// extra: more Tekken 2 clips (Tekken 2 axes, by Tekken 2 bone name); donor: a Tekken 3 glb whose clips are copied
/// by joint name (same axes); prefix: the name of this character's own clips (motion_NNN -> prefix NNN).
#[allow(clippy::too_many_arguments)]
pub fn transplant(
    fs: &dyn Vfs,
    src_glb: &str,
    out_path: &str,
    name: &str,
    extra: &[Clip],
    donor: Option<&str>,
    prefix: &str,
    chain_data: Option<(&[u8], &[u8], u32)>,
    wing_info: Option<&WingInfo>,
) -> Result<usize> {
    let src = GlbIn::read(fs, src_glb)?;
    let j = &src.j;
    let nodes2 = arr(&j["nodes"]);
    let names: Vec<Option<&str>> = nodes2.iter().map(|n| n["name"].as_str()).collect();
    // the joints after Tekken 3's: the chains' links (export_characters: chain<c>_<i>, hanging on a bone or the link
    // before), then the wings' vertices (wing<k>, on the chest)
    let chains: Vec<&Value> = nodes2.iter().filter(|n| n["extras"].get("chain").is_some()).collect();
    let mut parent_of: HashMap<String, String> =
        T3_ORDER.iter().filter_map(|n| Some((n.to_string(), t3_parent(n)?.to_string()))).collect();
    for n in &chains {
        parent_of.insert(n["name"].as_str().unwrap_or("").into(), n["extras"]["parent"].as_str().unwrap_or("").into());
    }
    let mut order: Vec<String> = T3_ORDER
        .iter()
        .map(|s| s.to_string())
        .chain(chains.iter().map(|n| n["name"].as_str().unwrap_or("").to_string()))
        .collect();
    let k2 = |n: &str| names.iter().position(|x| *x == Some(n));
    let dn = |n: &str| d_of(n);

    // Tekken 2 bind (rest) world matrices from the skin
    let skin = &j["skins"][0];
    let joints2: Vec<usize> = arr(&skin["joints"]).iter().map(|v| int(v) as usize).collect();
    let ibm2 = src.acc(&skin["inverseBindMatrices"])?;
    let mut b2: HashMap<String, M4> = HashMap::new();
    for (jn, m) in joints2.iter().zip(&ibm2) {
        let mut mm = [[0.0; 4]; 4];
        for (i, v) in m.iter().enumerate() {
            mm[i % 4][i / 4] = *v;
        }
        let nm = names.get(*jn).copied().flatten().context("a skin joint without a name")?;
        b2.insert(nm.into(), f32m(&np::inv4(&mm).context("a singular inverse bind matrix")?));
    }
    let head = *b2.get("head").context("no head joint")?;
    b2.insert("head_b".into(), head);

    let mut g = Glb::new();
    // mesh: the first primitive (the body), joints remapped to the Tekken 3 order
    let prim = &j["meshes"][0]["primitives"][0];
    let at = &prim["attributes"];
    let to3 = |r: &Vec<f64>| [r[0] as f32, r[1] as f32, r[2] as f32];
    let mut pos: Vec<[f32; 3]> = src.acc(&at["POSITION"])?.iter().map(to3).collect();
    let mut nrm: Vec<[f32; 3]> = src.acc(&at["NORMAL"])?.iter().map(to3).collect();
    let mut uv: Vec<[f32; 2]> = src.acc(&at["TEXCOORD_0"])?.iter().map(|r| [r[0] as f32, r[1] as f32]).collect();
    let mut remap = HashMap::new();
    for (i, jn) in joints2.iter().enumerate() {
        let nm = names.get(*jn).copied().flatten().context("a skin joint without a name")?;
        remap.insert(
            i,
            order.iter().position(|o| o == nm).with_context(|| format!("joint {nm} has no Tekken 3 place"))?,
        );
    }
    let mut jt3: Vec<[u8; 4]> = src
        .acc(&at["JOINTS_0"])?
        .iter()
        .map(|r| -> Result<[u8; 4]> {
            let mut o = [0u8; 4];
            for c in 0..4 {
                o[c] = *remap.get(&(r[c] as usize)).context("a vertex on a joint outside the skin")? as u8;
            }
            Ok(o)
        })
        .collect::<Result<_>>()?;
    let mut idx: Vec<u32> = src.acc(&prim["indices"])?.iter().map(|r| r[0] as u32).collect();
    let mut weights: Vec<[f32; 4]> = src.acc(&at["WEIGHTS_0"])?.iter().map(|r| f32s(&vec4(r))).collect();
    let mut attrs = attributes(&mut g, &pos, &nrm, &uv, &jt3, &weights);
    let image = g.view(src.image(0)?, None);
    g.j.insert("images".into(), json!([{"bufferView": image, "mimeType": "image/png", "name": "atlas"}]));
    g.j.insert("samplers".into(), j["samplers"].clone());
    g.j.insert("textures".into(), j["textures"].clone());
    g.j.insert("materials".into(), json!([j["materials"][0].clone()]));
    // Devil's / Angel's wings (export_characters: the 'wings' mesh, a morph target per wing frame): a joint per wing
    // vertex on the chest, its corners skinned to it; each clip moves the joints through the wing frames
    let wing_mesh = arr(&j["meshes"]).iter().find(|m| m["name"] == "wings");
    let mut wing_frames: Option<Vec<Vec<V3>>> = None; // [frame][joint]
    if let Some(wm) = wing_mesh {
        let wp = &wm["primitives"][0];
        let w0: Vec<V3> = src.acc(&wp["attributes"]["POSITION"])?.iter().map(|r| vec3(r)).collect();
        let mut frames: Vec<Vec<V3>> = Vec::new(); // [frame][corner]
        for t in arr(&wp["targets"]) {
            let d = src.acc(&t["POSITION"])?;
            frames.push(w0.iter().zip(&d).map(|(a, b)| [a[0] + b[0], a[1] + b[1], a[2] + b[2]]).collect());
        }
        let r5 = |x: f64| (x * 1e5).round_ties_even() / 1e5;
        let key: Vec<Vec<u64>> = (0..w0.len())
            .map(|c| frames.iter().flat_map(|f| f[c].map(|v| (r5(v) + 0.0).to_bits())).collect())
            .collect();
        let mut groups: Vec<&Vec<u64>> = Vec::new();
        for k in &key {
            if !groups.contains(&k) {
                groups.push(k);
            }
        }
        let corner_joint: Vec<usize> =
            key.iter().map(|k| order.len() + groups.iter().position(|g| *g == k).unwrap_or(0)).collect();
        let firsts: Vec<usize> = groups.iter().map(|g| key.iter().position(|k| k == *g).unwrap_or(0)).collect();
        wing_frames = Some(frames.iter().map(|f| firsts.iter().map(|&c| f[c]).collect()).collect());
        for k in 0..groups.len() {
            order.push(format!("wing{k}"));
            parent_of.insert(format!("wing{k}"), "chest".into());
        }
        let n0 = pos.len() as u32;
        idx.extend(src.acc(&wp["indices"])?.iter().map(|r| r[0] as u32 + n0));
        pos.extend(w0.iter().map(f32s));
        nrm.extend(w0.iter().map(|_| [0.0f32, 1.0, 0.0]));
        uv.extend(src.acc(&wp["attributes"]["TEXCOORD_0"])?.iter().map(|r| [r[0] as f32, r[1] as f32]));
        jt3.extend(corner_joint.iter().map(|&c| [c as u8, 0, 0, 0]));
        weights.extend(w0.iter().map(|_| [1.0f32, 0.0, 0.0, 0.0]));
        attrs = attributes(&mut g, &pos, &nrm, &uv, &jt3, &weights);
    }
    if order.len() > 64 {
        bail!("{} joints: the port skins up to 64", order.len());
    }
    let mesh_idx = g.acc_u32(&idx, Some(34963));
    g.j.insert(
        "meshes".into(),
        json!([{"name": name, "primitives": [{"attributes": attrs, "indices": mesh_idx, "material": 0}]}]),
    );

    // the wing joints' bind: at the wing vertex (frame 0), turned as the chest
    let mut bc = *b2.get("chest").context("no chest joint")?;
    let r = mm3(&np::rot_of(&bc), &d_of("chest"));
    for i in 0..3 {
        for c in 0..3 {
            bc[i][c] = r[i][c] as f32 as f64;
        }
    }
    let mut wing_local: Option<Vec<Vec<V3>>> = None;
    if let Some(wf) = &wing_frames {
        let inv = f32m(&np::inv4(&bc).context("a singular chest matrix")?);
        let ir = t3(&np::rot_of(&inv));
        let it = np::trans_of(&inv);
        // chest-local, [frame][joint]
        wing_local = Some(
            wf.iter()
                .map(|f| {
                    f.iter()
                        .map(|p| {
                            let v = row_times(p, &ir);
                            [v[0] + it[0], v[1] + it[1], v[2] + it[2]]
                        })
                        .collect()
                })
                .collect(),
        );
        if let Some(f0) = wf.first() {
            for (k, p) in f0.iter().enumerate() {
                let mut m = bc;
                for i in 0..3 {
                    m[i][3] = p[i] as f32 as f64;
                }
                b2.insert(format!("wing{k}"), m);
            }
        }
    }

    // nodes: Tekken 3's order and rest transforms (then the chains' and the wings' joints)
    let mut nodes = Vec::new();
    let (mut rest_t, mut rest_q) = (Vec::new(), Vec::new());
    for n in &order {
        let p = parent_of.get(n).map(String::as_str);
        let (t, r): (V3, M3) = if n == "head_b" {
            ([0.0; 3], I3)
        } else if let Some(k) = n.strip_prefix("wing") {
            let k: usize = k.parse().unwrap_or(0);
            (wing_local.as_ref().and_then(|w| w.first()).and_then(|f| f.get(k)).copied().unwrap_or([0.0; 3]), I3)
        } else {
            let t2n = &nodes2[k2(n).with_context(|| format!("the Tekken 2 glb has no {n}"))?];
            let tr = json_f(&t2n["translation"]);
            let rot = json_f(&t2n["rotation"]);
            let t = match p {
                Some(p) => mv3(&t3(&dn(p)), &if tr.is_empty() { [0.0; 3] } else { vec3(&tr) }),
                None => [0.0; 3],
            };
            let q = if rot.is_empty() { [0.0, 0.0, 0.0, 1.0] } else { vec4(&rot) };
            let dp = p.map_or(I3, |p| t3(&dn(p)));
            (t, mm3(&mm3(&dp, &qmat(&q)), &dn(n)))
        };
        let q = quat(&r);
        let mut nd = json!({"name": n, "translation": floats(&t), "rotation": floats(&q)});
        let kids: Vec<usize> =
            order.iter().enumerate().filter(|(_, c)| parent_of.get(*c) == Some(n)).map(|(i, _)| i).collect();
        if !kids.is_empty() {
            nd["children"] = json!(kids);
        }
        nodes.push(nd);
        rest_t.push(t);
        rest_q.push(q);
    }
    let mut ibm: Vec<[f32; 16]> = Vec::new();
    for n in &order {
        let mut m = *b2.get(n).with_context(|| format!("no bind matrix for {n}"))?;
        let r = mm3(&np::rot_of(&m), &dn(n));
        for i in 0..3 {
            for c in 0..3 {
                m[i][c] = r[i][c] as f32 as f64;
            }
        }
        let inv = f32m(&np::inv4(&m).context("a singular bind matrix")?);
        let mut o = [0f32; 16];
        for c in 0..4 {
            for rr in 0..4 {
                o[c * 4 + rr] = inv[rr][c] as f32;
            }
        }
        ibm.push(o);
    }
    let ibm_acc = g.acc_f32(&ibm, None, false);
    nodes.push(json!({"name": "mesh", "mesh": 0, "skin": 0}));
    g.j.insert("nodes".into(), json!(nodes));
    g.j.insert(
        "skins".into(),
        json!([{"joints": (0..order.len()).collect::<Vec<_>>(), "skeleton": 0, "inverseBindMatrices": ibm_acc}]),
    );
    g.j.insert("scenes".into(), json!([{"name": name, "nodes": [0, order.len()]}]));
    g.j.insert("scene".into(), json!(0));
    // rest_t / rest_q as written (the JSON floats)
    let parent_i: Vec<i64> = order
        .iter()
        .map(|n| parent_of.get(n).and_then(|p| order.iter().position(|o| o == p)).map_or(-1, |i| i as i64))
        .collect();
    let mut chain_links: BTreeMap<i64, Vec<usize>> = BTreeMap::new();
    for n in &chains {
        let nm = n["name"].as_str().unwrap_or("");
        chain_links
            .entry(int(&n["extras"]["chain"]))
            .or_default()
            .push(order.iter().position(|o| o == nm).unwrap_or(0));
    }

    let ctx = Anim {
        order: &order,
        rest_t: &rest_t,
        rest_q: &rest_q,
        parent_i: &parent_i,
        chain_joints: chain_links.values().flatten().copied().collect(),
        chain_data,
        wing_local: wing_local.as_deref(),
        wing_clips: wing_mesh
            .map(|w| arr(&w["extras"]["clips"]).iter().map(|c| arr(c).iter().map(int).collect()).collect())
            .unwrap_or_default(),
        wing_info,
    };
    // animations: every rotation re-expressed in Tekken 3's axes
    let mut anims = Vec::new();
    let mut times_of = HashMap::new();
    for a in arr(&j["animations"]) {
        let mut rots = BTreeMap::new();
        let mut root = None;
        for ch in arr(&a["channels"]) {
            let out = src.acc(&a["samplers"][int(&ch["sampler"]) as usize]["output"])?;
            let n = names.get(int(&ch["target"]["node"]) as usize).copied().flatten().unwrap_or("");
            match ch["target"]["path"].as_str() {
                Some("rotation") if T3_ORDER.contains(&n) => {
                    rots.insert(n.to_string(), out.iter().map(|r| vec4(r)).collect::<Vec<_>>());
                }
                Some("translation") if n == "root" => root = Some(out.iter().map(|r| vec3(r)).collect::<Vec<_>>()),
                _ => {}
            }
        }
        let nm = a["name"].as_str().unwrap_or("");
        let nm = match nm.strip_prefix("motion_") {
            Some(rest) => format!("{prefix}{rest}"),
            None => nm.to_string(),
        };
        let root = root.context("a Tekken 2 clip without the root's translation")?;
        anims.push(ctx.add(&mut g, &mut times_of, &nm, &from_t2(&rots), &root)?);
    }
    for (clip, bone_rots, root) in extra {
        anims.push(ctx.add(&mut g, &mut times_of, clip, &from_t2(bone_rots), root)?);
    }
    if let Some(donor) = donor {
        let d = GlbIn::read(fs, donor)?;
        let dnames: Vec<&str> = arr(&d.j["nodes"]).iter().map(|n| n["name"].as_str().unwrap_or("")).collect();
        for a in arr(&d.j["animations"]) {
            let (mut rots, mut root) = (BTreeMap::new(), None);
            for ch in arr(&a["channels"]) {
                let n = dnames.get(int(&ch["target"]["node"]) as usize).copied().unwrap_or("");
                let path = ch["target"]["path"].as_str();
                if path == Some("rotation") && T3_ORDER.contains(&n) {
                    let out = d.acc(&a["samplers"][int(&ch["sampler"]) as usize]["output"])?;
                    rots.insert(n.to_string(), out.iter().map(|r| vec4(r)).collect::<Vec<_>>());
                } else if path == Some("translation") && n == "root" {
                    let out = d.acc(&a["samplers"][int(&ch["sampler"]) as usize]["output"])?;
                    root = Some(out.iter().map(|r| vec3(r)).collect::<Vec<_>>());
                }
            }
            let root = root.unwrap_or_else(|| vec![[0.0; 3]; rots.values().map(Vec::len).max().unwrap_or(0)]);
            anims.push(ctx.add(&mut g, &mut times_of, a["name"].as_str().unwrap_or(""), &rots, &root)?);
        }
    }
    let n = anims.len();
    g.j.insert("animations".into(), json!(anims));
    g.j["asset"]["generator"] = json!("t2_extractor/transplant.py");
    g.write(fs, out_path)?;
    Ok(n)
}

/// A float32 matrix (numpy's single-precision inverse of a float32 matrix: exact for the rigid ones here).
fn f32m(m: &M4) -> M4 {
    m.map(|r| r.map(|v| v as f32 as f64))
}

/// `p @ m` of a row vector (the rows of `points @ inv[:3, :3].T`).
fn row_times(p: &V3, m: &M3) -> V3 {
    [0, 1, 2].map(|c| 0.0 + p[0] * m[0][c] + p[1] * m[1][c] + p[2] * m[2][c])
}

fn attributes(
    g: &mut Glb,
    pos: &[[f32; 3]],
    nrm: &[[f32; 3]],
    uv: &[[f32; 2]],
    jt: &[[u8; 4]],
    w: &[[f32; 4]],
) -> Value {
    json!({"POSITION": g.acc_f32(pos, Some(34962), true), "NORMAL": g.acc_f32(nrm, Some(34962), false),
           "TEXCOORD_0": g.acc_f32(uv, Some(34962), false), "JOINTS_0": g.acc_u8x4(jt, Some(34962)),
           "WEIGHTS_0": g.acc_f32(w, Some(34962), false)})
}

fn neg(q: [f64; 4]) -> [f64; 4] {
    q.map(|v| -v)
}

/// Tekken 2 bone rotations (Tekken 2 axes) as Tekken 3's.
fn from_t2(tracks: &BTreeMap<String, Vec<[f64; 4]>>) -> BTreeMap<String, Vec<[f64; 4]>> {
    let mut rots = BTreeMap::new();
    for n in T3_ORDER {
        let Some(tr) = tracks.get(n).filter(|_| n != "head_b") else {
            continue;
        };
        let dp = t3_parent(n).map_or(I3, d_of);
        let mut qs: Vec<[f64; 4]> = Vec::new();
        for q in tr {
            let mut q3 = quat(&mm3(&mm3(&t3(&dp), &qmat(q)), &d_of(n)));
            if qs.last().is_some_and(|l| np::dot4(&q3, l) < 0.0) {
                q3 = neg(q3);
            }
            qs.push(q3);
        }
        rots.insert(n.to_string(), qs);
    }
    rots
}

/// What making a clip needs: the joints and their rest, the chains and the wings.
struct Anim<'a> {
    order: &'a [String],
    rest_t: &'a [V3],
    rest_q: &'a [[f64; 4]],
    parent_i: &'a [i64],
    chain_joints: Vec<usize>,
    chain_data: Option<(&'a [u8], &'a [u8], u32)>,
    wing_local: Option<&'a [Vec<V3>]>,
    wing_clips: Vec<Vec<i64>>,
    wing_info: Option<&'a WingInfo>,
}

impl Anim<'_> {
    /// rots: {Tekken 3 bone name: quaternions in Tekken 3 axes}; root: glTF translations. The chains' and the wings'
    /// tracks are made here.
    fn add(
        &self,
        g: &mut Glb,
        times_of: &mut HashMap<usize, usize>,
        clip: &str,
        rots: &BTreeMap<String, Vec<[f64; 4]>>,
        root: &[V3],
    ) -> Result<Value> {
        let n = root.len();
        let t = t2::times(g, times_of, n);
        let (mut samplers, mut channels) = (Vec::new(), Vec::new());
        let (chain_q, chain_t) = self.hanging(rots, root, n)?;
        for (ji, ts) in &chain_t {
            let v: Vec<[f32; 3]> = ts.iter().map(f32s).collect();
            samplers.push(json!({"input": t, "output": g.acc_f32(&v, None, false), "interpolation": "LINEAR"}));
            channels.push(json!({"sampler": samplers.len() - 1, "target": {"node": ji, "path": "translation"}}));
        }
        if let Some(wl) = self.wing_local {
            let frames: Vec<&Vec<V3>> = self
                .wing_track(clip, root, n)?
                .into_iter()
                .map(|w| &wl[w.rem_euclid(wl.len() as i64) as usize])
                .collect();
            let joints = frames.first().map_or(0, |f| f.len());
            for k in 0..joints {
                let v: Vec<[f32; 3]> = frames.iter().map(|f| f32s(&f[k])).collect();
                let ni = self.order.iter().position(|o| *o == format!("wing{k}")).unwrap_or(0);
                samplers.push(json!({"input": t, "output": g.acc_f32(&v, None, false), "interpolation": "LINEAR"}));
                channels.push(json!({"sampler": samplers.len() - 1, "target": {"node": ni, "path": "translation"}}));
            }
        }
        for (ni, nm) in self.order.iter().enumerate() {
            let Some(qs) = rots.get(nm).or_else(|| chain_q.get(&ni)) else {
                continue;
            };
            let q: Vec<[i16; 4]> = qs.iter().map(q16).collect();
            samplers.push(json!({"input": t, "output": g.acc_q16(&q), "interpolation": "LINEAR"}));
            channels.push(json!({"sampler": samplers.len() - 1, "target": {"node": ni, "path": "rotation"}}));
        }
        let v: Vec<[f32; 3]> = root.iter().map(f32s).collect();
        samplers.push(json!({"input": t, "output": g.acc_f32(&v, None, false), "interpolation": "LINEAR"}));
        channels.push(json!({"sampler": samplers.len() - 1, "target": {"node": 0, "path": "translation"}}));
        Ok(json!({"name": clip, "samplers": samplers, "channels": channels}))
    }

    /// The wing frame each frame of a clip, as 0x8006F9E8 steps them: the wing mode each frame (0x80065250: 2 fly
    /// while in the move's air window (move +0x13..+0x14, fighter +0x8C) and rising (+0x34 > 0), 1 spread in it
    /// falling, 3 fold in the block reactions (hit, +0x5C, while blocking, +0x5F), 0 otherwise), then by mode and
    /// current wing clip: switch to a clip now, or play the current one out and go on with another (the tables
    /// 0x8004EB98 / B8 / D8). Mode 0's clip 0 follows the stance motion's frame in the stance, else holds its first
    /// frame. A clip starts in wing clip 0.
    fn wing_track(&self, clip: &str, root: &[V3], n: usize) -> Result<Vec<i64>> {
        let empty = WingInfo::default();
        let info = self.wing_info.unwrap_or(&empty);
        let window = info.air.get(&Some(clip.to_string()));
        let stance = info.stance.as_deref() == Some(clip);
        let guarding = info.guard.contains(clip);
        let clips = &self.wing_clips;
        let (mut cur, mut counter, mut out) = (0usize, -1i64, Vec::new());
        for f in 0..n {
            let frame_no = f as i64 + 1; // fighter +0x44
            let mut mode = if guarding { 3 } else { 0 };
            if let Some(&(a, b)) = window
                && (a as i64..=b as i64).contains(&frame_no)
            {
                let rising = f + 1 < n && root[f + 1][1] > root[f][1];
                mode = if rising { 2 } else { 1 };
            }
            let (mut nxt, mut a2) = (0, counter + 1);
            match mode {
                0 => {
                    if cur == 0 {
                        a2 = if stance { f as i64 } else { 0 };
                    } else if matches!(cur, 1 | 3 | 4 | 5) {
                        (cur, a2) = (2, 0);
                    } else if cur == 6 {
                        (cur, a2) = (0, 0);
                    }
                }
                1 => {
                    if matches!(cur, 0 | 6 | 7) {
                        (cur, a2) = (1, 0);
                    } else if cur == 2 {
                        (cur, a2) = (3, 0);
                    } else {
                        nxt = 3;
                    }
                }
                2 => {
                    if matches!(cur, 0 | 6 | 7) {
                        (cur, a2) = (4, 0);
                    } else if cur == 2 {
                        (cur, a2) = (5, 0);
                    } else {
                        nxt = 5;
                    }
                }
                _ => {
                    // fold: into clip 6, held folded (clip 7 at its first frame) once there
                    if cur < 6 {
                        (cur, a2) = (6, 0);
                    } else if cur == 7 {
                        (a2, nxt) = (0, 7);
                    } else {
                        nxt = 7;
                    }
                }
            }
            let mut seq = clips.get(cur).context("no such wing clip")?;
            if a2 >= seq.len() as i64 {
                (cur, a2) = (nxt, 0);
                seq = clips.get(cur).context("no such wing clip")?;
            }
            counter = a2;
            out.push(*seq.get(a2 as usize).context("an empty wing clip")?);
        }
        Ok(out)
    }

    /// The chains through a clip, swung by Tekken 2's own routine (chain_sim: 0x800811C4 once a frame): the bones'
    /// world matrices in Tekken 2 space each frame, the links' world matrices back, as the chain joints' local
    /// rotations and translations. A clip starts with the links laid out and left to hang 60 frames on its first pose.
    #[allow(clippy::type_complexity)]
    fn hanging(
        &self,
        rots: &BTreeMap<String, Vec<[f64; 4]>>,
        root: &[V3],
        n: usize,
    ) -> Result<(HashMap<usize, Vec<[f64; 4]>>, Vec<(usize, Vec<V3>)>)> {
        let Some((exe, part2, character)) = self.chain_data.filter(|_| !self.chain_joints.is_empty()) else {
            return Ok((HashMap::new(), vec![]));
        };
        let order = self.order;
        let mut world: Vec<Vec<M4>> = Vec::with_capacity(n);
        for f in 0..n {
            let mut wf: Vec<M4> = Vec::with_capacity(order.len());
            for (i, nm) in order.iter().enumerate() {
                let q = match rots.get(nm) {
                    Some(qs) if !qs.is_empty() => qs[f.min(qs.len() - 1)],
                    _ => self.rest_q[i],
                };
                let l = np::rt4(&qmat(&q), &if i == 0 { root[f] } else { self.rest_t[i] });
                let p = self.parent_i[i];
                wf.push(if p >= 0 { mm4(wf.get(p as usize).unwrap_or(&[[0.0; 4]; 4]), &l) } else { l });
            }
            world.push(wf);
        }
        let cm = C;
        let bones = |f: usize| -> Bones {
            BONE_NAMES
                .iter()
                .enumerate()
                .filter_map(|(k, nm)| {
                    let w = &world[f][order.iter().position(|o| o == nm)?];
                    let r = mm3(&mm3(&mm3(&cm, &np::rot_of(w)), &t3(&d_of(nm))), &cm);
                    let t = mv3(&cm, &np::trans_of(w)).map(|v| v / SCALE);
                    Some((k, r, t))
                })
                .collect()
        };
        let mut sim = ChainSim::new(exe, part2, character);
        let b0 = bones(0);
        let place = b0.iter().find(|b| b.0 == 0).map_or((0.0, 0.0), |b| (b.2[0], b.2[2]));
        sim.start(&b0, place)?;
        for _ in 0..60 {
            sim.step(&b0)?;
        }
        let joints = &self.chain_joints;
        let mut q_out: HashMap<usize, Vec<[f64; 4]>> = joints.iter().map(|&j| (j, Vec::new())).collect();
        let mut t_out: Vec<(usize, Vec<V3>)> = joints.iter().map(|&j| (j, Vec::new())).collect();
        let mut prev_world = np::eye4();
        for f in 0..n {
            let mats = sim.step(&if f != 0 { bones(f) } else { b0.clone() })?;
            for (k, &ji) in joints.iter().enumerate() {
                let Some((r, t)) = mats.get(k) else { break };
                let m = mm3(&mm3(&cm, &r.map(|row| row.map(|v| v / 4096.0))), &cm);
                let wl = np::rt4(&polar(&m), &mv3(&cm, t).map(|v| SCALE * v));
                // the parent's world: the bone's (from the pose) or the link before (its world just made)
                let pi = self.parent_i[ji];
                let parent_is_link = pi >= 0 && joints.contains(&(pi as usize));
                let p = if k == 0 || !parent_is_link {
                    world[f].get(pi as usize).copied().unwrap_or_else(np::eye4)
                } else {
                    prev_world
                };
                let l = mm4(&np::inv4(&p).context("a singular bone matrix")?, &wl);
                let qs = q_out.get_mut(&ji).context("a chain joint")?;
                let mut q = quat(&np::rot_of(&l));
                if qs.last().is_some_and(|last| np::dot4(&q, last) < 0.0) {
                    q = neg(q);
                }
                qs.push(q);
                if let Some(tt) = t_out.iter_mut().find(|x| x.0 == ji) {
                    tt.1.push(np::trans_of(&l));
                }
                prev_world = wl;
            }
        }
        Ok((q_out, t_out))
    }
}

/// U Vt of the SVD (the nearest rotation).
fn polar(m: &M3) -> M3 {
    let (u, _, vt) = np::svd(m);
    mm3(&u, &vt)
}

// the fighters brought over: every Tekken 2 character whose name no Tekken 3 character has (Marshall Law, King and
// Kuma have their Tekken 3 namesakes). Each takes its system moves (movement, guard, throws' setup, reactions) from a
// Tekken 3 donor of a like style:
fn donor_name(n: &str) -> Option<&'static str> {
    Some(match n {
        "KAZUYA" | "DEVIL" | "ANGEL" => "Jin",
        "JUN" | "MICHELLE" => "Julia",
        "JACK-2" | "P.JACK" => "Gun Jack",
        "WANG" => "Lei",
        "LEE" => "Law",
        "ARMOR KING" | "GANRYU" => "King",
        "KUNIMITSU" => "Yoshimitsu",
        "BRUCE" => "Bryan",
        "BAEK" => "Hwoarang",
        "ROGER" | "ALEX" => "Kuma",
        _ => return None,
    })
}
// port character ids: Kazuya and Jun came first (23, 24); the others missing from Tekken 3 follow in Tekken 2's
// character order (25..36); then the Tekken 2 versions of Tekken 3's own fighters, in the same order (37..)
fn first_id(t2id: i64) -> Option<i64> {
    match t2id {
        0 => Some(23),
        19 => Some(24),
        _ => None,
    }
}
// port stages of the home arenas: Kazuya's and Jun's came first (30, 31), the others 60 + the Tekken 2 stage
fn home_stage(stage: u8) -> i64 {
    match stage {
        21 => 30,
        2 => 31,
        s => 60 + s as i64,
    }
}

pub struct Fighter {
    t2id: i64,
    name: String,
    cid: i64,
    moveset_id: i64,
    blocks: Vec<usize>,
    t2ids: Vec<i64>,
    costume_names: Vec<String>,
    donor: (i64, String),
    home: i64,
    t2stage: u8,
    order: u8,
    select_row: i64,
    grid_slot: [i64; 2],
    portraits: Vec<i64>,
}

/// A port stage made from a Tekken 2 arena: (Tekken 2 stage, its block, name, home character ids, Tekken 2 home
/// names).
type Stage = (usize, usize, String, Vec<i64>, Vec<String>);

/// The fighters to bring over (the donor: model record and move set file; home: the port stage; select row and VS
/// grid slot on the Tekken 2 page), and the stages: port id -> Stage.
fn plan(disc: &Disc, assets: &dyn Vfs) -> Result<(Vec<Fighter>, BTreeMap<i64, Stage>)> {
    let chars = disc.characters()?;
    let names: HashMap<i64, String> = chars.iter().map(|c| (c.id, c.name.clone())).collect();
    let roster = load_json(assets, "gamedata/roster.json")?;
    let t3chars: Vec<&Value> = arr(&roster["characters"]).iter().filter(|c| int(&c["id"]) <= 22).collect();
    let t3: BTreeSet<String> = t3chars.iter().map(|c| c["name"].as_str().unwrap_or("").to_uppercase()).collect();
    let mut sel_entries: Vec<&Value> = arr(&roster["selections"]["entries"]).iter().collect();
    sel_entries.sort_by_key(|e| -int(&e["selection"]));
    let mut sel: HashMap<i64, &Value> = HashMap::new();
    for e in sel_entries {
        sel.insert(int(&e["character"]), e);
    }
    let by_name: HashMap<String, i64> =
        t3chars.iter().map(|c| (c["name"].as_str().unwrap_or("").to_string(), int(&c["id"]))).collect();
    // each character's costume blocks; a second slot that is another character (Devil's Angel, Roger's Alex: the
    // same fighter with an ending of its own) is the first one's second costume
    let mut blocks: HashMap<String, Vec<usize>> = HashMap::new();
    let mut block_t2: HashMap<usize, (i64, String)> = HashMap::new();
    let mut ids: HashMap<String, i64> = HashMap::new();
    for c in &chars {
        ids.insert(c.name.to_uppercase(), c.id);
    }
    let head = |s: &str| s.split(" (").next().unwrap_or("").to_uppercase();
    for b in 0..t2::BLOCKS {
        let n = t2::block_name(b, &names);
        let own = head(&n);
        // (a third costume, Kazuya's, is the first's; an odd block another character's second costume)
        let owner =
            if n.contains("costume 3") || b & 1 == 0 { own.clone() } else { head(&t2::block_name(b - 1, &names)) };
        blocks.entry(owner).or_default().push(b);
        block_t2.insert(b, (*ids.get(&own).with_context(|| format!("no Tekken 2 character {own}"))?, own));
    }
    // every Tekken 2 fighter: those missing from Tekken 3 first (their ids as before), then the Tekken 2 versions of
    // Tekken 3's own (a donor of their own name)
    let mut new: Vec<_> = chars.iter().filter(|c| !t3.contains(&c.name) && blocks.contains_key(&c.name)).collect();
    new.sort_by_key(|c| c.id);
    let mut also: Vec<_> = chars.iter().filter(|c| t3.contains(&c.name) && blocks.contains_key(&c.name)).collect();
    also.sort_by_key(|c| c.id);
    new.extend(also);
    let upper: HashMap<String, &String> = by_name.keys().map(|k| (k.to_uppercase(), k)).collect();
    let (mut fighters, mut cid) = (Vec::new(), 25);
    for c in new {
        let fid = match first_id(c.id) {
            Some(f) => f,
            None => {
                cid += 1;
                cid - 1
            }
        };
        let dn = match donor_name(&c.name) {
            Some(d) => d.to_string(),
            None => upper.get(&c.name).map(|s| s.to_string()).with_context(|| format!("no donor for {}", c.name))?,
        };
        let d = sel
            .get(by_name.get(&dn).with_context(|| format!("no Tekken 3 {dn}"))?)
            .with_context(|| format!("{dn} has no selection"))?;
        let bl = blocks[&c.name].clone();
        let mf = d["moveset_file"].as_str().unwrap_or("");
        fighters.push(Fighter {
            t2id: c.id,
            name: c.name.clone(),
            cid: fid,
            moveset_id: fid + 7,
            t2ids: bl.iter().map(|b| block_t2[b].0).collect(),
            costume_names: bl.iter().map(|b| block_t2[b].1.clone()).collect(),
            blocks: bl,
            donor: (int(&d["model_record"]), mf.rsplit('/').next().unwrap_or(mf).to_string()),
            home: home_stage(c.stage),
            t2stage: c.stage,
            order: c.order,
            select_row: 0,
            grid_slot: [0, 0],
            portraits: vec![],
        });
    }
    // the Tekken 2 page of the select screens, in Tekken 2's select order: 8 a row on the character select, 7 a row
    // on the VS grid
    let mut by_order: Vec<usize> = (0..fighters.len()).collect();
    by_order.sort_by_key(|&i| (fighters[i].order, fighters[i].t2id));
    for (k, &i) in by_order.iter().enumerate() {
        let k = k as i64;
        fighters[i].select_row = 2 + k / 8;
        fighters[i].grid_slot = [k / 7, k % 7];
    }
    // portraits: the character's id for its first costume; a costume that is another Tekken 2 character (Angel,
    // Alex) its own, numbered after the last character
    let mut extra = fighters.iter().map(|f| f.cid).max().unwrap_or(0) + 1;
    for f in &mut fighters {
        for (k, &t) in f.t2ids.iter().enumerate() {
            if k != 0 && t != f.t2ids[0] {
                f.portraits.push(extra);
                extra += 1;
            } else {
                f.portraits.push(f.cid);
            }
        }
    }
    let stage_block: HashMap<usize, (usize, String)> =
        crate::export_stages::stages(disc)?.into_iter().map(|(s, b, n)| (s, (b, n))).collect();
    let mut stages: BTreeMap<i64, Stage> = BTreeMap::new();
    for f in &fighters {
        let (b, n) = stage_block.get(&(f.t2stage as usize)).context("a home stage that is not exported")?;
        let homes: Vec<String> =
            n.split('(').nth(1).unwrap_or("").trim_end_matches(')').split(", ").map(String::from).collect();
        let e = stages
            .entry(f.home)
            .or_insert_with(|| (f.t2stage as usize, *b, format!("{}, Tekken 2)", &n[..n.len() - 1]), vec![], homes));
        e.3.push(f.cid);
    }
    Ok((fighters, stages))
}

/// A Tekken 2 arena as the port's stage_NN.glb (export_stages::convert in its port layout), listed in stages.json.
fn transplant_stage(disc: &Disc, assets: &dyn Vfs, port_id: i64, stage: &Stage) -> Result<()> {
    let (t2stage, block, name, homes, home_names) = stage;
    crate::export_stages::convert(
        disc,
        *t2stage,
        *block,
        assets,
        &format!("stages/stage_{port_id:02}.glb"),
        name,
        true,
    )?;
    // the fighters' shadow: the stage's own (its lighting, 0x800A1030): flattened along x -= y s0, z -= y s1 in the
    // game's axes (y down), i.e. towards the light (-s0, 1, s1) in the port's (y up, z flipped); its colour
    let light = crate::export_stages::lighting(disc, *t2stage)?;
    let s = |k: usize| light["shadow"][k].as_f64().unwrap_or(0.0);
    let shadow = json!({"toward": [-s(0), 1.0, s(1)], "colour": light["shadow_rgb"],
                        "source": "Tekken 2 stage lighting 0x800A1030 (shadow angles, shadow colour)"});
    let sp = "gamedata/stages.json";
    let mut stages = load_json(assets, sp)?;
    let l = list(&mut stages["stages"])?;
    l.retain(|x| x.get("id") != Some(&json!(port_id)));
    l.push(json!({"id": port_id, "name": name, "glb": format!("stage_{port_id:02}.glb"),
                  "source": format!("Tekken 2 stage {t2stage}, block {block}"), "home_characters": homes, "home_names": home_names,
                  "shadow": shadow}));
    write_json(assets, sp, &stages, pyjson::INDENT1)
}

/// world(clip, frame, joint) -> glTF position, from a glb's rest nodes and clips.
fn posed(fs: &dyn Vfs, path: &str) -> Result<impl FnMut(&str, i64, &str) -> Result<[f64; 3]>> {
    let g = GlbIn::read(fs, path)?;
    let nodes: Vec<Value> = arr(&g.j["nodes"]).to_vec();
    let names: Vec<Option<String>> = nodes.iter().map(|n| n["name"].as_str().map(String::from)).collect();
    let mut par: HashMap<usize, usize> = HashMap::new();
    for (i, n) in nodes.iter().enumerate() {
        for c in arr(&n["children"]) {
            par.insert(int(c) as usize, i);
        }
    }
    let anims: HashMap<String, Value> =
        arr(&g.j["animations"]).iter().map(|a| (a["name"].as_str().unwrap_or("").to_string(), a.clone())).collect();
    let mut cache: HashMap<String, HashMap<(usize, String), Vec<Vec<f64>>>> = HashMap::new();
    Ok(move |clip: &str, frame: i64, joint: &str| -> Result<[f64; 3]> {
        if !cache.contains_key(clip) {
            let a = anims.get(clip).with_context(|| format!("no clip {clip}"))?;
            let mut tr = HashMap::new();
            for ch in arr(&a["channels"]) {
                let out = g.acc(&a["samplers"][int(&ch["sampler"]) as usize]["output"])?;
                tr.insert(
                    (int(&ch["target"]["node"]) as usize, ch["target"]["path"].as_str().unwrap_or("").to_string()),
                    out,
                );
            }
            cache.insert(clip.to_string(), tr);
        }
        let tr = &cache[clip];
        let mut k = names.iter().position(|n| n.as_deref() == Some(joint));
        let mut chain = Vec::new();
        while let Some(i) = k {
            chain.push(i);
            k = par.get(&i).copied();
        }
        let mut m = np::eye4();
        let at = |v: &Vec<Vec<f64>>| v[(frame.max(0) as usize).min(v.len().saturating_sub(1))].clone();
        for &i in chain.iter().rev() {
            let n = &nodes[i];
            let q = tr.get(&(i, "rotation".into())).filter(|v| !v.is_empty()).map(at).unwrap_or_else(|| {
                let r = json_f(&n["rotation"]);
                if r.is_empty() { vec![0.0, 0.0, 0.0, 1.0] } else { r }
            });
            let t = tr.get(&(i, "translation".into())).filter(|v| !v.is_empty()).map(at).unwrap_or_else(|| {
                let t = json_f(&n["translation"]);
                if t.is_empty() { vec![0.0; 3] } else { t }
            });
            m = mm4(&m, &np::rt4(&qmat(&vec4(&q)), &vec3(&t)));
        }
        Ok(np::trans_of(&m))
    })
}

const T2_VOICES: u32 = 0x800A1B00; // per Tekken 2 character: pointer to 4-byte {u16 key, u16 volume} voice entries
const VOICE_SLOT: u32 = 0x80050D48; // sound index -> entry (bytes)

/// Tekken 2 voice sound idx of a character (0x800744DC with bank 6): (program, tone, note, volume) of its own VAB.
fn t2_voice(disc: &Disc, t2id: i64, idx: u16) -> Result<(usize, usize, i64, u16)> {
    let p = disc.u32(T2_VOICES + 4 * t2id as u32)?;
    let a = p + 4 * disc.u8(VOICE_SLOT + idx as u32)? as u32;
    let (s0, s3) = (disc.u16(a)?, disc.u16(a + 2)?);
    Ok(((s0 >> 14) as usize, ((s0 >> 2) & 15) as usize, ((s0 >> 6) & 127) as i64, s3 & 127))
}

const SYSTEM_SOUNDS: u32 = 0x80050D64; // bank 2 (the system VAB A, records 0 / 1): sound index -> {u16 key, u16 volume}
const SYSTEM_COUNT: usize = 48; // its indices brought into the fighters' banks (the tables name 0x200..0x223)
const RANDOM_VOICES: u32 = 0x800A1A8C; // per Tekken 2 character: s16 count, u16 voice sound indices (the random attack voice)
const VOICE_LOCK: u32 = 0x80050CB8; // per Tekken 2 character: the frames a voice locks the next one out
const JACKS: [i64; 2] = [3, 11]; // Tekken 2's Jack-2 and P.Jack (actor +0x1A): struck, they ring as metal

/// The sample (VAG number) that SsUtKeyOn's program / tone plays in a pBAV.
fn vab_tone(header: &[u8], prog: usize, tone: usize) -> Result<i16> {
    i16_(header, 32 + 2048 + (prog * 16 + tone) * 32 + 22)
}

/// The rate (Hz) a pBAV program / tone's sample plays at for `note` (its centre note and fine tune: 44.1 kHz at the
/// centre).
fn vab_rate(header: &[u8], prog: usize, tone: usize, note: i64) -> Result<f64> {
    let o = 32 + 2048 + (prog * 16 + tone) * 32;
    let (centre, fine) = (u8_(header, o + 4)? as f64, u8_(header, o + 5)? as f64);
    Ok(44100.0 * libm::pow(2.0, (note as f64 - centre + fine / 128.0) / 12.0))
}

/// A copy of a PCM WAV whose header says `rate` (the samples as they are: the pitch a note plays them at).
fn wav_at_rate(fs: &dyn Vfs, src: &str, dst: &str, rate: f64) -> Result<()> {
    let mut d = fs.read(src)?;
    let align = u16_(&d, 32)? as u32;
    let r = rate.round_ties_even() as u32;
    d.get_mut(24..32).context("a short WAV")?.copy_from_slice(&[r.to_le_bytes(), (r * align).to_le_bytes()].concat());
    fs.write(dst, &d)
}

/// Where Tekken 2's bank-2 sound idx sits in a transplanted fighter's own bank: (program 1..3, tone).
fn system_slot(idx: usize) -> (usize, usize) {
    (1 + idx / 16, idx % 16)
}

/// The sfx_table entries of the Tekken 2 system sounds (own bank, programs 1..3): their first index, added once
/// (sound.json sfx_table.t2_system).
fn system_codes(assets: &dyn Vfs) -> Result<(Value, i64)> {
    let mut sound = load_json(assets, "gamedata/sound.json")?;
    let tbl = &mut sound["sfx_table"];
    if tbl.get("t2_system").is_none() {
        let start = arr(&tbl["codes"]).len();
        obj(tbl)?.insert(
            "t2_system".into(),
            json!({"start": start, "count": SYSTEM_COUNT,
                         "note": "Tekken 2 bank-2 sounds (0x80050D64) in the transplanted fighters' own banks"}),
        );
        list(&mut tbl["codes"])?.extend((0..SYSTEM_COUNT).map(|_| json!(0)));
    }
    let start = int(&sound["sfx_table"]["t2_system"]["start"]);
    Ok((sound, start))
}

/// A Tekken 2 fighter's voice row: its character id, a later costume (Angel, Alex, the second and third costumes) a
/// row of its own past the Tekken 2 ids (50 + 2 * (id - 23) + costume - 1), so each speaks with its own bank's
/// voices and random list.
fn voice_row(cid: i64, costume: usize) -> i64 {
    if costume == 0 { cid } else { 50 + 2 * (cid - 23) + costume as i64 - 1 }
}

/// The fighter's Tekken 2 voice bank as the port's character bank m<record> (audio/sfx, index.json) and its voice
/// row (sound.json, row = voice_row(): the character id, a later costume its own): category 0 the attack voices
/// (sound indices 4.., the ones the moves name), 1..3 the hit voices (0, 1, 2: 0x80073230 / 0x80073DB4). Codes as the
/// port's: bank 3, voice 6, volume 6, program 0, tone. The voices are laid out afresh in program 0 (the VAB's own
/// programs and keys are dropped, so programs 1..3 hold the system sounds): the attack voices tones 0.., hit voice 2
/// tone 12, hit voices 0 / 1 tones 14 / 15 (notes 74 / 75: the port's hurt sounds 0x98 / 0x99); each on the sample its
/// Tekken 2 program / tone plays (0x800744DC). Returns the voice index, the hit voices' index and the system sounds'
/// first sfx entry.
fn transplant_sound(
    disc: &Disc,
    assets: &dyn Vfs,
    block: usize,
    record: usize,
    cid: i64,
    t2id: i64,
    tmp: &str,
) -> Result<(BTreeMap<u16, i64>, BTreeMap<u16, i64>, i64)> {
    let key = format!("m{record:03}");
    let base = t2::BLOCK0 + 8 * block;
    let vab = disc.rec(base + 3)?;
    let sfx = "audio/sfx";
    let mut bank = export_bank(&format!("Tekken 2 {key}"), vab, disc.rec(base + 6)?, assets, sfx, &key)?;
    let mut samples: Vec<Value> = arr(&bank["samples"]).to_vec();
    // by VAG number (the file name's first two digits)
    let vag_of = |x: &Value| -> i64 {
        let f = x["file"].as_str().unwrap_or("");
        f.split('/').nth(1).and_then(|s| s.get(..2)).and_then(|s| s.parse().ok()).unwrap_or(-1)
    };
    for x in &mut samples {
        x["tones"] = json!([]);
    }
    let code = |tone: usize| 0xD980 | tone as i64;
    // Tekken 2 voice idx on program 0 tone `tone`: its code, or None (silent or no sample). Each voice plays at its
    // own note (0x800744DC): one whose note's rate is not the sample's gets its own copy at that rate.
    let place = |samples: &mut Vec<Value>, idx: u16, tone: usize| -> Result<Option<i64>> {
        let (prog, t, note, vol) = t2_voice(disc, t2id, idx)?;
        let smp = if vol != 0 {
            let vag = vab_tone(vab, prog, t)? as i64;
            samples.iter().rposition(|x| vag_of(x) == vag && x.get("source").is_none())
        } else {
            None
        };
        let Some(si) = smp else { return Ok(None) };
        let rate = vab_rate(vab, prog, t, note)?;
        let srate = samples[si]["rate"].as_f64().unwrap_or(1.0);
        let tones = json!([[0, 60 + tone, 60 + tone]]);
        if (rate / srate - 1.0).abs() > 0.01 {
            let file = format!("{key}/v{idx:02}.wav");
            wav_at_rate(assets, &join(sfx, samples[si]["file"].as_str().unwrap_or("")), &join(sfx, &file), rate)?;
            let secs = py_round(samples[si]["seconds"].as_f64().unwrap_or(0.0) * srate / rate, 2);
            samples.push(json!({"file": file, "rate": rate.round_ties_even() as i64, "seconds": secs, "tones": tones,
                                "source": format!("Tekken 2 voice {idx} at note {note}")}));
        } else if let Some(t) = samples[si]["tones"].as_array_mut() {
            t.extend(arr(&tones).iter().cloned());
        }
        Ok(Some(code(tone)))
    };
    let (mut attack, mut hit) = (Vec::new(), Vec::new());
    let (mut voice_index, mut hit_index) = (BTreeMap::new(), BTreeMap::new());
    for idx in 4..16 {
        if let Some(c) = place(&mut samples, idx, attack.len())? {
            voice_index.insert(idx, attack.len() as i64);
            attack.push(c);
        }
    }
    for (idx, tone) in [(0, 14), (1, 15), (2, 12)] {
        if let Some(c) = place(&mut samples, idx, tone)? {
            hit_index.insert(idx, hit.len() as i64);
            hit.push(c);
        }
    }
    // Tekken 2's system sounds (bank 2: throws' slams and falls, whooshes) join the bank at programs 1..3 (their
    // sfx_table codes: sound.json sfx_table.t2_system)
    let (mut sound, start) = system_codes(assets)?;
    // the SePlay volumes (Tekken 3's 0x800278FC, sound.json volumes)
    let volumes: Vec<i64> = arr(&sound["volumes"]["table"]).iter().map(int).collect();
    if volumes.len() != 16 {
        bail!("sound.json has no volume table: convert the Tekken 3 disc again first");
    }
    let hdr = disc.rec(0)?;
    let sysdir = join(tmp, "sys_bank");
    let sysbank = export_bank("Tekken 2 system A", hdr, disc.rec(1)?, assets, &sysdir, "sys")?;
    let files: HashMap<i64, &Value> = arr(&sysbank["samples"])
        .iter()
        .map(|x| (x["file"].as_str().and_then(|f| f.get(4..6)).and_then(|s| s.parse().ok()).unwrap_or(-1), x))
        .collect();
    for idx in 0..SYSTEM_COUNT {
        let a = SYSTEM_SOUNDS + 4 * idx as u32;
        let (s0, s3) = (disc.u16(a)?, disc.u16(a + 2)? as i64);
        let vag = if s0 != 0xFFFF { vab_tone(hdr, (s0 >> 14) as usize, ((s0 >> 2) & 15) as usize)? as i64 } else { 0 };
        let Some(x) = files.get(&vag) else { continue };
        let (prog, tone) = system_slot(idx);
        let file = format!("{key}/s{idx:02}.wav");
        assets.write(&join(sfx, &file), &assets.read(&join(&sysdir, x["file"].as_str().unwrap_or("")))?)?;
        samples.push(
            json!({"file": file, "rate": x["rate"], "seconds": x["seconds"], "tones": [[prog, 60 + tone, 60 + tone]],
                            "source": format!("Tekken 2 bank 2 sound {idx}")}),
        );
        let vol = (0..16).min_by_key(|&v| (volumes[v] - (s3 & 127)).abs()).unwrap_or(0) as i64;
        sound["sfx_table"]["codes"][(start + idx as i64) as usize] =
            json!(0xD800 | vol << 6 | (prog as i64) << 4 | tone as i64);
    }
    // the bank-2 sounds Tekken 2's sound player plays itself (0x80073294): a K.O. by an arm (descriptor byte below
    // 0xB) or a leg, and a block; each a lui / lhu of a table word (bank << 8 | sound)
    let sound_at = |lui: u32| -> Result<i64> {
        if disc.u32(lui)? >> 26 != 0x0F || disc.u32(lui + 4)? >> 26 != 0x25 {
            bail!("no lui / lhu at 0x{lui:08X}");
        }
        let at = ((disc.u16(lui)? as u32) << 16).wrapping_add(disc.i16(lui + 4)? as i32 as u32);
        Ok(i64::from(disc.u16(at)? & 0xFF))
    };
    sound["system_impacts"] = json!({"routine": "0x80073294 (0x800732D8, 0x800732E8, 0x800733EC)",
                                     "ko": [sound_at(0x800732D8)?, sound_at(0x800732E8)?], "blocked": sound_at(0x800733EC)?,
                                     "format": "bank-2 sound indices: a row's entry system_start + index"});
    write_json(assets, "gamedata/sound.json", &sound, pyjson::INDENT1)?;
    bank["samples"] = json!(samples);
    let ip = "audio/index.json";
    let mut index = load_json(assets, ip)?;
    let prefix = format!("{key}/");
    let sfx_list = list(&mut index["sfx"])?;
    sfx_list.retain(|b| !arr(&b["samples"]).iter().any(|s| s["file"].as_str().is_some_and(|f| f.starts_with(&prefix))));
    sfx_list.push(bank);
    write_json(assets, ip, &index, pyjson::INDENT1)?;
    let sp = "gamedata/sound.json";
    let mut sound = load_json(assets, sp)?;
    let rows = list(&mut sound["voice_rows"]["rows"])?;
    while rows.len() as i64 <= cid {
        let n = rows.len();
        rows.push(json!({"character": n, "counts": [0, 0, 0, 0], "codes": []}));
    }
    // the random attack voice as an attack starts (0x80073C50): the character's list, the lock it sets
    let p = disc.u32(RANDOM_VOICES + 4 * t2id as u32)?;
    let n = disc.i16(p)?;
    let mut voices = Vec::new();
    for i in 0..n.max(0) as u32 {
        if let Some(v) = voice_index.get(&(disc.u16(p + 2 + 2 * i)? & 0x1FF)) {
            voices.push(*v);
        }
    }
    let lock = disc.u8(VOICE_LOCK + t2id as u32)?;
    let codes: Vec<i64> = attack.iter().chain(&hit).chain(&hit).chain(&hit).copied().collect();
    // (system_start: the row plays as Tekken 2's sound player does, its bank-2 sounds from that sfx_table entry)
    let mut row = json!({"character": cid, "source": format!("Tekken 2 block {block}"), "system_start": start,
                         "counts": [attack.len(), hit.len(), hit.len(), hit.len()], "codes": codes,
                         "random": {"voices": voices, "lock": lock, "source": format!("0x{RANDOM_VOICES:08X} / 0x{VOICE_LOCK:08X}")}});
    // struck, Jack-2 and P.Jack (Tekken 2 ids 3, 11) sound their own metal: bank-6 sound 12 for an arm, 11 for a leg
    // (0x800733B0: 0x8004ECD2 / 0x8004ECDC), on their own voice bank
    if JACKS.contains(&t2id) {
        let mut struck = Map::new();
        for (part, idx) in [("arm", 12), ("leg", 11)] {
            if let Some(v) = voice_index.get(&idx) {
                struck.insert(part.into(), json!(0xD980 | v));
            }
        }
        if !struck.is_empty() {
            row["struck"] = Value::Object(struck);
        }
    }
    rows[cid as usize] = row;
    write_json(assets, sp, &sound, pyjson::INDENT1)?;
    Ok((voice_index, hit_index, start))
}

const VOICE_SCRIPTS: u32 = 0x800A1644; // Tekken 2 voice script id -> s16 {time, flags | kind, value} up to time 0
const SOUND_TABLE_A: u32 = 0x8004EC10; // script kind 0: u16 bank << 8 | sound
const SOUND_TABLE_B: u32 = 0x8004EC98; // script kinds 2 / 4 (and move sound types 1 / 2): the same

/// A Tekken 2 voice script (the interpreter at 0x80073750: its counter +0xB8 runs from the move's first frame,
/// entries fire on their time) as a sound.json script [cmd, frame, code]; its index there (sound.json scripts.t2,
/// keyed by character and script, after the original entries). Flags bit 15: only for the character (bits 13..8;
/// with bit 14 a Tekken 2 special case for character 0x16). Kinds (0x80050CF8): 0 table A sound, 2 table B sound, 4
/// table B sound at the opponent (0x3C: the stage's fall sound, 0x800D64E8 & 3, first as 0x26), 1 / 5 / 6 an own
/// voice, 3 the opponent's voice, 7 the opponent's character voice 0x80050C38. Bank 2 sounds are the system sounds
/// in the fighter's bank (cmd 0, sfx_table.t2_system), own voices cmd 1 (category 0, or 1 for the hit voices 0..2),
/// the opponent's voices cmd 2 with its hit voices (category 1: 0, 1; 2 and beyond as 1).
fn transplant_script(
    disc: &Disc,
    assets: &dyn Vfs,
    t2id: i64,
    sid: u16,
    voice_index: &BTreeMap<u16, i64>,
    hit_index: &BTreeMap<u16, i64>,
) -> Result<Option<i64>> {
    let sp = "gamedata/sound.json";
    let mut sound = load_json(assets, sp)?;
    let start = int(&sound["sfx_table"]["t2_system"]["start"]);
    let codes: Vec<i64> = arr(&sound["sfx_table"]["codes"]).iter().map(int).collect();
    let own_voice = |idx: u16| {
        voice_index.get(&idx).map(|v| (1, 0x2000 | v)).or_else(|| hit_index.get(&idx).map(|v| (1, 0x3000 | v)))
    };
    let table = |entry: u16| -> Option<(i64, i64)> {
        if entry == 0xFF {
            return None;
        }
        let (bank, idx) = (entry >> 8, entry & 0xFF);
        if bank == 2
            && (idx as usize) < SYSTEM_COUNT
            && codes.get((start + idx as i64) as usize).is_some_and(|c| *c != 0)
        {
            return Some((0, start + idx as i64));
        }
        if bank == 6 {
            return own_voice(idx);
        }
        None
    };
    let mut out = Vec::new();
    let mut p = disc.u32(VOICE_SCRIPTS + 4 * sid as u32)?;
    loop {
        let (time, flags, value) = (disc.u16(p)?, disc.u16(p + 2)?, disc.i16(p + 4)?);
        p += 6;
        if time == 0 {
            break;
        }
        if flags & 0x8000 != 0 && (((flags >> 8) & 0x3F) as i64 != t2id || flags & 0x4000 != 0) {
            continue;
        }
        let kind = flags & 0xFF;
        let v = (value as u16) & 0x1FF;
        let e = match kind {
            0 => table(disc.u16(SOUND_TABLE_A.wrapping_add((2 * value as i32) as u32))?),
            2 | 4 => table(disc.u16(SOUND_TABLE_B + 2 * if kind == 4 && v == 0x3C { 0x26 } else { v as u32 })?),
            1 | 5 | 6 => own_voice(v),
            3 => Some((2, 0x3000 | (v as i64).min(1))),
            7 => Some((2, 0x3001)),
            _ => None,
        };
        if let Some((cmd, c)) = e {
            out.push(json!([cmd, time, c]));
        }
    }
    if out.is_empty() {
        return Ok(None);
    }
    let scripts = &mut sound["scripts"];
    let n_entries = arr(&scripts["entries"]).len();
    let s = obj(scripts)?;
    let base = int(s.entry("original_count").or_insert(json!(n_entries)));
    let added = obj(s.entry("t2").or_insert(json!({})))?;
    let key = format!("{t2id}:{sid}");
    added.insert(key.clone(), json!(out));
    let pos = added.keys().position(|k| *k == key).unwrap_or(0) as i64;
    let values: Vec<Value> = added.values().cloned().collect();
    let entries = list(&mut sound["scripts"]["entries"])?;
    entries.truncate(base as usize);
    entries.extend(values);
    write_json(assets, sp, &sound, pyjson::INDENT1)?;
    Ok(Some(base + pos))
}

/// The fighter's Tekken 2 stage music (character table +7 names the BGM; its two sets 0x800A3701, set 1 the default,
/// become the port's columns 0 and 1) as a music.json BGM played for the character; its BGM id.
fn transplant_music(disc: &Disc, src: &dyn DiscFile, assets: &dyn Vfs, block: usize, name: &str) -> Result<i64> {
    let p = disc.u32(em::CHAR_INFO + 4 * block as u32)?;
    let bgm = disc.u8(p + 7)? as u32;
    let streams = em::xa_streams(disc)?;
    let mp = "gamedata/music.json";
    let mut music = load_json(assets, mp)?;
    let mut columns = Vec::new();
    let mut iso = Iso::open(src)?;
    let xas_lba = iso.find("TEKKEN2/TEKKEN2.XAS")?.0;
    for v in 0..2 {
        let (cd, vol, xa) = em::bgm_entry(disc, bgm, v)?;
        if cd != 0 || xa < 0 {
            bail!("{name}'s BGM {bgm} set {v} is no XA stream");
        }
        let file = format!("t2_{xa:02}.flac");
        let path = join("audio/music", &file);
        if !assets.exists(&path) {
            let stream = *streams.get(xa as usize).context("no such XA stream")?;
            em::export_stream(&mut iso, xas_lba, stream, assets, &path)?;
        }
        let volume = if vol & 0x7F != 0 { (vol & 0x7F) as i64 } else { 100 };
        columns.push(json!({"raw": [xa, vol], "cdda": false, "xa_index": xa, "cdda_track": null, "volume": volume, "loop": true, "file": file}));
    }
    let t = title(name);
    let bgms = list(&mut music["bgm"])?;
    let old = bgms.iter().find(|b| {
        b["source"].as_str().unwrap_or("").starts_with("Tekken 2")
            && arr(&b["characters"]).iter().any(|c| c.as_str() == Some(t.as_str()))
    });
    let bid = old.map_or(bgms.len() as i64, |b| int(&b["id"]));
    bgms.retain(|b| int(&b["id"]) != bid);
    bgms.push(json!({"id": bid, "columns": columns, "characters": [t], "source": format!("Tekken 2 BGM {bgm} (0x{:08X})", em::BGM_TABLE)}));
    bgms.sort_by_key(|b| int(&b["id"]));
    write_json(assets, mp, &music, pyjson::INDENT1)?;
    Ok(bid)
}

const ENDING_BASE: i64 = 100; // the port's movie numbers of Tekken 2's movies: 100 + its movie number (video/1NN.avi)

/// (width, height) of an AVI from its main header.
fn avi_size(d: &[u8]) -> Result<(u32, u32)> {
    let d = &d[..d.len().min(256)];
    let i = d.windows(4).position(|w| w == b"avih").map_or(-1, |i| i as i64);
    let o = (i + 8 + 32) as usize;
    Ok((u32_(d, o)?, u32_(d, o + 4)?))
}

const THEATER_FIRST: usize = 24; // the Theater's grid cell of the first Tekken 2 ending (the row after the original's)
const THEATER_THUMBS: &str = "Theater thumbnails (Tekken 2)";
const FACES_RECORD: usize = 19; // Tekken 2's select faces: 160 x 100, by Tekken 2 character id (Kazuya 0, Jun 19)
const RANDOM_FACE: i64 = 23; // the select's "?" (random): Alex's id holds it, Alex shares Roger's slot (block 45 = his second)

/// The Tekken 2 select face of a character (or the block's, for the random face's id).
fn select_face(disc: &Disc, t2id: i64, block: usize) -> Result<Rgba> {
    let x = disc.rec(FACES_RECORD)?;
    let mut tims: Vec<_> = walk_tims(x).into_iter().filter(|t| t.img.is_some()).collect();
    tims.extend(scan_lz_tims(x).into_iter().map(|t| t.2).filter(|t| t.img.is_some()));
    let k = if t2id == RANDOM_FACE { (block / 2) as i64 } else { t2id };
    let t = usize::try_from(k).ok().and_then(|k| tims.get(k)).context("no select face")?;
    render(t)?.context("a 24-bit select face")
}

/// `Image.paste(src, (x, y))` (no mask: the pixels replaced), clipped to the picture.
fn paste_at(dst: &mut Rgba, src: &Rgba, x: usize, y: usize) {
    for sy in 0..src.h.min(dst.h.saturating_sub(y)) {
        for sx in 0..src.w.min(dst.w.saturating_sub(x)) {
            dst.set(x + sx, y + sy, src.at(sx, sy));
        }
    }
}

fn filled(w: usize, h: usize, c: [u8; 4]) -> Rgba {
    Rgba { w, h, px: c.repeat(w * h) }
}

/// The ending's Theater cell (theater.json entries from THEATER_FIRST, one per ending movie: costumes that share
/// one share it): its 48 x 64 thumbnail from the Tekken 2 select face (pictures/theater_t2/NN.png, listed in
/// pictures/index.json), the costume's name, unlocked by its clear (mask: bit 32 + its number; the port keeps the
/// Tekken 2 clears above the original's 32 ending bits). Returns its number.
fn theater_entry(
    disc: &Disc,
    assets: &dyn Vfs,
    theater: &mut Value,
    block: usize,
    costume_name: &str,
    movie_entry: usize,
) -> Result<usize> {
    let entries = list(&mut theater["entries"])?;
    let source = format!("Tekken 2 ending, theater movie {movie_entry}");
    let src_str = |e: &Value| e["source"].as_str().unwrap_or("").to_string();
    let truthy = |e: &Value| !e.is_null() && e.as_object().is_none_or(|o| !o.is_empty());
    let own: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter(|(_, e)| truthy(e) && src_str(e).starts_with("Tekken 2 ending"))
        .map(|(i, _)| i)
        .collect();
    let at = match own.iter().find(|&&i| entries[i]["source"] == source.as_str()) {
        Some(&i) => i,
        None => own.iter().copied().chain([THEATER_FIRST - 1]).max().unwrap_or(0) + 1,
    };
    while entries.len() <= at {
        entries.push(Value::Null);
    }
    let bit = at - THEATER_FIRST;
    if bit >= 32 {
        bail!("Tekken 2 ending {bit} has no clear bit");
    }
    // the thumbnail: the select face, as the 32 x 56 faces are made (160 x 100 square on its screen), 48 wide,
    // cropped to 64 tall around the eyes
    let ids: HashMap<String, i64> = disc.characters()?.into_iter().map(|c| (c.name.to_uppercase(), c.id)).collect();
    let t2id = ids.get(costume_name).copied().unwrap_or(0);
    let face = pil::resize_lanczos(&select_face(disc, t2id, block)?, 48, 84);
    let mut thumb = filled(48, 64, [0, 0, 0, 255]);
    let mut crop = Rgba::new(48, 64);
    crop.paste(0, 0, 48, 64, &face, 0, 8)?;
    paste_at(&mut thumb, &crop, 0, 0);
    let file = format!("theater_t2/{block:02}.png");
    pil::save_png(&thumb, assets, &join("pictures", &file))?;
    let ip = "pictures/index.json";
    let mut index = load_json(assets, ip)?;
    let sets = list(&mut index)?;
    let k = match sets.iter().position(|x| x["name"] == THEATER_THUMBS) {
        Some(k) => k,
        None => {
            sets.push(json!({"name": THEATER_THUMBS, "ui": true, "images": []}));
            sets.len() - 1
        }
    };
    let imgs = list(&mut sets[k]["images"])?;
    imgs.retain(|im| im["file"] != file.as_str());
    imgs.push(json!({"file": file, "w": 48, "h": 64}));
    write_json(assets, ip, &index, pyjson::INDENT1)?;
    theater["entries"][at] = json!({"movie": movie_entry, "movie_alt": movie_entry, "picture": file, "mask": 1u64 << (32 + bit), "kind": 0,
                                    "line1": costume_name, "line2": "TEKKEN 2", "line3": null, "source": source});
    Ok(bit)
}

/// The costume's Tekken 2 ending movie (record 20's table 0x801D00E4, by block: Devil's and Angel's, Roger's and
/// Alex's differ) as the port's movie ENDING_BASE + its number (frames .avi and soundtrack .flac, copied from export_media's
/// assets_t2/media/video: `videos`), a theater.json movie entry, and ending.extra[character][costume] = that entry; its entry
/// index.
fn transplant_ending(
    disc: &Disc,
    assets: &dyn Vfs,
    videos: &dyn Vfs,
    block: usize,
    cid: i64,
    costume: usize,
    costume_name: &str,
) -> Result<usize> {
    let m = em::ending_movie(disc, block)? as i64;
    let avi = format!("{m:02}.avi");
    if !videos.exists(&avi) {
        bail!("the Tekken 2 movie {avi} is missing: run export_media (video) first");
    }
    for ext in ["avi", "flac"] {
        let f = format!("{m:02}.{ext}");
        if videos.exists(&f) {
            assets.write(&format!("video/{:02}.{ext}", ENDING_BASE + m), &videos.read(&f)?)?;
        }
    }
    let (w, h) = avi_size(&videos.read(&avi)?)?;
    let tp = "gamedata/theater.json";
    let mut theater = load_json(assets, tp)?;
    let source = format!("Tekken 2 ending movie {m} (record 20, 0x801D00E4)");
    let movies = list(&mut theater["movies"])?;
    let entry = match movies.iter().position(|x| x["source"] == source.as_str()) {
        Some(e) => e,
        None => {
            movies.push(Value::Null);
            movies.len() - 1
        }
    };
    movies[entry] = json!({"file": (ENDING_BASE + m).to_string(), "w": w, "h": h, "frames": 0, "volume": 127, "loop": false,
                           "white": false, "end_mode": 3, "captions": null, "source": source});
    let ending = obj(&mut theater["ending"])?;
    let extra = obj(ending.entry("extra").or_insert(json!({})))?;
    let row = list(extra.entry(cid.to_string()).or_insert(json!([])))?;
    while row.len() <= costume {
        row.push(json!(entry));
    }
    row[costume] = json!(entry);
    let bits = obj(ending.entry("extra_bits").or_insert(json!({})))?;
    let bits = list(bits.entry(cid.to_string()).or_insert(json!([])))?;
    while bits.len() <= costume {
        bits.push(Value::Null);
    }
    let bit = theater_entry(disc, assets, &mut theater, block, costume_name, entry)?;
    theater["ending"]["extra_bits"][cid.to_string()][costume] = json!(bit);
    write_json(assets, tp, &theater, pyjson::INDENT1)?;
    Ok(entry)
}

/// Drop what earlier runs made for characters no longer brought over as such (Angel and Alex became Devil's and
/// Roger's second costumes): roster characters and selections, voice rows, music, endings.
fn purge(ctx: &Ctx, fighters: &[Fighter]) -> Result<()> {
    let assets = ctx.out;
    let keep: BTreeSet<i64> = fighters.iter().map(|f| f.cid).collect();
    let rp = "gamedata/roster.json";
    let mut roster = load_json(assets, rp)?;
    let gone: BTreeSet<i64> = arr(&roster["characters"])
        .iter()
        .filter(|c| c["source"] == "Tekken 2" && !keep.contains(&int(&c["id"])))
        .map(|c| int(&c["id"]))
        .collect();
    if gone.is_empty() {
        return Ok(());
    }
    list(&mut roster["characters"])?.retain(|c| !gone.contains(&int(&c["id"])));
    list(&mut roster["selections"]["entries"])?.retain(|e| !gone.contains(&int(&e["character"])));
    write_json(assets, rp, &roster, pyjson::INDENT1)?;
    let sp = "gamedata/sound.json";
    let mut sound = load_json(assets, sp)?;
    let rows = list(&mut sound["voice_rows"]["rows"])?;
    for &c in &gone {
        if (c as usize) < rows.len() {
            rows[c as usize] = json!({"character": c, "counts": [0, 0, 0, 0], "codes": []});
        }
    }
    write_json(assets, sp, &sound, pyjson::INDENT1)?;
    let names: BTreeSet<String> =
        arr(&roster["characters"]).iter().filter_map(|c| c["name"].as_str().map(String::from)).collect();
    let mp = "gamedata/music.json";
    let mut music = load_json(assets, mp)?;
    for b in list(&mut music["bgm"])? {
        if b["source"].as_str().unwrap_or("").starts_with("Tekken 2") {
            list(&mut b["characters"])?.retain(|n| n.as_str().is_some_and(|n| names.contains(n)));
        }
    }
    write_json(assets, mp, &music, pyjson::INDENT1)?;
    let tp = "gamedata/theater.json";
    let mut theater = load_json(assets, tp)?;
    if let Some(extra) = theater["ending"].get_mut("extra").and_then(Value::as_object_mut) {
        for c in &gone {
            extra.shift_remove(&c.to_string());
        }
    }
    write_json(assets, tp, &theater, pyjson::INDENT1)?;
    ctx.log(&format!("dropped characters {:?}", gone.iter().collect::<Vec<_>>()));
    Ok(())
}

/// One fighter's costume (block): its glb (model, Tekken 2 motions, the donor's motions) and its move set,
/// registered in the port's index and roster.
#[allow(clippy::too_many_arguments)]
fn transplant_fighter(
    disc: &Disc,
    src: &dyn DiscFile,
    assets: &dyn Vfs,
    videos: &dyn Vfs,
    f: &Fighter,
    block: usize,
    tmp: &str,
) -> Result<usize> {
    let (name, cid, home) = (&f.name, f.cid, f.home);
    let (donor_model, donor_set) = (&f.donor.0, &f.donor.1);
    let costume = f.blocks.iter().position(|b| *b == block).context("not this fighter's block")?;
    let (t2id, own_name) = (f.t2ids[costume], &f.costume_names[costume]);
    let record = 900 + block;
    let src_glb = join(tmp, "src.glb");
    t2::convert(disc, block, assets, &src_glb)?;
    let (voice_index, hit_index, system_start) =
        transplant_sound(disc, assets, block, record, voice_row(cid, costume), t2id, tmp)?;
    let bgm = transplant_music(disc, src, assets, f.blocks[0], name)?;
    let ending = transplant_ending(disc, assets, videos, block, cid, costume, own_name)?;
    let t2set = tm::T2Set::new(disc, block, t2id)?;
    let (stance, moves) = tm::graph(&t2set)?;
    let trig = t2::Trig::new(disc)?;
    let common = &disc.exe[off(tm::COMMON_MOTIONS).min(disc.exe.len())..];
    let mut dec = t2::Decoder::new(&disc.exe, common);
    let mut extra: Vec<Clip> = Vec::new();
    for &r in std::iter::once(&stance).chain(&moves) {
        if r.common {
            let o = t2set.motion(r)? as usize;
            let (rot, root) = t2::clip_tracks(&trig, &dec.decode(common, o)?);
            let rots = rot.into_iter().enumerate().map(|(k, v)| (BONE_NAMES[k].to_string(), v)).collect();
            extra.push((format!("t2c_{:03}", r.n), rots, root));
        }
    }
    let glb = format!("characters/model_{record:03}.glb");
    let model_arc = t2::arc(disc.rec(t2::BLOCK0 + 8 * block + 2)?)?;
    let part2 = *model_arc.get(2).context("the model arc has no chains part")?;
    // the wings' modes (Devil, Angel): the stance clip and the moves' air windows (move +0x13 .. +0x14)
    let own = t2::motions_of(&t2set.mv)?;
    // and the guard: the block reactions (reactions.json slots 10 / 11 through the donor's aliases; the common
    // moves' clips) fold the wings (mode 3: hit, +0x5C, while blocking, +0x5F)
    let dset = load_json(assets, &join("gamedata/movesets", donor_set))?;
    let common_moves = load_json(assets, "gamedata/movesets/common.json")?["moves"].take();
    let reactions_t3 = load_json(assets, "gamedata/reactions.json")?["entries"].take();
    let mut guard = BTreeSet::new();
    for e in arr(&reactions_t3) {
        for a in arr(&e["victims"]).iter().skip(10).take(2).map(int) {
            let al = arr(&dset["aliases"]);
            if 0 < a && (a as usize) < al.len() {
                let r = &al[a as usize];
                if r["bank"] == "common" {
                    let anim = &common_moves[int(&r["move"]) as usize]["anim"];
                    guard.insert(anim.as_str().map_or_else(|| pyjson::compact(anim), String::from));
                }
            }
        }
    }
    let mut air = HashMap::new();
    for &r in std::iter::once(&stance).chain(&moves) {
        let raw = t2set.raw(r);
        let (a, b) = (raw.get(0x13).copied().unwrap_or(0), raw.get(0x14).copied().unwrap_or(0));
        if a < b {
            air.insert(tm::clip_name(&t2set, r, &own)?, (a, b));
        }
    }
    let wing_info = WingInfo { stance: tm::clip_name(&t2set, stance, &own)?, guard, air };
    transplant(
        assets,
        &src_glb,
        &glb,
        &format!("Tekken 2 {}", title(name)),
        &extra,
        Some(&format!("characters/model_{donor_model:03}.glb")),
        "t2m_",
        Some((&disc.exe, part2, t2id as u32)),
        Some(&wing_info),
    )?;
    let shared = load_json(assets, "gamedata/shared.json")?;
    let donor = dset;
    let rp = "gamedata/reactions.json";
    let mut reactions = load_json(assets, rp)?;
    let tvo = obj(&mut reactions["throw_victims"])?;
    let n_tv = tvo.get("entries").map_or(0, |v| arr(v).len());
    let base = int(tvo.entry("original_count").or_insert(json!(n_tv))) as usize;
    let added = tvo.entry("added").or_insert(json!({})).clone();
    let mut keys: Vec<(i64, String)> =
        added.as_object().map_or(vec![], |o| o.keys().map(|k| (k.parse().unwrap_or(0), k.clone())).collect());
    keys.sort();
    let mut tv: Vec<Value> = arr(&reactions["throw_victims"]["entries"]).iter().take(base).cloned().collect();
    // the original entries, then those of the fighters with lower model records (built first, so indices hold)
    for (k, ks) in &keys {
        if *k < record as i64 {
            tv.extend(arr(&added[ks]).iter().cloned());
        }
    }
    let start = tv.len();
    let mut script = |sid: u16| transplant_script(disc, assets, t2id, sid, &voice_index, &hit_index);
    let mut ctx = tm::Ctx {
        reactions_t3: arr(&reactions_t3).to_vec(),
        common_t3: arr(&common_moves).to_vec(),
        shared_cancels: arr(&shared["shared_cancels"]["entries"]).to_vec(),
        voice_index: voice_index.clone(),
        system_start,
        script: Some(&mut script),
    };
    let mut world = posed(assets, &glb)?;
    let (out, moves) = tm::build(&mut ctx, &t2set, &donor, &own, &mut world, f.moveset_id, &mut tv)?;
    let tvo = obj(&mut reactions["throw_victims"])?;
    let added = obj(tvo.entry("added").or_insert(json!({})))?;
    added.insert(record.to_string(), json!(tv[start..]));
    let mut keys: Vec<(i64, String)> = added.keys().map(|k| (k.parse().unwrap_or(0), k.clone())).collect();
    keys.sort();
    let mut all: Vec<Value> = tv[..base.min(tv.len())].to_vec();
    for (_, k) in &keys {
        all.extend(arr(&added[k]).iter().cloned());
    }
    tvo.insert("entries".into(), json!(all));
    write_json(assets, rp, &reactions, pyjson::COMPACT)?;
    let set_file = format!("moveset_t2_{}.json", own_name.to_lowercase().replace(['.', '-'], "").replace(' ', "_"));
    write_json(assets, &join("gamedata/movesets", &set_file), &out, pyjson::COMPACT)?;
    // register: the moveset index and the roster (character, selection)
    let ip = "gamedata/movesets/index.json";
    let mut index = load_json(assets, ip)?;
    let models = list(&mut index["models"])?;
    models.retain(|m| int(&m["model_record"]) != record as i64);
    models.push(json!({"model_record": record, "glb": format!("model_{record:03}.glb"), "moveset_file": set_file,
                       "moveset_id": f.moveset_id, "source": format!("Tekken 2 block {block}")}));
    write_json(assets, ip, &index, pyjson::INDENT1)?;
    let rp = "gamedata/roster.json";
    let mut roster = load_json(assets, rp)?;
    let sel = cid * 4 + costume as i64;
    let glow = make_art(disc, assets, f.portraits[costume], t2id, block, costume == 0)?;
    let chars = list(&mut roster["characters"])?;
    chars.retain(|c| int(&c["id"]) != cid);
    // (prefix: the engine's command line names it t2:NAME beside Tekken 3's of the same name; its second costume is
    // always there; face: its select face, a picture of its own)
    chars.push(json!({"id": cid, "name": title(name), "exe_name": name, "selections": (0..f.blocks.len() as i64).map(|k| cid * 4 + k).collect::<Vec<_>>(),
                      "source": "Tekken 2", "select_row": f.select_row, "grid_slot": f.grid_slot,
                      "prefix": "t2", "extra_costume": true, "face": format!("faces/{cid:02}.png")}));
    chars.sort_by_key(|c| int(&c["id"]));
    let entries = list(&mut roster["selections"]["entries"])?;
    entries.retain(|e| int(&e["selection"]) != sel);
    entries.push(json!({"selection": sel, "character": cid, "costume": costume, "name": own_name, "model_index": null,
                        "model_record": record, "glb": format!("model_{record:03}.glb"), "moveset_file": format!("movesets/{set_file}"),
                        "moveset_id": f.moveset_id, "home_stage": home, "bgm_id": bgm, "source": format!("Tekken 2 block {block}"),
                        "ending_movie": ending, "voice_row": voice_row(cid, costume),
                        "portrait": {"portrait": f.portraits[costume], "flipped": false, "fade_type": 1, "glow": glow}}));
    entries.sort_by_key(|e| int(&e["selection"]));
    let models = list(&mut roster["models"])?;
    models.retain(|m| int(&m["model_record"]) != record as i64);
    models.push(json!({"model_record": record, "glb": format!("model_{record:03}.glb"), "size_percent": 100, "moveset_file": format!("movesets/{set_file}")}));
    write_json(assets, rp, &roster, pyjson::INDENT1)?;
    Ok(moves.len())
}

/// Kazuya's and Jun's portrait glows, chosen by eye; the others' from the face
fn glow_of(cid: i64) -> Option<[i64; 3]> {
    match cid {
        23 => Some([217, 0, 28]),
        24 => Some([240, 200, 210]),
        _ => None,
    }
}

/// The select pictures of a fighter brought from Tekken 2, from its Tekken 2 select face: a 126 x 252 portrait (the
/// face enlarged at the top, transparent below; portraits/<cid>) and, for its first costume (`face`), a 32 x 58 face
/// for the grids. Returns the portrait's glow.
fn make_art(disc: &Disc, assets: &dyn Vfs, cid: i64, t2id: i64, block: usize, face_too: bool) -> Result<[i64; 3]> {
    let face = select_face(disc, t2id, block)?;
    // the glow behind the portrait: the face's mean colour, brightened to full value
    let n = (face.w * face.h).max(1) as f64;
    let mut sum = [0f64; 3];
    for p in face.px.as_chunks::<4>().0 {
        for c in 0..3 {
            sum[c] += p[c] as f64;
        }
    }
    let rgb = sum.map(|s| s / n);
    let mx = rgb.iter().copied().fold(f64::NEG_INFINITY, f64::max).max(1.0);
    let glow = glow_of(cid).unwrap_or_else(|| rgb.map(|v| (v * 255.0 / mx).round_ties_even() as i64));
    // the face is drawn for a 512 x 240 screen (pixels 5/8 as wide as tall: 160 x 100 is square), the select screen
    // is 368 x 480 (pixels 480/368 x 4/3 = 1.74 as wide as tall): 126 wide is 219 tall
    let big = pil::resize_lanczos(&face, 126, 219);
    let mut portrait = Rgba::new(126, 252);
    paste_at(&mut portrait, &big, 0, 8);
    pil::save_png(&portrait, assets, &format!("pictures/portraits/{cid:02}.png"))?;
    if face_too {
        let mut small = filled(32, 58, [0, 0, 0, 255]);
        paste_at(&mut small, &pil::resize_lanczos(&face, 32, 56), 0, 1);
        pil::save_png(&small, assets, &format!("pictures/faces/{cid:02}.png"))?;
    }
    Ok(glow)
}

/// transplant BIN ASSETS [blocks]: every fighter (or those costume blocks) and their home arenas.
pub fn main(args: &[String]) -> Result<()> {
    let [src, assets_arg, rest @ ..] = args else { bail!("usage: t2_convert transplant BIN T3_ASSETS [block,...]") };
    let only: Option<BTreeSet<usize>> = match rest.first() {
        Some(s) => Some(
            s.split(',').map(|v| v.trim().parse().with_context(|| format!("bad block {v}"))).collect::<Result<_>>()?,
        ),
        None => None,
    };
    // the Tekken 2 movies: export_media's, in assets_t2/media/video beside the assets
    let abs = std::path::absolute(assets_arg)?;
    let videos = StdFs(abs.parent().unwrap_or(std::path::Path::new("/")).join("assets_t2/media/video"));
    let ctx = Ctx { out: &StdFs(assets_arg.into()), log: &conv_io::print_line, jobs: 1 };
    run(&ctx, &StdDisc(src.into()), &videos, only.as_ref())?;
    // (the files changed: online play needs the fingerprint again; Tekken 2's tools never run Tekken 3's)
    println!("now: target/release/t3_convert --fingerprint {assets_arg}");
    Ok(())
}

const TMP: &str = ".tmp/t2_transplant"; // the scratch files, removed at the end
const BOSSES: [&str; 2] = ["KAZUYA", "DEVIL"]; // Tekken 2's bosses, its arcade's last two

/// The fighters brought over (roster.json's) in the lists the engine draws from, anew each run (Tekken 3's own
/// ids are 0..=22, its models' records below 900): Tekken 3's arcade leaves the bosses out of stages 5-8 as its own,
/// TAG ARCADE ends with them as a pair before Tekken 3's; Survival draws the others from its second pool on and the
/// bosses in its last; they cast the shadow most of Tekken 3's fighters share.
fn register(assets: &dyn Vfs) -> Result<()> {
    let roster = load_json(assets, "gamedata/roster.json")?;
    let brought: Vec<&Value> = arr(&roster["characters"]).iter().filter(|c| c["source"] == "Tekken 2").collect();
    let ids: Vec<i64> = brought.iter().map(|c| int(&c["id"])).collect();
    let bosses: Vec<i64> =
        BOSSES.iter().filter_map(|b| brought.iter().find(|c| c["exe_name"] == *b)).map(|c| int(&c["id"])).collect();
    let own = |v: &Value| int(v) <= 22;
    let ap = "gamedata/ai.json";
    let mut ai = load_json(assets, ap)?;
    let again = "ai.json has no arcade ladder: convert the Tekken 3 disc again first";
    for step in list(&mut ai["arcade"]["ladder"]).context(again)? {
        if let Some(except) = step.get_mut("except") {
            let except = list(except)?;
            except.retain(own);
            except.extend(bosses.iter().map(|&b| json!(b)));
        }
    }
    let pairs = list(&mut ai["arcade"]["boss_pairs"]).context(again)?;
    pairs.retain(|p| arr(p).iter().all(own));
    if bosses.len() == 2 {
        pairs.insert(0, json!(bosses));
    }
    write_json(assets, ap, &ai, pyjson::INDENT1)?;
    let sp = "gamedata/screens.json";
    let mut screens = load_json(assets, sp)?;
    let pools = list(&mut screens["modes"]["survival"]["pools"])?;
    let last = pools.len().saturating_sub(1);
    for (k, p) in pools.iter_mut().enumerate().skip(1) {
        let chars = list(&mut p["characters"])?;
        chars.retain(own);
        chars.extend(ids.iter().filter(|i| k == last || !bosses.contains(i)).map(|&i| json!(i)));
    }
    write_json(assets, sp, &screens, pyjson::INDENT1)?;
    let records: Vec<Value> = arr(&roster["selections"]["entries"])
        .iter()
        .filter(|e| ids.contains(&int(&e["character"])))
        .map(|e| e["model_record"].clone())
        .collect();
    let hp = "gamedata/shadows.json";
    let mut shadows = load_json(assets, hp)?;
    let meshes = list(&mut shadows["meshes"])?;
    for m in meshes.iter_mut() {
        list(&mut m["models"])?.retain(|r| int(r) < 900);
    }
    // (the last of the largest)
    if let Some(m) = meshes.iter_mut().max_by_key(|m| arr(&m["models"]).len()) {
        list(&mut m["models"])?.extend(records);
    }
    write_json(assets, hp, &shadows, pyjson::COMPACT)
}

/// Every fighter (or those costume blocks) and their home arenas into the Tekken 3 assets (`ctx.out`), the endings'
/// movies from `videos` (export_media's video folder).
pub fn run(ctx: &Ctx, src: &dyn DiscFile, videos: &dyn Vfs, only: Option<&BTreeSet<usize>>) -> Result<()> {
    let assets = ctx.out;
    let disc = Disc::open(src)?;
    let (fighters, stages) = plan(&disc, assets)?;
    purge(ctx, &fighters)?;
    let r = (|| -> Result<()> {
        for (sid, stage) in &stages {
            if only.is_none_or(|o| fighters.iter().any(|f| f.home == *sid && f.blocks.iter().any(|b| o.contains(b)))) {
                transplant_stage(&disc, assets, *sid, stage)?;
                ctx.log(&format!("stage_{sid:02}.glb {}", stage.2));
            }
        }
        for f in &fighters {
            for &b in &f.blocks {
                if only.is_none_or(|o| o.contains(&b)) {
                    let n = transplant_fighter(&disc, src, assets, videos, f, b, TMP)
                        .with_context(|| format!("{} (block {b})", f.name))?;
                    ctx.log(&format!("model_{:03}.glb {} (block {b}): {n} moves from Tekken 2", 900 + b, f.name));
                }
            }
        }
        register(assets)
    })();
    // the scratch files go, and .tmp with them when nothing else is in it
    let cleaned = (|| {
        assets.remove_all(TMP)?;
        if assets.list(".tmp")?.is_empty() {
            assets.remove_all(".tmp")?;
        }
        Ok(())
    })();
    r.and(cleaned)
}
