//! Scoped reports for successful Boolean retries. Discarded attempts never enter
//! their caller's report, and independent evaluations cannot inherit old events.

use std::cell::RefCell;

use serde::Serialize;

const MAX_EVENTS: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BooleanStrategy {
    RefinedTolerance,
    ShiftedOperands,
    CoplanarSeparation,
    CommonBore,
    /// A boss in the other body's bore, grown into the material around it (nothing recut).
    GrownBoss,
    PlanarFallback,
    /// The bodies meet along curved faces, which no exact route takes: both were faceted and
    /// the polygons combined, so the result is flat where they curve.
    FacetedFallback,
    ToolNudge,
    Piecewise,
    /// No retry gave a result, and the answer from sample points was kept although the bodies'
    /// facetings disagree with it.
    UnverifiedShortcut,
}

/// Numerical adjustments used by a successful retry, in millimetres/radians.
/// Operand shifts are reversed on the result; face shifts are temporary working
/// offsets. A tool nudge, in contrast, changes the relative operand placement.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BooleanFallback {
    pub strategy: BooleanStrategy,
    pub translation_mm: f64,
    pub rotation_rad: f64,
    pub face_shift_mm: f64,
    pub tolerance_mm: f64,
}

impl BooleanStrategy {
    /// Whether a result made this way can differ from the exact one: the tool was moved
    /// against the other body, part of where they meet was ignored, or curved faces were
    /// replaced by facets. The other retries work on temporary geometry or tolerances only,
    /// and their results are the exact ones.
    pub fn alters_result(self) -> bool {
        matches!(self, BooleanStrategy::ToolNudge | BooleanStrategy::UnverifiedShortcut | BooleanStrategy::FacetedFallback)
    }
}

impl BooleanFallback {
    pub(crate) fn new(strategy: BooleanStrategy) -> Self {
        Self { strategy, translation_mm: 0., rotation_rad: 0., face_shift_mm: 0., tolerance_mm: 0. }
    }

