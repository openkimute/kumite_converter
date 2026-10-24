//! qa_glb: QA for exported .glb files: skins each model with plain glTF math at frames sampled from ~12
//! animations and reports triangles that stretch more than 3x their bind-pose size.
//!
//! usage: t3_convert qa_glb DIR [071,227,...]
//!
//! (pygltflib's reader replaced by a small one; the int16 rotations the converter writes are read as normalised,
//! where the Python script stops with a KeyError.)

use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

type M4 = [[f64; 4]; 4];

fn qm(q: &[f64]) -> [[f64; 3]; 3] {
    let (x, y, z, w) = (q[0], q[1], q[2], q[3]);
    [
        [1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y - z * w), 2.0 * (x * z + y * w)],
        [2.0 * (x * y + z * w), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z - x * w)],
        [2.0 * (x * z - y * w), 2.0 * (y * z + x * w), 1.0 - 2.0 * (x * x + y * y)],
    ]
}

fn mul(a: &M4, b: &M4) -> M4 {
    std::array::from_fn(|i| std::array::from_fn(|j| (0..4).map(|k| a[i][k] * b[k][j]).sum()))
}

struct Gltf {
    j: Value,
    blob: Vec<u8>,
}

impl Gltf {
    fn load(path: &Path) -> Result<Gltf> {
        let d = std::fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
        let u = |o: usize| d.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize);
        if u(0) != Some(0x46546C67) {
            bail!("{} is not a glb", path.display());
        }
        let jl = u(12).context("a glb without its JSON")?;
        let j = serde_json::from_slice(d.get(20..20 + jl).context("a glb's JSON runs past it")?)?;
        let bl = u(20 + jl).unwrap_or(0);
        let blob = d.get(28 + jl..28 + jl + bl).unwrap_or(&[]).to_vec();
        Ok(Gltf { j, blob })
    }

    /// An accessor's elements as float rows (normalised integers scaled as glTF says).
    fn acc(&self, i: &Value) -> Result<Vec<Vec<f64>>> {
        let a = &self.j["accessors"][i.as_u64().context("a bad accessor index")? as usize];
        let bv = &self.j["bufferViews"][a["bufferView"].as_u64().context("an accessor without a view")? as usize];
        let nc = match a["type"].as_str() {
            Some("SCALAR") => 1,
            Some("VEC2") => 2,
            Some("VEC3") => 3,
            Some("VEC4") => 4,
            Some("MAT4") => 16,
            _ => bail!("an accessor of an unknown type"),
        };
        let (ct, norm) = (a["componentType"].as_u64().unwrap_or(0), a["normalized"].as_bool().unwrap_or(false));
        let size = match ct {
            5126 | 5125 => 4,
            5122 => 2,
            5121 => 1,
            _ => bail!("an accessor of component type {ct}"),
        };
        let (count, at) = (a["count"].as_u64().unwrap_or(0) as usize, bv["byteOffset"].as_u64().unwrap_or(0) as usize);
        let b = self.blob.get(at..at + count * nc * size).context("an accessor runs past the buffer")?;
        let v = |c: &[u8]| match ct {
            5126 => f32::from_le_bytes([c[0], c[1], c[2], c[3]]) as f64,
            5125 => u32::from_le_bytes([c[0], c[1], c[2], c[3]]) as f64,
            5122 if norm => (i16::from_le_bytes([c[0], c[1]]) as f64 / 32767.0).max(-1.0),
            5122 => i16::from_le_bytes([c[0], c[1]]) as f64,
            _ => c[0] as f64,
        };
        Ok(b.chunks(nc * size).map(|e| e.chunks(size).map(v).collect()).collect())
    }
}

