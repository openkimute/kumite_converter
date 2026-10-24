//! export_gltf: convert Tekken 3 (USA, SLUS-00402) characters to glTF 2.0 binary (.glb).
//!
//! usage: t3_convert export_gltf "Tekken_3_(USA)_(Track 1).bin" OUTDIR [ids] [--with-common] [--with-embu]
//!
//! --with-embu also bakes the attract embu's 22 motions ("embu_000"...) into every character, and writes the
//! embu-only versions of some costumes (records 1-3: embu_N_PP.glb, embu motions only; see embu.rs).
//!
//! Each .glb contains one character: a skinned mesh, its texture atlas (embedded PNG), the
//! skeleton, and every motion as a named animation ("motion_000", ...). The game's arm/leg IK is
//! solved here and baked into plain joint rotations, so any glTF consumer (Blender, three.js,
//! Unity, Godot, ...) can play the animations without game-specific code.
//! OUTDIR/index.json lists the files. Data stays local: it is extracted from your own disc image.
//! The GLB writer is glb.rs.

use crate::anim::{self, Tables};
use crate::disc::{self, u32le};
use crate::glb::{Glb, f32_bytes, f64_value, u32_bytes};
use crate::np::{self, I3, M3, M4, V3, mm, mv};
use crate::skel::{self, Extras, Mode, Pose, Skeleton, Spring, Springs, Tr};
use crate::skin::{self, Vert};
use crate::{atlas, embu, export_stages, pyjson};
use anyhow::{Context, Result, bail};
use conv_io::{Ctx, DiscFile, StdDisc, StdFs, Vfs, jobs_from, join, print_line};
use serde_json::{Map, Value, json};
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{LazyLock, Mutex};

/// BNS record of the motion set shared by all characters
pub const COMMON_MOTIONS: usize = 279;
/// game frame rate; one motion frame per video frame
pub const FPS: u32 = 60;
/// frames of spring simulation run on a clip's first pose
pub const SETTLE: u32 = 90;
/// game units are ~1 mm
pub const SCALE: f64 = 0.001;
/// PS1 (y down, z forward) -> glTF (y up): 180 deg about X
pub const C: [[f64; 3]; 3] = [[1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, -1.0]];

/// Display names: each motion record's moveset id (byte 1 of its header) + the EXE roster strings.
pub const GROUPS: [(&str, &[usize]); 25] = [
    ("Paul", &[71, 75]),
    ("Law", &[79, 83, 231]),
    ("Lei", &[87, 91]),
    ("King", &[95, 99]),
    ("Yoshimitsu", &[103, 107]),
    ("Nina", &[111, 115]),
    ("Hwoarang", &[119, 123]),
    ("Xiaoyu", &[127, 131, 223]),
    ("Eddy", &[135, 139]),
    ("Tiger", &[219]),
    ("Jin", &[143, 147, 227]),
    ("Julia", &[151, 155]),
    ("Kuma", &[159]),
    ("Panda", &[163]),
    ("Bryan", &[167, 171]),
    ("Heihachi", &[175, 179]),
    ("Ogre", &[183, 187]),
    ("True Ogre", &[199, 203]),
    ("Mokujin", &[191, 195]),
    ("Gun Jack", &[207, 211]),
    ("Anna", &[215, 235, 255]),
    ("Dr. Bosconovitch", &[247, 251]),
    ("Gon", &[239, 243]),
    ("Tekken Force", &[259, 263, 267, 271]),
    ("Unknown (Gun Jack moveset)", &[275]),
];

/// model record -> display name
pub static NAMES: LazyLock<BTreeMap<usize, String>> = LazyLock::new(|| {
    let mut m = BTreeMap::new();
    for (n, ids) in GROUPS {
        for (k, &i) in ids.iter().enumerate() {
            let what = if n == "Tekken Force" { "soldier" } else { "costume" };
            m.insert(i, if ids.len() == 1 { n.to_string() } else { format!("{n} ({what} {})", k + 1) });
        }
    }
    m
});

/// glTF node hierarchy for the main skeleton (rows of the 3DMK table), as (row, parent) in the Python dict's order.
/// Row 20 = alternate head part.
pub const MAIN_PARENT: [(usize, usize); 18] = [
    (1, 0),
    (3, 0),
    (19, 1),
    (20, 19),
    (11, 1),
    (12, 11),
    (13, 12),
    (14, 13),
    (15, 1),
    (16, 15),
    (17, 16),
    (18, 17),
    (5, 3),
    (6, 5),
    (7, 6),
    (8, 3),
    (9, 8),
    (10, 9),
];

pub const ROW_NAMES: [(usize, &str); 19] = [
    (0, "root"),
    (1, "chest"),
    (3, "pelvis"),
    (19, "head"),
    (20, "head_b"),
    (11, "clavicle_A"),
    (12, "upperarm_A"),
    (13, "forearm_A"),
    (14, "hand_A"),
    (15, "clavicle_B"),
    (16, "upperarm_B"),
    (17, "forearm_B"),
    (18, "hand_B"),
    (5, "thigh_A"),
    (6, "shin_A"),
    (7, "foot_A"),
    (8, "thigh_B"),
    (9, "shin_B"),
    (10, "foot_B"),
];

