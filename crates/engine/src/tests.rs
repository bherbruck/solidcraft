use std::f64::consts::PI;

use serde_json::{Value, json};

use crate::*;

mod mesh_convert;

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    match s.execute(id, &p) {
        Ok(v) => v,
        Err(e) => panic!("{id} {p}: {e}"),
    }
}

fn volume(s: &mut Session) -> f64 {
    let m = run(s, "inspect.measure", json!({}));
    m["total"]["volume_mm3"].as_f64().unwrap()
}

fn rel(a: f64, b: f64) -> f64 {
    (a - b).abs() / b.abs().max(1e-12)
}

#[test]
fn round_as_tall_as_a_disc_uses_up_its_wall() {
    // #190: a disc 13 in radius and 2 high, its top edge rounded by 2.
    let mut s = Session::default();
    run(&mut s, "solid.cylinder", json!({"radius": 13, "height": 2}));
    run(&mut s, "solid.fillet", json!({"edges": [[0, 13, 2], [0, -13, 2]], "radius": "2 mm"}));
    let (a, c) = ((1.0 - PI / 4.0) * 4.0, 2.0 * (10.0 - 3.0 * PI) / (3.0 * (4.0 - PI)));
    let expect = PI * 169.0 * 2.0 - a * 2.0 * PI * (13.0 - c);
    assert!(rel(volume(&mut s), expect) < 2e-4, "{} vs {expect}", volume(&mut s));
}

/// #783: an edge pick far from every edge (beside the body, above it, inside it, or on another
/// body than the one named) is refused and adds nothing, instead of rounding the nearest edge;
/// a pick on an edge, or a little off it as a click lands, still works.
#[test]
fn edge_picks_far_from_every_edge_are_refused() {
    let mut s = Session::default();
    for (name, x) in [("A", 0), ("B", 40), ("C", 80), ("D", 120)] {
        run(&mut s, "solid.box", json!({"length": 20, "width": 20, "height": 10, "corner": [x, 0, 0], "operation": "new", "body_name": name}));
    }
    run(&mut s, "solid.fillet", json!({"body": "A", "edges": [[10, 0, 10]], "radius": 1}));
    run(&mut s, "solid.fillet", json!({"body": "B", "edges": [[50, 0.4, 10.3]], "radius": 1}));
    let features = s.doc.features.len();
    for (id, p, far) in [
        ("solid.fillet", json!({"body": "B", "edges": [[50, -15, 10]], "radius": 1}), "15.0"),
        ("solid.chamfer", json!({"body": "C", "edges": [[90, 10, 40]], "distance": 1}), "31.623 mm"),
        ("solid.fillet", json!({"edges": [[130, 10, 5]], "radius": 1}), "11.180 mm"),
        ("solid.fillet", json!({"body": "C", "edges": [[10, 20, 10]], "radius": 1}), "mm away"),
        ("solid.press_pull", json!({"edges": [[50, -15, 10]], "distance": 1}), "15.0"),
    ] {
        let err = s.execute(id, &p).expect_err(&format!("{id} {p}")).to_string();
        assert!(err.contains("no edge at") && err.contains(far), "{id} {p}: {err}");
    }
    assert_eq!(s.doc.features.len(), features);
}

#[test]
fn fillet_as_wide_as_a_face_rounds_it_off() {
    // #199: 14 x 6 x 10 box, the four vertical edges at r = 3 (half the 6 mm end faces).
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 14, "width": 6, "height": 10, "corner": [14, 0, 0]}));
    run(&mut s, "solid.fillet", json!({"edges": [[14, 0, 5], [28, 0, 5], [14, 6, 5], [28, 6, 5]], "radius": "3"}));
    let expect = 840.0 - (4.0 - PI) * 9.0 * 10.0;
    assert!(rel(volume(&mut s), expect) < 2e-4, "{} vs {expect}", volume(&mut s));
    assert!(s.execute("solid.fillet", &json!({"edges": [[21, 0, 10]], "radius": 3.5})).is_err());
}

#[test]
fn join_extrude_from_a_chamfered_top_face() {
    let mut s = Session::default();
    run(&mut s, "parameters.add", json!({"name":"height","expression":"20 mm"}));
    run(&mut s, "parameters.add", json!({"name":"bevel","expression":"2 mm"}));
    run(&mut s, "sketch.create", json!({"plane": "XY"}));
    run(&mut s, "sketch.circle.center", json!({"center": [0, 0], "radius": 10}));
    let source = run(&mut s, "solid.extrude", json!({"distance": "height","name":"Stem"}))["feature"].as_u64().unwrap();
    run(&mut s, "solid.chamfer", json!({"edges": [[10, 0, 20]], "distance": "bevel"}));
    let base = volume(&mut s);
    let id = run(&mut s, "sketch.create", json!({"plane": {"face": [0, 0, 20]}}))["sketch"].as_u64().unwrap();
    let solidcraft_doc::FeatureKind::Sketch { plane: solidcraft_doc::PlaneRef::Face { anchor: Some(anchor), .. }, sketch } =
        &s.doc.feature(id).unwrap().kind
    else {
        panic!("the chamfered cap retains its exact original construction support");
    };
    assert_eq!(anchor.target.origin_feature, source);
    assert!(matches!(anchor.target.certificate, Some(solidcraft_doc::config::FaceCertificate::ExtrudeCap { high: true, .. })));
    assert!(!sketch.links.is_empty());
    assert!(sketch.links.iter().all(|link| link.auto_face && !link.lost));
    // The default linked cap outline is radius8; no explicit fixed plane or
    // duplicate manually drawn circle is required to join a pad to that face.
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.extrude", json!({"distance": 5, "operation": "join"}));
    assert_eq!(s.model.state().bodies.len(), 1);
    assert!(rel(volume(&mut s), base + PI * 64.0 * 5.0) < 5e-4);
    for (height, bevel) in [(20., 2.), (25., 3.)] {
        run(&mut s, "parameters.change", json!({"name":"height","expression":format!("{height} mm")}));
        run(&mut s, "parameters.change", json!({"name":"bevel","expression":format!("{bevel} mm")}));
        let expected = PI * (100. * height - (10. * bevel * bevel - bevel * bevel * bevel / 3.) + (10. - bevel) * (10. - bevel) * 5.);
        let loaded = solidcraft_doc::Document::from_json(&s.doc.to_json()).unwrap();
        let mut cold = Session::new(loaded);
        let warm_volume = volume(&mut s);
        let cold_volume = volume(&mut cold);
        assert!(rel(warm_volume, expected) < 5e-4, "{warm_volume} vs {expected}");
        assert!((warm_volume - cold_volume).abs() < 1e-8);
        for current in [&s, &cold] {
            assert!(current.model.results.iter().all(|result| result.error.is_none() && result.warning.is_none()), "{:?}", current.model.results);
            let state = current.model.state();
            assert_eq!(state.bodies.len(), 1);
            let solved = state.sketch(id).unwrap();
            assert!((solved.plane.origin.z - height).abs() < 1e-8);
            assert!(solved.sketch.links.iter().all(|link| link.auto_face && !link.lost));
            let body = &state.bodies[0];
            assert!(body.body.contains(solidcraft_geom::Vec3::new(0., 0., height + 4.)));
            assert!(!body.body.contains(solidcraft_geom::Vec3::new(0., 0., height + 6.)));
            assert!(body.body.contains(solidcraft_geom::Vec3::new(10. - bevel - 0.5, 0., height + 4.)));
            assert!(!body.body.contains(solidcraft_geom::Vec3::new(10. - bevel + 0.5, 0., height + 4.)));
        }
    }
}

#[test]
fn a_body_cut_in_pieces_twice_gets_pieces_of_their_own_names() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 40, "width": 5, "height": 5, "body_name": "Bar"}));
    for x in [10, 25] {
        run(&mut s, "solid.box", json!({"corner": [x, -1, -1], "length": 2, "width": 7, "height": 7, "body_name": "Gap"}));
        run(&mut s, "solid.combine", json!({"target": "Bar", "tools": ["Gap"], "operation": "cut"}));
    }
    let names: Vec<String> = s.model.state().bodies.iter().map(|b| b.name.clone()).collect();
    let unique: std::collections::BTreeSet<&String> = names.iter().collect();
    assert_eq!(unique.len(), 3, "{names:?}");
    assert!(names.contains(&"Bar (2)".to_string()), "{names:?}");
}

/// A join that bridges two bodies makes them one body, not an overlap of the first and the second.
#[test]
fn extrude_a_face_where_another_body_lies_flush_on_it() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 20, "width": 20, "height": 4, "body_name": "A"}));
    run(&mut s, "solid.box", json!({"corner": [10, 0, 4], "length": 10, "width": 20, "height": 1, "body_name": "B"}));
    // A's top and B's bottom both lie at the point: which one is meant has to be said.
    let e = s.execute("solid.extrude", &json!({"face": [15, 5, 4], "distance": "2 mm", "operation": "new"})).unwrap_err().to_string();
    assert!(e.contains("A") && e.contains("B") && e.contains("face_body"), "{e}");
    let r = run(&mut s, "solid.extrude", json!({"face": [15, 5, 4], "face_body": "A", "distance": "2 mm", "operation": "new", "body_name": "C"}));
    let m = run(&mut s, "inspect.measure", json!({"bodies": ["C"]}));
    let v = m["total"]["volume_mm3"].as_f64().unwrap();
    assert!((v - 20.0 * 20.0 * 2.0).abs() < 1e-6, "{v} {r}");
    let e = s.execute("solid.extrude", &json!({"face": [15, 5, 4], "face_body": "Nope", "distance": "2 mm"})).unwrap_err().to_string();
    assert!(e.contains("Nope"), "{e}");
}

#[test]
fn join_bridging_two_bodies_merges_them() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10}));
    run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10, "corner": [20, 0, 0], "operation": "new_body"}));
    run(&mut s, "solid.box", json!({"length": 20, "width": 4, "height": 4, "corner": [5, 3, 3], "operation": "join"}));
    let st = s.model.state();
    assert_eq!(st.bodies.len(), 1, "both boxes and the bar are one body");
    assert_eq!(st.bodies[0].name, "Body1");
    assert_eq!(st.bodies[0].body.lumps().unwrap().len(), 1);
    // 2 boxes + the bar's 10 mm outside them.
    assert!(rel(volume(&mut s), 2000.0 + 10.0 * 16.0) < 1e-6);
}

/// A join touching only the second body merges into that body and leaves the first alone.
#[test]
fn join_merges_into_the_body_it_meets() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10}));
    run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10, "corner": [20, 0, 0], "operation": "new_body"}));
    // Sits on Body2's top face: a face contact, no shared volume.
    run(&mut s, "solid.box", json!({"length": 4, "width": 4, "height": 4, "corner": [23, 3, 10], "operation": "join"}));
    let st = s.model.state();
    assert_eq!(st.bodies.len(), 2);
    let b1 = st.bodies.iter().find(|b| b.name == "Body1").unwrap();
    let b2 = st.bodies.iter().find(|b| b.name == "Body2").unwrap();
    assert_eq!(b1.body.face_count(), 6, "Body1 is untouched");
    assert_eq!(b2.body.lumps().unwrap().len(), 1);
    assert!(rel(volume(&mut s), 2064.0) < 1e-6);
}

/// A join that meets no body still joins the first body, as a separate lump.
#[test]
fn join_meeting_nothing_keeps_the_first_body() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10}));
    run(&mut s, "solid.box", json!({"length": 4, "width": 4, "height": 4, "corner": [40, 0, 0], "operation": "join"}));
    let st = s.model.state();
    assert_eq!(st.bodies.len(), 1);
    assert_eq!(st.bodies[0].body.lumps().unwrap().len(), 2);
    assert!(rel(volume(&mut s), 1064.0) < 1e-6);
}

