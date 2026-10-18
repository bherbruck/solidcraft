//! Placement given as a matrix: a script's 3×4 (rotation rows and a translation column) or
//! 4×4 transform, read as a rigid move. A rotation that is one only up to rounding is
//! orthonormalised; a scale, shear or mirror is refused with the reason (a mirror is
//! `solid.mirror`'s, not a placement's).

use serde_json::Value;
use solidcraft_doc::{IDENTITY, Mat};

use crate::Result;
use crate::params::bad;

/// The default largest departure from a rotation a matrix may show: |RᵀR − I| per entry.
pub const TOLERANCE: f64 = 1e-9;
/// The loosest tolerance a caller may ask for.
const MAX_TOLERANCE: f64 = 1e-3;

/// The `tolerance` parameter: the default, or a finite positive number no looser than 1e-3.
pub fn tolerance(p: &Value, cmd: &str) -> Result<f64> {
    match p.get("tolerance") {
        None | Some(Value::Null) => Ok(TOLERANCE),
        Some(v) => v
            .as_f64()
            .filter(|t| t.is_finite() && *t > 0.0 && *t <= MAX_TOLERANCE)
            .ok_or_else(|| bad(cmd, format!("tolerance must be a number above 0 and at most {MAX_TOLERANCE}"))),
    }
}

/// Read `matrix` as a rigid transform. It is row-major: 12 numbers (three rows of rotation
/// and translation, `[r00, r01, r02, tx, r10, …, tz]`) or 16 (a fourth row `0, 0, 0, 1`), flat
/// or as rows.
pub fn rigid(v: &Value, tolerance: f64, cmd: &str) -> Result<Mat> {
    let numbers = numbers(v).ok_or_else(|| bad(cmd, "matrix must be 12 numbers (3x4, row by row) or 16 (4x4), flat or as rows"))?;
    if numbers.iter().any(|x| !x.is_finite()) {
        return Err(bad(cmd, "matrix must contain finite numbers"));
    }
    let at = |row: usize, col: usize| numbers.get(row * 4 + col).copied().unwrap_or(0.0);
    if numbers.len() == 16 {
        let last = [at(3, 0), at(3, 1), at(3, 2), at(3, 3) - 1.0];
        if last.iter().any(|x| x.abs() > tolerance) {
            return Err(bad(cmd, "matrix is not affine: its fourth row must be 0, 0, 0, 1 (a projective transform is not a placement)"));
        }
    }
    if (0..3).any(|r| at(r, 3).abs() > 1e9) {
        return Err(bad(cmd, "matrix translation exceeds supported numeric range"));
    }
    // r[row][col], then |RᵀR − I| measures how far it is from a rotation.
    let r: [[f64; 3]; 3] = std::array::from_fn(|row| std::array::from_fn(|col| at(row, col)));
    let deviation = (0..3)
        .flat_map(|i| (0..3).map(move |j| (i, j)))
        .map(|(i, j)| {
            let dot: f64 = (0..3).map(|k| r[k][i] * r[k][j]).sum();
            (dot - if i == j { 1.0 } else { 0.0 }).abs()
        })
        .fold(0.0, f64::max);
    let det = r[0][0] * (r[1][1] * r[2][2] - r[1][2] * r[2][1]) - r[0][1] * (r[1][0] * r[2][2] - r[1][2] * r[2][0])
        + r[0][2] * (r[1][0] * r[2][1] - r[1][1] * r[2][0]);
    if det < 0.0 {
        return Err(bad(cmd, "matrix mirrors (its determinant is negative): a placement only turns and moves; mirror bodies with solid.mirror"));
    }
    if !deviation.is_finite() || deviation > tolerance {
        return Err(bad(
            cmd,
            format!("matrix is not rigid: it scales or shears by {deviation:.3e} (|RᵀR − I|), above the tolerance {tolerance:.0e}"),
        ));
    }
    let rot = orthonormal(r);
    let mut m = IDENTITY;
    for (col, column) in m.iter_mut().enumerate().take(3) {
        for (row, cell) in column.iter_mut().enumerate().take(3) {
            *cell = rot[row][col];
        }
    }
    m[3] = [at(0, 3), at(1, 3), at(2, 3), 1.0];
    Ok(m)
}

/// The matrix's numbers, row by row: 12 or 16 of them, flat or nested as 3 or 4 rows of 4.
fn numbers(v: &Value) -> Option<Vec<f64>> {
    let a = v.as_array()?;
    let flat: Vec<f64> = if a.iter().all(Value::is_array) {
        if !matches!(a.len(), 3 | 4) {
            return None;
        }
        let mut out = Vec::with_capacity(16);
        for row in a {
            let row = row.as_array().filter(|r| r.len() == 4)?;
            for x in row {
                out.push(x.as_f64()?);
            }
        }
        out
    } else {
        a.iter().map(Value::as_f64).collect::<Option<_>>()?
    };
    matches!(flat.len(), 12 | 16).then_some(flat)
}

/// The rotation nearest to `r` (its polar factor), by Newton's iteration R ← (R + R⁻ᵀ) / 2,
/// which converges quadratically from a near-rotation; a few rounds reach machine precision.
fn orthonormal(mut r: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
    for _ in 0..4 {
        let Some(inv_t) = inverse_transpose(&r) else { break };
        for (row, inv_row) in r.iter_mut().zip(inv_t) {
            for (x, y) in row.iter_mut().zip(inv_row) {
                *x = 0.5 * (*x + y);
            }
        }
    }
    r
}

/// (R⁻¹)ᵀ: the cofactor matrix over the determinant.
fn inverse_transpose(r: &[[f64; 3]; 3]) -> Option<[[f64; 3]; 3]> {
    let a = |i: usize, j: usize| r[i % 3][j % 3];
    let cof: [[f64; 3]; 3] = std::array::from_fn(|i| std::array::from_fn(|j| a(i + 1, j + 1) * a(i + 2, j + 2) - a(i + 1, j + 2) * a(i + 2, j + 1)));
    let det: f64 = (0..3).map(|j| r[0][j] * cof[0][j]).sum();
    if !det.is_finite() || det.abs() < 1e-12 {
        return None;
    }
    Some(cof.map(|row| row.map(|x| x / det)))
}
