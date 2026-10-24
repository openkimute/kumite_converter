//! numpy's float64 linear algebra, the same bits on every platform, in the orders numpy on Apple's Accelerate sums
//! them where those are known (`A @ B` with B as stored and `A.T @ v`: fused multiply-adds from +0; `A @ v` and
//! dots plainly from +0, a 4-vector's dot as (x0 y0 + x2 y2) + (x1 y1 + x3 y3)); `B.T` operands, the SVD and the
//! inverse differ from Accelerate in the last bits (np_portable).
//!
//! Matrices are row-major (numpy's C order); `t` = the operand is a transposed view (`R.T`).

pub type M3 = [[f64; 3]; 3];
pub type V3 = [f64; 3];
pub type M4 = [[f64; 4]; 4];

pub const I3: M3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

pub use np_portable::{inv4, svd};

/// `op(A) @ op(B)`: op(A) is m x k, op(B) k x n; both packed row-major as stored (before the transpose).
pub fn gemm(a: &[f64], ta: bool, b: &[f64], tb: bool, m: usize, n: usize, k: usize) -> Vec<f64> {
    let mut c = vec![0.0; m * n];
    if m * n == 0 || a.len() < m * k || b.len() < k * n {
        return c;
    }
    for i in 0..m {
        for j in 0..n {
            c[i * n + j] = (0..k).fold(0.0, |s, l| {
                let x = if ta { a[l * m + i] } else { a[i * k + l] };
                let y = if tb { b[j * k + l] } else { b[l * n + j] };
                np_portable::fma(x, y, s)
            });
        }
    }
    c
}

fn flat(a: &M3) -> [f64; 9] {
    [a[0][0], a[0][1], a[0][2], a[1][0], a[1][1], a[1][2], a[2][0], a[2][1], a[2][2]]
}

/// `op(a) @ op(b)` of 3 x 3 matrices
pub fn mm(a: &M3, ta: bool, b: &M3, tb: bool) -> M3 {
    let c = gemm(&flat(a), ta, &flat(b), tb, 3, 3, 3);
    [[c[0], c[1], c[2]], [c[3], c[4], c[5]], [c[6], c[7], c[8]]]
}

/// `op(a) @ v`
pub fn mv(a: &M3, ta: bool, v: &V3) -> V3 {
    [0, 1, 2].map(|i| {
        let a = |k: usize| if ta { a[k][i] } else { a[i][k] };
        if ta {
            (0..3).fold(0.0, |s, k| np_portable::fma(a(k), v[k], s))
        } else {
            (0..3).fold(0.0, |s, k| s + a(k) * v[k])
        }
    })
}

pub fn dot(a: &[f64], b: &[f64]) -> f64 {
    match (a, b) {
        ([a0, a1, a2, a3], [b0, b1, b2, b3]) => (a0 * b0 + a2 * b2) + (a1 * b1 + a3 * b3),
        _ => a.iter().zip(b).fold(0.0, |s, (x, y)| s + x * y),
    }
}

/// `np.linalg.norm` of a vector
pub fn norm(a: &[f64]) -> f64 {
    dot(a, a).sqrt()
}

/// `np.sum` / `np.mean`'s pairwise summation of a contiguous float64 array (8 accumulators, blocks of 128).
pub fn sum(a: &[f64]) -> f64 {
    let n = a.len();
    if n < 8 {
        a.iter().fold(0.0, |s, x| s + x)
    } else if n <= 128 {
        let mut r = [a[0], a[1], a[2], a[3], a[4], a[5], a[6], a[7]];
        let mut i = 8;
        while i < n - n % 8 {
            for j in 0..8 {
                r[j] += a[i + j];
            }
            i += 8;
        }
        let mut res = ((r[0] + r[1]) + (r[2] + r[3])) + ((r[4] + r[5]) + (r[6] + r[7]));
        for x in &a[i..] {
            res += x;
        }
        res
    } else {
        let n2 = n / 2;
        let n2 = n2 - n2 % 8;
        sum(&a[..n2]) + sum(&a[n2..])
    }
}

/// (sin, cos): musl's, the same on every platform (Python's math.sin / math.cos are the platform's).
pub fn sin_cos(a: f64) -> (f64, f64) {
    (libm::sin(a), libm::cos(a))
}

pub fn add(a: &V3, b: &V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub fn sub(a: &V3, b: &V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub fn scale(k: f64, a: &V3) -> V3 {
    [k * a[0], k * a[1], k * a[2]]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bits(m: &[f64]) -> Vec<u64> {
        m.iter().map(|v| v.to_bits()).collect()
    }

    /// numpy 2.5 on Accelerate (macOS arm64): `a.T @ b`, `a @ b.T` (but one ulp), `a.T @ v`, `a @ v`, norm, mean, as
    /// python3 gives them for the same inputs.
    #[test]
    fn as_numpy() {
        let a = [[0.3, -1.7, 2.9], [1.1, 0.13, -0.77], [5.5, 2.25, 0.1]];
        let b = [[0.7, 0.2, -0.4], [-1.3, 2.2, 0.9], [0.05, -0.6, 1.9]];
        let cases: [(Vec<f64>, Vec<u64>); 4] = [
            (
                mm(&a, true, &b, false).concat(),
                vec![
                    0xbfee3d70a3d70a3f,
                    0xbfea3d70a3d70a38,
                    0x4026a3d70a3d70a4,
                    0xbff3f1a9fbe76c8b,
                    0xbff676c8b4395810,
                    0x401449ba5e353f7d,
                    0x400849ba5e353f7c,
                    0xbff2c8b439581064,
                    0xbffa9ba5e353f7cf,
                ],
            ),
            (
                mm(&a, false, &b, true).concat(),
                vec![
                    0xbff4a3d70a3d70a4,
                    0xbff851eb851eb852,
                    0x401a2e147ae147ae,
                    0x3ff1a9fbe76c8b44,
                    0xbffd645a1cac0832,
                    0xbff7c6a7ef9db22d,
                    0x40110a3d70a3d70a,
                    0xc000e147ae147ae2,
                    0xbfec51eb851eb852, // Accelerate: ...850
                ],
            ),
            (mv(&a, true, &b[0]).to_vec(), vec![0xbffc51eb851eb852, 0xc00083126e978d50, 0x3ffd604189374bc6]),
            (mv(&a, false, &b[0]).to_vec(), vec![0xbff4a3d70a3d70a4, 0x3ff1a9fbe76c8b44, 0x40110a3d70a3d70a]),
        ];
        for (k, (got, want)) in cases.iter().enumerate() {
            assert_eq!(bits(got), *want, "case {k}");
        }
        assert_eq!(norm(&[0.1, 0.2, 0.3, 0.4]).to_bits(), 0x3fe186f174f88473);
        let v: Vec<f64> = (0..37).map(|i| 0.1 * i as f64).collect();
        assert_eq!((sum(&v) / 37.0).to_bits(), 0x3ffccccccccccccc);
    }
}