/// A 1 mm floor extruded into a through hole, from the sketch the plate was extruded from: the
/// disc's wall lies on the hole's and its foot in the plate's bottom face. A wedge extruded
/// sideways on top keeps the plate from being a prism along Z (the 2D route would take it
/// otherwise), and the join failed with "boolean Union: the operation failed: no result".
#[test]
fn join_a_floor_into_a_through_hole() {
    let mut s = Session::default();
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Plate"}));
    run(&mut s, "sketch.rectangle.two_point", json!({"p0": [0, 0], "p1": [100, 60]}));
    run(&mut s, "sketch.circle.center", json!({"center": [60, 30], "radius": 20}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.extrude", json!({"sketch": "Plate", "profiles": [{"point": [5, 5]}], "distance": 10}));
    run(&mut s, "sketch.create", json!({"plane": "XZ", "name": "Wedge"}));
    run(&mut s, "sketch.line", json!({"points": [[5, -10], [25, -10], [5, -16]], "closed": true}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.extrude", json!({"sketch": "Wedge", "distance": 40, "operation": "join"}));
    let before = volume(&mut s);
    run(&mut s, "solid.extrude", json!({"sketch": "Plate", "profiles": [{"point": [60, 30]}], "distance": 1, "operation": "join"}));
    let st = s.model.state();
    assert_eq!(st.bodies.len(), 1);
    assert_eq!(st.bodies[0].body.lumps().unwrap().len(), 1);
    assert!(st.bodies[0].body.validity().is_empty(), "{:?}", st.bodies[0].body.validity());
    // Exact: the disc isn't moved to get the bodies to intersect, so the feature has no warning.
    let warning = s.model.results.last().and_then(|r| r.warning.clone());
    assert!(warning.is_none(), "{warning:?}");
    let added = volume(&mut s) - before;
    assert!(rel(added, PI * 400.0) < 1e-3, "{added}");
}

/// Two spheres of one size: the join's contact test tried every pair of their finely meshed
/// triangles and refused the join as too much work (#232).
#[test]
fn join_of_two_overlapping_equal_spheres_is_not_refused() {
    let mut s = Session::default();
    run(&mut s, "solid.sphere", json!({"radius": 10, "center": [0, 0, 0], "body_name": "A"}));
    run(&mut s, "solid.sphere", json!({"radius": 10, "center": [12, 0, 0], "body_name": "B"}));
    run(&mut s, "solid.combine", json!({"target": "A", "tools": ["B"], "operation": "join"}));
    assert_eq!(s.model.state().bodies.len(), 1);
    // Two spheres less the lens they share: 2 * 4/3 pi r^3 - pi (4r + d) (2r - d)^2 / 12.
    let (r, d) = (10.0, 12.0);
    let want = 2.0 * 4.0 / 3.0 * PI * r * r * r - PI * (4.0 * r + d) * (2.0 * r - d) * (2.0 * r - d) / 12.0;
    let got = volume(&mut s);
    assert!(rel(got, want) < 1e-3, "{got} vs {want}");
}

/// Area two discs of radii `r1`, `r2` with centres `d` apart have in common.
fn disc_overlap(r1: f64, r2: f64, d: f64) -> f64 {
    if d >= r1 + r2 {
        return 0.0;
    }
    if d <= (r1 - r2).abs() {
        return PI * r1.min(r2) * r1.min(r2);
    }
    let a = ((d * d + r1 * r1 - r2 * r2) / (2.0 * d * r1)).acos();
    let b = ((d * d + r2 * r2 - r1 * r1) / (2.0 * d * r2)).acos();
    r1 * r1 * a + r2 * r2 * b - 0.5 * ((-d + r1 + r2) * (d + r1 - r2) * (d - r1 + r2) * (d + r1 + r2)).sqrt()
}

/// Two thick rings 11 mm apart: their walls cross, and the join was refused as too much work (#232).
#[test]
fn join_of_two_overlapping_rings_is_not_refused() {
    let mut s = Session::default();
    for (name, x) in [("RingA", 0.0), ("RingB", 11.0)] {
        run(
            &mut s,
            "solid.cylinder",
            json!({"diameter": 120, "height": 60, "base": [x, 0, 0], "operation": "new", "name": format!("{name}Outer"), "body_name": name}),
        );
        run(
            &mut s,
            "solid.cylinder",
            json!({"diameter": 96, "height": 60, "base": [x, 0, 0], "operation": "cut", "targets": [name], "name": format!("{name}Bore")}),
        );
    }
    run(&mut s, "solid.combine", json!({"target": "RingA", "tools": ["RingB"], "operation": "join"}));
    assert_eq!(s.model.state().bodies.len(), 1);
    // Union of the two rings' plan areas: both less what they share, which is the overlap of the
    // two outer discs less the two bores (each inside the other's outer disc) plus the bores' own overlap.
    let (outer, bore, d) = (60.0, 48.0, 11.0);
    let ring = PI * (outer * outer - bore * bore);
    let shared = disc_overlap(outer, outer, d) - 2.0 * disc_overlap(bore, outer, d) + disc_overlap(bore, bore, d);
    let want = (2.0 * ring - shared) * 60.0;
    let got = volume(&mut s);
    assert!(rel(got, want) < 1e-3, "{got} vs {want}");
}

/// A disc with a lobe on its rim, patterned six times about its centre: every copy shares the
/// whole disc with the target, and joining them was refused as too much contact work (#1029).
#[test]
fn join_of_a_lobed_disc_pattern_is_not_refused() {
    let mut s = Session::default();
    run(&mut s, "sketch.create", json!({"plane": "XY"}));
    run(&mut s, "sketch.circle.center", json!({"center": [0, 0], "diameter": 15}));
    run(&mut s, "sketch.circle.center", json!({"center": [0, 7.5], "diameter": 5}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.extrude", json!({"distance": 10, "profiles": "all", "operation": "new", "body_name": "Disc"}));
    run(&mut s, "solid.pattern.circular", json!({"bodies": ["Disc"], "axis": "Z", "count": 6}));
    let tools: Vec<String> = s.model.state().bodies.iter().map(|b| b.name.clone()).filter(|n| n != "Disc").collect();
    assert_eq!(tools.len(), 5);
    run(&mut s, "solid.combine", json!({"target": "Disc", "tools": tools, "operation": "join"}));
    assert_eq!(s.model.state().bodies.len(), 1);
    // The disc plus six lobes, each less the part inside the disc; neighbouring lobes stay apart.
    let (disc, lobe) = (7.5, 2.5);
    let want = (PI * disc * disc + 6.0 * (PI * lobe * lobe - disc_overlap(disc, lobe, disc))) * 10.0;
    let got = volume(&mut s);
    assert!(rel(got, want) < 1e-3, "{got} vs {want}");
}

mod blend_fit;
mod boolean_diagnostics;
mod component_frames;
mod curved_seam_round;
mod flush_join_loft;
mod gltf_import;
mod import_component;
mod large_step_import;
mod loft_cut;
mod loft_ring;
mod loft_spacing;
mod mirror_placed;
mod mirror_planes;
#[cfg(not(target_arch = "wasm32"))]
mod native_cad_import;
mod nudge_slivers;
mod path_scope;
mod pleat_join;
mod primitive_points;
mod rim_cut;
mod robustness;
mod robustness_goldens;
mod spiral_join;
mod step_import_options;
mod xz_plane;

/// Plan area a round of radius `r` fills at the concave corner `x` where circles (c1, r1) and
/// (c2, r2) cross: the triangle of its contacts and the corner, less the three circular segments.
fn round_gap(c1: [f64; 2], r1: f64, c2: [f64; 2], r2: f64, x: [f64; 2], r: f64) -> f64 {
    let (a, b) = (r1 + r, r2 + r);
    let d = (c2[0] - c1[0]).hypot(c2[1] - c1[1]);
    let u = [(c2[0] - c1[0]) / d, (c2[1] - c1[1]) / d];
    let along = (a * a - b * b + d * d) / (2.0 * d);
    let side = if (x[0] - c1[0]) * -u[1] + (x[1] - c1[1]) * u[0] < 0.0 { -1.0 } else { 1.0 };
    let off = (a * a - along * along).sqrt() * side;
    let f = [c1[0] + u[0] * along - u[1] * off, c1[1] + u[1] * along + u[0] * off];
    let t1 = [c1[0] + (f[0] - c1[0]) * r1 / a, c1[1] + (f[1] - c1[1]) * r1 / a];
    let t2 = [c2[0] + (f[0] - c2[0]) * r2 / b, c2[1] + (f[1] - c2[1]) * r2 / b];
    let angle = |c: [f64; 2], p: [f64; 2], q: [f64; 2]| {
        let (v, w) = ([p[0] - c[0], p[1] - c[1]], [q[0] - c[0], q[1] - c[1]]);
        ((v[0] * w[0] + v[1] * w[1]) / (v[0].hypot(v[1]) * w[0].hypot(w[1]))).clamp(-1.0, 1.0).acos()
    };
    let segment = |r: f64, t: f64| r * r * (t - t.sin()) / 2.0;
    let triangle = ((x[0] - t1[0]) * (t2[1] - t1[1]) - (t2[0] - t1[0]) * (x[1] - t1[1])).abs() / 2.0;
    triangle - segment(r1, angle(c1, t1, x)) - segment(r2, angle(c2, x, t2)) - segment(r, angle(f, t2, t1))
}

/// A profile of two overlapping arcs, extruded: the vertical edges where the two walls meet are
/// straight lines between cylinders, and their rounds end on the caps across curved edges.
#[test]
fn fillet_the_edges_between_two_overlapping_cylinders() {
    let (c1, r1) = ([0.0, 0.0], 17.140595088852663);
    let (c2, r2) = ([10.715766135516247, 13.378055020402751], 9.666944358807896);
    let (p, q) = ([1.7728134670876865, 17.048669520256194], [16.250324167975673, 5.452243981674495]);
    let profile = 1087.5018766864137;
    let mut s = Session::default();
    run(&mut s, "sketch.create", json!({"plane": "XY"}));
    run(&mut s, "sketch.arc.center_point", json!({"center": c1, "start": p, "end": q}));
    run(&mut s, "sketch.arc.center_point", json!({"center": c2, "start": q, "end": p}));
    let si = run(&mut s, "sketch.inspect", json!({}));
    assert!(rel(si["profiles"][0]["area_mm2"].as_f64().unwrap(), profile) < 1e-9, "{si}");
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.extrude", json!({"distance": 10}));
    let counts = |s: &mut Session| {
        let m = run(s, "inspect.measure", json!({}));
        (m["total"]["faces"].as_u64(), m["total"]["edges"].as_u64(), m["total"]["vertices"].as_u64())
    };
    assert_eq!(counts(&mut s), (Some(4), Some(6), Some(4)));
    let (gp, gq) = (round_gap(c1, r1, c2, r2, p, 6.0), round_gap(c1, r1, c2, r2, q, 6.0));
    // One round: a blend face; the edge becomes two contact lines and two end arcs.
    run(&mut s, "solid.fillet", json!({"edges": [[p[0], p[1], 5]], "radius": 6, "name": "Round"}));
    let (v, expect) = (volume(&mut s), (profile + gp) * 10.0);
    assert!(rel(v, expect) < 2e-4, "{v} vs {expect}");
    assert_eq!(counts(&mut s), (Some(5), Some(9), Some(6)));
    // Both corners in one fillet.
    run(
        &mut s,
        "timeline.redefine",
        json!({"feature": "Round", "command": "solid.fillet", "params": {"edges": [[p[0], p[1], 5], [q[0], q[1], 5]], "radius": 6}}),
    );
    let (v, expect) = (volume(&mut s), (profile + gp + gq) * 10.0);
    assert!(rel(v, expect) < 2e-4, "{v} vs {expect}");
    assert_eq!(counts(&mut s), (Some(6), Some(12), Some(8)));
    // A larger round, whose contacts are found on the long arcs from their far ends too.
    run(&mut s, "timeline.redefine", json!({"feature": "Round", "command": "solid.fillet", "params": {"edges": [[q[0], q[1], 5]], "radius": 13}}));
    let (v, expect) = (volume(&mut s), (profile + round_gap(c1, r1, c2, r2, q, 13.0)) * 10.0);
    assert!(rel(v, expect) < 2e-4, "{v} vs {expect}");
}

/// The top outline of two overlapping cylinders whose concave edges were rounded: the walls of
/// the rounds meet the cylinders along the rolling-ball contact lines, straight but not lines.
#[test]
fn fillet_and_chamfer_the_top_loop_of_rounded_overlapping_cylinders() {
    use std::f64::consts::{PI, TAU};
    let (big, round, h) = (25.0_f64, 5.0_f64, 10.0);
    let mut s = Session::default();
    run(&mut s, "sketch.create", json!({"plane": "XY"}));
    run(&mut s, "sketch.circle.center", json!({"center": [0, 0], "radius": big}));
    run(&mut s, "sketch.circle.center", json!({"center": [-big, 0], "radius": big}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.extrude", json!({"distance": h, "profiles": [0, 1, 2]}));
    // Plan view: ball centres at x = −12.5, |y| = yb; each round's arc turns by th, each big arc by
    // TAU − 2·a1, and the loop turns by TAU in all.
    let half = big / 2.0;
    let yb = ((big + round).powi(2) - half * half).sqrt();
    run(&mut s, "solid.fillet", json!({"edges": [[-half, half * 3f64.sqrt(), 5], [-half, -half * 3f64.sqrt(), 5]], "radius": round}));
    let (a1, th) = (yb.atan2(half), 2.0 * (half / (big + round)).asin());
    let base = volume(&mut s);
    let top = [[big, 0.0, h], [-2.0 * big, 0.0, h], [-half, yb - round, h], [-half, round - yb, h]];
    // Removed volume by Pappus: the corner's section swept along the loop at its centroid's offset.
    let removed = |area: f64, xb: f64| area * (2.0 * (TAU - 2.0 * a1) * (big - xb) + 2.0 * th * (round + xb));
    run(&mut s, "solid.fillet", json!({"edges": top, "radius": 2}));
    let r: f64 = 2.0;
    let expect = base - removed(r * r * (1.0 - PI / 4.0), (10.0 - 3.0 * PI) / (3.0 * (4.0 - PI)) * r);
    let v = volume(&mut s);
    assert!((v - expect).abs() < 1.5, "{v} vs {expect}");
    let m = run(&mut s, "inspect.measure", json!({}));
    assert_eq!(m["bodies"][0]["kernel"]["faces"].as_u64(), Some(10), "{m}");
    run(&mut s, "edit.undo", json!({}));
    run(&mut s, "solid.chamfer", json!({"edges": top, "distance": 2}));
    let expect = base - removed(2.0, 2.0 / 3.0);
    let v = volume(&mut s);
    assert!((v - expect).abs() < 1.5, "{v} vs {expect}");
}

/// Sketch a 40 x 30 rectangle with dimensions, extrude 20, fillet a vertical edge, cut a hole.
#[test]
fn box_fillet_cut_part() {
    let mut s = Session::default();
    run(&mut s, "parameters.change", json!({"name": "width", "expression": "40 mm"}));
    run(&mut s, "sketch.create", json!({"plane": "XY"}));
    let r = run(&mut s, "sketch.rectangle.two_point", json!({"p0": [0, 0], "p1": [38, 29]}));
    let l: Vec<String> = r["curves"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect();
    let corner = s.doc.sketch(s.active_sketch.unwrap()).unwrap().curves[0].clone();
    let start = match corner.kind {
        solidcraft_sketch::CurveKind::Line { a, .. } => s.doc.sketch(s.active_sketch.unwrap()).unwrap().points[a].id.clone(),
        _ => panic!(),
    };
    run(&mut s, "sketch.constraint.coincident", json!({"a": start, "b": "origin"}));
    run(&mut s, "sketch.dimension", json!({"entities": [l[0]], "value": "width"}));
    run(&mut s, "sketch.dimension", json!({"entities": [l[1]], "value": 30}));
    let si = run(&mut s, "sketch.inspect", json!({}));
    assert_eq!(si["dof"], 0, "{si}");
    assert_eq!(si["profiles"].as_array().unwrap().len(), 1);
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.extrude", json!({"distance": 20}));
    assert!(rel(volume(&mut s), 24000.0) < 1e-9);
    run(&mut s, "solid.fillet", json!({"edges": [[0, 0, 10]], "radius": 3}));
    let filleted = 24000.0 - (9.0 - PI * 9.0 / 4.0) * 20.0;
    assert!(rel(volume(&mut s), filleted) < 2e-4);
    run(&mut s, "sketch.create", json!({"plane": "XY"}));
    run(&mut s, "sketch.circle.center", json!({"center": [20, 15], "radius": 5}));
    run(&mut s, "solid.extrude", json!({"distance": 20, "operation": "cut"}));
    let expect = filleted - PI * 25.0 * 20.0;
    let v = volume(&mut s);
    assert!(rel(v, expect) < 5e-4, "{v} vs {expect}");
    let m = run(&mut s, "inspect.measure", json!({}));
    assert_eq!(m["bodies"][0]["faces"], 8, "{m}");

    // Parameter edit re-evaluates the timeline from the sketch.
    let r = run(&mut s, "parameters.change", json!({"name": "width", "expression": "50"}));
    assert_eq!(r["errors"].as_array().unwrap().len(), 0, "{r}");
    let expect2 = 30000.0 - (9.0 - PI * 9.0 / 4.0) * 20.0 - PI * 25.0 * 20.0;
    assert!(rel(volume(&mut s), expect2) < 5e-4);

    // Undo the parameter change, redo it.
    run(&mut s, "edit.undo", json!({}));
    assert!(rel(volume(&mut s), expect) < 5e-4);
    run(&mut s, "edit.redo", json!({}));
    assert!(rel(volume(&mut s), expect2) < 5e-4);

    // Export STL and STEP.
    let dir = std::env::temp_dir().join(format!("solidcraft-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let stl = dir.join("part.stl");
    run(&mut s, "file.export", json!({"path": stl.to_string_lossy()}));
    assert!(std::fs::metadata(&stl).unwrap().len() > 1000);
    let step = dir.join("part.step");
    run(&mut s, "file.export", json!({"path": step.to_string_lossy()}));
    assert!(std::fs::read_to_string(&step).unwrap().contains("ISO-10303-21"));
    let design = dir.join("part.solidcraft");
    run(&mut s, "file.save_as", json!({"path": design.to_string_lossy()}));
    let mut s2 = Session::default();
    run(&mut s2, "doc.open", json!({"path": design.to_string_lossy()}));
    assert!(rel(volume(&mut s2), expect2) < 5e-4);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn revolve_and_primitives() {
    let mut s = Session::default();
    run(&mut s, "sketch.create", json!({"plane": "XZ"}));
    run(&mut s, "sketch.line", json!({"points": [[10, 0], [15, 0], [15, 5], [10, 5]], "closed": true}));
    run(&mut s, "solid.revolve", json!({"axis": "y"}));
    assert!(rel(volume(&mut s), PI * 125.0 * 5.0) < 5e-4);
    run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10, "corner": [30, 0, 0]}));
    run(&mut s, "solid.cylinder", json!({"diameter": 4, "height": 20, "base": [35, 5, -5]}));
    run(&mut s, "solid.combine", json!({"target": "Body2", "tools": ["Body3"], "operation": "cut"}));
    let m = run(&mut s, "inspect.measure", json!({}));
    assert_eq!(m["body_count"], 2, "{m}");
    assert!(rel(m["bodies"][1]["volume_mm3"].as_f64().unwrap(), 1000.0 - PI * 4.0 * 10.0) < 5e-4);
    run(&mut s, "solid.chamfer", json!({"edges": [[30, 5, 10]], "distance": 1}));
    run(&mut s, "timeline.rollback", json!({"position": 3}));
    assert_eq!(run(&mut s, "inspect.measure", json!({}))["body_count"], 2);
    run(&mut s, "timeline.rollback", json!({}));
    run(&mut s, "timeline.edit", json!({"feature": "Revolve1", "set": {"angle": "180 deg"}}));
    let m = run(&mut s, "inspect.measure", json!({"bodies": ["Body1"]}));
    assert!(rel(m["total"]["volume_mm3"].as_f64().unwrap(), PI * 125.0 * 5.0 / 2.0) < 5e-4);
}

/// #726: a revolve swept each profile edge into two or three patches and left the planar ones
/// apart, so a pick on such a face took part of it, and a sketch or an extrude there was refused
/// ("the selected planar face no longer exists") until a later feature merged them.
#[test]
fn revolved_planar_faces_come_out_whole() {
    for angle in [360, 240] {
        let mut s = Session::default();
        run(&mut s, "sketch.create", json!({"plane": "XZ"}));
        run(&mut s, "sketch.line", json!({"points": [[8, 0], [30, 0], [30, -6], [18, -6], [18, -22], [8, -22]], "closed": true}));
        run(&mut s, "sketch.finish", json!({}));
        run(&mut s, "solid.revolve", json!({"axis": "Z", "angle": angle}));
        // Picks at 170°, in the last patch, which 240° covers whichever way it turns.
        let at = |r: f64, z: f64| json!([r * 170f64.to_radians().cos(), r * 170f64.to_radians().sin(), z]);
        // The flange's top annulus (z = 6, r 18…30), well inside it.
        let sk = run(&mut s, "sketch.create", json!({"plane": {"face": at(24.0, 6.0)}}));
        assert_eq!(sk["attachment"], "exact_face", "{angle}°: {sk}");
        run(&mut s, "sketch.finish", json!({}));
        // The bottom annulus (z = 0, r 8…30).
        let v = volume(&mut s);
        run(&mut s, "solid.extrude", json!({"face": at(20.0, 0.0), "distance": 2, "operation": "join"}));
        assert!(volume(&mut s) > v + 1.0, "{angle}°: the extrude added nothing");
    }
}

/// A spherical shell (a half ring between two arcs, both radial lines on the axis) revolved on a
/// sketch plane that does not pass through the origin: the body is the shell, as it is on XY.
/// Before, built in place, it measured a third too large (truck tessellates the degenerate faces
/// on the axis reliably only with the axis through the origin).
#[test]
fn revolve_a_spherical_shell_off_the_origin() {
    for (plane, z) in [(json!("XY"), 0.0), (json!({"origin": [0, 0, 10], "x_dir": [1, 0, 0], "y_dir": [0, 1, 0]}), 10.0)] {
        let mut s = Session::default();
        run(&mut s, "sketch.create", json!({"plane": plane, "name": "Ring"}));
        run(&mut s, "sketch.line", json!({"points": [[-40, 0], [40, 0]]}));
        run(&mut s, "sketch.arc.center_point", json!({"center": [0, 0], "start": [40, 0], "end": [-40, 0]}));
        run(&mut s, "sketch.arc.center_point", json!({"center": [0, 0], "start": [18.5, 0], "end": [-18.5, 0]}));
        run(&mut s, "sketch.finish", json!({}));
        // The half ring: the larger of the two regions above the axis.
        let si = run(&mut s, "sketch.inspect", json!({"sketch": "Ring"}));
        let ring = si["profiles"]
            .as_array()
            .unwrap()
            .iter()
            .max_by(|a, b| a["area_mm2"].as_f64().unwrap().total_cmp(&b["area_mm2"].as_f64().unwrap()))
            .unwrap()["index"]
            .clone();
        run(&mut s, "solid.revolve", json!({"sketch": "Ring", "profiles": [ring], "axis": "l1", "body_name": "Shell"}));
        let m = run(&mut s, "inspect.measure", json!({"bodies": ["Shell"]}));
        let want = 4.0 / 3.0 * PI * (40.0f64.powi(3) - 18.5f64.powi(3));
        let v = m["total"]["volume_mm3"].as_f64().unwrap();
        assert!(rel(v, want) < 5e-4, "plane at z {z}: {v} vs {want}");
        let b = &m["bodies"][0]["bbox"];
        assert!((b["min"][2].as_f64().unwrap() - (z - 40.0)).abs() < 1e-6 && (b["max"][2].as_f64().unwrap() - (z + 40.0)).abs() < 1e-6, "{b}");
    }
}

#[test]
fn extrude_of_adjacent_profiles_has_whole_edges() {
    // A holder from two regions of one sketch (a disc and the ring around it, both cut off by a
    // chord): the chord used to stay split where the inner circle met it, leaving vertices right
    // on the surface of a handle of that radius, and cutting the handle out of the holder failed.
    let mut s = Session::default();
    run(&mut s, "sketch.create", json!({"plane": "XZ", "name": "Holder"}));
    run(&mut s, "sketch.circle.center", json!({"center": [0, 0], "radius": 40}));
    run(&mut s, "sketch.circle.center", json!({"center": [0, 0], "radius": 35}));
    let x = (40.0f64 * 40.0 - 33.0 * 33.0).sqrt();
    run(&mut s, "sketch.line", json!({"points": [[-x, -33], [x, -33]]}));
    run(&mut s, "sketch.finish", json!({}));
    let si = run(&mut s, "sketch.inspect", json!({"sketch": "Holder"}));
    let below: Vec<Value> = si["profiles"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| (500.0..4900.0).contains(&p["area_mm2"].as_f64().unwrap()))
        .map(|p| p["index"].clone())
        .collect();
    assert_eq!(below.len(), 2, "{si}");
    run(&mut s, "solid.extrude", json!({"sketch": "Holder", "profiles": below, "distance": 23, "body_name": "Holder"}));
    let topo = run(&mut s, "model.faces", json!({"body": "Holder"}));
    assert_eq!(topo["faces"].as_array().unwrap().len(), 5, "a cut-off cylinder: {topo}");
    let lo = run(&mut s, "inspect.measure", json!({"bodies": ["Holder"]}))["bodies"][0]["bbox"]["min"][1].as_f64().unwrap();
    run(&mut s, "solid.move", json!({"bodies": ["Holder"], "translate": [0, -5.0 - lo, 0]}));
    // The handle: r35 on the Y axis with a bevelled end.
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Handle"}));
    for pts in [
        [[0.0, 0.0], [0.0, 230.0]],
        [[0.0, 0.0], [25.0, 0.0]],
        [[25.0, 0.0], [35.0, 11.5]],
        [[35.0, 11.5], [35.0, 230.0]],
        [[35.0, 230.0], [0.0, 230.0]],
    ] {
        run(&mut s, "sketch.line", json!({ "points": pts }));
    }
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.revolve", json!({"sketch": "Handle", "profiles": [0], "axis": "Y", "body_name": "Handle"}));
    let before = run(&mut s, "inspect.measure", json!({"bodies": ["Holder"]}))["total"]["volume_mm3"].as_f64().unwrap();
    run(&mut s, "solid.combine", json!({"target": "Holder", "tools": ["Handle"], "operation": "cut", "keep_tools": true}));
    let after = run(&mut s, "inspect.measure", json!({"bodies": ["Holder"]}))["total"]["volume_mm3"].as_f64().unwrap();
    assert!(after > 1000.0 && after < before * 0.5, "{before} -> {after}");
}

#[test]
fn sketch_tools_and_constraints() {
    let mut s = Session::default();
    run(&mut s, "sketch.create", json!({"plane": "YZ"}));
    run(&mut s, "sketch.rectangle.center", json!({"center": [0, 0], "corner": [10, 5]}));
    run(&mut s, "sketch.rectangle.three_point", json!({"p0": [20, 0], "p1": [30, 5], "p2": [28, 10]}));
    run(&mut s, "sketch.circle.two_point", json!({"p0": [40, 0], "p1": [44, 0]}));
    run(&mut s, "sketch.circle.three_point", json!({"p0": [50, 0], "p1": [52, 2], "p2": [54, 0]}));
    run(&mut s, "sketch.arc.three_point", json!({"start": [60, 0], "end": [64, 0], "through": [62, 2]}));
    run(&mut s, "sketch.arc.center_point", json!({"center": [70, 0], "start": [72, 0], "end": [70, 2]}));
    run(&mut s, "sketch.polygon.inscribed", json!({"center": [80, 0], "radius": 3, "sides": 6}));
    run(&mut s, "sketch.polygon.circumscribed", json!({"center": [90, 0], "radius": 3, "sides": 5}));
    run(&mut s, "sketch.polygon.edge", json!({"p0": [100, 0], "p1": [103, 0], "sides": 4}));
    run(&mut s, "sketch.slot.center_to_center", json!({"p0": [110, 0], "p1": [120, 0], "width": 4}));
    run(&mut s, "sketch.slot.overall", json!({"p0": [130, 0], "p1": [140, 0], "width": 4}));
    run(&mut s, "sketch.point", json!({"point": [150, 0]}));
    let r = run(&mut s, "sketch.line", json!({"points": [[0, 20], [10, 21], [20, 30]], "ids": ["m1", "m2"]}));
    assert_eq!(r["curves"], json!(["m1", "m2"]));
    run(&mut s, "sketch.constraint.horizontal_vertical", json!({"line": "m1"}));
    run(&mut s, "sketch.constraint.perpendicular", json!({"a": "m1", "b": "m2"}));
    let _ = s.execute("sketch.dimension", &json!({"entities": ["m1", "m2"], "type": "angle", "value": "90 deg"}));
    let si = run(&mut s, "sketch.inspect", json!({}));
    assert_eq!(si["status"], "solved", "{}", si["failing"]);
    // Over-constraining is rejected and leaves the sketch unchanged.
    let before = s.doc.clone();
    assert!(s.execute("sketch.constraint.parallel", &json!({"a": "m1", "b": "m2"})).is_err());
    assert_eq!(*s.doc, *before);
    // Profiles: rectangles, circles, closed arcs/polygons/slots.
    let n = si["profiles"].as_array().unwrap().len();
    assert!(n >= 8, "{n} profiles");
    run(&mut s, "sketch.construction", json!({"curves": ["m1"]}));
    run(&mut s, "sketch.delete", json!({"entities": ["m2"]}));
    run(&mut s, "sketch.constraint.fix", json!({"entity": "m1"}));
    run(&mut s, "sketch.move_point", json!({"point": "m1.end", "to": [15, 20]}));
    run(&mut s, "sketch.finish", json!({}));
    assert!(s.execute("sketch.line", &json!({"points": [[0, 0], [1, 1]]})).is_err(), "needs an active sketch");
}

/// The recipe format of the Fusion oracle (01-box) drives the same commands.
#[test]
fn script_runs_commands() {
    let mut s = Session::default();
    let script = json!({"commands": [
        {"command": "sketch.create", "params": {"plane": "XY"}},
        {"command": "sketch.line", "params": {"points": [[0, 0], [40, 0], [40, 30], [0, 30]], "closed": true, "ids": ["l1", "l2", "l3", "l4"]}},
        {"command": "solid.extrude", "params": {"distance": 20, "profiles": [{"loops": [{"outer": true, "curves": ["l1", "l2", "l3", "l4"]}]}], "body_name": "Box"}},
    ]});
    let out = s.run_script(&script).unwrap();
    assert_eq!(out.len(), 3);
    let m = run(&mut s, "inspect.measure", json!({}));
    assert_eq!(m["bodies"][0]["name"], "Box");
    assert_eq!((m["total"]["faces"].as_u64(), m["total"]["edges"].as_u64(), m["total"]["vertices"].as_u64()), (Some(6), Some(12), Some(8)));
    assert!(s.run_script(&json!([{"command": "Nope"}])).is_err());
}

pub(crate) fn hostile_values() -> Vec<Value> {
    vec![
        Value::Null,
        json!(true),
        json!(-1),
        json!(0),
        json!(1e308),
        json!(-1e308),
        json!(""),
        json!("nope"),
        json!("1/0"),
        json!("((((((((((1"),
        json!([]),
        json!([1e308, -1e308]),
        json!([0, 0, 0]),
        json!([[0, 0], [0, 0]]),
        json!(["origin", "origin"]),
        json!({}),
        json!({"x": 1}),
        json!([{"point": [0, 0]}]),
        json!([{"body": "Body1", "index": 999999}]),
    ]
}

/// Every command, every parameter name it documents, every hostile value: no panic escapes and
/// no command panics internally.
#[test]
fn hostile_params_never_panic() {
    let keys = [
        "plane",
        "offset",
        "name",
        "sketch",
        "points",
        "closed",
        "construction",
        "ids",
        "infer",
        "p0",
        "p1",
        "p2",
        "center",
        "corner",
        "radius",
        "diameter",
        "start",
        "end",
        "through",
        "sides",
        "angle",
        "width",
        "point",
        "id",
        "entities",
        "type",
        "value",
        "line",
        "mode",
        "a",
        "b",
        "entity",
        "fixed",
        "curves",
        "to",
        "distance",
        "profiles",
        "direction",
        "distance2",
        "start_offset",
        "operation",
        "targets",
        "body_name",
        "axis",
        "length",
        "height",
        "base",
        "major",
        "minor",
        "edges",
        "body",
        "target",
        "tools",
        "keep_tools",
        "bodies",
        "translate",
        "expression",
        "unit",
        "comment",
        "delete",
        "features",
        "feature",
        "position",
        "suppressed",
        "set",
        "items",
        "add",
        "path",
        "format",
        "ascii",
        "measure",
        "refs",
        "link",
        "project_edges",
        "mid",
        "near",
        "factor",
        "copy",
        "count",
        "spacing",
        "count2",
        "spacing2",
        "dir",
        "side",
        "chain",
        "at",
        "curve",
        "minor_radius",
        "apex",
        "rho",
        "text",
        "lines",
        "driven",
        "from",
        "datum",
        "tolerance",
        "face",
        "new_name",
        "favorite",
        "favorites",
        "filter",
        "kind",
        "prefix",
        "orientation",
        "twist",
        "scale",
        "continuity",
        "crown",
        "faces",
    ];
    // Commands are independent; worker threads take them one at a time (their costs vary a lot).
    let ids: Vec<&str> = command_specs()
        .iter()
        .map(|spec| spec.id)
        .filter(|id| !matches!(*id, "doc.open" | "file.save" | "file.save_as" | "file.export" | "file.save_mesh" | "sketch.export_dxf")) // file system side effects are covered by their own tests
        .collect();
    let next = std::sync::atomic::AtomicUsize::new(0);
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let internal: Vec<String> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(|| {
                    let mut internal = Vec::new();
                    while let Some(&id) = ids.get(next.fetch_add(1, std::sync::atomic::Ordering::Relaxed)) {
                        for v in hostile_values() {
                            for k in keys {
                                if k == "path" && (id.starts_with("parameters.") || id == "timeline.export_script") {
                                    continue; // parameter and script files: covered by their own tests (no stray files)
                                }
                                let mut s = Session::default();
                                // A model with a sketch, a body and an active sketch, so most commands get far.
                                let _ = s.execute("solid.box", &json!({"length": 10, "width": 10, "height": 10}));
                                let _ = s.execute("sketch.create", &json!({"plane": "XY"}));
                                let _ = s.execute("sketch.rectangle.two_point", &json!({"p0": [0, 0], "p1": [5, 5]}));
                                let p = json!({ k: v.clone() });
                                if let Err(EngineError::Internal(id, msg)) = s.execute(id, &p) {
                                    internal.push(format!("{id} {p}: {msg}"));
                                }
                                // Whole-value hostility too.
                                if let Err(EngineError::Internal(id, msg)) = s.execute(id, &v) {
                                    internal.push(format!("{id} {v}: {msg}"));
                                }
                            }
                        }
                    }
                    internal
                })
            })
            .collect();
        workers.into_iter().flat_map(|w| w.join().unwrap()).collect()
    });
    assert!(internal.is_empty(), "commands panicked:\n{}", internal.join("\n"));
}

#[test]
fn registry_is_consistent() {
    let specs = command_specs();
    let mut ids: Vec<&str> = specs.iter().map(|c| c.id).collect();
    ids.sort();
    let n = ids.len();
    ids.dedup();
    assert_eq!(ids.len(), n, "duplicate command ids");
    assert_eq!(find_command("Solid.Extrude").map(|c| c.id), Some("solid.extrude"), "case-insensitive lookup");
    let s = Session::default();
    assert!(!find_command("sketch.line").unwrap().info(&s).enabled);
}

/// Only SolidCraft's own dotted ids name commands; other ids are unknown commands.
#[test]
fn only_own_ids_name_commands() {
    for old in ["Extrude", "SketchCreate", "FusionFilletEdgesCommand"] {
        assert!(find_command(old).is_none(), "{old}");
        let mut s = Session::default();
        let err = s.execute(old, &serde_json::json!({})).unwrap_err();
        assert!(matches!(err, crate::EngineError::UnknownCommand(_)), "{old}: {err}");
    }
}

/// An unknown command's error names the ids it was probably meant to be: a typo's, or the
/// command that ends in the same word.
#[test]
fn unknown_commands_suggest_the_closest_ids() {
    let mut s = Session::default();
    let mut msg = |id: &str| s.execute(id, &serde_json::json!({})).unwrap_err().to_string();
    assert_eq!(msg("file.open"), "unknown command `file.open` (did you mean `doc.open`?)");
    assert!(msg("solid.extrud").starts_with("unknown command `solid.extrud` (did you mean `solid.extrude`"), "{}", msg("solid.extrud"));
    assert!(msg("Extrude").contains("`solid.extrude`"), "{}", msg("Extrude"));
    assert_eq!(msg("FusionFilletEdgesCommand"), "unknown command `FusionFilletEdgesCommand`");
    let long = "x".repeat(10_000);
    assert_eq!(msg(&long), format!("unknown command `{long}`"));
}

#[test]
fn sample_design_builds() {
    let mut s = Session::default();
    s.run_script(&crate::sample::script()).unwrap();
    let m = run(&mut s, "inspect.measure", json!({}));
    assert_eq!(m["body_count"], 1, "{m}");
    let plate = 80.0 * 50.0 * 8.0 - 4.0 * (36.0 - PI * 9.0) * 8.0 - 2.0 * PI * 16.0 * 8.0;
    let boss = PI * 144.0 * 14.0;
    let bore = PI * 36.0 * 22.0;
    let v = m["total"]["volume_mm3"].as_f64().unwrap();
    assert!(rel(v, plate + boss - bore) < 1e-3, "{v} vs {}", plate + boss - bore);
}

fn silhouette_box(s: &mut Session, name: &str, x0: f64, x1: f64, z: f64) {
    let sketch = format!("{name}Sketch");
    run(s, "sketch.create", json!({"plane": "XY", "name": sketch}));
    run(s, "sketch.rectangle.two_point", json!({"p0": [x0, 0], "p1": [x1, 10]}));
    run(s, "sketch.finish", json!({}));
    run(s, "solid.extrude", json!({"sketch": sketch, "distance": z, "operation": "new_body", "body_name": name}));
}

fn silhouette_pixels(v: &Value) -> usize {
    v["mask"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row.as_array().unwrap().iter().map(|span| (span[1].as_u64().unwrap() - span[0].as_u64().unwrap()) as usize).sum::<usize>())
        .sum()
}

#[test]
fn silhouette_box_standard_views_and_full_mask() {
    let mut s = Session::default();
    silhouette_box(&mut s, "Box", 0.0, 20.0, 30.0);
    let front = run(&mut s, "inspect.silhouette", json!({"rows": 60}));
    assert_eq!(front["box_mm"], json!([[0.0, 0.0], [20.0, 30.0]]));
    assert_eq!(front["mm_per_px"], 0.5);
    assert_eq!((front["width"].as_u64(), front["height"].as_u64()), (Some(40), Some(60)));
    assert_eq!(silhouette_pixels(&front), 2400);
    assert_eq!(front["bodies"], 1);
    let right = run(&mut s, "inspect.silhouette", json!({"view": "right", "rows": 60}));
    assert_eq!(right["box_mm"], json!([[0.0, 0.0], [10.0, 30.0]]));
    assert_eq!((right["width"].as_u64(), right["height"].as_u64()), (Some(20), Some(60)));
    let top = run(&mut s, "inspect.silhouette", json!({"view": "top", "rows": 60}));
    assert_eq!(top["box_mm"], json!([[0.0, 0.0], [20.0, 10.0]]));
    assert_eq!((top["width"].as_u64(), top["height"].as_u64()), (Some(120), Some(60)));
}

#[test]
fn silhouette_front_and_back_mirror_tall_body() {
    let mut s = Session::default();
    silhouette_box(&mut s, "Tall", 0.0, 10.0, 30.0);
    silhouette_box(&mut s, "Short", 30.0, 40.0, 10.0);
    let front = run(&mut s, "inspect.silhouette", json!({"view": "front", "rows": 60}));
    let back = run(&mut s, "inspect.silhouette", json!({"view": "back", "rows": 60}));
    assert_eq!(front["mask"][0], json!([[0, 20]]));
    assert_eq!(back["mask"][0], json!([[60, 80]]));
    assert_eq!(front["right"], json!([1.0, 0.0, 0.0]));
    assert_eq!(back["right"], json!([-1.0, 0.0, 0.0]));
}

#[test]
fn silhouette_visibility_selection_and_bad_input() {
    let mut s = Session::default();
    silhouette_box(&mut s, "Shown", 0.0, 20.0, 30.0);
    silhouette_box(&mut s, "Hidden", 40.0, 50.0, 10.0);
    run(&mut s, "browser.visibility", json!({"items": ["Hidden"], "visible": false}));
    assert_eq!(run(&mut s, "inspect.silhouette", json!({}))["bodies"], 1);
    let named = run(&mut s, "inspect.silhouette", json!({"bodies": ["Hidden"]}));
    assert_eq!(named["bodies"], 1);
    assert_eq!(named["box_mm"], json!([[40.0, 0.0], [50.0, 10.0]]));
    assert!(s.execute("inspect.silhouette", &json!({"bodies": ["Unknown"]})).unwrap_err().to_string().contains("Unknown"));
    assert!(s.execute("inspect.silhouette", &json!({"view": "front", "camera": {}})).is_err());
    assert!(s.execute("inspect.silhouette", &json!({"camera": {"target": [0,0,0], "yaw": 0, "pitch": 0, "distance": 0}})).is_err());
}

#[test]
fn silhouette_perspective_scales_the_near_face_and_refuses_bodies_behind_the_eye() {
    let mut s = Session::default();
    silhouette_box(&mut s, "Box", 0.0, 20.0, 30.0);
    // Looking along +Y at the box's centre from 100 mm: the near face (Y 0) is 95 mm from the
    // eye, so it is drawn 100/95 times its size on the plane through the target.
    let camera = json!({"target": [10, 5, 15], "yaw": 0, "pitch": 0, "distance": 100, "fov": 0.6});
    let perspective = run(&mut s, "inspect.silhouette", json!({"camera": camera, "rows": 60}));
    assert_eq!(perspective["projection"], "perspective");
    assert_eq!(perspective["box_mm"], json!([[-0.5263, -0.7895], [20.5263, 30.7895]]));
    let flat = json!({"target": [10, 5, 15], "yaw": 0, "pitch": 0, "distance": 100});
    let parallel = run(&mut s, "inspect.silhouette", json!({"camera": flat, "rows": 60}));
    assert_eq!(parallel["projection"], "parallel");
    assert_eq!(parallel["box_mm"], json!([[0.0, 0.0], [20.0, 30.0]]));
    let inside = json!({"target": [10, 5, 15], "yaw": 0, "pitch": 0, "distance": 1, "fov": 0.6});
    assert!(s.execute("inspect.silhouette", &json!({"camera": inside})).unwrap_err().to_string().contains("behind the camera"));
    // Between two bodies, looking -X from X 28: Short (X 30..40) is behind the eye.
    silhouette_box(&mut s, "Short", 30.0, 40.0, 10.0);
    let between = json!({"target": [25, 5, 15], "yaw": -std::f64::consts::FRAC_PI_2, "pitch": 0, "distance": 3, "fov": 0.6});
    assert!(s.execute("inspect.silhouette", &json!({"camera": between})).unwrap_err().to_string().contains("behind the camera"));
    assert_eq!(run(&mut s, "inspect.silhouette", json!({"camera": between, "bodies": ["Box"]}))["bodies"], 1);
}

#[test]
fn silhouette_is_an_agent_command_not_a_toolbar_button() {
    let specs = command_specs();
    let spec = specs.iter().find(|c| c.id == "inspect.silhouette").unwrap();
    assert!(spec.tab.is_empty(), "{} › {}", spec.tab, spec.panel);
    assert!(!spec.undoable);
}

#[test]
fn sample_edges_chain_and_faces() {
    let mut s = Session::default();
    s.run_script(&crate::sample::script()).unwrap();
    let st = s.model.state();
    let m = st.bodies[0].mesh();
    let e = (0..m.edges.len()).find(|i| m.edges[*i].iter().all(|p| (p.z - 8.0).abs() < 1e-6 && p.y.abs() < 1e-6)).expect("front top edge");
    let chain = m.tangent_chain(e, 2f64.to_radians());
    // The plate's top outline: 4 lines and 4 corner arcs (a boolean may leave an edge twice,
    // once per face; count distinct ones).
    let mut mids: Vec<solidcraft_geom::Vec3> = Vec::new();
    for i in &chain {
        let p = &m.edges[*i];
        let mid = p[p.len() / 2];
        assert!(p.iter().all(|q| (q.z - 8.0).abs() < 1e-6), "{p:?}");
        if !mids.iter().any(|q| q.dist(mid) < 1e-6) {
            mids.push(mid);
        }
    }
    assert_eq!(mids.len(), 8, "{chain:?}");
    // Every edge bounds a face, and the top face has the outline and the hole rims.
    assert!(m.edge_faces.iter().all(|f| !f.is_empty()));
}

/// timeline.inspect returns a feature's definition in the shape timeline.edit's `set` takes, so
/// an agent can read the field names instead of guessing them.
#[test]
fn timeline_inspect_shows_what_timeline_edit_changes() {
    let mut s = Session::default();
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Base"}));
    run(&mut s, "sketch.rectangle.two_point", json!({"p0": [0, 0], "p1": [40, 30]}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.extrude", json!({"distance": 10, "name": "Slab"}));
    run(&mut s, "solid.fillet", json!({"edges": [[20, 0, 10]], "radius": 2, "name": "Round"}));
    let i = run(&mut s, "timeline.inspect", json!({"feature": "Slab"}));
    assert_eq!(i["type"], "ExtrudeFeature", "{i}");
    assert_eq!(i["definition"]["extent"]["distance"], "10", "{i}");
    assert_eq!(i["definition"]["operation"], "new_body", "{i}");
    assert!(i["definition"].get("type").is_none() && i["definition"].get("param_names").is_none(), "{i}");
    assert_eq!(i["inputs"][0]["label"], "Distance", "{i}");
    assert_eq!(i["inputs"][0]["unit"], "mm", "{i}");
    let d = i["inputs"][0]["param"].as_str().unwrap().to_string();
    // The definition's field names are what timeline.edit takes.
    run(&mut s, "timeline.edit", json!({"feature": "Slab", "set": {"extent": {"distance": "12"}}}));
    let i = run(&mut s, "timeline.inspect", json!({"feature": i["id"]}));
    assert_eq!(i["definition"]["extent"]["distance"], "12", "{i}");
    assert_eq!(i["inputs"][0]["param"], d.as_str(), "{i}");
    let f = run(&mut s, "timeline.inspect", json!({"feature": "Round"}));
    assert_eq!(f["definition"]["radius"], "2", "{f}");
    run(&mut s, "timeline.edit", json!({"feature": "Round", "set": {"radius": "3"}}));
    assert_eq!(run(&mut s, "timeline.inspect", json!({"feature": "Round"}))["definition"]["radius"], "3");
    // A sketch leaves its geometry out (sketch.inspect has it) and says so.
    let k = run(&mut s, "timeline.inspect", json!({"feature": "Base"}));
    assert!(k["definition"].get("sketch").is_none() && k["definition"].get("plane").is_some(), "{k}");
    assert_eq!(k["omitted"][0]["field"], "sketch", "{k}");
    // Read-only: no undo step; bad input is refused.
    let undo = s.undo.len();
    run(&mut s, "timeline.inspect", json!({"feature": "Slab"}));
    assert_eq!(s.undo.len(), undo);
    for bad in [json!({}), json!({"feature": "Nope"}), json!({"feature": [1]}), json!({"feature": 1e300}), json!({"feature": null})] {
        assert!(s.execute("timeline.inspect", &bad).is_err(), "{bad}");
    }
}

#[test]
fn timeline_edits_reresolve_references() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 40, "width": 30, "height": 20, "name": "Base"}));
    run(&mut s, "solid.fillet", json!({"edges": [[20, 0, 20]], "radius": 3, "name": "Round"}));
    run(&mut s, "solid.hole", json!({"position": [20, 15, 20], "diameter": 6, "name": "Bore"}));
    let fillet = |l: f64| (9.0 - 9.0 * PI / 4.0) * l;
    let expect = |l: f64, h: f64| l * 30.0 * h - fillet(l) - PI * 9.0 * h;
    assert!(rel(volume(&mut s), expect(40.0, 20.0)) < 1e-3);
    // Taller box: the fillet's edge point is now 5 mm below the edge; the edge is found by its
    // name (no warning) and the hole moves up with the top face.
    run(&mut s, "timeline.edit", json!({"feature": "Base", "set": {"height": "25"}}));
    assert!(rel(volume(&mut s), expect(40.0, 25.0)) < 1e-3, "{}", volume(&mut s));
    let round = s.doc.find_feature("Round").map(|f| f.id).unwrap();
    assert_eq!(s.model.result(round).and_then(|r| r.warning.clone()), None);
    // Longer box: the point still lies on the (longer) edge.
    run(&mut s, "timeline.edit", json!({"feature": "Base", "set": {"length": "60"}}));
    assert!(rel(volume(&mut s), expect(60.0, 25.0)) < 1e-3, "{}", volume(&mut s));
    // Reorder: the fillet can't go before the box; the hole can go before the fillet.
    let e = s.execute("timeline.reorder", &json!({"feature": "Round", "position": 0})).unwrap_err().to_string();
    assert!(e.contains("breaks"), "{e}");
    run(&mut s, "timeline.reorder", json!({"feature": "Bore", "position": 1}));
    let names: Vec<&str> = s.doc.features.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["Base", "Bore", "Round"]);
    assert!(rel(volume(&mut s), expect(60.0, 25.0)) < 1e-3);
    // Redefine the fillet in place with a new radius; it keeps its name and position.
    run(&mut s, "timeline.redefine", json!({"feature": "Round", "command": "solid.fillet", "params": {"edges": [[30, 0, 25]], "radius": 4}}));
    let f4 = (16.0 - 16.0 * PI / 4.0) * 60.0;
    assert!(rel(volume(&mut s), 60.0 * 30.0 * 25.0 - f4 - PI * 9.0 * 25.0) < 1e-3);
    assert_eq!(s.doc.features.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["Base", "Bore", "Round"]);
    // Dependents and roll back.
    let d = run(&mut s, "timeline.dependents", json!({"feature": "Base"}));
    assert_eq!(d["would_fail"].as_array().map(|a| a.len()), Some(2), "{d}");
    run(&mut s, "timeline.roll_to", json!({"feature": "Base"}));
    assert!(rel(volume(&mut s), 60.0 * 30.0 * 25.0) < 1e-6);
    run(&mut s, "timeline.roll_to", json!({}));
    run(&mut s, "edit.undo", json!({}));
    run(&mut s, "edit.undo", json!({}));
    assert!(rel(volume(&mut s), 60.0 * 30.0 * 25.0 - f4 - PI * 9.0 * 25.0) < 1e-3);
}

/// A 40 × 30 × 20 box exported to a STEP file in a fresh temp directory.
fn step_box_file(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("solidcraft-step-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 40, "width": 30, "height": 20}));
    let p = dir.join("Block.step");
    run(&mut s, "file.export", json!({"path": p.to_string_lossy()}));
    p
}

/// File → Open of a STEP file gives an Import base feature, and features work on its faces.
#[test]
fn step_open_then_model_on_it() {
    let p = step_box_file("open");
    let mut s = Session::default();
    let r = run(&mut s, "doc.open", json!({"path": p.to_string_lossy()}));
    assert_eq!(r["features"], 1, "{r}");
    assert_eq!(s.doc.name, "Block");
    assert_eq!(s.doc.features[0].kind.type_name(), "BaseFeature");
    assert!(matches!(&s.doc.features[0].kind, doc::FeatureKind::Import { file, .. } if file == "Block.step"));
    assert!(s.is_dirty() && s.path.is_none() && s.undo.is_empty());
    assert!(rel(volume(&mut s), 24000.0) < 1e-6);
    let corner = run(&mut s, "inspect.measure", json!({}))["bodies"][0]["bbox"]["min"].clone();
    let (x0, y0, z0) = (corner[0].as_f64().unwrap(), corner[1].as_f64().unwrap(), corner[2].as_f64().unwrap());
    let at = |x: f64, y: f64, z: f64| json!([x0 + x, y0 + y, z0 + z]);

    // Fillet a vertical edge, drill a through hole, cut a pocket sketched on the top face.
    run(&mut s, "solid.fillet", json!({"edges": [at(0.0, 0.0, 10.0)], "radius": 3}));
    let mut want = 24000.0 - (9.0 - PI * 9.0 / 4.0) * 20.0;
    assert!(rel(volume(&mut s), want) < 2e-4);
    run(&mut s, "solid.hole", json!({"position": at(28.0, 15.0, 20.0), "diameter": 6}));
    want -= PI * 9.0 * 20.0;
    assert!(rel(volume(&mut s), want) < 5e-4, "{} vs {want}", volume(&mut s));
    // Modified imported faces no longer have an immutable construction certificate; the top
    // attaches by its exact history name instead.
    let top = run(&mut s, "sketch.create", json!({"plane": {"face": at(10.0, 15.0, 20.0)}}))["sketch"].as_u64().unwrap();
    assert!(matches!(
        s.placed_document().sketch_face_attachment(top, &s.model),
        doc::FaceAttachmentStatus::Healthy {
            target: doc::config::ConfigFaceTarget { certificate: Some(doc::config::FaceCertificate::Lineage), .. },
            ..
        }
    ));
    let c = [x0 + 10.0, y0 + 15.0];
    // Sketch coordinates on the face are relative to the model origin projected onto it.
    run(&mut s, "sketch.rectangle.two_point", json!({"p0": [c[0] - 4.0, c[1] - 4.0], "p1": [c[0] + 4.0, c[1] + 4.0]}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.extrude", json!({"distance": -5, "operation": "cut"}));
    want -= 64.0 * 5.0;
    assert!(rel(volume(&mut s), want) < 5e-4, "{} vs {want}", volume(&mut s));
    assert_eq!(s.model.results.iter().filter(|r| r.error.is_some()).count(), 0);

    // The design saves and reopens without the STEP file.
    let before = volume(&mut s);
    let design = p.with_extension("solidcraft");
    run(&mut s, "file.save_as", json!({"path": design.to_string_lossy()}));
    std::fs::remove_file(&p).unwrap();
    let mut s2 = Session::default();
    run(&mut s2, "doc.open", json!({"path": design.to_string_lossy()}));
    assert!(rel(volume(&mut s2), before) < 1e-6);
}

/// A join onto an imported body of thousands of planar faces (a converted mesh, say): each face of
/// the result was compared with every planar face of the inputs whose source is unknown, and the
/// join was refused past the comparison budget.
#[test]
fn join_onto_a_finely_faceted_import() {
    let dir = std::env::temp_dir().join(format!("solidcraft-step-faceted-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let n = 4000;
    let mut pts: Vec<[f64; 2]> =
        (0..n).map(|i| std::f64::consts::TAU * f64::from(i) / f64::from(n)).map(|a| [20.0 * a.cos(), 20.0 * a.sin()]).collect();
    pts.push(pts[0]);
    let mut s = Session::default();
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "P"}));
    run(&mut s, "sketch.line", json!({"points": pts}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.extrude", json!({"sketch": "P", "distance": 10}));
    let p = dir.join("Faceted.step");
    run(&mut s, "file.export", json!({"path": p.to_string_lossy()}));
    let prism = volume(&mut s);

    let mut d = Session::default();
    // The import is a component (#1386): the pin is made in it to join its body.
    let r = run(&mut d, "file.insert_step", json!({"path": p.to_string_lossy()}));
    run(&mut d, "component.activate", json!({"component": r["component"]}));
    run(&mut d, "sketch.create", json!({"plane": "XY", "name": "Pin"}));
    run(&mut d, "sketch.rectangle.two_point", json!({"p0": [-5, -5], "p1": [5, 5]}));
    run(&mut d, "sketch.finish", json!({}));
    run(&mut d, "solid.extrude", json!({"sketch": "Pin", "distance": 25, "operation": "join"}));
    assert_eq!(d.model.state().bodies.len(), 1);
    assert!(rel(volume(&mut d), prism + 10.0 * 10.0 * 15.0) < 1e-6, "{}", volume(&mut d));
}

/// Shell an imported body; insert STEP into an existing design; extension case; bad files.
#[test]
fn step_insert_shell_and_errors() {
    let p = step_box_file("insert");
    let upper = p.with_file_name("BLOCK.STP");
    std::fs::copy(&p, &upper).unwrap();
    let mut s = Session::default();
    run(&mut s, "doc.open", json!({"path": upper.to_string_lossy()}));
    let top = run(&mut s, "inspect.measure", json!({}))["bodies"][0]["bbox"]["max"].clone();
    let (x1, y1, z1) = (top[0].as_f64().unwrap(), top[1].as_f64().unwrap(), top[2].as_f64().unwrap());
    run(&mut s, "solid.shell", json!({"faces": [[x1 - 20.0, y1 - 15.0, z1]], "thickness": 2}));
    let walls = 24000.0 - 36.0 * 26.0 * 18.0;
    assert!(rel(volume(&mut s), walls) < 5e-4, "{}", volume(&mut s));

    let mut d = Session::default();
    run(&mut d, "solid.box", json!({"length": 5, "width": 5, "height": 5, "corner": [100, 0, 0]}));
    let r = run(&mut d, "file.insert_step", json!({"path": p.to_string_lossy()}));
    assert_eq!(r["bodies"].as_array().unwrap().len(), 1, "{r}");
    run(&mut d, "file.insert_step", json!({"path": p.to_string_lossy()}));
    let names: Vec<String> = d.model.state().bodies.iter().map(|b| b.name.clone()).collect();
    assert_eq!(names.len(), 3);
    let mut uniq = names.clone();
    uniq.dedup();
    assert_eq!(uniq.len(), 3, "{names:?}");
    assert!(rel(volume(&mut d), 125.0 + 48000.0) < 1e-6);
    run(&mut d, "edit.undo", json!({}));
    assert_eq!(d.model.state().bodies.len(), 2);

    let junk = p.with_file_name("junk.step");
    std::fs::write(&junk, "ISO-10303-21;\nDATA;\n#1=CARTESIAN_POINT('',(0.,0.,0.));\nENDSEC;\nEND-ISO-10303-21;\n").unwrap();
    let before = d.doc.clone();
    assert!(d.execute("file.insert_step", &json!({"path": junk.to_string_lossy()})).is_err());
    assert!(d.execute("doc.open", &json!({"path": junk.to_string_lossy()})).is_err());
    assert!(d.execute("file.insert_step", &json!({"path": "/nonexistent/a.step"})).is_err());
    assert!(d.execute("file.insert_step", &json!({"path": p.with_extension("stl").to_string_lossy()})).is_err());
    assert_eq!(*d.doc, *before);
}

fn body_volume(st: &solidcraft_doc::ModelState) -> f64 {
    st.bodies.iter().map(|b| solidcraft_kernel::measure(&b.body).unwrap().volume).sum()
}

/// Previews evaluate on a scratch copy: the document, undo history and model are untouched.
#[test]
fn preview_has_no_side_effects() {
    let mut s = Session::default();
    run(&mut s, "sketch.create", json!({"plane": "XY"}));
    run(&mut s, "sketch.rectangle.two_point", json!({"p0": [0, 0], "p1": [40, 30]}));
    run(&mut s, "sketch.finish", json!({}));
    let doc = s.doc.clone();
    let (undo, rev) = (s.undo.len(), s.revision);
    let p = s.preview(&[("solid.extrude".into(), json!({"distance": 15}))]).unwrap();
    assert!(p.before.bodies.is_empty());
    assert!(rel(body_volume(&p.after), 40.0 * 30.0 * 15.0) < 1e-9);
    assert!(Arc::ptr_eq(&doc, &s.doc));
    assert_eq!((s.undo.len(), s.revision), (undo, rev));
    assert!(s.model.state().bodies.is_empty());
    // Errors come back as errors and change nothing either.
    assert!(s.preview(&[("solid.extrude".into(), json!({"distance": "nonsense"}))]).is_err());
    assert!(Arc::ptr_eq(&doc, &s.doc));

    // Fillet preview on a real body.
    run(&mut s, "solid.extrude", json!({"distance": 20}));
    let (doc, undo) = (s.doc.clone(), s.undo.len());
    let p = s.preview(&[("solid.fillet".into(), json!({"edges": [[0, 0, 10]], "radius": 3}))]).unwrap();
    let filleted = 24000.0 - (9.0 - PI * 9.0 / 4.0) * 20.0;
    assert!(rel(body_volume(&p.after), filleted) < 2e-4);
    assert!(rel(body_volume(&p.before), 24000.0) < 1e-9);
    assert!(Arc::ptr_eq(&doc, &s.doc));
    assert_eq!(s.undo.len(), undo);
}

/// Previewing an edit shows the rebuilt feature although the timeline is rolled back to it.
#[test]
fn preview_of_an_edit() {
    let mut s = Session::default();
    run(&mut s, "sketch.create", json!({"plane": "XY"}));
    run(&mut s, "sketch.rectangle.two_point", json!({"p0": [0, 0], "p1": [40, 30]}));
    run(&mut s, "sketch.finish", json!({}));
    let r = run(&mut s, "solid.extrude", json!({"distance": 20}));
    let fid = r["feature"].as_u64().unwrap();
    let idx = s.doc.feature_index(fid).unwrap();
    run(&mut s, "timeline.roll_to", json!({"position": idx}));
    assert!(s.model.state().bodies.is_empty());
    let doc = s.doc.clone();
    let p = s.preview(&[("timeline.redefine".into(), json!({"feature": fid, "command": "solid.extrude", "params": {"distance": 10}}))]).unwrap();
    assert!(rel(body_volume(&p.after), 12000.0) < 1e-9);
    assert!(Arc::ptr_eq(&doc, &s.doc));
}

/// `operation: "auto"`: into a body cuts; additions default to Join when a solid exists.
#[test]
fn extrude_auto_operation() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 40, "width": 30, "height": 20}));
    // A face pushed in (negative distance) cuts.
    let p = json!({"face": [20, 15, 20], "distance": -5, "operation": "auto"});
    assert_eq!(auto_operation(&s, &p), Some("cut"));
    run(&mut s, "solid.extrude", p);
    assert!(rel(volume(&mut s), 40.0 * 30.0 * 15.0) < 1e-6);
    // Pulled out, it joins.
    assert_eq!(auto_operation(&s, &json!({"face": [20, 15, 15], "distance": 5})), Some("join"));
    // Pushed in far past the bottom (halfway is below the box), it still cuts (#1257).
    assert_eq!(auto_operation(&s, &json!({"face": [20, 15, 15], "distance": -50})), Some("cut"));
    assert_eq!(auto_operation(&s, &json!({"face": [20, 15, 15], "distance": 50, "direction": "negative"})), Some("cut"));
    // A sketch on XY under the box: up goes into it (cut), down grows out of it (join), and a
    // profile beside the box still defaults to Join (New Body remains explicit).
    run(&mut s, "sketch.create", json!({"plane": "XY"}));
    run(&mut s, "sketch.rectangle.two_point", json!({"p0": [5, 5], "p1": [15, 15]}));
    run(&mut s, "sketch.rectangle.two_point", json!({"p0": [60, 5], "p1": [70, 15]}));
    run(&mut s, "sketch.finish", json!({}));
    let at = |x: f64, d: f64| json!({"profiles": [{"point": [x, 10]}], "distance": d});
    assert_eq!(auto_operation(&s, &at(10.0, 8.0)), Some("cut"));
    assert_eq!(auto_operation(&s, &at(10.0, -8.0)), Some("join"));
    assert_eq!(auto_operation(&s, &at(65.0, 8.0)), Some("join"));
    run(&mut s, "solid.extrude", json!({"profiles": [{"point": [10, 10]}], "distance": 8, "operation": "auto"}));
    assert!(rel(volume(&mut s), 40.0 * 30.0 * 15.0 - 100.0 * 8.0) < 1e-6);
}

/// #1666: a loft between two 18-point polylines has twisted (bilinear) side faces. Each was
/// meshed as two flat triangles, which cross the surface; the mesh volume was 16 800.6 mm³
/// against the B-rep's 17 884. Divided to the chord tolerance, the mesh matches the B-rep.
#[test]
fn twisted_ruled_faces_are_meshed_to_the_chord_tolerance() {
    let mut s = Session::default();
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Bottom"}));
    run(
        &mut s,
        "sketch.line",
        json!({"closed": true, "infer": false, "connect": false, "points": [[-26,3],[-24.76384,3],[-16.0568,14.97217],[-8,4.90117],[0,14.90117],[8,4.90117],[16.0568,14.97217],[24.76384,3],[26,3],[26,0],[23.23616,0],[15.9432,10.02783],[8,0.09883],[0,10.09883],[-8,0.09883],[-15.9432,10.02783],[-23.23616,0],[-26,0]]}),
    );
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "sketch.create", json!({"plane": "XY", "offset": 90.5, "name": "Top"}));
    run(
        &mut s,
        "sketch.line",
        json!({"closed": true, "infer": false, "connect": false, "points": [[-26,3],[-24,3],[-16,3],[-8,3],[0,3],[8,3],[16,3],[24,3],[26,3],[26,0],[24,0],[16,0],[8,0],[0,0],[-8,0],[-16,0],[-24,0],[-26,0]]}),
    );
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.loft", json!({"sections": [{"sketch": "Bottom"}, {"sketch": "Top"}], "operation": "new"}));
    let st = s.model.state();
    let body = &st.bodies.first().unwrap().body;
    let exact = solidcraft_kernel::measure(body).unwrap().volume;
    let mesh = body.tessellate(0.05).unwrap();
    let volume: f64 = mesh
        .triangles
        .iter()
        .map(|t| {
            let p = |i: u32| mesh.positions[i as usize];
            p(t[0]).dot(p(t[1]).cross(p(t[2]))) / 6.0
        })
        .sum();
    assert!(mesh.triangles.len() > 72, "the twisted faces are divided: {} triangles", mesh.triangles.len());
    assert!(rel(volume.abs(), exact) < 2e-3, "mesh {volume} against B-rep {exact}");
}

/// #1633: to an object, the extrude goes to it on whichever side it is, whatever its
/// `direction`: through the body to its far face is a Cut, out to something above is a Join.
#[test]
fn extrude_auto_operation_follows_the_to_object() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 40, "width": 30, "height": 20}));
    run(&mut s, "solid.box", json!({"length": 40, "width": 30, "height": 5, "corner": [0, 0, 50], "body_name": "Above"}));
    for direction in ["positive", "negative"] {
        // From the top face to the bottom face: through the body.
        let down = json!({"face": [20, 15, 20], "to": [10, 10, 0], "distance": 19, "direction": direction});
        assert!(extrude_reach(&s, &down).is_some_and(|h| (h + 20.0).abs() < 1e-6), "{down}");
        assert_eq!(auto_operation(&s, &down), Some("cut"), "{down}");
        // From the top face up to the box above: out of the body.
        let up = json!({"face": [20, 15, 20], "to": [10, 10, 50], "distance": 19, "direction": direction});
        assert!(extrude_reach(&s, &up).is_some_and(|h| (h - 30.0).abs() < 1e-6), "{up}");
        assert_eq!(auto_operation(&s, &up), Some("join"), "{up}");
    }
    // Without a To object the direction still counts.
    assert_eq!(extrude_reach(&s, &json!({"face": [20, 15, 20], "distance": 5, "direction": "negative"})), Some(-5.0));
}

