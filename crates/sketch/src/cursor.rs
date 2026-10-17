//! Feasible cursor motion: optimize only within the design Jacobian's null space, then
//! retract to the constraint manifold. Mouse error never competes with design residuals.
use super::{ACCEPT, Hold, Layout, State, TOL, build_blocks, columns, initial_x, layout, lm_capped, local_system, max_abs, points_on_curves, system};
#[cfg(feature = "solver-profiling")]
use crate::profiling::{self, Phase};
use crate::{
    CurveKind, Sketch,
    linalg::{Mat, rref},
    union_find::{find, union},
};
use solidcraft_geom::Vec2;

const MAX_VARIABLES: usize = 400;
const MAX_STEPS: usize = 96;
const CORRECTION_ITERS: usize = 16;
const TOTAL_CORRECTION_ITERS: usize = 256;

/// Move a point toward `target` using freedom left by the design constraints. `max_step`
/// bounds the full variable displacement before constraint correction; `valid` can reject
/// a corrected candidate (for example, an arc crossing its own start). Only feasible,
/// cursor-improving candidates are committed. False leaves the sketch unchanged when the
/// input is unsolved, invalid or outside the dense-work limit. Work is bounded; true may
/// retain the best feasible pose when the iteration budget is exhausted.
pub fn move_point_towards(sk: &mut Sketch, point: usize, target: Vec2, max_step: f64, valid: impl Fn(&Sketch, &Sketch) -> bool) -> bool {
    #[cfg(feature = "solver-profiling")]
    let _timer = profiling::timer(Phase::Cursor);
    #[cfg(feature = "solver-profiling")]
    profiling::count(|p| p.cursor_calls += 1);
    if !crate::model::in_range(target) || !max_step.is_finite() || max_step <= 0.0 || !in_range(sk) {
        return false;
    }
    let problem = sk.clone();
    let lay = layout(&problem, &Hold::default());
    let Some(px) = lay.pvar.get(point).copied().flatten() else { return false };
    let (blocks, _) = build_blocks(&problem, &lay, &points_on_curves(&problem));
    let mut parent: Vec<usize> = (0..lay.n).collect();
    union(&mut parent, px, px + 1);
    for (_, vars) in &blocks {
        if let Some(&first) = vars.first() {
            for &v in vars.iter().skip(1) {
                union(&mut parent, first, v);
            }
        }
    }
    let root = find(&mut parent, px);
    let vars: Vec<usize> = (0..lay.n).filter(|v| find(&mut parent, *v) == root).collect();
    if vars.len() > MAX_VARIABLES {
        return false;
    }
    let sub: Vec<_> =
        blocks.iter().filter(|(_, v)| v.iter().any(|i| vars.binary_search(i).is_ok())).map(|(b, v)| (super::copy_block(b), v.clone())).collect();
    let mut x = initial_x(&problem, &lay);
    let initial_residual = max_abs(&system(&problem, &lay, &blocks, &mut x, false).0);
    if initial_residual >= ACCEPT {
        return false;
    }
    let mut col = vec![usize::MAX; lay.n];
    columns(&mut col, &vars, &vars);
    let (Some(ix), Some(iy)) = (vars.iter().position(|v| *v == px), vars.iter().position(|v| *v == px + 1)) else { return false };
    let mut best = problem.clone();
    let mut spent = 0;
    // Held feasibility solves accept up to ACCEPT. A tiny, nearly feasible hard pin can
    // precede a failed step; refine it before constructing a tangent, rather than losing
    // the cursor objective just because the previous pose is between TOL and ACCEPT.
    if initial_residual >= TOL {
        let (its, residual) = lm_capped(&problem, &lay, &sub, &vars, &mut x, &col, CORRECTION_ITERS);
        spent += its.max(1);
        #[cfg(feature = "solver-profiling")]
        profiling::count(|p| p.cursor_retractions += 1);
        if !(residual < TOL) {
            return false;
        }
        apply(&mut best, &lay, &x);
        if !in_range(&best) || !valid(&problem, &best) {
            return false;
        }
    }
    let mut gain = 1.0_f64;
    let mut steps = 0;
    for _ in 0..MAX_STEPS {
        let at = State { sk: &problem, lay: &lay, x: &x }.p(point);
        let error = target - at;
        if error.len() < 1e-7 || spent >= TOTAL_CORRECTION_ITERS {
            break;
        }
        let (_, j, _) = local_system(&problem, &lay, &sub, &mut x, &col, vars.len());
        let z = null_space(j);
        if z.is_empty() {
            break;
        }
        let mut direction = target_step(&z, ix, iy, error);
        let norm = length(&direction);
        // Adapt to curvature of the reachable locus. Inside a circle the Gauss-Newton
        // cursor step can be arbitrarily small although useful freedom remains.
        let scale = if norm > 0.0 { gain.min(max_step / norm) } else { gain };
        for v in &mut direction {
            *v *= scale;
        }
        let mut directions = vec![(direction, false)];
        // A cursor across a circular locus has (nearly) zero first derivative at the
        // farthest point. If line search stalls, probe both tangent directions. Testing
        // only an exactly zero gradient would strand reversed drags after rounding.
        let influence = |v: &&Vec<f64>| v.get(ix).copied().unwrap_or(0.0).hypot(v.get(iy).copied().unwrap_or(0.0));
        let mut probes: Vec<_> = z.iter().collect();
        probes.sort_by(|a, b| influence(b).total_cmp(&influence(a)));
        for v in probes.into_iter().take(2) {
            directions.push((v.iter().map(|x| x * max_step).collect(), true));
            directions.push((v.iter().map(|x| -x * max_step).collect(), true));
        }
        let old_cost = error.dot(error);
        let mut accepted = None;
        'search: for (direction, probe) in directions {
            if length(&direction) < 1e-12 {
                continue;
            }
            for backtrack in 0..if probe { 1 } else { 8 } {
                if spent >= TOTAL_CORRECTION_ITERS {
                    break 'search;
                }
                let alpha = 0.5_f64.powi(backtrack);
                let mut trial = x.clone();
                for (&v, d) in vars.iter().zip(&direction) {
                    if let Some(t) = trial.get_mut(v) {
                        *t += alpha * d;
                    }
                }
                let (its, residual) = lm_capped(&problem, &lay, &sub, &vars, &mut trial, &col, CORRECTION_ITERS.min(TOTAL_CORRECTION_ITERS - spent));
                #[cfg(feature = "solver-profiling")]
                profiling::count(|p| p.cursor_retractions += 1);
                spent += its.max(1);
                if !(residual < TOL) {
                    continue;
                }
                let next = State { sk: &problem, lay: &lay, x: &trial }.p(point);
                let cost = (target - next).dot(target - next);
                if !cost.is_finite() || cost >= old_cost - 1e-14 * old_cost.max(1.0) {
                    continue;
                }
                let mut candidate = best.clone();
                apply(&mut candidate, &lay, &trial);
                if !in_range(&candidate) || !valid(&best, &candidate) {
                    continue;
                }
                accepted = Some((trial, candidate, ((if probe { 1.0 } else { scale }) * alpha * 1.5).clamp(1e-8, 1e8)));
                break 'search;
            }
        }
        let Some((next, candidate, next_gain)) = accepted else { break };
        x = next;
        best = candidate;
        gain = next_gain;
        steps += 1;
        #[cfg(feature = "solver-profiling")]
        profiling::count(|p| p.cursor_steps += 1);
    }
    #[cfg(feature = "solver-profiling")]
    if spent >= TOTAL_CORRECTION_ITERS || steps == MAX_STEPS {
        profiling::count(|p| p.cursor_budget_stops += 1);
    }
    #[cfg(not(feature = "solver-profiling"))]
    let _ = steps;
    // Full constraint validation includes constant blocks and disconnected components.
    if max_abs(&system(&best, &lay, &blocks, &mut x, false).0) >= ACCEPT {
        return false;
    }
    super::refresh_driven_dimensions(&mut best);
    *sk = best;
    true
}

