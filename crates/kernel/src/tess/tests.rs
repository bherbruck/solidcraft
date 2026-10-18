use std::collections::HashMap;

use solidcraft_geom::{Mesh, Vec3};

use super::*;
use crate::{Body, BoolOp};

/// Every triangle edge, with vertices merged by exact position, is shared by exactly two
/// triangles: the mesh is closed and its faces meet exactly (no cracks, no T-junctions).
fn assert_watertight(m: &Mesh, name: &str) {
    // By value: −0.0 and 0.0 are one position (adding 0.0 makes −0.0 into 0.0).
    let key = |i: u32| m.positions.get(i as usize).map(|p| [(p.x + 0.0).to_bits(), (p.y + 0.0).to_bits(), (p.z + 0.0).to_bits()]);
    let mut uses: HashMap<([u64; 3], [u64; 3]), i32> = HashMap::new();
    for t in &m.triangles {
        for e in 0..3 {
            let (Some(a), Some(b)) = (key(t[e]), key(t[(e + 1) % 3])) else { panic!("{name}: bad index") };
            if a == b {
                continue;
            }
            // Directed: a closed, consistently oriented mesh uses each edge once each way.
            *uses.entry((a, b)).or_default() += 1;
            *uses.entry((b, a)).or_default() -= 1;
        }
    }
    let open: Vec<_> = uses.iter().filter(|(_, n)| **n != 0).collect();
    assert!(open.is_empty(), "{name}: {} unmatched directed edges, e.g. {:?}", open.len(), open.first());
}

fn plan_body(b: &Body, tol: f64) -> (Mesh, PlanStats) {
    match plan(&b.solid, tol, Some(crate::tess::size::Turn { angle: crate::DISPLAY_ANGLE, floor: tol * 1e-3 })) {
        Ok((p, stats)) => (p.evaluate(), stats),
        Err(e) => panic!("plan: {e}"),
    }
}

fn check(name: &str, b: &Body, tol: f64, volume: f64) -> PlanStats {
    let (m, stats) = plan_body(b, tol);
    assert_watertight(&m, name);
    let got = m.measure().volume;
    // Chord error tol over the surface: the volume is short by at most about area · tol.
    let area = m.measure().area;
    assert!((got - volume).abs() <= area * tol * 1.5 + 1e-9, "{name}: volume {got} vs {volume} (area {area}, tol {tol})");
    // Outward normals: the divergence theorem gives a positive volume only if they are.
    assert!(got > 0.0, "{name}: inside out");
    let faces = b.solid.face_iter().count();
    assert_eq!(stats.planar + stats.structured + stats.delaunay, faces, "{name}: every face planned");
    stats
}

fn ok<T>(r: crate::Result<T>) -> T {
    match r {
        Ok(v) => v,
        Err(e) => panic!("{e}"),
    }
}

fn cut(a: &Body, b: &Body) -> Body {
    match crate::boolean(a, b, BoolOp::Cut) {
        Ok(Some(r)) => r,
        other => panic!("cut: {other:?}"),
    }
}

const PI: f64 = std::f64::consts::PI;

/// Smallest corner angle (degrees) over the mesh.
fn min_angle(m: &Mesh) -> f64 {
    let mut least = 180.0f64;
    for t in &m.triangles {
        let [a, b, c] = t.map(|i| m.positions[i as usize]);
        for (p, q, r) in [(a, b, c), (b, c, a), (c, a, b)] {
            let (u, v) = (q - p, r - p);
            if let (Some(u), Some(v)) = (u.normalized(), v.normalized()) {
                least = least.min(u.dot(v).clamp(-1.0, 1.0).acos().to_degrees());
            }
        }
    }
    least
}