/// #1678: a hole's `position` takes length expressions, keeps them and follows the parameters: a
/// hole on a plate's middle line stays there when the plate gets wider. A position of plain
/// numbers is stored as before, with no `at`.
#[test]
fn a_hole_position_of_expressions_follows_the_parameters() {
    let mut s = Session::default();
    run(&mut s, "parameters.add", json!({"name": "width", "expression": "30 mm"}));
    run(&mut s, "parameters.add", json!({"name": "t", "expression": "4 mm"}));
    run(&mut s, "solid.box", json!({"length": 100, "width": "width", "height": "t", "body_name": "Plate"}));
    run(&mut s, "solid.hole", json!({"position": ["40", "width/2", "t"], "clearance": "M5", "name": "Middle"}));
    run(&mut s, "solid.hole", json!({"position": [80, 10, 4], "clearance": "M5", "name": "Fixed"}));
    let centre_y = |s: &Session| {
        let st = s.model.state();
        solidcraft_kernel::measure(&st.body("Plate").unwrap().body).unwrap().centroid.y
    };
    let hole = |s: &Session, name: &str| -> Value {
        let saved: Value = serde_json::from_str(&s.doc.to_json()).unwrap();
        saved["features"].as_array().and_then(|f| f.iter().find(|f| f["name"] == name)).cloned().unwrap()
    };
    assert_eq!(hole(&s, "Middle")["at"], json!([40.0, "width/2", "t"]), "the expressions are kept");
    assert!(hole(&s, "Fixed").get("at").is_none(), "plain numbers are stored as before");
    // The fixed hole sits off the middle line, so it alone moves the centre; widen the plate.
    let before = centre_y(&s);
    run(&mut s, "parameters.change", json!({"name": "width", "expression": "40 mm"}));
    let after = centre_y(&s);
    // The middle hole is at y = 20 now (the plate's middle); the fixed one stays at y = 10. With
    // the middle hole left at y = 15 the centre would sit lower still.
    let plate = 100.0 * 40.0 * 4.0;
    let hole_v = std::f64::consts::PI * (5.5f64 / 2.0).powi(2) * 4.0;
    let want = (plate * 20.0 - hole_v * 20.0 - hole_v * 10.0) / (plate - 2.0 * hole_v);
    assert!((after - want).abs() < 1e-3, "centre y {after}, want {want} (was {before})");
    // A bad expression is refused.
    assert!(s.execute("solid.hole", &json!({"position": ["40", "nope/2", "t"], "diameter": 3})).is_err());
}

/// Additive extrusions default to joining even when only a profile side touches material.
#[test]
fn auto_extrude_joins_side_contact_elbow_and_keeps_new_body_explicit() {
    for operation in ["auto", "new"] {
        let mut s = Session::default();
        run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 30}));
        run(&mut s, "sketch.create", json!({"plane": {"origin": [10, 0, 0], "x_dir": [0, 1, 0], "y_dir": [0, 0, 1]}}));
        run(&mut s, "sketch.rectangle.two_point", json!({"p0": [10, 20], "p1": [30, 30]}));
        run(&mut s, "sketch.finish", json!({}));
        // The profile centre is outside the vertical arm. Contact is along its
        // y=10 side, not behind the centre along the extrusion's X direction.
        let p = json!({"distance": 10, "to": [0, 15, 25], "operation": operation});
        assert_eq!(auto_operation(&s, &p), Some("join"));
        run(&mut s, "solid.extrude", p);
        assert!((volume(&mut s) - 5000.0).abs() < 1e-6);
        let st = s.model.state();
        assert_eq!(st.bodies.len(), if operation == "auto" { 1 } else { 2 });
        if operation == "auto" {
            let body = &st.bodies.first().unwrap().body;
            assert!(body.validity().is_empty());
            assert_eq!(body.lumps().unwrap().len(), 1);
            assert_eq!(body.face_count(), 8, "the two flat elbow sides must be single faces");
            assert!(body.contains(geom::Vec3::new(5.0, 10.0, 25.0)));
        }
    }
}

#[test]
fn auto_extrude_joins_corner_screw_boss() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 30}));
    run(&mut s, "sketch.create", json!({"plane": {"origin": [0, 0, 30], "x_dir": [1, 0, 0], "y_dir": [0, 1, 0]}}));
    run(&mut s, "sketch.circle.center", json!({"center": [10, 10], "radius": 4}));
    run(&mut s, "sketch.circle.center", json!({"center": [10, 10], "radius": 1}));
    run(&mut s, "sketch.finish", json!({}));
    let p = json!({"profiles": [{"point": [12, 10]}], "distance": 5, "operation": "auto"});
    assert_eq!(auto_operation(&s, &p), Some("join"));
    run(&mut s, "solid.extrude", p);
    assert!(rel(volume(&mut s), 3000.0 + PI * 15.0 * 5.0) < 1e-3);
    let st = s.model.state();
    assert_eq!(st.bodies.len(), 1);
    let body = &st.bodies.first().unwrap().body;
    assert_eq!(body.lumps().unwrap().len(), 1);
    assert!(body.validity().is_empty());
    assert!(!body.contains(geom::Vec3::new(10.0, 10.0, 32.0)), "the screw hole stays open");
}

#[test]
fn auto_extrude_starts_new_body_in_empty_component() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10}));
    run(&mut s, "component.create", json!({"name": "Other"}));
    run(&mut s, "sketch.create", json!({"plane": "XY"}));
    run(&mut s, "sketch.circle.center", json!({"center": [0, 0], "radius": 2}));
    run(&mut s, "sketch.finish", json!({}));
    assert_eq!(auto_operation(&s, &json!({"distance": 5})), Some("new"));
    assert_eq!(auto_operation(&s, &json!({"distance": 5, "participants": ["missing"]})), Some("new"));
}

