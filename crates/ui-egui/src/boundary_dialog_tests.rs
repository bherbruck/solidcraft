//! The Boundary Surface dialog (#763): a loop of sketch curves becomes the loop patch, edges and
//! curves across become a network with its continuity and operation, curves that meet end to end
//! are one chain, and Edit Feature brings a network back as it was.
use crate::dialogs::Kind;
use crate::dialogs_surface::Surf;
use crate::scenario::Harness;
use serde_json::{Map, json};
use solidcraft_engine::Sel;
use solidcraft_engine::doc::{FeatureKind, Operation, kernel::Continuity};
use solidcraft_engine::geom::Vec3;

fn ok(h: &mut Harness) {
    h.frames(2);
    h.step(&json!({"widget":"OK"})).unwrap();
    // An edit is applied off the UI thread: give it the frames it takes.
    for _ in 0..200 {
        if h.app.dialog.is_none() {
            break;
        }
        h.frames(1);
    }
    assert!(h.app.dialog.is_none(), "{:?} {:?} {:?}", h.app.preview.error, h.app.dialog.as_ref().map(|d| (&d.error, &d.inputs)), h.app.session.log);
}

#[test]
fn a_loop_of_sketch_curves_is_the_loop_patch() {
    let mut h = Harness::new();
    h.app.run("sketch.create", json!({"plane":"XY","name":"Loop"})).unwrap();
    for (id, a, b) in [("s1", [0, 0], [10, 0]), ("s2", [10, 0], [10, 10]), ("s3", [10, 10], [0, 10]), ("s4", [0, 10], [0, 0])] {
        h.app.run("sketch.line", json!({"points":[a, b],"ids":[id]})).unwrap();
    }
    h.app.run("sketch.finish", json!({})).unwrap();
    h.step(&json!({"call":"ui.start","params":{"command":"surface.boundary"}})).unwrap();
    h.frames(3);
    assert!(h.widget("Cross curves").is_some() && h.widget("Continuity").is_some() && h.widget("Operation").is_some());
    let d = h.app.dialog.as_mut().unwrap();
    d.inputs[0].items = ["s1", "s2", "s3", "s4"].iter().map(|id| Sel::SketchCurve { id: (*id).into() }).collect();
    ok(&mut h);
    let f = h.app.session.doc.features.last().unwrap();
    let FeatureKind::SurfaceBoundary { curves, orders, .. } = &f.kind else { panic!("{:?}", f.kind) };
    assert_eq!(curves, &["s1", "s2", "s3", "s4"]);
    assert!(orders.iter().all(|o| *o == Continuity::G1), "{orders:?}");
    assert_eq!(h.app.session.world_state().bodies.len(), 1);
}

/// A quarter cylinder sheet (its top edge open at y = 10, z = 0) and a 3D line above it.
fn cylinder_and_line(h: &mut Harness) -> String {
    h.app.run("sketch.create", json!({"plane":"YZ","name":"Arc"})).unwrap();
    let c = 10.0 * std::f64::consts::FRAC_1_SQRT_2;
    let r = h.app.run("sketch.arc.three_point", json!({"start":[0,-10],"end":[10,0],"through":[c,-c]})).unwrap();
    let arc = r["created"]["curves"][0].as_str().unwrap().to_string();
    h.app.run("sketch.finish", json!({})).unwrap();
    h.app.run("surface.extrude", json!({"sketch":"Arc","curves":[arc],"distance":10})).unwrap();
    h.app.run("sketch.create", json!({"plane":"XY","name":"Top"})).unwrap();
    h.app.run("sketch.options", json!({"sketch_3d": true})).unwrap();
    let r = h.app.run("sketch.line3d", json!({"points":[[0,3,12],[10,3,12]]})).unwrap();
    h.app.run("sketch.finish", json!({})).unwrap();
    r["curves"][0].as_str().unwrap().to_string()
}