#[test]
fn a_box_is_a_regular_grid_on_every_face() {
    // 40 × 30 × 20: sides at most 1/5 of the diagonal (10.77 mm), so 4 × 3 × 2 cells.
    let b = ok(crate::box_solid(Vec3::ZERO, Vec3::new(40.0, 30.0, 20.0)));
    let (m, s) = plan_body(&b, 0.05);
    assert_eq!(s.planar, 6);
    assert_watertight(&m, "box");
    let cells = 2 * (4 * 3 + 4 * 2 + 3 * 2);
    assert_eq!(m.triangles.len(), 2 * cells, "two triangles per grid cell");
    assert!(min_angle(&m) > 40.0, "{}", min_angle(&m));
}

#[test]
fn primitives_are_watertight_with_the_right_volume() {
    for tol in [0.05, 0.005] {
        let s = check("box", &ok(crate::box_solid(Vec3::ZERO, Vec3::new(10.0, 6.0, 4.0))), tol, 240.0);
        assert_eq!(s, PlanStats { planar: 6, structured: 0, delaunay: 0 });
        let s = check("cylinder", &ok(crate::cylinder(Vec3::new(1.0, 2.0, 3.0), Vec3::new(0.0, 1.0, 1.0), 2.5, 7.0)), tol, PI * 2.5 * 2.5 * 7.0);
        assert_eq!(s.delaunay, 0, "cylinder walls are structured");
        let s = check("torus", &ok(crate::torus(Vec3::ZERO, 6.0, 1.5)), tol, 2.0 * PI * PI * 6.0 * 1.5 * 1.5);
        assert_eq!(s.delaunay, 0, "torus patches are structured");
    }
}

#[test]
fn holes_and_trimmed_faces_are_watertight() {
    let tol = 0.01;
    let bx = ok(crate::box_solid(Vec3::ZERO, Vec3::new(10.0, 10.0, 4.0)));
    let pin = ok(crate::cylinder(Vec3::new(5.0, 5.0, -1.0), Vec3::Z, 2.0, 6.0));
    check("box with a hole", &cut(&bx, &pin), tol, 400.0 - PI * 4.0 * 4.0);
    // A cylinder with a cross hole: trimmed cylinder halves (an iso-line grid cut by the hole)
    // and intersection-curve walls; no face needs Delaunay.
    let cyl = ok(crate::cylinder(Vec3::new(5.0, 5.0, -1.0), Vec3::Z, 2.0, 6.0));
    let cross = ok(crate::cylinder(Vec3::new(5.0, -1.0, 2.0), Vec3::Y, 1.0, 12.0));
    let body = cut(&cyl, &cross);
    let (m, stats) = plan_body(&body, tol);
    assert_watertight(&m, "cross hole");
    assert_eq!(stats.delaunay, 0, "{stats:?}");
    // Against the reference mesher's volume (no closed form for the two-cylinder intersection).
    let Ok(reference) = body.tessellate(tol * 0.2) else { panic!("reference mesh") };
    let (a, b) = (m.measure().volume, reference.measure().volume);
    assert!((a - b).abs() < 0.01 * b, "cross hole volume {a} vs {b}");
}

#[test]
fn faces_and_edges_keep_the_body_order() {
    let body = ok(crate::cylinder(Vec3::ZERO, Vec3::Z, 2.0, 5.0));
    let (m, _) = plan_body(&body, 0.01);
    let Ok(reference) = body.tessellate(0.01) else { panic!("reference mesh") };
    assert_eq!(m.edges.len(), reference.edges.len());
    assert_eq!(m.edge_faces, reference.edge_faces);
    // Same faces in the same order: each face's area agrees with the reference mesher's.
    let face_area = |m: &Mesh, f: u32| -> f64 {
        m.triangles
            .iter()
            .zip(&m.tri_face)
            .filter(|(_, tf)| **tf == f)
            .map(|(t, _)| {
                let p = |i: u32| m.positions.get(i as usize).copied().unwrap_or_default();
                (p(t[1]) - p(t[0])).cross(p(t[2]) - p(t[0])).len() * 0.5
            })
            .sum()
    };
    for f in 0..body.solid.face_iter().count() as u32 {
        let (a, b) = (face_area(&m, f), face_area(&reference, f));
        assert!((a - b).abs() < 0.01 * b.max(1e-9), "face {f}: {a} vs {b}");
    }
    // Each edge polyline ends on the same points as the reference's.
    for (a, b) in m.edges.iter().zip(&reference.edges) {
        assert!(a.first().zip(b.first()).is_some_and(|(p, q)| p.dist(*q) < 1e-9));
        assert!(a.last().zip(b.last()).is_some_and(|(p, q)| p.dist(*q) < 1e-9));
    }
}

