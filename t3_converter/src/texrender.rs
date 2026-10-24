//! texrender: a textured OBJ rendered from a few yaw angles side by side (orthographic, z-buffered), for previews.

use anyhow::{Context, Result};
use image::{RgbImage, RgbaImage};
use std::path::Path;

type Obj = (Vec<[f64; 3]>, Vec<[f64; 2]>, Vec<Vec<(i64, i64)>>);

pub fn load_obj(p: &Path) -> Result<Obj> {
    let text = std::fs::read_to_string(p).with_context(|| format!("cannot read {}", p.display()))?;
    let (mut v, mut t, mut f) = (Vec::new(), Vec::new(), Vec::new());
    let num = |s: Option<&str>| -> Result<f64> { s.context("a short OBJ line")?.parse().context("a bad OBJ number") };
    for l in text.lines() {
        let s: Vec<&str> = l.split_whitespace().collect();
        match s.first() {
            Some(&"v") => v.push([num(s.get(1).copied())?, num(s.get(2).copied())?, num(s.get(3).copied())?]),
            Some(&"vt") => t.push([num(s.get(1).copied())?, num(s.get(2).copied())?]),
            Some(&"f") => f.push(
                s[1..]
                    .iter()
                    .map(|c| {
                        let mut q = c.split('/').map(|q| if q.is_empty() { Ok(0) } else { q.parse() });
                        Ok((q.next().unwrap_or(Ok(0))?, q.next().unwrap_or(Ok(0))?))
                    })
                    .collect::<Result<Vec<_>, std::num::ParseIntError>>()
                    .context("a bad OBJ face")?,
            ),
            _ => {}
        }
    }
    Ok((v, t, f))
}

pub fn render(obj: &Path, png: &Path, out: &Path) -> Result<()> {
    let (yaw_list, size) = ([0.0f64, 90.0, 180.0, 35.0], 420);
    let (v, tc, faces) = load_obj(obj)?;
    let tex: RgbaImage = image::open(png).with_context(|| format!("cannot read {}", png.display()))?.to_rgba8();
    let (th, tw) = (tex.height() as i64, tex.width() as i64);
    if th == 0 || tw == 0 {
        anyhow::bail!("{} is empty", png.display());
    }
    let fsize = size as f64;
    let mut panels = Vec::new();
    for yaw in yaw_list {
        let a = yaw.to_radians();
        let r = [[a.cos(), 0.0, a.sin()], [0.0, 1.0, 0.0], [-a.sin(), 0.0, a.cos()]];
        // P = V @ R.T, fused as numpy's BLAS (Accelerate) computes it
        let p: Vec<[f64; 3]> =
            v.iter().map(|q| r.map(|row| q[2].mul_add(row[2], q[1].mul_add(row[1], q[0] * row[0])))).collect();
        let (mut mn, mut mx) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
        for q in &p {
            for c in 0..3 {
                mn[c] = mn[c].min(q[c]);
                mx[c] = mx[c].max(q[c]);
            }
        }
        let sc = (fsize * 0.9) / (mx[1] - mn[1]).max(mx[0] - mn[0]);
        let sx: Vec<f64> = p.iter().map(|q| (q[0] - (mn[0] + mx[0]) / 2.0) * sc + fsize / 2.0).collect();
        let sy: Vec<f64> = p.iter().map(|q| fsize / 2.0 - (q[1] - (mn[1] + mx[1]) / 2.0) * sc).collect();
        let mut img = vec![[235u8; 3]; size * size];
        let mut zb = vec![-1e9f64; size * size];
        for f in &faces {
            let tris: Vec<Vec<(i64, i64)>> =
                if f.len() == 4 { vec![vec![f[0], f[1], f[2]], vec![f[0], f[2], f[3]]] } else { vec![f.clone()] };
            for tri in tris {
                let get = |a: &[f64], i: i64| a.get((i - 1) as usize).copied().context("an OBJ vertex is out of range");
                let mut x = [0.0; 3];
                let mut y = [0.0; 3];
                let mut z = [0.0; 3];
                for c in 0..3 {
                    let vi = tri.get(c).context("an OBJ face has under 3 corners")?.0;
                    x[c] = get(&sx, vi)?;
                    y[c] = get(&sy, vi)?;
                    z[c] = p.get((vi - 1) as usize).context("an OBJ vertex is out of range")?[2];
                }
                let x0 = 0f64.max(x.iter().copied().fold(f64::INFINITY, f64::min).floor()) as i64;
                let x1 = (size as f64 - 1.0).min(x.iter().copied().fold(f64::NEG_INFINITY, f64::max).ceil()) as i64;
                let y0 = 0f64.max(y.iter().copied().fold(f64::INFINITY, f64::min).floor()) as i64;
                let y1 = (size as f64 - 1.0).min(y.iter().copied().fold(f64::NEG_INFINITY, f64::max).ceil()) as i64;
                if x1 < x0 || y1 < y0 {
                    continue;
                }
                let d = (y[1] - y[2]) * (x[0] - x[2]) + (x[2] - x[1]) * (y[0] - y[2]);
                if d.abs() < 1e-9 {
                    continue;
                }
                let ti: Vec<i64> = tri.iter().map(|c| c.1 - 1).collect();
                let uv = if ti.iter().any(|&t| t < 0) {
                    None
                } else {
                    let mut uv = [[0.0; 2]; 3];
                    for c in 0..3 {
                        uv[c] = *tc.get(ti[c] as usize).context("an OBJ texture coordinate is out of range")?;
                    }
                    Some(uv)
                };
                for py in y0..=y1 {
                    for px in x0..=x1 {
                        let (gx, gy) = (px as f64 + 0.5, py as f64 + 0.5);
                        let w0 = ((y[1] - y[2]) * (gx - x[2]) + (x[2] - x[1]) * (gy - y[2])) / d;
                        let w1 = ((y[2] - y[0]) * (gx - x[2]) + (x[0] - x[2]) * (gy - y[2])) / d;
                        let w2 = 1.0 - w0 - w1;
                        if w0 < -1e-6 || w1 < -1e-6 || w2 < -1e-6 {
                            continue;
                        }
                        let zz = w0 * z[0] + w1 * z[1] + w2 * z[2];
                        let at = py as usize * size + px as usize;
                        if zz <= zb[at] {
                            continue;
                        }
                        zb[at] = zz;
                        img[at] = match uv {
                            None => [255, 0, 255],
                            Some(uv) => {
                                let u = w0 * uv[0][0] + w1 * uv[1][0] + w2 * uv[2][0];
                                let v = w0 * uv[0][1] + w1 * uv[1][1] + w2 * uv[2][1];
                                let tx = ((u * tw as f64) as i64).clamp(0, tw - 1);
                                let ty = (((1.0 - v) * th as f64) as i64).clamp(0, th - 1);
                                let c = tex.get_pixel(tx as u32, ty as u32).0;
                                [c[0], c[1], c[2]]
                            }
                        };
                    }
                }
            }
        }
        panels.push(img);
    }
    let mut out_img = RgbImage::new((size * panels.len()) as u32, size as u32);
    for (k, img) in panels.iter().enumerate() {
        for (i, c) in img.iter().enumerate() {
            out_img.put_pixel((k * size + i % size) as u32, (i / size) as u32, image::Rgb(*c));
        }
    }
    out_img.save(out).with_context(|| format!("cannot write {}", out.display()))
}

pub fn main(args: &[String]) -> Result<()> {
    let [obj, png, out, ..] = args else { anyhow::bail!("usage: texrender OBJ PNG OUT") };
    render(Path::new(obj), Path::new(png), Path::new(out))
}
