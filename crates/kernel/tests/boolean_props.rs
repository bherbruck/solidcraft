//! Property tests for the kernel boolean (`boolean`, `measure`, `transform`).
//!
//! Each test states one invariant that every correct B-rep boolean holds. proptest picks the
//! inputs, and when a case fails it shrinks the inputs to a small case that still fails. Paste
//! that case into a normal `#[test]` as a regression test.
//!
//! Case count: `PROPTEST_CASES` (default 8). Pull requests and pushes to main skip this file
//! (.config/nextest.toml, .github/workflows/ci.yml): its random inputs differ on every run, so a
//! known kernel bug would turn main red at random. The nightly `property-tests` job runs it with
//! more cases. A long local run:
//! `PROPTEST_CASES=2000 cargo test -p solidcraft-kernel --test boolean_props`.
//! proptest saves every failing seed in `tests/boolean_props.proptest-regressions`. Commit it, and
//! copy the shrunk case into a `#[test]` below (ignored, with its issue, until the kernel is fixed).

use proptest::prelude::*;
use solidcraft_geom::Vec3;
use solidcraft_kernel::{Body, BoolOp, boolean, box_solid, cylinder, measure, sphere, transform};

/// Relative volume tolerance. The same 0.2 % that `fuzz::boolean_case` uses.
const REL: f64 = 2e-3;

#[derive(Clone, Debug)]
enum Prim {
    Box { at: [f64; 3], size: [f64; 3] },
    Cyl { at: [f64; 3], axis: usize, r: f64, h: f64 },
    Sph { at: [f64; 3], r: f64 },
}

impl Prim {
    fn body(&self) -> Body {
        let v = |p: [f64; 3]| Vec3::new(p[0], p[1], p[2]);
        let axes = [Vec3::X, Vec3::Y, Vec3::Z];
        match *self {
            Prim::Box { at, size } => box_solid(v(at), v([at[0] + size[0], at[1] + size[1], at[2] + size[2]])),
            Prim::Cyl { at, axis, r, h } => cylinder(v(at), axes[axis], r, h),
            Prim::Sph { at, r } => sphere(v(at), r),
        }
        .expect("a primitive with positive size must build")
    }
}

/// Coordinates on a 2.5 mm grid: coincident faces, shared edges and tangencies come up often.
/// These are the cases that break B-rep booleans.
fn grid() -> impl Strategy<Value = f64> + Clone {
    (0u32..=12).prop_map(|i| i as f64 * 2.5)
}
/// Size on the grid, never zero.
fn grid_size() -> impl Strategy<Value = f64> + Clone {
    (2u32..=12).prop_map(|i| i as f64 * 2.5)
}
/// Generic coordinates: almost never coincident. A failure here is a plain bug, not an edge case.
fn free() -> impl Strategy<Value = f64> + Clone {
    0.0f64..30.0
}
fn free_size() -> impl Strategy<Value = f64> + Clone {
    3.0f64..30.0
}

fn prim(c: impl Strategy<Value = f64> + Clone, s: impl Strategy<Value = f64> + Clone) -> impl Strategy<Value = Prim> {
    let p = [c.clone(), c.clone(), c];
    prop_oneof![
        5 => (p.clone(), [s.clone(), s.clone(), s.clone()]).prop_map(|(at, size)| Prim::Box { at, size }),
        3 => (p.clone(), 0usize..3, s.clone(), s.clone()).prop_map(|(at, axis, r, h)| Prim::Cyl { at, axis, r: r / 2.0, h }),
        2 => (p, s).prop_map(|(at, r)| Prim::Sph { at, r: r / 2.0 }),
    ]
}
fn grid_prim() -> impl Strategy<Value = Prim> {
    prim(grid(), grid_size())
}

/// A union across only a box edge or corner is non-manifold, so the kernel may
/// refuse it. This is the same analytic boundary case allowed by box_box_exact.
fn edge_only_boxes(a: &Prim, b: &Prim) -> bool {
    let (Prim::Box { at: a, size: sa }, Prim::Box { at: b, size: sb }) = (a, b) else { return false };
    let gap = [0, 1, 2].map(|i| a[i].max(b[i]) - (a[i] + sa[i]).min(b[i] + sb[i]));
    gap.iter().all(|g| *g <= 0.0) && gap.iter().filter(|g| **g == 0.0).count() >= 2
}

fn edge_contact_refusal(a: &Prim, b: &Prim, op: BoolOp, result: &Result<f64, TestCaseError>) -> bool {
    op == BoolOp::Union && edge_only_boxes(a, b) && result.as_ref().is_err_and(|e| e.to_string().contains("zero-thickness edge contact"))
}
fn free_prim() -> impl Strategy<Value = Prim> {
    prim(free(), free_size())
}

