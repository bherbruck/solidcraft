//! Dragging sketch geometry without jumping to another solution.
//!
//! A constrained sketch usually has several solutions for one cursor position (a two-link arm's
//! elbow up or down, a fillet inside or outside its corner). Each held solve of a drag is seeded
//! on the positions from before it: the points the solver's construction places (from distances
//! and alignments to placed points) keep the orientation they had there however far the step.
//! The rest is left to the iteration, which from far away can land on any solution, so a long
//! drag still walks from where the geometry is to where the cursor puts it in short steps, each
//! solved from the previous step's result (continuation). Continuation is what keeps lengths the
//! construction does not read (an Equal to another line, a rigid shape hanging off a held point);
//! it also lets the arc sweep guard see every step, and lets a path that crosses where the
//! geometry cannot reach stop short at the edge of that stretch and go on past it. It steps only
//! when the move is long next to the dragged geometry, so a normal mouse frame is a single solve.

use std::f64::consts::{PI, TAU};

use solidcraft_geom::Vec2;
use solidcraft_sketch::{Analysis, Budget, ConstraintKind, CurveKind, Hold, Sketch, conic_rho_from_point};

use super::{Ent, Left, counted_solve};

/// Longest step, as a fraction of the smallest driving length or radius of the dragged group.
/// Each step is seeded on its pre-step positions, so what the seed constructs keeps its branch
/// whatever the step; the steps are for what it does not (an arc's sweep guard, paths across
/// unreachable stretches). The drag tests first fail at 1 (an arc end wraps through its start):
/// this keeps a factor of four. It holds where the seed placed what it could: once a held
/// solve's decomposition runs out of budget (see [`Budget::exhausted`]), the rest of the drag
/// takes `SHORT_STEP_FRACTION` steps, and continuation keeps the branch.
const STEP_FRACTION: f64 = 0.25;
/// The steps of a drag whose seed did not place everything it could (as before seeding steps).
const SHORT_STEP_FRACTION: f64 = 0.125;
/// Shortest step (mm), so a tiny feature cannot make a drag crawl.
const MIN_STEP: f64 = 1e-3;
/// Most steps one drag is split into.
const MAX_STEPS: usize = 64;
/// After a step fails, the way to it from the last solved state is halved until what is left is
/// shorter than a step over this; at most `MAX_STEPS` such solves per drag.
const HALVINGS: usize = 16;
/// An arc whose sweep gets this close (radians) to 0 or a full turn has run into its start.
const SWEEP_EPS: f64 = 1e-6;

/// What a drag moves, from where it is (`t = 0`) to where the cursor puts it (`t = 1`).
enum Path {
    /// Points slide by `d` (a dragged point by its way to the cursor).
    Slide { pts: Vec<(usize, Vec2)>, d: Vec2 },
    /// A circle's radius or an ellipse's minor radius follows the cursor.
    Radius { c: usize, from: f64, to: f64 },
    /// An arc's ends move along their rays from the centre `o` to the cursor's radius.
    ArcRadius { o: Vec2, ends: Vec<(usize, Vec2)>, from: f64, to: f64 },
    /// A conic's `rho` follows the cursor, read off its position between the chord's midpoint
    /// and the apex (see [`solidcraft_sketch::conic_rho_from_point`]). `a`, `b` and `apex` stay
    /// put; only the curve's fullness changes, the way an ellipse's outline changes its minor
    /// radius rather than sliding the ellipse.
    ConicRho { c: usize, from: f64, to: f64 },
}

impl Path {
    fn length(&self) -> f64 {
        match self {
            Path::Slide { d, .. } => d.len(),
            Path::Radius { from, to, .. } | Path::ArcRadius { from, to, .. } | Path::ConicRho { from, to, .. } => (to - from).abs(),
        }
    }

