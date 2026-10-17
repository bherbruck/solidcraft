//! Opt-in, per-thread solver measurements (`solver-profiling` feature).
//!
//! Wrap a public solve in [`capture`]. Collection never changes solver decisions. Timings are
//! inclusive (LM includes Jacobian and linear solve time), not additive. On wasm the counters
//! and trace work but clocks are disabled: `std::time::Instant` is unavailable there.

use std::cell::Cell;

use serde::Serialize;

/// Bounds retained records, independently of how many solves the closure performs.
pub const MAX_RUNS: usize = 1024;
pub const MAX_TRIALS: usize = 10_000;

/// Inclusive phase wall times in nanoseconds. Empty phases stay zero.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Timings {
    pub solve: u64,
    pub seed: u64,
    pub lm: u64,
    pub residual_system: u64,
    pub jacobian: u64,
    pub normal_equations: u64,
    pub linear_solve: u64,
    pub analysis: u64,
    /// Dependency classification, including its Jacobian and optional explanation.
    pub redundancy: u64,
    /// Jacobian construction within dependency classification (also included in `jacobian`).
    pub redundancy_jacobian: u64,
    /// Ordered, twice-reorthogonalized row classification, including row storage.
    pub redundancy_classification: u64,
    /// Optional `determined_by` explanation; ordinary solve reports do not request it.
    pub redundancy_support: u64,
    /// Feasible cursor optimization; includes null-space construction and LM retractions.
    pub cursor: u64,
    pub cursor_null_space: u64,
}

/// Work counts include all actual calls in the captured closure, including retries and
/// continuation. A residual block evaluation is one constraint/implicit-curve block, not one row.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Counters {
    pub solve_calls: u64,
    pub components: u64,
    pub oversized_components: u64,
    pub constructed_variables: u64,
    pub residual_system_evaluations: u64,
    pub residual_block_evaluations: u64,
    pub jacobian_evaluations: u64,
    pub lm_runs: u64,
    pub outer_iterations: u64,
    pub linear_solves: u64,
    pub linear_failures: u64,
    pub accepted_trials: u64,
    pub rejected_trials: u64,
    pub plain_start_retries: u64,
    pub continuation_runs: u64,
    pub redundancy_checks: u64,
    pub cursor_calls: u64,
    pub cursor_steps: u64,
    pub cursor_retractions: u64,
    pub cursor_budget_stops: u64,
}

/// Why this invocation of LM returned. This is distinct from the final solve's acceptance
/// tolerance, and does not certify that inconsistent constraints have no solution.
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    ResidualTolerance,
    NoImprovement,
    IterationLimit,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Accepted,
    Rejected,
    LinearFailure,
}

/// One LM call, identified even when the caller retries the same component.
#[derive(Debug, Serialize)]
pub struct LmRun {
    pub id: u64,
    pub variables: usize,
    pub residual_rows: usize,
    pub iteration_limit: usize,
    pub initial_cost: Option<f64>,
    pub final_max_residual: Option<f64>,
    pub stop_reason: Option<StopReason>,
}

/// One damping trial. `None` means unavailable or non-finite. Cost is sum(r²), without 1/2.
/// Predicted reduction uses the undamped residual model; rho is observational only.
#[derive(Debug, Serialize)]
pub struct Trial {
    pub run: u64,
    pub iteration: usize,
    pub attempt: usize,
    pub lambda: f64,
    pub cost_before: Option<f64>,
    pub cost_after: Option<f64>,
    pub gradient_inf: Option<f64>,
    pub step_norm: Option<f64>,
    pub predicted_reduction: Option<f64>,
    pub actual_reduction: Option<f64>,
    pub rho: Option<f64>,
    pub decision: Decision,
}

#[derive(Debug, Default, Serialize)]
pub struct SolveProfile {
    pub timings_available: bool,
    pub timings_ns: Timings,
    pub counters: Counters,
    pub runs: Vec<LmRun>,
    pub trials: Vec<Trial>,
    pub dropped_runs: u64,
    pub dropped_trials: u64,
}