/// Measuring picked geometry: distance between faces, angle, edge length.
#[test]
fn measure_items() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 40, "width": 30, "height": 20}));
    let st = s.model.state();
    let m = st.body("Body1").unwrap().mesh();
    // Find the faces by their centroid: top (z = 20), bottom (z = 0), front (y = 0).
    let face_at = |z: Option<f64>, y: Option<f64>| -> usize {
        (0..6)
            .find(|f| {
                let tris: Vec<_> = m.triangles.iter().zip(&m.tri_face).filter(|(_, tf)| **tf as usize == *f).filter_map(|(t, _)| m.tri(t)).collect();
                tris.iter().all(|t| t.iter().all(|p| z.is_none_or(|z| (p.z - z).abs() < 1e-9) && y.is_none_or(|y| (p.y - y).abs() < 1e-9)))
            })
            .unwrap()
    };
    let (ft, fb, ff) = (face_at(Some(20.0), None), face_at(Some(0.0), None), face_at(None, Some(0.0)));
    let face = |i: usize, p: [f64; 3]| json!({"type": "face", "body": "Body1", "index": i, "point": p});
    let r = run(&mut s, "inspect.measure", json!({"items": [face(ft, [20.0, 15.0, 20.0]), face(fb, [20.0, 15.0, 0.0])]}));
    assert!((r["distance_mm"].as_f64().unwrap() - 20.0).abs() < 1e-9, "{r}");
    assert!((r["angle_deg"].as_f64().unwrap() - 180.0).abs() < 1e-6, "{r}");
    let r = run(&mut s, "inspect.measure", json!({"items": [face(ft, [20.0, 15.0, 20.0]), face(ff, [20.0, 0.0, 10.0])]}));
    assert!(r["distance_mm"].as_f64().unwrap() < 1e-9);
    assert!((r["angle_deg"].as_f64().unwrap() - 90.0).abs() < 1e-6);
    let r = run(&mut s, "inspect.measure", json!({"items": [face(ft, [20.0, 15.0, 20.0])]}));
    assert!((r["items"][0]["area_mm2"].as_f64().unwrap() - 1200.0).abs() < 1e-6, "{r}");
    let v = json!({"type": "vertex", "body": "Body1", "point": [0, 0, 0]});
    let w = json!({"type": "vertex", "body": "Body1", "point": [40, 30, 20]});
    let r = run(&mut s, "inspect.measure", json!({"items": [v, w]}));
    assert!((r["distance_mm"].as_f64().unwrap() - (1600.0f64 + 900.0 + 400.0).sqrt()).abs() < 1e-9);
    assert!(s.execute("inspect.measure", &json!({"items": []})).is_err());
    assert!(s.execute("inspect.measure", &json!({"items": [{"type": "nonsense"}]})).is_err());
}

/// Section Analysis sets and clears the view cut without touching the design or history.
#[test]
fn section_analysis() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 40, "width": 30, "height": 20}));
    let (doc, undo) = (s.doc.clone(), s.undo.len());
    run(&mut s, "inspect.section", json!({"plane": "XZ", "offset": 15}));
    let (o, n) = s.section.unwrap();
    assert!((o - n * 15.0).len() < 1e-12 && (n.y.abs() - 1.0).abs() < 1e-12);
    run(&mut s, "inspect.section", json!({"plane": {"origin": [0, 0, 5], "normal": [0, 0, 2]}, "flip": true}));
    let (o, n) = s.section.unwrap();
    assert!((o.z - 5.0).abs() < 1e-12 && (n.z + 1.0).abs() < 1e-12);
    assert!(s.execute("inspect.section", &json!({"plane": {"normal": [0, 0, 0]}})).is_err());
    run(&mut s, "inspect.section", json!({"clear": true}));
    assert!(s.section.is_none());
    // XZ's normal is +Y: an offset moves the cut to +Y, `at` places it at a world Y, and the
    // result says where the cut landed either way.
    let r = run(&mut s, "inspect.section", json!({"plane": "XZ", "offset": 10}));
    assert_eq!((r["section"]["axis"].as_str(), r["section"]["at"].as_f64()), (Some("Y"), Some(10.0)), "{r}");
    for (plane, at, flip) in [("XZ", -282.5, false), ("XZ", 7.0, true), ("XY", 3.0, false), ("YZ", -4.0, true)] {
        let r = run(&mut s, "inspect.section", json!({"plane": plane, "at": at, "flip": flip}));
        assert_eq!(r["section"]["at"].as_f64(), Some(at), "{plane} {r}");
        let (o, _) = s.section.unwrap();
        assert!(((o.x + o.y + o.z) - at).abs() < 1e-12, "{plane} {o:?}");
    }
    assert!(s.execute("inspect.section", &json!({"plane": "XZ", "at": 1, "offset": 1})).is_err());
    assert!(s.execute("inspect.section", &json!({"plane": {"normal": [0, 1, 0]}, "at": 1})).is_err());
    run(&mut s, "inspect.section", json!({"clear": true}));
    assert!(Arc::ptr_eq(&doc, &s.doc));
    assert_eq!(s.undo.len(), undo);
}

/// Press Pull: positive pulls a face out, negative pushes it in (a cut), edges get a fillet.
#[test]
fn press_pull() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 40, "width": 30, "height": 20}));
    run(&mut s, "solid.press_pull", json!({"face": [20, 15, 20], "distance": 5}));
    assert!(rel(volume(&mut s), 40.0 * 30.0 * 25.0) < 1e-6);
    run(&mut s, "solid.press_pull", json!({"face": [20, 15, 25], "distance": "-10"}));
    assert!(rel(volume(&mut s), 40.0 * 30.0 * 15.0) < 1e-6);
    assert!(s.execute("solid.press_pull", &json!({"face": [20, 15, 15], "distance": 0})).is_err());
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 40, "width": 30, "height": 20}));
    run(&mut s, "solid.press_pull", json!({"edges": [[0, 0, 7]], "distance": 3}));
    assert!(rel(volume(&mut s), 24000.0 - (9.0 - PI * 9.0 / 4.0) * 20.0) < 2e-4);
}

/// Extruding a planar body face (press-pull style): the face boundary becomes a sketch profile.
#[test]
fn extrude_a_body_face() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 40, "width": 30, "height": 20}));
    run(&mut s, "solid.extrude", json!({"face": [20, 15, 20], "distance": 15, "operation": "join"}));
    assert!(rel(volume(&mut s), 40.0 * 30.0 * 35.0) < 1e-6);
    assert!(s.active_sketch.is_none());
    // A face with a round hole: the hole stays open (a new body: joins of curved bodies on a
    // shared face are a kernel gap for now).
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 40, "width": 30, "height": 20}));
    run(&mut s, "sketch.create", json!({"plane": "XY"}));
    run(&mut s, "sketch.circle.center", json!({"center": [20, 15], "radius": 5}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.extrude", json!({"distance": 20, "operation": "cut"}));
    let before = volume(&mut s);
    run(&mut s, "solid.extrude", json!({"face": [5, 5, 20], "distance": 10, "operation": "new"}));
    let v = volume(&mut s);
    assert!(rel(v - before, (1200.0 - PI * 25.0) * 10.0) < 1e-3, "{v} {before}");
    assert!(s.execute("solid.extrude", &json!({"face": [500, 5, 20], "distance": 10})).is_err());
}

/// Export 3MF; open and insert it as mesh bodies; mesh bodies move and export but refuse
/// solid features.
#[test]
fn threemf_export_open_insert() {
    let dir = std::env::temp_dir().join(format!("solidcraft-3mf-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 40, "width": 30, "height": 20}));
    run(&mut s, "solid.cylinder", json!({"radius": 5, "height": 10, "base": [100, 0, 0]}));
    let p = dir.join("Parts.3MF");
    run(&mut s, "file.export", json!({"path": p.to_string_lossy()}));
    let want = volume(&mut s);
    // With face tags it is still a 3MF any reader opens, with the same bodies.
    let tagged = dir.join("tagged.3mf");
    let r = run(&mut s, "file.export", json!({"path": tagged.to_string_lossy(), "format": "3mf-tagged"}));
    assert_eq!(r["format"], "ThreeMfTagged", "{r}");
    let mut t = Session::default();
    let r = run(&mut t, "doc.open", json!({"path": tagged.to_string_lossy()}));
    assert_eq!(r["bodies"].as_array().unwrap().len(), 2, "{r}");
    assert!(rel(volume(&mut t), want) < 2e-3, "{} vs {want}", volume(&mut t));

    let mut m = Session::default();
    let r = run(&mut m, "doc.open", json!({"path": p.to_string_lossy()}));
    assert_eq!(r["bodies"].as_array().unwrap().len(), 2, "{r}");
    assert_eq!(m.doc.features[0].kind.type_name(), "MeshFeature");
    assert!(rel(volume(&mut m), want) < 2e-3, "{} vs {want}", volume(&mut m));
    let meas = run(&mut m, "inspect.measure", json!({}));
    assert!(meas["bodies"][0]["face_types"]["mesh"].as_u64().unwrap() >= 12, "{meas}");
    let e = m.execute("solid.fillet", &json!({"edges": [[0, 0, 10]], "radius": 2})).unwrap_err().to_string();
    assert!(e.contains("mesh"), "{e}");
    assert!(m.execute("file.export", &json!({"path": dir.join("x.step").to_string_lossy()})).is_err());
    let stl = dir.join("again.stl");
    run(&mut m, "file.export", json!({"path": stl.to_string_lossy()}));

    // Insert the STL into the solid design; names stay unique.
    run(&mut s, "file.insert_mesh", json!({"path": stl.to_string_lossy()}));
    assert_eq!(s.model.state().bodies.len(), 3);
    assert!(rel(volume(&mut s), 2.0 * want) < 2e-3);
    assert!(s.execute("file.insert_mesh", &json!({"path": dir.join("x.step").to_string_lossy()})).is_err());
    let junk = dir.join("junk.3mf");
    std::fs::write(&junk, b"PK\x03\x04 not really").unwrap();
    let before = s.doc.clone();
    assert!(s.execute("file.insert_mesh", &json!({"path": junk.to_string_lossy()})).is_err());
    assert!(s.execute("doc.open", &json!({"path": junk.to_string_lossy()})).is_err());
    assert_eq!(*s.doc, *before);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn holes_at_sketch_points_threads_and_components() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 60, "width": 40, "height": 10, "name": "Plate"}));
    run(&mut s, "sketch.create", json!({"plane": {"face": [30, 20, 10]}, "name": "HolePoints"}));
    let mut ids = Vec::new();
    for (k, (x, y)) in [(10.0, 10.0), (50.0, 10.0), (50.0, 30.0), (10.0, 30.0)].into_iter().enumerate() {
        let id = format!("h{k}");
        run(&mut s, "sketch.point", json!({ "point": [x, y], "id": id }));
        ids.push(id);
    }
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.hole", json!({"sketch": "HolePoints", "points": ids, "diameter": 5, "thread": "M6", "name": "Holes"}));
    let v = volume(&mut s);
    assert!(rel(v, 60.0 * 40.0 * 10.0 - 4.0 * PI * 6.25 * 10.0) < 1e-3, "{v}");
    let t = run(&mut s, "model.threads", json!({}));
    let threads = t["threads"].as_array().unwrap();
    assert_eq!(threads.len(), 4, "{t}");
    assert_eq!(threads[0]["designation"], "M6");
    assert_eq!(threads[0]["internal"], true);
    // A cosmetic thread on a shaft; designation from its size.
    run(&mut s, "solid.cylinder", json!({"base": [100, 0, 0], "radius": 4, "height": 20, "name": "Shaft", "body_name": "Shaft"}));
    let th = run(&mut s, "solid.thread", json!({"face": [104, 0, 10], "length": 12}));
    assert_eq!(th["thread"]["designation"], "M8", "{th}");
    assert!((th["thread"]["end"].as_f64().unwrap() - th["thread"]["start"].as_f64().unwrap() - 12.0).abs() < 1e-6, "{th}");
    assert!(s.execute("solid.thread", &json!({"face": [104, 0, 10], "designation": "M20"})).is_err());
    // Components: features made while one is active belong to it.
    let c = run(&mut s, "component.create", json!({"name": "Bracket"}));
    let cid = c["component"].as_u64().unwrap();
    run(&mut s, "solid.box", json!({"corner": [0, 60, 0], "length": 10, "width": 10, "height": 10, "name": "Block"}));
    run(&mut s, "component.activate", json!({"component": "root"}));
    let l = run(&mut s, "component.list", json!({}));
    let comps = l["components"].as_array().unwrap();
    let bracket = comps.iter().find(|x| x["id"] == cid).unwrap();
    assert_eq!(bracket["features"], json!(["Block"]), "{l}");
    assert_eq!(bracket["bodies"].as_array().map(|a| a.len()), Some(1), "{l}");
    let r = run(&mut s, "component.from_bodies", json!({"bodies": ["Shaft"]}));
    assert_eq!(r["components"].as_array().map(|a| a.len()), Some(1));
    // Saved and loaded with the components.
    let text = s.doc.to_json();
    let back = solidcraft_doc::Document::from_json(&text).unwrap();
    assert_eq!(back.components.len(), 2);
    let assigned = r["components"][0]["component"].as_u64().unwrap();
    let transfer = r["components"][0]["feature"].as_u64().unwrap();
    assert!(
        matches!(&back.feature(transfer).unwrap().kind, solidcraft_doc::FeatureKind::BodyTransfer { bodies, to, .. } if bodies == &["Shaft"] && *to == assigned)
    );
    let reopened = Session::new(back);
    assert_eq!(reopened.model.state().body("Shaft").unwrap().component, assigned);
    assert_eq!(reopened.world_state().threads.len(), s.world_state().threads.len());
}

#[test]
fn pipe_scale_offset_bounding_and_materials() {
    let mut s = Session::default();
    // Pipe along an L path (line, arc, line) on XZ.
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Path"}));
    run(&mut s, "sketch.line", json!({"points": [[0, 0], [30, 0]], "ids": ["l1"]}));
    run(&mut s, "sketch.arc.center_point", json!({"center": [30, 10], "start": "l1.end", "end": [40, 10], "id": "a1"}));
    run(&mut s, "sketch.line", json!({"points": ["a1.end", [40, 40]], "ids": ["l2"]}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.pipe", json!({"path_sketch": "Path", "path": ["l1", "a1", "l2"], "diameter": 4, "wall": 1, "body_name": "Tube"}));
    let len = 30.0 + PI * 10.0 / 2.0 + 30.0;
    let v = volume(&mut s);
    assert!(rel(v, PI * (4.0 - 1.0) * len) < 2e-3, "{v} vs {}", PI * 3.0 * len);
    // Bounding solid around it; then scale a box and push one of its faces.
    run(&mut s, "solid.bounding_solid", json!({"bodies": ["Tube"], "margin": 1, "body_name": "Stock"}));
    let m = run(&mut s, "inspect.measure", json!({"bodies": ["Stock"]}));
    let bb = &m["bodies"][0]["bbox"];
    assert!((bb["min"][1].as_f64().unwrap() + 3.0).abs() < 1e-6 && (bb["max"][1].as_f64().unwrap() - 41.0).abs() < 1e-6, "{bb}");
    run(&mut s, "solid.box", json!({"corner": [100, 0, 0], "length": 10, "width": 10, "height": 10, "body_name": "Cube"}));
    run(&mut s, "solid.scale", json!({"bodies": ["Cube"], "factor": 2, "origin": [100, 0, 0]}));
    let vc = run(&mut s, "inspect.measure", json!({"bodies": ["Cube"]}))["total"]["volume_mm3"].as_f64().unwrap();
    assert!(rel(vc, 8000.0) < 1e-9, "{vc}");
    run(&mut s, "solid.offset_face", json!({"faces": [[110, 10, 20]], "distance": 5}));
    let vc = run(&mut s, "inspect.measure", json!({"bodies": ["Cube"]}))["total"]["volume_mm3"].as_f64().unwrap();
    assert!(rel(vc, 20.0 * 20.0 * 25.0) < 1e-9, "{vc}");
    // Materials and mass.
    run(&mut s, "material.assign", json!({"bodies": ["Cube"], "material": "aluminum"}));
    let m = run(&mut s, "inspect.measure", json!({"bodies": ["Cube"]}));
    assert!((m["bodies"][0]["mass_g"].as_f64().unwrap() - 10_000.0 * 2.7 / 1000.0).abs() < 1e-6, "{m}");
    assert_eq!(m["bodies"][0]["material"], "Aluminum");
    assert!(s.execute("material.assign", &json!({"bodies": ["Cube"], "material": "unobtainium"})).is_err());
}

/// Regression: deleting a pipe's path sketch left the pipe in the timeline ("unknown sketch").
#[test]
fn deleting_a_path_sketch_deletes_its_pipe() {
    let mut s = Session::default();
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Path"}));
    run(&mut s, "sketch.line", json!({"points": [[0, 0], [30, 0]], "ids": ["l1"]}));
    run(&mut s, "sketch.finish", json!({}));
    let pipe = run(&mut s, "solid.pipe", json!({"path_sketch": "Path", "path": ["l1"], "diameter": 4}));
    let deps = run(&mut s, "timeline.dependents", json!({"feature": "Path"}));
    assert_eq!(deps["deleted_with_it"].as_array().map(Vec::len), Some(1), "{deps} (pipe {pipe})");
    let r = run(&mut s, "timeline.delete", json!({"features": ["Path"]}));
    assert_eq!(r["deleted"].as_array().map(Vec::len), Some(2), "{r}");
    assert!(s.doc.features.is_empty(), "{:?}", s.doc.features.iter().map(|f| &f.name).collect::<Vec<_>>());
    assert!(s.model.results.iter().all(|r| r.error.is_none()));
}

/// Regression: batches were rolled back by undoing as many steps as the undo history grew,
/// which is none once the history is full, leaving their sketches behind.
#[test]
fn checkpoint_restores_with_a_full_undo_history() {
    let mut s = Session::default();
    for i in 0..MAX_UNDO + 5 {
        run(&mut s, "parameters.add", json!({"name": format!("p{i}"), "expression": "1 mm"}));
    }
    assert_eq!(s.undo.len(), MAX_UNDO);
    let labels: Vec<String> = s.undo.iter().map(|u| u.label.clone()).collect();
    let doc = s.doc.clone();
    let id = run(&mut s, "edit.checkpoint", json!({}))["checkpoint"].as_u64().unwrap();
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "TapHole"}));
    run(&mut s, "sketch.circle.center", json!({"center": [0, 0], "radius": 2}));
    assert!(s.active_sketch.is_some());
    run(&mut s, "edit.restore_checkpoint", json!({"checkpoint": id}));
    assert!(Arc::ptr_eq(&s.doc, &doc) && s.active_sketch.is_none());
    assert!(s.doc.find_feature("TapHole").is_none());
    assert_eq!(s.undo.iter().map(|u| u.label.clone()).collect::<Vec<_>>(), labels);
    // Used up: a second restore is an error, not a jump somewhere else.
    assert!(s.execute("edit.restore_checkpoint", &json!({"checkpoint": id})).is_err());
    // Forgetting one leaves the design alone.
    let id = run(&mut s, "edit.checkpoint", json!({}))["checkpoint"].as_u64().unwrap();
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Kept"}));
    run(&mut s, "edit.restore_checkpoint", json!({"checkpoint": id, "forget": true}));
    assert!(s.doc.find_feature("Kept").is_some());
    assert!(s.execute("edit.restore_checkpoint", &json!({"checkpoint": id})).is_err());
}

#[test]
fn components_occurrences_and_world_placement() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10, "body_name": "Base"}));
    // A component, active: its box is authored in its frame.
    let c = run(&mut s, "component.create", json!({"name": "Arm"}));
    let (cid, occ) = (c["component"].as_u64().unwrap(), c["occurrence"].as_u64().unwrap());
    run(&mut s, "solid.box", json!({"corner": [20, 0, 0], "length": 10, "width": 4, "height": 4, "body_name": "ArmBody"}));
    run(&mut s, "component.activate", json!({"component": 0}));
    let bbox = |s: &mut Session, b: &str| run(s, "inspect.measure", json!({ "bodies": [b] }))["bodies"][0]["bbox"].clone();
    assert_eq!(bbox(&mut s, "ArmBody")["min"][0], 20.0);
    // Move the occurrence: the world placement follows, the authoring model doesn't change.
    run(&mut s, "occurrence.move", json!({"occurrence": occ, "translate": [0, 50, 0]}));
    assert!((bbox(&mut s, "ArmBody")["min"][1].as_f64().unwrap() - 50.0).abs() < 1e-9);
    assert_eq!(s.model.state().body("ArmBody").unwrap().mesh().bounds().min.y, 0.0);
    // A fillet picked in the world, while the component is active, lands on its edge.
    run(&mut s, "component.activate", json!({"component": "Arm"}));
    run(&mut s, "solid.fillet", json!({"edges": [[25, 54, 4]], "radius": 1}));
    let v = run(&mut s, "inspect.measure", json!({"bodies": ["ArmBody"]}))["total"]["volume_mm3"].as_f64().unwrap();
    assert!(rel(v, 160.0 - (1.0 - PI / 4.0) * 10.0) < 1e-4, "{v}");
    run(&mut s, "component.activate", json!({"component": 0}));
    // Instances share the contents; a copy is independent.
    run(&mut s, "occurrence.copy", json!({"component": cid, "translate": [0, 20, 0]}));
    let w = s.world_state();
    assert_eq!(w.bodies.len(), 3, "{:?}", w.bodies.iter().map(|b| &b.name).collect::<Vec<_>>());
    let pn = run(&mut s, "component.paste_new", json!({"component": cid, "translate": [0, -30, 0]}));
    assert_eq!(pn["features"].as_array().map(|a| a.len()), Some(2));
    assert_eq!(s.world_state().bodies.len(), 4);
    // Ground stops moves; pending moves show until captured or reverted.
    run(&mut s, "occurrence.ground", json!({"occurrence": occ}));
    assert!(s.execute("occurrence.move", &json!({"occurrence": occ, "translate": [1, 0, 0]})).is_err());
    run(&mut s, "occurrence.ground", json!({"occurrence": occ, "grounded": false}));
    run(&mut s, "occurrence.move", json!({"occurrence": occ, "translate": [0, 0, 5], "capture": false}));
    assert!((bbox(&mut s, "ArmBody")["min"][2].as_f64().unwrap() - 5.0).abs() < 1e-9);
    run(&mut s, "component.revert_position", json!({}));
    assert!(bbox(&mut s, "ArmBody")["min"][2].as_f64().unwrap().abs() < 1e-9);
    run(&mut s, "occurrence.move", json!({"occurrence": occ, "translate": [0, 0, 5], "capture": false}));
    run(&mut s, "component.capture_position", json!({}));
    assert!(s.pending_moves.is_empty());
    assert!((bbox(&mut s, "ArmBody")["min"][2].as_f64().unwrap() - 5.0).abs() < 1e-9);
    // A definition placed twice cannot receive one world body without duplicating it.
    let document = s.doc.to_json();
    let pending = s.pending_moves.clone();
    let error = s.execute("component.move_bodies", &json!({"bodies": ["Base"], "component": "Arm"})).unwrap_err();
    assert!(error.to_string().contains("one source and destination placement"), "{error}");
    assert_eq!(s.doc.to_json(), document);
    assert_eq!(s.pending_moves, pending);
    // The independent Paste New result has one placement and can receive it.
    let copied = pn["component"].as_u64().unwrap();
    run(&mut s, "component.move_bodies", json!({"bodies": ["Base"], "component": copied}));
    let l = run(&mut s, "component.list", json!({}));
    let arm = l["components"].as_array().unwrap().iter().find(|x| x["id"] == cid).unwrap().clone();
    assert!(!arm["bodies"].as_array().unwrap().iter().any(|b| b == "Base"), "{arm}");
    let receiver = l["components"].as_array().unwrap().iter().find(|x| x["id"] == copied).unwrap();
    assert!(receiver["bodies"].as_array().unwrap().iter().any(|b| b == "Base"), "{receiver}");
    assert_eq!(bbox(&mut s, "Base")["min"], json!([0.0, 0.0, 0.0]));
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "S1"}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "component.move_sketches", json!({"sketches": ["S1"], "component": cid}));
    assert_eq!(s.doc.find_feature("S1").unwrap().component, cid);
    // Undo/redo and the timeline work with features in components.
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(s.doc.find_feature("S1").unwrap().component, 0);
    run(&mut s, "edit.redo", json!({}));
    run(&mut s, "timeline.roll_to", json!({"position": 1}));
    // The ownership boundary is later: before it Base remains the sole root body.
    assert_eq!(s.world_state().bodies.len(), 1);
    assert_eq!(s.world_state().body("Base").unwrap().component, 0);
    run(&mut s, "timeline.roll_to", json!({}));
    // Saved and loaded; older designs get occurrences.
    let back = solidcraft_doc::Document::from_json(&s.doc.to_json()).unwrap();
    assert_eq!(back.occurrences.len(), s.doc.occurrences.len());
    let mut old: serde_json::Value = serde_json::from_str(&s.doc.to_json()).unwrap();
    old.as_object_mut().unwrap().remove("occurrences");
    let migrated = solidcraft_doc::Document::from_json(&old.to_string()).unwrap();
    assert_eq!(migrated.occurrences.len(), migrated.components.len());
}

#[test]
fn feature_inputs_are_parameters() {
    let mut s = Session::default();
    run(&mut s, "parameters.change", json!({"name": "w", "expression": "15 mm", "comment": "width"}));
    run(&mut s, "solid.box", json!({"length": 10, "width": "w", "height": "1 in", "name": "B"}));
    let l = run(&mut s, "parameters.list", json!({}));
    let rows = l["parameters"].as_array().unwrap();
    let feat: Vec<&Value> = rows.iter().filter(|r| r["source"] == "feature").collect();
    assert_eq!(feat.len(), 3, "{l}");
    let height = feat.iter().find(|r| r["input"] == "Height").unwrap();
    assert!((height["value"].as_f64().unwrap() - 25.4).abs() < 1e-9);
    let length_name = feat.iter().find(|r| r["input"] == "Length").unwrap()["name"].as_str().unwrap().to_string();
    // A feature parameter changes the feature; other expressions can use it.
    run(&mut s, "parameters.change", json!({"name": length_name, "expression": "2 * w"}));
    assert!(rel(volume(&mut s), 30.0 * 15.0 * 25.4) < 1e-9);
    run(&mut s, "parameters.change", json!({"name": "t", "expression": format!("{length_name} / 3")}));
    let t = run(&mut s, "parameters.list", json!({}))["parameters"].as_array().unwrap().iter().find(|r| r["name"] == "t").unwrap()["value"]
        .as_f64()
        .unwrap();
    assert!((t - 10.0).abs() < 1e-9);
    // Cycles are refused.
    assert!(s.execute("parameters.change", &json!({"name": "w", "expression": "t"})).is_err());
    // Favourites, export and import.
    run(&mut s, "parameters.favorite", json!({"name": "w"}));
    assert_eq!(run(&mut s, "parameters.list", json!({"favorites": true}))["parameters"].as_array().unwrap().len(), 1);
    let csv = run(&mut s, "parameters.export", json!({}))["text"].as_str().unwrap().to_string();
    assert!(csv.contains("w,mm,15 mm,15,width,true"), "{csv}");
    run(&mut s, "parameters.import", json!({"text": "name,unit,expression,comment\nw,mm,20 mm,wider\nnew_p,mm,\"w + 1\","}));
    assert!(rel(volume(&mut s), 40.0 * 20.0 * 25.4) < 1e-9);
    run(&mut s, "parameters.import", json!({"text": "[{\"name\": \"w\", \"expression\": \"12\"}]"}));
    assert!(rel(volume(&mut s), 24.0 * 12.0 * 25.4) < 1e-9);
    assert!(s.execute("parameters.import", &json!({"text": "name,expression\nw,1/"})).is_err());
    // Saved and reloaded, the names stay.
    let back = solidcraft_doc::Document::from_json(&s.doc.to_json()).unwrap();
    assert_eq!(back.features[0].param_names, s.doc.features[0].param_names);
}

#[test]
fn rolling_back_and_forward_leaves_no_projection_warnings() {
    let mut s = Session::default();
    s.run_script(&crate::sample::script()).unwrap();
    let warnings = |s: &Session| -> Vec<String> {
        s.doc.features.iter().filter_map(|f| s.model.result(f.id).and_then(|r| r.warning.clone()).map(|w| format!("{}: {w}", f.name))).collect()
    };
    assert!(warnings(&s).is_empty(), "{:?}", warnings(&s));
    for pos in 0..s.doc.features.len() {
        run(&mut s, "timeline.roll_to", json!({ "position": pos }));
        run(&mut s, "timeline.roll_to", json!({}));
        assert!(warnings(&s).is_empty(), "after rolling to {pos}: {:?}", warnings(&s));
    }
}