    /// The points the drag moves.
    fn points(&self) -> impl Iterator<Item = usize> + '_ {
        let pts = match self {
            Path::Slide { pts, .. } | Path::ArcRadius { ends: pts, .. } => pts.as_slice(),
            Path::Radius { .. } | Path::ConicRho { .. } => &[],
        };
        pts.iter().map(|(q, _)| *q)
    }

    /// Put the dragged geometry where it is at `t` along the drag.
    fn apply(&self, sk: &mut Sketch, t: f64) {
        let mut set = |q: usize, at: Vec2| {
            if let Some(pt) = sk.points.get_mut(q) {
                pt.pos = at;
            }
        };
        match self {
            Path::Slide { pts, d } => {
                for &(q, at) in pts {
                    set(q, at + *d * t);
                }
            }
            Path::ArcRadius { o, ends, from, to } => {
                let r = from + (to - from) * t;
                for &(q, u) in ends {
                    set(q, *o + u * r);
                }
            }
            Path::Radius { c, from, to } => {
                if let Some(CurveKind::Circle { r, .. } | CurveKind::Ellipse { r, .. }) = sk.curves.get_mut(*c).map(|x| &mut x.kind) {
                    *r = from + (to - from) * t;
                }
            }
            Path::ConicRho { c, from, to } => {
                if let Some(CurveKind::Conic { rho, .. }) = sk.curves.get_mut(*c).map(|x| &mut x.kind) {
                    *rho = from + (to - from) * t;
                }
            }
        }
    }
}

/// Drag: the grabbed entity follows the cursor exactly (a point goes to it; a line or spline
/// slides by the cursor's move; a circle or arc grabbed on its edge changes radius about its
/// centre; a conic grabbed on its curve reads its fullness, `rho`, off the cursor) and the rest
/// of the sketch moves as little as it can, staying on the
/// solution it started on. Fixed geometry stays. A step that cannot be solved with the entity
/// held (the path crosses where the geometry cannot reach) is approached by halving the way to
/// it, and the next steps are tried from the last solved state, the last of them being the whole
/// remaining move. When none past it solves, the first failed step stays placed but unsolved:
/// the edit's ordinary, non-strict solve that follows (seeded on the last solved state, see
/// [`Left::MovedFrom`]) then pulls it back (fully constrained geometry) or leaves the sketch
/// unsolved for the user to see. A step that would sweep an arc through its own start is undone
/// and the drag ends there. Returns how far it got and what it leaves for the edit's solve
/// ([`Left::Solved`]: the held solve of the last step it kept left the sketch solved).
///
/// `live`: one frame of an interactive drag, whose held solves share one frame's decomposition
/// budget ([`Budget::frame`]); otherwise each has a solve's own.
pub(super) fn drag_entity(
    sk: &mut Sketch,
    e: Ent,
    from: Vec2,
    to: Vec2,
    live: bool,
    solved: Option<&solidcraft_doc::SolvedSketch>,
) -> (Dragged, Left) {
    // An unchanged, solved entity with no freedom cannot follow a drag. Holding it at a
    // different position only makes its constraints conflict, and a large pattern would spend
    // minutes trying dense solves before returning to the same geometry (#2006).
    if solved.is_some_and(|ss| {
        ss.report.ok()
            && ss.sketch == *sk
            && match e {
                Ent::Point(q) => ss.report.point_determined.get(q),
                Ent::Curve(c) => ss.report.curve_determined.get(c),
            }
            .copied()
            .unwrap_or(false)
    }) {
        return (Dragged { reached: 0.0, moving: Vec::new() }, Left::Solved);
    }
    let Some((path, hold, n, short)) = prepare(sk, e, from, to) else {
        return (Dragged { reached: 0.0, moving: Vec::new() }, Left::Unsolved);
    };
    #[cfg(test)]
    let (n, short) = (n.min(step_cap::get()), short.min(step_cap::get()));
    let moving = path.points().filter_map(|q| Some((q, sk.point(q)?))).collect();
    let mut frame = live.then(frame_budget);
    // Only success matters to these solves: the degrees of freedom of a sketch with the entity
    // held are not the sketch's. Each is seeded on the last solved state (`last`), so what the
    // constraints pin down keeps the orientation it had there. A failed or wrapping step is
    // undone (a failed solve leaves the step placed). Also returns whether the decomposition ran
    // out of budget.
    let mut solves = Solves::default();
    let mut step = |sk: &mut Sketch, last: &Geometry, t: f64, solves: &mut Solves| {
        path.apply(sk, t);
        let mut own = Budget::full();
        let budget = frame.as_mut().unwrap_or(&mut own);
        let spent = budget.spent();
        let ok = counted_solve(sk, &hold, Some(&last.pos), Analysis::StatusOnly, Some(&mut *budget)).ok();
        solves.count += 1;
        solves.work += budget.spent() - spent;
        solves.cut |= budget.exhausted();
        let outcome = if !ok {
            Step::Failed
        } else if last.sweep_wrapped(sk) {
            Step::Wrapped
        } else {
            return Step::Solved;
        };
        last.restore(sk);
        outcome
    };
    let mut last = Geometry::default();
    last.take(sk);
    // How far along the last solved state is; the first step since it that failed.
    let (mut good, mut failed) = (0.0, None);
    let mut spare = MAX_STEPS;
    // The steps: `k` of `dt` from `base`; once a seed ran out of budget, the rest of the way in
    // short steps.
    let (mut base, mut dt, mut k, mut t) = (0.0, 1.0 / n as f64, 0, 0.0);
    let mut shortened = false;
    'walk: while t < 1.0 {
        if solves.cut && !shortened {
            shortened = true;
            let rest = 1.0 - t;
            let m = steps(rest * short as f64, 1.0);
            (base, dt, k) = (t, rest / m as f64, 0);
        }
        k += 1;
        t = base + dt * k as f64;
        // The last step ends exactly at the cursor.
        if 1.0 - t < 0.5 * dt {
            t = 1.0;
        }
        match step(sk, &last, t, &mut solves) {
            Step::Solved => {
                (good, failed) = (t, None);
                if t < 1.0 {
                    last.take(sk);
                }
                continue;
            }
            Step::Wrapped => {
                failed = None;
                break 'walk;
            }
            Step::Failed => {}
        }
        if failed.is_none() {
            // Halve the way to the failed step, so the state the next steps start from (and are
            // seeded on) is as close to it as the geometry follows. The shortest move is tried
            // first: geometry that cannot move at all (pinned) costs one solve, not every halving.
            let fine = dt / HALVINGS as f64;
            let mut miss = t;
            let mut first = true;
            while spare > 0 && miss - good > fine {
                spare -= 1;
                let mid = if std::mem::take(&mut first) { good + fine } else { 0.5 * (good + miss) };
                match step(sk, &last, mid, &mut solves) {
                    Step::Solved => {
                        good = mid;
                        last.take(sk);
                    }
                    Step::Wrapped => break 'walk,
                    Step::Failed => miss = mid,
                }
            }
            failed = Some(t);
        }
    }
    #[cfg(test)]
    last_drag::set(solves);
    let left = match failed {
        // The failed step is left placed for the edit's solve, seeded on the last solved state.
        Some(t) => {
            path.apply(sk, t);
            Left::MovedFrom(last.pos)
        }
        None if good > 0.0 => Left::Solved,
        None => Left::Unsolved,
    };
    (Dragged { reached: good, moving }, left)
}

