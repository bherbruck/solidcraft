use serde_json::{Value, json};

use crate::Session;

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    match s.execute(id, &p) {
        Ok(v) => v,
        Err(e) => panic!("{id} {p}: {e}"),
    }
}

fn auto_emboss_params(mut p: Value, auto: bool) -> Value {
    if auto {
        if let Some(params) = p.get_mut("params").and_then(Value::as_object_mut) {
            params.remove("face");
        } else if let Some(object) = p.as_object_mut() {
            object.remove("face");
        }
    }
    p
}

fn measure(s: &mut Session) -> Value {
    run(s, "inspect.measure", json!({}))
}

fn volume(s: &mut Session) -> f64 {
    measure(s)["total"]["volume_mm3"].as_f64().unwrap_or(f64::NAN)
}

fn rel(a: f64, b: f64) -> f64 {
    (a - b).abs() / b.abs().max(1e-12)
}

/// An L bracket: a 40 x 20 x 5 base and a 5 x 20 x 40 wall at x = 0..5.
fn bracket() -> Session {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 40, "width": 20, "height": 5, "corner": [0, 0, 0], "body_name": "L"}));
    run(&mut s, "solid.box", json!({"length": 5, "width": 20, "height": 40, "corner": [0, 0, 0], "operation": "join"}));
    assert!(rel(volume(&mut s), 4000.0 + 3500.0) < 1e-9);
    // Mid-plane of the bracket (y = 10): sketch x = X, sketch y = Z.
    run(&mut s, "sketch.create", json!({"plane": {"origin": [0, 10, 0], "x_dir": [1, 0, 0], "y_dir": [0, 0, 1]}, "name": "RibSketch"}));
    run(&mut s, "sketch.line", json!({"points": [[5, 25], [25, 5]], "ids": ["l1"]}));
    run(&mut s, "sketch.finish", json!({}));
    s
}

#[test]
fn rib_to_the_next_face_fills_the_corner() {
    let mut s = bracket();
    run(&mut s, "solid.rib", json!({"sketch": "RibSketch", "curves": ["l1"], "thickness": 4}));
    // The triangle (5,5)-(5,25)-(25,5) of area 200, 4 thick.
    assert!(rel(volume(&mut s), 7500.0 + 800.0) < 1e-6, "{}", volume(&mut s));
    assert_eq!(measure(&mut s)["body_count"], 1);
    // The thickness is a parameter.
    let t = s.doc.features.last().and_then(|f| f.param_names.first().cloned()).unwrap_or_default();
    run(&mut s, "parameters.change", json!({"name": t, "expression": "2"}));
    assert!(rel(volume(&mut s), 7500.0 + 400.0) < 1e-6);
}

#[test]
fn rib_with_a_depth_and_web() {
    let mut s = bracket();
    run(&mut s, "solid.rib", json!({"sketch": "RibSketch", "curves": ["l1"], "thickness": 4, "depth": 2}));
    let v = volume(&mut s);
    assert!(v > 7500.0 + 100.0 && v < 7500.0 + 800.0, "{v}");
    let mut s = bracket();
    run(&mut s, "solid.web", json!({"sketch": "RibSketch", "curves": ["l1"], "thickness": 4}));
    assert!(rel(volume(&mut s), 8300.0) < 1e-6);
    // Curves that face no body.
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10}));
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Far"}));
    run(&mut s, "sketch.line", json!({"points": [[50, 50], [60, 60]], "ids": ["l1"]}));
    run(&mut s, "sketch.finish", json!({}));
    let e = s.execute("solid.rib", &json!({"sketch": "Far", "curves": ["l1"], "thickness": 1}));
    assert!(e.is_err_and(|e| e.to_string().contains("do not face a body")));
}

#[test]
fn rib_along_the_sketch_normal_reaches_the_body() {
    // A 20 x 20 x 10 box; a line along X on a sketch 10 in front of it (y = -10), whose plane
    // the box does not cross, so only a rib along the sketch normal can reach it.
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 20, "width": 20, "height": 10, "corner": [0, 0, 0]}));
    run(&mut s, "sketch.create", json!({"plane": {"origin": [0, -10, 0], "x_dir": [1, 0, 0], "y_dir": [0, 0, 1]}, "name": "Front"}));
    run(&mut s, "sketch.line", json!({"points": [[2, 5], [18, 5]], "ids": ["l1"]}));
    run(&mut s, "sketch.finish", json!({}));
    let e = s.execute("solid.rib", &json!({"sketch": "Front", "curves": ["l1"], "thickness": 2}));
    assert!(e.is_err_and(|e| e.to_string().contains("do not face a body")));
    run(&mut s, "solid.rib", json!({"sketch": "Front", "curves": ["l1"], "thickness": 2, "direction": "normal"}));
    // 16 long, 2 thick, 10 from the sketch to the box.
    assert!(rel(volume(&mut s), 4000.0 + 320.0) < 1e-6, "{}", volume(&mut s));
    assert_eq!(measure(&mut s)["body_count"], 1);
    run(&mut s, "edit.undo", json!({}));
    run(&mut s, "solid.rib", json!({"sketch": "Front", "curves": ["l1"], "thickness": 2, "direction": "normal", "depth": 4}));
    assert!(rel(volume(&mut s), 4000.0 + 128.0) < 1e-6, "{}", volume(&mut s));
    let e = s.execute("solid.rib", &json!({"sketch": "Front", "curves": ["l1"], "thickness": 2, "direction": "sideways"}));
    assert!(e.is_err_and(|e| e.to_string().contains("unknown direction")));
}

#[test]
fn emboss_and_deboss_on_a_face() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 40, "width": 30, "height": 10}));
    run(&mut s, "sketch.create", json!({"plane": {"face": [20, 15, 10]}, "name": "Logo"}));
    run(&mut s, "sketch.rectangle.two_point", json!({"p0": [5, 5], "p1": [15, 15]}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.emboss", json!({"sketch": "Logo", "depth": 2}));
    assert!(rel(volume(&mut s), 12000.0 + 200.0) < 1e-6, "{}", volume(&mut s));
    run(&mut s, "edit.undo", json!({}));
    run(&mut s, "solid.emboss", json!({"sketch": "Logo", "depth": 3, "mode": "deboss"}));
    assert!(rel(volume(&mut s), 12000.0 - 300.0) < 1e-6);
    assert!(s.execute("solid.emboss", &json!({"sketch": "Logo", "depth": 1, "mode": "sideways"})).is_err());
}

#[test]
fn emboss_face_off_the_part_says_how_far() {
    // #392: a point beside the face was refused with "`face` must identify exactly one target
    // body surface", which didn't say the point was off the face.
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 40, "width": 30, "height": 10, "body_name": "A"}));
    run(&mut s, "sketch.create", json!({"plane": {"face": [20, 15, 10]}, "name": "Logo"}));
    run(&mut s, "sketch.rectangle.two_point", json!({"p0": [5, 5], "p1": [15, 15]}));
    run(&mut s, "sketch.finish", json!({}));
    let e = s.execute("solid.emboss", &json!({"sketch": "Logo", "depth": 1, "face": [20, 15, 11.25]})).unwrap_err().to_string();
    assert!(e.contains("[20, 15, 11.25] lies on no face: the nearest, on `A`, is 1.250 mm away"), "{e}");
    run(&mut s, "solid.box", json!({"length": 10, "width": 30, "height": 10, "corner": [40, 0, 0], "operation": "new_body", "body_name": "B"}));
    let e = s.execute("solid.emboss", &json!({"sketch": "Logo", "depth": 1, "face": [40, 15, 5]})).unwrap_err().to_string();
    assert!(e.contains("lies on faces of 2 bodies (A, B): name one in `targets`"), "{e}");
    run(&mut s, "solid.emboss", json!({"sketch": "Logo", "depth": 1, "face": [20, 15, 10]}));
}

fn cylinder_badge() -> Session {
    let mut s = Session::default();
    run(&mut s, "parameters.add", json!({"name":"badge_depth","expression":"1 mm"}));
    run(&mut s, "solid.cylinder", json!({"radius":"10 mm","height":"20 mm","body_name":"Sleeve"}));
    run(&mut s, "sketch.create", json!({"plane":{"origin":[0,10,0],"x_dir":[-1,0,0],"y_dir":[0,0,1]},"name":"Badge"}));
    run(&mut s, "sketch.rectangle.two_point", json!({"p0":[-2,5],"p1":[2,10]}));
    run(&mut s, "sketch.finish", json!({}));
    s
}

fn exact_cylinder_volume(s: &Session) -> f64 {
    s.model.state().body("Sleeve").unwrap().body.tessellate_with(1e-4, true).unwrap().measure().volume
}

#[test]
fn cylinder_emboss_persists_selection_and_replays_radial_depth() {
    use solidcraft_geom::Vec3;
    let mut s = cylinder_badge();
    let before = exact_cylinder_volume(&s);
    run(
        &mut s,
        "solid.emboss",
        json!({"sketch":"Badge","profiles":"all","face":[0,10,7.5],"depth":"badge_depth","targets":["Sleeve"],"name":"Raised Badge"}),
    );
    assert!((exact_cylinder_volume(&s) - before - 21.0).abs() < 0.02);
    let state = s.model.state();
    let body = &state.body("Sleeve").unwrap().body;
    assert_eq!(body.shell_count(), 1);
    let mesh = body.tessellate_with(0.001, true).unwrap();
    assert!(mesh.contains(Vec3::new(-10.5 * 0.15_f64.sin(), 10.5 * 0.15_f64.cos(), 7.5)));
    let feature = s.doc.find_feature("Raised Badge").unwrap();
    assert!(
        matches!(&feature.kind,solidcraft_doc::FeatureKind::Emboss{face:Some(face),..} if face.body=="Sleeve"&&face.at.dist(Vec3::new(0.,10.,7.5))<1e-10)
    );
    let saved = s.doc.to_json();
    let mut loaded = Session { doc: solidcraft_doc::Document::from_json(&saved).unwrap().into(), ..Default::default() };
    loaded.refresh();
    assert!(loaded.model.results.iter().all(|f| f.error.is_none()), "{:?}", loaded.model.results);
    assert!((exact_cylinder_volume(&loaded) - before - 21.0).abs() < 0.02);
    run(&mut loaded, "parameters.change", json!({"name":"badge_depth","expression":"2 mm"}));
    assert!(loaded.model.results.iter().all(|f| f.error.is_none()), "{:?}", loaded.model.results);
    assert!((exact_cylinder_volume(&loaded) - before - 44.0).abs() < 0.02);
}

#[test]
fn cylinder_deboss_is_connected_and_unsupported_profile_failure_is_atomic() {
    let mut s = cylinder_badge();
    let before = exact_cylinder_volume(&s);
    run(&mut s, "solid.emboss", json!({"sketch":"Badge","face":[0,10,7.5],"depth":1,"mode":"deboss","targets":["Sleeve"]}));
    assert!((exact_cylinder_volume(&s) - before + 19.0).abs() < 0.02);
    assert_eq!(s.model.state().body("Sleeve").unwrap().body.shell_count(), 1);
    let mut bad = cylinder_badge();
    run(&mut bad, "sketch.create", json!({"plane":{"origin":[0,10,0],"x_dir":[-1,0,0],"y_dir":[0,0,1]},"name":"Circle Badge"}));
    run(&mut bad, "sketch.circle.center", json!({"center":[0,7.5],"radius":1}));
    run(&mut bad, "sketch.finish", json!({}));
    let saved = bad.doc.to_json();
    let v = exact_cylinder_volume(&bad);
    assert!(bad.execute("solid.emboss", &json!({"sketch":"Circle Badge","face":[0,10,7.5],"depth":1,"targets":["Sleeve"]})).is_err());
    assert_eq!(bad.doc.to_json(), saved);
    assert_eq!(exact_cylinder_volume(&bad), v);
}