thread_local! {
    static ACTIVE: Cell<Option<SolveProfile>> = const { Cell::new(None) };
}

struct CaptureGuard(Option<SolveProfile>);

impl Drop for CaptureGuard {
    fn drop(&mut self) {
        ACTIVE.set(self.0.take());
    }
}

/// Measure only the closure on the current thread. Nested captures are isolated and the outer
/// collector resumes afterwards. Unwinding restores the previous collector as well.
pub fn capture<T>(f: impl FnOnce() -> T) -> (T, SolveProfile) {
    let previous = ACTIVE.replace(Some(SolveProfile { timings_available: !cfg!(target_arch = "wasm32"), ..SolveProfile::default() }));
    let guard = CaptureGuard(previous);
    let result = f();
    let profile = ACTIVE.take().unwrap_or_default();
    drop(guard);
    (result, profile)
}

fn update(f: impl FnOnce(&mut SolveProfile)) {
    if let Some(mut profile) = ACTIVE.take() {
        f(&mut profile);
        ACTIVE.set(Some(profile));
    }
}

pub(crate) fn count(f: impl FnOnce(&mut Counters)) {
    update(|p| f(&mut p.counters));
}

pub(crate) fn finite(x: f64) -> Option<f64> {
    x.is_finite().then_some(x)
}

pub(crate) fn record_trial(
    run: u64,
    iteration: usize,
    attempt: usize,
    lambda: f64,
    before: f64,
    after: Option<f64>,
    gradient: &[f64],
    j: &crate::linalg::Mat,
    step: Option<&[f64]>,
    decision: Decision,
) {
    // Keep the extra matrix-vector product out of uncaptured timing baselines.
    let active = ACTIVE.with(|cell| {
        let p = cell.take();
        let active = p.is_some();
        cell.set(p);
        active
    });
    if !active {
        return;
    }
    let predicted = step.map(|step| {
        let gp: f64 = gradient.iter().zip(step).map(|(g, s)| g * s).sum();
        let jp2: f64 = (0..j.rows).map(|row| step.iter().enumerate().map(|(c, s)| j.get(row, c) * s).sum::<f64>().powi(2)).sum();
        -2.0 * gp - jp2
    });
    let actual = after.map(|after| before - after);
    trial(Trial {
        run,
        iteration,
        attempt,
        lambda,
        cost_before: finite(before),
        cost_after: after.and_then(finite),
        gradient_inf: finite(gradient.iter().fold(0.0_f64, |m, g| if g.is_finite() { m.max(g.abs()) } else { f64::INFINITY })),
        step_norm: step.and_then(|s| finite(s.iter().fold(0.0_f64, |n, x| n.hypot(*x)))),
        predicted_reduction: predicted.and_then(finite),
        actual_reduction: actual.and_then(finite),
        rho: predicted.filter(|p| *p > 0.0).zip(actual).and_then(|(p, a)| finite(a / p)),
        decision,
    });
}

pub(crate) enum Phase {
    Solve,
    Seed,
    Lm,
    ResidualSystem,
    Jacobian,
    NormalEquations,
    LinearSolve,
    Analysis,
    Redundancy,
    RedundancyJacobian,
    RedundancyClassification,
    RedundancySupport,
    Cursor,
    CursorNullSpace,
}

pub(crate) struct Timer {
    phase: Phase,
    #[cfg(not(target_arch = "wasm32"))]
    start: Option<std::time::Instant>,
}

pub(crate) fn timer(phase: Phase) -> Timer {
    Timer {
        phase,
        #[cfg(not(target_arch = "wasm32"))]
        start: ACTIVE.with(|cell| {
            let profile = cell.take();
            let active = profile.is_some();
            cell.set(profile);
            active.then(std::time::Instant::now)
        }),
    }
}

