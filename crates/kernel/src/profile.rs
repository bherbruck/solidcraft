//! Where a command's time goes inside the kernel (#1833): wall time in booleans and in
//! tessellation, measured on this thread while [`capture`] runs and not at all otherwise (one
//! thread-local check per timed call).
//!
//! Only the outermost timed call counts: a tessellation a boolean does to check its result is
//! the boolean's time, so the two never add up to more than the command took. Work a call hands
//! to other threads is inside that call's wall time.

use std::cell::Cell;

/// Kernel time spent while a [`capture`] ran, in milliseconds.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct KernelTimes {
    pub boolean_ms: f64,
    pub tessellate_ms: f64,
}

/// What a timed call is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Boolean,
    Tessellate,
}

#[derive(Clone, Copy, Default)]
struct State {
    /// A capture is running on this thread.
    on: bool,
    /// A timed call is running (nested ones aren't timed).
    busy: bool,
    times: KernelTimes,
}

thread_local! {
    static STATE: Cell<State> = const { Cell::new(State { on: false, busy: false, times: KernelTimes { boolean_ms: 0.0, tessellate_ms: 0.0 } }) };
}

/// Run `f`, and the kernel time it spent in booleans and tessellation on this thread. Captures
/// nest: an inner capture's time is the outer one's too.
pub fn capture<T>(f: impl FnOnce() -> T) -> (T, KernelTimes) {
    struct Restore(State);
    impl Drop for Restore {
        fn drop(&mut self) {
            let inner = STATE.with(Cell::get).times;
            let mut outer = self.0;
            if outer.on {
                outer.times.boolean_ms += inner.boolean_ms;
                outer.times.tessellate_ms += inner.tessellate_ms;
            }
            STATE.with(|s| s.set(outer));
        }
    }
    let outer = STATE.with(Cell::get);
    STATE.with(|s| s.set(State { on: true, busy: outer.busy, times: KernelTimes::default() }));
    let restore = Restore(outer);
    let r = f();
    let times = STATE.with(Cell::get).times;
    drop(restore);
    (r, times)
}

/// Time `f` as `kind` when a capture is running and no timed call is.
pub(crate) fn timed<T>(kind: Kind, f: impl FnOnce() -> T) -> T {
    let st = STATE.with(Cell::get);
    if !st.on || st.busy {
        return f();
    }
    struct Busy {
        kind: Kind,
        started: web_time::Instant,
    }
    impl Drop for Busy {
        fn drop(&mut self) {
            let ms = self.started.elapsed().as_secs_f64() * 1000.0;
            STATE.with(|s| {
                let mut st = s.get();
                st.busy = false;
                match self.kind {
                    Kind::Boolean => st.times.boolean_ms += ms,
                    Kind::Tessellate => st.times.tessellate_ms += ms,
                }
                s.set(st);
            });
        }
    }
    STATE.with(|s| s.set(State { busy: true, ..st }));
    let busy = Busy { kind, started: web_time::Instant::now() };
    let r = f();
    drop(busy);
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spin(ms: u64) {
        let t = web_time::Instant::now();
        while t.elapsed().as_millis() < u128::from(ms) {}
    }

    #[test]
    fn nothing_is_timed_outside_a_capture() {
        timed(Kind::Boolean, || spin(2));
        let ((), t) = capture(|| ());
        assert_eq!(t, KernelTimes::default());
    }

    #[test]
    fn only_the_outermost_call_counts() {
        let started = web_time::Instant::now();
        let ((), t) = capture(|| {
            timed(Kind::Boolean, || {
                spin(3);
                timed(Kind::Tessellate, || spin(3));
            });
            timed(Kind::Tessellate, || spin(2));
        });
        let elapsed = started.elapsed().as_secs_f64() * 1000.0;
        assert!(t.boolean_ms >= 6.0 && t.tessellate_ms >= 2.0, "{t:?}");
        // The tessellation inside the boolean is not counted twice.
        assert!(t.boolean_ms + t.tessellate_ms <= elapsed + 1e-6, "{t:?} in {elapsed} ms");
    }

    #[test]
    fn inner_captures_add_to_the_outer_one_and_a_panic_restores_it() {
        let ((), outer) = capture(|| {
            let ((), inner) = capture(|| timed(Kind::Boolean, || spin(2)));
            assert!(inner.boolean_ms >= 2.0);
            let _ = std::panic::catch_unwind(|| capture(|| timed(Kind::Tessellate, || panic!("kernel"))));
        });
        assert!(outer.boolean_ms >= 2.0, "{outer:?}");
        let st = STATE.with(Cell::get);
        assert!(!st.on && !st.busy);
    }
}