/// Export STEP, validate the file, read it back: no warnings, same volume, area and faces.
fn step_round_trip(s: &mut Session, tag: &str) -> solidcraft_kernel::StepImport {
    let p = std::env::temp_dir().join(format!("solidcraft-steprt-{tag}-{}.step", std::process::id()));
    run(s, "file.export", json!({"path": p.to_string_lossy()}));
    let text = std::fs::read_to_string(&p).unwrap();
    let _ = std::fs::remove_file(&p);
    solidcraft_kernel::step_validate(&text).unwrap();
    assert_eq!(solidcraft_kernel::step_orientation_errors(&text).unwrap(), Vec::<String>::new());
    assert!(text.contains("AP242_MANAGED_MODEL_BASED_3D_ENGINEERING_MIM_LF"));
    let imp = solidcraft_kernel::step_import(&text).unwrap();
    assert!(imp.warnings.is_empty(), "{:?}", imp.warnings);
    imp
}

#[test]
fn sample_plate_step_round_trip() {
    let mut s = Session::default();
    s.run_script(&crate::sample::script()).unwrap();
    let imp = step_round_trip(&mut s, "sample");
    let st = s.world_state();
    assert_eq!(imp.bodies.len(), st.bodies.len());
    for (a, b) in st.bodies.iter().zip(&imp.bodies) {
        assert_eq!(a.name, b.name);
        let (ma, mb) = (solidcraft_kernel::measure(&a.body).unwrap(), solidcraft_kernel::measure(&b.body).unwrap());
        assert!(rel(mb.volume, ma.volume) < 1e-4, "{} vs {}", mb.volume, ma.volume);
        assert!(rel(mb.area, ma.area) < 1e-4, "{} vs {}", mb.area, ma.area);
        assert!(b.file_faces <= a.body.face_count() && b.file_faces >= ma.merged.faces.min(a.body.face_count()));
        assert_eq!(mb.merged.faces, ma.merged.faces);
    }
}

/// Bodies export in the colour they are shown in: a material's look, an appearance over it,
/// none for the default; and colours read from a STEP file survive the next export.
#[test]
fn material_and_appearance_colours_export_to_step() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10, "body_name": "BrassBox"}));
    run(&mut s, "solid.box", json!({"corner": [20, 0, 0], "length": 10, "width": 10, "height": 10, "body_name": "RedAbs"}));
    run(&mut s, "solid.box", json!({"corner": [40, 0, 0], "length": 10, "width": 10, "height": 10, "body_name": "Plain"}));
    run(&mut s, "material.assign", json!({"bodies": ["BrassBox"], "material": "Brass"}));
    run(&mut s, "material.assign", json!({"bodies": ["RedAbs"], "material": "ABS Plastic"}));
    run(&mut s, "appearance.assign", json!({"bodies": ["RedAbs"], "color": "#d01c1c"}));
    assert!(s.execute("appearance.assign", &json!({"bodies": ["RedAbs"], "color": "#12"})).is_err());
    let rgb = |c: [u8; 3]| c.map(|x| x as f32 / 255.0);
    let want =
        [("BrassBox", Some(rgb(solidcraft_doc::material_color("Brass").unwrap()))), ("RedAbs", Some(rgb([0xd0, 0x1c, 0x1c]))), ("Plain", None)];
    let same = |a: Option<[f32; 3]>, b: Option<[f32; 3]>| match (a, b) {
        (Some(a), Some(b)) => a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-3),
        (None, None) => true,
        _ => false,
    };
    let check = |imp: &solidcraft_kernel::StepImport| {
        for (name, c) in want {
            let b = imp.bodies.iter().find(|b| b.name == name).unwrap_or_else(|| panic!("no body {name}"));
            assert!(same(b.body.color(), c), "{name}: {:?} vs {c:?}", b.body.color());
        }
    };
    check(&step_round_trip(&mut s, "colours"));
    // Open the export and export again: the file's colours are kept.
    let p = std::env::temp_dir().join(format!("solidcraft-colours-{}.step", std::process::id()));
    run(&mut s, "file.export", json!({"path": p.to_string_lossy()}));
    let mut s2 = Session::default();
    run(&mut s2, "doc.open", json!({"path": p.to_string_lossy()}));
    let _ = std::fs::remove_file(&p);
    check(&step_round_trip(&mut s2, "colours2"));
}

/// Face appearances and see-through bodies export: STEP styles on the faces with a
/// transparency, read back as face colours.
#[test]
fn face_colours_and_opacity_export() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10, "body_name": "Cube"}));
    run(&mut s, "appearance.assign", json!({"bodies": ["Cube"], "color": "#2040c0", "opacity": 0.5}));
    run(&mut s, "appearance.assign", json!({"faces": [[5, 5, 10]], "color": "#ff0000"}));
    let imp = step_round_trip(&mut s, "facecol");
    let b = &imp.bodies[0];
    let paint = b.body.paint().expect("paint read back");
    assert!((paint.opacity - 0.5).abs() < 1e-3, "{paint:?}");
    let faces = b.body.faces(0.01).unwrap();
    let top = faces.iter().position(|f| (f.centroid.z - 10.0).abs() < 1e-6).unwrap();
    assert_eq!(paint.faces.len(), 1, "{paint:?}");
    assert_eq!(paint.faces[0].face, top);
    assert!((paint.faces[0].color[0] - 1.0).abs() < 1e-3 && paint.faces[0].color[1].abs() < 1e-3, "{paint:?}");
    // Opened again, the face keeps its colour (shown and exported).
    let p = std::env::temp_dir().join(format!("solidcraft-facecol-{}.step", std::process::id()));
    run(&mut s, "file.export", json!({"path": p.to_string_lossy()}));
    let mut s2 = Session::default();
    run(&mut s2, "doc.open", json!({"path": p.to_string_lossy()}));
    let _ = std::fs::remove_file(&p);
    let st = s2.world_state();
    let looks = s2.doc.face_colors(&st.bodies[0]);
    assert_eq!(looks.len(), 1);
    assert_eq!(looks[0].1.color, [255, 0, 0]);
}

/// IGES: a design exports as IGES and the file opens again as an Import feature with the same
/// bodies, names and colours.
#[test]
fn iges_export_and_open() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 40, "width": 30, "height": 5, "body_name": "Plate"}));
    run(&mut s, "solid.cylinder", json!({"radius": 4, "height": 20, "base": [10, 10, 5], "body_name": "Pin"}));
    run(&mut s, "appearance.assign", json!({"bodies": ["Pin"], "color": "#ff0000"}));
    let want: Vec<f64> = s.world_state().bodies.iter().map(|b| solidcraft_kernel::measure(&b.body).unwrap().volume).collect();
    let p = std::env::temp_dir().join(format!("solidcraft-iges-{}.igs", std::process::id()));
    run(&mut s, "file.export", json!({"path": p.to_string_lossy()}));
    let mut s2 = Session::default();
    run(&mut s2, "doc.open", json!({"path": p.to_string_lossy()}));
    let _ = std::fs::remove_file(&p);
    let st = s2.world_state();
    let names: Vec<&str> = st.bodies.iter().map(|b| b.name.as_str()).collect();
    assert_eq!(names, ["Plate", "Pin"]);
    for (b, v) in st.bodies.iter().zip(&want) {
        assert!(rel(solidcraft_kernel::measure(&b.body).unwrap().volume, *v) < 1e-4);
    }
    assert_eq!(st.bodies[1].body.color(), Some([1.0, 0.0, 0.0]));
}

/// Split Body by a face of another body (its surface extended): a box cut by a cylinder's
/// side into the core and the rest; volumes add up exactly.
#[test]
fn split_body_by_a_face() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 10, "width": 20, "height": 30, "body_name": "Block"}));
    run(&mut s, "solid.cylinder", json!({"radius": 4, "height": 40, "base": [5, 10, -5], "body_name": "Tool"}));
    run(&mut s, "solid.split_body", json!({"body": "Block", "tool": {"body": "Tool", "point": [9, 10, 10]}}));
    let st = s.world_state();
    let mut v: Vec<f64> =
        st.bodies.iter().filter(|b| b.name.starts_with("Block")).map(|b| solidcraft_kernel::measure(&b.body).unwrap().volume).collect();
    v.sort_by(f64::total_cmp);
    let core = PI * 16.0 * 30.0;
    assert!(v.len() == 2 && rel(v[0], core) < 1e-3 && rel(v[1], 6000.0 - core) < 1e-3, "{v:?}");
    assert!(s.execute("solid.split_body", &json!({"body": "Block", "tool": {"body": "Tool", "point": [100, 0, 0]}})).is_err());
}

/// Split Body by a sketch line (#1552): the plane through the line along the sketch's normal
/// splits a box; the split follows the line when the sketch changes, the design saves the line,
/// and anything but a straight line of a sketch is refused.
#[test]
fn split_body_by_a_sketch_line() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 10, "width": 20, "height": 30, "body_name": "Block"}));
    let sk = run(&mut s, "sketch.create", json!({"plane": "XY"}))["sketch"].as_u64().unwrap();
    run(&mut s, "sketch.line", json!({"points": [[4, -5], [4, 25]], "ids": ["cut"]}));
    run(&mut s, "sketch.circle.center", json!({"center": [50, 50], "radius": 2, "id": "c"}));
    run(&mut s, "sketch.finish", json!({}));
    let volumes = |s: &Session| {
        let st = s.world_state();
        let mut v: Vec<f64> =
            st.bodies.iter().filter(|b| b.name.starts_with("Block")).map(|b| solidcraft_kernel::measure(&b.body).unwrap().volume).collect();
        v.sort_by(f64::total_cmp);
        v
    };
    for bad in [
        json!({"body": "Block", "sketch": sk, "line": "c"}),
        json!({"body": "Block", "sketch": sk}),
        json!({"body": "Block", "sketch": 999, "line": "cut"}),
    ] {
        assert!(s.execute("solid.split_body", &bad).is_err(), "{bad}");
    }
    run(&mut s, "solid.split_body", json!({"body": "Block", "sketch": sk, "line": "cut"}));
    let v = volumes(&s);
    assert!(v.len() == 2 && rel(v[0], 2400.0) < 1e-6 && rel(v[1], 3600.0) < 1e-6, "{v:?}");
    // The split follows the line.
    run(&mut s, "sketch.edit", json!({"sketch": sk}));
    run(&mut s, "sketch.move_point", json!({"point": "cut.start", "to": [2, -5]}));
    run(&mut s, "sketch.move_point", json!({"point": "cut.end", "to": [2, 25]}));
    run(&mut s, "sketch.finish", json!({}));
    let v = volumes(&s);
    assert!(v.len() == 2 && rel(v[0], 1200.0) < 1e-6 && rel(v[1], 4800.0) < 1e-6, "{v:?}");
    // The design keeps the sketch and the line.
    let saved: Value = serde_json::from_str(&s.doc.to_json()).unwrap();
    let split = saved["features"].as_array().and_then(|f| f.iter().find(|f| f["type"] == "split")).cloned().unwrap();
    assert_eq!((split["sketch"].as_u64(), split["line"].as_str()), (Some(sk), Some("cut")), "{split}");
}

/// Fillet types: chord length (radius from the face angle) and a radius varying along the
/// edge; volumes from the removed material.
#[test]
fn chord_and_variable_fillets() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 10, "width": 20, "height": 30, "body_name": "B"}));
    run(&mut s, "solid.fillet", json!({"edges": [[10, 10, 30]], "radius": 4, "type": "chord"}));
    let r = 4.0 / 2f64.sqrt();
    let v = solidcraft_kernel::measure(&s.world_state().body("B").unwrap().body).unwrap().volume;
    assert!(rel(v, 6000.0 - (1.0 - PI / 4.0) * r * r * 20.0) < 1e-4, "{v}");
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 10, "width": 20, "height": 30, "body_name": "B"}));
    run(&mut s, "solid.fillet", json!({"edges": [[10, 10, 30]], "radius": 2, "type": "variable", "radius2": 5, "start": [10, 0, 30]}));
    let v = solidcraft_kernel::measure(&s.world_state().body("B").unwrap().body).unwrap().volume;
    let removed = (1.0 - PI / 4.0) * 20.0 * (4.0 + 10.0 + 25.0) / 3.0;
    assert!(rel(v, 6000.0 - removed) < 1e-4, "{v}");
    assert!(s.execute("solid.fillet", &json!({"edges": [[10, 10, 30]], "radius": 2, "type": "variable"})).is_err());
    // Down to zero at the far end: the round closes to the corner.
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 10, "width": 20, "height": 30, "body_name": "B"}));
    run(&mut s, "solid.fillet", json!({"edges": [[10, 10, 30]], "radius": 4, "type": "variable", "radius2": 0, "start": [10, 0, 30]}));
    let v = solidcraft_kernel::measure(&s.world_state().body("B").unwrap().body).unwrap().volume;
    assert!(rel(v, 6000.0 - (1.0 - PI / 4.0) * 20.0 * 16.0 / 3.0) < 1e-4, "{v}");
    // A radius through a middle point (a user parameter), straight between: 2 → 4 → 2.
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 10, "width": 20, "height": 30, "body_name": "B"}));
    run(&mut s, "parameters.add", json!({"name": "waist", "expression": "4 mm"}));
    run(
        &mut s,
        "solid.fillet",
        json!({"edges": [[10, 10, 30]], "radius": 2, "type": "variable", "radius2": 2, "start": [10, 0, 30], "mid": [[0.5, "waist"]]}),
    );
    let vol = |s: &Session| solidcraft_kernel::measure(&s.world_state().body("B").unwrap().body).unwrap().volume;
    let removed = 2.0 * (1.0 - PI / 4.0) * 10.0 * (4.0 + 8.0 + 16.0) / 3.0;
    assert!(rel(vol(&s), 6000.0 - removed) < 1e-4, "{}", vol(&s));
    // The middle radius follows its parameter.
    run(&mut s, "parameters.change", json!({"name": "waist", "expression": "3 mm"}));
    let removed = 2.0 * (1.0 - PI / 4.0) * 10.0 * (4.0 + 6.0 + 9.0) / 3.0;
    assert!(rel(vol(&s), 6000.0 - removed) < 1e-4, "{}", vol(&s));
    assert!(
        s.execute(
            "solid.fillet",
            &json!({"edges": [[10, 10, 30]], "radius": 2, "type": "variable", "radius2": 2, "start": [10, 0, 30], "mid": [[1.5, 3]]})
        )
        .is_err()
    );
}

/// Components export as a STEP assembly: products, occurrences, placements.
#[test]
fn components_export_as_step_assembly() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 40, "width": 30, "height": 5, "name": "Plate"}));
    run(&mut s, "component.create", json!({"name": "Pin"}));
    run(&mut s, "solid.cylinder", json!({"radius": 2, "height": 10, "base": [5, 5, 5]}));
    run(&mut s, "occurrence.copy", json!({"component": "Pin", "translate": [20, 0, 0]}));
    let want: f64 = s.world_state().bodies.iter().map(|b| solidcraft_kernel::measure(&b.body).unwrap().volume).sum();
    let imp = step_round_trip(&mut s, "asm");
    assert_eq!(imp.bodies.len(), 3, "{:?}", imp.bodies.iter().map(|b| &b.name).collect::<Vec<_>>());
    let got: f64 = imp.bodies.iter().map(|b| solidcraft_kernel::measure(&b.body).unwrap().volume).sum();
    assert!(rel(got, want) < 1e-4, "{got} vs {want}");
    let root = &imp.tree[0];
    assert_eq!(root.children.len(), 2);
    assert!(root.children.iter().all(|c| c.name == "Pin"));
    // The second pin sits 20 mm along X from the first.
    let xs: Vec<f64> = imp
        .bodies
        .iter()
        .filter(|b| b.path.last().is_some_and(|p| p == "Pin"))
        .map(|b| solidcraft_kernel::measure(&b.body).unwrap().centroid.x)
        .collect();
    assert_eq!(xs.len(), 2);
    assert!(((xs[0] - xs[1]).abs() - 20.0).abs() < 1e-6, "{xs:?}");
}

/// A face extrude follows its face when earlier features change.
#[test]
fn face_extrude_follows_edits() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 40, "width": 30, "height": 20, "name": "Base"}));
    run(&mut s, "solid.extrude", json!({"face": [20, 15, 20], "distance": 10, "operation": "join", "name": "Pad"}));
    assert!(rel(volume(&mut s), 40.0 * 30.0 * 30.0) < 1e-6);
    run(&mut s, "timeline.edit", json!({"feature": "Base", "set": {"height": "25", "length": "50"}}));
    assert!(rel(volume(&mut s), 50.0 * 30.0 * 35.0) < 1e-6, "{}", volume(&mut s));
    // A face revolved about a line: Pappus (area × centroid path).
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 40, "width": 30, "height": 20}));
    let before = volume(&mut s);
    run(
        &mut s,
        "solid.revolve",
        json!({"face": [40, 15, 10], "axis": {"origin": [40, 0, 30], "dir": [0, 1, 0]}, "angle": "90 deg", "operation": "new"}),
    );
    let added = volume(&mut s) - before;
    let want = 30.0 * 20.0 * 20.0 * PI / 2.0;
    assert!(rel(added, want) < 1e-4, "{added} vs {want}");
}

/// Appearances on faces, bodies and components: face > body > component > material.
#[test]
fn appearances_by_face_body_and_component() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10, "body_name": "Root"}));
    run(&mut s, "component.create", json!({"name": "Case"}));
    run(&mut s, "solid.box", json!({"corner": [20, 0, 0], "length": 10, "width": 10, "height": 10, "body_name": "Shell"}));
    run(&mut s, "component.activate", json!({"component": "root"}));
    let lib = run(&mut s, "appearance.library", json!({}));
    assert!(lib["appearances"].as_array().is_some_and(|a| a.len() >= 15 && a.iter().any(|x| x["name"] == "Glass - Clear")));
    let look = |s: &mut Session, b: &str| run(s, "appearance.list", json!({"body": b}))["body"].clone();
    // Material, then component, then body.
    run(&mut s, "material.assign", json!({"bodies": ["Shell"], "material": "Brass"}));
    assert_eq!(look(&mut s, "Shell")["appearance"]["name"], "Brass");
    run(&mut s, "appearance.assign", json!({"components": ["Case"], "appearance": "ABS - Blue"}));
    assert_eq!(look(&mut s, "Shell")["appearance"]["name"], "ABS - Blue");
    assert!(look(&mut s, "Root")["appearance"].is_null(), "root bodies have no component look");
    run(&mut s, "appearance.assign", json!({"bodies": ["Shell"], "color": [255, 0, 0], "opacity": 0.5}));
    let l = look(&mut s, "Shell");
    assert_eq!((l["appearance"]["color"].as_str(), l["appearance"]["opacity"].as_f64()), (Some("#ff0000"), Some(0.5)), "{l}");
    // A face: the top of Shell.
    run(&mut s, "appearance.assign", json!({"faces": [[25, 5, 10]], "appearance": "Glass - Clear"}));
    let l = look(&mut s, "Shell");
    let faces = l["faces"].as_array().cloned().unwrap_or_default();
    assert_eq!(faces.len(), 1, "{l}");
    assert_eq!(faces[0]["appearance"]["name"], "Glass - Clear");
    // Clearing goes back down the chain.
    run(&mut s, "appearance.assign", json!({"faces": [[26, 6, 10]]}));
    assert_eq!(look(&mut s, "Shell")["faces"].as_array().map(Vec::len), Some(0), "cleared by another point on the same face");
    run(&mut s, "appearance.assign", json!({"bodies": ["Shell"], "color": null}));
    assert_eq!(look(&mut s, "Shell")["appearance"]["name"], "ABS - Blue");
    run(&mut s, "appearance.assign", json!({"components": ["Case"]}));
    assert_eq!(look(&mut s, "Shell")["appearance"]["name"], "Brass");
    // Saved and opened again.
    run(&mut s, "appearance.assign", json!({"faces": [[5, 5, 10]], "color": "#00ff00"}));
    let back = solidcraft_doc::Document::from_json(&s.doc.to_json()).unwrap();
    assert_eq!(back.appearances, s.doc.appearances);
    assert!(s.execute("appearance.assign", &json!({"bodies": ["Shell"], "appearance": "Unobtainium"})).is_err());
    assert!(s.execute("appearance.assign", &json!({"color": "#ffffff"})).is_err(), "no target");
    assert!(s.execute("appearance.assign", &json!({"faces": [[500, 500, 500]], "color": "#ffffff"})).is_err(), "no face there");
}