impl Drop for Timer {
    fn drop(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        let ns = self.start.map(|s| s.elapsed().as_nanos().min(u64::MAX as u128) as u64).unwrap_or(0);
        #[cfg(target_arch = "wasm32")]
        let ns = 0;
        update(|p| {
            let slot = match self.phase {
                Phase::Solve => &mut p.timings_ns.solve,
                Phase::Seed => &mut p.timings_ns.seed,
                Phase::Lm => &mut p.timings_ns.lm,
                Phase::ResidualSystem => &mut p.timings_ns.residual_system,
                Phase::Jacobian => &mut p.timings_ns.jacobian,
                Phase::NormalEquations => &mut p.timings_ns.normal_equations,
                Phase::LinearSolve => &mut p.timings_ns.linear_solve,
                Phase::Analysis => &mut p.timings_ns.analysis,
                Phase::Redundancy => &mut p.timings_ns.redundancy,
                Phase::RedundancyJacobian => &mut p.timings_ns.redundancy_jacobian,
                Phase::RedundancyClassification => &mut p.timings_ns.redundancy_classification,
                Phase::RedundancySupport => &mut p.timings_ns.redundancy_support,
                Phase::Cursor => &mut p.timings_ns.cursor,
                Phase::CursorNullSpace => &mut p.timings_ns.cursor_null_space,
            };
            *slot = slot.saturating_add(ns);
        });
    }
}

pub(crate) fn start_run(variables: usize, residual_rows: usize, iteration_limit: usize, cost: f64) -> u64 {
    let mut id = 0;
    update(|p| {
        id = p.counters.lm_runs;
        p.counters.lm_runs = p.counters.lm_runs.saturating_add(1);
        if p.runs.len() < MAX_RUNS {
            p.runs.push(LmRun {
                id,
                variables,
                residual_rows,
                iteration_limit,
                initial_cost: finite(cost),
                final_max_residual: None,
                stop_reason: None,
            });
        } else {
            p.dropped_runs = p.dropped_runs.saturating_add(1);
        }
    });
    id
}

pub(crate) fn finish_run(id: u64, reason: StopReason, residual: f64) {
    update(|p| {
        if let Some(run) = p.runs.iter_mut().find(|r| r.id == id) {
            run.stop_reason = Some(reason);
            run.final_max_residual = finite(residual);
        }
    });
}