/// The held solves of one drag: how many, the decomposition work they did, and whether one ran
/// out of budget.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct Solves {
    pub count: usize,
    pub work: usize,
    pub cut: bool,
}

/// One live frame's decomposition budget (smaller in tests that set one).
fn frame_budget() -> Budget {
    #[cfg(test)]
    if let Some(work) = last_drag::frame_work() {
        return Budget::new(work);
    }
    Budget::frame()
}

/// What the last drag on this thread did, and a frame budget for tests to set.
#[cfg(test)]
pub(super) mod last_drag {
    use std::cell::Cell;

    use super::Solves;

    thread_local! {
        static LAST: Cell<Solves> = const { Cell::new(Solves { count: 0, work: 0, cut: false }) };
        static FRAME: Cell<Option<usize>> = const { Cell::new(None) };
    }

    pub fn set(s: Solves) {
        LAST.with(|c| c.set(s));
    }

    /// The held solves of the last drag.
    pub fn get() -> Solves {
        LAST.with(Cell::get)
    }

    pub fn frame_work() -> Option<usize> {
        FRAME.with(Cell::get)
    }

    /// Restores the frame budget when dropped, also when the test panics.
    struct Restore(Option<usize>);

    impl Drop for Restore {
        fn drop(&mut self) {
            FRAME.with(|c| c.set(self.0));
        }
    }

