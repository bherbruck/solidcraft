//! Hyperbola and parabola edges (ISO 10303-42 HYPERBOLA, PARABOLA): a cone cut by a plane
//! parallel to its axis meets it in a hyperbola, by a plane parallel to a generator in a
//! parabola. Hand-written files, volumes against closed forms.

use std::f64::consts::{FRAC_PI_4, PI};

use mt::{BoundedCurve, InnerSpace, ParametricCurve};
use truck_modeling as mt;

use super::geom::{Frame, hyperbola_arc, parabola_arc};
use super::*;
use crate::{measure, step_export};

const HEAD: &str = "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION((''),'2;1');\nFILE_NAME('t','',(''),(''),'','hand','');\nFILE_SCHEMA(('AUTOMOTIVE_DESIGN { 1 0 10303 214 1 1 1 1 }'));\nENDSEC;\nDATA;\n";
const TAIL: &str = "ENDSEC;\nEND-ISO-10303-21;\n";
const CONTEXT: &str = "#90=ADVANCED_BREP_SHAPE_REPRESENTATION('',(#81),#91);\n#91=(GEOMETRIC_REPRESENTATION_CONTEXT(3)GLOBAL_UNCERTAINTY_ASSIGNED_CONTEXT((#92))GLOBAL_UNIT_ASSIGNED_CONTEXT((#93,#94))REPRESENTATION_CONTEXT('',''));\n#92=UNCERTAINTY_MEASURE_WITH_UNIT(LENGTH_MEASURE(1.E-6),#93,'','');\n#93=(LENGTH_UNIT()NAMED_UNIT(*)SI_UNIT(.MILLI.,.METRE.));\n#94=(NAMED_UNIT(*)PLANE_ANGLE_UNIT()SI_UNIT($,.RADIAN.));\n";

/// Height of the cone pieces: the 45° cone `x² + y² = z²` up to `z = H`.
const H: f64 = 10.0;

fn rel(a: f64, b: f64) -> f64 {
    (a - b).abs() / b.abs().max(1e-9)
}

/// A solid bounded by the 45° cone (apex at the origin, axis Z), its top plane `z = H` and a
/// cutting plane. `curve` defines `#30` (the conic edge from `v1` to `v2`, rising parameter) and
/// `#41`, the cutting plane's placement (normal pointing out of the solid). The top circle runs
/// `#51` from `circle_from` to the other vertex counter-clockwise seen from above; the loops say
/// which way each face uses each edge.
fn cone_piece(curve: &str, v1: [f64; 3], v2: [f64; 3], circle_from_v1: bool, loops: [&str; 3]) -> String {
    let mut t = String::from(HEAD);
    t += "#1=CARTESIAN_POINT('',(0.,0.,10.));\n#2=DIRECTION('',(0.,0.,1.));\n#3=DIRECTION('',(1.,0.,0.));\n#4=AXIS2_PLACEMENT_3D('',#1,#2,#3);\n";
    t += &format!("#5=CONICAL_SURFACE('',#4,10.,{FRAC_PI_4:.15});\n#6=PLANE('',#4);\n");
    t += &format!("#10=CARTESIAN_POINT('',({:.15},{:.15},{:.15}));\n#11=VERTEX_POINT('',#10);\n", v1[0], v1[1], v1[2]);
    t += &format!("#12=CARTESIAN_POINT('',({:.15},{:.15},{:.15}));\n#13=VERTEX_POINT('',#12);\n", v2[0], v2[1], v2[2]);
    t += curve;
    t += "#31=EDGE_CURVE('',#11,#13,#30,.T.);\n#42=PLANE('',#41);\n";
    t += "#50=CIRCLE('',#4,10.);\n";
    t += if circle_from_v1 { "#51=EDGE_CURVE('',#11,#13,#50,.T.);\n" } else { "#51=EDGE_CURVE('',#13,#11,#50,.T.);\n" };
    t += "#52=VECTOR('',#3,1.);\n#53=LINE('',#10,#52);\n#54=EDGE_CURVE('',#11,#13,#53,.T.);\n";
    // Faces: the cone, the cutting plane, the top.
    for (k, (lp, surface)) in loops.iter().zip(["#5", "#42", "#6"]).enumerate() {
        let b = 60 + 6 * k;
        let edges: Vec<&str> = lp.split(' ').collect();
        let mut oes = Vec::new();
        for (j, e) in edges.iter().enumerate() {
            let (id, fwd) = e.split_at(e.len() - 1);
            t += &format!("#{}=ORIENTED_EDGE('',*,*,{id},{});\n", b + j, if fwd == "+" { ".T." } else { ".F." });
            oes.push(format!("#{}", b + j));
        }
        t += &format!("#{}=EDGE_LOOP('',({}));\n#{}=FACE_OUTER_BOUND('',#{},.T.);\n", b + 3, oes.join(","), b + 4, b + 3);
        t += &format!("#{}=ADVANCED_FACE('',(#{}),{surface},.T.);\n", b + 5, b + 4);
    }
    t += "#80=CLOSED_SHELL('',(#65,#71,#77));\n#81=MANIFOLD_SOLID_BREP('Piece',#80);\n";
    t += CONTEXT;
    t += TAIL;
    t
}

