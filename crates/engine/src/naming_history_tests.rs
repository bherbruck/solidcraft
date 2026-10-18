//! Persistent names follow the kernel's exact face history: a reference to a face or edge finds
//! the face it was made on after upstream edits that split, merge, mirror, shrink or move what it
//! names, or says it could not. It never quietly attaches to another face, which is what finding
//! names again by sampling geometry could do.

use serde_json::{Value, json};
use solidcraft_geom::Vec3;

use crate::Session;

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    match s.execute(id, &p) {
        Ok(v) => v,
        Err(e) => panic!("{id} {p}: {e}"),
    }
}

fn id(s: &Session, name: &str) -> u64 {
    s.doc.features.iter().rev().find(|f| f.name == name).map(|f| f.id).unwrap_or_else(|| panic!("feature {name}"))
}

fn warning(s: &Session, name: &str) -> Option<String> {
    s.model.result(id(s, name)).and_then(|r| r.warning.clone())
}

fn error(s: &Session, name: &str) -> Option<String> {
    s.model.result(id(s, name)).and_then(|r| r.error.clone())
}

/// Whether any body holds `p`.
fn solid_at(s: &Session, p: [f64; 3]) -> bool {
    s.model.state().bodies.iter().any(|b| b.body.contains(Vec3::new(p[0], p[1], p[2])))
}

/// Which body holds `p`.
fn body_at(s: &Session, p: [f64; 3]) -> Option<String> {
    s.model.state().bodies.iter().find(|b| b.body.contains(Vec3::new(p[0], p[1], p[2]))).map(|b| b.name.clone())
}

fn names(s: &Session, body: &str) -> Vec<(String, bool)> {
    let state = s.model.state();
    let b = state.body(body).unwrap_or_else(|| panic!("body {body}"));
    let names = solidcraft_doc::naming::face_names(b);
    names.iter().enumerate().map(|(i, n)| (n.clone(), solidcraft_doc::naming::face_name_exact(b, i))).collect()
}

/// A 40 x 30 x 10 block ("Block": sketch lines l1 bottom, l2 right, l3 top, l4 left).
fn block(s: &mut Session) {
    run(s, "sketch.create", json!({"plane": "XY", "name": "S"}));
    run(s, "sketch.rectangle.two_point", json!({"p0": [0, 0], "p1": [40, 30]}));
    run(s, "sketch.finish", json!({}));
    run(s, "solid.extrude", json!({"distance": 10, "name": "Block"}));
}

/// A slot across the block from x = `slot_x` to `slot_x + slot_w`, from z = `floor` up through
/// the top: it splits the top (and, cut to the floor, the block) in two.
fn slot(s: &mut Session, floor: f64) {
    run(s, "parameters.add", json!({"name": "slot_x", "expression": "15 mm"}));
    run(s, "parameters.add", json!({"name": "slot_w", "expression": "10 mm"}));
    run(s, "sketch.create", json!({"plane": "XY", "name": "SlotSketch"}));
    run(s, "sketch.rectangle.two_point", json!({"p0": [15, -5], "p1": [25, 35]}));
    run(s, "sketch.dimension", json!({"entities": ["origin", "p1"], "type": "horizontal", "value": "slot_x"}));
    run(s, "sketch.dimension", json!({"entities": ["l1"], "value": "slot_w"}));
    run(s, "sketch.finish", json!({}));
    let mut p = json!({"sketch": "SlotSketch", "distance": 12, "operation": "cut", "name": "Slot"});
    if floor > 0.0 {
        p["start_offset"] = json!(floor);
    }
    run(s, "solid.extrude", p);
}

fn change(s: &mut Session, name: &str, expression: &str) {
    run(s, "parameters.change", json!({"name": name, "expression": expression}));
}