/// Volume of a boolean result (an empty result has volume 0). Also fails the case when the
/// result is not a sound solid.
fn run(a: &Body, b: &Body, op: BoolOp) -> Result<f64, TestCaseError> {
    let r = boolean(a, b, op).map_err(|e| TestCaseError::fail(format!("{op:?} returned an error: {e}")))?;
    match r {
        None => Ok(0.0),
        Some(body) => {
            let bad = body.validity();
            prop_assert!(bad.is_empty(), "{op:?} result is not a sound solid: {}", bad.join("; "));
            Ok(measure(&body).map_err(|e| TestCaseError::fail(format!("measure: {e}")))?.volume)
        }
    }
}
fn vol(b: &Body) -> f64 {
    measure(b).expect("measure a primitive").volume
}
fn close(x: f64, y: f64, scale: f64) -> bool {
    (x - y).abs() <= REL * scale.max(1e-9)
}

/// Inclusion-exclusion: V(A∪B) + V(A∩B) = V(A) + V(B), and V(A−B) = V(A) − V(A∩B).
fn identity(a: &Prim, b: &Prim) -> Result<(), TestCaseError> {
    let (ba, bb) = (a.body(), b.body());
    let (va, vb) = (vol(&ba), vol(&bb));
    let (vu, vi, vc) = (run(&ba, &bb, BoolOp::Union), run(&ba, &bb, BoolOp::Intersect)?, run(&ba, &bb, BoolOp::Cut)?);
    if edge_contact_refusal(a, b, BoolOp::Union, &vu) {
        prop_assert!(close(vi, 0.0, va + vb), "edge-only contact must have no intersection volume");
    } else {
        let vu = vu?;
        prop_assert!(close(vu + vi, va + vb, va + vb), "union+intersect {} != A+B {} (A {va:.3}, B {vb:.3}, U {vu:.3}, I {vi:.3})", vu + vi, va + vb);
    }
    prop_assert!(close(vc, va - vi, va + vb), "cut {vc:.3} != A-I {:.3}", va - vi);
    Ok(())
}

fn cfg(default: u32) -> ProptestConfig {
    // The env var overrides the default; proptest reads PROPTEST_CASES itself only when the
    // config does not set `cases`, so read it here.
    let cases = std::env::var("PROPTEST_CASES").ok().and_then(|s| s.parse().ok()).unwrap_or(default);
    // Integration tests have no lib.rs or main.rs beside them, so proptest's default persistence
    // cannot place its file; name it.
    let file = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/boolean_props.proptest-regressions");
    ProptestConfig {
        cases,
        max_shrink_iters: 200,
        failure_persistence: Some(Box::new(prop::test_runner::FileFailurePersistence::Direct(file))),
        ..ProptestConfig::default()
    }
}