fn length(v: &[f64]) -> f64 {
    v.iter().map(|x| x * x).sum::<f64>().sqrt()
}

fn in_range(sk: &Sketch) -> bool {
    sk.points.iter().all(|p| crate::model::in_range(p.pos))
        && sk
            .curves
            .iter()
            .all(|c| !matches!(c.kind, CurveKind::Circle { r, .. } | CurveKind::Ellipse { r, .. } if !crate::model::radius_in_range(r)))
}

fn null_space(mut j: Mat) -> Vec<Vec<f64>> {
    #[cfg(feature = "solver-profiling")]
    let _timer = profiling::timer(Phase::CursorNullSpace);
    for row in 0..j.rows {
        let scale = (0..j.cols).map(|c| j.get(row, c).abs()).fold(0.0_f64, f64::max);
        if scale > 0.0 {
            for c in 0..j.cols {
                j.set(row, c, j.get(row, c) / scale);
            }
        }
    }
    let pivots = rref(&mut j, 1e-9);
    let mut z: Vec<Vec<f64>> = Vec::new();
    for free in (0..j.cols).filter(|c| !pivots.contains(c)) {
        let mut v = vec![0.0; j.cols];
        if let Some(x) = v.get_mut(free) {
            *x = 1.0;
        }
        for (row, &pivot) in pivots.iter().enumerate() {
            if let Some(x) = v.get_mut(pivot) {
                *x = -j.get(row, free);
            }
        }
        for _ in 0..2 {
            for q in &z {
                let d: f64 = v.iter().zip(q).map(|(a, b)| a * b).sum();
                for (a, b) in v.iter_mut().zip(q) {
                    *a -= d * b;
                }
            }
        }
        let norm = length(&v);
        if norm.is_finite() && norm > 1e-10 {
            for x in &mut v {
                *x /= norm;
            }
            z.push(v);
        }
    }
    z
}