#[test]
fn faces_built_from_a_profile_are_named_exactly() {
    let mut s = Session::default();
    block(&mut s);
    slot(&mut s, 5.0);
    let block = id(&s, "Block");
    let cut = id(&s, "Slot");
    let n = names(&s, "Body1");
    assert!(n.iter().all(|(_, exact)| *exact), "{n:?}");
    let has = |want: &str| n.iter().any(|(x, _)| x == want);
    // The top is in two pieces, each told apart by the walls only it touches.
    assert!(has(&format!("F{block}:top#[F{block}:side:l4,F{cut}:side:l4]")), "{n:?}");
    assert!(has(&format!("F{block}:top#[F{block}:side:l2,F{cut}:side:l2]")), "{n:?}");
    // The slot's floor is its start cap (above the sketch plane), its walls its sketch curves.
    for want in [format!("F{cut}:top"), format!("F{cut}:side:l2"), format!("F{cut}:side:l4"), format!("F{block}:start")] {
        assert!(has(&want), "{want} in {n:?}");
    }
}

#[test]
fn an_upstream_cut_that_splits_a_face_keeps_the_edge_that_bounded_it() {
    let mut s = Session::default();
    block(&mut s);
    run(&mut s, "solid.fillet", json!({"edges": [[40, 15, 10]], "radius": 2, "name": "Round"}));
    let block = id(&s, "Block");
    let round = s.doc.find_feature("Round").cloned().unwrap();
    assert_eq!(round.edge_names, vec![format!("F{block}:side:l2|F{block}:top")]);
    // Upstream of the fillet, a slot splits the top it bounds.
    run(&mut s, "timeline.roll_to", json!({"feature": "Block"}));
    slot(&mut s, 5.0);
    run(&mut s, "timeline.roll_to", json!({}));
    assert_eq!(error(&s, "Round"), None);
    // Only one edge runs between the right wall and a piece of the top: the same edge.
    assert_eq!(warning(&s, "Round"), None);
    let strip = (4.0 - std::f64::consts::PI) * 30.0;
    let v = run(&mut s, "inspect.measure", json!({}))["total"]["volume_mm3"].as_f64().unwrap();
    assert!((v - (12000.0 - 10.0 * 30.0 * 5.0 - strip)).abs() < 0.05, "{v}");
    assert!(!solid_at(&s, [39.9, 15.0, 9.9]), "the right top edge is rounded");
    assert!(solid_at(&s, [0.1, 15.0, 9.9]), "the left one is not");
}

#[test]
fn a_piece_keeps_its_identity_when_the_cut_that_made_it_moves_or_shrinks() {
    let mut s = Session::default();
    block(&mut s);
    slot(&mut s, 5.0);
    let block = id(&s, "Block");
    let cut = id(&s, "Slot");
    // Raise the piece right of the slot, picked near the slot.
    run(&mut s, "solid.offset_face", json!({"faces": [[27, 15, 10]], "distance": 2, "name": "Raise"}));
    let raise = s.doc.find_feature("Raise").cloned().unwrap();
    assert_eq!(raise.face_names, vec![format!("F{block}:top#[F{block}:side:l2,F{cut}:side:l2]")]);
    assert!(solid_at(&s, [27.0, 15.0, 11.0]) && solid_at(&s, [39.0, 15.0, 11.0]) && !solid_at(&s, [10.0, 15.0, 11.0]));
    // The slot moves right past the picked point, which now lies on the other piece: the name,
    // not the point, says which piece was raised.
    change(&mut s, "slot_x", "28 mm");
    assert_eq!(error(&s, "Raise"), None);
    assert_eq!(warning(&s, "Raise"), None);
    assert!(solid_at(&s, [39.0, 15.0, 11.0]), "the right piece is still the raised one");
    assert!(!solid_at(&s, [27.0, 15.0, 11.0]) && !solid_at(&s, [10.0, 15.0, 11.0]), "the left piece is not raised");
    // Shrunk to a sliver, the slot still splits the top: the same piece, exactly.
    change(&mut s, "slot_w", "0.05 mm");
    assert_eq!(warning(&s, "Raise"), None);
    assert!(solid_at(&s, [39.0, 15.0, 11.0]) && !solid_at(&s, [27.0, 15.0, 11.0]));
    // Gone altogether (suppressed), the piece is the whole top again: said, not guessed.
    run(&mut s, "timeline.suppress", json!({"feature": "Slot", "suppressed": true}));
    let w = warning(&s, "Raise").unwrap_or_default();
    assert!(w.contains("split") || w.contains("no longer exists"), "{w}");
}