/// `ROW_NAMES.get(r, f'bone_{r}')`
pub fn row_name(r: usize) -> String {
    ROW_NAMES.iter().find(|e| e.0 == r).map_or(format!("bone_{r}"), |e| e.1.to_string())
}

pub fn s32(v: u32) -> i64 {
    v as i32 as i64
}

fn main_parent(r: usize) -> Option<usize> {
    MAIN_PARENT.iter().find(|e| e.0 == r).map(|e| e.1)
}

/// Rotation matrix -> unit quaternion (x, y, z, w).
pub fn quat(m: &M3) -> [f64; 4] {
    let tr = m[0][0] + m[1][1] + m[2][2];
    let (x, y, z, w);
    if tr > 0.0 {
        let s = 0.5 / (tr + 1.0).sqrt();
        w = 0.25 / s;
        (x, y, z) = ((m[2][1] - m[1][2]) * s, (m[0][2] - m[2][0]) * s, (m[1][0] - m[0][1]) * s);
    } else if m[0][0] > m[1][1] && m[0][0] > m[2][2] {
        let s = 2.0 * (1.0 + m[0][0] - m[1][1] - m[2][2]).sqrt();
        w = (m[2][1] - m[1][2]) / s;
        (x, y, z) = (0.25 * s, (m[0][1] + m[1][0]) / s, (m[0][2] + m[2][0]) / s);
    } else if m[1][1] > m[2][2] {
        let s = 2.0 * (1.0 + m[1][1] - m[0][0] - m[2][2]).sqrt();
        w = (m[0][2] - m[2][0]) / s;
        (x, y, z) = ((m[0][1] + m[1][0]) / s, 0.25 * s, (m[1][2] + m[2][1]) / s);
    } else {
        let s = 2.0 * (1.0 + m[2][2] - m[0][0] - m[1][1]).sqrt();
        w = (m[1][0] - m[0][1]) / s;
        (x, y, z) = ((m[0][2] + m[2][0]) / s, (m[1][2] + m[2][1]) / s, 0.25 * s);
    }
    let q = [x, y, z, w];
    let n = np::norm(&q);
    q.map(|v| v / n)
}

pub fn orthonormalize(r: &M3) -> M3 {
    let (u, _, vt) = np::svd(r);
    mm(&u, false, &vt, false)
}

/// Bind-pose rotation of each main joint relative to its parent, as fitted from welded seams.
/// Only joints whose fitted rotation is identical across many models are listed (pelvis 47, clavicles 29,
/// head 16-23, feet 36-37 models).
/// (upper-arm bind rotation varies between models, so it is not standardised; unlisted pairs use none)
pub fn canon_bind(q0: usize, r: usize) -> Option<M3> {
    let m = |v: [f64; 9]| [[v[0], v[1], v[2]], [v[3], v[4], v[5]], [v[6], v[7], v[8]]];
    Some(match (q0, r) {
        (1, 3) => m([-1., 0., 0., 0., -1., 0., 0., 0., 1.]),
        (1, 19) => m([0., 0., 1., 1., 0., 0., 0., 1., 0.]),
        (1, 11) => m([0., 1., 0., 0., 0., -1., -1., 0., 0.]),
        (1, 15) => m([0., -1., 0., 0., 0., -1., 1., 0., 0.]),
        (6, 7) | (9, 10) => m([0., 1., 0., -1., 0., 0., 0., 0., 1.]),
        _ => return None,
    })
}

/// FIT_PARENT = {**MAIN_PARENT, 3: 1}: pelvis is fitted against the chest (both sit at the root)
fn fit_parent(r: usize) -> Option<usize> {
    if r == 3 { Some(1) } else { main_parent(r) }
}

/// row -> bind transform, in the Python dict's insertion order
pub type Rest = Vec<(usize, Tr)>;

fn rest_of(rest: &Rest, r: usize) -> Result<Tr> {
    rest.iter().find(|e| e.0 == r).map(|e| e.1).with_context(|| format!("bone {r} has no bind pose"))
}

/// The glTF parent of each joint row.
pub type Parents = BTreeMap<usize, usize>;

