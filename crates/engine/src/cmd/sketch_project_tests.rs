use serde_json::{Value, json};

use crate::Session;

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    match s.execute(id, &p) {
        Ok(v) => v,
        Err(e) => panic!("{id} {p}: {e}"),
    }
}

fn ids(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default()
}

#[test]
fn manual_projection_captures_the_requested_coincident_body_producer() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length":10,"width":10,"height":10,"body_name":"Control","name":"ControlFeature"}));
    run(&mut s, "solid.box", json!({"length":20,"width":20,"height":10,"body_name":"Target","name":"TargetFeature"}));
    let producer = s.doc.find_feature("TargetFeature").unwrap().id;
    let id = run(&mut s, "sketch.create", json!({"plane":"XY","offset":10}))["sketch"].as_u64().unwrap();
    run(&mut s, "sketch.project", json!({"refs":[{"body":"Target","face":[1,1,10]}]}));
    run(&mut s, "sketch.finish", json!({}));
    let initial_state = s.model.state();
    let initial = &initial_state.sketch(id).unwrap().sketch.links[0];
    assert_eq!(initial.face_name, Some(format!("F{producer}:+z")));
    assert!(!initial.lost);
    let height = s.doc.find_feature("TargetFeature").unwrap().param_names[2].clone();
    run(&mut s, "parameters.change", json!({"name":height,"expression":"20 mm"}));
    let verify = |session: &Session| {
        let state = session.model.state();
        let sk = &state.sketch(id).unwrap().sketch;
        assert!(!sk.links[0].lost);
        assert_eq!(sk.links[0].face_name, Some(format!("F{producer}:+z")));
        assert!(matches!(&sk.links[0].source,solidcraft_sketch::LinkSource::Face{body,at} if body=="Target" && (at.z-20.).abs()<1e-6));
        assert!(sk.points.iter().any(|point| point.pos.x > 19.));
        assert!(sk.points.iter().any(|point| point.pos.y > 19.));
        assert!((session.world_state().body("Control").unwrap().mesh().bounds().max.z - 10.).abs() < 1e-6);
    };
    verify(&s);
    verify(&Session::new(solidcraft_doc::Document::from_json(&s.doc.to_json()).unwrap()));
}

#[test]
fn initial_projection_retains_selected_face_when_closest_point_lands_on_shared_edge() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length":20,"width":20,"height":10,"body_name":"Target"}));
    let state = s.model.state();
    let body = state.body("Target").unwrap();
    let source = solidcraft_sketch::LinkSource::Face { body: "Target".into(), at: solidcraft_geom::Vec3::new(-0.1, 5., 10.5) };
    let resolved = solidcraft_doc::project::resolve(
        &s.doc,
        &std::collections::BTreeMap::new(),
        &state,
        &solidcraft_geom::Plane::XY,
        solidcraft_sketch::LinkKind::Project,
        &source,
    )
    .unwrap();
    let index = resolved.face_index.unwrap();
    let expected = solidcraft_doc::naming::face_names(body)[index].clone();
    let solidcraft_sketch::LinkSource::Face { at, .. } = resolved.source else { panic!("not face") };
    assert!(at.x.abs() < 1e-6 && (at.z - 10.).abs() < 1e-6, "closestpoint must be on shared top/wall edge: {at:?}");
    let plane = body.body.face_plane(index).unwrap();
    let id = run(&mut s, "sketch.create", json!({"plane":{"origin":plane.origin,"x_dir":plane.x,"y_dir":plane.y}}))["sketch"].as_u64().unwrap();
    run(&mut s, "sketch.project", json!({"refs":[{"body":"Target","face":[-0.1,5.,10.5]}]}));
    run(&mut s, "sketch.finish", json!({}));
    let current = s.model.state();
    let sk = &current.sketch(id).unwrap().sketch;
    assert_eq!(sk.links[0].face_name, Some(expected));
    assert!(!sk.links[0].lost);
    assert_eq!(sk.curves.len(), 4);
    let reopened = Session::new(solidcraft_doc::Document::from_json(&s.doc.to_json()).unwrap());
    assert_eq!(reopened.model.state().sketch(id).unwrap().sketch.curves, sk.curves);
}

/// A 40 x 30 x 20 box from a sketch whose width is the user parameter `width`.
fn box_part(s: &mut Session) {
    run(s, "parameters.change", json!({"name": "width", "expression": "40 mm"}));
    run(s, "sketch.create", json!({"plane": "XY"}));
    let r = run(s, "sketch.rectangle.two_point", json!({"p0": [0, 0], "p1": [40, 30]}));
    let l = ids(&r["curves"]);
    let sk = s.doc.sketch(s.active_sketch.unwrap()).unwrap();
    let start = match sk.curves[0].kind {
        solidcraft_sketch::CurveKind::Line { a, .. } => sk.points[a].id.clone(),
        _ => panic!(),
    };
    run(s, "sketch.constraint.coincident", json!({"a": start, "b": "origin"}));
    run(s, "sketch.dimension", json!({"entities": [l[0]], "value": "width"}));
    run(s, "sketch.dimension", json!({"entities": [l[1]], "value": 30}));
    run(s, "sketch.finish", json!({}));
    run(s, "solid.extrude", json!({"distance": 20}));
}