proptest! {
    #![proptest_config(cfg(8))]

    /// Generic placement: these must all pass. A failure is a real bug.
    #[test]
    fn identity_generic(a in free_prim(), b in free_prim()) {
        identity(&a, &b)?;
    }

    /// Grid placement: coincident and tangent faces. The hard cases.
    #[test]
    fn identity_on_grid(a in grid_prim(), b in grid_prim()) {
        identity(&a, &b)?;
    }

    /// Union and intersection do not depend on operand order.
    #[test]
    fn commutative(a in grid_prim(), b in grid_prim()) {
        let (ba, bb) = (a.body(), b.body());
        let s = vol(&ba) + vol(&bb);
        for op in [BoolOp::Union, BoolOp::Intersect] {
            let (ab, ba_) = (run(&ba, &bb, op), run(&bb, &ba, op));
            if edge_contact_refusal(&a, &b, op, &ab) && edge_contact_refusal(&a, &b, op, &ba_) {
                continue;
            }
            let (ab, ba_) = (ab?, ba_?);
            prop_assert!(close(ab, ba_, s), "{op:?}: A,B gives {ab:.4} but B,A gives {ba_:.4}");
        }
    }

    /// Moving both operands together does not change the answer. A large offset exposes
    /// absolute tolerances (1e-6, 1e-9 literals) that should scale with the model.
    #[test]
    fn translation_invariant(a in grid_prim(), b in grid_prim(), off in prop::sample::select(vec![0.0, 1.0e3, 1.0e5])) {
        let (ba, bb) = (a.body(), b.body());
        let t = Vec3::new(off, -off * 0.7, off * 0.3);
        let mv = |x: &Body| transform(x, t, Vec3::ZERO, Vec3::Z, 0.0).expect("translate");
        let (ta, tb) = (mv(&ba), mv(&bb));
        let s = vol(&ba) + vol(&bb);
        for op in [BoolOp::Union, BoolOp::Intersect, BoolOp::Cut] {
            let (here, there) = (run(&ba, &bb, op), run(&ta, &tb, op));
            if edge_contact_refusal(&a, &b, op, &here) && edge_contact_refusal(&a, &b, op, &there) {
                continue;
            }
            let (here, there) = (here?, there?);
            prop_assert!(close(here, there, s), "{op:?} at origin {here:.4}, moved by {off} gives {there:.4}");
        }
    }

    /// Scaling the whole problem scales volume by k³. Small parts (0.05x) and large parts
    /// (40x) expose tolerances that do not scale.
    #[test]
    fn scale_invariant(a in grid_prim(), b in grid_prim(), k in prop::sample::select(vec![0.05, 1.0, 40.0])) {
        let sc = |p: &Prim| match p.clone() {
            Prim::Box { at, size } => Prim::Box { at: at.map(|x| x * k), size: size.map(|x| x * k) },
            Prim::Cyl { at, axis, r, h } => Prim::Cyl { at: at.map(|x| x * k), axis, r: r * k, h: h * k },
            Prim::Sph { at, r } => Prim::Sph { at: at.map(|x| x * k), r: r * k },
        };
        let (ba, bb, ka, kb) = (a.body(), b.body(), sc(&a).body(), sc(&b).body());
        let s = vol(&ba) + vol(&bb);
        for op in [BoolOp::Union, BoolOp::Intersect, BoolOp::Cut] {
            let (one, big) = (run(&ba, &bb, op), run(&ka, &kb, op));
            if edge_contact_refusal(&a, &b, op, &one) && edge_contact_refusal(&a, &b, op, &big) {
                continue;
            }
            let (one, big) = (one?, big? / (k * k * k));
            prop_assert!(close(one, big, s), "{op:?} at scale 1 gives {one:.4}, at scale {k} gives {big:.4} (normalised)");
        }
    }

    /// A∪A = A, A∩A = A, A−A = nothing.
    #[test]
    fn self_operations(a in grid_prim()) {
        let ba = a.body();
        let va = vol(&ba);
        prop_assert!(close(run(&ba, &ba, BoolOp::Union)?, va, va), "A∪A != A");
        prop_assert!(close(run(&ba, &ba, BoolOp::Intersect)?, va, va), "A∩A != A");
        prop_assert!(run(&ba, &ba, BoolOp::Cut)? <= REL * va, "A−A is not empty");
    }

    /// The same inputs give the same output, twice in a row. The boolean keeps thread-local
    /// state (the nudge guard); this catches state that leaks from one call to the next.
    #[test]
    fn deterministic(a in grid_prim(), b in grid_prim()) {
        let (ba, bb) = (a.body(), b.body());
        for op in [BoolOp::Union, BoolOp::Intersect, BoolOp::Cut] {
            let m = |r: Option<Body>| r.map(|x| { let m = measure(&x).unwrap(); (m.volume.to_bits(), m.faces, m.edges) });
            let first = boolean(&ba, &bb, op).ok().map(m);
            let second = boolean(&ba, &bb, op).ok().map(m);
            prop_assert_eq!(first, second, "{:?} gave two different answers for the same input", op);
        }
    }

    /// Two boxes whose overlap is known exactly (closed form, no tessellation guesswork).
    #[test]
    fn box_box_exact(a in [grid(), grid(), grid()], sa in [grid_size(), grid_size(), grid_size()],
                     b in [grid(), grid(), grid()], sb in [grid_size(), grid_size(), grid_size()]) {
        let overlap: f64 = (0..3).map(|i| ((a[i] + sa[i]).min(b[i] + sb[i]) - a[i].max(b[i])).max(0.0)).product();
        let (pa, pb) = (Prim::Box { at: a, size: sa }, Prim::Box { at: b, size: sb });
        let (ba, bb) = (pa.body(), pb.body());
        let (va, vb) = (sa.iter().product::<f64>(), sb.iter().product::<f64>());
        let s = va + vb;
        // Per axis: the gap between the boxes (<0 overlap, 0 touch, >0 apart).
        let gap: Vec<f64> = (0..3).map(|i| a[i].max(b[i]) - (a[i] + sa[i]).min(b[i] + sb[i])).collect();
        // Touching along an edge or at a corner only: a union there is not a manifold solid.
        // The kernel refuses it on purpose ("zero-thickness edge contact"). Allowed, as an error.
        let edge_only = gap.iter().all(|g| *g <= 0.0) && gap.iter().filter(|g| **g == 0.0).count() >= 2;
        prop_assert!(close(run(&ba, &bb, BoolOp::Intersect)?, overlap, s), "intersect != closed-form overlap {overlap}");
        prop_assert!(close(run(&ba, &bb, BoolOp::Cut)?, va - overlap, s), "cut != closed form {}", va - overlap);
        match run(&ba, &bb, BoolOp::Union) {
            Err(_) if edge_only => {}
            r => prop_assert!(close(r?, s - overlap, s), "union != closed form {}", s - overlap),
        }
    }

    /// Two boxes that share part of a face must fuse into one solid. This is the most common
    /// join in real parts (a boss on a plate).
    #[test]
    fn face_touching_boxes_fuse(x in grid(), y in grid(), w in grid_size(), d in grid_size(), h in grid_size()) {
        let plate = Prim::Box { at: [0.0, 0.0, 0.0], size: [30.0, 30.0, 5.0] }.body();
        let boss = Prim::Box { at: [x.min(27.5), y.min(27.5), 5.0], size: [w, d, h] }.body();
        let (vp, vb) = (vol(&plate), vol(&boss));
        prop_assert!(close(run(&plate, &boss, BoolOp::Union)?, vp + vb, vp + vb), "plate + boss lost or gained volume");
    }
}