/// Bind (rest) pose: chain rigid transforms fitted from welded seam vertices so every blended
/// seam vertex has coinciding copies. Main joints are fitted only against their parent and only
/// accepted when the fit lands on the stored joint offset; a few models have too few seam pairs
/// for a reliable fit, and those joints use the standard bind rotation instead.
pub fn bind_pose(order: &[usize], parent: &Parents, verts: &[Vert], extras: &Extras, sk: &Skeleton) -> Result<Rest> {
    let pairs = skel::weld_pairs(verts);
    let mut rest: Rest = vec![(0, (I3, [0.0; 3]))];
    let fit = |q: usize, r: usize| pairs.get(&(q, r)).and_then(|p| skel::kabsch(p));
    let put = |rest: &mut Rest, r: usize, base: usize, rr: &M3, t: &V3| -> Result<()> {
        let (rb, tb) = rest_of(rest, base)?;
        rest.push((r, (mm(&rb, false, rr, false), np::add(&mv(&rb, false, t), &tb))));
        Ok(())
    };
    for &r in order {
        if rest.iter().any(|e| e.0 == r) {
            continue;
        }
        if r == 20 {
            // second head part: same bone as the head
            let h = rest_of(&rest, 19)?;
            rest.push((r, h));
            continue;
        }
        if let Some(q0) = fit_parent(r) {
            let expect = if r == 3 { [0.0; 3] } else { *sk.t.get(r).context("a main bone is missing")? };
            let (rp, tp) = rest_of(&rest, q0)?;
            let mut best: Option<(usize, M3, V3)> = None;
            // seams may be shared with any placed bone
            for &(q, (rq, tq)) in &rest {
                let Some(f) = fit(q, r) else { continue };
                if f.2 >= 8.0 {
                    continue;
                }
                let (rr, t) = (mm(&rq, false, &f.0, false), np::add(&mv(&rq, false, &f.1), &tq));
                // accepted only if it puts the joint on its offset
                if np::norm(&np::sub(&mv(&rp, true, &np::sub(&t, &tp)), &expect)) < 20.0
                    && best.as_ref().is_none_or(|b| f.3 > b.0)
                {
                    best = Some((f.3, rr, t));
                }
            }
            match best {
                Some((_, rr, t)) => rest.push((r, (rr, t))),
                None => put(&mut rest, r, q0, &canon_bind(q0, r).unwrap_or(I3), &expect)?,
            }
            continue;
        }
        let mut best: Option<(usize, skel::Fit)> = None;
        for &(q, _) in &rest {
            if let Some(f) = fit(q, r)
                && f.2 < 8.0
                && best.as_ref().is_none_or(|b| f.3 > b.1.3)
            {
                best = Some((q, f));
            }
        }
        if let Some((q, f)) = best {
            put(&mut rest, r, q, &f.0, &f.1)?;
        } else {
            let p = *parent.get(&r).with_context(|| format!("bone {r} has no parent"))?;
            match skel::get_extra(extras, r) {
                Some(e) => match &e.mode {
                    Mode::Fit(rr, t) => put(&mut rest, r, p, rr, t)?,
                    _ => put(&mut rest, r, p, &I3, &e.t())?,
                },
                None => put(&mut rest, r, p, &I3, sk.t.get(r).context("a bone is missing")?)?,
            }
        }
    }
    Ok(rest)
}

