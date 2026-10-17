//! The Sweep and Surface Sweep dialogs' options (#771): orientation, twist and end scale reach
//! the commands, and editing a sweep keeps them.
use crate::dialogs::Kind;
use crate::dialogs_surface::Surf;
use crate::scenario::Harness;
use serde_json::json;
use solidcraft_engine::Sel;
use solidcraft_engine::doc::{FeatureKind, SweepOrientation};
use solidcraft_engine::geom::Vec3;

/// A square `Q` on XY and a path `P` 40 up the Z axis; the square's sketch id.
fn setup(h: &mut Harness) -> u64 {
    h.app.run("sketch.create", json!({"plane":"XY","name":"Q"})).unwrap();
    h.app.run("sketch.rectangle.two_point", json!({"p0":[-5,-5],"p1":[5,5]})).unwrap();
    h.app.run("sketch.finish", json!({})).unwrap();
    h.app.run("sketch.create", json!({"plane":"XZ","name":"P"})).unwrap();
    h.app.run("sketch.line", json!({"points":[[0,0],[0,-40]],"ids":["up"]})).unwrap();
    h.app.run("sketch.finish", json!({})).unwrap();
    h.app.session.doc.find_feature("Q").unwrap().id
}

#[test]
fn a_new_sweep_takes_its_options_from_the_dialog() {
    let mut h = Harness::new();
    let sketch = setup(&mut h);
    h.step(&json!({"call":"ui.start","params":{"command":"solid.sweep"}})).unwrap();
    h.frames(3);
    assert!(h.widget("Twist").is_some() && h.widget("End Scale").is_some() && h.widget("Orientation").is_some());
    let d = h.app.dialog.as_mut().unwrap();
    d.inputs[0].items = vec![Sel::Profile { sketch, index: 0 }];
    d.inputs[1].items = vec![Sel::SketchCurve { id: "up".into() }];
    d.inputs[2].items = vec![Sel::Axis { name: "X".into() }];
    let Kind::Sweep { opts, .. } = &mut d.kind else { panic!("{:?}", d.kind) };
    opts.orientation = 2;
    opts.twist = "90 deg".into();
    opts.scale = "0.5".into();
    h.frames(2);
    h.step(&json!({"widget":"OK"})).unwrap();
    assert!(h.app.dialog.is_none(), "{:?}", h.app.session.log);
    let f = h.app.session.doc.features.last().unwrap();
    let FeatureKind::Sweep { orientation, twist, scale, .. } = &f.kind else { panic!("{:?}", f.kind) };
    assert_eq!(*orientation, SweepOrientation::Direction(solidcraft_engine::doc::Dir(Vec3::X)));
    assert_eq!(twist.as_ref().map(|t| t.as_str()), Some("90 deg"));
    assert_eq!(scale.as_ref().map(|k| k.as_str()), Some("0.5"));
}

#[test]
fn a_plain_sweep_from_the_dialog_stores_no_options() {
    let mut h = Harness::new();
    let sketch = setup(&mut h);
    h.step(&json!({"call":"ui.start","params":{"command":"solid.sweep"}})).unwrap();
    h.frames(3);
    let d = h.app.dialog.as_mut().unwrap();
    d.inputs[0].items = vec![Sel::Profile { sketch, index: 0 }];
    d.inputs[1].items = vec![Sel::SketchCurve { id: "up".into() }];
    h.step(&json!({"widget":"OK"})).unwrap();
    assert!(h.app.dialog.is_none(), "{:?}", h.app.session.log);
    let f = h.app.session.doc.features.last().unwrap();
    let FeatureKind::Sweep { orientation, twist, scale, .. } = &f.kind else { panic!("{:?}", f.kind) };
    assert!(orientation.is_perpendicular() && twist.is_none() && scale.is_none());
}

