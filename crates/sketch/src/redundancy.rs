//! Redundant constraints: ones whose equations are already implied by the others where the
//! sketch is now (two rectangles sharing a side both say the side is vertical; a coincidence
//! between two corners that equal sizes already put together). They remove no degree of freedom
//! but they do not conflict either: the solver satisfies them along with the rest.
//!
//! The test is the Jacobian's: walking the rows in order (the curves' own relations, then the
//! constraints in the order they were added), a row that adds no rank to the rows before it is
//! dependent on them. A constraint none of whose rows add rank is redundant. Whether a
//! redundant constraint is consistent is a question of its residual (see
//! [`constraint_residual`]): zero where the other constraints hold means it is implied, not in
//! conflict.

use super::{Block, Hold, Layout, State, block_residuals, build_blocks, columns, initial_x, layout, local_system, max_abs, points_on_curves};
use crate::linalg::{Mat, solve as lin_solve};
use crate::model::Sketch;
use crate::union_find::{find, union};

/// A row is dependent on the rows before it when what is left of it, once their directions are
/// taken out, is below this share of its length (the rank analysis' pivot tolerance is 1e-7 on
/// rows of a comparable scale).
const DEPENDENT: f64 = 1e-6;
/// Variables in one component past which its constraints are not checked for redundancy: the
/// check is dense, rows × rank × variables.
pub(super) const MAX_REDUNDANCY_VARS: usize = 400;
/// A constraint's residual (mm, or radians for angles) below which it holds.
pub const CONSISTENT: f64 = 1e-6;

/// The constraint a block stands for.
pub(super) fn block_constraint(b: &Block) -> Option<usize> {
    match b {
        Block::User(ci, _) | Block::TangentAt(ci, _, _) => Some(*ci),
        Block::Closure { .. } | Block::Arc { .. } => None,
    }
}