#[test]
fn cylinder_small_chord_pick_is_saved_on_the_exact_surface() {
    let mut s = Session::default();
    run(&mut s, "solid.cylinder", json!({"radius":"0.1mm","height":"0.2mm","body_name":"Tiny"}));
    run(&mut s, "sketch.create", json!({"plane":{"origin":[0,0.1,0],"x_dir":[-1,0,0],"y_dir":[0,0,1]},"name":"Badge"}));
    run(&mut s, "sketch.rectangle.two_point", json!({"p0":[-0.02,0.05],"p1":[0.02,0.1]}));
    run(&mut s, "sketch.finish", json!({}));
    let mesh = s.model.state().bodies.first().unwrap().mesh();
    let pick = mesh
        .triangles
        .iter()
        .filter_map(|tri| mesh.tri(tri))
        .map(|[a, b, c]| (a + b + c) / 3.)
        .find(|p| p.y > 0.08 && p.z > 0.04 && p.z < 0.16)
        .unwrap();
    assert!(((pick.x * pick.x + pick.y * pick.y).sqrt() - 0.1).abs() > 1e-7);
    run(&mut s, "solid.emboss", json!({"sketch":"Badge","profiles":"all","face":[pick.x,pick.y,pick.z],"depth":"0.01mm","targets":["Tiny"]}));
    let solidcraft_doc::FeatureKind::Emboss { face: Some(face), .. } = &s.doc.features.last().unwrap().kind else {
        panic!("missing face");
    };
    assert!(((face.at.x * face.at.x + face.at.y * face.at.y).sqrt() - 0.1).abs() < 1e-12);
    assert_eq!(s.model.state().bodies.first().unwrap().body.shell_count(), 1);
}

#[test]
fn cylinder_cross_component_emboss_keeps_source_owner_and_placements() {
    for (pick_offset, auto) in [(0., false), (80., false), (0., true), (80., true)] {
        let run = |s: &mut Session, id: &str, p: Value| run(s, id, auto_emboss_params(p, auto && matches!(id, "solid.emboss" | "timeline.redefine")));
        let mut s = Session::default();
        let a = run(&mut s, "component.create", json!({"name":"Source"}))["component"].as_u64().unwrap();
        run(&mut s, "solid.cylinder", json!({"radius":10,"height":20,"body_name":"Sleeve"}));
        let source_feature = s.model.state().bodies.first().unwrap().feature;
        let source_names = solidcraft_doc::naming::face_names(s.model.state().body("Sleeve").unwrap());
        let before = exact_cylinder_volume(&s);
        run(&mut s, "occurrence.move", json!({"occurrence":"Source:1","axis":[0,0,1],"angle":"90 deg","translate":[50,0,0]}));
        let second = run(&mut s, "occurrence.copy", json!({"component":a,"translate":[80,0,0]}))["occurrence"].as_u64().unwrap();
        run(&mut s, "component.activate", json!({"component":"root"}));
        let author = run(&mut s, "component.create", json!({"name":"Author"}))["component"].as_u64().unwrap();
        run(&mut s, "occurrence.move", json!({"occurrence":"Author:1","axis":[1,0,0],"angle":"90 deg","translate":[-100,40,30]}));
        let author_second = run(&mut s, "occurrence.copy", json!({"component":author,"translate":[0,100,0]}))["occurrence"].as_u64().unwrap();
        run(&mut s, "component.activate", json!({"component":author,"occurrence":author_second}));
        run(&mut s, "parameters.add", json!({"name":"badge_depth","expression":"1mm"}));
        run(&mut s, "sketch.create", json!({"plane":{"origin":[40.+pick_offset,0,0],"x_dir":[0,-1,0],"y_dir":[0,0,1]},"name":"Badge"}));
        run(&mut s, "sketch.rectangle.two_point", json!({"p0":[-2,5],"p1":[2,10]}));
        run(&mut s, "sketch.finish", json!({}));
        let placements = serde_json::to_value(&s.doc.occurrences).unwrap();
        let target = s
            .world_state()
            .bodies
            .iter()
            .find(|b| b.body.tessellate_with(0.001, true).unwrap().contains(solidcraft_geom::Vec3::new(40.5 + pick_offset, 0., 7.5)))
            .unwrap()
            .name
            .clone();
        run(
            &mut s,
            "solid.emboss",
            json!({"sketch":"Badge","profiles":"all","face":[40.+pick_offset,0,7.5],"depth":"badge_depth","targets":[target.clone()]}),
        );
        let solidcraft_doc::FeatureKind::Emboss { face: Some(face), .. } = &s.doc.features.last().unwrap().kind else {
            panic!("missing selection");
        };
        assert_eq!(face.path, if pick_offset == 0. { s.doc.first_path(a) } else { vec![second] });
        assert_eq!(face.author_path, vec![author_second]);
        let state = s.model.state();
        let body = state.body("Sleeve").unwrap();
        assert_eq!(body.feature, source_feature);
        assert_eq!(s.doc.body_component("Sleeve", body.feature), a);
        assert_eq!(serde_json::to_value(&s.doc.occurrences).unwrap(), placements);
        let selected = s.doc.features.last().unwrap().face_names.first().unwrap();
        assert!(!selected.is_empty() && source_names.contains(selected), "{selected:?} not in {source_names:?}");
        assert!((exact_cylinder_volume(&s) - before - 21.).abs() < 0.02);
        let world = s.world_state();
        for x in [39.5, 119.5] {
            assert!(world.bodies.iter().any(|b| b.body.tessellate_with(0.001, true).unwrap().contains(solidcraft_geom::Vec3::new(x, 0., 7.5))));
        }
        let mut loaded = Session { doc: solidcraft_doc::Document::from_json(&s.doc.to_json()).unwrap().into(), ..Default::default() };
        loaded.refresh();
        assert!(loaded.model.results.iter().all(|r| r.error.is_none()), "{:?}", loaded.model.results);
        run(&mut loaded, "parameters.change", json!({"name":"badge_depth","expression":"2mm"}));
        assert!((exact_cylinder_volume(&loaded) - before - 44.).abs() < 0.02);
        assert_eq!(loaded.doc.body_component("Sleeve", loaded.model.state().body("Sleeve").unwrap().feature), a);
        assert_eq!(serde_json::to_value(&loaded.doc.occurrences).unwrap(), placements);
        let feature = loaded.doc.features.last().unwrap().clone();
        run(&mut loaded, "component.activate", json!({"component":"root"}));
        run(
            &mut loaded,
            "timeline.redefine",
            json!({"feature":feature.id,"command":"solid.emboss","params":{"sketch":"Badge","profiles":"all","face":[40.+pick_offset,0,7.5],"depth":"badge_depth","targets":[target.clone()]}}),
        );
        assert_eq!(loaded.doc.feature(feature.id).unwrap().kind, feature.kind);
        assert_eq!(loaded.doc.feature(feature.id).unwrap().face_names, feature.face_names);
        run(
            &mut loaded,
            "timeline.redefine",
            json!({"feature":feature.id,"command":"solid.emboss","params":{"sketch":"Badge","profiles":"all","face":[40.+pick_offset,0,7.5],"depth":"3mm","targets":[target]}}),
        );
        assert!((exact_cylinder_volume(&loaded) - before - 69.).abs() < 0.02);
        let mut broken = loaded.doc.as_ref().clone();
        let id = broken.features.last().unwrap().id;
        let solidcraft_doc::FeatureKind::Emboss { face: Some(face), .. } = &mut broken.feature_mut(id).unwrap().kind else {
            panic!("missing selection");
        };
        face.path = vec![u64::MAX];
        let mut invalid = Session { doc: broken.into(), ..Default::default() };
        invalid.refresh();
        assert!(invalid.model.results.iter().find(|r| r.id == id).unwrap().error.as_ref().unwrap().contains("placement"));
        assert!((exact_cylinder_volume(&invalid) - before).abs() < 0.02);
    }
}