#[test]
fn edges_and_curves_make_a_network_and_come_back_in_an_edit() {
    let mut h = Harness::new();
    let top = cylinder_and_line(&mut h);
    let rim = crate::dialogs::edge_sel(&h.app.session, Vec3::new(5.0, 10.0, 0.0)).unwrap();
    let Sel::Edge { body, .. } = &rim else { panic!("{rim:?}") };
    let body = body.clone();
    h.step(&json!({"call":"ui.start","params":{"command":"surface.boundary"}})).unwrap();
    h.frames(3);
    let d = h.app.dialog.as_mut().unwrap();
    d.inputs[0].items = vec![rim.clone(), Sel::SketchCurve { id: top.clone() }];
    let Kind::Surface(Surf::Boundary { continuity, join }) = &mut d.kind else { panic!("{:?}", d.kind) };
    *continuity = 2;
    *join = true;
    ok(&mut h);
    let name = h.app.session.doc.features.last().unwrap().name.clone();
    let network = |h: &Harness| match &h.app.session.doc.find_feature(&name).unwrap().kind {
        FeatureKind::SurfaceNetwork { u, v, operation } => (u.clone(), v.clone(), *operation),
        k => panic!("{k:?}"),
    };
    let (u, v, operation) = network(&h);
    assert_eq!((u.len(), v.len(), operation), (2, 0, Operation::Join));
    assert_eq!(u[0].body.as_ref().map(|b| b.to_string()), Some(body.clone()));
    assert_eq!(u[0].continuity, Continuity::G2);
    assert_eq!(u[1].curves, vec![top.clone()]);
    // Joined into the sheet: one body, two faces.
    let st = h.app.session.world_state();
    assert_eq!(st.bodies.len(), 1);
    assert_eq!(st.body(&body).map(|b| b.body.face_count()), Some(2));
    drop(st);
    h.step(&json!({"call":"ui.editFeature","params":{"feature":name}})).unwrap();
    h.frames(8);
    let d = h.app.dialog.as_ref().unwrap();
    assert!(matches!(d.kind, Kind::Surface(Surf::Boundary { continuity: 2, join: true })), "{:?}", d.kind);
    assert_eq!(d.inputs[0].items.len(), 2, "{:?}", d.inputs[0].items);
    ok(&mut h);
    let (u2, v2, op2) = network(&h);
    assert_eq!((u2.len(), v2.len(), op2), (2, 0, Operation::Join));
    assert_eq!((u2[0].continuity, &u2[1].curves), (Continuity::G2, &vec![top]));
}

/// Picks that meet end to end in one sketch are one curve of the network; a pick elsewhere is
/// another.
#[test]
fn curves_that_meet_are_one_chain() {
    let mut h = Harness::new();
    h.app.run("sketch.create", json!({"plane":"XY","name":"Net"})).unwrap();
    h.app.run("sketch.line", json!({"points":[[0,0],[5,0]],"ids":["a1"]})).unwrap();
    h.app.run("sketch.line", json!({"points":[[5,0],[10,0]],"ids":["a2"]})).unwrap();
    h.app.run("sketch.line", json!({"points":[[0,8],[10,8]],"ids":["b"]})).unwrap();
    h.app.run("sketch.line", json!({"points":[[0,0],[0,8]],"ids":["c"]})).unwrap();
    h.app.run("sketch.line", json!({"points":[[10,0],[10,8]],"ids":["d"]})).unwrap();
    h.app.run("sketch.finish", json!({})).unwrap();
    let sketch = h.app.session.doc.find_feature("Net").unwrap().id;
    let k = Surf::Boundary { continuity: 0, join: false };
    let (_, mut inputs) = crate::dialogs_surface::start(&h.app, "surface.boundary").unwrap();
    inputs[0].items = ["a2", "b", "a1"].iter().map(|id| Sel::SketchCurve { id: (*id).into() }).collect();
    inputs[1].items = ["c", "d"].iter().map(|id| Sel::SketchCurve { id: (*id).into() }).collect();
    let cmds = crate::dialogs_surface::commands(&h.app, &k, &inputs, &Map::new()).unwrap();
    let (id, p) = &cmds[0];
    assert_eq!(id, "surface.boundary");
    assert_eq!(p["u"], json!([{"sketch": sketch, "curves": ["a2", "a1"]}, {"sketch": sketch, "curves": ["b"]}]), "{p}");
    assert_eq!(p["v"], json!([{"sketch": sketch, "curves": ["c"]}, {"sketch": sketch, "curves": ["d"]}]), "{p}");
    // And it builds: the chain a1–a2 is one boundary of the patch.
    h.app.run("surface.boundary", p.clone()).unwrap();
    assert_eq!(h.app.session.world_state().bodies.len(), 1);
}
