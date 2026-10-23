//! `np.linalg.svd` and `np.linalg.inv` of the converters' small float64 matrices, the same bits on every platform:
//! only + - * / and sqrt in a fixed order (numpy's LAPACK and nalgebra go through the platform's BLAS or `hypot`).
//! Holds no game knowledge: both converters use it.
#![allow(clippy::needless_range_loop)] // matrix indices

pub type M3 = [[f64; 3]; 3];
pub type M4 = [[f64; 4]; 4];

/// `U, s, Vt = np.linalg.svd(a)`, the singular values descending. One-sided Jacobi: rotate pairs of a's
/// columns until they are orthogonal; their lengths are s, their directions U, the rotations V.
pub fn svd(a: &M3) -> (M3, [f64; 3], M3) {
    let mut w = *a;
    let mut v = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    // a column this short is rounding noise: a zero singular value, its direction not to be trusted
    let tol = f64::EPSILON * a.iter().flatten().fold(0.0, |s, x| s + x * x).sqrt();
    for _ in 0..60 {
        let mut done = true;
        for (p, q) in [(0, 1), (0, 2), (1, 2)] {
            let (mut al, mut be, mut ga) = (0.0, 0.0, 0.0);
            for r in &w {
                al += r[p] * r[p];
                be += r[q] * r[q];
                ga += r[p] * r[q];
            }
            let (na, nb) = (al.sqrt(), be.sqrt());
            if ga == 0.0 || na <= tol || nb <= tol || ga.abs() <= f64::EPSILON * na * nb {
                continue;
            }
            done = false;
            let ze = (be - al) / (2.0 * ga);
            let t = ze.signum() / (ze.abs() + (1.0 + ze * ze).sqrt());
            let c = 1.0 / (1.0 + t * t).sqrt();
            let s = c * t;
            for m in [&mut w, &mut v] {
                for r in m.iter_mut() {
                    let (x, y) = (r[p], r[q]);
                    r[p] = c * x - s * y;
                    r[q] = s * x + c * y;
                }
            }
        }
        if done {
            break;
        }
    }
    let len = |k: usize| (w[0][k] * w[0][k] + w[1][k] * w[1][k] + w[2][k] * w[2][k]).sqrt();
    let mut ord = [0, 1, 2];
    let s = [len(0), len(1), len(2)];
    for i in 1..3 {
        for j in (1..=i).rev() {
            if s[ord[j]] > s[ord[j - 1]] {
                ord.swap(j, j - 1);
            }
        }
    }
    // U's columns (as rows here) and Vt's rows; a zero singular value's U column completes an orthonormal basis
    let mut u = [[0.0; 3]; 3];
    let mut rank = 0;
    for (k, &c) in ord.iter().enumerate() {
        if s[c] > tol {
            u[k] = [w[0][c] / s[c], w[1][c] / s[c], w[2][c] / s[c]];
            rank += 1;
        }
    }
    let cross =
        |a: &[f64; 3], b: &[f64; 3]| [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
    if rank == 0 {
        u = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    } else if rank == 1 {
        // the axis u0 leans on least, made orthogonal to u0
        let i = (0..3).fold(0, |m, i| if u[0][i].abs() < u[0][m].abs() { i } else { m });
        let mut e = u[0].map(|x| -u[0][i] * x);
        e[i] += 1.0;
        let n = (e[0] * e[0] + e[1] * e[1] + e[2] * e[2]).sqrt();
        u[1] = e.map(|x| x / n);
    }
    if rank < 3 {
        u[2] = cross(&u[0], &u[1]);
    }
    let ut = [0, 1, 2].map(|i| [u[0][i], u[1][i], u[2][i]]);
    let vt = ord.map(|c| [v[0][c], v[1][c], v[2][c]]);
    (ut, ord.map(|c| s[c]), vt)
}

/// `np.linalg.inv` (None when singular: numpy raises): LU with partial pivoting, then solving for each column of the
/// identity (exact for rigid transforms with ±1 / 0 rotations).
pub fn inv4(m: &M4) -> Option<M4> {
    let mut a = *m;
    let mut perm = [0usize, 1, 2, 3];
    for j in 0..4 {
        let p = (j..4).fold(j, |p, i| if a[i][j].abs() > a[p][j].abs() { i } else { p });
        a.swap(j, p);
        perm.swap(j, p);
        if a[j][j] == 0.0 {
            return None;
        }
        for i in j + 1..4 {
            a[i][j] /= a[j][j];
            for k in j + 1..4 {
                a[i][k] -= a[i][j] * a[j][k];
            }
        }
    }
    let mut x = [[0.0; 4]; 4];
    for c in 0..4 {
        let mut y = [0.0; 4];
        for i in 0..4 {
            y[i] = if perm[i] == c { 1.0 } else { 0.0 };
            for k in 0..i {
                y[i] -= a[i][k] * y[k];
            }
        }
        for i in (0..4).rev() {
            for k in i + 1..4 {
                y[i] -= a[i][k] * y[k];
            }
            y[i] /= a[i][i];
        }
        for i in 0..4 {
            x[i][c] = y[i];
        }
    }
    Some(x)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bits(m: &[[f64; 3]]) -> Vec<u64> {
        m.concat().iter().map(|v| v.to_bits()).collect()
    }

    fn mm(a: &M3, b: &M3) -> M3 {
        [0, 1, 2].map(|i| [0, 1, 2].map(|j| a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j]))
    }

    /// U diag(s) Vt gives a back, U and Vt are orthonormal, s descends; and the bits, which every platform must give.
    #[test]
    fn svd_golden() {
        let cases: [M3; 4] = [
            [[0.3, -1.7, 2.9], [1.1, 0.13, -0.77], [5.5, 2.25, 0.1]],
            [[1.0, 2.0, 3.0], [2.0, 4.0, 6.0], [-1.0, 0.5, 0.25]], // rank 2
            [[1.0, 2.0, 3.0], [2.0, 4.0, 6.0], [-3.0, -6.0, -9.0]], // rank 1
            [[0.0; 3]; 3],
        ];
        for a in &cases {
            let (u, _, vt) = svd(a);
            let ut = [0, 1, 2].map(|i| [u[0][i], u[1][i], u[2][i]]);
            let v = [0, 1, 2].map(|i| [vt[0][i], vt[1][i], vt[2][i]]);
            let s = mm(&mm(&ut, a), &v);
            for (x, y) in [(mm(&ut, &u), 1.0), (mm(&vt, &v), 1.0), (s, 0.0)] {
                for i in 0..3 {
                    for j in 0..3 {
                        let want = if i == j { if y == 1.0 { 1.0 } else { x[i][i] } } else { 0.0 };
                        assert!((x[i][j] - want).abs() < 1e-12, "{a:?}: {x:?}");
                    }
                }
            }
            assert!(s[0][0] >= s[1][1] - 1e-12 && s[1][1] >= s[2][2] - 1e-12, "{s:?}");
        }
        let (u, _, vt) = svd(&cases[0]);
        // U @ Vt (numpy on Accelerate differs by a few ulps)
        let want = [
            0x3fc8e2ab55e1c1c2,
            0xbfe2937ef7bd5034,
            0x3fe94d76a57c7d47,
            0x3fdcc421b54bf9ab,
            0xbfe53d8d7158b90f,
            0xbfe3215fe5186e50,
            0x3febe674869e2899,
            0x3fde2f21f6214b30,
            0x3fc0e135a1b87fc6,
        ];
        assert_eq!(bits(&mm(&u, &vt)), want);
    }

    #[test]
    fn inv4_golden() {
        let m = [[0.3, -1.7, 2.9, 0.0], [1.1, 0.13, -0.77, 0.0], [5.5, 2.25, 0.1, 0.0], [0.0, 0.0, 0.0, 1.0]];
        let x = inv4(&m).map(|x| x.concat().iter().map(|v| v.to_bits()).collect::<Vec<_>>());
        let want = [
            0x3fc12af34eaebf15,
            0x3fe0764afbc611b8,
            0x3fb255518e782be6,
            0,
            0xbfd55e153c1775a3,
            0xbff392922010c67d,
            0x3fd0d2d3263d3ece,
            0x8000000000000000,
            0x3fc14f75a894a01c,
            0xbfe8a66c749aea8b,
            0x3fc2c69f79533e89,
            0x8000000000000000,
            0,
            0,
            0,
            0x3ff0000000000000,
        ];
        assert_eq!(x, Some(want.to_vec()));
        assert_eq!(inv4(&[[1.0, 2.0, 0.0, 0.0], [2.0, 4.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0; 4]]), None);
        // a rigid transform's inverse is exact
        let r = [[0.0, -1.0, 0.0, 0.25], [1.0, 0.0, 0.0, -1.5], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]];
        let x = inv4(&r);
        assert_eq!(x, Some([[0.0, 1.0, 0.0, 1.5], [-1.0, 0.0, 0.0, 0.25], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]]));
    }
}