#[test]
fn cylinder_active_second_source_occurrence_authors_shared_geometry() {
    check_active_second_source(false);
    check_active_second_source(true);
}
fn check_active_second_source(auto: bool) {
    let run = |s: &mut Session, id: &str, p: Value| run(s, id, auto_emboss_params(p, auto && id == "solid.emboss"));
    let mut s = Session::default();
    let component = run(&mut s, "component.create", json!({"name":"Source"}))["component"].as_u64().unwrap();
    run(&mut s, "solid.cylinder", json!({"radius":10,"height":20,"body_name":"Sleeve"}));
    let before = exact_cylinder_volume(&s);
    let second = run(&mut s, "occurrence.copy", json!({"component":component,"translate":[80,0,0]}))["occurrence"].as_u64().unwrap();
    run(&mut s, "component.activate", json!({"component":component,"occurrence":second}));
    run(&mut s, "sketch.create", json!({"plane":{"origin":[80,10,0],"x_dir":[-1,0,0],"y_dir":[0,0,1]},"name":"Badge"}));
    run(&mut s, "sketch.rectangle.two_point", json!({"p0":[-2,5],"p1":[2,10]}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.emboss", json!({"sketch":"Badge","profiles":"all","face":[80,10,7.5],"depth":1}));
    assert!((exact_cylinder_volume(&s) - before - 21.).abs() < 0.02);
    let solidcraft_doc::FeatureKind::Emboss { face: Some(face), .. } = &s.doc.features.last().unwrap().kind else {
        panic!("missing selection");
    };
    assert_eq!(face.path, vec![second]);
    assert_eq!(face.author_path, vec![second]);
    let mut loaded = Session { doc: solidcraft_doc::Document::from_json(&s.doc.to_json()).unwrap().into(), ..Default::default() };
    loaded.refresh();
    assert!(loaded.model.results.iter().all(|r| r.error.is_none()), "{:?}", loaded.model.results);
    assert!((exact_cylinder_volume(&loaded) - before - 21.).abs() < 0.02);
}

#[test]
fn cylinder_pending_leaf_or_ancestor_positions_require_capture_atomically() {
    for (ancestor, auto) in [(false, false), (true, false), (false, true), (true, true)] {
        let mut s = Session::default();
        let parent = run(&mut s, "component.create", json!({"name":"Parent"}))["occurrence"].as_u64().unwrap();
        let leaf = run(&mut s, "component.create", json!({"name":"Source"}))["occurrence"].as_u64().unwrap();
        run(&mut s, "solid.cylinder", json!({"radius":10,"height":20,"body_name":"Sleeve"}));
        run(&mut s, "sketch.create", json!({"plane":{"origin":[0,10,0],"x_dir":[-1,0,0],"y_dir":[0,0,1]},"name":"Badge"}));
        run(&mut s, "sketch.rectangle.two_point", json!({"p0":[-2,5],"p1":[2,10]}));
        run(&mut s, "sketch.finish", json!({}));
        run(&mut s, "occurrence.move", json!({"occurrence":if ancestor {parent}else{leaf},"translate":[5,0,0],"capture":false}));
        let before = s.doc.to_json();
        let pending = s.pending_moves.clone();
        let volume = exact_cylinder_volume(&s);
        let error = s
            .execute("solid.emboss", &auto_emboss_params(json!({"sketch":"Badge","profiles":"all","face":[5,10,7.5],"depth":1}), auto))
            .unwrap_err()
            .to_string();
        assert!(error.contains("Capture Position"), "{error}");
        assert_eq!(s.doc.to_json(), before);
        assert_eq!(s.pending_moves, pending);
        assert_eq!(exact_cylinder_volume(&s), volume);
    }
}

#[test]
fn cylinder_pending_third_component_profile_requires_capture_atomically() {
    check_pending_third_profile(false);
    check_pending_third_profile(true);
}
fn check_pending_third_profile(auto: bool) {
    let mut s = Session::default();
    run(&mut s, "solid.cylinder", json!({"radius":10,"height":20,"body_name":"Sleeve"}));
    let profile_occurrence = run(&mut s, "component.create", json!({"name":"ProfileOwner"}))["occurrence"].as_u64().unwrap();
    run(&mut s, "sketch.create", json!({"plane":{"origin":[0,10,0],"x_dir":[-1,0,0],"y_dir":[0,0,1]},"name":"Badge"}));
    run(&mut s, "sketch.rectangle.two_point", json!({"p0":[-2,5],"p1":[2,10]}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "component.activate", json!({"component":"root"}));
    run(&mut s, "component.create", json!({"name":"Author"}));
    run(&mut s, "occurrence.move", json!({"occurrence":profile_occurrence,"translate":[5,0,0],"capture":false}));
    let before = s.doc.to_json();
    let pending = s.pending_moves.clone();
    let volume = exact_cylinder_volume(&s);
    let error = s
        .execute(
            "solid.emboss",
            &auto_emboss_params(json!({"sketch":"Badge","profiles":"all","face":[0,10,7.5],"depth":1,"targets":["Sleeve"]}), auto),
        )
        .unwrap_err()
        .to_string();
    assert!(error.contains("Capture Position"), "{error}");
    assert_eq!(s.doc.to_json(), before);
    assert_eq!(s.pending_moves, pending);
    assert_eq!(exact_cylinder_volume(&s), volume);
}

#[test]
fn cylinder_body_offset_keeps_exact_profile_and_anchor_frames() {
    check_body_offset(false);
    check_body_offset(true);
}
fn check_body_offset(auto: bool) {
    let run = |s: &mut Session, id: &str, p: Value| run(s, id, auto_emboss_params(p, auto && id == "solid.emboss"));
    let mut s = Session::default();
    run(&mut s, "solid.cylinder", json!({"radius":10,"height":20,"body_name":"Sleeve"}));
    let before = exact_cylinder_volume(&s);
    let holder = run(&mut s, "component.create", json!({"name":"Holder"}))["component"].as_u64().unwrap();
    run(&mut s, "occurrence.move", json!({"occurrence":"Holder:1","translate":[0,50,0]}));
    std::sync::Arc::make_mut(&mut s.doc).body_components.insert("Sleeve".into(), holder);
    std::sync::Arc::make_mut(&mut s.doc)
        .body_offsets
        .insert("Sleeve".into(), [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.], [0., -50., 0., 1.]]);
    s.refresh();
    assert!(s.doc.body_offsets.contains_key("Sleeve"));
    let offset = s.doc.body_offsets["Sleeve"];
    run(&mut s, "sketch.create", json!({"plane":{"origin":[0,10,0],"x_dir":[-1,0,0],"y_dir":[0,0,1]},"name":"Badge"}));
    run(&mut s, "sketch.rectangle.two_point", json!({"p0":[-2,5],"p1":[2,10]}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.emboss", json!({"sketch":"Badge","profiles":"all","face":[0,10,7.5],"depth":1,"targets":["Sleeve"]}));
    assert!((exact_cylinder_volume(&s) - before - 21.).abs() < 0.02);
    assert_eq!(s.doc.body_offsets["Sleeve"], offset);
    assert_eq!(s.doc.body_component("Sleeve", s.model.state().body("Sleeve").unwrap().feature), holder);
    assert!(s.world_state().body("Sleeve").unwrap().body.tessellate_with(0.001, true).unwrap().contains(solidcraft_geom::Vec3::new(0., 10.5, 7.5)));
}

#[test]
fn replace_face_align_remove() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10, "body_name": "A"}));
    run(&mut s, "solid.replace_face", json!({"faces": [[5, 5, 10]], "target": {"origin": [0, 0, 15], "normal": [0, 0, 1]}}));
    assert!(rel(volume(&mut s), 1500.0) < 1e-6);
    let e = s.execute("solid.replace_face", &json!({"faces": [[5, 5, 15]], "target": {"origin": [0, 0, 15], "normal": [1, 0, 1]}}));
    assert!(e.is_err() || s.model.results.last().is_some_and(|r| r.error.as_deref().unwrap_or("").contains("parallel")));
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10, "body_name": "A"}));
    run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10, "corner": [50, 0, 0], "body_name": "B"}));
    // B's top face onto A's top face: B turns over and sits on A.
    run(&mut s, "solid.align", json!({"bodies": ["B"], "from_face": [55, 5, 10], "to_face": [5, 5, 10]}));
    let m = measure(&mut s);
    let b = m["bodies"].as_array().unwrap().iter().find(|b| b["name"] == "B").unwrap().clone();
    let (lo, hi) = (b["bbox"]["min"].clone(), b["bbox"]["max"].clone());
    for (got, want) in [(&lo[0], 0.0), (&lo[2], 10.0), (&hi[0], 10.0), (&hi[2], 20.0)] {
        assert!((got.as_f64().unwrap_or(f64::NAN) - want).abs() < 1e-6, "{b}");
    }
    // Points only: a plain move.
    run(&mut s, "solid.align", json!({"bodies": ["B"], "from": [0, 0, 10], "to": [0, 0, 30]}));
    let m = measure(&mut s);
    let b = m["bodies"].as_array().unwrap().iter().find(|b| b["name"] == "B").unwrap().clone();
    assert!((b["bbox"]["min"][2].as_f64().unwrap_or(0.0) - 30.0).abs() < 1e-6, "{b}");
    run(&mut s, "solid.remove", json!({"bodies": ["B"]}));
    assert_eq!(measure(&mut s)["body_count"], 1);
}