fn target_step(z: &[Vec<f64>], ix: usize, iy: usize, error: Vec2) -> Vec<f64> {
    let mut a = 0.0;
    let mut b = 0.0;
    let mut c = 0.0;
    for q in z {
        let x = q.get(ix).copied().unwrap_or(0.0);
        let y = q.get(iy).copied().unwrap_or(0.0);
        a += x * x;
        b += x * y;
        c += y * y;
    }
    let high = 0.5 * (a + c + (a - c).hypot(2.0 * b));
    let low = if high > 0.0 { (a * c - b * b).max(0.0) / high } else { 0.0 };
    let axis = if a >= c { Vec2::new(high - c, b) } else { Vec2::new(b, high - a) }.normalized().unwrap_or(if a >= c { Vec2::X } else { Vec2::Y });
    let other = Vec2::new(-axis.y, axis.x);
    let mut u = if high > 1e-12 { axis * (axis.dot(error) / high) } else { Vec2::ZERO };
    if low > high * 1e-8 {
        u += other * (other.dot(error) / low);
    }
    let mut step = vec![0.0; z.first().map_or(0, Vec::len)];
    for q in z {
        let d = q.get(ix).copied().unwrap_or(0.0) * u.x + q.get(iy).copied().unwrap_or(0.0) * u.y;
        for (x, v) in step.iter_mut().zip(q) {
            *x += d * v;
        }
    }
    step
}

fn apply(sk: &mut Sketch, lay: &Layout, x: &[f64]) {
    for (i, p) in sk.points.iter_mut().enumerate() {
        if let Some(v) = lay.pvar.get(i).copied().flatten()
            && let (Some(a), Some(b)) = (x.get(v), x.get(v + 1))
        {
            p.pos = Vec2::new(*a, *b);
        }
    }
    for (i, c) in sk.curves.iter_mut().enumerate() {
        if let Some(v) = lay.rvar.get(i).copied().flatten()
            && let Some(value) = x.get(v)
            && let CurveKind::Circle { r, .. } | CurveKind::Ellipse { r, .. } = &mut c.kind
        {
            *r = *value;
        }
    }
}