    /// Run `f` with live frames given `work` units of decomposition budget.
    pub fn with_frame_work<R>(work: usize, f: impl FnOnce() -> R) -> R {
        let _restore = Restore(FRAME.with(|c| c.replace(Some(work))));
        f()
    }
}

/// How one held solve of a drag went.
enum Step {
    Solved,
    Failed,
    /// Solved, but it swept an arc through its own start.
    Wrapped,
}

/// How far a drag got.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Dragged {
    /// The part of the way from where it was grabbed to the cursor the geometry followed (0..1).
    pub reached: f64,
    /// The points the drag moves (not the fixed ones it holds), where they were before it.
    pub moving: Vec<(usize, Vec2)>,
}

/// The geometry a step changes (point positions; curves, for radii and the arc ends a solve
/// may swap), kept to undo a step. Its buffers are reused from step to step.
#[derive(Default)]
struct Geometry {
    pos: Vec<Vec2>,
    curves: Vec<(CurveKind, bool)>,
}

impl Geometry {
    fn take(&mut self, sk: &Sketch) {
        self.pos.clear();
        self.pos.extend(sk.points.iter().map(|p| p.pos));
        self.curves.clear();
        self.curves.extend(sk.curves.iter().map(|c| (c.kind.clone(), c.reversed)));
    }

    fn restore(&self, sk: &mut Sketch) {
        for (p, at) in sk.points.iter_mut().zip(&self.pos) {
            p.pos = *at;
        }
        for (c, (kind, reversed)) in sk.curves.iter_mut().zip(&self.curves) {
            c.kind = kind.clone();
            c.reversed = *reversed;
        }
    }

    /// Did the step drive some arc's end through its start: the sweep jumped through 0 / a full
    /// turn, or ran from clear of them onto one? An arc already that close may still move.
    /// Measured on the arc's ends as they were before the step (a solve may swap them).
    fn sweep_wrapped(&self, sk: &Sketch) -> bool {
        let was = |q: usize| self.pos.get(q).copied();
        let now = |q: usize| sk.point(q);
        let clear = |s: f64| (SWEEP_EPS..=TAU - SWEEP_EPS).contains(&s);
        self.curves.iter().any(|(kind, _)| {
            let CurveKind::Arc { c, a, b } = *kind else { return false };
            let (Some(s0), Some(s1)) = (sweep(was(c), was(a), was(b)), sweep(now(c), now(a), now(b))) else { return false };
            (s1 - s0).abs() > PI || (clear(s0) && !clear(s1))
        })
    }
}

/// The held solves dragging `e` from `from` to `to` takes (0 when it cannot move): the plan
/// `drag_entity` runs, for tests.
#[cfg(test)]
pub(super) fn drag_steps(sk: &Sketch, e: Ent, from: Vec2, to: Vec2) -> usize {
    prepare(sk, e, from, to).map_or(0, |(_, _, n, _)| n)
}

/// A drag's path, what its solves hold and its number of steps (and of short steps, see
/// `SHORT_STEP_FRACTION`), or `None` if it cannot move.
fn prepare(sk: &Sketch, e: Ent, from: Vec2, to: Vec2) -> Option<(Path, Hold, usize, usize)> {
    if !(from.is_finite() && to.is_finite()) {
        return None;
    }
    let (path, hold) = plan(sk, e, from, to)?;
    let n = step_count(sk, &hold, path.length(), STEP_FRACTION);
    let short = step_count(sk, &hold, path.length(), SHORT_STEP_FRACTION);
    Some((path, hold, n, short))
}

/// A cap on the steps `drag_entity` takes on this thread, for tests that measure what fewer
/// steps keep (the planned count, `drag_steps`, is not capped).
#[cfg(test)]
pub(super) mod step_cap {
    use std::cell::Cell;

    thread_local! {
        static CAP: Cell<usize> = const { Cell::new(super::MAX_STEPS) };
    }

    pub fn get() -> usize {
        CAP.with(Cell::get)
    }

    /// Restores the cap when dropped, also when the test panics.
    struct Restore(usize);

    impl Drop for Restore {
        fn drop(&mut self) {
            CAP.with(|c| c.set(self.0));
        }
    }

    /// Run `f` with drags split into at most `cap` steps (at least one).
    pub fn with<R>(cap: usize, f: impl FnOnce() -> R) -> R {
        let _restore = Restore(CAP.with(|c| c.replace(cap.max(1))));
        f()
    }
}