fn inspect(s: &mut Session) -> Value {
    run(s, "sketch.inspect", json!({}))
}

fn linked_curves(si: &Value) -> Vec<Value> {
    si["curves"].as_array().unwrap().iter().filter(|c| c.get("link").is_some()).cloned().collect()
}

#[test]
fn sketch_on_face_projects_its_edges_and_follows_edits() {
    let mut s = Session::default();
    box_part(&mut s);
    let r = run(&mut s, "sketch.create", json!({"plane": {"face": [20, 15, 20]}}));
    assert!(r["projected"].is_string(), "{r}");
    let si = inspect(&mut s);
    let lc = linked_curves(&si);
    assert_eq!(lc.len(), 4, "{si}");
    assert!(lc.iter().all(|c| c["type"] == "line"));
    // The face loop is one closed profile of projected lines.
    assert_eq!(si["profiles"].as_array().unwrap().len(), 1, "{si}");
    assert!((si["profiles"][0]["area_mm2"].as_f64().unwrap() - 1200.0).abs() < 1e-6);
    // Projected geometry is fixed: the sketch has no degrees of freedom.
    assert_eq!(si["dof"], 0, "{si}");
    // A circle on the face, dimensioned to a projected edge.
    let c = run(&mut s, "sketch.circle.center", json!({"center": [10, 10], "radius": 3}));
    let cid = ids(&c["curves"])[0].clone();
    let e = lc.iter().find(|c| c["start_at"][1].as_f64().unwrap().abs() < 1e-9 && c["end_at"][1].as_f64().unwrap().abs() < 1e-9).unwrap();
    run(&mut s, "sketch.dimension", json!({"entities": [format!("{cid}.center"), e["id"]], "value": 8}));
    let si = inspect(&mut s);
    let center = si["points"].as_array().unwrap().iter().find(|p| p["id"] == format!("{cid}.center")).unwrap();
    assert!((center["at"][1].as_f64().unwrap() - 8.0).abs() < 1e-7, "{center}");
    run(&mut s, "sketch.finish", json!({}));
    // Widen the box: the projected loop follows.
    run(&mut s, "parameters.change", json!({"name": "width", "expression": "50 mm"}));
    let st = s.model.state();
    let ss = st.sketches.last().unwrap();
    assert_eq!(ss.profiles.len(), 2);
    let outer = ss.profiles.iter().map(|p| p.area).fold(0.0, f64::max);
    assert!((outer - (1500.0 - std::f64::consts::PI * 9.0)).abs() < 1e-6, "{outer}");
    let w = s.model.results.last().unwrap().warning.clone();
    assert!(w.is_none(), "{w:?}");
}

#[test]
fn auto_project_can_be_turned_off() {
    let mut s = Session::default();
    box_part(&mut s);
    let r = run(&mut s, "sketch.create", json!({"plane": {"face": [20, 15, 20]}, "project_edges": false}));
    assert!(r["projected"].is_null());
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "sketch.auto_project", json!({"value": false}));
    let r = run(&mut s, "sketch.create", json!({"plane": {"face": [20, 15, 20]}}));
    assert!(r["projected"].is_null());
}