/// The cone inside `z ≤ H` cut by the plane `y = D` (parallel to the axis), keeping `y ≥ D`:
/// the cut meets the cone in the hyperbola `z² − x² = D²`, centre `(0, D, 0)`, both semi axes D.
fn hyperbola_piece() -> String {
    const D: f64 = 4.0;
    let w = (H * H - D * D).sqrt();
    // Real axis up the plane (X = +z), normal +y, so the imaginary axis is +x:
    // (x, y, z) = (D·sinh t, D, D·cosh t).
    let curve = "#20=CARTESIAN_POINT('',(0.,4.,0.));\n#21=DIRECTION('',(0.,1.,0.));\n#22=AXIS2_PLACEMENT_3D('',#20,#21,#2);\n#30=HYPERBOLA('',#22,4.,4.);\n#40=DIRECTION('',(0.,-1.,0.));\n#41=AXIS2_PLACEMENT_3D('',#20,#40,#3);\n";
    // Cone: up the hyperbola from +x to −x, back along the arc; the plane (seen from −y):
    // the hyperbola from −x to +x, back along the line; the top: the arc, then the line.
    cone_piece(curve, [-w, D, H], [w, D, H], false, ["#31- #51-", "#31+ #54-", "#51+ #54+"])
}

/// The cone inside `z ≤ H` cut by the plane `z − y = C` (parallel to the generator in the
/// x = 0, y > 0 half plane), keeping `z − y ≥ C`: the cut meets the cone in a parabola with its
/// vertex at `(0, −C/2, C/2)`, axis along (0, 1, 1) and focal distance `C·√2 / 4`.
fn parabola_piece() -> String {
    const C: f64 = 8.0;
    let w = (H * H - (H - C) * (H - C)).sqrt();
    // Normal (0, 1, −1) out of the solid, axis X = (0, 1, 1), so Y = Z × X = +x:
    // (x, y, z) = (4√2·t, −4 + 2t², 4 + 2t²).
    let curve = format!(
        "#20=CARTESIAN_POINT('',(0.,-4.,4.));\n#21=DIRECTION('',(0.,1.,-1.));\n#23=DIRECTION('',(0.,1.,1.));\n#22=AXIS2_PLACEMENT_3D('',#20,#21,#23);\n#30=PARABOLA('',#22,{:.15});\n#41=AXIS2_PLACEMENT_3D('',#20,#21,#23);\n",
        C * 2f64.sqrt() / 4.0
    );
    // Cone (seen from −y): the parabola from −x to +x, back along the arc; the plane (seen from
    // outside, +x on the left): the parabola from +x to −x, back along the line; the top: the
    // arc round through −y, then the line back.
    cone_piece(&curve, [-w, H - C, H], [w, H - C, H], true, ["#31+ #51-", "#31- #54+", "#51+ #54-"])
}

/// ∫ from D to H of the circular segment `y ≥ D` of the disc of radius z.
fn hyperbola_piece_volume() -> f64 {
    let d = 4.0;
    let s = (H * H - d * d).sqrt();
    H.powi(3) / 3.0 * (d / H).acos() - 2.0 / 3.0 * d * H * s + d.powi(3) / 3.0 * ((H + s) / d).ln()
}

/// ∫ from C/2 to H of the part `y ≤ z − C` of the disc of radius z.
fn parabola_piece_volume() -> f64 {
    let c = 8.0;
    let q: f64 = 2.0 * c * H - c * c;
    H.powi(3) / 3.0 * (PI - (1.0 - c / H).acos())
        - (0.4 * q.powf(2.5) + 4.0 / 3.0 * c * c * q.powf(1.5) + 2.0 * c.powi(4) * q.sqrt()) / (24.0 * c * c)
        + (0.4 * q.powf(2.5) - 2.0 / 3.0 * c * c * q.powf(1.5)) / (4.0 * c * c)
}

fn import_piece(text: &str, want: f64) -> StepImport {
    assert_eq!(step_orientation_errors(text).unwrap(), Vec::<String>::new());
    let imp = step_import(text).unwrap();
    assert!(imp.warnings.is_empty(), "{:?}", imp.warnings);
    assert_eq!(imp.bodies.len(), 1);
    assert!(imp.bodies[0].closed);
    let m = measure(&imp.bodies[0].body).unwrap();
    assert_eq!(m.faces, 3);
    assert!(rel(m.volume, want) < 1e-4, "volume {} vs {want}", m.volume);
    imp
}