/// Side-by-side wireframes, reference mesher (left) and plan (right):
/// `SOLIDCRAFT_TESS_SVG=<dir> cargo test -p solidcraft-kernel render_wireframes -- --ignored`.
#[test]
#[ignore]
fn render_wireframes() {
    let Ok(dir) = std::env::var("SOLIDCRAFT_TESS_SVG") else { return };
    let tol = 0.02;
    let bx = ok(crate::box_solid(Vec3::ZERO, Vec3::new(10.0, 10.0, 4.0)));
    let pin = ok(crate::cylinder(Vec3::new(5.0, 5.0, -1.0), Vec3::Z, 2.0, 6.0));
    let cross = ok(crate::cylinder(Vec3::new(5.0, -1.0, 2.0), Vec3::Y, 1.0, 12.0));
    let filleted =
        ok(crate::fillet(&bx, &[Vec3::new(5.0, 0.0, 4.0), Vec3::new(10.0, 5.0, 4.0), Vec3::new(5.0, 10.0, 4.0), Vec3::new(0.0, 5.0, 4.0)], 1.5));
    let shapes = [
        ("box_hole", cut(&bx, &pin)),
        ("cylinder", pin.clone()),
        ("cross_hole", cut(&pin, &cross)),
        ("torus", ok(crate::torus(Vec3::ZERO, 6.0, 1.5))),
        ("fillet", filleted),
    ];
    for (name, body) in shapes {
        let t0 = std::time::Instant::now();
        let Ok(old) = body.tessellate(tol) else { panic!("reference") };
        let t_old = t0.elapsed().as_secs_f64() * 1e3;
        let t0 = std::time::Instant::now();
        let new = match plan(&body.solid, tol, Some(crate::tess::size::Turn { angle: crate::DISPLAY_ANGLE, floor: tol * 1e-3 })) {
            Ok((p, st)) => Some((p.evaluate(), st)),
            Err(e) => {
                println!("{name}: no plan ({e})");
                None
            }
        };
        let t_new = t0.elapsed().as_secs_f64() * 1e3;
        let mut svg = String::from("<svg xmlns='http://www.w3.org/2000/svg' width='1600' height='860' style='background:#fff'>");
        let label = |x: f64, text: String| format!("<text x='{x}' y='40' font-family='sans-serif' font-size='26'>{text}</text>");
        svg += &label(20.0, format!("meshalgo (Delaunay): {} tris, {t_old:.1} ms", old.triangles.len()));
        svg += &draw(&old, 0.0);
        if let Some((m, st)) = &new {
            svg += &label(
                820.0,
                format!("plan: {} tris, {t_new:.1} ms  [{} planar, {} grid, {} cdt]", m.triangles.len(), st.planar, st.structured, st.delaunay),
            );
            svg += &draw(m, 800.0);
        }
        svg += "</svg>";
        let _ = std::fs::write(format!("{dir}/{name}.svg"), svg);
    }
}