#[test]
fn a_face_merged_into_another_answers_to_its_name_and_says_so() {
    let mut s = Session::default();
    block(&mut s);
    run(&mut s, "parameters.add", json!({"name": "boss_h", "expression": "5 mm"}));
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "B"}));
    run(&mut s, "sketch.rectangle.two_point", json!({"p0": [40, 0], "p1": [60, 30]}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.extrude", json!({"sketch": "B", "distance": "boss_h", "operation": "join", "name": "Boss"}));
    let boss = id(&s, "Boss");
    run(&mut s, "solid.offset_face", json!({"faces": [[50, 15, 5]], "distance": 1, "name": "Raise"}));
    let raise = s.doc.find_feature("Raise").cloned().unwrap();
    assert_eq!(raise.face_names, vec![format!("F{boss}:top")]);
    assert!(solid_at(&s, [50.0, 15.0, 5.5]) && !solid_at(&s, [50.0, 15.0, 6.5]));
    // As tall as the block, the boss's top and the block's are one face: the reference finds
    // it and says it was merged.
    change(&mut s, "boss_h", "10 mm");
    assert_eq!(error(&s, "Raise"), None);
    let w = warning(&s, "Raise").unwrap_or_default();
    assert!(w.contains("merged"), "{w}");
    assert!(solid_at(&s, [50.0, 15.0, 10.5]) && solid_at(&s, [20.0, 15.0, 10.5]), "the merged top is raised as one");
}

#[test]
fn mirrored_twins_keep_their_own_edges() {
    let mut s = Session::default();
    block(&mut s);
    run(&mut s, "parameters.add", json!({"name": "hole_x", "expression": "10 mm"}));
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "H"}));
    run(&mut s, "sketch.circle.center", json!({"center": [10, 15], "radius": 3, "id": "c"}));
    run(&mut s, "sketch.dimension", json!({"entities": ["origin", "c.center"], "type": "horizontal", "value": "hole_x"}));
    run(&mut s, "sketch.dimension", json!({"entities": ["origin", "c.center"], "type": "vertical", "value": "15 mm"}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.extrude", json!({"sketch": "H", "distance": 10, "operation": "cut", "name": "Hole"}));
    run(&mut s, "solid.mirror", json!({"features": ["Hole"], "plane": {"origin": [20, 0, 0], "normal": [1, 0, 0]}, "name": "Twin"}));
    let twin = id(&s, "Twin");
    // Chamfer the twin's rim (at x = 30) only.
    run(&mut s, "solid.chamfer", json!({"edges": [[33, 15, 10]], "distance": 1, "name": "Bevel"}));
    let bevel = s.doc.find_feature("Bevel").cloned().unwrap();
    assert!(bevel.edge_names.iter().all(|n| n.contains(&format!("@F{twin}.1"))), "{:?}", bevel.edge_names);
    let rim = |c: f64| [c + 3.5, 15.0, 9.8];
    assert!(!solid_at(&s, rim(30.0)) && solid_at(&s, rim(10.0)));
    // The holes move toward each other: the chamfer's picked point is now off both rims, and
    // the twin is the hole nearer it than its source is. Its name says which.
    change(&mut s, "hole_x", "14 mm");
    assert_eq!(error(&s, "Bevel"), None);
    assert_eq!(warning(&s, "Bevel"), None);
    assert!(!solid_at(&s, rim(26.0)), "the twin's rim is chamfered");
    assert!(solid_at(&s, rim(14.0)), "its source's is not");
}

#[test]
fn pieces_of_a_cut_body_keep_their_body_names_when_the_cut_moves() {
    let mut s = Session::default();
    block(&mut s);
    slot(&mut s, 0.0);
    let right = body_at(&s, [39.0, 15.0, 5.0]).unwrap();
    let left = body_at(&s, [1.0, 15.0, 5.0]).unwrap();
    assert_ne!(right, left);
    run(&mut s, "solid.fillet", json!({"edges": [[40, 15, 10]], "radius": 2, "name": "Round"}));
    for x in ["5 mm", "22 mm", "15 mm", "2 mm"] {
        change(&mut s, "slot_x", x);
        assert_eq!(body_at(&s, [39.0, 15.0, 5.0]), Some(right.clone()), "slot at {x}");
        assert_eq!(body_at(&s, [1.0, 15.0, 5.0]), Some(left.clone()), "slot at {x}");
        assert_eq!(error(&s, "Round"), None, "slot at {x}");
        assert_eq!(warning(&s, "Round"), None, "slot at {x}");
        assert!(!solid_at(&s, [39.9, 15.0, 9.9]), "slot at {x}");
        // Faces of the pieces, of one face cut in two, are told apart across the bodies.
        let all: Vec<String> = [&right, &left].iter().flat_map(|b| names(&s, b)).map(|(n, _)| n).collect();
        let unique: std::collections::BTreeSet<&String> = all.iter().collect();
        assert_eq!(unique.len(), all.len(), "{all:?}");
    }
}

#[test]
fn a_name_of_a_body_without_history_is_found_by_geometry_and_says_so_once_it_moves() {
    let mut s = Session::default();
    run(&mut s, "parameters.add", json!({"name": "len", "expression": "40 mm"}));
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "S"}));
    run(&mut s, "sketch.rectangle.two_point", json!({"p0": [0, 0], "p1": [40, 30]}));
    run(&mut s, "sketch.dimension", json!({"entities": ["l1"], "value": "len"}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.extrude", json!({"distance": 10, "name": "Block"}));
    // Replacing the top by a plane: an operation that reports no history.
    run(&mut s, "solid.replace_face", json!({"faces": [[20, 15, 10]], "target": {"origin": [0, 0, 12], "normal": [0, 0, 1]}, "name": "Lift"}));
    let state = s.model.state();
    let body = state.bodies.first().unwrap();
    assert!((0..solidcraft_doc::naming::face_names(body).len()).all(|i| !solidcraft_doc::naming::face_name_exact(body, i)));
    run(&mut s, "solid.fillet", json!({"edges": [[40, 15, 12]], "radius": 2, "name": "Round"}));
    // Where it was picked, the pick and the name agree: nothing to say.
    assert_eq!(warning(&s, "Round"), None);
    // Moved away from the pick, only geometry found it: the fillet says so.
    change(&mut s, "len", "50 mm");
    assert_eq!(error(&s, "Round"), None);
    let w = warning(&s, "Round").unwrap_or_default();
    assert!(w.contains("found by geometry, not exact history"), "{w}");
    assert!(!solid_at(&s, [49.9, 15.0, 11.9]));
}

#[test]
fn a_sketch_on_a_certified_face_survives_a_fillet_and_a_cut_upstream_of_it() {
    let mut s = Session::default();
    run(&mut s, "parameters.add", json!({"name": "h", "expression": "10 mm"}));
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "S"}));
    run(&mut s, "sketch.rectangle.two_point", json!({"p0": [0, 0], "p1": [40, 30]}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.extrude", json!({"distance": "h", "name": "Block"}));
    run(&mut s, "solid.fillet", json!({"edges": [[40, 0, 5]], "radius": 3, "name": "Corner"}));
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "H"}));
    run(&mut s, "sketch.circle.center", json!({"center": [8, 8], "radius": 2}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.extrude", json!({"sketch": "H", "distance": 20, "operation": "cut", "name": "Hole"}));
    run(&mut s, "sketch.create", json!({"plane": {"face": [25, 18, 10]}, "name": "Top"}));
    run(&mut s, "sketch.circle.center", json!({"center": [25, 18], "radius": 3}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.extrude", json!({"sketch": "Top", "distance": 4, "operation": "join", "name": "Peg"}));
    let top = |s: &Session| s.model.state().bodies.iter().map(|b| b.mesh().bounds().max.z).fold(f64::MIN, f64::max);
    assert!((top(&s) - 14.0).abs() < 1e-6);
    let state = s.model.state();
    let body = state.bodies.first().unwrap();
    assert!((0..solidcraft_doc::naming::face_names(body).len()).all(|i| solidcraft_doc::naming::face_name_exact(body, i)));
    change(&mut s, "h", "16 mm");
    for f in ["Corner", "Hole", "Top", "Peg"] {
        assert_eq!(error(&s, f), None, "{f}");
        assert_eq!(warning(&s, f), None, "{f}");
    }
    assert!((top(&s) - 20.0).abs() < 1e-6, "{}", top(&s));
}

/// A fillet of a shell's inner corners names those edges. Thinning the wall past the outer
/// round removes them. The fillet must say the names are gone, not blend the seam nearest
/// the old pick, which now sits on the axis of the inner round (#775).
#[test]
fn a_shell_fillet_does_not_follow_a_gone_edge_onto_a_smooth_seam() {
    let mut s = Session::default();
    run(&mut s, "sketch.create", json!({"plane": "XY"}));
    run(&mut s, "sketch.rectangle.two_point", json!({"p0": [-27.5, -40], "p1": [27.5, 40]}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.extrude", json!({"distance": 25, "name": "Extrude1"}));
    run(
        &mut s,
        "solid.fillet",
        json!({"radius": 5, "name": "Fillet1", "edges": [[-27.5, -40, 12.5], [27.5, -40, 12.5], [27.5, 40, 12.5], [-27.5, 40, 12.5]]}),
    );
    run(&mut s, "solid.shell", json!({"faces": [[0, 0, 25]], "thickness": 5, "direction": "inside", "name": "Shell1"}));
    run(
        &mut s,
        "solid.fillet",
        json!({"radius": 5, "name": "Fillet2", "edges": [[-22.5, 35, 15], [22.5, 35, 15], [22.5, -35, 15], [-22.5, -35, 15]]}),
    );
    assert_eq!(error(&s, "Fillet2"), None);
    let edited = s.execute("timeline.edit", &json!({"feature": "Shell1", "set": {"thickness": "2"}}));
    let message = error(&s, "Fillet2").or_else(|| edited.err().map(|e| e.to_string())).unwrap_or_default();
    assert!(message.contains("no longer exists"), "{message}");
    assert!(!message.contains("smooth edge"), "{message}");
}

/// The faces `model.faces` lists for `body`: name, whether it is exact, and centroid.
fn listed(s: &mut Session, body: &str) -> Vec<(String, bool, [f64; 3])> {
    let v = run(s, "model.faces", json!({"body": body}));
    let faces = v["faces"].as_array().unwrap_or_else(|| panic!("faces of {body}: {v}"));
    faces
        .iter()
        .map(|f| {
            let c = |k: usize| f["centroid"][k].as_f64().unwrap_or(f64::NAN);
            (f["name"].as_str().unwrap_or_default().to_string(), f["exact"].as_bool() == Some(true), [c(0), c(1), c(2)])
        })
        .collect()
}

/// The face of `faces` whose centroid is within 0.05 of `at`.
fn named_at(faces: &[(String, bool, [f64; 3])], at: [f64; 3]) -> String {
    let near = |c: &[f64; 3]| (0..3).all(|k| (c[k] - at[k]).abs() < 0.05);
    let found: Vec<&String> = faces.iter().filter(|(_, _, c)| near(c)).map(|(n, _, _)| n).collect();
    match found.as_slice() {
        [n] => (*n).clone(),
        _ => panic!("faces at {at:?}: {found:?} in {faces:?}"),
    }
}

/// #1209: an arm with a boss at each end (each boss the radius of the arm's round end under it)
/// and a square cut through one. The second boss and the cut are prism booleans sewn slab by
/// slab, where the arm's round end and the boss over it are two pieces of one cylinder: every
/// face keeps exact history, so the cut's walls are its own four faces and the boss's outer
/// cylinder keeps its name, through the fillets on the bore's corners too.
#[test]
fn a_cut_through_one_of_two_bosses_names_only_its_own_walls() {
    let mut s = Session::default();
    run(&mut s, "sketch.create", json!({"plane": "XY"}));
    let (lo0, lo1) = ([16.54527384497186, -18.318724018148217], [159.66891048662592, -12.425397803491872]);
    let (hi0, hi1) = ([159.66891048662592, 12.425397803491872], [16.54527384497186, 18.318724018148217]);
    let (j0, j1) = ([162.79697152270225, -13.15736982664994], [162.79697152270225, 13.15736982664994]);
    let (k0, k1) = ([11.818299035314524, 19.73139143375053], [11.818299035314524, -19.73139143375053]);
    run(&mut s, "sketch.line", json!({"points": [lo0, lo1]}));
    run(&mut s, "sketch.arc.center_point", json!({"center": [159.91576013178314, -18.420317757309917], "start": j0, "end": lo1}));
    run(&mut s, "sketch.arc.center_point", json!({"center": [170.0, 0], "start": j0, "end": j1}));
    run(&mut s, "sketch.arc.center_point", json!({"center": [159.91576013178314, 18.420317757309917], "start": hi0, "end": j1}));
    run(&mut s, "sketch.line", json!({"points": [hi0, hi1]}));
    run(&mut s, "sketch.arc.center_point", json!({"center": [16.95668992023388, 28.310257274511628], "start": k0, "end": hi1}));
    run(&mut s, "sketch.arc.center_point", json!({"center": [0, 0], "start": k0, "end": k1}));
    run(&mut s, "sketch.arc.center_point", json!({"center": [16.95668992023388, -28.310257274511628], "start": lo0, "end": k1}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.extrude", json!({"distance": 16, "body_name": "Arm"}));
    run(&mut s, "solid.cylinder", json!({"diameter": 46.0, "height": 22, "base": [0, 0, 0], "operation": "join", "targets": ["Arm"]}));
    let boss = named_at(&listed(&mut s, "Arm"), [19.98, -4.24, 19.0]);
    assert!(boss.starts_with("F3:side"), "{boss}");
    run(&mut s, "solid.cylinder", json!({"diameter": 30.0, "height": 20, "base": [170.0, 0, 0], "operation": "join", "targets": ["Arm"]}));
    let joined = listed(&mut s, "Arm");
    assert!(joined.iter().all(|(_, exact, _)| *exact), "{joined:?}");
    run(&mut s, "sketch.create", json!({"plane": "XY"}));
    run(&mut s, "sketch.rectangle.center", json!({"center": [0, 0], "corner": [6.35, 6.35]}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.extrude", json!({"through_all": true, "operation": "cut", "targets": ["Arm"]}));
    let cut = listed(&mut s, "Arm");
    assert!(cut.iter().all(|(_, exact, _)| *exact), "{cut:?}");
    let walls: Vec<&String> = cut.iter().map(|(n, _, _)| n).filter(|n| n.starts_with("F6:")).collect();
    assert_eq!(walls.len(), 4, "{cut:?}");
    for (at, line) in [([0.0, -6.35, 11.0], "l1"), ([6.35, 0.0, 11.0], "l2"), ([0.0, 6.35, 11.0], "l3"), ([-6.35, 0.0, 11.0], "l4")] {
        assert_eq!(named_at(&cut, at), format!("F6:side:{line}"));
    }
    assert_eq!(named_at(&cut, [19.98, -4.24, 19.0]), boss);
    run(&mut s, "solid.fillet", json!({"edges": [[6.35, 6.35, 10], [-6.35, 6.35, 10], [-6.35, -6.35, 10], [6.35, -6.35, 10]], "radius": 1.5}));
    let filleted = listed(&mut s, "Arm");
    let blends = filleted.iter().filter(|(n, _, _)| n.starts_with("F7:blend:")).count();
    assert_eq!(blends, 4, "{filleted:?}");
    assert_eq!(named_at(&filleted, [19.98, -4.24, 19.0]), boss);
}