/// Export the imported piece and read it back: the conic edges are written as exact rational
/// B-splines and come back as the same solid.
fn round_trip(imp: &StepImport, want: f64) {
    let text = step_export(&[&imp.bodies[0].body], "conic").unwrap();
    step_validate(&text).unwrap();
    let back = step_import(&text).unwrap();
    assert!(back.warnings.is_empty(), "{:?}", back.warnings);
    assert_eq!(back.bodies.len(), 1);
    assert!(back.bodies[0].closed);
    let v = measure(&back.bodies[0].body).unwrap().volume;
    assert!(rel(v, want) < 1e-4, "round trip volume {v} vs {want}");
}

#[test]
fn hyperbola_bounded_faces_import() {
    let want = hyperbola_piece_volume();
    assert!(rel(want, 175.447_506_9) < 1e-9, "{want}");
    let imp = import_piece(&hyperbola_piece(), want);
    round_trip(&imp, want);
}

#[test]
fn parabola_bounded_faces_import() {
    let want = parabola_piece_volume();
    assert!(rel(want, 447.014_684_29) < 1e-9, "{want}");
    let imp = import_piece(&parabola_piece(), want);
    round_trip(&imp, want);
}

/// The kernel curves lie exactly on the conics, end to end, whichever way they run.
#[test]
fn conic_arcs_are_exact() {
    let f = Frame { o: mt::Point3::new(1.0, 2.0, 3.0), x: mt::Vector3::unit_y(), y: mt::Vector3::unit_z(), z: mt::Vector3::unit_x() };
    let local = |p: mt::Point3| ((p - f.o).dot(f.x), (p - f.o).dot(f.y), (p - f.o).dot(f.z));
    for (t0, t1) in [(-1.5, 2.0), (3.0, -0.25), (0.0, 7.5)] {
        let c = hyperbola_arc(&f, 3.0, 2.0, t0, t1);
        let (s0, s1) = c.range_tuple();
        for i in 0..=200 {
            let (x, y, z) = local(c.subs(s0 + (s1 - s0) * i as f64 / 200.0));
            assert!((x * x / 9.0 - y * y / 4.0 - 1.0).abs() < 1e-9 && x > 0.0 && z.abs() < 1e-12, "{x} {y}");
        }
        let (x, y, _) = local(c.subs(s0));
        assert!((x - 3.0 * f64::cosh(t0)).abs() < 1e-9 && (y - 2.0 * f64::sinh(t0)).abs() < 1e-9);
        let (x, y, _) = local(c.subs(s1));
        assert!((x - 3.0 * f64::cosh(t1)).abs() < 1e-9 && (y - 2.0 * f64::sinh(t1)).abs() < 1e-9);

        let c = parabola_arc(&f, 1.5, t0, t1);
        let (s0, s1) = c.range_tuple();
        for i in 0..=200 {
            let (x, y, z) = local(c.subs(s0 + (s1 - s0) * i as f64 / 200.0));
            assert!((y * y - 4.0 * 1.5 * x).abs() < 1e-9 * (1.0 + x.abs()) && z.abs() < 1e-12, "{x} {y}");
        }
        let (x, y, _) = local(c.subs(s1));
        assert!((x - 1.5 * t1 * t1).abs() < 1e-9 && (y - 3.0 * t1).abs() < 1e-9);
    }
}

/// Hostile conics are errors (the face is reported and left out), never panics.
#[test]
fn hostile_conics_never_panic() {
    let good = hyperbola_piece();
    for bad in [
        good.replace("HYPERBOLA('',#22,4.,4.)", "HYPERBOLA('',#22,0.,4.)"),
        good.replace("HYPERBOLA('',#22,4.,4.)", "HYPERBOLA('',#22,4.,-1.)"),
        good.replace("HYPERBOLA('',#22,4.,4.)", "HYPERBOLA('',#22,4.)"),
        good.replace("HYPERBOLA('',#22,4.,4.)", "HYPERBOLA('',#22,1.E300,4.)"),
        good.replace("HYPERBOLA('',#22,4.,4.)", "HYPERBOLA('',#22,1.E-300,1.E-300)"),
        good.replace("HYPERBOLA('',#22,4.,4.)", "HYPERBOLA('',#22,5.,4.)"),
        good.replace("HYPERBOLA('',#22,4.,4.)", "PARABOLA('',#22,0.)"),
        good.replace("HYPERBOLA('',#22,4.,4.)", "PARABOLA('',#22,1.)"),
        good.replace("#31=EDGE_CURVE('',#11,#13,#30,.T.)", "#31=EDGE_CURVE('',#11,#11,#30,.T.)"),
    ] {
        let r = std::panic::catch_unwind(|| step_import(&bad));
        let Ok(r) = r else { panic!("STEP import panicked") };
        if let Ok(imp) = r {
            assert!(!imp.warnings.is_empty(), "a broken conic imported silently");
        }
    }
}