/// The evaluation cache gives exactly what evaluating from scratch gives, through edits,
/// suppression, roll-back, undo and redo.
#[test]
fn cached_evaluation_matches_from_scratch() {
    let mut s = Session::default();
    s.run_script(&crate::sample::script()).unwrap();
    run(
        &mut s,
        "solid.pattern.rectangular",
        json!({"bodies": ["Plate"], "dir1": [1, 0, 0], "count1": 3, "spacing1": 100, "dir2": [0, 1, 0], "count2": 2, "spacing2": 70}),
    );
    run(&mut s, "solid.box", json!({"corner": [-50, 0, 0], "length": 10, "width": 10, "height": 10, "body_name": "Side"}));
    let same = |s: &Session, what: &str| {
        let mut fresh = solidcraft_doc::Model::new();
        fresh.no_cache = true;
        fresh.evaluate(&s.doc);
        let errs = |m: &solidcraft_doc::Model| m.results.iter().map(|r| (r.id, r.error.clone(), r.skipped)).collect::<Vec<_>>();
        assert_eq!(errs(&s.model), errs(&fresh), "{what}: feature results");
        let shape = |st: &solidcraft_doc::ModelState| {
            st.bodies
                .iter()
                .map(|b| {
                    let m = solidcraft_kernel::measure(&b.body).unwrap();
                    // Volume and area to 1e-9: two evaluations from scratch differ in the last
                    // bits too (the kernel's tessellation is not bit-for-bit repeatable).
                    (b.name.clone(), b.feature, (m.volume * 1e6).round(), (m.area * 1e6).round(), m.faces, m.edges, m.vertices, m.merged.faces)
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(shape(&s.model.state()), shape(&fresh.state()), "{what}: bodies");
        assert_eq!(s.model.state().sketches.len(), fresh.state().sketches.len(), "{what}: sketches");
    };
    // Two evaluations from scratch agree to the same precision.
    let mut a = solidcraft_doc::Model::new();
    a.no_cache = true;
    a.evaluate(&s.doc);
    let mut b = solidcraft_doc::Model::new();
    b.no_cache = true;
    b.evaluate(&s.doc);
    let vols = |m: &solidcraft_doc::Model| {
        m.state().bodies.iter().map(|x| (solidcraft_kernel::measure(&x.body).unwrap().volume * 1e6).round()).collect::<Vec<_>>()
    };
    assert_eq!(vols(&a), vols(&b));
    same(&s, "built");
    let hits = |s: &Session| s.model.cache.lock().map(|c| c.hits).unwrap_or(0);
    for (step, (id, p)) in [
        ("parameters.change", json!({"name": "width", "expression": "90 mm"})),
        ("parameters.change", json!({"name": "width", "expression": "80 mm"})),
        ("parameters.change", json!({"name": "thickness", "expression": "10 mm"})),
        ("edit.undo", json!({})),
        ("edit.redo", json!({})),
        ("timeline.suppress", json!({"feature": "Holes", "suppressed": true})),
        ("timeline.suppress", json!({"feature": "Holes", "suppressed": false})),
        ("timeline.roll_to", json!({"position": 6})),
        ("timeline.roll_to", json!({})),
        ("edit.undo", json!({})),
        ("edit.undo", json!({})),
    ]
    .into_iter()
    .enumerate()
    {
        let before = hits(&s);
        run(&mut s, id, p.clone());
        same(&s, &format!("step {step}: {id} {p}"));
        // Setting the width back solves the sketch again from its current state; the
        // features after it see the same geometry and reuse their results. So does undo.
        if step == 1 || step == 3 {
            assert!(hits(&s) > before, "step {step} reuses the earlier results");
        }
    }
}

/// A fillet keeps its edge when an earlier sketch dimension resizes the body (QA s06).
#[test]
fn fillet_follows_its_edge_after_an_upstream_resize() {
    let mut s = Session::default();
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "S"}));
    let r = run(&mut s, "sketch.rectangle.two_point", json!({"p0": [0, 0], "p1": [40, 30]}));
    let bottom = r["curves"][0].as_str().unwrap_or_default().to_string();
    let d = run(&mut s, "sketch.dimension", json!({"entities": [bottom], "value": 40}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.extrude", json!({"distance": 10}));
    run(&mut s, "solid.fillet", json!({"edges": [[40, 15, 10]], "radius": 2}));
    let v = |s: &mut Session| run(s, "inspect.measure", json!({}))["total"]["volume_mm3"].as_f64().unwrap_or(0.0);
    let strip = (4.0 - std::f64::consts::PI) * 30.0;
    assert!((v(&mut s) - (12000.0 - strip)).abs() < 0.05, "{}", v(&mut s));
    let name = d["param"].as_str().or_else(|| d["name"].as_str()).map(str::to_string).unwrap_or_else(|| "d1".into());
    run(&mut s, "parameters.change", json!({"name": name, "expression": "60"}));
    // The right top edge, now at x = 60: 18000 less the same 30 mm strip.
    assert!((v(&mut s) - (18000.0 - strip)).abs() < 0.05, "{}", v(&mut s));
    let fillet = s.doc.features.last().map(|f| f.id).unwrap_or(0);
    assert!(s.model.result(fillet).and_then(|r| r.warning.clone()).is_none(), "followed without a warning");
}

mod naming {
    use serde_json::{Value, json};

    use super::run;
    use crate::Session;

    fn volume(s: &mut Session) -> f64 {
        run(s, "inspect.measure", json!({}))["total"]["volume_mm3"].as_f64().unwrap_or(0.0)
    }

    fn warning(s: &Session, name: &str) -> Option<String> {
        let id = s.doc.find_feature(name).map(|f| f.id)?;
        s.model.result(id).and_then(|r| r.warning.clone())
    }

    /// A 40 x 30 x 10 block (sketch S: l1…l4 counter-clockwise from the origin), a hole H, a
    /// fillet on the right top edge.
    fn block() -> Session {
        let mut s = Session::default();
        run(&mut s, "sketch.create", json!({"plane": "XY", "name": "S"}));
        run(&mut s, "sketch.rectangle.two_point", json!({"p0": [0, 0], "p1": [40, 30]}));
        run(&mut s, "sketch.dimension", json!({"entities": ["l1"], "value": 40}));
        run(&mut s, "sketch.finish", json!({}));
        run(&mut s, "solid.extrude", json!({"distance": 10, "name": "Block"}));
        run(&mut s, "sketch.create", json!({"plane": "XY", "name": "H"}));
        run(&mut s, "sketch.circle.center", json!({"center": [12, 15], "radius": 4}));
        run(&mut s, "sketch.finish", json!({}));
        run(&mut s, "solid.extrude", json!({"sketch": "H", "distance": 10, "operation": "cut", "name": "Hole"}));
        run(&mut s, "solid.fillet", json!({"edges": [[40, 15, 10]], "radius": 2, "name": "Round"}));
        s
    }

    const STRIP: f64 = (4.0 - std::f64::consts::PI) * 30.0;

    #[test]
    fn faces_and_edges_are_named_from_their_history() {
        let mut s = block();
        let f = run(&mut s, "model.faces", json!({"body": "Body1"}));
        let names: Vec<&str> = f["faces"].as_array().into_iter().flatten().filter_map(|x| x["name"].as_str()).collect();
        for want in ["F2:start", "F2:top", "F2:side:l1", "F2:side:l2", "F2:side:l3", "F2:side:l4"] {
            assert!(names.contains(&want), "{want} in {names:?}");
        }
        assert!(names.iter().any(|n| n.starts_with("F4:side:c1")), "{names:?}");
        assert!(names.iter().any(|n| n.starts_with("F5:blend:")), "{names:?}");
        let round = s.doc.find_feature("Round").cloned().unwrap();
        assert_eq!(round.edge_names, vec!["F2:side:l2|F2:top".to_string()]);
    }

    #[test]
    fn a_resize_keeps_the_edge() {
        let mut s = block();
        let d = s.doc.find_feature("S").and_then(|f| f.param_names.first().cloned()).unwrap_or_else(|| "d1".into());
        run(&mut s, "parameters.change", json!({"name": d, "expression": "60"}));
        assert!((volume(&mut s) - (18000.0 - std::f64::consts::PI * 16.0 * 10.0 - STRIP)).abs() < 0.1, "{}", volume(&mut s));
        assert_eq!(warning(&s, "Round"), None);
    }

    #[test]
    fn suppress_and_reorder_upstream_keep_the_edge() {
        let mut s = block();
        let v = volume(&mut s);
        run(&mut s, "timeline.suppress", json!({"feature": "Hole", "suppressed": true}));
        assert!((volume(&mut s) - (12000.0 - STRIP)).abs() < 0.1);
        assert_eq!(warning(&s, "Round"), None);
        run(&mut s, "timeline.suppress", json!({"feature": "Hole", "suppressed": false}));
        assert!((volume(&mut s) - v).abs() < 1e-6);
        // The hole after the fillet: same part, same edge.
        run(&mut s, "timeline.reorder", json!({"feature": "Hole", "position": 4}));
        assert!((volume(&mut s) - v).abs() < 0.1, "{} {v}", volume(&mut s));
        assert_eq!(warning(&s, "Round"), None);
    }

    #[test]
    fn a_split_edge_warns_and_takes_the_nearest_piece() {
        let mut s = block();
        // Upstream of the fillet, a notch through the middle of the filleted edge.
        run(&mut s, "timeline.roll_to", json!({"feature": "Hole"}));
        run(&mut s, "sketch.create", json!({"plane": "XY", "name": "N"}));
        run(&mut s, "sketch.rectangle.two_point", json!({"p0": [36, 12], "p1": [44, 18]}));
        run(&mut s, "sketch.finish", json!({}));
        run(&mut s, "solid.extrude", json!({"sketch": "N", "distance": 10, "operation": "cut", "name": "Notch"}));
        run(&mut s, "timeline.roll_to", json!({}));
        let w = warning(&s, "Round").unwrap_or_default();
        assert!(w.contains("split"), "{w}");
    }

    #[test]
    fn a_removed_segment_warns() {
        let mut s = block();
        // Replace the right side (l2) of the rectangle with a new line: the face it made is gone.
        let sk = s.doc.find_feature("S").map(|f| f.id).unwrap();
        run(&mut s, "sketch.edit", json!({"sketch": sk}));
        run(&mut s, "sketch.delete", json!({"entities": ["l2"]}));
        run(&mut s, "sketch.line", json!({"points": [[40, 0], [40, 30]], "ids": ["right"]}));
        let r: Result<Value, _> = s.execute("sketch.finish", &json!({}));
        assert!(r.is_ok() || r.is_err());
        let w = warning(&s, "Round").unwrap_or_default();
        assert!(w.contains("no longer exists"), "{w}");
    }

    fn face_names(s: &mut Session, body: &str) -> Vec<String> {
        let f = run(s, "model.faces", json!({ "body": body }));
        f["faces"].as_array().into_iter().flatten().filter_map(|x| x["name"].as_str().map(str::to_string)).collect()
    }

    #[test]
    fn primitives_revolves_and_pattern_copies_are_named() {
        let mut s = Session::default();
        run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10, "body_name": "B"}));
        let n = face_names(&mut s, "B");
        for want in ["F1:+x", "F1:-x", "F1:+y", "F1:-y", "F1:+z", "F1:-z"] {
            assert!(n.contains(&want.to_string()), "{want} in {n:?}");
        }
        run(&mut s, "solid.pattern.rectangular", json!({"bodies": ["B"], "dir1": [1, 0, 0], "count1": 3, "spacing1": 20}));
        let bodies: Vec<String> = s.model.state().bodies.iter().map(|b| b.name.clone()).collect();
        let copy = face_names(&mut s, &bodies[2]);
        assert!(copy.iter().all(|x| x.starts_with("F1:") && x.contains("@F2.")), "{copy:?}");
        // A revolved ring: the section's four lines name the four faces.
        let mut r = Session::default();
        run(&mut r, "sketch.create", json!({"plane": "XZ"}));
        run(&mut r, "sketch.line", json!({"points": [[10, 0], [15, 0], [15, 5], [10, 5]], "closed": true, "ids": ["a", "b", "c", "d"]}));
        run(&mut r, "sketch.finish", json!({}));
        run(&mut r, "solid.revolve", json!({"axis": "y", "angle": "270 deg"}));
        let body = r.model.state().bodies[0].name.clone();
        let n = face_names(&mut r, &body);
        for want in ["F2:side:a", "F2:side:b", "F2:side:c", "F2:side:d", "F2:start", "F2:end"] {
            assert!(n.iter().any(|x| x.starts_with(want)), "{want} in {n:?}");
        }
    }

    #[test]
    fn a_sketch_on_a_face_stays_on_that_face() {
        let mut s = Session::default();
        run(&mut s, "sketch.create", json!({"plane": "XY", "name": "S"}));
        run(&mut s, "sketch.rectangle.two_point", json!({"p0": [0, 0], "p1": [40, 30]}));
        run(&mut s, "sketch.finish", json!({}));
        run(&mut s, "solid.extrude", json!({"distance": 10, "name": "Block"}));
        run(&mut s, "sketch.create", json!({"plane": {"face": [30, 15, 10]}, "name": "Top"}));
        run(&mut s, "sketch.circle.center", json!({"center": [30, 15], "radius": 3}));
        run(&mut s, "sketch.finish", json!({}));
        run(&mut s, "solid.extrude", json!({"sketch": "Top", "distance": 5, "operation": "join", "name": "Peg"}));
        let top = |s: &mut Session| run(s, "inspect.measure", json!({}))["bodies"][0]["bbox"]["max"][2].as_f64().unwrap_or(0.0);
        assert!((top(&mut s) - 15.0).abs() < 1e-6);
        let attached = |s: &Session| s.model.state().sketch(s.doc.find_feature("Top").unwrap().id).map(|sketch| sketch.plane.origin.z);
        assert_eq!(attached(&s), Some(10.));
        run(&mut s, "timeline.edit", json!({"feature":"Block","set":{"extent":{"distance":"12"}}}));
        assert_eq!(attached(&s), Some(12.));
        assert!((top(&mut s) - 17.).abs() < 1e-6);
        run(&mut s, "edit.undo", json!({}));
        assert_eq!(attached(&s), Some(10.));
        assert!((top(&mut s) - 15.).abs() < 1e-6);
        let reopened = solidcraft_doc::Document::from_json(&s.doc.to_json()).unwrap();
        let mut model = solidcraft_doc::Model::new();
        model.evaluate(&reopened);
        assert_eq!(model.state().sketch(reopened.find_feature("Top").unwrap().id).unwrap().plane.origin.z, 10.);
        assert!((model.state().bodies[0].mesh().bounds().max.z - 15.).abs() < 1e-6);
        // A historical format-10 attachment uses its saved generic face name and no strong anchor.
        let mut old = (*s.doc).clone();
        let solidcraft_doc::FeatureKind::Sketch { plane: solidcraft_doc::PlaneRef::Face { anchor, name, .. }, sketch } =
            &mut old.features.iter_mut().find(|f| f.name == "Top").unwrap().kind
        else {
            panic!("fixture must contain a face sketch");
        };
        *anchor = None;
        for link in &mut sketch.links {
            if link.auto_face {
                link.auto_face = false;
                link.face_name = name.clone();
            }
        }
        old.format = "solidcraft/10".into();
        let mut legacy = Session::new(solidcraft_doc::Document::from_json(&old.to_json()).unwrap());
        assert_eq!(attached(&legacy), Some(10.));
        assert!((top(&mut legacy) - 15.).abs() < 1e-6);
        // An upstream join that trims the certified face: the attachment stays on what is left
        // of it and never follows the substitute roof (see face_tests for a face covered whole).
        for target in [&mut s, &mut legacy] {
            run(target, "timeline.roll_to", json!({"feature": "Block"}));
            run(target, "sketch.create", json!({"plane": "XY", "name": "B"}));
            run(target, "sketch.rectangle.two_point", json!({"p0": [8, -5], "p1": [45, 35]}));
            run(target, "sketch.finish", json!({}));
            run(target, "solid.extrude", json!({"sketch": "B", "distance": 20, "operation": "join", "name": "Boss"}));
            run(target, "timeline.roll_to", json!({}));
        }
        assert_eq!(attached(&legacy), Some(10.), "historical weak naming retains its saved geometry behavior");
        assert!((top(&mut legacy) - 20.).abs() < 1e-6);
        let legacy_reopened = Session::new(solidcraft_doc::Document::from_json(&legacy.doc.to_json()).unwrap());
        assert_eq!(attached(&legacy_reopened), Some(10.));
        assert!((legacy_reopened.model.state().bodies[0].mesh().bounds().max.z - 20.).abs() < 1e-6);
        let solidcraft_doc::FeatureKind::Sketch { plane: solidcraft_doc::PlaneRef::Face { anchor, .. }, .. } =
            &legacy_reopened.doc.find_feature("Top").unwrap().kind
        else {
            panic!("saved historical face reference must remain present");
        };
        assert!(anchor.is_none(), "opening old face references must not infer a strict attachment");
        let failed = |s: &Session, name: &str| s.model.results.iter().find(|r| r.id == s.doc.find_feature(name).unwrap().id).unwrap().error.is_some();
        assert!(!failed(&s, "Top"));
        assert!(!failed(&s, "Peg"));
        assert_eq!(attached(&s), Some(10.), "the strip of Block's top left beside the boss is still that face");
        assert!((top(&mut s) - 20.0).abs() < 1e-6, "{}", top(&mut s));
        run(&mut s, "timeline.suppress", json!({"feature":"Boss","suppressed":true}));
        assert!(!failed(&s, "Top"));
        assert_eq!(attached(&s), Some(10.));
        assert!((top(&mut s) - 15.).abs() < 1e-6);
    }

    #[test]
    fn shell_inner_faces_are_named_after_the_walls() {
        let mut s = Session::default();
        run(&mut s, "solid.box", json!({"length": 40, "width": 30, "height": 20, "body_name": "B"}));
        run(&mut s, "solid.shell", json!({"faces": [[20, 15, 20]], "thickness": 2}));
        let n = face_names(&mut s, "B");
        for want in ["F2:inner:F1:-z", "F2:inner:F1:+x", "F2:inner:F1:-x", "F2:inner:F1:+y", "F2:inner:F1:-y"] {
            assert!(n.contains(&want.to_string()), "{want} in {n:?}");
        }
    }

    #[test]
    fn shell_and_hole_faces_follow_by_name() {
        let mut s = Session::default();
        run(&mut s, "solid.box", json!({"length": 40, "width": 30, "height": 20, "name": "Base"}));
        run(&mut s, "solid.shell", json!({"faces": [[20, 15, 20]], "thickness": 2, "name": "Hollow"}));
        let shell = |h: f64| 40.0 * 30.0 * h - 36.0 * 26.0 * (h - 2.0);
        assert!((volume(&mut s) - shell(20.0)).abs() < 0.5, "{}", volume(&mut s));
        // Taller: the picked point is 5 mm under the open face now; its name still finds it.
        run(&mut s, "timeline.edit", json!({"feature": "Base", "set": {"height": "25"}}));
        assert!((volume(&mut s) - shell(25.0)).abs() < 0.5, "{}", volume(&mut s));
        assert_eq!(warning(&s, "Hollow"), None);
        let h = s.doc.find_feature("Hollow").map(|f| f.face_names.clone()).unwrap_or_default();
        assert_eq!(h, vec!["F1:+z".to_string()]);
    }
}

/// Tapped holes take the tap drill (major − pitch); clearance holes the ISO 273 diameter.
#[test]
fn tapped_and_clearance_holes() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 40, "width": 30, "height": 10}));
    let v0 = volume(&mut s);
    run(&mut s, "solid.hole", json!({"position": [10, 15, 10], "thread": "M6"}));
    let v1 = volume(&mut s);
    assert!(rel(v0 - v1, PI * 2.5 * 2.5 * 10.0) < 1e-3, "{}", v0 - v1);
    run(&mut s, "solid.hole", json!({"position": [30, 15, 10], "clearance": "M6"}));
    let v2 = volume(&mut s);
    assert!(rel(v1 - v2, PI * 3.3 * 3.3 * 10.0) < 1e-3, "{}", v1 - v2);
    run(&mut s, "solid.hole", json!({"position": [20, 5, 10], "clearance": "M4", "fit": "close"}));
    assert!(rel(v2 - volume(&mut s), PI * 2.15 * 2.15 * 10.0) < 1e-3);
    assert!(s.execute("solid.hole", &json!({"position": [20, 25, 10], "clearance": "M7"})).is_err());
    assert!(s.execute("solid.hole", &json!({"position": [20, 25, 10]})).is_err());
}

/// In an inch design, a bare length typed into a feature means inches (stored with the unit);
/// lengths with units and angles are left alone.
#[test]
fn bare_lengths_take_the_design_units() {
    let mut s = Session::default();
    s.doc_mut().units = "in".into();
    run(&mut s, "solid.box", json!({"length": 2, "width": "10 mm", "height": 1}));
    let m = run(&mut s, "inspect.measure", json!({}));
    let max = |i: usize| m["bodies"][0]["bbox"]["max"][i].as_f64().unwrap_or(0.0);
    assert!((max(0) - 50.8).abs() < 1e-9 && (max(1) - 10.0).abs() < 1e-9 && (max(2) - 25.4).abs() < 1e-9, "{m}");
    let d = s.doc.features[0].param_names[2].clone();
    run(&mut s, "parameters.change", json!({"name": d, "expression": "2"}));
    let m = run(&mut s, "inspect.measure", json!({}));
    assert!((m["bodies"][0]["bbox"]["max"][2].as_f64().unwrap_or(0.0) - 50.8).abs() < 1e-9);
    assert_eq!(s.doc.features[0].kind.inputs()[2].1, "2 in");
}

/// Press Pull on a fillet's face changes its radius; on another curved face it offsets it.
#[test]
fn press_pull_on_curved_faces() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 40, "width": 30, "height": 20}));
    run(&mut s, "solid.fillet", json!({"edges": [[40, 0, 10]], "radius": 3, "name": "Round"}));
    let corner = |r: f64| (4.0 - PI) * r * r / 4.0 * 20.0;
    assert!(rel(volume(&mut s), 24000.0 - corner(3.0)) < 1e-4);
    // A point on the fillet: the arc's middle.
    let c = Vec3::new(37.0, 3.0, 10.0);
    let on = c + Vec3::new(1.0, -1.0, 0.0).normalized().unwrap() * 3.0;
    let out = run(&mut s, "solid.press_pull", json!({"face": [on.x, on.y, on.z], "distance": 2}));
    assert_eq!(out["edited"], "Round", "{out}");
    assert!(rel(volume(&mut s), 24000.0 - corner(5.0)) < 1e-4, "{}", volume(&mut s));
    // A hole's wall: pulled along its normal (into the hole), the hole gets smaller.
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 40, "width": 30, "height": 20}));
    run(&mut s, "solid.hole", json!({"position": [20, 15, 20], "diameter": 10}));
    let v0 = volume(&mut s);
    run(&mut s, "solid.press_pull", json!({"face": [25, 15, 10], "distance": 1}));
    let v1 = volume(&mut s);
    assert!(rel(v1 - v0, PI * (25.0 - 16.0) * 20.0) < 1e-3, "{v0} {v1}");
}

/// A cut in one component leaves other components' bodies alone unless they are named as
/// participants; auto join/cut looks at the active component only.
#[test]
fn booleans_stay_in_their_component() {
    let mut s = Session::default();
    for (name, x) in [("A", 0), ("B", 20)] {
        run(&mut s, "component.activate", json!({"component": "root"}));
        run(&mut s, "component.create", json!({"name": name}));
        run(&mut s, "solid.box", json!({"length": 20, "width": 10, "height": 10, "corner": [x, 0, 0], "body_name": format!("{name}Box")}));
    }
    // A's box spans x 0…20, B's x 20…40 (in their own frames, both occurrences in place).
    run(&mut s, "component.activate", json!({"component": "A"}));
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Slot"}));
    run(&mut s, "sketch.rectangle.two_point", json!({"p0": [8, 2], "p1": [28, 8]}));
    run(&mut s, "sketch.finish", json!({}));
    let vol = |s: &mut Session, b: &str| run(s, "inspect.measure", json!({"bodies": [b]}))["total"]["volume_mm3"].as_f64().unwrap_or(0.0);
    assert_eq!(auto_operation(&s, &json!({"sketch": "Slot", "distance": 10})), Some("cut"));
    run(&mut s, "solid.extrude", json!({"sketch": "Slot", "distance": 10, "operation": "cut", "name": "Cut"}));
    assert!((vol(&mut s, "ABox") - (2000.0 - 12.0 * 6.0 * 10.0)).abs() < 1e-6, "{}", vol(&mut s, "ABox"));
    assert!((vol(&mut s, "BBox") - 2000.0).abs() < 1e-6, "B untouched: {}", vol(&mut s, "BBox"));
    // Named as a participant, B is cut too.
    run(&mut s, "edit.undo", json!({}));
    run(&mut s, "solid.extrude", json!({"sketch": "Slot", "distance": 10, "operation": "cut", "participants": ["ABox", "BBox"]}));
    assert!((vol(&mut s, "BBox") - (2000.0 - 8.0 * 6.0 * 10.0)).abs() < 1e-6, "{}", vol(&mut s, "BBox"));
    assert!(s.execute("solid.extrude", &json!({"sketch": "Slot", "distance": 1, "operation": "cut", "participants": ["Nope"]})).is_err());
}

/// #1234: a cut sketched in the root, where there is no body, through a body in each of two
/// components cuts both (the root has nothing of its own to cut). A cut in a component with a
/// body of its own still stays there (`booleans_stay_in_their_component`).
#[test]
fn a_cut_from_a_component_without_bodies_cuts_the_parts_it_meets() {
    let mut s = Session::default();
    for (name, x) in [("A", 0), ("B", 30)] {
        run(&mut s, "component.activate", json!({"component": "root"}));
        run(&mut s, "component.create", json!({"name": name}));
        run(&mut s, "solid.box", json!({"length": 20, "width": 10, "height": 10, "corner": [x, 0, 0], "body_name": format!("{name}Box")}));
    }
    run(&mut s, "component.activate", json!({"component": "root"}));
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Slot"}));
    run(&mut s, "sketch.rectangle.two_point", json!({"p0": [10, 2], "p1": [40, 8]}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.extrude", json!({"sketch": "Slot", "distance": 10, "operation": "cut"}));
    let vol = |s: &mut Session, b: &str| run(s, "inspect.measure", json!({"bodies": [b]}))["total"]["volume_mm3"].as_f64().unwrap_or(0.0);
    assert!((vol(&mut s, "ABox") - (2000.0 - 10.0 * 6.0 * 10.0)).abs() < 1e-6, "{}", vol(&mut s, "ABox"));
    assert!((vol(&mut s, "BBox") - (2000.0 - 10.0 * 6.0 * 10.0)).abs() < 1e-6, "{}", vol(&mut s, "BBox"));
}

/// A unit-less parameter used as a length means the design's units.
#[test]
fn unitless_parameters_used_as_lengths_take_the_design_units() {
    let mut s = Session::default();
    s.doc_mut().units = "in".into();
    run(&mut s, "parameters.add", json!({"name": "w", "expression": "2", "unit": ""}));
    run(&mut s, "solid.box", json!({"length": "w", "width": "w * 1", "height": "10 mm"}));
    let m = run(&mut s, "inspect.measure", json!({}));
    let max = |i: usize| m["bodies"][0]["bbox"]["max"][i].as_f64().unwrap_or(0.0);
    assert!((max(0) - 50.8).abs() < 1e-9 && (max(1) - 50.8).abs() < 1e-9 && (max(2) - 10.0).abs() < 1e-9, "{m}");
}

/// A chamfer on a tapered pocket survives parameter edits and undo/redo.
#[test]
fn chamfer_sloped_basin_rim_recomputes() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 100, "width": 60, "height": 30}));
    for (name, z, p0, p1) in [("Rim", 30, [20, 4], [80, 50]), ("Floor", 10, [30, 20], [70, 40])] {
        run(&mut s, "sketch.create", json!({"plane": "XY", "offset": z, "name": name}));
        run(&mut s, "sketch.rectangle.two_point", json!({"p0": p0, "p1": p1}));
        run(&mut s, "sketch.finish", json!({}));
    }
    run(&mut s, "solid.loft", json!({"sections": [{"sketch": "Rim"}, {"sketch": "Floor"}], "operation": "cut"}));
    run(&mut s, "parameters.add", json!({"name": "rim_chamfer", "expression": "1 mm"}));
    let base = volume(&mut s);
    run(&mut s, "solid.chamfer", json!({"edges": [[50, 4, 30]], "distance": "rim_chamfer"}));
    let expected = |size: f64| {
        let wall_slope: f64 = (20.0 - 4.0) / (30.0 - 10.0); // horizontal run / vertical drop
        let drop = size / (1.0 + wall_slope.powi(2)).sqrt();
        base - 0.5 * size * drop * (60.0 - drop / 3.0)
    };
    assert!(rel(volume(&mut s), expected(1.0)) < 1e-8);
    let edited = run(&mut s, "parameters.change", json!({"name": "rim_chamfer", "expression": "2 mm"}));
    assert_eq!(edited["errors"].as_array().unwrap().len(), 0, "{edited}");
    assert!(rel(volume(&mut s), expected(2.0)) < 1e-8);
    run(&mut s, "edit.undo", json!({}));
    assert!(rel(volume(&mut s), expected(1.0)) < 1e-8);
    run(&mut s, "edit.redo", json!({}));
    assert!(rel(volume(&mut s), expected(2.0)) < 1e-8);
}

/// Chamfers with two distances or a distance and an angle.
#[test]
fn unequal_chamfers() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 40, "width": 30, "height": 20}));
    // Top front edge: 2 along the top (the face facing up), 4 down the front.
    run(&mut s, "solid.chamfer", json!({"edges": [[20, 0, 20]], "distance": 2, "distance2": 4}));
    assert!(rel(volume(&mut s), 24000.0 - 0.5 * 2.0 * 4.0 * 40.0) < 1e-6, "{}", volume(&mut s));
    // Bottom back edge: 3 along the bottom, at 30° from it (down the back face 3·tan 30°).
    run(&mut s, "solid.chamfer", json!({"edges": [[20, 30, 0]], "distance": 3, "angle": "30 deg", "flip": true}));
    let d2 = 3.0 * (30f64).to_radians().tan();
    assert!(rel(volume(&mut s), 24000.0 - 160.0 - 0.5 * 3.0 * d2 * 40.0) < 1e-6, "{}", volume(&mut s));
    assert!(s.execute("solid.chamfer", &json!({"edges": [[0, 15, 20]], "distance": 1, "distance2": 2, "angle": "30 deg"})).is_err());
}

/// An enclosure built in the usual order: shell, bosses, then the lip on the rim.
#[test]
fn lip_after_boss() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 80, "width": 50, "height": 30}));
    run(&mut s, "solid.shell", json!({"faces": [[40, 25, 30]], "thickness": 2}));
    run(&mut s, "plastic.boss", json!({"position": [10, 10, 2], "diameter": 7, "height": 12, "hole_diameter": 2.5}));
    run(&mut s, "plastic.boss", json!({"position": [70, 40, 2], "diameter": 7, "height": 12, "hole_diameter": 2.5}));
    let v0 = volume(&mut s);
    run(&mut s, "plastic.lip", json!({"face": [1, 25, 30], "width": 1, "height": 2}));
    let v = volume(&mut s);
    assert!(v > v0 + 1.0, "{v0} {v}");
    // Bosses up to the rim, with ribs, against a wall; lips, grooves and outside rims.
    let bosses = [
        json!({"position": [15, 15, 2], "diameter": 8, "height": 28, "hole_diameter": 3}),
        // Ribs out to the inner walls (15 − 4 − 9 = 2), and short of them.
        json!({"position": [15, 15, 2], "diameter": 8, "height": 12, "ribs": 4, "rib_thickness": 1.5, "rib_length": 9}),
        json!({"position": [15, 15, 2], "diameter": 8, "height": 12, "ribs": 4, "rib_thickness": 1.5, "rib_length": 6}),
        json!({"position": [5.5, 25, 2], "diameter": 7, "height": 12, "hole_diameter": 2.5}),
        json!({"position": [40, 25, 2], "diameter": 8, "height": 20, "draft": "1 deg", "fillet": 1}),
    ];
    let lips = [
        json!({"face": [1, 25, 30], "width": 1, "height": 2}),
        json!({"face": [1, 25, 30], "width": 1, "height": 2, "type": "groove"}),
        json!({"face": [1, 25, 30], "width": 1, "height": 2, "side": "outside"}),
    ];
    for (i, b) in bosses.iter().enumerate() {
        for (j, l) in lips.iter().enumerate() {
            let mut s = Session::default();
            run(&mut s, "solid.box", json!({"length": 80, "width": 50, "height": 30}));
            run(&mut s, "solid.shell", json!({"faces": [[40, 25, 30]], "thickness": 2}));
            let r = s.execute("plastic.boss", b);
            assert!(r.is_ok(), "boss {i}: {r:?}");
            let v0 = volume(&mut s);
            let r = s.execute("plastic.lip", l);
            assert!(r.is_ok(), "boss {i} lip {j}: {r:?}");
            assert!((volume(&mut s) - v0).abs() > 1.0, "boss {i} lip {j}");
        }
    }
}

/// A lip on a rim with a face elsewhere in line with the pick: a notch under the floor whose
/// wall lies in the plane y = 25 through the picked point. The pick is the rim's, the face the
/// point lies on, not the first face whose plane runs through it.
#[test]
fn lip_on_a_rim_in_line_with_another_face() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 80, "width": 50, "height": 30}));
    run(&mut s, "solid.shell", json!({"faces": [[40, 25, 30]], "thickness": 2}));
    run(&mut s, "solid.box", json!({"length": 20, "width": 10, "height": 1, "corner": [30, 25, 0], "operation": "cut", "targets": ["Body1"]}));
    let v0 = volume(&mut s);
    run(&mut s, "plastic.lip", json!({"face": [1, 25, 30], "width": 1, "height": 2}));
    // The band inside the rim, 1 wide and 2 high: (78 × 48 − 76 × 46) × 2.
    assert!(rel(volume(&mut s) - v0, 496.0) < 1e-6, "{}", volume(&mut s) - v0);
}

/// A drilled hole as deep as the plate: the drill point breaks through the bottom.
#[test]
fn hole_as_deep_as_the_plate() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 50, "width": 25, "height": 10}));
    let v0 = volume(&mut s);
    run(&mut s, "solid.hole", json!({"position": [25, 12.5, 10], "diameter": 6, "depth": 10, "type": "drilled"}));
    let v = volume(&mut s);
    assert!(v < v0 - PI * 9.0 * 9.0 && v > v0 - PI * 9.0 * 10.0 - 1.0, "{v0} {v}");
    // Other sizes and places, the defaults, and a tapped hole.
    for (i, p) in [
        json!({"position": [10, 10, 10], "diameter": 5, "depth": 10, "type": "drilled"}),
        json!({"position": [40, 15, 10], "diameter": 5, "depth": 10}),
        json!({"position": [12, 6, 10], "diameter": 8.5, "depth": 10}),
        json!({"position": [30, 7, 10], "thread": "M6", "depth": 10}),
        json!({"position": [20, 18, 10], "diameter": 4, "depth": 10, "tip_angle": 90}),
    ]
    .into_iter()
    .enumerate()
    {
        let before = volume(&mut s);
        let r = s.execute("solid.hole", &p);
        assert!(r.is_ok(), "{i} {p}: {r:?}");
        assert!(volume(&mut s) < before - 1.0, "{i} {p}");
    }
}

