use serde_json::{Value, json};

use crate::Session;

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    match s.execute(id, &p) {
        Ok(v) => v,
        Err(e) => panic!("{id} {p}: {e}"),
    }
}

fn cube(s: &mut Session, name: &str, size: f64, corner: [f64; 3]) {
    run(s, "solid.box", json!({"length": size, "width": size, "height": size, "corner": corner, "operation": "new", "body_name": name}));
}

fn f(v: &Value) -> f64 {
    v.as_f64().unwrap_or(f64::NAN)
}

/// The pair `{a, b}` of a list entry, sorted.
fn pair(v: &Value) -> [String; 2] {
    let mut p = [v["a"].as_str().unwrap_or("").to_string(), v["b"].as_str().unwrap_or("").to_string()];
    p.sort();
    p
}

/// Two 10 mm cubes sharing a 1 mm corner cube overlap by 1 mm³; a third, far off, is pruned.
#[test]
fn an_overlap_of_one_cubic_millimetre_is_reported() {
    let mut s = Session::default();
    cube(&mut s, "A", 10.0, [0.0, 0.0, 0.0]);
    cube(&mut s, "B", 10.0, [9.0, 9.0, 9.0]);
    cube(&mut s, "Far", 10.0, [100.0, 0.0, 0.0]);
    let r = run(&mut s, "inspect.clearance", json!({}));
    let overlaps = r["overlaps"].as_array().cloned().unwrap_or_default();
    assert_eq!(overlaps.len(), 1, "{r}");
    assert_eq!(pair(&overlaps[0]), ["A", "B"]);
    assert!((f(&overlaps[0]["volume_mm3"]) - 1.0).abs() < 1e-6, "{r}");
    // Three faces of 1 x 1 from each cube: 6 mm².
    assert!((f(&overlaps[0]["area_mm2"]) - 6.0).abs() < 1e-6, "{r}");
    assert!((f(&overlaps[0]["center"][0]) - 9.5).abs() < 1e-6, "{r}");
    assert_eq!(r["pairs_checked"], 1, "{r}");
    assert_eq!(r["pairs_pruned"], 2, "{r}");
    assert_eq!(r["unchecked"].as_array().map(Vec::len), Some(0), "{r}");
    // An overlap under min_volume is contact, not an overlap: with a clearance it is near, at 0.
    let r = run(&mut s, "inspect.clearance", json!({"min_volume": 2, "clearance": 0.1}));
    assert_eq!(r["overlaps"].as_array().map(Vec::len), Some(0), "{r}");
    assert_eq!(f(&r["near"][0]["distance_mm"]), 0.0, "{r}");
    assert!((f(&r["near"][0]["overlap_mm3"]) - 1.0).abs() < 1e-6, "{r}");
}

/// Two cubes 0.5 mm apart are near with a clearance of 1 mm; 2 mm apart they are not.
#[test]
fn pairs_closer_than_the_clearance_are_near() {
    let mut s = Session::default();
    cube(&mut s, "A", 10.0, [0.0, 0.0, 0.0]);
    cube(&mut s, "B", 10.0, [10.5, 2.0, 3.0]);
    cube(&mut s, "C", 10.0, [-12.0, 0.0, 0.0]);
    let r = run(&mut s, "inspect.clearance", json!({"clearance": 1}));
    let near = r["near"].as_array().cloned().unwrap_or_default();
    assert_eq!(near.len(), 1, "{r}");
    assert_eq!(pair(&near[0]), ["A", "B"]);
    assert!((f(&near[0]["distance_mm"]) - 0.5).abs() < 1e-9, "{r}");
    assert!(near[0].get("exact").is_none(), "measured on the exact faces: {r}");
    // The points lie on the facing faces x = 10 and x = 10.5.
    let xs = [f(&near[0]["from"][0]), f(&near[0]["to"][0])];
    assert!(xs.iter().any(|x| (x - 10.0).abs() < 1e-9) && xs.iter().any(|x| (x - 10.5).abs() < 1e-9), "{r}");
    assert_eq!(r["overlaps"].as_array().map(Vec::len), Some(0), "{r}");
    // With no clearance nothing is near; with 2.5 mm C (2 mm from A) is near too.
    let r = run(&mut s, "inspect.clearance", json!({}));
    assert_eq!(r["near"].as_array().map(Vec::len), Some(0), "{r}");
    let r = run(&mut s, "inspect.clearance", json!({"clearance": 2.5}));
    let near = r["near"].as_array().cloned().unwrap_or_default();
    assert_eq!(near.len(), 2, "{r}");
    assert_eq!(pair(&near[1]), ["A", "C"]);
    assert!((f(&near[1]["distance_mm"]) - 2.0).abs() < 1e-9, "{r}");
}