/// #680: auto-projection is a preference, not part of a design: turning it off holds for the
/// next design started or opened in the session (`doc.recover` replaces the session the same way).
#[test]
fn auto_project_off_holds_for_designs_started_and_opened() {
    let dir = std::env::temp_dir().join(format!("solidcraft-auto-project-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("box.solidcraft");
    let mut s = Session::default();
    box_part(&mut s);
    run(&mut s, "file.save_as", json!({"path": file.to_string_lossy()}));
    run(&mut s, "sketch.auto_project", json!({"value": false}));
    run(&mut s, "file.new", json!({}));
    assert!(!s.auto_project, "file.new");
    run(&mut s, "doc.open", json!({"path": file.to_string_lossy()}));
    assert!(!s.auto_project, "doc.open");
    let r = run(&mut s, "sketch.create", json!({"plane": {"face": [20, 15, 20]}}));
    assert!(r["projected"].is_null(), "{r}");
    run(&mut s, "sketch.auto_project", json!({"value": true}));
    run(&mut s, "file.new", json!({}));
    assert!(s.auto_project, "turned back on, it stays on");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn project_edges_bodies_sketches_and_origin() {
    let mut s = Session::default();
    box_part(&mut s);
    run(&mut s, "solid.cylinder", json!({"base": [80, 0, 5], "radius": 6, "height": 10}));
    run(&mut s, "sketch.create", json!({"plane": "XY"}));
    // An edge on the top face projects to a line on XY.
    let r = run(&mut s, "sketch.project", json!({"refs": [{"edge": [20, 30, 20]}]}));
    let si = inspect(&mut s);
    let c = si["curves"].as_array().unwrap().iter().find(|c| c["id"] == r["links"][0]["curves"][0]).unwrap().clone();
    assert_eq!(c["type"], "line");
    let ys: Vec<f64> = [c["start_at"][1].as_f64().unwrap(), c["end_at"][1].as_f64().unwrap()].to_vec();
    assert!(ys.iter().all(|y| (y - 30.0).abs() < 1e-9), "{c}");
    // The cylinder's silhouette seen from above is its circle, exactly.
    let name = s.model.state().bodies[1].name.clone();
    let r = run(&mut s, "sketch.project", json!({"refs": [{"body": name}]}));
    let cid = r["links"][0]["curves"][0].clone();
    let si = inspect(&mut s);
    let c = si["curves"].as_array().unwrap().iter().find(|c| c["id"] == cid).unwrap().clone();
    assert_eq!(c["type"], "circle", "{c}");
    assert!((c["radius"].as_f64().unwrap() - 6.0).abs() < 1e-6, "{c}");
    assert!((c["center_at"][0].as_f64().unwrap() - 80.0).abs() < 1e-6, "{c}");
    // Origin, the Y axis and the YZ plane.
    run(&mut s, "sketch.project", json!({"refs": ["origin", {"axis": "Y"}, {"plane": "YZ"}]}));
    // The box silhouette from above is its 40 x 30 outline.
    let b0 = s.model.state().bodies[0].name.clone();
    let r = run(&mut s, "sketch.project", json!({"refs": [{"body": b0}]}));
    assert_eq!(r["links"][0]["curves"].as_array().unwrap().len(), 4, "{r}");
    run(&mut s, "sketch.finish", json!({}));
    let first = s.active_sketch;
    assert!(first.is_none());
    let sk1 = s.doc.features.last().unwrap().id;
    // Another sketch projects a curve of the first one.
    run(&mut s, "sketch.create", json!({"plane": "XY", "offset": 50}));
    let r = run(&mut s, "sketch.project", json!({"refs": [{"sketch": sk1, "curve": cid}]}));
    let c2 = r["links"][0]["curves"][0].clone();
    let si = inspect(&mut s);
    let c = si["curves"].as_array().unwrap().iter().find(|c| c["id"] == c2).unwrap().clone();
    assert_eq!(c["type"], "circle");
    // Bad references fail cleanly.
    assert!(s.execute("sketch.project", &json!({"refs": [{"edge": [1000, 1000, 1000]}]})).is_err());
    assert!(s.execute("sketch.project", &json!({"refs": [{"plane": "XY"}]})).is_err(), "parallel plane projects to nothing");
}

#[test]
fn intersect_cuts_a_body_with_the_sketch_plane() {
    let mut s = Session::default();
    run(&mut s, "solid.cylinder", json!({"base": [0, 0, 0], "radius": 7, "height": 30}));
    run(&mut s, "sketch.create", json!({"plane": "XY", "offset": 12}));
    let name = s.model.state().bodies[0].name.clone();
    let r = run(&mut s, "sketch.intersect", json!({"refs": [{"body": name}]}));
    let si = inspect(&mut s);
    let c = si["curves"].as_array().unwrap().iter().find(|c| c["id"] == r["links"][0]["curves"][0]).unwrap().clone();
    assert_eq!(c["type"], "circle", "{c}");
    assert!((c["radius"].as_f64().unwrap() - 7.0).abs() < 1e-6, "{c}");
    assert_eq!(si["profiles"].as_array().unwrap().len(), 1);
}

#[test]
fn break_link_makes_normal_geometry() {
    let mut s = Session::default();
    box_part(&mut s);
    let r = run(&mut s, "sketch.create", json!({"plane": {"face": [20, 15, 20]}}));
    let link = r["projected"].as_str().unwrap().to_string();
    run(&mut s, "sketch.break_link", json!({"link": link}));
    let si = inspect(&mut s);
    assert!(linked_curves(&si).is_empty());
    assert!(si["links"].as_array().unwrap().is_empty());
    // Now free: 4 lines with 4 shared corners, 8 degrees of freedom.
    assert_eq!(si["dof"], 8, "{si}");
    assert!(s.execute("sketch.break_link", &json!({"link": "j99"})).is_err());
}

#[test]
fn lost_reference_keeps_geometry_with_a_warning() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10}));
    let boxf = s.doc.features.last().unwrap().id;
    run(&mut s, "sketch.create", json!({"plane": "XY"}));
    run(&mut s, "sketch.project", json!({"refs": [{"edge": [5, 0, 10]}]}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "timeline.suppress", json!({"feature": boxf, "suppressed": true}));
    let st = s.model.state();
    let ss = st.sketches.last().unwrap();
    assert_eq!(ss.sketch.curves.len(), 1, "geometry kept");
    assert!(ss.sketch.links[0].lost);
    let w = s.model.results.last().unwrap().warning.clone().unwrap_or_default();
    assert!(w.contains("lost"), "{w}");
}

#[test]
fn drawing_on_a_model_vertex_projects_it() {
    let mut s = Session::default();
    box_part(&mut s);
    run(&mut s, "sketch.create", json!({"plane": "XY"}));
    let r = run(&mut s, "sketch.line", json!({"points": ["vertex:40,30,20", [60, 45]]}));
    let l = ids(&r["curves"])[0].clone();
    let si = inspect(&mut s);
    let start = si["curves"].as_array().unwrap().iter().find(|c| c["id"] == l).unwrap()["start"].clone();
    let p = si["points"].as_array().unwrap().iter().find(|p| p["id"] == start).unwrap().clone();
    assert!(p["link"].is_string(), "{p}");
    assert_eq!(p["fixed"], true);
    assert!((p["at"][0].as_f64().unwrap() - 40.0).abs() < 1e-9);
    // Snapping onto an edge projects it and keeps the point on it.
    let r = run(&mut s, "sketch.line", json!({"points": [{"on_edge": [20, 0, 20]}, [20, -15]]}));
    assert_eq!(ids(&r["curves"]).len(), 1);
    let si = inspect(&mut s);
    assert!(si["constraints"].as_array().unwrap().iter().any(|c| c["type"] == "point_on_curve"), "{si}");
}