pub(crate) fn trial(record: Trial) {
    update(|p| {
        let c = &mut p.counters;
        c.linear_solves = c.linear_solves.saturating_add(1);
        match record.decision {
            Decision::Accepted => c.accepted_trials = c.accepted_trials.saturating_add(1),
            Decision::Rejected => c.rejected_trials = c.rejected_trials.saturating_add(1),
            Decision::LinearFailure => c.linear_failures = c.linear_failures.saturating_add(1),
        }
        if p.trials.len() < MAX_TRIALS {
            p.trials.push(record);
        } else {
            p.dropped_trials = p.dropped_trials.saturating_add(1);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Analysis, ConstraintKind, Hold, Sketch, solve_with};
    use solidcraft_geom::Vec2;

    fn distance(x: f64) -> Sketch {
        let mut sk = Sketch::new();
        let q = sk.add_point(Vec2::new(x, 0.0), None).unwrap();
        sk.add_constraint(ConstraintKind::Distance { p: 0, q, value: 10.0 }, None).unwrap();
        sk
    }

    #[test]
    fn measurements_preserve_geometry_and_explain_a_stalled_solve() {
        for x in [0.0, 1e-5] {
            let mut plain = distance(x);
            let mut measured = plain.clone();
            let run = |sk: &mut Sketch| solve_with(sk, &Hold::default(), None, Analysis::StatusOnly, None);
            let a = run(&mut plain);
            let (b, profile) = crate::construct::switch::without_differential(|| capture(|| run(&mut measured)));
            assert_eq!(serde_json::to_value(a).unwrap(), serde_json::to_value(b).unwrap());
            assert_eq!(plain, measured);
            assert_eq!(profile.counters.solve_calls, 1);
            assert_eq!(profile.counters.lm_runs, 1);
            assert_eq!(
                profile.counters.linear_solves,
                profile.counters.accepted_trials + profile.counters.rejected_trials + profile.counters.linear_failures
            );
            assert!(profile.counters.residual_block_evaluations > profile.counters.residual_system_evaluations);
            let first = profile.trials.first().unwrap();
            if x == 0.0 {
                assert_eq!(profile.runs[0].stop_reason, Some(StopReason::NoImprovement));
                assert_eq!(first.gradient_inf, Some(0.0));
                assert_eq!(first.step_norm, Some(0.0));
                assert_eq!(profile.counters.accepted_trials, 0);
            } else {
                assert_eq!(profile.runs[0].stop_reason, Some(StopReason::ResidualTolerance));
                assert!(profile.counters.accepted_trials > 0);
                assert!((first.rho.unwrap() - 1.0).abs() < 1e-6, "the positive radial step is a linear residual");
            }
            serde_json::to_string(&profile).unwrap();
        }
    }

    #[test]
    fn freedom_and_status_queries_do_not_run_dependency_analysis() {
        let mut sk = distance(10.0);
        sk.add_constraint(ConstraintKind::Distance { p: 0, q: 1, value: 10.0 }, None).unwrap();
        for analysis in [Analysis::Full, Analysis::Freedom, Analysis::StatusOnly] {
            let (report, profile) =
                crate::construct::switch::without_differential(|| capture(|| solve_with(&mut sk.clone(), &Hold::default(), None, analysis, None)));
            assert!(report.ok());
            if analysis == Analysis::Full {
                assert_eq!(profile.counters.redundancy_checks, 1);
                assert_eq!(report.redundant.len(), 1);
            } else {
                assert_eq!(profile.counters.redundancy_checks, 0);
                assert_eq!(profile.timings_ns.redundancy, 0);
                assert!(report.redundant.is_empty());
            }
            assert_eq!(report.dof, usize::from(analysis != Analysis::StatusOnly));
        }
    }

    #[test]
    fn nested_captures_and_unwinds_restore_the_outer_collector() {
        let (_, outer) = capture(|| {
            count(|c| c.solve_calls += 1);
            let (_, inner) = capture(|| count(|c| c.solve_calls += 2));
            assert_eq!(inner.counters.solve_calls, 2);
            let unwind = std::panic::catch_unwind(|| capture(|| panic!("test unwind")));
            assert!(unwind.is_err());
            count(|c| c.solve_calls += 3);
        });
        assert_eq!(outer.counters.solve_calls, 4);
        assert!(ACTIVE.take().is_none());
    }

    #[test]
    fn run_storage_is_bounded_but_counts_are_not_truncated() {
        let (_, p) = capture(|| {
            for _ in 0..MAX_RUNS + 3 {
                start_run(2, 1, 500, f64::INFINITY);
            }
            for _ in 0..MAX_TRIALS + 2 {
                record_trial(0, 0, 0, 1.0, f64::INFINITY, None, &[], &crate::linalg::Mat::zeros(0, 0), None, Decision::LinearFailure);
            }
        });
        assert_eq!(p.runs.len(), MAX_RUNS);
        assert_eq!(p.dropped_runs, 3);
        assert_eq!(p.counters.lm_runs, (MAX_RUNS + 3) as u64);
        assert_eq!(p.runs[0].initial_cost, None);
        assert_eq!(p.trials.len(), MAX_TRIALS);
        assert_eq!(p.dropped_trials, 2);
        assert_eq!(p.counters.linear_failures, (MAX_TRIALS + 2) as u64);
    }
}