/// Distances where the nearest points are on edges, and on curved faces whose meshes are far
/// coarser than the answer: cube edge to cube edge, a cylinder's side to a cube's face, and a
/// large sphere (mesh chord about 1 mm) to a cube.
#[test]
fn distances_are_exact_on_edges_and_curved_faces() {
    let mut s = Session::default();
    cube(&mut s, "A", 10.0, [0.0, 0.0, 0.0]);
    // Its vertical edge at (12, 13) faces A's at (10, 10): √(2² + 3²).
    cube(&mut s, "B", 10.0, [12.0, 13.0, 0.0]);
    run(&mut s, "solid.cylinder", json!({"base": [-30, 5, 0], "radius": 5, "height": 10, "operation": "new", "body_name": "Pin"}));
    run(&mut s, "solid.sphere", json!({"radius": 500, "center": [5, 5, 510.7], "operation": "new", "body_name": "Ball"}));
    let r = run(&mut s, "inspect.clearance", json!({"clearance": 50, "against": ["A"], "bodies": ["B", "Pin", "Ball"]}));
    let near = r["near"].as_array().cloned().unwrap_or_default();
    let d = |other: &str| near.iter().find(|n| pair(n).contains(&other.to_string())).map(|n| (f(&n["distance_mm"]), n.get("exact").is_none()));
    assert_eq!(near.len(), 3, "{r}");
    let (edge, exact) = d("B").unwrap_or((f64::NAN, false));
    assert!((edge - 13f64.sqrt()).abs() < 1e-6 && exact, "{r}");
    // The pin's side (x = -25) is 25 from A's face x = 0.
    let (side, exact) = d("Pin").unwrap_or((f64::NAN, false));
    assert!((side - 25.0).abs() < 1e-6 && exact, "{r}");
    // The ball's lowest point, (5, 5, 10.7), is 0.7 above A's top.
    let (ball, exact) = d("Ball").unwrap_or((f64::NAN, false));
    assert!((ball - 0.7).abs() < 1e-6 && exact, "{r}");
    // `against` leaves out the pairs among `bodies`, which are by default every other body.
    assert!(near.iter().all(|n| pair(n).contains(&"A".to_string())), "{r}");
    let all = run(&mut s, "inspect.clearance", json!({"clearance": 50, "against": ["A"]}));
    assert_eq!(all["near"].as_array().map(Vec::len), Some(3), "{all}");
}

/// A pin in a hole 0.2 mm wider all round, in a plate whose mesh is coarser than that gap: the
/// plate's flat faces reach over the hole on their surfaces but not on the faces, so the
/// distance is the gap to the hole's wall.
#[test]
fn a_pin_in_a_hole_is_as_far_as_the_holes_wall() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 100, "width": 60, "height": 2, "operation": "new", "body_name": "Plate"}));
    run(&mut s, "solid.cylinder", json!({"radius": 3.2, "height": 4, "base": [10, 10, -1], "operation": "cut", "targets": ["Plate"]}));
    run(&mut s, "solid.cylinder", json!({"radius": 3, "height": 12, "base": [10, 10, -5], "operation": "new", "body_name": "Pin"}));
    let r = run(&mut s, "inspect.clearance", json!({"clearance": 1}));
    assert_eq!(r["overlaps"].as_array().map(Vec::len), Some(0), "{r}");
    assert!((f(&r["near"][0]["distance_mm"]) - 0.2).abs() < 1e-6, "{r}");
    let r = run(&mut s, "inspect.clearance", json!({"clearance": 0.15}));
    assert_eq!(r["near"].as_array().map(Vec::len), Some(0), "{r}");
}