#[test]
fn fit_curves_to_a_mesh_section() {
    let mut s = Session::default();
    run(&mut s, "solid.cylinder", json!({"base": [0, 0, 0], "radius": 7, "height": 30}));
    run(&mut s, "sketch.create", json!({"plane": "XY", "offset": 10}));
    let name = s.model.state().bodies[0].name.clone();
    let r = run(&mut s, "sketch.fit_curves_to_section", json!({"body": name}));
    assert_eq!(r["curves"].as_array().unwrap().len(), 1, "{r}");
    let si = inspect(&mut s);
    assert!(si["links"].as_array().unwrap().is_empty());
    let c = &si["curves"][0];
    assert_eq!(c["type"], "circle");
    assert!(c.get("link").is_none());
    assert!(s.execute("sketch.fit_curves_to_section", &json!({"body": "nope"})).is_err());
}

#[test]
fn three_d_sketch_curves() {
    let mut s = Session::default();
    // Include keeps an out-of-plane edge 3D.
    run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10}));
    run(&mut s, "sketch.create", json!({"plane": "XY"}));
    let r = run(&mut s, "sketch.include_3d", json!({"refs": [{"edge": [10, 10, 5]}]}));
    let st = s.model.state();
    let sk = &st.sketch(s.active_sketch.unwrap()).unwrap().sketch;
    assert_eq!(sk.wires.len(), 1, "{r}");
    assert!((sk.wires[0].pts.iter().map(|p| p.z).fold(0.0, f64::max) - 10.0).abs() < 1e-9);
    run(&mut s, "sketch.finish", json!({}));

    // Project a line onto a sphere along the sketch normal.
    let mut s = Session::default();
    run(&mut s, "solid.sphere", json!({"center": [0, 0, 0], "radius": 20}));
    run(&mut s, "sketch.create", json!({"plane": "XY", "offset": 50}));
    let l = ids(&run(&mut s, "sketch.line", json!({"points": [[-10, 0], [10, 0]]}))["curves"])[0].clone();
    run(&mut s, "sketch.finish", json!({}));
    let src = s.doc.features.last().unwrap().id;
    run(&mut s, "sketch.create", json!({"plane": "XY"}));
    let r = run(&mut s, "sketch.project_to_surface", json!({"curves": [{"sketch": src, "curve": l}], "face": [0, 0, 20]}));
    let wid = r["links"][0]["wires"][0].as_str().unwrap().to_string();
    let st = s.model.state();
    let sk = &st.sketch(s.active_sketch.unwrap()).unwrap().sketch;
    let w = &sk.wires[sk.wire_index(&wid).unwrap()];
    assert!(w.pts.len() > 20);
    for p in &w.pts {
        assert!((p.len() - 20.0).abs() < 0.1, "{p:?}");
        assert!(p.z > 0.0);
    }
    // Delete the wire.
    run(&mut s, "sketch.delete", json!({"entities": [wid]}));
    assert!(s.model.state().sketch(s.active_sketch.unwrap()).unwrap().sketch.wires.is_empty());
    run(&mut s, "sketch.finish", json!({}));

    // Two crossing cylinders meet in 3D curves.
    let mut s = Session::default();
    run(&mut s, "solid.cylinder", json!({"base": [0, 0, -20], "radius": 5, "height": 40}));
    run(&mut s, "solid.cylinder", json!({"base": [-20, 0, 0], "axis": [1, 0, 0], "radius": 4, "height": 40, "operation": "new"}));
    let names: Vec<String> = s.model.state().bodies.iter().map(|b| b.name.clone()).collect();
    assert_eq!(names.len(), 2, "{names:?}");
    run(&mut s, "sketch.create", json!({"plane": "XY"}));
    let r = run(&mut s, "sketch.intersection_curve", json!({"a": {"body": names[0]}, "b": {"body": names[1]}}));
    assert!(!r["wires"].as_array().unwrap().is_empty(), "{r}");
    let st = s.model.state();
    let sk = &st.sketch(s.active_sketch.unwrap()).unwrap().sketch;
    for w in &sk.wires {
        for p in &w.pts {
            assert!(((p.x * p.x + p.y * p.y).sqrt() - 5.0).abs() < 0.1, "{p:?}");
            assert!(((p.y * p.y + p.z * p.z).sqrt() - 4.0).abs() < 0.1, "{p:?}");
        }
    }
    run(&mut s, "sketch.finish", json!({}));

    // Spun profile of a cylinder about Z, on the XZ plane: a straight outline 6 from the axis.
    let mut s = Session::default();
    run(&mut s, "solid.cylinder", json!({"base": [0, 0, 0], "radius": 6, "height": 20}));
    let name = s.model.state().bodies[0].name.clone();
    run(&mut s, "sketch.create", json!({"plane": "XZ"}));
    let r = run(&mut s, "sketch.spun_profile", json!({"body": name, "axis": "Z"}));
    let si = inspect(&mut s);
    let c = si["curves"].as_array().unwrap().iter().find(|c| c["id"] == r["curves"][0]).unwrap().clone();
    assert_eq!(c["type"], "line", "{c}");
    assert!((c["start_at"][0].as_f64().unwrap().abs() - 6.0).abs() < 1e-3, "{c}");
}

