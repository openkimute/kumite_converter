//! What the Python tools take from Pillow: saving RGBA pictures as PNG (the pixels as Pillow's; the compressed bytes
//! differ, Pillow's zlib being zlib-ng) and `Image.resize(size, LANCZOS)` of an RGBA picture (Pillow's Resample.c:
//! premultiplied alpha, 8-bit fixed-point coefficients, a horizontal then a vertical pass).

use crate::ps1::Rgba;
use anyhow::Result;
use conv_io::Vfs;

/// `Image.fromarray(a, 'RGBA').save(f, 'PNG')`
pub fn png_bytes(im: &Rgba) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut e = png::Encoder::new(&mut out, im.w as u32, im.h as u32);
    e.set_color(png::ColorType::Rgba);
    e.set_depth(png::BitDepth::Eight);
    let mut w = e.write_header()?;
    w.write_image_data(&im.px)?;
    w.finish()?;
    Ok(out)
}

pub fn save_png(im: &Rgba, out: &dyn Vfs, path: &str) -> Result<()> {
    out.write(path, &png_bytes(im)?)
}

const PRECISION_BITS: u32 = 32 - 8 - 2;

fn sinc(x: f64) -> f64 {
    if x == 0.0 {
        return 1.0;
    }
    let x = x * std::f64::consts::PI;
    libm::sin(x) / x
}

fn lanczos(x: f64) -> f64 {
    if (-3.0..3.0).contains(&x) { sinc(x) * sinc(x / 3.0) } else { 0.0 }
}

/// precompute_coeffs + normalize_coeffs_8bpc: per output pixel its first input pixel, count and fixed-point weights.
fn coeffs(in_size: usize, out_size: usize) -> (usize, Vec<(usize, usize)>, Vec<i32>) {
    let scale = in_size as f64 / out_size as f64;
    let filterscale = scale.max(1.0);
    let support = 3.0 * filterscale;
    let ksize = support.ceil() as usize * 2 + 1;
    let mut bounds = Vec::with_capacity(out_size);
    let mut kk = vec![0i32; out_size * ksize];
    for xx in 0..out_size {
        let center = (xx as f64 + 0.5) * scale;
        let ss = 1.0 / filterscale;
        let xmin = ((center - support + 0.5) as i64).max(0) as usize;
        let xmax = ((center + support + 0.5) as i64).min(in_size as i64) as usize - xmin;
        let mut k: Vec<f64> = (0..xmax).map(|x| lanczos((x as f64 + xmin as f64 - center + 0.5) * ss)).collect();
        let ww: f64 = k.iter().sum();
        if ww != 0.0 {
            for w in &mut k {
                *w /= ww;
            }
        }
        for (x, w) in k.iter().enumerate() {
            let v = w * (1u32 << PRECISION_BITS) as f64;
            kk[xx * ksize + x] = if *w < 0.0 { (-0.5 + v) as i32 } else { (0.5 + v) as i32 };
        }
        bounds.push((xmin, xmax));
    }
    (ksize, bounds, kk)
}

fn clip8(v: i32) -> u8 {
    (v >> PRECISION_BITS).clamp(0, 255) as u8
}

/// One pass: out[i] = sum of in[first + j] * k[j], over `n` lines of `len` pixels (`at(line, i)` the pixel's byte
/// offset in the source).
fn pass(
    src: &[u8],
    out_len: usize,
    lines: usize,
    ksize: usize,
    bounds: &[(usize, usize)],
    kk: &[i32],
    at: impl Fn(usize, usize) -> usize,
) -> Vec<u8> {
    let mut out = vec![0u8; out_len * lines * 4];
    for line in 0..lines {
        for (i, &(min, n)) in bounds.iter().enumerate() {
            let k = &kk[i * ksize..i * ksize + n];
            let mut ss = [1i32 << (PRECISION_BITS - 1); 4];
            for (j, w) in k.iter().enumerate() {
                let o = at(line, min + j);
                for c in 0..4 {
                    ss[c] = ss[c].wrapping_add((src[o + c] as i32).wrapping_mul(*w));
                }
            }
            let o = (line * out_len + i) * 4;
            for c in 0..4 {
                out[o + c] = clip8(ss[c]);
            }
        }
    }
    out
}

/// `Image.resize((w, h), Image.LANCZOS)` of an RGBA picture.
pub fn resize_lanczos(im: &Rgba, w: usize, h: usize) -> Rgba {
    if (im.w, im.h) == (w, h) {
        return im.clone();
    }
    // RGBA -> RGBa (MULDIV255)
    let mut px = im.px.clone();
    for p in px.as_chunks_mut::<4>().0 {
        let a = p[3] as u32;
        for c in &mut p[..3] {
            let t = *c as u32 * a + 128;
            *c = (((t >> 8) + t) >> 8) as u8;
        }
    }
    let (kx, bx, kkx) = coeffs(im.w, w);
    let (ky, by, kky) = coeffs(im.h, h);
    let (mut cur, mut cw) = (px, im.w);
    let mut by = by;
    if w != im.w {
        // the rows the vertical pass uses
        let first = by[0].0;
        let last = by[h - 1].0 + by[h - 1].1;
        for b in &mut by {
            b.0 -= first;
        }
        cur = pass(&cur, w, last - first, kx, &bx, &kkx, |y, x| ((y + first) * cw + x) * 4);
        cw = w;
    }
    if h != im.h {
        let mut out = vec![0u8; cw * h * 4];
        for x in 0..cw {
            let col = pass(&cur, h, 1, ky, &by, &kky, |_, y| (y * cw + x) * 4);
            for y in 0..h {
                out[(y * cw + x) * 4..(y * cw + x) * 4 + 4].copy_from_slice(&col[y * 4..y * 4 + 4]);
            }
        }
        cur = out;
    }
    // RGBa -> RGBA
    for p in cur.as_chunks_mut::<4>().0 {
        let a = p[3] as u32;
        if a != 0 && a != 255 {
            for c in &mut p[..3] {
                *c = ((255 * *c as u32) / a).min(255) as u8;
            }
        }
    }
    Rgba { w, h, px: cur }
}