    fn description(&self) -> String {
        match self.strategy {
            BooleanStrategy::RefinedTolerance => format!("refined tolerance {:.6e} mm", self.tolerance_mm),
            BooleanStrategy::ShiftedOperands => {
                format!("shifted operands {:.6e} mm (result shifted back), tolerance {:.6e} mm", self.translation_mm, self.tolerance_mm)
            }
            BooleanStrategy::CoplanarSeparation => format!("temporary coincident-face separation up to {:.6e} mm per face", self.face_shift_mm),
            BooleanStrategy::CommonBore => format!("temporary bore offset {:.6e} mm, followed by recutting the original bore", self.face_shift_mm),
            BooleanStrategy::GrownBoss => format!("boss grown {:.6e} mm into the material around the bore it fills", self.face_shift_mm),
            BooleanStrategy::PlanarFallback => "exact planar fallback (no displacement)".into(),
            BooleanStrategy::FacetedFallback => {
                format!("curved contact faceted to {:.6e} mm (the result is flat where the faces curve)", self.tolerance_mm)
            }
            BooleanStrategy::ToolNudge => format!("tool displacement {:.6e} mm, rotation {:.6e} rad", self.translation_mm, self.rotation_rad),
            BooleanStrategy::Piecewise => "piecewise retry (no displacement)".into(),
            BooleanStrategy::UnverifiedShortcut => format!(
                "not verified: the bodies' facets pass through each other by more than {:.6e} mm, but they could not be intersected there; that part is ignored",
                self.tolerance_mm
            ),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct BooleanDiagnostics {
    pub events: Vec<BooleanFallback>,
    pub truncated: bool,
}

impl BooleanDiagnostics {
    pub(crate) fn push(&mut self, event: BooleanFallback) {
        if self.events.len() < MAX_EVENTS {
            self.events.push(event);
        } else {
            self.truncated = true;
        }
    }

    /// Why the result is not verified, when it ignores a place where the bodies seem to pass
    /// through each other ([`BooleanStrategy::UnverifiedShortcut`]).
    pub fn unverified(&self) -> Option<String> {
        self.events.iter().find(|event| event.strategy == BooleanStrategy::UnverifiedShortcut).map(BooleanFallback::description)
    }

    /// Every retry the result took, for a message about the boolean (one that failed its
    /// checks lists what was tried).
    pub fn summary(&self) -> Option<String> {
        self.described(&self.events.iter().collect::<Vec<_>>())
    }

    /// The note a feature carries: the retries that can make its result differ from the exact
    /// one ([`BooleanStrategy::alters_result`]). Exact retries are in `events` all the same
    /// (`timeline.inspect`), but they leave the result as it should be: no reason to warn.
    pub fn warning(&self) -> Option<String> {
        self.described(&self.events.iter().filter(|e| e.strategy.alters_result()).collect::<Vec<_>>())
    }

    fn described(&self, events: &[&BooleanFallback]) -> Option<String> {
        if events.is_empty() && !self.truncated {
            return None;
        }
        let mut descriptions: Vec<String> = events.iter().take(4).map(|e| e.description()).collect();
        if events.len() > 4 {
            descriptions.push(format!("{} additional retries", events.len() - 4));
        }
        if self.truncated {
            descriptions.push(format!("report capped at {MAX_EVENTS} retries"));
        }
        Some(format!("Boolean retry: {}", descriptions.join("; ")))
    }
}

thread_local! {
    static REPORT: RefCell<Option<BooleanDiagnostics>> = const { RefCell::new(None) };
}

struct Scope {
    previous: Option<Option<BooleanDiagnostics>>,
}

impl Drop for Scope {
    fn drop(&mut self) {
        if let Some(previous) = self.previous.take() {
            REPORT.with(|cell| {
                if let Ok(mut report) = cell.try_borrow_mut() {
                    *report = previous;
                }
            });
        }
    }
}

/// Capture only the events produced by this call. Nested callers explicitly merge
/// an accepted attempt; dropping a scope restores its parent even during unwinding.
pub fn capture_boolean_diagnostics<T>(run: impl FnOnce() -> T) -> (T, BooleanDiagnostics) {
    let previous = REPORT.with(|cell| cell.try_borrow_mut().ok().map(|mut report| report.replace(BooleanDiagnostics::default())));
    let scope = Scope { previous };
    let result = run();
    let report = REPORT.with(|cell| cell.try_borrow_mut().ok().and_then(|mut report| report.take())).unwrap_or_default();
    drop(scope);
    (result, report)
}

pub(crate) fn record(event: BooleanFallback) {
    REPORT.with(|cell| {
        if let Ok(mut report) = cell.try_borrow_mut()
            && let Some(report) = report.as_mut()
        {
            report.push(event);
        }
    });
}

pub(crate) fn merge(report: BooleanDiagnostics) {
    for event in report.events {
        record(event);
    }
    if report.truncated {
        REPORT.with(|cell| {
            if let Ok(mut report) = cell.try_borrow_mut()
                && let Some(report) = report.as_mut()
            {
                report.truncated = true;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_attempts_are_committed_explicitly_and_do_not_leak() {
        let (_, report) = capture_boolean_diagnostics(|| {
            let (_, discarded) = capture_boolean_diagnostics(|| record(BooleanFallback::new(BooleanStrategy::ToolNudge)));
            assert_eq!(discarded.events.len(), 1);
            let (_, accepted) = capture_boolean_diagnostics(|| record(BooleanFallback::new(BooleanStrategy::PlanarFallback)));
            merge(accepted);
        });
        assert_eq!(report.events.len(), 1);
        assert_eq!(report.events[0].strategy, BooleanStrategy::PlanarFallback);
        assert!(capture_boolean_diagnostics(|| {}).1.events.is_empty());
        record(BooleanFallback::new(BooleanStrategy::ToolNudge));
        assert!(capture_boolean_diagnostics(|| {}).1.events.is_empty());
    }

    /// Only retries whose result can differ from the exact one make a warning; the summary
    /// lists every retry.
    #[test]
    fn only_retries_that_alter_the_result_warn() {
        let report = |strategies: &[BooleanStrategy]| {
            capture_boolean_diagnostics(|| {
                for s in strategies {
                    record(BooleanFallback::new(*s));
                }
            })
            .1
        };
        let exact = report(&[BooleanStrategy::ShiftedOperands, BooleanStrategy::GrownBoss, BooleanStrategy::CoplanarSeparation]);
        assert!(exact.warning().is_none(), "{exact:?}");
        assert!(exact.summary().is_some_and(|s| s.contains("shifted operands") && s.contains("boss grown")));
        let nudged = report(&[BooleanStrategy::GrownBoss, BooleanStrategy::ToolNudge]);
        let warning = nudged.warning().unwrap();
        assert!(warning.contains("tool displacement") && !warning.contains("boss grown"), "{warning}");
    }

    #[test]
    fn exact_operations_and_failed_calls_have_no_retry_report() {
        use solidcraft_geom::Vec3;
        let body = crate::box_solid(Vec3::ZERO, Vec3::new(10., 10., 10.)).unwrap();
        let other = crate::box_solid(Vec3::new(5., 0., 0.), Vec3::new(15., 10., 10.)).unwrap();
        let (result, diagnostics) = capture_boolean_diagnostics(|| crate::boolean(&body, &other, crate::BoolOp::Union));
        assert!(result.unwrap().is_some());
        assert!(diagnostics.warning().is_none(), "{diagnostics:?}");
        let mesh = crate::mesh_body(&[Vec3::ZERO, Vec3::X, Vec3::Y], &[[0, 1, 2]]).unwrap();
        let (result, diagnostics) = capture_boolean_diagnostics(|| crate::boolean(&body, &mesh, crate::BoolOp::Cut));
        assert!(result.is_err());
        assert!(diagnostics.warning().is_none(), "{diagnostics:?}");
    }

    #[test]
    fn unwinding_restores_parent_and_reports_are_thread_local_and_bounded() {
        let (_, report) = capture_boolean_diagnostics(|| {
            let failed = crate::guard("diagnostic test", || {
                capture_boolean_diagnostics(|| -> crate::Result<()> {
                    record(BooleanFallback::new(BooleanStrategy::ToolNudge));
                    panic!("discard this attempt");
                })
                .0
            });
            assert!(failed.is_err());
            let separate = std::thread::spawn(|| capture_boolean_diagnostics(|| record(BooleanFallback::new(BooleanStrategy::ToolNudge))).1);
            assert_eq!(separate.join().unwrap().events.len(), 1);
            for _ in 0..MAX_EVENTS + 1 {
                record(BooleanFallback::new(BooleanStrategy::PlanarFallback));
            }
        });
        assert_eq!(report.events.len(), MAX_EVENTS);
        assert!(report.truncated);
        assert!(report.events.iter().all(|event| event.strategy == BooleanStrategy::PlanarFallback));
        assert!(report.warning().unwrap().contains("capped"));
    }
}