/// Isometric, painter-sorted, flat-shaded triangles with their edges, in an 800×800 panel.
fn draw(m: &Mesh, x0: f64) -> String {
    let (c30, s30) = (30f64.to_radians().cos(), 30f64.to_radians().sin());
    let proj = |p: Vec3| -> (f64, f64, f64) {
        // Turn 45° about Z, tilt 30° about X; depth along the view.
        let (x, y) = ((p.x - p.y) * std::f64::consts::FRAC_1_SQRT_2, (p.x + p.y) * std::f64::consts::FRAC_1_SQRT_2);
        (x, p.z * c30 - y * s30, y * c30 + p.z * s30)
    };
    let pts: Vec<(f64, f64, f64)> = m.positions.iter().map(|p| proj(*p)).collect();
    let (mut lo, mut hi) = ((f64::MAX, f64::MAX), (f64::MIN, f64::MIN));
    for p in &pts {
        lo = (lo.0.min(p.0), lo.1.min(p.1));
        hi = (hi.0.max(p.0), hi.1.max(p.1));
    }
    let k = 700.0 / (hi.0 - lo.0).max(hi.1 - lo.1).max(1e-9);
    let sx = |p: (f64, f64, f64)| (x0 + 50.0 + (p.0 - lo.0) * k, 800.0 - (p.1 - lo.1) * k);
    let mut tris: Vec<(f64, [u32; 3], u32)> =
        m.triangles.iter().zip(&m.tri_face).map(|(t, f)| (t.iter().map(|&i| pts[i as usize].2).sum::<f64>(), *t, *f)).collect();
    tris.sort_by(|a, b| b.0.total_cmp(&a.0));
    let light = Vec3::new(0.3, -0.5, 0.8).normalized().unwrap_or(Vec3::new(0.0, 0.0, 1.0));
    let mut out = String::new();
    let tint = [(1.0, 1.0, 1.15), (1.15, 1.0, 1.0), (1.0, 1.15, 1.0), (1.1, 1.1, 0.9), (0.9, 1.1, 1.1), (1.1, 0.9, 1.1)];
    for (_, t, f) in tris {
        let (tr, tg, tb) = tint[f as usize % tint.len()];
        let [a, b, c] = t.map(|i| m.positions[i as usize]);
        let n = (b - a).cross(c - a).normalized().unwrap_or(Vec3::new(0.0, 0.0, 1.0));
        let shade = (90.0 + 150.0 * n.dot(light).max(0.0)) as u8;
        let q: Vec<(f64, f64)> = t.iter().map(|&i| sx(pts[i as usize])).collect();
        // Back faces away (screen y points down, so outward triangles wind clockwise here).
        if (q[1].0 - q[0].0) * (q[2].1 - q[0].1) - (q[2].0 - q[0].0) * (q[1].1 - q[0].1) > 0.0 {
            continue;
        }
        let pp: Vec<String> = q.iter().map(|(x, y)| format!("{x:.1},{y:.1}")).collect();
        out += &format!(
            "<polygon points='{}' fill='rgb({},{},{})' stroke='#1d3557' stroke-width='0.5'/>",
            pp.join(" "),
            (f64::from(shade) * tr).min(255.0) as u8,
            (f64::from(shade) * tg).min(255.0) as u8,
            (f64::from(shade) * tb).min(255.0) as u8,
        );
    }
    out
}

/// Blind drilled holes (a box cut by a revolved drill with a 118° point, as the hole feature
/// builds it): the point's apex is a pole of the cone faces, two copies of one boundary node.
/// The plan is closed and has no triangle on both copies (no zero-area triangles), also where
/// such a triangle carried a boundary side of the cone.
#[test]
fn a_drill_point_apex_leaves_no_zero_area_triangles() {
    use solidcraft_geom::{Loop2, Plane, Region2, Vec2};
    // (box size, hole position, radius, depth)
    let cases =
        [(Vec3::new(40.0, 30.0, 15.0), Vec3::new(20.0, 15.0, 15.0), 3.4, 8.0), (Vec3::new(60.0, 40.0, 15.0), Vec3::new(50.0, 20.0, 15.0), 3.0, 10.0)];
    for (size, at, r, depth) in cases {
        let bx = ok(crate::box_solid(Vec3::ZERO, size));
        let tip = r / (118f64 / 2.0).to_radians().tan();
        let Some(rp) = Plane::new(at, Vec3::X, Vec3::Z) else { panic!("plane") };
        let pts = [Vec2::new(0.0, 0.6), Vec2::new(r, 0.6), Vec2::new(r, -depth), Vec2::new(0.0, -depth - tip)];
        let region = Region2 { outer: Loop2::polygon(&pts), holes: vec![] };
        let Some(drill) = ok(crate::revolve(&rp, &[region], Vec2::ZERO, Vec2::Y, std::f64::consts::TAU)).pop() else { panic!("drill") };
        let body = cut(&bx, &drill);
        let name = format!("hole r {r} depth {depth}");
        let (m, _) = plan_body(&body, 0.02);
        assert_watertight(&m, &name);
        for t in &m.triangles {
            let [a, b, c] = t.map(|i| m.positions[i as usize]);
            assert!((b - a).cross(c - a).len() > 1e-12, "{name}: zero-area triangle {a:?} {b:?} {c:?}");
        }
    }
}