#[test]
fn isoparametric_curves_of_a_cylinder() {
    let mut s = Session::default();
    run(&mut s, "solid.cylinder", json!({"base": [0, 0, 0], "radius": 6, "height": 20}));
    run(&mut s, "sketch.create", json!({"plane": "XY"}));
    // u: around (a circle at z = 10); v: along (a line from z 0 to 20).
    let r = run(&mut s, "sketch.isoparametric_curve", json!({"face": [6, 0, 10]}));
    let st = s.model.state();
    let sk = &st.sketch(s.active_sketch.unwrap()).unwrap().sketch;
    let w = &sk.wires[sk.wire_index(r["wires"][0].as_str().unwrap()).unwrap()];
    assert!(w.pts.iter().all(|p| (p.z - 10.0).abs() < 1e-6 && ((p.x * p.x + p.y * p.y).sqrt() - 6.0).abs() < 1e-3), "{:?}", &w.pts[..3]);
    let r = run(&mut s, "sketch.isoparametric_curve", json!({"face": [6, 0, 10], "direction": "v"}));
    let st = s.model.state();
    let sk = &st.sketch(s.active_sketch.unwrap()).unwrap().sketch;
    let w = &sk.wires[sk.wire_index(r["wires"][0].as_str().unwrap()).unwrap()];
    let zs: Vec<f64> = w.pts.iter().map(|p| p.z).collect();
    assert!(zs.iter().cloned().fold(f64::MIN, f64::max) > 19.9 && zs.iter().cloned().fold(f64::MAX, f64::min) < 0.1, "{zs:?}");
    // Within the mesh's sag: the cut runs across facets unless a seam happens to lie there.
    assert!(w.pts.iter().all(|p| (p.x - 6.0).abs() < 1e-2 && p.y.abs() < 1e-2), "{:?}", w.pts);
}