/// Three spheres' nearest neighbours against their distances worked out by hand.
#[test]
fn nearest_distances_of_three_spheres() {
    let mut s = Session::default();
    for (name, r, c) in [("S1", 5.0, [0.0, 0.0, 0.0]), ("S2", 3.0, [12.0, 0.0, 0.0]), ("S3", 2.0, [3.0, 20.0, 4.0])] {
        run(&mut s, "solid.sphere", json!({"radius": r, "center": c, "operation": "new", "body_name": name}));
    }
    let r = run(&mut s, "inspect.clearance", json!({"nearest": 2}));
    let d12 = 12.0 - 8.0;
    let d13 = (9.0f64 + 400.0 + 16.0).sqrt() - 7.0;
    let d23 = (81.0f64 + 400.0 + 16.0).sqrt() - 5.0;
    let want = [("S1", [("S2", d12), ("S3", d13)]), ("S2", [("S1", d12), ("S3", d23)]), ("S3", [("S1", d13), ("S2", d23)])];
    for (body, list) in want {
        let got = r["nearest"][body].as_array().cloned().unwrap_or_default();
        assert_eq!(got.len(), 2, "{body}: {r}");
        for (g, (other, d)) in got.iter().zip(list) {
            assert_eq!(g["body"], other, "{body}: {r}");
            assert!((f(&g["distance_mm"]) - d).abs() < 1e-6, "{body} to {other}: {} against {d}", g["distance_mm"]);
        }
    }
    // One neighbour each.
    let r = run(&mut s, "inspect.clearance", json!({"nearest": 1, "bodies": ["S3"]}));
    assert_eq!(r["nearest"]["S3"].as_array().map(Vec::len), Some(0), "only S3 is in `bodies`, so it has no partner: {r}");
    let r = run(&mut s, "inspect.clearance", json!({"nearest": 1, "bodies": ["S3"], "against": ["S1", "S2"]}));
    assert_eq!(r["nearest"]["S3"][0]["body"], "S1", "{r}");
}

/// `skip_within` leaves out the pairs inside one component (two bodies made to touch, as the
/// parts of a bought component do) and keeps those between components.
#[test]
fn skip_within_leaves_out_a_components_own_pairs() {
    let mut s = Session::default();
    run(&mut s, "component.create", json!({"name": "Bought"}));
    cube(&mut s, "Housing", 10.0, [0.0, 0.0, 0.0]);
    cube(&mut s, "Insert", 4.0, [8.0, 3.0, 3.0]);
    run(&mut s, "component.activate", json!({"component": "root"}));
    run(&mut s, "component.create", json!({"name": "Bracket"}));
    cube(&mut s, "Plate", 10.0, [-9.5, 0.0, 0.0]);
    let all = run(&mut s, "inspect.clearance", json!({}));
    let pairs: Vec<[String; 2]> = all["overlaps"].as_array().into_iter().flatten().map(pair).collect();
    assert!(pairs.contains(&["Housing".into(), "Insert".into()]) && pairs.contains(&["Housing".into(), "Plate".into()]), "{all}");
    for mode in ["component", "occurrence"] {
        let r = run(&mut s, "inspect.clearance", json!({"skip_within": mode}));
        let pairs: Vec<[String; 2]> = r["overlaps"].as_array().into_iter().flatten().map(pair).collect();
        assert_eq!(pairs, vec![["Housing".to_string(), "Plate".to_string()]], "{mode}: {r}");
        assert!((f(&r["overlaps"][0]["volume_mm3"]) - 50.0).abs() < 1e-6, "{r}");
    }
}

/// Parameters out of range are refused, not guessed at.
#[test]
fn bad_parameters_are_refused() {
    let mut s = Session::default();
    cube(&mut s, "A", 10.0, [0.0, 0.0, 0.0]);
    for p in [
        json!({"clearance": -1}),
        json!({"clearance": "wide"}),
        json!({"min_volume": -0.1}),
        json!({"nearest": 1e9}),
        json!({"tolerance": 5}),
        json!({"skip_within": "body"}),
        json!({"bodies": ["Nope"]}),
        json!({"against": ["Nope"]}),
    ] {
        assert!(s.execute("inspect.clearance", &p).is_err(), "{p}");
    }
    let r = run(&mut s, "inspect.clearance", json!({"nearest": 3, "skip_within": null}));
    assert_eq!(r["pairs_checked"], 0, "{r}");
    assert_eq!(r["nearest"]["A"].as_array().map(Vec::len), Some(0), "{r}");
}