/// Steps of at most `step` covering `length`: at least one, at most `MAX_STEPS`.
fn steps(length: f64, step: f64) -> usize {
    let n = (length / step).ceil();
    if n.is_finite() && n >= 1.0 { (n as usize).min(MAX_STEPS) } else { 1 }
}

/// The dragged geometry's path and what the solves hold, or `None` if it cannot move.
fn plan(sk: &Sketch, e: Ent, from: Vec2, to: Vec2) -> Option<(Path, Hold)> {
    let mut hold = Hold::default();
    let path = match e {
        Ent::Point(q) => {
            if sk.point_locked(q) {
                return None;
            }
            hold.points.push(q);
            let at = sk.point(q)?;
            Path::Slide { pts: vec![(q, at)], d: to - at }
        }
        Ent::Curve(c) => {
            if sk.curve_locked(c) {
                return None;
            }
            let kind = sk.curves.get(c).map(|x| x.kind.clone())?;
            let locked = sk.locked_points();
            let free = |q: &usize| !locked.get(*q).copied().unwrap_or(true);
            // A cursor on the centre leaves the radius as it is.
            let radius_to = |o: Vec2, r: f64| Some(o.dist(to)).filter(|x| *x > 1e-9).unwrap_or(r);
            match kind {
                CurveKind::Circle { c: cc, r } => {
                    let to_r = sk.point(cc).map_or(r, |o| radius_to(o, r));
                    hold.points.push(cc);
                    hold.radii.push(c);
                    Path::Radius { c, from: r, to: to_r }
                }
                CurveKind::Arc { c: cc, a, b } => {
                    let o = sk.point(cc)?;
                    let r = sk.radius(c).unwrap_or(0.0);
                    hold.points.extend([cc, a, b]);
                    let ends = [a, b].into_iter().filter(free).filter_map(|q| Some((q, (sk.point(q)? - o).normalized()?))).collect();
                    Path::ArcRadius { o, ends, from: r, to: radius_to(o, r) }
                }
                CurveKind::Ellipse { c: cc, m, r } => {
                    // Grabbed on the outline: the minor radius follows (the major stays).
                    let (o, pm) = (sk.point(cc)?, sk.point(m)?);
                    let u = (pm - o).normalized().unwrap_or(Vec2::X);
                    let minor = u.cross(to - o).abs();
                    hold.points.extend([cc, m]);
                    hold.radii.push(c);
                    Path::Radius { c, from: r, to: if minor > 1e-9 { minor } else { r } }
                }
                CurveKind::Conic { a, b, apex, rho } => {
                    // Grabbed on the curve itself: its fullness (`rho`) follows the drag, the
                    // way an ellipse's outline changes its minor radius rather than sliding the
                    // ellipse. `a`, `b` and the apex (the tangent intersection, dragged
                    // separately as an ordinary point) stay where they are.
                    let (pa, px, pb) = (sk.point(a)?, sk.point(apex)?, sk.point(b)?);
                    hold.points.extend([a, b, apex]);
                    let to_rho = conic_rho_from_point(pa, px, pb, to);
                    Path::ConicRho { c, from: rho, to: to_rho }
                }
                k => {
                    // Lines and splines slide with the cursor.
                    let mut ids = k.point_ids();
                    ids.sort_unstable();
                    ids.dedup();
                    let pts = ids.iter().filter(|q| free(q)).filter_map(|&q| Some((q, sk.point(q)?))).collect();
                    hold.points.extend(ids);
                    Path::Slide { pts, d: to - from }
                }
            }
        }
    };
    Some((path, hold))
}

/// Steps for a move of `length`: enough that none is longer than `fraction` of the smallest
/// size of the geometry the solve moves with what it holds (at least `MIN_STEP`), at most
/// `MAX_STEPS`. A move short next to every size in the sketch (a normal mouse frame) is one
/// step, found without grouping the sketch.
fn step_count(sk: &Sketch, hold: &Hold, length: f64, fraction: f64) -> usize {
    let step = |size: f64| (size * fraction).max(MIN_STEP);
    if length <= step(smallest_size(sk, hold, None)) {
        return 1;
    }
    let (points, radii) = sk.coupled(&hold.points, &hold.radii);
    steps(length, step(smallest_size(sk, hold, Some(Group { points: &points, radii: &radii }))))
}

