//! Progress and cooperative cancellation for a long evaluation (opening a large design) run
//! off the window thread. The caller wraps the work in [`watch`]; the timeline's evaluation
//! loop reports each feature it reaches and, once [`Progress::cancel`] is called, skips the
//! rest. Nothing changes for evaluations that are not watched.

use std::cell::RefCell;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

/// Shared between the worker evaluating a design and the window showing its progress.
#[derive(Debug)]
pub struct Progress {
    cancelled: AtomicBool,
    done: AtomicUsize,
    total: AtomicUsize,
    /// The feature the evaluation waits at until cancelled ([`Progress::hold_at`]).
    hold: AtomicUsize,
    /// Long work inside one feature (reading a large STEP file): its step and how far it is.
    work: std::sync::Mutex<Option<(&'static str, usize, usize)>>,
    /// A window wants the model as far as it is built, to show while the rest is ([`Progress::want_partial`]).
    partial_wanted: AtomicBool,
    /// The latest such model and when it was offered.
    partial: std::sync::Mutex<(Option<Partial>, Option<web_time::Instant>)>,
}

/// The model as far as an evaluation has built it, placed in the world as the design places it.
#[derive(Clone, Debug)]
pub struct Partial {
    /// Features evaluated.
    pub features: usize,
    pub world: Arc<crate::ModelState>,
}

/// How often an evaluation offers the model built so far.
const PARTIAL_EVERY: std::time::Duration = std::time::Duration::from_millis(1500);

impl Default for Progress {
    fn default() -> Self {
        Progress {
            cancelled: AtomicBool::new(false),
            done: AtomicUsize::new(0),
            total: AtomicUsize::new(0),
            hold: AtomicUsize::new(usize::MAX),
            work: std::sync::Mutex::new(None),
            partial_wanted: AtomicBool::new(false),
            partial: std::sync::Mutex::new((None, None)),
        }
    }
}

impl Progress {
    /// Ask the watched evaluation to stop at its next feature.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }
    /// Make the watched evaluation wait before feature `index` until it is cancelled: tests
    /// use it to act while a load is certainly still running, whatever the machine's speed.
    pub fn hold_at(&self, index: usize) {
        self.hold.store(index, Ordering::Relaxed);
    }
    /// Features evaluated so far and the timeline's length (0 before evaluation starts).
    pub fn features(&self) -> (usize, usize) {
        (self.done.load(Ordering::Relaxed), self.total.load(Ordering::Relaxed))
    }
    /// Ask the watched evaluation to offer the model as far as it is built, now and then.
    pub fn want_partial(&self) {
        self.partial_wanted.store(true, Ordering::Relaxed);
    }
    /// The model as far as it was built when last offered, once (`None` until there is a newer one).
    pub fn take_partial(&self) -> Option<Partial> {
        self.partial.lock().ok().and_then(|mut p| p.0.take())
    }
    /// Long work inside a feature, such as reading a large STEP file: (step, done, total);
    /// `None` while there is none to show.
    pub fn work(&self) -> Option<(&'static str, usize, usize)> {
        self.work.lock().ok().and_then(|w| *w)
    }
    fn report_work(&self, stage: &'static str, done: usize, total: usize) {
        if let Ok(mut work) = self.work.lock() {
            *work = Some((stage, done.min(total), total));
        }
    }
}

/// The kernel's long work (STEP import) reports to the same progress and stops on Cancel.
struct KernelWatch(Arc<Progress>);

impl solidcraft_kernel::watch::Watch for KernelWatch {
    fn cancelled(&self) -> bool {
        self.0.is_cancelled()
    }
    fn progress(&self, stage: &'static str, done: usize, total: usize) {
        self.0.report_work(stage, done, total);
    }
}

struct Current {
    progress: Arc<Progress>,
    /// Nested evaluations (derived designs) don't overwrite the outer timeline's count.
    depth: usize,
}

thread_local! {
    static CURRENT: RefCell<Option<Current>> = const { RefCell::new(None) };
}

/// Report a step of file work on this thread. True when its watcher asked to stop.
/// Without a watch, work continues without recording progress.
pub fn work_step(stage: &'static str, done: usize, total: usize) -> bool {
    CURRENT.with(|c| {
        let c = c.borrow();
        let Some(cur) = c.as_ref() else { return false };
        cur.progress.report_work(stage, done, total);
        cur.progress.is_cancelled()
    })
}

/// Run `f` with `progress` watching every evaluation it makes on this thread.
pub fn watch<R>(progress: Arc<Progress>, f: impl FnOnce() -> R) -> R {
    let kernel = Arc::new(KernelWatch(progress.clone()));
    let previous = CURRENT.with(|c| c.replace(Some(Current { progress, depth: 0 })));
    struct Restore(Option<Option<Current>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            if let Some(previous) = self.0.take() {
                CURRENT.with(|c| *c.borrow_mut() = previous);
            }
        }
    }
    let _restore = Restore(Some(previous));
    solidcraft_kernel::watch::watching(kernel, f)
}

/// Entered at the start of an evaluation; the guard leaves it.
pub(crate) struct Evaluation {
    outer: bool,
}