#[test]
fn tilted_circles_project_to_exact_ellipses_and_splines_stay_splines() {
    let mut s = Session::default();
    run(&mut s, "solid.cylinder", json!({"base": [0, 0, 0], "axis": [1, 0, 1], "radius": 5, "height": 20}));
    run(&mut s, "sketch.create", json!({"plane": "XY", "offset": -10}));
    // The base circle (centred at the origin) seen from above.
    let r = run(&mut s, "sketch.project", json!({"refs": [{"edge": [0, 5, 0]}]}));
    // One seam half: exact conics (rational quadratic arcs).
    let si = inspect(&mut s);
    let c = si["curves"].as_array().unwrap().iter().find(|c| c["id"] == r["links"][0]["curves"][0]).unwrap().clone();
    assert_eq!(c["type"], "conic", "{c}");
    let st = s.model.state();
    let sk = &st.sketch(s.active_sketch.unwrap()).unwrap().sketch;
    for ci in sk.link_curves(r["links"][0]["link"].as_str().unwrap()) {
        for q in sk.polyline(ci) {
            // On the ellipse x²/(5/√2)² + y²/5² = 1.
            let e = (q.x / (5.0 * std::f64::consts::FRAC_1_SQRT_2)).powi(2) + (q.y / 5.0).powi(2);
            assert!((e - 1.0).abs() < 1e-9, "{q:?}");
        }
    }
    // The whole face (both halves of its rim, plus its seam) gives the full ellipse.
    let r = run(&mut s, "sketch.project", json!({"refs": [{"face": [0, 0, 0]}]}));
    let si = inspect(&mut s);
    let types: Vec<String> = r["links"][0]["curves"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| si["curves"].as_array().unwrap().iter().find(|c| c["id"] == *id).unwrap()["type"].as_str().unwrap().to_string())
        .collect();
    assert!(types.iter().all(|t| t == "conic" || t == "ellipse"), "{types:?}");
    let r = run(&mut s, "sketch.project", json!({"refs": [{"edge": [0, -5, 0]}]}));
    let si = inspect(&mut s);
    let c = si["curves"].as_array().unwrap().iter().find(|c| c["id"] == r["links"][0]["curves"][0]).unwrap().clone();
    if c["type"] == "ellipse" {
        assert!((c["minor_radius"].as_f64().unwrap() - 5.0 * std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-6, "{c}");
        let major = &c["major_at"];
        assert!(((major[0].as_f64().unwrap().powi(2) + major[1].as_f64().unwrap().powi(2)).sqrt() - 5.0).abs() < 1e-6, "{c}");
    }
    run(&mut s, "sketch.finish", json!({}));
    // A spline of a parallel sketch projects as the same spline.
    run(&mut s, "sketch.create", json!({"plane": "XY", "offset": 30}));
    let sp = run(&mut s, "sketch.spline.fit_point", json!({"points": [[0, 0], [10, 5], [20, 0]]}))["curves"][0].clone();
    run(&mut s, "sketch.finish", json!({}));
    let sk1 = s.doc.features.last().unwrap().id;
    run(&mut s, "sketch.create", json!({"plane": "XY"}));
    let r = run(&mut s, "sketch.project", json!({"refs": [{"sketch": sk1, "curve": sp}]}));
    let si = inspect(&mut s);
    let c = si["curves"].as_array().unwrap().iter().find(|c| c["id"] == r["links"][0]["curves"][0]).unwrap().clone();
    assert_eq!(c["type"], "spline", "{c}");
    assert_eq!(c["points"].as_array().unwrap().len(), 3);
}

#[test]
fn sweeps_and_pipes_follow_3d_sketch_curves() {
    // A line projected onto a sphere: a 3D arc on its top.
    let mut s = Session::default();
    run(&mut s, "solid.sphere", json!({"center": [0, 0, 0], "radius": 20}));
    run(&mut s, "sketch.create", json!({"plane": "XY", "offset": 50}));
    let l = ids(&run(&mut s, "sketch.line", json!({"points": [[-12, 0], [12, 0]]}))["curves"])[0].clone();
    run(&mut s, "sketch.finish", json!({}));
    let src = s.doc.features.last().unwrap().id;
    run(&mut s, "sketch.create", json!({"plane": "XY"}));
    let r = run(&mut s, "sketch.project_to_surface", json!({"curves": [{"sketch": src, "curve": l}], "face": [0, 0, 20]}));
    let wid = r["links"][0]["wires"][0].as_str().unwrap().to_string();
    let path_sketch = s.active_sketch.unwrap();
    let pts = {
        let st = s.model.state();
        let sk = &st.sketch(path_sketch).unwrap().sketch;
        sk.wires[sk.wire_index(&wid).unwrap()].pts.clone()
    };
    run(&mut s, "sketch.finish", json!({}));
    let len: f64 = pts.windows(2).map(|w| w[0].dist(w[1])).sum();
    // A pipe along it: volume ≈ π r² × length.
    let before = s.model.state().bodies.len();
    run(&mut s, "solid.pipe", json!({"path_sketch": path_sketch, "path": [wid.clone()], "diameter": "2 mm", "operation": "new"}));
    let st = s.model.state();
    assert_eq!(st.bodies.len(), before + 1);
    let v = st.bodies.last().unwrap().mesh().measure().volume;
    let want = std::f64::consts::PI * len;
    assert!((v - want).abs() < 0.05 * want, "{v} vs {want}");
    drop(st);
    // A sweep of a square profile drawn square to the curve at its start.
    let (a, b) = (pts[0], pts[1]);
    let t = (b - a).normalized().unwrap();
    let x = t.cross(solidcraft_geom::Vec3::Z).normalized().unwrap();
    let y = t.cross(x);
    run(&mut s, "sketch.create", json!({"plane": {"origin": [a.x, a.y, a.z], "x_dir": [x.x, x.y, x.z], "y_dir": [y.x, y.y, y.z]}}));
    run(&mut s, "sketch.rectangle.two_point", json!({"p0": [-1, -1], "p1": [1, 1]}));
    let prof = s.active_sketch.unwrap();
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.sweep", json!({"sketch": prof, "path_sketch": path_sketch, "path": [wid], "operation": "new"}));
    let st = s.model.state();
    assert_eq!(st.bodies.len(), before + 2);
    let v = st.bodies.last().unwrap().mesh().measure().volume;
    assert!((v - 4.0 * len).abs() < 0.05 * 4.0 * len, "{v} vs {}", 4.0 * len);
}

/// #47: references to other bodies: an edge, a face's loops and a silhouette of a second body
/// project into a sketch on the first; they follow an edit of that body (a parameter) and
/// are reported lost, not crashed on, when it goes away.
#[test]
fn project_and_include_from_other_bodies() {
    let mut s = Session::default();
    run(&mut s, "parameters.change", json!({"name": "w", "expression": "10 mm"}));
    run(&mut s, "solid.box", json!({"corner": [0, 0, 0], "length": 40, "width": 30, "height": 5}));
    run(&mut s, "solid.box", json!({"corner": [50, 0, 0], "length": "w", "width": 10, "height": 20, "operation": "new"}));
    run(&mut s, "solid.cylinder", json!({"base": [80, 5, 0], "radius": 4, "height": 10, "operation": "new"}));
    run(&mut s, "sketch.create", json!({"plane": {"face": [20, 15, 5]}, "project_edges": false}));
    // An edge on the second box's top, its top face's loop, the cylinder's silhouette.
    run(&mut s, "sketch.project", json!({"refs": [{"edge": [55, 0, 20]}]}));
    run(&mut s, "sketch.project", json!({"refs": [{"face": [55, 5, 20]}]}));
    run(&mut s, "sketch.include_3d", json!({"refs": [{"edge": [80, 1, 10]}]}));
    run(&mut s, "sketch.project", json!({"refs": [{"body": "Body3"}]}));
    let si = inspect(&mut s);
    let lc = linked_curves(&si);
    // (Geometry already there is not drawn twice: the edge lies on the face's loop, the included
    // circle on the silhouette.)
    assert!(lc.len() >= 6, "{}", lc.len());
    // The projected edge of the second box runs from x 50 to 60 at y 0.
    let xs = |si: &Value| -> Vec<f64> {
        linked_curves(si).iter().flat_map(|c| [c["start_at"][0].as_f64().unwrap_or(0.0), c["end_at"][0].as_f64().unwrap_or(0.0)]).collect()
    };
    assert!(xs(&si).iter().any(|x| (x - 60.0).abs() < 1e-6), "{:?}", xs(&si));
    run(&mut s, "sketch.finish", json!({}));
    // Widen the second box: the projection follows (its far edge is at 70).
    run(&mut s, "parameters.change", json!({"name": "w", "expression": "20 mm"}));
    let id = s.doc.features.iter().find(|f| matches!(f.kind, solidcraft_doc::FeatureKind::Sketch { .. })).unwrap().id;
    let si = run(&mut s, "sketch.inspect", json!({"sketch": id}));
    assert!(xs(&si).iter().any(|x| (x - 70.0).abs() < 1e-6), "{:?}", xs(&si));
    // Remove the cylinder: its silhouette and edge are lost, with a warning and no error.
    let cyl = s.doc.features.iter().find(|f| matches!(f.kind, solidcraft_doc::FeatureKind::Cylinder { .. })).unwrap().id;
    run(&mut s, "timeline.delete", json!({"feature": cyl}));
    let t = run(&mut s, "document.inspect", json!({}));
    let row = t["timeline"].as_array().unwrap().iter().find(|r| r["id"] == json!(id)).cloned().unwrap();
    assert!(row["error"].is_null(), "{row}");
    assert!(row["warning"].as_str().is_some_and(|w| w.contains("lost")), "{row}");
}

/// A cylinder's wall and the torus of a rounded rim are drawn as one face but stored in halves
/// joined by hidden seams; projecting one projects the whole face, not an open half.
#[test]
fn faces_split_by_hidden_seams_project_whole() {
    let mut s = Session::default();
    run(&mut s, "sketch.create", json!({"plane":"XY"}));
    run(&mut s, "sketch.circle.center", json!({"center":[0,0],"radius":20}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.extrude", json!({"distance":10}));
    run(&mut s, "sketch.create", json!({"plane":"XZ"}));
    run(&mut s, "sketch.project", json!({"refs":[{"face":[20,0,5]}]}));
    let wall = run(&mut s, "sketch.inspect", json!({}));
    let heights: Vec<f64> =
        wall["curves"].as_array().unwrap().iter().flat_map(|c| [c["start_at"][1].as_f64().unwrap(), c["end_at"][1].as_f64().unwrap()]).collect();
    assert!(heights.iter().any(|y| y.abs() < 1e-6) && heights.iter().any(|y| (y.abs() - 10.).abs() < 1e-6), "{wall}");
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.fillet", json!({"edges":[[20,0,10]],"radius":3}));
    run(&mut s, "sketch.create", json!({"plane":"XY"}));
    let t = 0.5f64.sqrt() * 3.;
    run(&mut s, "sketch.project", json!({"refs":[{"face":[17. + t, 0, 7. + t]}]}));
    let round = run(&mut s, "sketch.inspect", json!({}));
    let mut radii: Vec<f64> = round["curves"].as_array().unwrap().iter().map(|c| c["radius"].as_f64().unwrap()).collect();
    radii.sort_by(f64::total_cmp);
    assert_eq!(radii.len(), 4, "{round}");
    assert!(radii[..2].iter().all(|r| (r - 17.).abs() < 1e-6) && radii[2..].iter().all(|r| (r - 20.).abs() < 1e-6), "{round}");
}

/// #635: a rectangle whose corner is made coincident with a projected edge of the solid under
/// the sketch, so that its bottom side lies on that edge, is still one closed profile (the
/// projected line stays, and names the shared edge as it was there first), and it extrudes to
/// its own volume, also when the design is opened again.
#[test]
fn a_rectangle_put_onto_a_projected_edge_stays_a_closed_profile() {
    let mut s = Session::default();
    run(&mut s, "sketch.create", json!({"plane": "XY"}));
    run(&mut s, "sketch.rectangle.two_point", json!({"p0": [0, 0], "p1": [40, 30]}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.extrude", json!({"distance": 20}));
    let block = s.model.state().bodies[0].name.clone();
    let id = run(&mut s, "sketch.create", json!({"plane": "XY"}))["sketch"].as_u64().unwrap();
    // The solid's bottom edge along X, from (0, 0) to (40, 0).
    let r = run(&mut s, "sketch.project", json!({"refs": [{"edge": [20, 0, 0]}]}));
    let projected = r["links"][0]["curves"][0].as_str().unwrap().to_string();
    // A 20 x 15 rectangle clear of it is a closed profile of its own.
    let sides = ids(&run(&mut s, "sketch.rectangle.two_point", json!({"p0": [5, 10], "p1": [25, 25]}))["curves"]);
    run(&mut s, "sketch.dimension", json!({"entities": [sides[0]], "value": 20}));
    run(&mut s, "sketch.dimension", json!({"entities": [sides[1]], "value": 15}));
    let mut own = sides.clone();
    own.sort();
    let profile_curves = |si: &Value| -> Vec<Vec<String>> {
        si["profiles"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| {
                let mut c = ids(&p["curves"]);
                c.sort();
                c
            })
            .collect()
    };
    assert_eq!(profile_curves(&inspect(&mut s)), vec![own.clone()]);
    // Its bottom-left corner onto the projected line: the bottom side lies on it.
    run(&mut s, "sketch.constraint.coincident", json!({"a": format!("{}.start", sides[0]), "b": projected}));
    let si = inspect(&mut s);
    assert_eq!(si["status"], "solved", "{si}");
    let curve = |id: &str| si["curves"].as_array().unwrap().iter().find(|c| c["id"] == id).unwrap().clone();
    let bottom = curve(&sides[0]);
    for end in ["start_at", "end_at"] {
        assert!(bottom[end][1].as_f64().unwrap().abs() < 1e-9, "{bottom}");
    }
    assert!(curve(&projected).get("link").is_some(), "the projected line stays");
    let mut on_line = vec![projected.clone(), sides[1].clone(), sides[2].clone(), sides[3].clone()];
    on_line.sort();
    assert_eq!(profile_curves(&si), vec![on_line], "{si}");
    assert!((si["profiles"][0]["area_mm2"].as_f64().unwrap() - 300.0).abs() < 1e-6, "{si}");
    // Picked by a point inside, it extrudes to a 20 x 15 x 10 block of its own.
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.extrude", json!({"sketch": id, "profiles": [{"point": [15, 7.5]}], "distance": 10, "operation": "new", "body_name": "Pad"}));
    let volume = |s: &Session, name: &str| s.model.state().body(name).unwrap().body.tessellate_with(1e-4, true).unwrap().measure().volume;
    assert!((volume(&s, "Pad") - 3000.0).abs() < 1e-6, "{}", volume(&s, "Pad"));
    assert!((volume(&s, &block) - 24000.0).abs() < 1e-6, "{}", volume(&s, &block));
    let reopened = Session::new(solidcraft_doc::Document::from_json(&s.doc.to_json()).unwrap());
    assert!((volume(&reopened, "Pad") - 3000.0).abs() < 1e-6, "{}", volume(&reopened, "Pad"));
}

/// A face with letters cut into it, extruded as a cut: its edges come into the face's sketch
/// as the letters' exact curves (a font's quadratics and cubics), not fitted to the mesh, so the
/// tool's walls lie on the letters' walls and the cut takes what the same region picked in the
/// sketch takes (#1360). Fitted to the mesh (684 lines, 31 arcs and 18 splines here), they lay
/// a few hundredths off the letters' walls, and the boolean found no result.
#[test]
fn a_face_engraved_with_text_projects_its_letters_exactly_and_extrudes() {
    let volume = |s: &mut Session| run(s, "inspect.measure", json!({}))["total"]["volume_mm3"].as_f64().unwrap();
    let engraved = || {
        let mut s = Session::default();
        run(&mut s, "sketch.create", json!({"plane":"XY"}));
        run(&mut s, "sketch.rectangle.two_point", json!({"p0":[0,0],"p1":[105,26]}));
        run(&mut s, "sketch.finish", json!({}));
        run(&mut s, "solid.extrude", json!({"distance":40}));
        run(&mut s, "sketch.create", json!({"plane":{"face":[50,13,40]}}));
        run(&mut s, "sketch.text", json!({"text":"textvraiment tres long","at":[3,9],"height":6.5}));
        run(&mut s, "sketch.finish", json!({}));
        run(&mut s, "solid.extrude", json!({"distance":-1,"operation":"cut"}));
        s
    };
    // The face round the letters, drawn in a sketch in its plane.
    let mut drawn = engraved();
    let before = volume(&mut drawn);
    run(&mut drawn, "sketch.create", json!({"plane":"XY","offset":40}));
    run(&mut drawn, "sketch.rectangle.two_point", json!({"p0":[0,0],"p1":[105,26]}));
    run(&mut drawn, "sketch.text", json!({"text":"textvraiment tres long","at":[3,9],"height":6.5}));
    run(&mut drawn, "sketch.finish", json!({}));
    run(&mut drawn, "solid.extrude", json!({"distance":-3.5,"operation":"cut","profiles":[{"point":[1,1]}]}));
    let taken = before - volume(&mut drawn);
    assert!(taken > 0.0, "{taken}");
    // The engraved face itself, extruded as a cut.
    let mut s = engraved();
    let id = run(&mut s, "solid.extrude", json!({"distance":-3.5,"operation":"cut","face":[1,1,40]}))["feature"].as_u64().unwrap();
    let r = s.model.result(id).unwrap();
    assert!(r.error.is_none(), "{r:?}");
    let face = before - volume(&mut s);
    assert!((face - taken).abs() < 1e-6 * before, "{face} {taken}");
    // Its sketch got the letters as Bézier pieces.
    let st = s.model.state();
    let sketch = &st.sketches.last().unwrap().sketch;
    let pieces =
        sketch.curves.iter().filter(|c| c.link.is_some() && matches!(c.kind, solidcraft_sketch::CurveKind::Spline { control: true, .. })).count();
    assert!(pieces > 100, "{pieces}");
}