/// What a solve moves together: per point and per curve radius.
#[derive(Clone, Copy)]
struct Group<'a> {
    points: &'a [bool],
    radii: &'a [bool],
}

impl Group<'_> {
    /// Does an entity with these points and curves belong to the group?
    fn has(&self, points: &[usize], curves: &[usize]) -> bool {
        let any = |flags: &[bool], ids: &[usize]| ids.iter().any(|i| flags.get(*i).copied().unwrap_or(false));
        any(self.points, points) || any(self.radii, curves)
    }
}

/// The smallest size that can make a constrained shape take another solution (of the geometry
/// in `group`, or of the whole sketch), or infinity when there is none: every length the
/// constraints hold. That is driving length dimensions (offsets along an axis or from a line
/// too: the solver holds their size, either side), radii and the current lengths of lines held
/// equal to another (a link's length can come from an Equal as well as a dimension). A line that
/// only keeps a direction (a rectangle's side) holds no length: it may pass through zero and turn
/// over, so it does not shrink the steps. Nor does a distance between two held points (the
/// dragged line's own length): held geometry moves as one. A circle's radius counts when the
/// solve moves the radius itself (not merely its centre).
fn smallest_size(sk: &Sketch, hold: &Hold, group: Option<Group>) -> f64 {
    // Point lists are built only to test a size smaller than the smallest so far against the
    // group; without a group nothing is built.
    let in_group = |points: &dyn Fn() -> Vec<usize>, curves: &[usize]| group.is_none_or(|g| g.has(&points(), curves));
    // A radius is the circle's own variable, or (an arc's) the distance between its points.
    let radius_in_group = |c: usize| match sk.curves.get(c).map(|x| &x.kind) {
        Some(CurveKind::Circle { .. } | CurveKind::Ellipse { .. }) => in_group(&Vec::new, &[c]),
        Some(k) => in_group(&|| k.point_ids(), &[]),
        None => false,
    };
    let line_length = |l: usize| super::line_pts(sk, l).map(|(a, b)| a.dist(b));
    let points = |k: &ConstraintKind| sk.constraint_points(k);
    let mut min = f64::INFINITY;
    let mut take = |size: f64, counts: &dyn Fn() -> bool| {
        let size = size.abs();
        if size.is_finite() && size > 1e-9 && size < min && counts() {
            min = size;
        }
    };
    for k in sk.constraints.iter().filter(|k| !k.driven).map(|k| &k.kind) {
        match *k {
            ConstraintKind::Equal { a, b } => {
                if let Some(v) = line_length(a).into_iter().chain(line_length(b)).reduce(f64::min) {
                    take(v, &|| in_group(&|| points(k), &[]));
                }
            }
            ConstraintKind::Radius { c, value } => take(value, &|| radius_in_group(c)),
            ConstraintKind::Diameter { c, value } => take(value / 2.0, &|| radius_in_group(c)),
            _ => {
                if let Some(v) = driving_length(k) {
                    // Held points move as one: a length between them never changes.
                    take(v, &|| {
                        let pts = points(k);
                        !pts.iter().all(|p| hold.points.contains(p)) && in_group(&|| pts.clone(), &k.indices().1)
                    });
                }
            }
        }
    }
    for c in 0..sk.curves.len() {
        if let Some(r) = sk.radius(c) {
            take(r, &|| radius_in_group(c));
        }
    }
    min
}

/// The length a driving dimension sets (a linear diameter's radius), if it sets one.
fn driving_length(k: &ConstraintKind) -> Option<f64> {
    if k.is_angle() {
        return None;
    }
    let v = k.value()?;
    Some(if matches!(k, ConstraintKind::LinearDiameter { .. }) { v / 2.0 } else { v })
}

/// Counter-clockwise sweep (radians, `[0, 2π)`) from `a` to `b` about `c`.
fn sweep(c: Option<Vec2>, a: Option<Vec2>, b: Option<Vec2>) -> Option<f64> {
    let (o, pa, pb) = (c?, a?, b?);
    Some(((pb - o).angle() - (pa - o).angle()).rem_euclid(TAU))
}