/// glTF joints (rows) with parents listed before children, and the parent map.
pub fn node_order(verts: &[Vert], extras: &Extras) -> Result<(Vec<usize>, Parents)> {
    let mut parent: Parents = MAIN_PARENT.into_iter().collect();
    parent.extend(extras.iter().map(|(r, e)| (*r, e.r#ref)));
    let mut nodes = BTreeSet::from([0]);
    let used = verts.iter().flatten().flat_map(|v| v.iter().map(|e| e.0));
    for r in used.chain(MAIN_PARENT.iter().map(|e| e.0)) {
        let mut r = Some(r);
        while let Some(x) = r {
            if nodes.contains(&x) || !(parent.contains_key(&x) || x == 0) {
                break;
            }
            nodes.insert(x);
            r = parent.get(&x).copied();
        }
    }
    fn visit(r: usize, parent: &Parents, order: &mut Vec<usize>, depth: usize) -> Result<()> {
        if order.contains(&r) {
            return Ok(());
        }
        if depth > 256 {
            bail!("the bone hierarchy has a cycle");
        }
        if let Some(&p) = parent.get(&r) {
            visit(p, parent, order, depth + 1)?;
        }
        order.push(r);
        Ok(())
    }
    let mut order = Vec::new();
    for &r in &nodes {
        visit(r, &parent, &mut order, 0)?;
    }
    Ok((order, parent))
}

/// Rows 2/4 hold extra sections drawn in some bone's local frame (chest / pelvis by default).
/// Where they share welded seams, pick the bone whose frame makes the seam copies coincide in
/// the bind pose (an exact test: the right frame gives ~0 mismatch).
pub fn place_secondary_sections(verts: &[Vert], extras: &mut Extras, rest: &Rest) -> bool {
    let pairs = skel::weld_pairs(verts);
    let mut changed = false;
    for t in [2, 4] {
        let Some(k) = extras.iter().position(|e| e.0 == t) else { continue };
        let mut best: Option<(f64, usize)> = None;
        for &(c, (rc, tc)) in rest {
            if [t, 0, 2, 4].contains(&c) {
                continue; // root shares the chest bind spot but animates differently
            }
            let mut d = Vec::new();
            for &(q, (rq, tq)) in rest {
                if q == t {
                    continue;
                }
                for (pq, pt) in pairs.get(&(q, t)).map_or(&[][..], |v| v) {
                    let a = np::add(&mv(&rq, false, pq), &tq);
                    let b = np::add(&mv(&rc, false, pt), &tc);
                    d.push(np::norm(&np::sub(&a, &b)));
                }
            }
            if !d.is_empty() {
                let m = np::sum(&d) / d.len() as f64;
                if best.is_none_or(|b| m < b.0) {
                    best = Some((m, c));
                }
            }
        }
        if let Some((m, c)) = best
            && m < 40.0
            && c != extras[k].1.r#ref
        {
            extras[k].1 = skel::Extra { mode: Mode::Fit(I3, [0.0; 3]), r#ref: c };
            changed = true;
        }
    }
    changed
}

fn to_gl(r: &M3, t: &V3) -> Tr {
    (mm(&mm(&C, false, r, false), false, &C, false), np::scale(SCALE, &mv(&C, false, t)))
}

fn local(wp: &Tr, wc: &Tr) -> Tr {
    let ((rp, tp), (rc, tc)) = (wp, wc);
    (mm(rp, true, rc, false), mv(rp, true, &np::sub(tc, tp)))
}

/// One character model to glb: (vertices, triangles, animations).
#[allow(clippy::too_many_arguments)]
pub fn convert(
    mid: usize,
    model: &[u8],
    tex: &[u8],
    anm: &[u8],
    t_: &Tables,
    vfs: &dyn Vfs,
    out_path: &str,
    springs: &[Option<Spring>],
    common: Option<&[u8]>,
    embu_set: Option<&[u8]>,
    own: bool,
    costume: usize,
) -> Result<(usize, usize, usize)> {
    let (raw, _) = crate::t3m::parse(model)?;
    let rows: Vec<skel::ExtraRow> = raw
        .iter()
        .map(|r| {
            let s = skel::s32;
            let lo = |v: u32| (v & 0xffff) as i64;
            [s(r[6]), s(r[3]), s(r[4]), s(r[5]), r[10] as i64, lo(r[7]), lo(r[8]), lo(r[9])]
        })
        .collect();
    let (verts, faces) = skin::build(model)?;
    let mut extras = skel::fit_extras(&rows, &verts)?;
    let sk = Skeleton::new(model, Vec::new())?;
    let (mut order, mut parent) = node_order(&verts, &extras)?;
    let mut rest = bind_pose(&order, &parent, &verts, &extras, &sk)?;
    if place_secondary_sections(&verts, &mut extras, &rest) {
        (order, parent) = node_order(&verts, &extras)?;
        rest = bind_pose(&order, &parent, &verts, &extras, &sk)?;
    }
    // the skeleton shares the extras dict in Python: it sees place_secondary_sections's change
    let sk = Skeleton { extras, ..sk };
    let h = |m: &[u8], k: usize| u32le(m, 4 * k) as usize;
    let motions = anim::find_motions(anm, h(anm, 7), h(anm, 8))?;

    // ---- mesh -------------------------------------------------------------------------
    let (a, bycl) = atlas::build_atlas(tex, costume)?;
    let (hh, ww) = (a.height() as f64, a.width() as f64);
    let joint_of: HashMap<usize, usize> = order.iter().enumerate().map(|(i, &r)| (r, i)).collect();
    let joint = |r: usize| joint_of.get(&r).copied().with_context(|| format!("bone {r} is not a joint"));
    let mut vkey: HashMap<(usize, (u32, u32)), u32> = HashMap::new();
    let (mut pos, mut jts, mut wts, mut uvs) = (Vec::<[f32; 3]>::new(), Vec::<[u8; 4]>::new(), Vec::new(), Vec::new());
    let (mut idx_one, mut idx_two) = (Vec::<u32>::new(), Vec::<u32>::new());
    for f in &faces {
        let (Some(uv), Some(cl)) = (&f.uv, f.attr) else { continue };
        let ids: Option<Vec<usize>> =
            f.ids.iter().map(|&i| i.filter(|&i| verts.get(i).is_some_and(|v| v.is_some()))).collect();
        let Some(ids) = ids else { continue };
        let tuv: Option<Vec<(u32, u32)>> =
            uv.iter().map(|&(u, v)| atlas::atlas_uv(u as u32, v as u32, cl, &bycl)).collect();
        let Some(tuv) = tuv else { continue };
        let mut corner = Vec::new();
        for (j, &i) in ids.iter().enumerate() {
            let tj = *tuv.get(j).context("a polygon has fewer UVs than corners")?;
            let key = (i, tj);
            if let Some(&v) = vkey.get(&key) {
                corner.push(v);
                continue;
            }
            let mut inf: Vec<(usize, f64)> = Vec::new();
            let mut p = [0.0; 3];
            for &(r, w, lp) in verts[i].iter().flatten() {
                match inf.iter_mut().find(|e| e.0 == r) {
                    Some(e) => e.1 += w,
                    None => inf.push((r, 0.0 + w)),
                }
                let (rr, tt) = rest_of(&rest, r)?;
                let q = np::add(&mv(&rr, false, &lp.map(|c| c as f64)), &tt);
                p = np::add(&p, &np::scale(w, &q));
            }
            inf.sort_by(|a, b| b.1.total_cmp(&a.1));
            let top = &inf[..inf.len().min(4)];
            let s = top.iter().fold(0.0, |s, e| s + e.1);
            vkey.insert(key, pos.len() as u32);
            corner.push(pos.len() as u32);
            pos.push(np::scale(SCALE, &mv(&C, false, &p)).map(|c| c as f32));
            let (mut jj, mut wv) = ([0u8; 4], [0f32; 4]);
            for (k, &(r, w)) in top.iter().enumerate() {
                jj[k] = joint(r)? as u8;
                wv[k] = (w / s) as f32;
            }
            jts.push(jj);
            wts.extend(wv);
            uvs.extend([((tj.0 as f64 + 0.5) / ww) as f32, ((tj.1 as f64 + 0.5) / hh) as f32]);
        }
        // PS1 front faces wind the other way round from glTF's counter-clockwise rule
        let tris: &[[usize; 3]] = if ids.len() == 4 { &[[0, 2, 1], [1, 2, 3]] } else { &[[0, 2, 1]] };
        let dst = if f.ds { &mut idx_two } else { &mut idx_one };
        for tri in tris {
            for &t in tri {
                dst.push(*corner.get(t).context("a polygon has too few corners")?);
            }
        }
    }
    if pos.is_empty() {
        bail!("model {mid} has no textured polygons");
    }
    let idx = [idx_one.as_slice(), idx_two.as_slice()].concat();
    let nrm = export_stages::normals(&pos, &idx);

    let mut g = Glb::default();
    let n = pos.len();
    let minmax = {
        let col = |k: usize, f: fn(f32, f32) -> f32| pos.iter().map(|p| p[k]).reduce(f).unwrap_or(0.0) as f64;
        ((0..3).map(|k| col(k, f32::min)).collect(), (0..3).map(|k| col(k, f32::max)).collect())
    };
    let mut attrs = Map::new();
    let vec3 = |g: &mut Glb, v: &[[f32; 3]], mm_| {
        g.acc(&f32_bytes(v.as_flattened()), n, 5126, "VEC3", Some(34962), mm_, false)
    };
    attrs.insert("POSITION".into(), json!(vec3(&mut g, &pos, Some(minmax))));
    attrs.insert("NORMAL".into(), json!(vec3(&mut g, &nrm, None)));
    attrs.insert("TEXCOORD_0".into(), json!(g.acc(&f32_bytes(&uvs), n, 5126, "VEC2", Some(34962), None, false)));
    attrs.insert("JOINTS_0".into(), json!(g.acc(jts.as_flattened(), n, 5121, "VEC4", Some(34962), None, false)));
    attrs.insert("WEIGHTS_0".into(), json!(g.acc(&f32_bytes(&wts), n, 5126, "VEC4", Some(34962), None, false)));
    let mut prims = Vec::new();
    for (ix, two) in [(&idx_one, false), (&idx_two, true)] {
        if !ix.is_empty() {
            let i = g.acc(&u32_bytes(ix), ix.len(), 5125, "SCALAR", Some(34963), None, false);
            prims.push(json!({"attributes": attrs, "indices": i, "material": two as u8}));
        }
    }
    let img = g.view(&export_stages::png(&a)?, None);
    let name = NAMES.get(&mid).cloned().unwrap_or(mid.to_string());
    g.j.insert("images".into(), json!([{"bufferView": img, "mimeType": "image/png", "name": "atlas"}]));
    // nearest, clamp
    g.j.insert("samplers".into(), json!([{"magFilter": 9728, "minFilter": 9728, "wrapS": 33071, "wrapT": 33071}]));
    g.j.insert("textures".into(), json!([{"source": 0, "sampler": 0}]));
    // material 0: one-sided polygons (culled from behind like on the PS1); material 1: flagged double-sided
    let mats: Vec<Value> = [("atlas", false), ("atlas_double_sided", true)]
        .iter()
        .map(|(n, two)| {
            json!({"name": n, "doubleSided": two, "alphaMode": "MASK", "alphaCutoff": 0.5,
                   "pbrMetallicRoughness": {"baseColorTexture": {"index": 0}, "metallicFactor": 0, "roughnessFactor": 1}})
        })
        .collect();
    g.j.insert("materials".into(), Value::Array(mats));
    g.j.insert("meshes".into(), json!([{"name": name, "primitives": prims}]));

    // ---- skeleton nodes (rest pose) ---------------------------------------------------
    let mut nodes = Vec::new();
    let mut rest_q: HashMap<usize, [f64; 4]> = HashMap::new();
    for &r in &order {
        let (rl, tl) = match parent.get(&r) {
            Some(&p) => local(&rest_of(&rest, p)?, &rest_of(&rest, r)?),
            None => rest_of(&rest, r)?,
        };
        let (rg, tg) = to_gl(&orthonormalize(&rl), &tl);
        let q = quat(&rg);
        rest_q.insert(r, q);
        let floats = |v: &[f64]| Value::Array(v.iter().map(|&x| f64_value(x)).collect());
        let mut nd = Map::new();
        nd.insert("name".into(), json!(row_name(r)));
        nd.insert("translation".into(), floats(&tg));
        nd.insert("rotation".into(), floats(&q));
        let kids: Vec<usize> = order.iter().filter(|c| parent.get(c) == Some(&r)).map(|c| joint_of[c]).collect();
        if !kids.is_empty() {
            nd.insert("children".into(), json!(kids));
        }
        nodes.push(Value::Object(nd));
    }
    let mut ibm = Vec::new();
    for &r in &order {
        let (rr, t) = rest_of(&rest, r)?;
        let (rr, t) = to_gl(&rr, &t);
        let mut m: M4 = [[0.0; 4], [0.0; 4], [0.0; 4], [0.0, 0.0, 0.0, 1.0]];
        for i in 0..3 {
            m[i][..3].copy_from_slice(&rr[i]);
            m[i][3] = t[i];
        }
        let x = np::inv4(&m).context("a bind matrix is singular")?;
        ibm.extend((0..16).map(|k| x[k % 4][k / 4] as f32)); // column-major
    }
    let root = joint(0)?;
    g.j.insert("nodes".into(), Value::Null); // keeps the key's place: Python sets j['nodes'] before j['skins']
    let ibm_acc = g.acc(&f32_bytes(&ibm), order.len(), 5126, "MAT4", None, None, false);
    let joints: Vec<usize> = (0..order.len()).collect();
    g.j.insert("skins".into(), json!([{"joints": joints, "skeleton": root, "inverseBindMatrices": ibm_acc}]));
    let mesh_node = nodes.len();
    nodes.push(json!({"name": "mesh", "mesh": 0, "skin": 0}));
    g.j.insert("nodes".into(), Value::Array(nodes));
    g.j.insert("scenes".into(), json!([{"name": name, "nodes": [root, mesh_node]}]));
    g.j.insert("scene".into(), json!(0));

    // ---- animations: solve every frame (angles + IK) and bake joint rotations -----------
    // Banks: the character's own motions (`motion_NNN`, record m+3) and, if given, the shared common set
    // (`common_NNN`, record 279) that every character's moves and hit reactions also use. NNN is the index in
    // anim.find_motions order; gameplay JSON (export_gamedata) refers to animations by these names.
    let mut banks: Vec<(&str, &[u8], Vec<usize>)> = Vec::new();
    if own {
        banks.push(("motion", anm, motions));
    }
    if let Some(e) = embu_set {
        // motions packed without a prefix: the ones its moves use
        let he = |k: usize| u32le(e, 4 * k) as usize;
        let set: BTreeSet<usize> =
            (0..he(0) >> 16).map(|i| (u32le(e, he(2) + 56 * i) & 0x3FFFFFFF) as usize + he(7)).collect();
        banks.push(("embu", e, set.into_iter().collect()));
    }
    if let Some(c) = common {
        banks.push(("common", c, anim::find_motions(c, h(c, 7), h(c, 8))?));
    }
    // root motion is drawn scaled by the model's size (3DMK byte +4, P+0x4EC: r * scale >> 12 at 0x80038970;
    // Kuma 90 %, King 106 %, Gon 45 %, the embu's own bears 70 %); only the root carries a translation
    let size = match model.get(4) {
        Some(&s) if s != 0 => s as f64 / 100.0,
        _ => 1.0,
    };
    let rest_map: HashMap<usize, Tr> = rest.iter().copied().collect();
    let pick = |wf: &Pose, r: usize| {
        wf.get(r).copied().flatten().or_else(|| rest_map.get(&r).copied()).context("a joint has no transform")
    };
    let mut anims = Vec::new();
    let mut times_of: HashMap<usize, usize> = HashMap::new();
    for (prefix, bank, bank_motions) in &banks {
        for (mi, &o) in bank_motions.iter().enumerate() {
            let nf = anim::frames(bank, o)?;
            if nf == 0 {
                bail!("{prefix}_{mi:03} has no frames");
            }
            let mut rot: Vec<Vec<[f64; 4]>> = vec![Vec::with_capacity(nf); order.len()];
            let mut root_t = Vec::with_capacity(3 * nf);
            let mut sim = Springs::new(&sk, springs, (mid as i64 - 71).div_euclid(4));
            // let hair/cloth settle on the first pose
            let p0 = anim::decode(t_, bank, o, 0)?;
            for _ in 0..SETTLE {
                sk.pose(&p0, Some(&mut sim))?;
            }
            for f in 0..nf {
                let wf = sk.pose(&anim::decode(t_, bank, o, f as i64)?, Some(&mut sim))?;
                for (k, &r) in order.iter().enumerate() {
                    let wc = pick(&wf, r)?;
                    let (rl, tl) = match parent.get(&r) {
                        Some(&p) => local(&pick(&wf, p)?, &wc),
                        None => wc,
                    };
                    let (rg, tg) = to_gl(&orthonormalize(&rl), &tl);
                    let mut q = quat(&rg);
                    // keep quaternions continuous
                    if let Some(last) = rot[k].last()
                        && np::dot(&q, last) < 0.0
                    {
                        q = q.map(|v| -v);
                    }
                    rot[k].push(q);
                    if r == 0 {
                        root_t.extend(tg.map(|v| (v * size) as f32));
                    }
                }
            }
            let times = match times_of.get(&nf) {
                Some(&t) => t,
                None => {
                    let ts: Vec<f32> = (0..nf).map(|i| i as f32 / FPS as f32).collect();
                    let range = (vec![ts[0] as f64], vec![ts[nf - 1] as f64]);
                    let t = g.acc(&f32_bytes(&ts), nf, 5126, "SCALAR", None, Some(range), false);
                    times_of.insert(nf, t);
                    t
                }
            };
            let (mut samplers, mut channels) = (Vec::new(), Vec::new());
            for (k, &r) in order.iter().enumerate() {
                let q: Vec<[f32; 4]> = rot[k].iter().map(|q| q.map(|v| v as f32)).collect();
                if r != 0 {
                    let ptp = (0..4)
                        .map(|c| {
                            let col = q.iter().map(|e| e[c]);
                            col.clone().fold(f32::NEG_INFINITY, f32::max) - col.fold(f32::INFINITY, f32::min)
                        })
                        .fold(f32::NEG_INFINITY, f32::max);
                    if ptp < 1e-4f32 && np::dot(&q[0].map(|v| v as f64), &rest_q[&r]).abs() > 0.99999 {
                        continue; // never leaves the rest pose: no channel needed
                    }
                }
                // rotations as normalised int16 (allowed by glTF 2.0 for rotation outputs): half the size of floats
                let q16: Vec<u8> = q
                    .iter()
                    .flatten()
                    .flat_map(|&v| ((v.clamp(-1.0, 1.0) * 32767.0).round_ties_even() as i16).to_le_bytes())
                    .collect();
                let out = g.acc(&q16, q.len(), 5122, "VEC4", None, None, true);
                samplers.push(json!({"input": times, "output": out, "interpolation": "LINEAR"}));
                let target = json!({"node": joint_of[&r], "path": "rotation"});
                channels.push(json!({"sampler": samplers.len() - 1, "target": target}));
            }
            let out = g.acc(&f32_bytes(&root_t), nf, 5126, "VEC3", None, None, false);
            samplers.push(json!({"input": times, "output": out, "interpolation": "LINEAR"}));
            let target = json!({"node": root, "path": "translation"});
            channels.push(json!({"sampler": samplers.len() - 1, "target": target}));
            anims.push(json!({"name": format!("{prefix}_{mi:03}"), "samplers": samplers, "channels": channels}));
        }
    }
    let na = anims.len();
    g.j.insert("animations".into(), Value::Array(anims));
    vfs.write(out_path, &g.to_bytes())?;
    Ok((n, idx.len() / 3, na))
}

/// The costume (selection & 3) whose model is `record` (selection -> model index table 0x800958C4: record =
/// 71 + 4 * index); it picks the costume set of a shared texture record (atlas.costume_sets).
pub fn model_costume(exe: &[u8], record: usize) -> usize {
    for s in 0..92 {
        let Some(&mi) = exe.get(0x800958C4 + s - 0x8000F800) else { break };
        if mi != 255 && 71 + 4 * mi as usize == record {
            return s & 3;
        }
    }
    0
}

/// convert's result: vertices, triangles, animations
type Counts = Result<(usize, usize, usize)>;

/// One glb to write (convert's arguments).
struct Job<'a> {
    mid: usize,
    model: &'a [u8],
    tex: Cow<'a, [u8]>,
    anm: &'a [u8],
    file: String,
    springs: Vec<Option<Spring>>,
    common: Option<&'a [u8]>,
    embu: Option<&'a [u8]>,
    own: bool,
    costume: usize,
    /// the printed line's text between the file name and the counts
    label: String,
}

pub fn main(args: &[String]) -> Result<()> {
    let (Some(src), Some(out)) = (args.first(), args.get(1)) else {
        bail!("usage: export_gltf BIN OUTDIR [ids] [--with-common] [--with-embu] [--jobs N]")
    };
    let ids = (2..args.len()).find(|&i| !args[i].starts_with("--") && args[i - 1] != "--jobs");
    let only: Option<Vec<usize>> = match ids.map(|i| &args[i]) {
        Some(a) => Some(
            a.split(',').map(|v| v.trim().parse().context("ids: comma-separated numbers")).collect::<Result<_>>()?,
        ),
        None => None,
    };
    // also bake the shared common motion set (game engine assets), and the attract embu's motions
    let with_common = args.iter().any(|a| a == "--with-common");
    let with_embu = args.iter().any(|a| a == "--with-embu");
    let ctx = Ctx { out: &StdFs(out.into()), log: &print_line, jobs: jobs_from(args) };
    run(&ctx, &StdDisc(src.into()), "", only, with_common, with_embu)
}

/// Writes the character models (`only` these records, or all) and index.json under `out`.
pub fn run(
    ctx: &Ctx,
    disc: &dyn DiscFile,
    out: &str,
    only: Option<Vec<usize>>,
    with_common: bool,
    with_embu: bool,
) -> Result<()> {
    let game = disc::Game::load(disc)?;
    let rec = &game.rec;
    let t_ = Tables::new(&game.exe)?;
    let r0 = &rec[0];
    let eo = 0x800DD818 - embu::R0_BASE; // the embu's set in overlay record 0
    let embu_set = disc::py_slice(r0, eo, eo + u32le(r0, eo + 44) as usize);
    let common = rec.get(COMMON_MOTIONS).map(Vec::as_slice).filter(|_| with_common);
    let char_of = |i: usize| (i as i64 - 71).div_euclid(4);
    let mut jobs = Vec::new();
    for (i, r) in rec.iter().enumerate() {
        if r.get(8..12) != Some(b"3DMK") || only.as_ref().is_some_and(|o| !o.contains(&i)) {
            continue;
        }
        let (Some(tex), Some(anm)) = (rec.get(i + 2), rec.get(i + 3)) else {
            bail!("model {i} has no texture or motion record")
        };
        jobs.push(Job {
            mid: i,
            model: r,
            tex: Cow::Borrowed(tex),
            anm,
            file: format!("model_{i:03}.glb"),
            springs: skel::spring_params(&game.exe, char_of(i))?,
            common,
            embu: with_embu.then_some(embu_set),
            own: true,
            costume: model_costume(&game.exe, i),
            label: format!("{:<28}", NAMES.get(&i).map_or("", String::as_str)),
        });
    }
    let models = jobs.len();
    if with_embu && only.is_none() {
        // the embu-only meshes (records 1-3): the costume's springs and texture TIMs, the embu motions only
        for (n, variant) in embu::models(rec)?.iter().enumerate() {
            let arc = embu::archive(&rec[1 + n])?;
            for &(p, normal, same) in variant.values() {
                if same {
                    continue;
                }
                let t = arc[2 * p + 1]; // as a 1-part texture record
                let tex = [&1u32.to_le_bytes()[..], &12u32.to_le_bytes(), &(t.len() as u32).to_le_bytes(), t].concat();
                jobs.push(Job {
                    mid: normal,
                    model: arc[2 * p],
                    tex: Cow::Owned(tex),
                    anm: rec.get(normal + 3).context("an embu model's motion record is missing")?,
                    file: embu::glb_name(n, p),
                    springs: skel::spring_params(&game.exe, char_of(normal))?,
                    common: None,
                    embu: Some(embu_set),
                    own: false,
                    costume: 0,
                    label: format!("embu {}", NAMES.get(&normal).map_or("", String::as_str)),
                });
            }
        }
    }
    // Python converts one model after another; here they go to ctx.jobs threads, and the lines print in order
    let results: Vec<Mutex<Option<Counts>>> = jobs.iter().map(|_| Mutex::new(None)).collect();
    let next = AtomicUsize::new(0);
    let work = || {
        loop {
            let k = next.fetch_add(1, Ordering::Relaxed);
            let Some(j) = jobs.get(k) else { break };
            let r = convert(
                j.mid,
                j.model,
                &j.tex,
                j.anm,
                &t_,
                ctx.out,
                &join(out, &j.file),
                &j.springs,
                j.common,
                j.embu,
                j.own,
                j.costume,
            )
            .with_context(|| j.file.clone());
            if let Ok(mut slot) = results[k].lock() {
                *slot = Some(r);
            }
        }
    };
    match ctx.jobs.min(jobs.len()) {
        0 | 1 => work(),
        threads => std::thread::scope(|s| {
            for _ in 0..threads {
                s.spawn(work);
            }
        }),
    }
    let ipath = join(out, "index.json");
    let mut index: BTreeMap<i64, Value> = BTreeMap::new();
    if ctx.out.exists(&ipath) {
        let old: Value = serde_json::from_str(&ctx.out.read_string(&ipath)?).context("index.json is not JSON")?;
        for e in old.as_array().into_iter().flatten() {
            index.insert(e["id"].as_i64().context("an index.json entry has no id")?, e.clone());
        }
    }
    for (k, (j, r)) in jobs.iter().zip(results).enumerate() {
        let (nv, nt, na) = r.into_inner().ok().flatten().context("a conversion thread failed")??;
        if k < models {
            let name = NAMES.get(&j.mid).cloned().unwrap_or(j.mid.to_string());
            index.insert(j.mid as i64, json!({"id": j.mid, "name": name, "file": j.file, "motions": na}));
        }
        ctx.log(&format!("{} {} verts {nv} tris {nt} animations {na}", j.file, j.label));
    }
    let mut entries: Vec<Value> = index.into_values().collect();
    entries.sort_by_key(|e| (e["name"].as_str().unwrap_or("").to_string(), e["id"].as_i64().unwrap_or(0)));
    ctx.out.write(&ipath, pyjson::indent1(&Value::Array(entries)).as_bytes())
}