#[test]
fn a_placed_body_meshes_like_the_original() {
    let b = ok(crate::box_solid(Vec3::ZERO, Vec3::new(2.0, 3.0, 4.0)));
    let placed = ok(crate::transform(&b, Vec3::new(15.0, -2.0, 8.0), Vec3::ZERO, Vec3::X, 0.7));
    let (m, _) = plan_body(&b, 0.005);
    let (p, _) = plan_body(&placed, 0.005);
    assert_eq!(m.tri_face, p.tri_face);
}

/// Seeds on a motorcycle trace start at the spacing of the boundary it leaves and grow
/// geometrically to the spacing at its other end (here 0.1 to 1 over 5 mm).
#[test]
fn trace_seeds_grade_from_one_end_to_the_other() {
    let t = graded([0.0, 0.0], [5.0, 0.0], 0.1, 1.0, 10.0, [1.0, 1.0], &|_, _| true);
    let all: Vec<f64> = std::iter::once(0.0).chain(t.iter().copied()).chain(std::iter::once(1.0)).map(|f| f * 5.0).collect();
    let sizes: Vec<f64> = all.windows(2).map(|w| w[1] - w[0]).collect();
    assert!((sizes[0] - 0.1).abs() < 0.03, "first {}", sizes[0]);
    assert!((sizes[sizes.len() - 1] - 1.0).abs() < 0.3, "last {}", sizes[sizes.len() - 1]);
    assert!(sizes.windows(2).all(|w| w[1] >= w[0] - 1e-12), "growing: {sizes:?}");
    // Equal ends: even; a segment no longer than its spacing: no seeds.
    let even = graded([0.0, 0.0], [4.0, 0.0], 1.0, 1.0, 10.0, [1.0, 1.0], &|_, _| true);
    assert_eq!(even.len(), 3);
    assert!(graded([0.0, 0.0], [0.5, 0.0], 1.0, 1.0, 10.0, [1.0, 1.0], &|_, _| true).is_empty());
}

/// A surface that joins itself along a direction is periodic there: its parameters repeat, so
/// the evaluator takes a parameter outside the range back into it. A boundary that runs across
/// the join is unwrapped past the range's end and must still land on the surface.
#[test]
fn a_closed_spline_surface_is_periodic_and_evaluates_outside_its_range() {
    use solidcraft_geom::tess::EvalSurface;
    // A five-sided tube: the control net's last column repeats its first.
    let corner = |k: usize| {
        let a = std::f64::consts::TAU * (k % 5) as f64 / 5.0;
        (10.0 * a.cos(), 10.0 * a.sin())
    };
    let mut ctrl = Vec::new();
    for j in 0..2 {
        for k in 0..=5 {
            let (x, y) = corner(k);
            ctrl.push([x, y, f64::from(j) * 8.0, 1.0]);
        }
    }
    let s = EvalSurface::Nurbs {
        degree: [1, 1],
        knots: [vec![0.0, 0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 5.0], vec![0.0, 0.0, 1.0, 1.0]],
        count: [6, 2],
        ctrl,
        closed: [true, false],
    };
    let dom = uv::domain(&s);
    assert_eq!(dom.period, [Some(5.0), None]);
    assert_eq!(dom.range, [Some((0.0, 5.0)), Some((0.0, 1.0))]);
    // A parameter a whole range below the start is the same point as one inside it.
    for (a, b) in [(-0.5, 4.5), (-2.0, 3.0), (5.5, 0.5), (7.0, 2.0)] {
        let (Some(out), Some(inside)) = (s.derivs(a, 0.25), s.derivs(b, 0.25)) else { panic!("evaluates") };
        assert!(out.0.dist(inside.0) < 1e-12, "u = {a} and u = {b}: {:?} vs {:?}", out.0, inside.0);
    }
    // The same net left open is not periodic, and keeps truck's end-span extension outside it.
    let EvalSurface::Nurbs { degree, knots, count, ctrl, .. } = s else { panic!("nurbs") };
    let open = EvalSurface::Nurbs { degree, knots, count, ctrl, closed: [false, false] };
    assert_eq!(uv::domain(&open).period, [None, None]);
    let (Some(out), Some(inside)) = (open.derivs(-0.5, 0.25), open.derivs(4.5, 0.25)) else { panic!("evaluates") };
    assert!(out.0.dist(inside.0) > 1.0, "an open surface does not wrap");
}