pub(crate) fn begin(total: usize) -> Evaluation {
    let outer = CURRENT.with(|c| {
        let mut c = c.borrow_mut();
        let Some(cur) = c.as_mut() else { return false };
        cur.depth += 1;
        if cur.depth == 1 {
            cur.progress.total.store(total, Ordering::Relaxed);
            cur.progress.done.store(0, Ordering::Relaxed);
            true
        } else {
            false
        }
    });
    Evaluation { outer }
}

impl Evaluation {
    /// Feature `index` is next. True when the watcher asked to stop: skip it.
    pub(crate) fn step(&self, index: usize) -> bool {
        CURRENT.with(|c| {
            let c = c.borrow();
            let Some(cur) = c.as_ref() else { return false };
            if self.outer {
                if let Ok(mut work) = cur.progress.work.lock() {
                    *work = None;
                }
                cur.progress.done.store(index, Ordering::Relaxed);
                while index == cur.progress.hold.load(Ordering::Relaxed) && !cur.progress.is_cancelled() {
                    #[cfg(not(target_arch = "wasm32"))]
                    std::thread::sleep(std::time::Duration::from_millis(1));
                    #[cfg(target_arch = "wasm32")]
                    std::hint::spin_loop();
                }
            }
            cur.progress.is_cancelled()
        })
    }
}

impl Evaluation {
    /// The outer evaluation has built the model up to feature `features`: a watcher that wants
    /// partial models gets it, placed by `doc`, at most every [`PARTIAL_EVERY`] (the first one
    /// after one interval, so a design that opens quickly offers none).
    pub(crate) fn offer(&self, features: usize, doc: &crate::Document, state: &Arc<crate::ModelState>) {
        if !self.outer {
            return;
        }
        let progress = CURRENT.with(|c| c.borrow().as_ref().map(|cur| cur.progress.clone()));
        let Some(progress) = progress.filter(|p| p.partial_wanted.load(Ordering::Relaxed) && !p.is_cancelled()) else { return };
        let now = web_time::Instant::now();
        let due = match progress.partial.lock() {
            Ok(mut p) => match p.1 {
                None => {
                    p.1 = Some(now);
                    false
                }
                Some(last) => now.duration_since(last) >= PARTIAL_EVERY,
            },
            Err(_) => false,
        };
        if !due {
            return;
        }
        let world = Arc::new(crate::world_state(doc, state));
        if let Ok(mut p) = progress.partial.lock() {
            *p = (Some(Partial { features, world }), Some(web_time::Instant::now()));
        }
    }
}

impl Drop for Evaluation {
    fn drop(&mut self) {
        CURRENT.with(|c| {
            if let Some(cur) = c.borrow_mut().as_mut() {
                cur.depth = cur.depth.saturating_sub(1);
                if self.outer {
                    let total = cur.progress.total.load(Ordering::Relaxed);
                    cur.progress.done.store(total, Ordering::Relaxed);
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unwatched_evaluations_never_stop_and_watched_ones_report_and_stop() {
        let e = begin(3);
        assert!(!e.step(0));
        drop(e);
        let p = Arc::new(Progress::default());
        watch(p.clone(), || {
            let e = begin(5);
            assert!(!e.step(2));
            assert_eq!(p.features(), (2, 5));
            {
                let inner = begin(100);
                assert!(!inner.step(50));
            }
            assert_eq!(p.features(), (2, 5), "a nested evaluation keeps the outer count");
            p.cancel();
            assert!(e.step(3));
        });
        assert_eq!(p.features(), (5, 5));
        assert!(!begin(1).step(0), "the watch ends with its scope");
    }

    /// A watcher that asks for partial models gets the model built so far, placed, now and then;
    /// one that doesn't ask, or an unwatched evaluation, never pays for it.
    #[test]
    fn partial_models_go_only_to_a_watcher_that_wants_them() {
        let mut doc = crate::Document::new("partial");
        let block = crate::FeatureKind::Box {
            corner: crate::point_expr(solidcraft_geom::Vec3::ZERO),
            length: "2 mm".into(),
            width: "3 mm".into(),
            height: "4 mm".into(),
            operation: crate::Operation::NewBody,
        };
        doc.add_feature(block, Some("Block")).unwrap();
        let mut model = crate::Model::default();
        model.evaluate(&doc);
        let state = model.state();
        let offer = |p: &Arc<Progress>| {
            watch(p.clone(), || {
                let e = begin(1);
                e.offer(1, &doc, &state);
            })
        };
        let p = Arc::new(Progress::default());
        offer(&p);
        assert!(p.take_partial().is_none(), "not asked for");
        p.want_partial();
        offer(&p);
        assert!(p.take_partial().is_none(), "nothing in the first interval: a quick open offers none");
        if let Ok(mut g) = p.partial.lock() {
            g.1 = web_time::Instant::now().checked_sub(PARTIAL_EVERY * 2);
        }
        offer(&p);
        let partial = p.take_partial().unwrap();
        assert_eq!(partial.features, 1);
        assert_eq!(partial.world.bodies.len(), 1);
        assert!(p.take_partial().is_none(), "taken once");
        offer(&p);
        assert!(p.take_partial().is_none(), "not again within the interval");
    }
}