/// Appearances outlive later features: a coloured face keeps its colour after a fillet on a
/// neighbouring edge; an imported STEP's body and face colours survive a hole.
#[test]
fn appearances_survive_later_features() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 40, "width": 30, "height": 20, "body_name": "B"}));
    run(&mut s, "appearance.assign", json!({"faces": [[20, 15, 20]], "color": "#ff0000"}));
    run(&mut s, "solid.fillet", json!({"edges": [[40, 15, 0]], "radius": 3}));
    let red = |s: &mut Session, body: &str| {
        let l = run(s, "appearance.list", json!({ "body": body }));
        l["body"]["faces"].as_array().into_iter().flatten().any(|f| f["appearance"]["color"] == "#ff0000")
    };
    assert!(red(&mut s, "B"), "the top face is still red after the fillet");
    // Exported with a body colour and the red face, then opened: the colours come in as the
    // body's own; a hole afterwards keeps them.
    run(&mut s, "appearance.assign", json!({"bodies": ["B"], "color": "#2050c0"}));
    let dir = std::env::temp_dir().join(format!("solidcraft-paint-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let step = dir.join("painted.step");
    run(&mut s, "file.export", json!({"path": step.to_string_lossy()}));
    let mut t = Session::default();
    run(&mut t, "doc.open", json!({"path": step.to_string_lossy()}));
    let body = t.model.state().bodies[0].name.clone();
    let before = t.doc.body_color(&t.model.state().bodies[0]);
    assert!(before.is_some());
    run(&mut t, "solid.hole", json!({"position": [10, 10, 20], "diameter": 4}));
    let st = t.model.state();
    let b = st.body(&body).unwrap();
    assert!(b.body.paint().is_none(), "the hole made a new kernel body");
    assert_eq!(t.doc.body_color(b), before, "the imported body colour carries on");
    assert!(red(&mut t, &body), "the imported red face carries on");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The original #2 RGB import acceptance, including later topology and placement edits.
#[test]
fn imported_rgb_appearances_survive_edits_reopen_and_step_export() {
    let dir = std::env::temp_dir().join(format!("solidcraft-imported-rgb-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let input = dir.join("rgb.step");
    let saved = dir.join("edited.solidcraft");
    let output = dir.join("edited.step");
    let mut source = Session::default();
    for (name, x, color) in [("Red", 0, "#ff0000"), ("Green", 40, "#00ff00"), ("Blue", 80, "#0000ff")] {
        run(&mut source, "solid.box", json!({"corner":[x,0,0],"length":30,"width":20,"height":20,"body_name":name}));
        run(&mut source, "appearance.assign", json!({"bodies":[name],"color":color}));
    }
    run(&mut source, "appearance.assign", json!({"faces":[[15,10,20]],"color":"#ffff00"}));
    run(&mut source, "file.export", json!({"path":input}));
    let check = |session: &Session| {
        let state = session.model.state();
        assert_eq!(state.bodies.len(), 3);
        for (name, color) in [("Red", [1., 0., 0.]), ("Green", [0., 1., 0.]), ("Blue", [0., 0., 1.])] {
            let body = state.body(name).unwrap();
            assert_eq!(session.doc.body_color(body), Some(color), "{name} retains its imported body colour");
        }
        let red = state.body("Red").unwrap();
        assert!(session.doc.face_colors(red).iter().any(|(_, look)| look.color == [255, 255, 0]), "Red retains its yellow top face");
        assert!(session.model.results.iter().all(|result| result.error.is_none()));
    };
    let mut edited = Session::default();
    run(&mut edited, "doc.open", json!({"path":input}));
    check(&edited);
    let initial_volume = volume(&mut edited);
    run(&mut edited, "solid.fillet", json!({"bodies":["Red"],"edges":[[0,10,20]],"radius":2}));
    check(&edited);
    run(&mut edited, "solid.move", json!({"bodies":["Red"],"translate":[0,0,5]}));
    check(&edited);
    run(&mut edited, "solid.box", json!({"corner":[85,5,15],"length":10,"width":10,"height":10,"body_name":"Cutter"}));
    run(&mut edited, "solid.combine", json!({"target":"Blue","tools":["Cutter"],"operation":"cut"}));
    check(&edited);
    run(&mut edited, "solid.box", json!({"corner":[65,0,0],"length":10,"width":20,"height":20,"body_name":"Joiner"}));
    run(&mut edited, "solid.combine", json!({"target":"Green","tools":["Joiner"],"operation":"join"}));
    check(&edited);
    let edited_volume = volume(&mut edited);
    // The join adds 5*20*20, the pocket removes 10*10*5, and the
    // radius-2 quarter-round replaces a 2*2 square along a 20 mm edge.
    let expected_volume = initial_volume + 2000. - 500. - (4. - PI) * 20.;
    assert!((edited_volume - expected_volume).abs() < 0.1, "{edited_volume} vs {expected_volume}");
    run(&mut edited, "edit.undo", json!({}));
    run(&mut edited, "edit.redo", json!({}));
    check(&edited);
    run(&mut edited, "file.save_as", json!({"path":saved}));
    let mut reopened = Session::default();
    run(&mut reopened, "doc.open", json!({"path":saved}));
    check(&reopened);
    assert!(rel(volume(&mut reopened), edited_volume) < 1e-8);
    run(&mut reopened, "file.export", json!({"path":output}));
    let mut roundtrip = Session::default();
    run(&mut roundtrip, "doc.open", json!({"path":output}));
    check(&roundtrip);
    assert!(rel(volume(&mut roundtrip), edited_volume) < 1e-3);
    let _ = std::fs::remove_dir_all(dir);
}

/// Extrude To Object (a parallel face, with an offset; a slanted face) and From Object.
#[test]
fn extrude_to_and_from_objects() {
    let mut s = Session::default();
    // A 10 mm plate at z 30…40 to extrude up to, and a profile on XY.
    run(&mut s, "solid.box", json!({"corner": [0, 0, 30], "length": 40, "width": 40, "height": 10, "body_name": "Roof"}));
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "P"}));
    run(&mut s, "sketch.rectangle.two_point", json!({"p0": [5, 5], "p1": [15, 15]}));
    run(&mut s, "sketch.finish", json!({}));
    let top = |s: &mut Session, b: &str| run(s, "inspect.measure", json!({"bodies": [b]}))["bodies"][0]["bbox"]["max"][2].as_f64().unwrap_or(0.0);
    run(&mut s, "solid.extrude", json!({"sketch": "P", "to": [20, 20, 30], "body_name": "Post"}));
    assert!((top(&mut s, "Post") - 30.0).abs() < 1e-6, "{}", top(&mut s, "Post"));
    run(&mut s, "edit.undo", json!({}));
    run(&mut s, "solid.extrude", json!({"sketch": "P", "to": [20, 20, 30], "to_offset": -5, "body_name": "Post"}));
    assert!((top(&mut s, "Post") - 25.0).abs() < 1e-6);
    run(&mut s, "edit.undo", json!({}));
    // From the roof's top face (z 40) to 10 mm above it.
    run(&mut s, "solid.extrude", json!({"sketch": "P", "from": [20, 20, 40], "distance": 10, "body_name": "Post"}));
    let m = run(&mut s, "inspect.measure", json!({"bodies": ["Post"]}));
    assert!((m["bodies"][0]["bbox"]["min"][2].as_f64().unwrap_or(0.0) - 40.0).abs() < 1e-6 && (top(&mut s, "Post") - 50.0).abs() < 1e-6, "{m}");
    // The To face is kept by its persistent name, so it is followed when it moves.
    run(&mut s, "edit.undo", json!({}));
    run(&mut s, "solid.extrude", json!({"sketch": "P", "to": [20, 20, 30], "body_name": "Post"}));
    let name = s.doc.features.last().map(|f| f.face_names.clone()).unwrap_or_default();
    assert_eq!(name, vec!["F1:-z".to_string()], "the To face is stored by name");
    assert!(s.execute("solid.extrude", &json!({"sketch": "P", "to": "nope"})).is_err());
}

/// Revolve Two Sides, Symmetric and To Object.
#[test]
fn revolve_two_sides_symmetric_and_to_object() {
    let ring = |extra: Value| {
        let mut s = Session::default();
        run(&mut s, "sketch.create", json!({"plane": "XZ", "name": "S"}));
        run(&mut s, "sketch.line", json!({"points": [[10, 0], [15, 0], [15, 5], [10, 5]], "closed": true}));
        run(&mut s, "sketch.finish", json!({}));
        let mut p = json!({"sketch": "S", "axis": "y", "angle": "90 deg"});
        for (k, v) in extra.as_object().into_iter().flatten() {
            p[k] = v.clone();
        }
        run(&mut s, "solid.revolve", p);
        let m = run(&mut s, "inspect.measure", json!({}));
        (m["total"]["volume_mm3"].as_f64().unwrap_or(0.0), m["bodies"][0]["bbox"].clone())
    };
    let full = PI * (225.0 - 100.0) * 5.0;
    let (v, _) = ring(json!({}));
    assert!(rel(v, full / 4.0) < 1e-3);
    let (v, _) = ring(json!({"angle2": "45 deg"}));
    assert!(rel(v, full * 135.0 / 360.0) < 1e-3, "{v}");
    let (v, b) = ring(json!({"direction": "symmetric"}));
    assert!(rel(v, full / 4.0) < 1e-3);
    // Split evenly: the extent either side of the sketch plane (y = 0) is the same.
    assert!((b["max"][1].as_f64().unwrap_or(0.0) + b["min"][1].as_f64().unwrap_or(0.0)).abs() < 1e-3, "{b}");
    // To a point a quarter turn round: the same as 90°.
    let (v90, b90) = ring(json!({}));
    let corner = if b90["max"][1].as_f64().unwrap_or(0.0) > 1.0 { json!([0, 12, 0]) } else { json!([0, -12, 0]) };
    let (v, _) = ring(json!({"to": corner}));
    assert!(rel(v, v90) < 1e-3, "{v} {v90}");
}

/// Shell Inside, Outside and Both, and a tangent chain of open faces.
#[test]
fn shell_directions_and_tangent_chain() {
    let shell = |extra: Value| {
        let mut s = Session::default();
        run(&mut s, "solid.box", json!({"length": 40, "width": 30, "height": 20}));
        let mut p = json!({"faces": [[20, 15, 20]], "thickness": 2});
        for (k, v) in extra.as_object().into_iter().flatten() {
            p[k] = v.clone();
        }
        run(&mut s, "solid.shell", p);
        let m = run(&mut s, "inspect.measure", json!({}));
        (m["total"]["volume_mm3"].as_f64().unwrap_or(0.0), m["bodies"][0]["bbox"].clone())
    };
    let (v, _) = shell(json!({}));
    assert!(rel(v, 40.0 * 30.0 * 20.0 - 36.0 * 26.0 * 18.0) < 1e-6, "{v}");
    // Outside: the walls grow out; the original box is the cavity.
    let (v, b) = shell(json!({"direction": "outside"}));
    assert!(rel(v, 44.0 * 34.0 * 22.0 - 40.0 * 30.0 * 20.0) < 1e-6, "{v} {b}");
    // Both: 1 mm out, 1 mm in.
    let (v, _) = shell(json!({"direction": "both"}));
    assert!(rel(v, 42.0 * 32.0 * 21.0 - 38.0 * 28.0 * 19.0) < 1e-6, "{v}");
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 40, "width": 30, "height": 20}));
    assert!(s.execute("solid.shell", &json!({"faces": [[20, 15, 20]], "thickness": 2, "direction": "sideways"})).is_err());
}

/// Hole To Object (a face it meets, or a point) and holes at every point of a sketch.
#[test]
fn hole_to_object_and_at_sketch_points() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 40, "width": 30, "height": 20}));
    // A pocket from below: its ceiling is at z = 10.
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Pocket"}));
    run(&mut s, "sketch.rectangle.two_point", json!({"p0": [10, 10], "p1": [30, 20]}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.extrude", json!({"sketch": "Pocket", "distance": 10, "operation": "cut"}));
    let v0 = volume(&mut s);
    run(&mut s, "solid.hole", json!({"position": [20, 15, 20], "diameter": 4, "to": [25, 15, 10]}));
    assert!(rel(v0 - volume(&mut s), PI * 4.0 * 10.0) < 1e-3, "{}", v0 - volume(&mut s));
    run(&mut s, "edit.undo", json!({}));
    run(&mut s, "solid.hole", json!({"position": [5, 5, 20], "diameter": 4, "to": [5, 5, 12]}));
    assert!(rel(v0 - volume(&mut s), PI * 4.0 * 8.0) < 1e-3, "{}", v0 - volume(&mut s));
    // Every point of a sketch: three points (the rectangle's corners are not hole points).
    let mut t = Session::default();
    run(&mut t, "solid.box", json!({"length": 40, "width": 30, "height": 10}));
    run(&mut t, "sketch.create", json!({"plane": {"face": [20, 15, 10]}, "name": "Spots"}));
    for x in [10, 20, 30] {
        run(&mut t, "sketch.point", json!({"point": [x, 15]}));
    }
    run(&mut t, "sketch.rectangle.two_point", json!({"p0": [2, 2], "p1": [6, 6]}));
    run(&mut t, "sketch.finish", json!({}));
    let w0 = volume(&mut t);
    let r = run(&mut t, "solid.hole", json!({"sketch": "Spots", "diameter": 3}));
    assert!(rel(w0 - volume(&mut t), 3.0 * PI * 2.25 * 10.0) < 1e-3, "{r} {}", w0 - volume(&mut t));
}

/// Combine keeps its tools on request and can put its result in a new component.
#[test]
fn combine_keep_tools_and_new_component() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 20, "width": 20, "height": 20, "body_name": "A"}));
    run(&mut s, "solid.box", json!({"corner": [10, 0, 0], "length": 20, "width": 20, "height": 20, "body_name": "B"}));
    let r = run(&mut s, "solid.combine", json!({"target": "A", "tools": ["B"], "operation": "cut", "keep_tools": true, "new_component": true}));
    let comp = r["component"].as_u64().unwrap_or(0);
    assert!(comp > 0, "{r}");
    let st = s.model.state();
    assert!(st.body("B").is_some(), "the tool is kept");
    let a = st.body("A").unwrap();
    assert_eq!(s.doc.body_component("A", a.feature), comp);
    assert!(rel(solidcraft_kernel::measure(&a.body).unwrap().volume, 4000.0) < 1e-9);
}

/// Align with snap points: a box's vertex onto another's face centre, an edge midpoint onto a
/// hole's circle centre.
#[test]
fn align_with_snap_points() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 40, "width": 40, "height": 10, "body_name": "Base"}));
    run(&mut s, "solid.box", json!({"corner": [100, 0, 0], "length": 10, "width": 10, "height": 10, "body_name": "Cube"}));
    // Cube's corner (100, 0, 0) onto Base's top face centre (20, 20, 10).
    run(
        &mut s,
        "solid.align",
        json!({"bodies": ["Cube"], "from": {"snap": "vertex", "at": [100.4, 0.3, 0.2]}, "to": {"snap": "face_center", "at": [5, 5, 10]}}),
    );
    let m = run(&mut s, "inspect.measure", json!({"bodies": ["Cube"]}));
    let min = |i: usize| m["bodies"][0]["bbox"]["min"][i].as_f64().unwrap_or(f64::NAN);
    assert!((min(0) - 20.0).abs() < 1e-6 && (min(1) - 20.0).abs() < 1e-6 && (min(2) - 10.0).abs() < 1e-6, "{m}");
    assert!(s.execute("solid.align", &json!({"bodies": ["Cube"], "from": {"snap": "nope", "at": [0, 0, 0]}, "to": [0, 0, 0]})).is_err());
}

/// Boundary Fill: the lens two overlapping boxes share, and one box less the other.
#[test]
fn boundary_fill_cells() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 20, "width": 20, "height": 20, "body_name": "A"}));
    run(&mut s, "solid.box", json!({"corner": [10, 0, 0], "length": 20, "width": 20, "height": 20, "body_name": "B"}));
    let r = run(&mut s, "solid.boundary_fill", json!({"tools": ["A", "B"], "cells": [[15, 10, 10]], "remove_tools": true}));
    let m = run(&mut s, "inspect.measure", json!({}));
    assert_eq!(m["body_count"], 1, "{r} {m}");
    assert!(rel(m["total"]["volume_mm3"].as_f64().unwrap_or(0.0), 10.0 * 20.0 * 20.0) < 1e-9);
    run(&mut s, "edit.undo", json!({}));
    run(&mut s, "solid.boundary_fill", json!({"tools": ["A", "B"], "cells": [[5, 10, 10], [25, 10, 10]], "remove_tools": true}));
    let m = run(&mut s, "inspect.measure", json!({}));
    assert!(rel(m["total"]["volume_mm3"].as_f64().unwrap_or(0.0), 2.0 * 10.0 * 20.0 * 20.0) < 1e-9, "{m}");
    assert!(s.execute("solid.boundary_fill", &json!({"tools": ["A"], "cells": [[100, 100, 100]]})).is_err());
}

/// Rectangles name their sides and corners, so they can be dimensioned without inspecting the
/// sketch first, whichever way round the corners were given.
#[test]
fn rectangle_names_sides_and_corners() {
    let mut s = Session::default();
    run(&mut s, "sketch.create", json!({"plane": "XY"}));
    let r = run(&mut s, "sketch.rectangle.two_point", json!({"p0": [40, 30], "p1": [0, 0]}));
    let pos = |s: &Session, id: &Value| {
        let sk = s.doc.sketch(s.active_sketch.unwrap()).unwrap();
        sk.point(sk.resolve_point(id.as_str().unwrap()).unwrap()).unwrap()
    };
    for (corner, at) in [("bottom_left", [0.0, 0.0]), ("bottom_right", [40.0, 0.0]), ("top_right", [40.0, 30.0]), ("top_left", [0.0, 30.0])] {
        let p = pos(&s, &r["corners"][corner]);
        assert!((p.x - at[0]).abs() < 1e-9 && (p.y - at[1]).abs() < 1e-9, "{corner}: {p:?} in {r}");
    }
    for (side, from, to) in [
        ("bottom", "bottom_left", "bottom_right"),
        ("right", "bottom_right", "top_right"),
        ("top", "top_right", "top_left"),
        ("left", "top_left", "bottom_left"),
    ] {
        let line = r["sides"][side].as_str().unwrap();
        assert!(r["curves"].as_array().unwrap().iter().any(|c| c == line), "{r}");
        assert_eq!(pos(&s, &json!(format!("{line}.start"))), pos(&s, &r["corners"][from]), "{side} in {r}");
        assert_eq!(pos(&s, &json!(format!("{line}.end"))), pos(&s, &r["corners"][to]), "{side} in {r}");
    }
    run(&mut s, "sketch.constraint.coincident", json!({"a": r["corners"]["bottom_left"], "b": "origin"}));
    run(&mut s, "sketch.dimension", json!({"entities": [r["sides"]["bottom"]], "value": 50}));
    run(&mut s, "sketch.dimension", json!({"entities": [r["sides"]["left"]], "value": 20}));
    let si = run(&mut s, "sketch.inspect", json!({}));
    assert_eq!(si["dof"], 0, "{si}");
    let p = pos(&s, &r["corners"]["top_right"]);
    assert!((p.x - 50.0).abs() < 1e-6 && (p.y - 20.0).abs() < 1e-6, "{p:?}");

    let c = run(&mut s, "sketch.rectangle.center", json!({"center": [100, 0], "corner": [90, 5]}));
    let p = pos(&s, &c["corners"]["bottom_left"]);
    assert!((p.x - 90.0).abs() < 1e-9 && (p.y + 5.0).abs() < 1e-9, "{c}");
    assert_eq!(pos(&s, &json!(format!("{}.start", c["sides"]["top"].as_str().unwrap()))), pos(&s, &c["corners"]["top_right"]), "{c}");
}

/// Points drawn exactly onto an existing point (the origin, a corner) are connected to it;
/// `connect: false` leaves them free, and tying them again by hand is a no-op, not a conflict.
#[test]
fn drawing_on_existing_points_connects_to_them() {
    let mut s = Session::default();
    run(&mut s, "sketch.create", json!({"plane": "XY"}));
    let n = |v: &Value| v["constraints"].as_array().map_or(0, Vec::len);
    // Horizontal/vertical x4, plus the corner on the origin.
    let r = run(&mut s, "sketch.rectangle.two_point", json!({"p0": [0, 0], "p1": [40, 30]}));
    assert_eq!(n(&r), 5, "{r}");
    run(&mut s, "sketch.dimension", json!({"entities": [r["curves"][0]], "value": 40}));
    run(&mut s, "sketch.dimension", json!({"entities": [r["curves"][1]], "value": 30}));
    let si = run(&mut s, "sketch.inspect", json!({}));
    assert_eq!(si["dof"], 0, "fully constrained without a hand-made coincident: {si}");
    let again = run(&mut s, "sketch.constraint.coincident", json!({"a": "origin", "b": "l1.start"}));
    assert_eq!(again["existing"], true, "{again}");

    // A line from a corner, a circle on a corner and a point on the origin connect too.
    assert_eq!(n(&run(&mut s, "sketch.line", json!({"points": [[40, 30], [60, 30]]}))), 1);
    assert_eq!(n(&run(&mut s, "sketch.circle.center", json!({"center": [0, 30], "radius": 3}))), 1);
    assert_eq!(n(&run(&mut s, "sketch.point", json!({"point": [0, 0]}))), 1);
    // Close is not on it, and `connect: false` opts out.
    assert_eq!(n(&run(&mut s, "sketch.point", json!({"point": [40.001, 0]}))), 0);
    assert_eq!(n(&run(&mut s, "sketch.line", json!({"points": [[0, 0], [0, -10]], "connect": false}))), 0);
    let si = run(&mut s, "sketch.inspect", json!({}));
    assert_ne!(si["status"], "conflict", "{si}");
}

#[test]
fn computed_import_preserves_file_and_earlier_undo_history() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10}));
    s.path = Some("existing.solidcraft".into());
    s.mark_saved();
    let saved = s.doc.clone();
    let mut computed = s.scratch();
    run(&mut computed, "solid.box", json!({"length": 5, "width": 5, "height": 5, "corner": [30, 0, 0]}));
    s.accept_computed(computed);
    assert_eq!(s.path.as_deref(), Some("existing.solidcraft"));
    assert!(s.is_dirty());
    assert_eq!(s.model.state().bodies.len(), 2);
    run(&mut s, "edit.undo", json!({}));
    assert!(std::sync::Arc::ptr_eq(&saved, &s.doc));
    assert!(!s.is_dirty());
    run(&mut s, "edit.undo", json!({}));
    assert!(s.model.state().bodies.is_empty());
}

#[test]
fn adopting_a_computed_component_keeps_its_warm_world_geometry() {
    let mut s = Session::default();
    run(&mut s, "component.create", json!({"name": "Part"}));
    run(&mut s, "solid.box", json!({"length":10,"width":10,"height":10}));
    let mut computed = s.scratch();
    run(&mut computed, "solid.box", json!({"length":5,"width":5,"height":5,"corner":[30,0,0]}));
    let prepared = computed.world_state();
    for b in &prepared.bodies {
        let _ = b.mesh();
    }
    s.accept_computed(computed);
    assert!(std::sync::Arc::ptr_eq(&prepared, &s.world_state()));
    s.invalidate_view(100);
    assert!(std::sync::Arc::ptr_eq(&prepared, &s.world_state()));
}

/// Every file command checked the size with the metadata and then read to the end. A character
/// device reports a length of 0, so the cap passed and the read never finished; a FIFO blocked
/// the calling thread, which over the control channel is the thread drawing the window.
#[cfg(unix)]
#[test]
fn special_files_are_refused_by_every_file_command() {
    let mut s = Session::default();
    for cmd in ["doc.open", "parameters.import"] {
        let r = s.execute(cmd, &json!({"path": "/dev/zero"}));
        assert!(r.as_ref().is_err_and(|e| e.to_string().contains("not a regular file")), "{cmd}: {r:?}");
    }
    // The commands that check the extension first need a special file with the right name.
    let dir = std::env::temp_dir().join(format!("solidcraft-special-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    for (cmd, name) in [("file.insert_step", "x.step"), ("file.insert_mesh", "x.3mf"), ("sketch.insert_dxf", "x.dxf")] {
        let p = dir.join(name);
        let _ = std::fs::remove_file(&p);
        if !std::process::Command::new("mkfifo").arg(&p).status().map(|st| st.success()).unwrap_or(false) {
            continue;
        }
        let r = s.execute(cmd, &json!({"path": p.to_string_lossy()}));
        assert!(r.as_ref().is_err_and(|e| e.to_string().contains("not a regular file")), "{cmd}: {r:?}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// A cut targeted at one body leaves a non-target body alone, even one the tool covers completely (#260).
#[test]
fn targeted_cut_keeps_a_covered_body_that_is_not_a_target() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10, "operation": "new_body", "body_name": "Frame"}));
    run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10, "corner": [20, 0, 0], "operation": "new_body", "body_name": "Other"}));
    // Exactly Other's extent; it does not touch Frame.
    run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10, "corner": [20, 0, 0], "operation": "cut", "targets": ["Frame"]}));
    let names: Vec<String> = s.model.state().bodies.iter().map(|b| b.name.clone()).collect();
    assert_eq!(names, ["Frame", "Other"]);
    assert!(rel(volume(&mut s), 2000.0) < 1e-6);
}

/// A frame's opening cut over a separate mesh body that sits in it: the mesh survives (#260).
#[test]
fn targeted_cut_keeps_an_overlapping_body_that_is_not_a_target() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 50, "width": 50, "height": 5, "operation": "new_body", "body_name": "Frame"}));
    run(&mut s, "solid.box", json!({"length": 30, "width": 30, "height": 1, "corner": [10, 10, 2], "operation": "new_body", "body_name": "Mesh"}));
    run(&mut s, "solid.box", json!({"length": 30, "width": 30, "height": 5, "corner": [10, 10, 0], "operation": "cut", "targets": ["Frame"]}));
    let names: Vec<String> = s.model.state().bodies.iter().map(|b| b.name.clone()).collect();
    assert_eq!(names, ["Frame", "Mesh"]);
    assert!(rel(volume(&mut s), 50.0 * 50.0 * 5.0 - 30.0 * 30.0 * 5.0 + 900.0) < 1e-6);
}

/// A join with explicit `targets` merges into those bodies only; a touched non-target stays (#259).
#[test]
fn join_with_targets_leaves_other_touched_bodies_alone() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10, "operation": "new_body", "body_name": "A"}));
    run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10, "corner": [10, 0, 0], "operation": "new_body", "body_name": "B"}));
    // Straddles the face between A and B; only A is a target.
    run(&mut s, "solid.box", json!({"length": 4, "width": 4, "height": 4, "corner": [8, 3, 3], "operation": "join", "targets": ["A"]}));
    let names: Vec<String> = s.model.state().bodies.iter().map(|b| b.name.clone()).collect();
    assert_eq!(names, ["A", "B"], "B is not a target and must stay a body");
    let b = run(&mut s, "inspect.measure", json!({"bodies": ["B"]}))["total"]["volume_mm3"].as_f64().unwrap();
    assert!(rel(b, 1000.0) < 1e-6, "B is unchanged: {b}");
    // A gains the half of the tool outside it.
    let a = run(&mut s, "inspect.measure", json!({"bodies": ["A"]}))["total"]["volume_mm3"].as_f64().unwrap();
    assert!(rel(a, 1032.0) < 1e-6, "A: {a}");
}

/// Deleting a sketch takes its extrude with it; a later combine that names the extrude's body must
/// not be left failing silently: it is deleted too, or the reply reports it (#267).
#[test]
fn deleting_a_sketch_leaves_no_silently_failing_feature() {
    let mut s = Session::default();
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "S"}));
    run(&mut s, "sketch.rectangle.two_point", json!({"p0": [0, 0], "p1": [20, 20]}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.extrude", json!({"sketch": "S", "distance": "10", "body_name": "P"}));
    run(&mut s, "solid.box", json!({"length": 5, "width": 5, "height": 5, "corner": [18, 5, 0], "operation": "new_body", "body_name": "Q"}));
    run(&mut s, "solid.combine", json!({"target": "P", "tools": ["Q"], "name": "JoinPQ"}));
    let reply = run(&mut s, "timeline.delete", json!({"features": ["S"]}));
    if let Ok(join) = s.execute("timeline.inspect", &json!({"feature": "JoinPQ"})) {
        assert!(
            join["error"].is_null() || reply.to_string().contains("JoinPQ"),
            "JoinPQ now fails ({}) but the reply only says {reply}",
            join["error"]
        );
    }
}

/// Every body a multi-profile `new` extrude creates carries its `body_name`, not a generic BodyN (#266).
#[test]
fn multi_profile_extrude_names_every_body_after_body_name() {
    let mut s = Session::default();
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "R"}));
    for x in [0, 10, 20] {
        run(&mut s, "sketch.rectangle.two_point", json!({"p0": [x, 0], "p1": [x + 5, 30]}));
    }
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.extrude", json!({"distance": "10", "operation": "new_body", "body_name": "Ribs"}));
    let names: Vec<String> = s.model.state().bodies.iter().map(|b| b.name.clone()).collect();
    assert!(names.iter().all(|n| n.starts_with("Ribs")), "{names:?}");
    assert!(rel(volume(&mut s), 4500.0) < 1e-6);
}

/// Renaming one pattern copy leaves the other copies' names alone (#265).
#[test]
fn renaming_a_pattern_copy_keeps_the_other_copies_names() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10, "operation": "new_body", "body_name": "X"}));
    run(&mut s, "solid.pattern.rectangular", json!({"bodies": ["X"], "dir1": [1, 0, 0], "count1": 3, "spacing1": 20}));
    run(&mut s, "body.rename", json!({"body": "X (2)", "name": "Second"}));
    let third = run(&mut s, "inspect.measure", json!({"bodies": ["X (3)"]}));
    assert_eq!(third["bodies"][0]["bbox"]["min"][0].as_f64().map(f64::round), Some(40.0), "X (3) is still the copy at x = 40: {third}");
    run(&mut s, "body.rename", json!({"body": "X (3)", "name": "Third"}));
    let names: Vec<String> = s.model.state().bodies.iter().map(|b| b.name.clone()).collect();
    assert_eq!(names, ["X", "Second", "Third"]);
}