fn dot(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// Per block of `blocks` (in order, over the variables `vars`): whether every one of its rows
/// is a combination of the rows before it. With `support`, also for the block at that index
/// (when redundant) the earlier blocks its rows combine.
pub(super) fn dependent_blocks(
    sk: &Sketch,
    lay: &Layout,
    blocks: &[(Block, Vec<usize>)],
    vars: &[usize],
    x: &mut [f64],
    support: Option<usize>,
) -> (Vec<bool>, Vec<usize>) {
    let mut col = vec![usize::MAX; lay.n];
    columns(&mut col, vars, vars);
    let (_, j, owner) = local_system(sk, lay, blocks, x, &col, vars.len());
    let n = j.cols;
    let row = |k: usize| -> Vec<f64> { j.data.get(k * n..(k + 1) * n).map(<[f64]>::to_vec).unwrap_or_default() };
    // Orthonormal directions of the independent rows so far, and those rows with their block.
    let mut q: Vec<Vec<f64>> = Vec::new();
    let mut basis: Vec<(Vec<f64>, usize)> = Vec::new();
    let mut has_rows = vec![false; blocks.len()];
    let mut adds_rank = vec![false; blocks.len()];
    let mut target_rows: Vec<Vec<f64>> = Vec::new();
    for k in 0..j.rows {
        let Some(&b) = owner.get(k) else { continue };
        let v = row(k);
        let norm = dot(&v, &v).sqrt();
        let mut w = v.clone();
        // Twice, for the orthogonality lost to rounding.
        for _ in 0..2 {
            for qi in &q {
                let d = dot(qi, &w);
                for (wk, qk) in w.iter_mut().zip(qi) {
                    *wk -= d * qk;
                }
            }
        }
        let left = dot(&w, &w).sqrt();
        if let Some(h) = has_rows.get_mut(b) {
            *h = true;
        }
        if norm.is_finite() && left.is_finite() && norm > 0.0 && left > DEPENDENT * norm {
            q.push(w.iter().map(|wk| wk / left).collect());
            basis.push((v, b));
            if let Some(a) = adds_rank.get_mut(b) {
                *a = true;
            }
        } else if Some(b) == support {
            target_rows.push(v);
        }
    }
    let dependent: Vec<bool> = has_rows.iter().zip(&adds_rank).map(|(h, a)| *h && !*a).collect();
    let mut by = Vec::new();
    if let Some(t) = support
        && dependent.get(t).copied().unwrap_or(false)
        && !basis.is_empty()
    {
        // The combination of the independent rows that gives each of the target's rows: the
        // least-squares solution of Bᵀ a = v, from (B Bᵀ) a = B v (B has full row rank).
        let m = basis.len();
        let mut g = Mat::zeros(m, m);
        for (r, (br, _)) in basis.iter().enumerate() {
            for (c, (bc, _)) in basis.iter().enumerate() {
                g.set(r, c, dot(br, bc));
            }
        }
        for v in &target_rows {
            let rhs: Vec<f64> = basis.iter().map(|(b, _)| dot(b, v)).collect();
            let Some(a) = lin_solve(g.clone(), rhs) else { continue };
            let big = a.iter().fold(0.0_f64, |m, x| m.max(x.abs()));
            for (ak, (_, b)) in a.iter().zip(&basis) {
                if ak.abs() > 1e-6 * big && !by.contains(b) {
                    by.push(*b);
                }
            }
        }
        by.sort_unstable();
    }
    (dependent, by)
}

/// How far the constraint at index `ci` is from holding where the sketch is now: its largest
/// residual (mm, or radians for angles). Zero for a constraint that holds, so a constraint that
/// removes no degree of freedom but holds is redundant, not conflicting. `None` for a driven
/// dimension (it constrains nothing) or an unknown index.
pub fn constraint_residual(sk: &Sketch, ci: usize) -> Option<f64> {
    let c = sk.constraints.get(ci).filter(|c| !c.driven)?;
    let fixed = Layout { pvar: vec![None; sk.points.len()], rvar: vec![None; sk.curves.len()], n: 0, points: 0, slide: Vec::new() };
    let on_curve = points_on_curves(sk);
    let b = match super::touch_point(sk, &c.kind, &on_curve) {
        Some(p) => Block::TangentAt(ci, &c.kind, p),
        None => Block::User(ci, &c.kind),
    };
    let mut r = Vec::new();
    block_residuals(&State { sk, lay: &fixed, x: &[] }, &b, &mut r);
    Some(max_abs(&r))
}

/// The constraints (indices) that already determine what the constraint at index `ci` asks
/// for, where the sketch is now: the ones whose equations its own are a combination of. Empty
/// when it is not redundant (it removes a degree of freedom), or when its component is too
/// large to check.
pub fn determined_by(sk: &Sketch, ci: usize) -> Vec<usize> {
    let lay = layout(sk, &Hold::default());
    let mut x = initial_x(sk, &lay);
    let on_curve = points_on_curves(sk);
    let (blocks, _) = build_blocks(sk, &lay, &on_curve);
    let Some(t) = blocks.iter().position(|(b, _)| block_constraint(b) == Some(ci)) else { return Vec::new() };
    let mut parent: Vec<usize> = (0..lay.n).collect();
    for (_, vars) in &blocks {
        if let Some(&first) = vars.first() {
            for &v in vars.iter().skip(1) {
                union(&mut parent, first, v);
            }
        }
    }
    let Some(root) = blocks.get(t).and_then(|(_, v)| v.first().copied()).map(|v| find(&mut parent, v)) else { return Vec::new() };
    let vars: Vec<usize> = (0..lay.n).filter(|v| find(&mut parent, *v) == root).collect();
    if vars.len() > MAX_REDUNDANCY_VARS {
        return Vec::new();
    }
    // The component's other blocks in order, then the constraint asked about.
    let mut order: Vec<usize> = (0..blocks.len())
        .filter(|bi| *bi != t && blocks.get(*bi).and_then(|(_, v)| v.first()).is_some_and(|v| find(&mut parent, *v) == root))
        .collect();
    order.push(t);
    let sub: Vec<(Block, Vec<usize>)> = order.iter().filter_map(|bi| blocks.get(*bi).map(|(b, v)| (super::copy_block(b), v.clone()))).collect();
    let (_, by) = dependent_blocks(sk, &lay, &sub, &vars, &mut x, Some(sub.len().saturating_sub(1)));
    let mut out: Vec<usize> = by.iter().filter_map(|k| sub.get(*k).and_then(|(b, _)| block_constraint(b))).collect();
    out.sort_unstable();
    out.dedup();
    out
}