fn qa(path: &Path) -> Result<()> {
    let g = Gltf::load(path)?;
    let sk = &g.j["skins"][0];
    let ibm: Vec<M4> = g
        .acc(&sk["inverseBindMatrices"])?
        .iter()
        .map(|m| std::array::from_fn(|i| std::array::from_fn(|j| m[4 * j + i])))
        .collect();
    let prims = g.j["meshes"][0]["primitives"].as_array().context("a glb without a mesh")?;
    let at = &prims.first().context("a mesh without primitives")?["attributes"];
    let p = g.acc(&at["POSITION"])?;
    let jn = g.acc(&at["JOINTS_0"])?;
    let wt = g.acc(&at["WEIGHTS_0"])?;
    let mut tris = Vec::new();
    for pr in prims {
        for t in g.acc(&pr["indices"])?.as_chunks::<3>().0 {
            tris.push([t[0][0] as usize, t[1][0] as usize, t[2][0] as usize]);
        }
    }
    if tris.iter().flatten().any(|&i| i >= p.len()) {
        bail!("an index past the vertices");
    }
    let nodes = g.j["nodes"].as_array().context("a glb without nodes")?;
    let mut par = HashMap::new();
    for (i, n) in nodes.iter().enumerate() {
        for c in n["children"].as_array().into_iter().flatten() {
            par.insert(c.as_u64().unwrap_or(0) as usize, i);
        }
    }
    let edge_max = |x: &[[f64; 3]]| -> Vec<f64> {
        let d = |a: usize, b: usize| {
            ((x[a][0] - x[b][0]).powi(2) + (x[a][1] - x[b][1]).powi(2) + (x[a][2] - x[b][2]).powi(2)).sqrt()
        };
        tris.iter().map(|t| d(t[0], t[1]).max(d(t[1], t[2])).max(d(t[0], t[2]))).collect()
    };
    let p3: Vec<[f64; 3]> = p.iter().map(|v| [v[0], v[1], v[2]]).collect();
    let bind = edge_max(&p3);
    let mut worst = vec![0.0f64; tris.len()];
    let mut place: Vec<Option<(String, usize)>> = vec![None; tris.len()];
    let anims = g.j["animations"].as_array().cloned().unwrap_or_default();
    let joints: Vec<usize> =
        sk["joints"].as_array().into_iter().flatten().map(|v| v.as_u64().unwrap_or(0) as usize).collect();
    for an in anims.iter().step_by((anims.len() / 12).max(1)) {
        let mut outs: HashMap<(usize, String), Vec<Vec<f64>>> = HashMap::new();
        let samplers = an["samplers"].as_array().context("an animation without samplers")?;
        for ch in an["channels"].as_array().into_iter().flatten() {
            let s = &samplers[ch["sampler"].as_u64().unwrap_or(0) as usize];
            let key = (
                ch["target"]["node"].as_u64().unwrap_or(0) as usize,
                ch["target"]["path"].as_str().unwrap_or("").into(),
            );
            outs.insert(key, g.acc(&s["output"])?);
        }
        let nf = g.acc(&samplers.first().context("no samplers")?["input"])?.len();
        for f in [0, nf / 3, 2 * nf / 3, nf.saturating_sub(1)] {
            let mut world: HashMap<usize, M4> = HashMap::new();
            fn wd(
                i: usize,
                f: usize,
                nodes: &[Value],
                par: &HashMap<usize, usize>,
                outs: &HashMap<(usize, String), Vec<Vec<f64>>>,
                world: &mut HashMap<usize, M4>,
                depth: usize,
            ) -> Result<M4> {
                if let Some(m) = world.get(&i) {
                    return Ok(*m);
                }
                if depth > 256 {
                    bail!("the node hierarchy has a cycle");
                }
                let n = &nodes[i];
                let fl = |v: &Value, d: &[f64]| -> Vec<f64> {
                    v.as_array().map_or(d.to_vec(), |a| a.iter().map(|x| x.as_f64().unwrap_or(0.0)).collect())
                };
                let q = match outs.get(&(i, "rotation".into())) {
                    Some(q) => q.get(f).context("a frame past the rotations")?.clone(),
                    None => fl(&n["rotation"], &[0.0, 0.0, 0.0, 1.0]),
                };
                let t = match outs.get(&(i, "translation".into())) {
                    Some(t) => t.get(f).context("a frame past the translations")?.clone(),
                    None => fl(&n["translation"], &[0.0, 0.0, 0.0]),
                };
                let r = qm(&q);
                let mut m: M4 = [[0.0; 4], [0.0; 4], [0.0; 4], [0.0, 0.0, 0.0, 1.0]];
                for k in 0..3 {
                    m[k][..3].copy_from_slice(&r[k]);
                    m[k][3] = t[k];
                }
                if let Some(&pp) = par.get(&i) {
                    m = mul(&wd(pp, f, nodes, par, outs, world, depth + 1)?, &m);
                }
                world.insert(i, m);
                Ok(m)
            }
            let jm: Vec<M4> = joints
                .iter()
                .zip(&ibm)
                .map(|(&j, ib)| Ok(mul(&wd(j, f, nodes, &par, &outs, &mut world, 0)?, ib)))
                .collect::<Result<_>>()?;
            let x: Vec<[f64; 3]> = p3
                .iter()
                .enumerate()
                .map(|(v, pv)| {
                    let mut acc = [0.0; 3];
                    for k in 0..4 {
                        let m = &jm[(jn[v][k] as usize).min(jm.len() - 1)];
                        for (r, a) in acc.iter_mut().enumerate() {
                            *a += wt[v][k] * (m[r][0] * pv[0] + m[r][1] * pv[1] + m[r][2] * pv[2] + m[r][3]);
                        }
                    }
                    acc
                })
                .collect();
            for (t, e) in edge_max(&x).iter().enumerate() {
                let r = e / bind[t].max(1e-3);
                if r > worst[t] {
                    worst[t] = r;
                    place[t] = Some((an["name"].as_str().unwrap_or("").into(), f));
                }
            }
        }
    }
    let bad = worst.iter().zip(&bind).filter(|(w, b)| **w > 3.0 && **b > 0.02).count();
    let names: Vec<&str> = joints.iter().map(|&j| nodes[j]["name"].as_str().unwrap_or("")).collect();
    let mut by: Vec<usize> = (0..tris.len()).collect();
    by.sort_by(|a, b| worst[*b].total_cmp(&worst[*a]));
    for &t in by.iter().take(4) {
        let mut js: Vec<&str> = tris[t]
            .iter()
            .flat_map(|&v| (0..4).map(move |k| (v, k)))
            .filter(|&(v, k)| wt[v][k] > 0.0)
            .map(|(v, k)| names.get(jn[v][k] as usize).copied().unwrap_or(""))
            .collect();
        js.sort();
        js.dedup();
        let w = place[t].as_ref().map_or("None".into(), |(n, f)| format!("('{n}', {f})"));
        let js = js.iter().map(|j| format!("'{j}'")).collect::<Vec<_>>().join(", ");
        println!(
            "   worst tri {t} {:?} x bind {:?} m joints [{js}] at {w}",
            (worst[t] * 10.0).round() / 10.0,
            (bind[t] * 1000.0).round() / 1000.0
        );
    }
    let max = worst.iter().copied().fold(0.0, f64::max);
    let name = path.file_name().map_or(String::new(), |n| n.to_string_lossy().into());
    println!("{name}: triangles stretched >3x in sampled frames: {bad:3} / {}  (max {max:.1}x)", tris.len());
    Ok(())
}

pub fn main(args: &[String]) -> Result<()> {
    let dir = args.first().context("usage: qa_glb DIR [071,227,...]")?;
    let paths: Vec<PathBuf> = match args.get(1) {
        Some(ids) => ids.split(',').map(|m| Path::new(dir).join(format!("model_{m}.glb"))).collect(),
        None => {
            let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
                .with_context(|| format!("cannot list {dir}"))?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().is_some_and(|e| e == "glb"))
                .collect();
            v.sort();
            v
        }
    };
    for p in paths {
        qa(&p)?;
    }
    Ok(())
}
