//! numpy's small-matrix arithmetic in the orders numpy on Apple's Accelerate sums it, the same bits on every platform:
//! `A @ B` of 3 x 3 matrices sums with fused multiply-adds from +0, `A @ v` plainly from +0, 4 x 4 products as two
//! fused pairs (k 0, 2 and 1, 3), a 4-vector's dot (and norm) as (x0² + x2²) + (x1² + x3²). Sums from +0 never give
//! -0.0, which shows in the written floats. `np.linalg.svd` and `np.linalg.inv` are np_portable's (Accelerate's
//! LAPACK differs in the last bits).

use crate::ps1::{M3, V3};

pub type M4 = [[f64; 4]; 4];

pub const I3: M3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

pub use np_portable::{inv4, svd};

/// (sin, cos): musl's, the same on every platform (Python's math.sin / math.cos are the platform's).
pub fn sin_cos(a: f64) -> (f64, f64) {
    (libm::sin(a), libm::cos(a))
}

pub fn mm3(a: &M3, b: &M3) -> M3 {
    let mut r = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            r[i][j] = (0..3).fold(0.0, |s, k| a[i][k].mul_add(b[k][j], s));
        }
    }
    r
}

pub fn mv3(a: &M3, v: &V3) -> V3 {
    [0, 1, 2].map(|i| 0.0 + a[i][0] * v[0] + a[i][1] * v[1] + a[i][2] * v[2])
}

pub fn t3(a: &M3) -> M3 {
    [0, 1, 2].map(|i| [a[0][i], a[1][i], a[2][i]])
}

/// `a @ b` of 4 x 4 matrices: two fused pairs.
pub fn mm4(a: &M4, b: &M4) -> M4 {
    let mut r = [[0.0; 4]; 4];
    for i in 0..4 {
        for j in 0..4 {
            r[i][j] = a[i][2].mul_add(b[2][j], a[i][0] * b[0][j]) + a[i][3].mul_add(b[3][j], a[i][1] * b[1][j]);
        }
    }
    r
}

pub fn dot4(a: &[f64; 4], b: &[f64; 4]) -> f64 {
    (a[0] * b[0] + a[2] * b[2]) + (a[1] * b[1] + a[3] * b[3])
}

pub fn eye4() -> M4 {
    let mut m = [[0.0; 4]; 4];
    for (i, r) in m.iter_mut().enumerate() {
        r[i] = 1.0;
    }
    m
}

/// A 4 x 4 transform from a rotation and a translation.
pub fn rt4(r: &M3, t: &V3) -> M4 {
    let mut m = eye4();
    for i in 0..3 {
        m[i][..3].copy_from_slice(&r[i]);
        m[i][3] = t[i];
    }
    m
}

pub fn rot_of(m: &M4) -> M3 {
    [0, 1, 2].map(|i| [m[i][0], m[i][1], m[i][2]])
}

pub fn trans_of(m: &M4) -> V3 {
    [m[0][3], m[1][3], m[2][3]]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inverse_of_a_rigid_transform_is_exact() {
        let m = rt4(&[[0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]], &[0.25, -1.5, 0.0]);
        let x = inv4(&m).unwrap_or_default();
        assert_eq!(mm4(&m, &x), eye4());
        assert_eq!(trans_of(&x), [1.5, 0.25, 0.0]);
        assert!(trans_of(&x)[2].is_sign_positive());
    }
}