/// A pin sunk 0.01 mm into a plate 1.7 m long: the shared ring holds π (4.43² − 2.5²) × 0.01 mm³
/// wherever the plate stands, as on a plate of 100 mm (the boolean of the whole plate gave 0.31).
#[test]
fn a_thin_overlap_with_a_large_plate_is_measured_in_full() {
    let ring = std::f64::consts::PI * (4.43f64.powi(2) - 2.5f64.powi(2)) * 0.01;
    for (length, far) in [(100.0, false), (1700.0, false), (1700.0, true)] {
        let mut s = Session::default();
        let corner = [-length / 2.0, -length / 4.0, 0.0];
        run(&mut s, "solid.box", json!({"length": length, "width": length / 2.0, "height": 5, "corner": corner, "body_name": "Plate"}));
        run(&mut s, "solid.cylinder", json!({"radius": 2.5, "height": 9, "base": [0, 0, -2], "operation": "cut", "targets": ["Plate"]}));
        run(&mut s, "solid.cylinder", json!({"radius": 2.4, "height": 7, "base": [0, 0, -1.99], "operation": "new", "body_name": "Pin"}));
        run(&mut s, "solid.cylinder", json!({"radius": 4.43, "height": 2, "base": [0, 0, 4.99], "operation": "join", "targets": ["Pin"]}));
        let shift = if far { [-326.3, 13.5, 1278.3] } else { [0.0; 3] };
        if far {
            run(&mut s, "solid.move", json!({"bodies": ["Plate", "Pin"], "translate": shift}));
        }
        let r = run(&mut s, "inspect.clearance", json!({"min_volume": 1e-4}));
        let o = &r["overlaps"][0];
        assert!((f(&o["volume_mm3"]) - ring).abs() < 1e-3 * ring, "{length} {far}: {r}");
        // The ring's centre is on the axis, 0.005 below the plate's face.
        assert!((f(&o["center"][2]) - (4.995 + shift[2])).abs() < 1e-4, "{length} {far}: {r}");
        assert!((f(&o["center"][0]) - shift[0]).abs() < 1e-4, "{length} {far}: {r}");
    }
}

/// A shared part whose centre lies outside its own box, or which holds more than its box, is
/// malformed: the pair is reported unchecked rather than with that volume.
#[test]
fn a_malformed_shared_part_is_not_measured() {
    use solidcraft_geom::{Aabb3, Vec3};
    let bbox = Aabb3 { min: Vec3::new(0.0, 0.0, 0.0), max: Vec3::new(10.0, 10.0, 1.0) };
    assert!(super::plausible(50.0, Vec3::new(5.0, 5.0, 0.5), bbox));
    assert!(!super::plausible(50.0, Vec3::new(5.0, 5.0, 3.0), bbox));
    assert!(!super::plausible(1239.0, Vec3::new(5.0, 5.0, 0.5), bbox));
    assert!(!super::plausible(f64::NAN, Vec3::new(5.0, 5.0, 0.5), bbox));
}

/// Two plates 0.2 mm apart, the upper one set 10 mm in from the lower one's corner, and two set
/// side by side with a 0.2 mm gap: 0.2, not 0 (a point past a corner, on the line of one of its
/// edges, was taken for a point of the face). Gaps under twice the meshes' chord went wrong.
#[test]
fn a_small_gap_past_a_corner_is_not_contact() {
    let mut s = Session::default();
    let plate = |s: &mut Session, name: &str, size: [f64; 3], corner: [f64; 3]| {
        run(s, "solid.box", json!({"length": size[0], "width": size[1], "height": size[2], "corner": corner, "operation": "new", "body_name": name}));
    };
    plate(&mut s, "A", [100.0, 100.0, 2.0], [0.0, 0.0, 0.0]);
    plate(&mut s, "B", [100.0, 100.0, 2.0], [10.0, 10.0, 2.2]);
    plate(&mut s, "C", [100.0, 100.0, 2.0], [0.0, 0.0, 10.0]);
    plate(&mut s, "D", [100.0, 100.0, 2.0], [100.2, 0.0, 10.0]);
    // 3 mm apart in x and 0.2 in z, along their long sides: √(3² + 0.2²).
    plate(&mut s, "E", [1000.0, 100.0, 2.0], [0.0, 300.0, 0.0]);
    plate(&mut s, "F", [1000.0, 100.0, 2.0], [1003.0, 300.0, 2.2]);
    for (a, b, want) in [("A", "B", 0.2), ("C", "D", 0.2), ("E", "F", (9.0f64 + 0.04).sqrt())] {
        let r = run(&mut s, "inspect.clearance", json!({"bodies": [a], "against": [b], "clearance": 5, "nearest": 4}));
        assert!((f(&r["near"][0]["distance_mm"]) - want).abs() < 1e-9, "{a} {b}: {r}");
        assert!((f(&r["nearest"][a][0]["distance_mm"]) - want).abs() < 1e-9, "{a} {b}: {r}");
    }
}