#[test]
fn editing_sweeps_keeps_their_options() {
    let mut h = Harness::new();
    setup(&mut h);
    h.app
        .run(
            "solid.sweep",
            json!({"sketch":"Q","path_sketch":"P","path":["up"],"orientation":[1,1,0],"twist":"30 deg","scale":"2","operation":"new","name":"Solid"}),
        )
        .unwrap();
    h.app
        .run(
            "surface.sweep",
            json!({"sketch":"Q","path_sketch":"P","path":["up"],"orientation":"parallel","twist":"45 deg","scale":"0.5","name":"Sheet"}),
        )
        .unwrap();
    h.frames(4);
    for name in ["Solid", "Sheet"] {
        let before = h.app.session.doc.find_feature(name).unwrap().clone();
        h.step(&json!({"call":"ui.editFeature","params":{"feature":name}})).unwrap();
        h.frames(8);
        match &h.app.dialog.as_ref().unwrap().kind {
            Kind::Sweep { opts, .. } | Kind::Surface(Surf::Sweep(opts)) => assert_eq!(opts.twist, if name == "Solid" { "30 deg" } else { "45 deg" }),
            k => panic!("{k:?}"),
        }
        h.step(&json!({"widget":"OK"})).unwrap();
        assert!(h.app.dialog.is_none(), "{:?}", h.app.session.log);
        // The options come back as they were (the profile pick is stored by its curves).
        let options = |k: &FeatureKind| match k {
            FeatureKind::Sweep { orientation, twist, scale, .. } | FeatureKind::SurfaceSweep { orientation, twist, scale, .. } => {
                (*orientation, twist.clone(), scale.clone())
            }
            k => panic!("{k:?}"),
        };
        let after = h.app.session.doc.find_feature(name).unwrap();
        assert_eq!(options(&after.kind), options(&before.kind), "{name}");
        assert_eq!(after.param_names, before.param_names, "{name}");
    }
}

/// A rail picked in the dialog reaches the command with its sketch, the fit with it; editing the
/// sweep brings both back and keeps them.
#[test]
fn a_rail_picked_in_the_dialog_is_kept_through_an_edit() {
    let mut h = Harness::new();
    let sketch = setup(&mut h);
    h.app.run("sketch.create", json!({"plane":"XZ","name":"R"})).unwrap();
    h.app.run("sketch.line", json!({"points":[[5,0],[10,-40]],"ids":["out"]})).unwrap();
    h.app.run("sketch.finish", json!({})).unwrap();
    let rail_sketch = h.app.session.doc.find_feature("R").unwrap().id;
    h.step(&json!({"call":"ui.start","params":{"command":"solid.sweep"}})).unwrap();
    h.frames(3);
    assert!(h.widget("Rail Fit").is_some());
    let d = h.app.dialog.as_mut().unwrap();
    d.inputs[0].items = vec![Sel::Profile { sketch, index: 0 }];
    d.inputs[1].items = vec![Sel::SketchCurve { id: "up".into() }];
    d.inputs[3].items = vec![Sel::SketchCurve { id: "out".into() }];
    let Kind::Sweep { opts, .. } = &mut d.kind else { panic!("{:?}", d.kind) };
    opts.rail_fit = 1;
    h.frames(2);
    h.step(&json!({"widget":"OK"})).unwrap();
    assert!(h.app.dialog.is_none(), "{:?}", h.app.session.log);
    let name = h.app.session.doc.features.last().unwrap().name.clone();
    let rails = |h: &Harness| match &h.app.session.doc.find_feature(&name).unwrap().kind {
        FeatureKind::Sweep { rails, rail_fit, .. } => (rails.clone(), *rail_fit),
        k => panic!("{k:?}"),
    };
    let (stored, fit) = rails(&h);
    assert_eq!(stored, vec![solidcraft_engine::doc::SweepRail { sketch: solidcraft_engine::doc::SketchId(rail_sketch), curves: vec!["out".into()] }]);
    assert_eq!(fit, solidcraft_engine::doc::RailFit::Stretch);
    h.step(&json!({"call":"ui.editFeature","params":{"feature":name}})).unwrap();
    h.frames(8);
    let d = h.app.dialog.as_ref().unwrap();
    assert_eq!(d.inputs[3].items, vec![Sel::SketchCurve { id: "out".into() }]);
    h.step(&json!({"widget":"OK"})).unwrap();
    assert!(h.app.dialog.is_none(), "{:?}", h.app.session.log);
    assert_eq!(rails(&h), (stored, fit));
}