/// Each loop of a face is projected on its own, so on a periodic surface a hole can come out a
/// whole turn away from the boundary that holds it. Every loop is brought into the outer
/// boundary's period, where the hole cuts the face again.
#[test]
fn holes_are_brought_into_the_outer_boundarys_period() {
    let tau = std::f64::consts::TAU;
    let at = |u: f64, v: f64| seam::Loop { uv: vec![[u, v], [u + 0.2, v], [u + 0.2, v + 0.2], [u, v + 0.2]], pts: vec![Vec3::ZERO; 4] };
    let dom = uv::Domain { range: [Some((0.0, 1.0)), Some((0.0, tau))], period: [None, Some(tau)] };
    let mut loops = vec![at(0.0, -tau), at(0.4, 1.5), at(0.6, 1.5 - tau)];
    align_periods(&mut loops, &dom);
    // The outer loop stays where it is; both holes land beside it, a whole turn moved or not.
    assert_eq!(loops[0].uv[0], [0.0, -tau]);
    assert!((loops[1].uv[0][1] - (1.5 - tau)).abs() < 1e-12, "{:?}", loops[1].uv[0]);
    assert!((loops[2].uv[0][1] - (1.5 - tau)).abs() < 1e-12, "{:?}", loops[2].uv[0]);
    // Nothing moves along a direction that does not repeat.
    assert_eq!(loops[1].uv[0][0], 0.4);
}

/// A boundary that steps out to a node and straight back encloses no area: the pair is dropped,
/// so what is left no longer runs along itself. The two sides need only agree within the given
/// distance — a STEP writer may draw them as two curves a hair apart.
#[test]
fn zero_width_slits_are_dropped_from_a_boundary() {
    let p = |x: f64| Vec3::new(x, 0.0, 0.0);
    // A square with a slit poked into it from the corner (2, 1): out to (2, 2) and back.
    let slit = vec![
        p(0.0),
        p(1.0),
        Vec3::new(2.0, 1.0, 0.0),
        Vec3::new(2.0, 2.0, 0.0),
        Vec3::new(2.0 + 1e-9, 1.0, 0.0),
        p(3.0),
        Vec3::new(3.0, 3.0, 0.0),
        Vec3::new(0.0, 3.0, 0.0),
    ];
    let (out, known) = drop_slits(&[slit], &[Vec::new()], 1e-6);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0], vec![p(0.0), p(1.0), Vec3::new(2.0, 1.0, 0.0), p(3.0), Vec3::new(3.0, 3.0, 0.0), Vec3::new(0.0, 3.0, 0.0)]);
    assert_eq!(known[0].len(), out[0].len());
    // Too far apart to be one slit: the boundary is left as it is.
    let wide = vec![p(0.0), p(1.0), Vec3::new(2.0, 1.0, 0.0), Vec3::new(2.0, 2.0, 0.0), Vec3::new(2.5, 1.0, 0.0), p(3.0)];
    let kept = drop_slits(std::slice::from_ref(&wide), &[Vec::new()], 1e-6).0;
    assert_eq!(kept, vec![wide]);
}