#[test]
fn coil_and_loft_to_a_point() {
    use std::f64::consts::{PI, TAU};
    let mut s = Session::default();
    run(&mut s, "solid.coil", json!({"diameter": 40, "revolutions": 2, "pitch": 8, "section_size": 4}));
    assert!(rel(volume(&mut s), PI * 4.0 * TAU * 20.0 * 2.0) < 1e-3, "{}", volume(&mut s));
    // Height and revolutions give the pitch; a square section outside the diameter.
    let mut s = Session::default();
    run(
        &mut s,
        "solid.coil",
        json!({"diameter": 40, "revolutions": 1.5, "height": 15, "section": "square", "section_size": 2, "section_position": "outside", "axis": "X"}),
    );
    assert!(rel(volume(&mut s), 4.0 * TAU * 21.0 * 1.5) < 1e-3, "{}", volume(&mut s));
    assert!(s.execute("solid.coil", &json!({"diameter": 40, "pitch": 1, "revolutions": 3, "section_size": 4})).is_err(), "runs into itself");
    assert!(s.execute("solid.coil", &json!({"diameter": 40, "section_size": 4, "pitch": 8})).is_err(), "two of three");
    // Pyramid: a square lofted to a point.
    let mut s = Session::default();
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Base"}));
    run(&mut s, "sketch.rectangle.two_point", json!({"p0": [-10, -10], "p1": [10, 10]}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "sketch.create", json!({"plane": {"origin": [0, 0, 30], "x_dir": [1, 0, 0], "y_dir": [0, 1, 0]}, "name": "Top"}));
    run(&mut s, "sketch.point", json!({"point": [0, 0], "id": "apex"}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.loft", json!({"sections": [{"sketch": "Base"}, {"sketch": "Top", "point": "apex"}]}));
    assert!(rel(volume(&mut s), 400.0 * 30.0 / 3.0) < 1e-6);
    assert!(s.execute("solid.loft", &json!({"sections": [{"sketch": "Base"}, {"sketch": "Top", "point": "nope"}]})).is_err());
}

/// A 3 mm shelled basin (100 x 60 x 30, open top) with a fall lofted from its whole outline
/// 10 up down to a drain pad (`pad`: corners, 1 up), in either section order. The loft's top
/// edges lie on the outer walls; the cut used to fail for one order of the sections (an
/// `Option::unwrap()` panic inside the kernel, or "shell not oriented and closed").
fn basin_fall(pad: [f64; 4], reverse: bool, op: &str) -> (Session, std::result::Result<Value, String>) {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 100, "width": 60, "height": 30, "corner": [0, 0, 0]}));
    run(&mut s, "solid.shell", json!({"faces": [[50, 30, 30]], "thickness": 3}));
    run(&mut s, "sketch.create", json!({"plane": {"origin": [0, 0, 10], "x_dir": [1, 0, 0], "y_dir": [0, 1, 0]}, "name": "Rim"}));
    run(&mut s, "sketch.rectangle.two_point", json!({"p0": [0, 0], "p1": [100, 60]}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "sketch.create", json!({"plane": {"origin": [0, 0, 1], "x_dir": [1, 0, 0], "y_dir": [0, 1, 0]}, "name": "Pad"}));
    run(&mut s, "sketch.rectangle.two_point", json!({"p0": [pad[0], pad[1]], "p1": [pad[2], pad[3]]}));
    run(&mut s, "sketch.finish", json!({}));
    let mut secs = vec![json!({"sketch": "Rim"}), json!({"sketch": "Pad"})];
    if reverse {
        secs.reverse();
    }
    let r = s.execute("solid.loft", &json!({"sections": secs, "operation": op})).map_err(|e| e.to_string());
    (s, r)
}

#[test]
fn loft_cut_through_a_shelled_basin_in_either_order() {
    // What is left after the cut: the basin less the loft's part of the floor and walls
    // (integrated over the height), the same for a pad in any corner.
    let want = 41582.028;
    for pad in [[80.0, 40.0, 90.0, 50.0], [10.0, 40.0, 20.0, 50.0], [10.0, 10.0, 20.0, 20.0], [80.0, 10.0, 90.0, 20.0]] {
        for reverse in [false, true] {
            let (mut s, r) = basin_fall(pad, reverse, "cut");
            assert!(r.is_ok(), "{pad:?} reverse {reverse}: {r:?}");
            let v = volume(&mut s);
            assert!((v - want).abs() < 1.0, "{pad:?} reverse {reverse}: {v} vs {want}");
            assert_eq!(measure(&mut s)["body_count"], 1);
        }
    }
    // As a new body and joined: a valid solid of positive volume either way.
    for op in ["new", "join"] {
        let mut vols = Vec::new();
        for reverse in [false, true] {
            let (mut s, r) = basin_fall([80.0, 40.0, 90.0, 50.0], reverse, op);
            assert!(r.is_ok(), "{op} reverse {reverse}: {r:?}");
            vols.push(volume(&mut s));
        }
        assert!(vols[0] > 42948.0 && rel(vols[0], vols[1]) < 1e-6, "{op}: {vols:?}");
    }
}

#[test]
fn cylinder_auto_inference_saves_a_profile_anchor_and_replays_both_depth_signs() {
    use solidcraft_geom::Vec3;
    for mode in ["emboss", "deboss"] {
        let mut s = cylinder_badge();
        let before = exact_cylinder_volume(&s);
        let initial = s.doc.to_json();
        run(&mut s, "solid.emboss", json!({"sketch":"Badge","profiles":"all","depth":"badge_depth","mode":mode,"name":"Auto Badge"}));
        let feature = s.doc.find_feature("Auto Badge").unwrap().clone();
        let solidcraft_doc::FeatureKind::Emboss { face: Some(face), targets, .. } = &feature.kind else {
            panic!("missing inferred association");
        };
        assert_eq!(targets, &["Sleeve"]);
        assert_eq!(face.body, "Sleeve");
        assert!(face.at.z > 0. && face.at.z < 20.);
        assert!((face.at.x.hypot(face.at.y) - 10.).abs() < 1e-10);
        assert!(!feature.face_names.first().unwrap().is_empty());
        let expected = if mode == "emboss" { 21. } else { -19. };
        assert!((exact_cylinder_volume(&s) - before - expected).abs() < 0.02);
        let state = s.model.state();
        let body = &state.body("Sleeve").unwrap().body;
        assert_eq!(body.shell_count(), 1);
        assert_eq!(
            body.tessellate_with(0.001, true).unwrap().contains(Vec3::new(0., if mode == "emboss" { 10.5 } else { 9.5 }, 7.5)),
            mode == "emboss"
        );
        run(&mut s, "edit.undo", json!({}));
        assert_eq!(s.doc.to_json(), initial);
        run(&mut s, "edit.redo", json!({}));
        assert_eq!(s.doc.find_feature("Auto Badge").unwrap().kind, feature.kind);
        let mut loaded = Session { doc: solidcraft_doc::Document::from_json(&s.doc.to_json()).unwrap().into(), ..Default::default() };
        loaded.refresh();
        assert!(loaded.model.results.iter().all(|r| r.error.is_none()), "{:?}", loaded.model.results);
        run(
            &mut loaded,
            "timeline.redefine",
            json!({"feature":feature.id,"command":"solid.emboss","params":{"sketch":"Badge","profiles":"all","depth":"badge_depth","mode":mode}}),
        );
        assert_eq!(loaded.doc.feature(feature.id).unwrap().kind, feature.kind);
        assert_eq!(loaded.doc.feature(feature.id).unwrap().face_names, feature.face_names);
        run(&mut loaded, "parameters.change", json!({"name":"badge_depth","expression":"2mm"}));
        assert!((exact_cylinder_volume(&loaded) - before - if mode == "emboss" { 44. } else { -36. }).abs() < 0.02);
    }
}

#[test]
fn cylinder_auto_inference_ignores_remote_finite_tangent_walls_in_each_body_order() {
    for remote_first in [false, true] {
        let mut s = Session::default();
        for (name, z) in if remote_first { [("Remote", 100.), ("Sleeve", 0.)] } else { [("Sleeve", 0.), ("Remote", 100.)] } {
            run(&mut s, "solid.cylinder", json!({"radius":10,"height":20,"base":[0,0,z],"body_name":name}));
        }
        run(&mut s, "solid.cylinder", json!({"radius":8,"height":10,"base":[0,0,120],"body_name":"Remote Step"}));
        run(&mut s, "solid.combine", json!({"target":"Remote","tools":["Remote Step"],"operation":"join"}));
        run(&mut s, "sketch.create", json!({"plane":{"origin":[0,10,0],"x_dir":[-1,0,0],"y_dir":[0,0,1]},"name":"Badge"}));
        run(&mut s, "sketch.rectangle.two_point", json!({"p0":[-2,5],"p1":[2,10]}));
        run(&mut s, "sketch.finish", json!({}));
        let before = exact_cylinder_volume(&s);
        let remote = s.model.state().body("Remote").unwrap().body.tessellate_with(1e-4, true).unwrap().measure().volume;
        run(&mut s, "solid.emboss", json!({"sketch":"Badge","depth":1}));
        assert!((exact_cylinder_volume(&s) - before - 21.).abs() < 0.02);
        assert_eq!(s.model.state().body("Remote").unwrap().body.tessellate_with(1e-4, true).unwrap().measure().volume, remote);
    }
}

#[test]
fn cylinder_auto_inference_retains_selected_holes_and_nested_islands() {
    use solidcraft_geom::Vec3;
    for mode in ["emboss", "deboss"] {
        let mut s = cylinder_badge();
        run(&mut s, "sketch.edit", json!({"sketch":"Badge"}));
        run(&mut s, "sketch.rectangle.two_point", json!({"p0":[-1,6],"p1":[1,9]}));
        run(&mut s, "sketch.rectangle.two_point", json!({"p0":[-0.5,6.5],"p1":[0.5,8.5]}));
        run(&mut s, "sketch.finish", json!({}));
        let before = exact_cylinder_volume(&s);
        run(&mut s, "solid.emboss", json!({"sketch":"Badge","profiles":[{"point":[1.5,7.5]},{"point":[0,7.5]}],"depth":1,"mode":mode}));
        let depth = if mode == "emboss" { 1. } else { -1. };
        let expected = 16. * (depth + depth * depth / 20.);
        assert!((exact_cylinder_volume(&s) - before - expected).abs() < 0.02);
        let st = s.model.state();
        let body = &st.body("Sleeve").unwrap().body;
        assert_eq!(body.shell_count(), 1);
        let mesh = body.tessellate_with(0.001, true).unwrap();
        for (s, raised) in [(0., true), (0.75, false), (1.5, true)] {
            let theta = s / 10.0_f64;
            let radius = if mode == "emboss" { 10.5 } else { 9.5 };
            assert_eq!(mesh.contains(Vec3::new(-radius * theta.sin(), radius * theta.cos(), 7.5)), if mode == "emboss" { raised } else { !raised });
        }
    }
}

#[test]
fn cylinder_auto_inference_refuses_ambiguity_curves_and_seam_crossing_atomically() {
    let mut s = cylinder_badge();
    run(&mut s, "solid.cylinder", json!({"radius":10,"height":20,"body_name":"Coincident"}));
    let before = s.doc.to_json();
    let error = s.execute("solid.emboss", &json!({"sketch":"Badge","depth":1})).unwrap_err().to_string();
    assert!(error.contains("multiple tangent cylinders"), "{error}");
    assert_eq!(s.doc.to_json(), before);
    run(&mut s, "solid.emboss", json!({"sketch":"Badge","depth":1,"targets":["Sleeve"]}));
    assert!(matches!(&s.doc.features.last().unwrap().kind,solidcraft_doc::FeatureKind::Emboss{face:Some(face),..} if face.body=="Sleeve"));
    for circle in [false, true] {
        let mut s = cylinder_badge();
        run(&mut s, "sketch.create", json!({"plane":{"origin":[0,10,0],"x_dir":[-1,0,0],"y_dir":[0,0,1]},"name":"Unsupported"}));
        if circle {
            run(&mut s, "sketch.circle.center", json!({"center":[0,7.5],"radius":1}));
        } else {
            run(&mut s, "sketch.rectangle.two_point", json!({"p0":[-20,5],"p1":[20,10]}));
        }
        run(&mut s, "sketch.finish", json!({}));
        let before = s.doc.to_json();
        let volume = exact_cylinder_volume(&s);
        let error = s.execute("solid.emboss", &json!({"sketch":"Unsupported","depth":1})).unwrap_err().to_string();
        assert!(error.contains(if circle { "orthogonal line" } else { "seam" }), "{error}");
        assert_eq!(s.doc.to_json(), before);
        assert_eq!(exact_cylinder_volume(&s), volume);
    }
}

/// The design from #396: a loft from a Ø12 circle on a 40 × 30 × 20 box's top to a hexagon 15 up.
fn loft_on_box(op: &str) -> Session {
    let mut s = Session::default();
    run(&mut s, "sketch.create", json!({"plane": "XY"}));
    run(&mut s, "sketch.rectangle.center", json!({"center": [0, 0], "corner": [20, 15]}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.extrude", json!({"distance": 20, "body_name": "Box"}));
    run(&mut s, "construct.plane.offset", json!({"base": "XY", "offset": 20, "name": "A"}));
    run(&mut s, "construct.plane.offset", json!({"base": "XY", "offset": 35, "name": "B"}));
    run(&mut s, "sketch.create", json!({"plane": "A", "name": "L0"}));
    run(&mut s, "sketch.circle.center", json!({"center": [0, 0], "diameter": 12}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "sketch.create", json!({"plane": "B", "name": "L1"}));
    run(&mut s, "sketch.polygon.inscribed", json!({"center": [0, 0], "radius": 4, "sides": 6}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.loft", json!({"sections": [{"sketch": "L0"}, {"sketch": "L1"}], "operation": op}));
    s
}

#[test]
fn loft_joined_onto_a_box_exports_one_connected_mesh() {
    // #396: one body, but its two shells only touched, and the STL came out in two pieces.
    let mut apart = loft_on_box("new_body");
    let m = measure(&mut apart);
    assert_eq!(m["body_count"], 2);
    let loft = m["bodies"][1]["volume_mm3"].as_f64().unwrap();
    let mut s = loft_on_box("join");
    let m = measure(&mut s);
    assert_eq!(m["body_count"], 1);
    assert_eq!(m["bodies"][0]["kernel"]["shells"], 1);
    let v = volume(&mut s);
    assert!(rel(v, 24000.0 + loft) < 1e-5, "{v} vs {}", 24000.0 + loft);
    // The exported mesh, its vertices merged, is one connected piece.
    let mesh = s.model.state().body("Box").unwrap().body.tessellate(0.01).unwrap();
    let (positions, triangles) = solidcraft_io::weld(&mesh.positions, &mesh.triangles, 1e-4);
    let mut piece: Vec<usize> = (0..positions.len()).collect();
    let root = |piece: &mut Vec<usize>, mut i: usize| {
        while piece[i] != i {
            i = piece[i];
        }
        i
    };
    for t in &triangles {
        let r0 = root(&mut piece, t[0] as usize);
        for k in [1, 2] {
            let rk = root(&mut piece, t[k] as usize);
            piece[rk] = r0;
        }
    }
    let pieces: std::collections::BTreeSet<usize> = triangles.iter().map(|t| root(&mut piece, t[0] as usize)).collect();
    assert_eq!(pieces.len(), 1);
}

/// A cone that takes only a 1 mm chamfer off a cylinder's top edge leaves nearly all of the
/// cylinder inside it; the intersection is still the chamfered cylinder (#443).
#[test]
fn loft_intersect_that_trims_one_edge_changes_the_body() {
    let mut s = Session::default();
    run(&mut s, "sketch.create", json!({"plane": "XY"}));
    run(&mut s, "sketch.circle.center", json!({"center": [0, 0], "radius": 35}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.extrude", json!({"distance": 10, "body_name": "Body"}));
    for (name, offset, radius) in [("A", -1, 45), ("B", 11, 33)] {
        run(&mut s, "sketch.create", json!({"plane": "XY", "offset": offset, "name": name}));
        run(&mut s, "sketch.circle.center", json!({"center": [0, 0], "radius": radius}));
        run(&mut s, "sketch.finish", json!({}));
    }
    run(&mut s, "solid.loft", json!({"sections": [{"sketch": "A"}, {"sketch": "B"}], "operation": "intersect"}));
    // r 35 up to z = 9, then a frustum to r 34 at z = 10.
    let want = std::f64::consts::PI * (35.0 * 35.0 * 9.0 + (35.0 * 35.0 + 35.0 * 34.0 + 34.0 * 34.0) / 3.0);
    let got = volume(&mut s);
    assert!((got - want).abs() < want * 2e-4, "{got} vs {want}");
    let m = measure(&mut s);
    assert_eq!(m["bodies"][0]["face_types"]["cone"], 1, "{m}");
}

/// #623: a moulded front frame (rounded block, recess, through opening, four pockets from the back, two joined ribs,
/// one joined Ø7 boss) and a Ø3.2 x 8 blind bore into the boss. Reduced from the issue's script; on 0.7.0 the bore
/// fails on every run with "lumps: cannot mesh a complete shell boundary" (the original failed on about half).
const RECIPE_623: &str = r#"{"commands":[{"command":"component.activate","params":{"component":"root"}},{"command":"sketch.create","params":{"plane":{"origin":[466.0,0,0],"x_dir":[0,1,0],"y_dir":[0,0,1]},"name":"PL01 Outline Sketch"}},{"command":"sketch.line","params":{"points":[[9.0,17.35],[231.0,17.35]],"closed":false}},{"command":"sketch.arc.center_point","params":{"center":[231.0,25.5],"start":[231.0,17.35],"end":[239.15,25.5]}},{"command":"sketch.line","params":{"points":[[239.15,25.5],[239.15,500.0]],"closed":false}},{"command":"sketch.arc.center_point","params":{"center":[231.0,500.0],"start":[239.15,500.0],"end":[231.0,508.15]}},{"command":"sketch.line","params":{"points":[[231.0,508.15],[9.0,508.15]],"closed":false}},{"command":"sketch.arc.center_point","params":{"center":[9.0,500.0],"start":[9.0,508.15],"end":[0.8500000000000001,500.0]}},{"command":"sketch.line","params":{"points":[[0.8500000000000001,500.0],[0.8500000000000001,25.5]],"closed":false}},{"command":"sketch.arc.center_point","params":{"center":[9.0,25.5],"start":[0.8500000000000001,25.5],"end":[9.0,17.35]}},{"command":"sketch.finish","params":{}},{"command":"solid.extrude","params":{"sketch":"PL01 Outline Sketch","distance":30.0,"operation":"new","direction":"positive","name":"PL01 Outline","body_name":"PL01 Front Frame"}},{"command":"sketch.create","params":{"plane":{"origin":[496.0,0,0],"x_dir":[0,1,0],"y_dir":[0,0,1]},"name":"PL01 Recess Sketch"}},{"command":"sketch.line","params":{"points":[[10.0,22.5],[230.0,22.5]],"closed":false}},{"command":"sketch.arc.center_point","params":{"center":[230.0,26.5],"start":[230.0,22.5],"end":[234.0,26.5]}},{"command":"sketch.line","params":{"points":[[234.0,26.5],[234.0,499.0]],"closed":false}},{"command":"sketch.arc.center_point","params":{"center":[230.0,499.0],"start":[234.0,499.0],"end":[230.0,503.0]}},{"command":"sketch.line","params":{"points":[[230.0,503.0],[10.0,503.0]],"closed":false}},{"command":"sketch.arc.center_point","params":{"center":[10.0,499.0],"start":[10.0,503.0],"end":[6.0,499.0]}},{"command":"sketch.line","params":{"points":[[6.0,499.0],[6.0,26.5]],"closed":false}},{"command":"sketch.arc.center_point","params":{"center":[10.0,26.5],"start":[6.0,26.5],"end":[10.0,22.5]}},{"command":"sketch.finish","params":{}},{"command":"solid.extrude","params":{"sketch":"PL01 Recess Sketch","distance":12.0,"operation":"cut","direction":"negative","name":"PL01 Recess","targets":["PL01 Front Frame"]}},{"command":"sketch.create","params":{"plane":{"origin":[484.0,0,0],"x_dir":[0,1,0],"y_dir":[0,0,1]},"name":"PL01 Opening Sketch"}},{"command":"sketch.line","params":{"points":[[20.5,36.0],[219.5,36.0]],"closed":false}},{"command":"sketch.arc.center_point","params":{"center":[219.5,38.0],"start":[219.5,36.0],"end":[221.5,38.0]}},{"command":"sketch.line","params":{"points":[[221.5,38.0],[221.5,488.5]],"closed":false}},{"command":"sketch.arc.center_point","params":{"center":[219.5,488.5],"start":[221.5,488.5],"end":[219.5,490.5]}},{"command":"sketch.line","params":{"points":[[219.5,490.5],[20.5,490.5]],"closed":false}},{"command":"sketch.arc.center_point","params":{"center":[20.5,488.5],"start":[20.5,490.5],"end":[18.5,488.5]}},{"command":"sketch.line","params":{"points":[[18.5,488.5],[18.5,38.0]],"closed":false}},{"command":"sketch.arc.center_point","params":{"center":[20.5,38.0],"start":[18.5,38.0],"end":[20.5,36.0]}},{"command":"sketch.finish","params":{}},{"command":"solid.extrude","params":{"sketch":"PL01 Opening Sketch","distance":18.0,"operation":"cut","direction":"negative","name":"PL01 Opening","targets":["PL01 Front Frame"]}},{"command":"sketch.create","params":{"plane":{"origin":[466.0,0,0],"x_dir":[0,1,0],"y_dir":[0,0,1]},"name":"PL01 Hollow 1 Sketch"}},{"command":"sketch.rectangle.two_point","params":{"p0":[3.35,28.85],"p1":[16.0,496.65]}},{"command":"sketch.finish","params":{}},{"command":"solid.extrude","params":{"sketch":"PL01 Hollow 1 Sketch","distance":15.5,"operation":"cut","direction":"positive","name":"PL01 Hollow 1","targets":["PL01 Front Frame"]}},{"command":"sketch.create","params":{"plane":{"origin":[466.0,0,0],"x_dir":[0,1,0],"y_dir":[0,0,1]},"name":"PL01 Hollow 2 Sketch"}},{"command":"sketch.rectangle.two_point","params":{"p0":[224.0,28.85],"p1":[236.65,496.65]}},{"command":"sketch.finish","params":{}},{"command":"solid.extrude","params":{"sketch":"PL01 Hollow 2 Sketch","distance":15.5,"operation":"cut","direction":"positive","name":"PL01 Hollow 2","targets":["PL01 Front Frame"]}},{"command":"sketch.create","params":{"plane":{"origin":[466.0,0,0],"x_dir":[0,1,0],"y_dir":[0,0,1]},"name":"PL01 Hollow 3 Sketch"}},{"command":"sketch.rectangle.two_point","params":{"p0":[12.35,19.85],"p1":[227.65,33.5]}},{"command":"sketch.finish","params":{}},{"command":"solid.extrude","params":{"sketch":"PL01 Hollow 3 Sketch","distance":15.5,"operation":"cut","direction":"positive","name":"PL01 Hollow 3","targets":["PL01 Front Frame"]}},{"command":"sketch.create","params":{"plane":{"origin":[466.0,0,0],"x_dir":[0,1,0],"y_dir":[0,0,1]},"name":"PL01 Hollow 4 Sketch"}},{"command":"sketch.rectangle.two_point","params":{"p0":[12.35,493.0],"p1":[227.65,505.65]}},{"command":"sketch.finish","params":{}},{"command":"solid.extrude","params":{"sketch":"PL01 Hollow 4 Sketch","distance":15.5,"operation":"cut","direction":"positive","name":"PL01 Hollow 4","targets":["PL01 Front Frame"]}},{"command":"sketch.create","params":{"plane":{"origin":[471.0,0,0],"x_dir":[0,1,0],"y_dir":[0,0,1]},"name":"PL01 Rib 1 Sketch"}},{"command":"sketch.rectangle.two_point","params":{"p0":[16.0,69.0],"p1":[224.0,73.0]}},{"command":"sketch.finish","params":{}},{"command":"solid.extrude","params":{"sketch":"PL01 Rib 1 Sketch","distance":12.0,"operation":"join","direction":"positive","name":"PL01 Rib 1","targets":["PL01 Front Frame"]}},{"command":"sketch.create","params":{"plane":{"origin":[471.0,0,0],"x_dir":[0,1,0],"y_dir":[0,0,1]},"name":"PL01 Rib 6 Sketch"}},{"command":"sketch.rectangle.two_point","params":{"p0":[16.0,470.0],"p1":[224.0,474.0]}},{"command":"sketch.finish","params":{}},{"command":"solid.extrude","params":{"sketch":"PL01 Rib 6 Sketch","distance":12.0,"operation":"join","direction":"positive","name":"PL01 Rib 6","targets":["PL01 Front Frame"]}},{"command":"sketch.create","params":{"plane":{"origin":[481.5,0,0],"x_dir":[0,1,0],"y_dir":[0,0,1]},"name":"PL01 Boss 4 Sketch"}},{"command":"sketch.circle.center","params":{"center":[227.5,492.0],"radius":3.5}},{"command":"sketch.finish","params":{}},{"command":"solid.extrude","params":{"sketch":"PL01 Boss 4 Sketch","distance":24.5,"operation":"join","direction":"negative","name":"PL01 Boss 4","targets":["PL01 Front Frame"]}},{"command":"sketch.create","params":{"plane":{"origin":[457.0,0,0],"x_dir":[0,1,0],"y_dir":[0,0,1]},"name":"B"}},{"command":"sketch.circle.center","params":{"center":[227.5,492.0],"radius":1.6}},{"command":"sketch.finish","params":{}},{"command":"solid.extrude","params":{"sketch":"B","distance":8.0,"operation":"cut","direction":"positive","name":"B","targets":["PL01 Front Frame"]}}]}"#;

#[test]
fn blind_bore_into_a_joined_boss_meshes() {
    let recipe: Value = serde_json::from_str(RECIPE_623).unwrap();
    let mut s = Session::default();
    for c in recipe["commands"].as_array().unwrap() {
        run(&mut s, c["command"].as_str().unwrap(), c["params"].clone());
    }
    let m = measure(&mut s);
    assert_eq!(m["bodies"].as_array().map(Vec::len), Some(1), "{m}");
}

#[test]
fn cutting_a_text_removes_each_letter_whether_its_letters_stand_apart_or_touch() {
    // Letters apart (cut at once by all of them), and two touching blocks (cut one after the other).
    for (text, touching) in [("SOLIDCRAFT", false), ("", true)] {
        let mut s = Session::default();
        run(&mut s, "sketch.create", json!({"plane":"XY"}));
        run(&mut s, "sketch.rectangle.two_point", json!({"p0":[0,0],"p1":[200,40]}));
        run(&mut s, "sketch.finish", json!({}));
        run(&mut s, "solid.extrude", json!({"distance":10}));
        run(&mut s, "sketch.create", json!({"plane":"XY"}));
        if touching {
            run(&mut s, "sketch.rectangle.two_point", json!({"p0":[10,10],"p1":[20,20]}));
            run(&mut s, "sketch.rectangle.two_point", json!({"p0":[20,12],"p1":[30,18]}));
        } else {
            run(&mut s, "sketch.text", json!({"text":text,"at":[5,12],"height":14}));
        }
        let state = s.world_state();
        let ss = state.sketch(s.active_sketch.unwrap()).unwrap();
        // The profiles cut: the letters, not their counters (the inside of an O, whose outline
        // is a hole of its letter).
        let sorted = |ids: &[String]| {
            let mut ids = ids.to_vec();
            ids.sort();
            ids
        };
        let counter = |p: &solidcraft_sketch::Profile| ss.profiles.iter().any(|q| q.hole_curves.iter().any(|h| sorted(h) == sorted(&p.outer_curves)));
        let area: f64 = ss.profiles.iter().filter(|p| !counter(p)).map(|p| p.area).sum();
        run(&mut s, "sketch.finish", json!({}));
        run(&mut s, "solid.extrude", json!({"distance":2,"operation":"cut"}));
        let volume = run(&mut s, "inspect.measure", json!({}))["total"]["volume_mm3"].as_f64().unwrap();
        assert!((volume - (200.0 * 40.0 * 10.0 - 2.0 * area)).abs() < 1e-3 * area, "{text:?}: {volume} for {area} mm² cut 2 mm deep");
        assert_eq!(s.world_state().bodies.len(), 1);
    }
}

/// A text joined onto a block with a taper: its letters are spline and conic outlines whose
/// sides slope, standing on the block's face (run on into it) or hanging over its edge (a
/// straight foot first). An outward taper adds a little more than the straight text; one
/// steeper than the letters' strokes is refused, saying so.
#[test]
fn a_tapered_text_joins_a_block_on_its_face_or_hanging_over_its_edge() {
    for (at, hanging) in [([5.0, 12.0], false), ([150.0, 12.0], true)] {
        let mut s = Session::default();
        run(&mut s, "sketch.create", json!({"plane":"XY"}));
        run(&mut s, "sketch.rectangle.two_point", json!({"p0":[0,0],"p1":[200,40]}));
        run(&mut s, "sketch.finish", json!({}));
        run(&mut s, "solid.extrude", json!({"distance":-10}));
        run(&mut s, "sketch.create", json!({"plane":"XY"}));
        run(&mut s, "sketch.text", json!({"text":"tres long","at":at,"height":8}));
        let state = s.world_state();
        let ss = state.sketch(s.active_sketch.unwrap()).unwrap();
        // The letters' area, counters left out (a letter's hole is its own profile too).
        let sorted = |ids: &[String]| {
            let mut ids = ids.to_vec();
            ids.sort();
            ids
        };
        let counter = |p: &solidcraft_sketch::Profile| ss.profiles.iter().any(|q| q.hole_curves.iter().any(|h| sorted(h) == sorted(&p.outer_curves)));
        let area: f64 = ss.profiles.iter().filter(|p| !counter(p)).map(|p| p.area).sum();
        run(&mut s, "sketch.finish", json!({}));
        let steep = s.execute("solid.extrude", &json!({"distance":2,"operation":"join","taper":"15 deg"})).unwrap_err();
        assert!(steep.to_string().contains("the taper closes the profile"), "{steep}");
        run(&mut s, "solid.extrude", json!({"distance":2,"operation":"join","taper":"3 deg"}));
        let volume = run(&mut s, "inspect.measure", json!({}))["total"]["volume_mm3"].as_f64().unwrap();
        let (block, text) = (200.0 * 40.0 * 10.0, 2.0 * area);
        assert!(volume > block + text && volume < block + 1.3 * text, "hanging {hanging}: {volume} for {text} mm³ of straight text");
        assert_eq!(s.world_state().bodies.len(), 1);
    }
}

/// A tapered foot extruded 3 mm down from a part's bottom face and joined (#1534). Run on into
/// the part with their slope, sides that narrow the foot away from the face widened it past the
/// face: a box got a lip round its walls (0.16 mm wide, and nothing said), a cylinder's join found
/// no result, and sides that widen the foot met the face's edge there. A foot on the whole face is
/// sewn on along it; one drawn just inside the face's edge starts on it. Either way: the part plus
/// the frustum, and nothing beside the part's walls.
#[test]
fn a_tapered_foot_under_a_part_joins_as_drawn() {
    // (round, the foot's half width: the whole face, or just inside its edge)
    for (round, half) in [(false, 15.0), (true, 15.0), (false, 14.9)] {
        for taper in [-20.0f64, 20.0] {
            let t = taper.to_radians().tan();
            let mut s = Session::default();
            run(&mut s, "sketch.create", json!({"plane":"XY"}));
            if round {
                run(&mut s, "sketch.circle.center", json!({"center":[0,0],"radius":15}));
            } else {
                run(&mut s, "sketch.rectangle.two_point", json!({"p0":[-15,-15],"p1":[15,15]}));
            }
            run(&mut s, "sketch.finish", json!({}));
            run(&mut s, "solid.extrude", json!({"distance":10,"body_name":"B"}));
            let taper_deg = format!("{taper} deg");
            if half < 15.0 {
                run(&mut s, "sketch.create", json!({"plane":"XY"}));
                run(&mut s, "sketch.rectangle.two_point", json!({"p0":[-half,-half],"p1":[half,half]}));
                run(&mut s, "sketch.finish", json!({}));
                run(&mut s, "solid.extrude", json!({"distance":3,"direction":"negative","operation":"join","taper":taper_deg}));
            } else {
                let foot = json!({"face":[5,1,0],"face_body":"B","distance":3,"direction":"positive","operation":"join","taper":taper_deg});
                run(&mut s, "solid.extrude", foot);
            }
            let (part, frustum) = if round {
                let r = 15.0 + 3.0 * t;
                (std::f64::consts::PI * 225.0 * 10.0, std::f64::consts::PI * (225.0 + 15.0 * r + r * r))
            } else {
                let side = 2.0 * half + 6.0 * t;
                (9000.0, 4.0 * half * half + 2.0 * half * side + side * side)
            };
            let v = volume(&mut s);
            // The cylinder is measured on its mesh; a foot that starts on the face stands straight
            // for a hair first.
            let tol = if round || half < 15.0 { 2e-4 } else { 1e-9 } * (part + frustum);
            assert!((v - (part + frustum)).abs() < tol, "round {round}, half {half}, taper {taper}: {v} for {}", part + frustum);
            assert_eq!(s.world_state().bodies.len(), 1);
            let state = s.model.state();
            let mesh = state.body("B").unwrap().mesh();
            // Beside the part, above the foot's start: nothing.
            let out = |x: f64, y: f64| if round { x.hypot(y) } else { x.abs().max(y.abs()) } > 15.0 + 1e-6;
            let beside = mesh.positions.iter().filter(|p| p.z > 1e-6 && out(p.x, p.y)).count();
            assert_eq!(beside, 0, "round {round}, half {half}, taper {taper}: {:?}", mesh.bounds());
        }
    }
}

/// A rounded part as drawn in the app: a rectangle and a quarter circle tangent to its top
/// line, extruded together. The arc (about (62.17, -21), radius 21) ends square at the front
/// face and in a knife-edge tip where it runs into the back face.
fn knife_edge_part() -> Session {
    let mut s = Session::default();
    run(&mut s, "sketch.create", json!({"plane": "XY"}));
    run(&mut s, "sketch.rectangle.two_point", json!({"p0": [0.0, 0.0], "p1": [41.5, -21.0]}));
    run(&mut s, "sketch.dimension", json!({"entities": ["l2"], "text_at": [36.5, -8.5], "value": "21 mm"}));
    run(&mut s, "sketch.line", json!({"infer": true, "points": ["p2", [61.5, -21.0]]}));
    run(&mut s, "sketch.dimension", json!({"entities": ["l5"], "text_at": [54.5, -26.5], "value": "21"}));
    run(&mut s, "sketch.circle.center", json!({"center": "l5.end", "radius": 21.0}));
    run(&mut s, "sketch.line", json!({"infer": true, "points": ["p3", [62.166666666666664, 0.0]]}));
    run(&mut s, "sketch.constraint.tangent", json!({"a": "c1", "b": "l6"}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.extrude", json!({"direction": "positive", "distance": "10 mm", "operation": "new", "profiles": [0, 1], "sketch": 1}));
    s
}

/// The volume of the session's bodies through the display plan (the measure's mesh cuts across
/// the concave arc). It holds to about 1e-3 of what a blend removes here; the bounds below are
/// about twice what each test reads.
fn planned(s: &Session) -> f64 {
    s.model.state().bodies.iter().map(|b| b.body.display_plan(2e-4).unwrap().evaluate().measure().volume).sum()
}

/// The middle of the part's arc.
const KNIFE_ARC_MID: [f64; 2] = [62.166666666666664 - 21.0 * std::f64::consts::FRAC_1_SQRT_2, -21.0 + 21.0 * std::f64::consts::FRAC_1_SQRT_2];

/// Simpson's rule over the round's corner (2 × 2 less a quarter disc) at distance ρ = 21 + x
/// from the arc's axis, times `length(ρ)`.
fn round_along(length: impl Fn(f64) -> f64) -> f64 {
    let (r, n) = (2.0f64, 2000);
    let h = r / n as f64;
    (0..=n)
        .map(|i| {
            let x = i as f64 * h;
            let w = if i == 0 || i == n { 1.0 } else { [2.0, 4.0][i % 2] };
            w * (r - (r * r - (r - x) * (r - x)).max(0.0).sqrt()) * length(21.0 + x)
        })
        .sum::<f64>()
        * h
        / 3.0
}

/// Its edges are stored the other way round from their curves, which once chained the arc
/// with the front edge across their corner. Rounding the arc at the bottom and the top removes
/// the round's section along it, cut off at the tip by the back face's plane: ρ asin(21 / ρ).
#[test]
fn fillet_an_arc_that_runs_into_a_knife_edge_tip() {
    let mut s = knife_edge_part();
    let before = planned(&s);
    let [x, y] = KNIFE_ARC_MID;
    run(&mut s, "solid.fillet", json!({"edges": [[x, y, 0.0]], "radius": 2}));
    run(&mut s, "solid.fillet", json!({"edges": [[x, y, 10.0]], "radius": 2}));
    let removed = round_along(|rho| rho * (21.0 / rho).asin());
    let after = planned(&s);
    assert!(rel(before - after, 2.0 * removed) < 2e-3, "removed {} vs {}", before - after, 2.0 * removed);
}

/// The part's front corner (the front face into the arc) rounded with 2 first, then the bottom
/// run (front edge, the corner round's arc, the arc) with 2 as well: a round on a round of the
/// same radius. Along the front edge a straight round; at the corner the ball turns about one
/// point, a piece of sphere (θ r³ / 6 over the corner's θ); along the arc the torus section from
/// the corner round to the tip.
#[test]
fn fillet_a_run_over_a_corner_rounded_with_the_same_radius() {
    let mut s = knife_edge_part();
    run(&mut s, "solid.fillet", json!({"edges": [[41.1667, -21.0, 5.0]], "radius": 2}));
    let before = planned(&s);
    let [x, y] = KNIFE_ARC_MID;
    // The corner round: about (62.17 - √(23² - 2²), -19), its arc's middle 2 from there.
    let cx = 62.166666666666664 - (23.0f64 * 23.0 - 4.0).sqrt();
    let joint = 2.0f64.atan2((23.0f64 * 23.0 - 4.0).sqrt());
    let theta = std::f64::consts::FRAC_PI_2 - joint;
    let corner_mid = [cx + 2.0 * (theta / 2.0).sin(), -19.0 - 2.0 * (theta / 2.0).cos()];
    run(&mut s, "solid.fillet", json!({"edges": [[x, y, 0.0], [corner_mid[0], corner_mid[1], 0.0], [20.0, -21.0, 0.0]], "radius": 2}));
    let removed = (1.0 - std::f64::consts::FRAC_PI_4) * 4.0 * cx + theta * 8.0 / 6.0 + round_along(|rho| rho * ((21.0 / rho).asin() - joint));
    let after = planned(&s);
    assert!(rel(before - after, removed) < 1e-3, "removed {} vs {removed}", before - after);
}

/// Straight edges of the part ending at its arc wall: the long front edge at the top (the wall
/// cuts the round off at x = 62.17 − √(21² − d²), d from the front face) and the back edge at
/// the top, which runs into the knife-edge tip (x = 62.17 − √(21² − (21 − d)²), d from the back
/// face).
#[test]
fn fillet_straight_edges_that_end_at_the_arc_wall() {
    let cx = 62.166666666666664;
    for (edge, cut) in [([20.0, -21.0, 10.0], 0.0), ([30.0, 0.0, 10.0], 21.0)] {
        let mut s = knife_edge_part();
        let before = planned(&s);
        run(&mut s, "solid.fillet", json!({"edges": [edge], "radius": 2}));
        let removed = round_along(|rho| {
            let d = rho - 21.0;
            cx - (441.0 - (cut - d) * (cut - d)).sqrt()
        });
        let after = planned(&s);
        assert!(rel(before - after, removed) < 3e-3, "{edge:?}: removed {} vs {removed}", before - after);
    }
}

/// The vertical knife edge at the tip (the arc and the back face meeting at no angle) rounds to
/// the ball 2 from both, about 13 back from the tip: the cusp beyond the ball, on the tip's side
/// of its contacts, all the way up.
#[test]
fn fillet_the_knife_edge_at_the_tip() {
    let cx = 62.166666666666664;
    let (kx, ky) = (cx - (23.0f64 * 23.0 - 19.0 * 19.0).sqrt(), -2.0);
    let (hx, hy) = (0.002, 0.0005);
    let mut area = 0.0;
    let mut x = kx + hx / 2.0;
    while x < cx {
        let lo = -21.0 + (441.0 - (x - cx) * (x - cx)).max(0.0).sqrt();
        for j in 0..((0.0 - lo) / hy).ceil() as usize {
            let (y0, y1) = (lo + j as f64 * hy, (lo + (j + 1) as f64 * hy).min(0.0));
            let y = (y0 + y1) * 0.5;
            let a = (y - ky).atan2(x - kx);
            if (x - kx).hypot(y - ky) > 2.0 && a >= (-21.0 - ky).atan2(cx - kx) {
                area += (y1 - y0) * hx;
            }
        }
        x += hx;
    }
    let mut s = knife_edge_part();
    let before = planned(&s);
    run(&mut s, "solid.fillet", json!({"edges": [[cx, 0.0, 5.0]], "radius": 2}));
    let after = planned(&s);
    assert!(rel(before - after, area * 10.0) < 5e-4, "removed {} vs {}", before - after, area * 10.0);
    let mut s = knife_edge_part();
    run(&mut s, "solid.chamfer", json!({"edges": [[cx, 0.0, 5.0]], "distance": 2}));
    assert!(before - planned(&s) > before - after, "the chamfer takes the ball's bulge off too");
}

/// Rounds of the edges at the arc's top front corner, two or all three together (r 2): they meet
/// in creases (with all three, a piece of sphere fills the corner). Each set takes off its rounds
/// alone, less what each alone takes in a box about the corner, plus what they take there
/// together: the CSG grid of the kernel's `rounds_meeting_at_a_corner_of_an_arc_wall`
/// (curvecorner_tests.rs, 0.02 mm cells), within its accuracy.
#[test]
fn fillet_edges_meeting_at_the_front_corner_of_the_arc() {
    // In the box about the corner: what the long edge's, the arc's and the upright's rounds take
    // alone, and what two or all three take together.
    let corner = [2.719176, 3.015864, 1.863];
    let together = [(vec![0, 2], 3.886936), (vec![0, 1], 4.971256), (vec![1, 2], 4.19864), (vec![0, 1, 2], 6.3332)];
    let [x, y] = KNIFE_ARC_MID;
    let (long, arc, upright) = ([20.0, -21.0, 10.0], [x, y, 10.0], [41.1667, -21.0, 5.0]);
    let off = |edges: Vec<[f64; 3]>| {
        let mut s = knife_edge_part();
        let before = planned(&s);
        run(&mut s, "solid.fillet", json!({"edges": edges, "radius": 2}));
        before - planned(&s)
    };
    let edges = [long, arc, upright];
    let alone = edges.map(|e| off(vec![e]));
    for (which, both) in together {
        let want = which.iter().map(|i| alone[*i] - corner[*i]).sum::<f64>() + both;
        let got = off(which.iter().map(|i| edges[*i]).collect());
        assert!((got - want).abs() < 0.03, "{which:?}: removed {got} vs {want}");
    }
}

/// The long top front edge rounded, then the small front-left upright as a fillet of its own
/// (as in the app): the new round ends in the earlier one, taking off what both take together.
#[test]
fn fillet_the_long_edge_then_the_small_left_edge() {
    let mut s = knife_edge_part();
    let before = planned(&s);
    run(&mut s, "solid.fillet", json!({"edges": [[20.0, -21.0, 10.0]], "radius": 2}));
    run(&mut s, "solid.fillet", json!({"edges": [[0.0, -21.0, 5.0]], "radius": 2}));
    let one_by_one = before - planned(&s);
    let mut s = knife_edge_part();
    run(&mut s, "solid.fillet", json!({"edges": [[20.0, -21.0, 10.0], [0.0, -21.0, 5.0]], "radius": 2}));
    let together = before - planned(&s);
    // Within the display meshes' accuracy (the two are built of different faces).
    assert!(rel(one_by_one, together) < 2e-4, "one by one {one_by_one} vs together {together}");
}

/// The long top front edge and the front-left vertical edge rounded together (r 2): the long
/// round ends at the arc wall, the two meet at the top-left corner (the top-left edge sharp).
/// Each removes its own round (the long one cut off by the wall), less what both would take at
/// the corner: in its 2 mm cube, what lies outside both quarter circles, ∫ (2 − √(4 − t²))² dt
/// over 0..2 = 16 − 8/3 − 4π.
#[test]
fn fillet_two_edges_meeting_at_a_corner_one_running_into_the_arc_wall() {
    let mut s = knife_edge_part();
    let before = planned(&s);
    run(&mut s, "solid.fillet", json!({"edges": [[20.0, -21.0, 10.0], [0.0, -21.0, 5.0]], "radius": 2}));
    let long = round_along(|rho| 62.166666666666664 - (441.0 - (rho - 21.0) * (rho - 21.0)).sqrt());
    let upright = (1.0 - std::f64::consts::FRAC_PI_4) * 4.0 * 10.0;
    let removed = long + upright - (16.0 - 8.0 / 3.0 - 4.0 * std::f64::consts::PI);
    let after = planned(&s);
    assert!(rel(before - after, removed) < 1e-3, "removed {} vs {removed}", before - after);
}

/// A line of text cut into a block (#391): the boolean came out empty on it after two minutes
/// of retries, at 3.5 mm deep. Two walls of a letter met the block's face 2e-7 mm apart, which
/// truck read as two vertices, and the result's shell did not close. The cut works at any
/// depth now, and takes out the same section at each (the text runs off the block's end, so a
/// little less than the letters).
#[test]
fn a_line_of_text_cuts_into_a_block_at_any_depth() {
    let mut removed = Vec::new();
    let mut area = 0.0;
    for depth in [1.0, 3.5] {
        let mut s = Session::default();
        run(&mut s, "sketch.create", json!({"plane":"XY"}));
        run(&mut s, "sketch.rectangle.two_point", json!({"p0":[0,0],"p1":[105,26]}));
        run(&mut s, "sketch.finish", json!({}));
        run(&mut s, "solid.extrude", json!({"distance":-40}));
        run(&mut s, "sketch.create", json!({"plane":"XY"}));
        run(&mut s, "sketch.text", json!({"text":"textvraiment tres long","at":[3,9],"height":7.81}));
        let state = s.world_state();
        let ss = state.sketch(s.active_sketch.unwrap()).unwrap();
        // The letters' area, counters left out (a letter's hole is its own profile too).
        let sorted = |ids: &[String]| {
            let mut ids = ids.to_vec();
            ids.sort();
            ids
        };
        let counter = |p: &solidcraft_sketch::Profile| ss.profiles.iter().any(|q| q.hole_curves.iter().any(|h| sorted(h) == sorted(&p.outer_curves)));
        area = ss.profiles.iter().filter(|p| !counter(p)).map(|p| p.area).sum();
        run(&mut s, "sketch.finish", json!({}));
        run(&mut s, "solid.extrude", json!({"distance":-depth,"operation":"cut"}));
        let volume = run(&mut s, "inspect.measure", json!({}))["total"]["volume_mm3"].as_f64().unwrap();
        removed.push((105.0 * 26.0 * 40.0 - volume) / depth);
        assert_eq!(s.world_state().bodies.len(), 1);
    }
    let (shallow, deep) = (removed[0], removed[1]);
    assert!((deep - shallow).abs() < 1e-3 * shallow, "{shallow} mm² taken out at 1 mm, {deep} mm² at 3.5 mm");
    assert!(shallow > 0.85 * area && shallow < area, "{shallow} mm² taken out, the letters are {area} mm²");
}

/// A design saved before tapered sides were built through `taper_loop` (#1014), by the build
/// before it: two tapered bosses apart from each other (now joined in one boolean), a fillet on
/// a sloping side edge of one and on the top edge of the other, both saved by edge name. They
/// still find their edges, and the part comes out the same.
#[test]
fn fillets_on_tapered_bosses_saved_before_still_find_their_edges() {
    let text = include_str!("../../../../tests/taper/before-1014/two_tapered_bosses_filleted.json");
    let mut s = Session::new(solidcraft_doc::Document::from_json(text).unwrap());
    let volume = run(&mut s, "inspect.measure", json!({}))["total"]["volume_mm3"].as_f64().unwrap();
    for name in ["Extrude2", "Fillet1", "Fillet2"] {
        let id = s.doc.find_feature(name).unwrap().id;
        let r = s.model.result(id).unwrap();
        assert!(r.error.is_none() && r.warning.is_none(), "{name}: {:?} {:?}", r.error, r.warning);
    }
    // The build that saved it measured 69454.885209 mm³.
    assert!((volume - 69454.8852092132).abs() < 1e-3, "{volume}");
    assert_eq!(s.world_state().bodies.len(), 1);
}

/// #1360: a ring drawn round a hole, its inner circle on the hole's wall, cut into a chamfered
/// disc. The ring's hole lies over the part's hole all the way down the cut, so the tool takes
/// it in (a disc instead of a ring cuts the same): its wall no longer lies on the hole's wall,
/// which the boolean couldn't intersect. Over a hole whose floor the cut reaches, the ring's hole
/// stays: the material under the floor is not cut (the cut may still fail there, as before).
#[test]
fn a_ring_cut_round_a_hole_takes_in_the_hole_it_is_drawn_round() {
    let ring = |hole: Value| {
        let mut s = Session::default();
        for (c, p) in [
            ("sketch.create", json!({"plane":"XY"})),
            ("sketch.circle.center", json!({"center":[0,0],"radius":40})),
            ("sketch.finish", json!({})),
            ("solid.extrude", json!({"distance":10})),
            ("solid.hole", hole),
            ("solid.chamfer", json!({"edges":[[40,0,10]],"distance":1})),
            ("sketch.create", json!({"plane":{"face":[0,0,10]}})),
            ("sketch.circle.center", json!({"center":[20,0],"radius":3})),
            ("sketch.circle.center", json!({"center":[20,0],"radius":6})),
            ("sketch.finish", json!({})),
        ] {
            run(&mut s, c, p);
        }
        let before = volume(&mut s);
        // Profile 2: the ring between the hole's wall and the larger circle.
        let r = s.execute("solid.extrude", &json!({"direction":"negative","distance":"3","operation":"cut","profiles":[2]}));
        let clean = r
            .as_ref()
            .ok()
            .and_then(|v| v["feature"].as_u64())
            .and_then(|id| s.model.result(id))
            .is_some_and(|f| f.boolean_diagnostics.events.is_empty());
        (r.is_ok(), before - volume(&mut s), clean)
    };
    let taken = std::f64::consts::PI * (36.0 - 9.0) * 3.0;
    let (ok, through, clean) = ring(json!({"position":[20,0,10],"diameter":6}));
    assert!(ok && clean && rel(through, taken) < 1e-3, "{through} vs {taken}");
    // A blind hole 1 deep: under its floor the ring's hole is material, and nothing of it goes.
    let (ok, blind, _) = ring(json!({"position":[20,0,10],"diameter":6,"depth":1}));
    assert!(if ok { rel(blind, taken) < 1e-3 } else { blind.abs() < 1e-6 }, "{ok} {blind} vs {taken}");
}

/// #1360: an annulus cut 3 mm into a chamfered disc, its inner circle on a hole's wall. The
/// boolean can't intersect the two walls lying on each other; its last resort (the tool moved by
/// a micron) left a face a hair wide that meshes as its whole plane at a fine step, and the part
/// came back 21000 mm³ larger than before, a square plane across it. A cut may fail there, but it
/// never gives back more than it was given.
#[test]
fn a_cut_flush_with_a_hole_never_grows_the_part() {
    let mut s = Session::default();
    for (c, p) in [
        ("sketch.create", json!({"plane":"XY"})),
        ("sketch.circle.center", json!({"center":[0,0],"radius":40})),
        ("sketch.finish", json!({})),
        ("solid.extrude", json!({"distance":10})),
        ("solid.hole", json!({"position":[20,0,10],"diameter":6})),
        ("solid.chamfer", json!({"edges":[[40,0,10]],"distance":1})),
        ("sketch.create", json!({"plane":{"face":[0,0,10]}})),
        ("sketch.circle.center", json!({"center":[20,0],"radius":3})),
        ("sketch.circle.center", json!({"center":[20,0],"radius":6})),
        ("sketch.finish", json!({})),
    ] {
        run(&mut s, c, p);
    }
    let before = volume(&mut s);
    // Profile 2: the annulus between the hole's wall and the larger circle.
    let r = s.execute("solid.extrude", &json!({"direction":"negative","distance":"3","operation":"cut","profiles":[2]}));
    let after = volume(&mut s);
    let ring = std::f64::consts::PI * (36.0 - 9.0) * 3.0;
    match r {
        Ok(_) => assert!(rel(before - after, ring) < 1e-2, "{before} -> {after}, want {ring} taken"),
        Err(_) => assert!(rel(after, before) < 1e-9, "{before} -> {after} after a failed cut"),
    }
}

/// A line of text cut into a face: the letters' curves (the font's quadratics) are read
/// exactly, and the cut is decided in 2D in one boolean, at any depth and where the text runs
/// off the face. The B-rep boolean failed on many letters at once ("no result" at 3.5 mm, and
/// off the edge), else took one letter at a time on an ever larger body (tens of seconds).
#[test]
fn a_line_of_text_is_cut_in_one_exact_boolean() {
    let cut = |at: [f64; 2], depth: f64| {
        let mut s = Session::default();
        run(&mut s, "sketch.create", json!({"plane":"XY"}));
        run(&mut s, "sketch.rectangle.two_point", json!({"p0":[0,0],"p1":[105,26]}));
        run(&mut s, "sketch.finish", json!({}));
        run(&mut s, "solid.extrude", json!({"distance":40}));
        let before = volume(&mut s);
        run(&mut s, "sketch.create", json!({"plane":{"face":[50,13,40]}}));
        run(&mut s, "sketch.text", json!({"text":"textvraiment tres long","at":at,"height":7.81}));
        run(&mut s, "sketch.finish", json!({}));
        let id = run(&mut s, "solid.extrude", json!({"distance":-depth,"operation":"cut"}))["feature"].as_u64().unwrap();
        let r = s.model.result(id).unwrap();
        assert!(r.error.is_none() && r.boolean_diagnostics.events.is_empty(), "{at:?} {depth}: {r:?}");
        before - volume(&mut s)
    };
    // Straight walls: the material taken grows with the depth, the letters' area each mm.
    let (one, deep) = (cut([3.0, 9.0], 1.0), cut([3.0, 9.0], 3.5));
    assert!(rel(deep, 3.5 * one) < 1e-3, "{one} {deep}");
    // Off the face's edge, the part of the text still over it.
    let off = cut([-6.0, 20.0], 1.0);
    assert!(off > 0.0 && off < one, "{off} {one}");
}

/// A line of text cut right through a part, and one joined on in relief, are decided in 2D in
/// one boolean too. Through, each letter's counter (inside an "e" or an "a") comes out as a
/// piece of its own. In relief the letters are joined to the part all at once: one by one, each
/// joined to a part grown by the ones before, it took minutes.
#[test]
fn a_line_of_text_cut_through_or_joined_in_relief_is_one_boolean() {
    let text = |op: &str, distance: f64| {
        let mut s = Session::default();
        run(&mut s, "sketch.create", json!({"plane":"XY"}));
        run(&mut s, "sketch.rectangle.two_point", json!({"p0":[0,0],"p1":[105,26]}));
        run(&mut s, "sketch.finish", json!({}));
        run(&mut s, "solid.extrude", json!({"distance":40}));
        let before = volume(&mut s);
        run(&mut s, "sketch.create", json!({"plane":{"face":[50,13,40]}}));
        run(&mut s, "sketch.text", json!({"text":"textvraiment tres long","at":[3,9],"height":6.5}));
        run(&mut s, "sketch.finish", json!({}));
        let id = run(&mut s, "solid.extrude", json!({"distance":distance,"operation":op}))["feature"].as_u64().unwrap();
        let r = s.model.result(id).unwrap();
        assert!(r.error.is_none() && r.boolean_diagnostics.events.is_empty(), "{op}: {r:?}");
        (before, volume(&mut s), s.model.state().bodies.len())
    };
    let (before, through, pieces) = text("cut", -40.0);
    let (_, one, _) = text("cut", -1.0);
    // Through: the letters' area all the way down, with the counters left as pieces.
    assert!(pieces > 1, "{pieces}");
    assert!(rel(before - through, 40.0 * (before - one)) < 2e-3, "{} vs {}", before - through, 40.0 * (before - one));
    // In relief 2 high: twice what a 1 mm cut takes.
    let (_, relief, pieces) = text("join", 2.0);
    assert_eq!(pieces, 1);
    assert!(rel(relief - before, 2.0 * (before - one)) < 2e-3, "{} vs {}", relief - before, 2.0 * (before - one));
}

/// The same line of text cut again, deeper: the new walls lie on the letters' walls. The 2D route
/// takes it as one exact boolean: a point a hair from a segment's end is that end, a sliver of a
/// letter's curve shorter than the tolerance is dropped, and a piece of a letter so short that the
/// engraved walls read it as an arc while the tool has the curve itself is one edge. Before, the
/// sewing found a degenerate edge, then the faces did not close, and the cut went to the B-rep
/// boolean's retries (90 s).
#[test]
fn a_line_of_text_cut_again_deeper_is_one_exact_boolean() {
    let mut s = Session::default();
    run(&mut s, "sketch.create", json!({"plane":"XY"}));
    run(&mut s, "sketch.rectangle.two_point", json!({"p0":[0,0],"p1":[105,26]}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.extrude", json!({"distance":40}));
    let cut = |s: &mut Session, depth: f64| {
        let before = volume(s);
        run(s, "sketch.create", json!({"plane":"XY","offset":40}));
        run(s, "sketch.text", json!({"text":"textvraiment tres long","at":[3,9],"height":6.5}));
        run(s, "sketch.finish", json!({}));
        let id = run(s, "solid.extrude", json!({"distance":-depth,"operation":"cut"}))["feature"].as_u64().unwrap();
        let r = s.model.result(id).unwrap();
        assert!(r.error.is_none() && r.boolean_diagnostics.events.is_empty(), "{depth}: {r:?}");
        before - volume(s)
    };
    let first = cut(&mut s, 1.0);
    // Cut again 2 mm deep: one more millimetre of the letters.
    let again = cut(&mut s, 2.0);
    assert!(rel(again, first) < 1e-3, "{first} {again}");
}

/// Text cut through a large plate (2 m) is decided in 2D in one exact boolean, as it is in a
/// small one. The 2D route's tolerances grow with the part: a font's curve that is merely near a
/// circle was read as an arc ending a micron off the next curve, so the letter's outline did not
/// close, and where the curves of a 3 come close the arrangement left a sliver that closed up to
/// nothing. The cut went to the B-rep boolean instead, which took tens of seconds and moved the
/// tool to find a result at all.
#[test]
fn text_cut_through_a_large_plate_is_one_exact_boolean() {
    let taken = |w: f64, h: f64, at: [f64; 2]| {
        let mut s = Session::default();
        run(&mut s, "solid.box", json!({"length":w,"width":h,"height":1.5,"corner":[0,0,0]}));
        let before = volume(&mut s);
        run(&mut s, "sketch.create", json!({"plane":"XY","offset":1.5}));
        run(&mut s, "sketch.text", json!({"text":"PART-23","at":at,"height":7}));
        run(&mut s, "sketch.finish", json!({}));
        let id = run(&mut s, "solid.extrude", json!({"distance":-2,"operation":"cut"}))["feature"].as_u64().unwrap();
        let r = s.model.result(id).unwrap();
        assert!(r.error.is_none() && r.boolean_diagnostics.events.is_empty(), "{w} x {h}: {r:?}");
        before - volume(&mut s)
    };
    let (small, large) = (taken(200.0, 120.0, [60.0, 40.0]), taken(2000.0, 1200.0, [860.0, 125.0]));
    // The letters right through the plate, their counters left as pieces of their own: the same
    // material in either plate (measured on meshes, which are coarser on the larger one).
    assert!(small > 10.0 && rel(large, small) < 5e-2, "{small} {large}");
}