/// Three solids in one STEP file that carry the same label: the file written for three boxes, each
/// named `X`.
fn step_three_solids_labelled_x(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("solidcraft-step-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut s = Session::default();
    for (i, label) in ["Aa", "Bb", "Cc"].iter().enumerate() {
        let corner = [30.0 * i as f64, 0.0, 0.0];
        run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10, "corner": corner, "operation": "new_body", "body_name": label}));
    }
    let p = dir.join("Three.step");
    run(&mut s, "file.export", json!({"path": p.to_string_lossy()}));
    let mut text = std::fs::read_to_string(&p).unwrap();
    for label in ["Aa", "Bb", "Cc"] {
        assert!(text.contains(&format!("MANIFOLD_SOLID_BREP('{label}'")), "the file names its solids: {label}");
        text = text.replace(&format!("MANIFOLD_SOLID_BREP('{label}'"), "MANIFOLD_SOLID_BREP('X'");
    }
    std::fs::write(&p, text).unwrap();
    p
}

/// Renaming one of several imported bodies that share a label leaves the names of the others alone:
/// the model lists them as `X`, `X (2)`, `X (3)`, and `X (3)` is still there after `X (2)` is renamed.
#[test]
fn renaming_an_imported_body_keeps_the_names_of_its_duplicates() {
    let p = step_three_solids_labelled_x("duplicate-labels");
    let mut s = Session::default();
    run(&mut s, "file.insert_step", json!({"path": p.to_string_lossy()}));
    let names = |s: &Session| s.model.state().bodies.iter().map(|b| b.name.clone()).collect::<Vec<_>>();
    assert_eq!(names(&s), ["X", "X (2)", "X (3)"]);
    let x_of =
        |s: &mut Session, body: &str| run(s, "inspect.measure", json!({"bodies": [body]}))["bodies"][0]["bbox"]["min"][0].as_f64().map(f64::round);
    assert_eq!(x_of(&mut s, "X (3)"), Some(60.0));
    run(&mut s, "body.rename", json!({"body": "X (2)", "name": "Second"}));
    assert_eq!(names(&s), ["X", "Second", "X (3)"]);
    assert_eq!(x_of(&mut s, "X (3)"), Some(60.0), "X (3) is still the solid at x = 60");
    run(&mut s, "body.rename", json!({"body": "X (3)", "name": "Third"}));
    assert_eq!(names(&s), ["X", "Second", "Third"]);
    assert_eq!(x_of(&mut s, "Third"), Some(60.0));
    // The names survive a rebuild and a reopen of the saved design.
    let design = p.with_file_name("Three.solidcraft");
    run(&mut s, "file.save_as", json!({"path": design.to_string_lossy()}));
    let mut r = Session::default();
    run(&mut r, "doc.open", json!({"path": design.to_string_lossy()}));
    assert_eq!(names(&r), ["X", "Second", "Third"]);
}

/// The same when the first of the three is renamed: `X (2)` and `X (3)` do not slide down to `X` and
/// `X (2)`, and what is set on one of them (a material) stays with it.
#[test]
fn renaming_the_first_imported_duplicate_keeps_the_numbers_of_the_others() {
    let p = step_three_solids_labelled_x("duplicate-labels-first");
    let mut s = Session::default();
    run(&mut s, "file.insert_step", json!({"path": p.to_string_lossy()}));
    run(&mut s, "material.assign", json!({"bodies": ["X (3)"], "material": "Steel"}));
    run(&mut s, "body.rename", json!({"body": "X", "name": "First"}));
    let names: Vec<String> = s.model.state().bodies.iter().map(|b| b.name.clone()).collect();
    assert_eq!(names, ["First", "X (2)", "X (3)"]);
    assert!(s.doc.materials.contains_key("X (3)"), "the material is still on X (3): {:?}", s.doc.materials.keys().collect::<Vec<_>>());
}

/// Parameters that work are listed in the command's params doc, which is all agents see (#271).
#[test]
fn params_docs_list_working_parameters() {
    let mut s = Session::default();
    let cmds = run(&mut s, "engine.commands", json!({}));
    let params =
        |id: &str| cmds.as_array().unwrap().iter().find(|c| c["id"] == id).and_then(|c| c["params"].as_str()).unwrap_or_default().to_string();
    // Both work today: a named base flange, and a loft cut limited to its targets.
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "A"}));
    run(&mut s, "sketch.rectangle.two_point", json!({"p0": [0, 0], "p1": [50, 50]}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "sheet.flange", json!({"sketch": "A", "body_name": "Panel A"}));
    assert!(s.model.state().body("Panel A").is_some());
    assert!(params("sheet.flange").contains("body_name"), "sheet.flange: {}", params("sheet.flange"));
    assert!(params("solid.loft").contains("targets"), "solid.loft: {}", params("solid.loft"));
}

/// Every body-creating feature documents the keys `add_feature` reads for all of them (`name`,
/// `body_name`), and those that take `targets` say so (#271).
#[test]
fn feature_params_docs_list_shared_keys() {
    let mut s = Session::default();
    let cmds = run(&mut s, "engine.commands", json!({}));
    let params =
        |id: &str| cmds.as_array().unwrap().iter().find(|c| c["id"] == id).and_then(|c| c["params"].as_str()).unwrap_or_default().to_string();
    let with_targets = ["solid.extrude", "solid.revolve", "solid.sweep", "solid.loft", "solid.pipe", "solid.coil"];
    let without_targets = ["solid.box", "solid.cylinder", "solid.sphere", "solid.torus", "sheet.flange"];
    for id in with_targets.iter().chain(&without_targets) {
        let doc = params(id);
        assert!(doc.contains("name?") && doc.contains("body_name?"), "{id}: {doc}");
    }
    for id in with_targets {
        assert!(params(id).contains("targets?"), "{id}: {}", params(id));
    }
}

#[test]
fn a_pattern_repeats_a_fillet_on_the_edges_it_found() {
    // #219: a filleted pocket patterned along the plate, its height from a parameter. A taller
    // pocket moves its far corner edges away from the points picked for the fillet; the pattern
    // repeats the edges the fillet found (by name), not those points.
    let mut s = Session::default();
    run(&mut s, "parameters.add", json!({"name": "n", "expression": "14", "unit": ""}));
    run(&mut s, "parameters.add", json!({"name": "h", "expression": "(419.1 mm - 3.175 mm - (n - 1) * 3.175 mm) / n", "unit": "mm"}));
    run(&mut s, "sketch.create", json!({"plane": "XY"}));
    run(&mut s, "sketch.rectangle.two_point", json!({"p0": [0, 0], "p1": [150, 422.275]}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.extrude", json!({"distance": 19.05, "name": "Plate"}));
    run(&mut s, "construct.plane.offset", json!({"base": "XY", "offset": 19.05, "name": "PocketPlane"}));
    run(&mut s, "sketch.create", json!({"plane": "PocketPlane"}));
    run(&mut s, "sketch.rectangle.two_point", json!({"p0": [3.175, 3.175], "p1": [146.825, 29.9357]}));
    run(&mut s, "sketch.dimension", json!({"entities": ["origin", "p1"], "type": "horizontal", "value": "3.175"}));
    run(&mut s, "sketch.dimension", json!({"entities": ["origin", "p1"], "type": "vertical", "value": "3.175"}));
    run(&mut s, "sketch.dimension", json!({"entities": ["l1"], "type": "length", "value": "143.65"}));
    run(&mut s, "sketch.dimension", json!({"entities": ["l4"], "type": "length", "value": "h"}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.extrude", json!({"distance": 12.7, "direction": "negative", "operation": "cut", "name": "Pocket"}));
    run(
        &mut s,
        "solid.fillet",
        json!({"edges": [[3.175, 3.175, 12.700000000000001], [146.825, 3.175, 12.700000000000001], [146.825, 29.9357, 12.700000000000001], [3.175, 29.9357, 12.700000000000001]], "radius": 3.2}),
    );
    run(
        &mut s,
        "solid.pattern.rectangular",
        json!({"features": ["Pocket", "Fillet1"], "dir1": [0, 1, 0], "count1": "n", "spacing1": "h + 3.175 mm"}),
    );
    let r = run(&mut s, "parameters.change", json!({"name": "n", "expression": "10"}));
    assert_eq!(r["errors"].as_array().map(Vec::len), Some(0), "{r}");
    let (n, h) = (10.0, (419.1 - 3.175 - 9.0 * 3.175) / 10.0);
    let pocket = 143.65 * h * 12.7 - 4.0 * (1.0 - PI / 4.0) * 3.2 * 3.2 * 12.7;
    let expect = 150.0 * 422.275 * 19.05 - n * pocket;
    assert!(rel(volume(&mut s), expect) < 1e-5, "{} vs {expect}", volume(&mut s));
}

/// A copy added by raising the count after a rename takes the next number, not the freed one (#372).
#[test]
fn raising_a_pattern_count_after_a_rename_numbers_the_new_copy_after_the_others() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10, "operation": "new_body", "body_name": "X"}));
    run(&mut s, "solid.pattern.rectangular", json!({"bodies": ["X"], "dir1": [1, 0, 0], "count1": 3, "spacing1": 20, "name": "Pat"}));
    run(&mut s, "body.rename", json!({"body": "X (2)", "name": "Second"}));
    run(&mut s, "timeline.edit", json!({"feature": "Pat", "set": {"pattern": {"count1": "4"}}}));
    let names: Vec<String> = s.model.state().bodies.iter().map(|b| b.name.clone()).collect();
    assert_eq!(names, ["X", "Second", "X (3)", "X (4)"]);
    let fourth = run(&mut s, "inspect.measure", json!({"bodies": ["X (4)"]}));
    assert_eq!(fourth["bodies"][0]["bbox"]["min"][0].as_f64().map(f64::round), Some(60.0), "{fourth}");
}

/// A profile added to the sketch after a named multi-profile extrude is named after `body_name`
/// too (#373).
#[test]
fn a_profile_added_later_is_named_after_body_name() {
    let mut s = Session::default();
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "R"}));
    for x in [0, 10, 20] {
        run(&mut s, "sketch.rectangle.two_point", json!({"p0": [x, 0], "p1": [x + 5, 30]}));
    }
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.extrude", json!({"sketch": "R", "distance": "10", "operation": "new_body", "body_name": "Ribs"}));
    run(&mut s, "sketch.edit", json!({"sketch": "R"}));
    run(&mut s, "sketch.rectangle.two_point", json!({"p0": [30, 0], "p1": [35, 30]}));
    run(&mut s, "sketch.finish", json!({}));
    let names: Vec<String> = s.model.state().bodies.iter().map(|b| b.name.clone()).collect();
    assert_eq!(names, ["Ribs", "Ribs (2)", "Ribs (3)", "Ribs (4)"]);
}

/// The reply names the bodies the feature created, apart from the ones already there (#374).
#[test]
fn extrude_reply_lists_the_bodies_it_created() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 5, "width": 5, "height": 5, "corner": [0, 50, 0], "operation": "new_body", "body_name": "Base"}));
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "R"}));
    for x in [0, 10, 20] {
        run(&mut s, "sketch.rectangle.two_point", json!({"p0": [x, 0], "p1": [x + 5, 30]}));
    }
    run(&mut s, "sketch.finish", json!({}));
    let r = run(&mut s, "solid.extrude", json!({"sketch": "R", "distance": "10", "operation": "new_body", "body_name": "Ribs"}));
    assert_eq!(r["created_bodies"], json!(["Ribs", "Ribs (2)", "Ribs (3)"]), "{r}");
}

/// A design saved before format 14 names the bodies past its stored names `BodyN`, as it did when
/// it was saved, so later features that name them still find them (#373).
#[test]
fn a_design_saved_before_format_14_keeps_generic_names_past_body_name() {
    let mut s = Session::default();
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "R"}));
    for x in [0, 10, 20] {
        run(&mut s, "sketch.rectangle.two_point", json!({"p0": [x, 0], "p1": [x + 5, 30]}));
    }
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.extrude", json!({"sketch": "R", "distance": "10", "operation": "new_body", "body_name": "Ribs"}));
    let mut legacy: Value = serde_json::from_str(&s.doc.to_json()).unwrap();
    legacy["format"] = json!("solidcraft/13");
    let extrude = legacy["features"].as_array_mut().unwrap().iter_mut().find(|f| f["type"] == "extrude").unwrap();
    extrude["body_names"] = json!(["Ribs"]);
    let dir = std::env::temp_dir().join(format!("solidcraft-373-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let design = dir.join("ribs.solidcraft");
    std::fs::write(&design, legacy.to_string()).unwrap();
    let mut old = Session::default();
    run(&mut old, "doc.open", json!({"path": design.to_string_lossy()}));
    let names: Vec<String> = old.model.state().bodies.iter().map(|b| b.name.clone()).collect();
    assert_eq!(names, ["Ribs", "Body1", "Body2"]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn fillet_fit_reduce_uses_the_largest_radius_that_fits() {
    // #293: 3.5 is too large for the 6 mm end faces of a 14 x 6 x 10 box; reduce gives 3 (a full
    // round), says so, and refits when the box changes.
    let mut s = Session::default();
    run(&mut s, "parameters.add", json!({"name": "w", "expression": "6 mm"}));
    run(&mut s, "solid.box", json!({"length": 14, "width": "w", "height": 10, "corner": [14, 0, 0]}));
    let mids = json!([[14, 0, 5], [28, 0, 5], [14, 6, 5], [28, 6, 5]]);
    assert!(s.execute("solid.fillet", &json!({"edges": mids, "radius": 3.5})).is_err());
    let out = run(&mut s, "solid.fillet", json!({"edges": mids, "radius": 3.5, "fit": "reduce", "name": "Round"}));
    assert_eq!(out["radius_applied"].as_f64(), Some(3.0), "{out}");
    let full = 840.0 - (4.0 - PI) * 9.0 * 10.0;
    assert!(rel(volume(&mut s), full) < 2e-4, "{} vs {full}", volume(&mut s));
    let id = out["feature"].as_u64().unwrap();
    let warning = s.model.result(id).and_then(|r| r.warning.clone()).unwrap_or_default();
    assert!(warning.contains("reduced to 3 mm"), "{warning}");
    // Wider: the requested 3.5 fits, nothing is reduced.
    let r = run(&mut s, "parameters.change", json!({"name": "w", "expression": "8 mm"}));
    assert_eq!(r["errors"].as_array().map(Vec::len), Some(0), "{r}");
    assert_eq!(s.model.result(id).and_then(|r| r.state.fitted_radius(id)), None);
    assert!(s.model.result(id).and_then(|r| r.warning.clone()).is_none());
    // Saved only when set.
    let json = serde_json::to_string(&s.doc.features).unwrap();
    assert_eq!(json.matches("\"fit\":\"reduce\"").count(), 1, "{json}");
    // An unknown mode is refused, and so are reduce and taper with a radius that is not constant.
    assert!(s.execute("solid.fillet", &json!({"edges": mids, "radius": 1, "fit": "stretch"})).is_err());
    assert!(s.execute("solid.fillet", &json!({"edges": mids, "radius": 1, "fit": "reduce", "type": "chord"})).is_err());
    assert!(s.execute("solid.fillet", &json!({"edges": mids, "radius": 1, "fit": "taper", "type": "chord"})).is_err());
}

#[test]
fn fillet_fit_taper_follows_the_room_along_the_edge() {
    // #293: the top front edge of a plate whose top narrows from 20 to `tip` mm along 40 mm.
    // At a 2 mm tip it can't take 3 mm; taper keeps 3 where it fits and comes down to 0.9 of the
    // 2 mm there. With an 8 mm tip 3 fits and stays constant; a plate closing to a point
    // tapers to 0. The warning says what was done.
    let (k, edge) = (1.0 - PI / 4.0, json!([[20, 0, 15]]));
    let plate = |tip: f64| {
        let mut s = Session::default();
        run(&mut s, "sketch.create", json!({"plane": "XY"}));
        let pts = if tip > 0.0 { json!([[0, 0], [40, 0], [40, tip], [0, 20]]) } else { json!([[0, 0], [40, 0], [0, 20]]) };
        run(&mut s, "sketch.line", json!({"points": pts, "closed": true}));
        run(&mut s, "solid.extrude", json!({"distance": 15}));
        s
    };
    let warning = |s: &Session, out: &Value| out["feature"].as_u64().and_then(|id| s.model.result(id)).and_then(|r| r.warning.clone());

    let mut s = plate(2.0);
    assert!(s.execute("solid.fillet", &json!({"edges": edge, "radius": 3})).is_err());
    let out = run(&mut s, "solid.fillet", json!({"edges": edge, "radius": 3, "fit": "taper"}));
    let w = warning(&s, &out).unwrap_or_default();
    assert!(w.contains("tapers to the room on 1 of 1 edges, down to 1.8 mm"), "{w}");
    // Between the material a constant 1.8 and a constant 3 would remove.
    let (v, full) = (volume(&mut s), 40.0 * 22.0 / 2.0 * 15.0);
    assert!(v < full - k * 1.8 * 1.8 * 40.0 && v > full - k * 9.0 * 40.0, "{v}");
    let json = serde_json::to_string(&s.doc.features).unwrap();
    assert_eq!(json.matches("\"fit\":\"taper\"").count(), 1, "{json}");

    let mut s = plate(8.0);
    let out = run(&mut s, "solid.fillet", json!({"edges": edge, "radius": 3, "fit": "taper"}));
    assert_eq!(warning(&s, &out), None);
    assert!(rel(volume(&mut s), 40.0 * 28.0 / 2.0 * 15.0 - k * 9.0 * 40.0) < 2e-4);

    let mut s = plate(0.0);
    let out = run(&mut s, "solid.fillet", json!({"edges": edge, "radius": 16, "fit": "taper"}));
    let w = warning(&s, &out).unwrap_or_default();
    assert!(w.contains("on 1 of 1 edges, down to 0 mm"), "{w}");
}

#[test]
fn spline_fillet_takes_over_a_taper() {
    // A tapered fillet keeps each edge's radius spline; given back as a `spline` fillet it makes
    // the same part, and its control radii are then the user's to change.
    let mut s = Session::default();
    run(&mut s, "sketch.create", json!({"plane": "XY"}));
    run(&mut s, "sketch.line", json!({"points": [[0, 0], [40, 0], [40, 2], [0, 20]], "closed": true}));
    run(&mut s, "solid.extrude", json!({"distance": 15}));
    let edge = json!([[20, 0, 15]]);
    let out = run(&mut s, "solid.fillet", json!({"edges": edge, "radius": 3, "fit": "taper"}));
    let id = out["feature"].as_u64().unwrap();
    let tapered = s.model.result(id).and_then(|r| r.state.tapered(id).map(<[_]>::to_vec)).unwrap();
    let [(_, t)] = &tapered[..] else { panic!("{tapered:?}") };
    let tapered_volume = volume(&mut s);
    let radii: Vec<Value> = t.spline.radii.iter().skip(1).map(|r| json!(r)).collect();
    let spline = json!({"edges": edge, "radius": t.spline.radii[0], "type": "spline", "start": [t.start.x, t.start.y, t.start.z], "knots": t.spline.knots, "radii": radii});
    run(&mut s, "timeline.redefine", json!({"feature": id, "command": "solid.fillet", "params": spline}));
    assert!(rel(volume(&mut s), tapered_volume) < 1e-9, "{} vs {tapered_volume}", volume(&mut s));
    let json = serde_json::to_string(&s.doc.features).unwrap();
    assert!(json.contains("\"type\":\"spline\""), "{json}");
    // A control radius raised in the middle: more material goes.
    let mut raised = spline.clone();
    raised["radii"][2] = json!("4 mm");
    run(&mut s, "timeline.redefine", json!({"feature": id, "command": "solid.fillet", "params": raised}));
    assert!(volume(&mut s) < tapered_volume);
    // Knots that don't match the radii are refused.
    let mut bad = spline;
    bad["knots"] = json!([0, 1]);
    assert!(s.execute("solid.fillet", &bad).is_err());
}

#[test]
fn fillet_fit_reduce_keeps_the_radius_where_it_fits() {
    // A 2 mm fin on a block: its top edge can't take 4, a corner of the block far from it can.
    // Only the fin's edge is reduced; the reply gives each edge's radius.
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 40, "width": 30, "height": 20}));
    run(&mut s, "solid.box", json!({"length": 2, "width": 30, "height": 10, "corner": [19, 0, 20], "operation": "join"}));
    assert_eq!(s.model.state().bodies.len(), 1);
    let edges = json!([[21, 15, 30], [40, 0, 10]]);
    let out = run(&mut s, "solid.fillet", json!({"edges": edges, "radius": 4, "fit": "reduce"}));
    let fin = out["radius_applied"].as_f64().unwrap();
    assert!(fin < 4.0, "{out}");
    let each: Vec<(Vec<f64>, f64)> = out["radii_applied"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| (x["edge"].as_array().unwrap().iter().filter_map(Value::as_f64).collect(), x["radius"].as_f64().unwrap()))
        .collect();
    assert_eq!(each, [(vec![21.0, 15.0, 30.0], fin), (vec![40.0, 0.0, 10.0], 4.0)], "{out}");
    let removed = (1.0 - PI / 4.0) * (fin * fin * 30.0 + 16.0 * 20.0);
    let want = 40.0 * 30.0 * 20.0 + 2.0 * 30.0 * 10.0 - removed;
    assert!(rel(volume(&mut s), want) < 2e-4, "{} vs {want}", volume(&mut s));
    let id = out["feature"].as_u64().unwrap();
    let warning = s.model.result(id).and_then(|r| r.warning.clone()).unwrap_or_default();
    assert!(warning.contains("on 1 of 2 edges"), "{warning}");
}

#[test]
fn a_pattern_of_a_reduced_fillet_rounds_each_copy_alike() {
    // #293: a 6 mm wide post on a plate, its four vertical edges at 3.5 reduced to 3, patterned
    // three times: every copy is a full round.
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 60, "width": 20, "height": 2}));
    run(&mut s, "solid.box", json!({"length": 14, "width": 6, "height": 10, "corner": [2, 7, 2], "operation": "join", "name": "Post"}));
    let mids = json!([[2, 7, 7], [16, 7, 7], [2, 13, 7], [16, 13, 7]]);
    run(&mut s, "solid.fillet", json!({"edges": mids, "radius": 3.5, "fit": "reduce", "name": "Round"}));
    run(&mut s, "solid.pattern.rectangular", json!({"features": ["Post", "Round"], "dir1": [1, 0, 0], "count1": 3, "spacing1": 20}));
    let expect = 60.0 * 20.0 * 2.0 + 3.0 * (840.0 - (4.0 - PI) * 9.0 * 10.0);
    assert!(rel(volume(&mut s), expect) < 2e-4, "{} vs {expect}", volume(&mut s));
}

/// A taper on a profile of lines and an arc (#546). The D profile (back 9.2, sides 7.4, a
/// half-round front R 4.6 tangent to them) grows by d = z · tan 5° with mitred corners:
/// A(d) = A₀ + P·d + (2 + π/2)·d², integrated over the 10 mm height.
#[test]
fn tapered_extrude_of_a_profile_mixing_lines_and_an_arc() {
    let mut s = Session::default();
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "D"}));
    run(&mut s, "sketch.line", json!({"points": [[0, 4.6], [-7.4, 4.6], [-7.4, -4.6], [0, -4.6]], "closed": false}));
    run(&mut s, "sketch.arc.center_point", json!({"center": [0, 0], "start": [0, -4.6], "end": [0, 4.6]}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.extrude", json!({"sketch": "D", "distance": 10, "taper": "5 deg"}));
    let (t, h) = (5f64.to_radians().tan(), 10.0);
    let (a0, p, k) = (7.4 * 9.2 + PI * 4.6 * 4.6 / 2.0, 2.0 * 7.4 + 9.2 + PI * 4.6, 2.0 + PI / 2.0);
    let expect = a0 * h + p * t * h * h / 2.0 + k * t * t * h.powi(3) / 3.0;
    assert!(rel(volume(&mut s), expect) < 1e-3, "{} vs {expect}", volume(&mut s));
}

/// A feature that joins into a body creates none, and its reply says so; a primitive's reply
/// names the body it made (#374).
#[test]
fn feature_replies_list_only_the_bodies_they_created() {
    let mut s = Session::default();
    let r = run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10, "body_name": "Base"}));
    assert_eq!(r["created_bodies"], json!(["Base"]), "{r}");
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Boss"}));
    run(&mut s, "sketch.circle.center", json!({"center": [5, 5], "radius": 2}));
    run(&mut s, "sketch.finish", json!({}));
    let r = run(&mut s, "solid.extrude", json!({"sketch": "Boss", "distance": "15", "operation": "join"}));
    assert_eq!(r["created_bodies"], json!([]), "{r}");
    assert_eq!(r["bodies"], json!(["Base"]), "{r}");
}

/// A feature reply also says which existing bodies it changed and which are gone, so a client
/// that skips the full body list (MCP `batch` with `verbose: false`, #530) still sees every effect.
#[test]
fn feature_replies_list_the_bodies_they_changed_and_removed() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10, "body_name": "A"}));
    run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10, "corner": [20, 0, 0], "operation": "new", "body_name": "B"}));
    run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10, "corner": [40, 0, 0], "operation": "new", "body_name": "C"}));
    // A pocket in A changes A only.
    let r = run(&mut s, "solid.box", json!({"length": 4, "width": 4, "height": 10, "corner": [3, 3, 5], "operation": "cut", "targets": ["A"]}));
    assert_eq!((&r["created_bodies"], &r["changed_bodies"], &r["removed_bodies"]), (&json!([]), &json!(["A"]), &json!([])), "{r}");
    // Joining C into B changes B and removes C.
    let r = run(&mut s, "solid.combine", json!({"target": "B", "tools": ["C"], "operation": "join"}));
    assert_eq!((&r["changed_bodies"], &r["removed_bodies"]), (&json!(["B"]), &json!(["C"])), "{r}");
    // A new body changes nothing else.
    let r = run(&mut s, "solid.box", json!({"length": 1, "width": 1, "height": 1, "corner": [0, 50, 0], "operation": "new", "body_name": "D"}));
    assert_eq!((&r["created_bodies"], &r["changed_bodies"], &r["removed_bodies"]), (&json!(["D"]), &json!([]), &json!([])), "{r}");
}

/// Drafting a cylinder's side face 5° from the base plane makes a cone frustum (#547): radius 10
/// at the base, 10 − 20 · tan 5° at the top.
#[test]
fn draft_on_a_cylinder_side_makes_a_frustum() {
    let mut s = Session::default();
    run(&mut s, "solid.cylinder", json!({"radius": 10, "height": 20}));
    run(&mut s, "solid.draft", json!({"faces": [[10, 0, 10]], "angle": "5 deg", "neutral": "XY"}));
    let (big, h) = (10.0, 20.0);
    let small = big - h * 5f64.to_radians().tan();
    let expect = PI * h / 3.0 * (big * big + big * small + small * small);
    assert!(rel(volume(&mut s), expect) < 1e-3, "{} vs {expect}", volume(&mut s));
}

/// The copies added after a rename keep their numbers through undo/redo, save/reopen and a
/// further count change; lowering the count drops the last copies (#372).
#[test]
fn pattern_copies_added_after_a_rename_keep_their_numbers() {
    let mut s = Session::default();
    let names = |s: &Session| -> Vec<String> { s.model.state().bodies.iter().map(|b| b.name.clone()).collect() };
    run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10, "operation": "new_body", "body_name": "X"}));
    run(&mut s, "solid.pattern.rectangular", json!({"bodies": ["X"], "dir1": [1, 0, 0], "count1": 3, "spacing1": 20, "name": "Pat"}));
    run(&mut s, "body.rename", json!({"body": "X (2)", "name": "Second"}));
    run(&mut s, "timeline.edit", json!({"feature": "Pat", "set": {"pattern": {"count1": "5"}}}));
    assert_eq!(names(&s), ["X", "Second", "X (3)", "X (4)", "X (5)"]);
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(names(&s), ["X", "Second", "X (3)"]);
    run(&mut s, "edit.redo", json!({}));
    assert_eq!(names(&s), ["X", "Second", "X (3)", "X (4)", "X (5)"]);
    let reopened = Session::new(solidcraft_doc::Document::from_json(&s.doc.to_json()).unwrap());
    assert_eq!(names(&reopened), ["X", "Second", "X (3)", "X (4)", "X (5)"]);
    run(&mut s, "timeline.edit", json!({"feature": "Pat", "set": {"pattern": {"count1": "2"}}}));
    assert_eq!(names(&s), ["X", "Second"]);
}