// Cases the property tests found, as plain tests. Ignored until the kernel bug is fixed: run them
// with `cargo test -p solidcraft-kernel --test boolean_props -- --ignored`.

/// The saved commutativity case is an intentional non-manifold join refusal,
/// not a failure to join overlapping volumes. Intersection and cut remain valid.
#[test]
fn edge_only_box_join_is_refused_in_both_orders() {
    let a = Prim::Box { at: [15.0, 22.5, 0.0], size: [5.0, 5.0, 17.5] };
    let b = Prim::Box { at: [2.5, 12.5, 0.0], size: [12.5, 10.0, 5.0] };
    assert!(edge_only_boxes(&a, &b));
    assert!(!edge_only_boxes(&a, &Prim::Box { at: [15.0, 22.5, 0.0], size: [5.0, 5.0, 5.0] }));
    let (a, b) = (a.body(), b.body());
    for (left, right) in [(&a, &b), (&b, &a)] {
        let error = run(left, right, BoolOp::Union).expect_err("an edge-only join is non-manifold");
        assert!(error.to_string().contains("zero-thickness edge contact"), "{error}");
        assert_eq!(run(left, right, BoolOp::Intersect).unwrap(), 0.0);
        assert!(close(run(left, right, BoolOp::Cut).unwrap(), vol(left), vol(left)));
    }
}

fn check(r: Result<(), TestCaseError>) {
    if let Err(e) = r {
        panic!("{e}");
    }
}

/// A box and a cylinder in general position: the intersect is a sliver 0.5 mm thick. It was
/// refused as an implausible volume, because the meshes of the check were faceted at 0.017 mm.
#[test]
fn generic_box_cylinder_intersect() {
    let a = Prim::Box { at: [11.483767761089219, 4.801612107504224, 3.6313476467611787], size: [4.789088988834864, 3.0, 10.419335961063835] };
    let b = Prim::Cyl { at: [29.2480632732879, 8.3683289461485, 6.947922225984966], axis: 2, r: 13.502884497756853, h: 19.719218725477496 };
    check(identity(&a, &b));
}

/// A sphere and a box on the grid: the union has a face that does not mesh.
#[test]
#[ignore = "kernel bug, #1648"]
fn grid_sphere_box_union_is_sound() {
    let a = Prim::Sph { at: [2.5, 15.0, 27.5], r: 12.5 };
    let b = Prim::Box { at: [0.0, 0.0, 17.5], size: [5.0, 7.5, 5.0] };
    check(identity(&a, &b));
}

/// Two crossing cylinders: the union moved 1000 mm away loses about a tenth of its volume.
#[test]
#[ignore = "kernel bug, #1649"]
fn crossing_cylinders_union_moved_far() {
    let a = Prim::Cyl { at: [17.5, 30.0, 7.5], axis: 0, r: 5.0, h: 10.0 }.body();
    let b = Prim::Cyl { at: [17.5, 30.0, 15.0], axis: 1, r: 12.5, h: 5.0 }.body();
    let t = Vec3::new(1000.0, -700.0, 300.0);
    let mv = |x: &Body| transform(x, t, Vec3::ZERO, Vec3::Z, 0.0).expect("translate");
    let s = vol(&a) + vol(&b);
    let here = run(&a, &b, BoolOp::Union).unwrap();
    let there = run(&mv(&a), &mv(&b), BoolOp::Union).unwrap();
    assert!(close(here, there, s), "union at origin {here:.4}, moved by 1000 gives {there:.4}");
}
