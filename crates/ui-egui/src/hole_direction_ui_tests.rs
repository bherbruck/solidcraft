//! Which way a hole picked in the viewport drills, as the dialog's arrow shows it and as the
//! result has it: into the face that was picked (also the underside of a thin plate on XY, and a
//! round face), and, at sketch points, into the part unless Flip is on.

use serde_json::json;
use solidcraft_engine::doc::FeatureKind;
use solidcraft_engine::geom::Vec3;

use crate::scenario::Harness;

fn solid_at(h: &Harness, p: Vec3) -> bool {
    h.app.session.model.state().bodies.iter().any(|b| b.body.contains(p))
}

fn screen(h: &mut Harness, p: Vec3) -> [f64; 2] {
    h.at(&json!({"world": [p.x, p.y, p.z]})).unwrap()
}

/// The open dialog's arrow: where it stands and the outward normal it points against.
fn arrow(h: &Harness) -> (Vec3, Vec3) {
    let d = h.app.dialog.as_ref().expect("the hole dialog");
    let (at, n) = crate::canvas::anchor(&h.app, d).expect("an anchor");
    (at, n.expect("an arrow"))
}

fn last_hole_direction(h: &Harness) -> Vec3 {
    match &h.app.session.doc.features.last().unwrap().kind {
        FeatureKind::Hole { direction, .. } => direction.0,
        other => panic!("the last feature is not a hole: {}", other.type_name()),
    }
}

#[test]
fn a_hole_picked_under_a_thin_plate_on_xy_drills_up_into_it_as_its_arrow_shows() {
    let mut h = Harness::new();
    h.app.set_origin_shown(false);
    h.app.run("sketch.create", json!({"plane": "XY", "name": "S"})).unwrap();
    h.app.run("sketch.rectangle.two_point", json!({"p0": [0, 0], "p1": [150, 100]})).unwrap();
    h.app.run("sketch.finish", json!({})).unwrap();
    h.app.run("solid.extrude", json!({"sketch": "S", "distance": 1.5})).unwrap();
    h.step(&json!({"call": "ui.view", "params": {"view": "bottom"}})).unwrap();
    h.settle();
    h.step(&json!({"start": "solid.hole"})).unwrap();
    let click = screen(&mut h, Vec3::new(75.0, 50.0, 0.0));
    h.step(&json!({"click": click})).unwrap();
    h.frames(2);
    let (_, n) = arrow(&h);
    assert!(n.dist(Vec3::new(0.0, 0.0, -1.0)) < 1e-6, "the arrow stands on the underside, facing down: {n:?}");
    if let Some(crate::dialogs::Dialog { kind: crate::dialogs::Kind::Hole { depth, opts, .. }, .. }) = h.app.dialog.as_mut() {
        *depth = "1 mm".into();
        opts.all = false;
        opts.angled = false;
    }
    h.step(&json!({"key": "Enter"})).unwrap();
    assert!(h.app.dialog.is_none(), "OK closed the dialog");
    // The hole goes up into the plate, 1 mm, against the arrow's normal (it used to go through).
    let dir = last_hole_direction(&h);
    assert!(dir.dist(-n) < 1e-6, "the hole drills along {dir:?}, the arrow says {:?}", -n);
    assert!(!solid_at(&h, Vec3::new(75.0, 50.0, 0.5)), "the bore starts at the underside");
    assert!(solid_at(&h, Vec3::new(75.0, 50.0, 1.25)), "and stops 1 mm in");
}

#[test]
fn a_hole_picked_on_a_round_bar_drills_to_its_axis_as_its_arrow_shows() {
    let mut h = Harness::new();
    h.app.set_origin_shown(false);
    h.app.run("solid.cylinder", json!({"radius": 10, "height": 40})).unwrap();
    h.step(&json!({"call": "ui.view", "params": {"view": "front"}})).unwrap();
    h.settle();
    h.step(&json!({"start": "solid.hole"})).unwrap();
    // The side facing the viewer (-y), at mid-height.
    let click = screen(&mut h, Vec3::new(0.0, -10.0, 20.0));
    h.step(&json!({"click": click})).unwrap();
    h.frames(2);
    let (at, n) = arrow(&h);
    assert!((at.dist(Vec3::new(0.0, -10.0, 20.0))) < 0.5, "the round face is picked where clicked: {at:?}");
    assert!(n.dist(Vec3::new(0.0, -1.0, 0.0)) < 0.05, "the arrow stands out of the face, radially: {n:?}");
    if let Some(crate::dialogs::Dialog { kind: crate::dialogs::Kind::Hole { depth, diameter, opts, .. }, .. }) = h.app.dialog.as_mut() {
        *depth = "5 mm".into();
        *diameter = "4 mm".into();
        opts.all = false;
        opts.angled = false;
    }
    h.step(&json!({"key": "Enter"})).unwrap();
    assert!(h.app.dialog.is_none(), "OK closed the dialog: {:?}", h.app.dialog.as_ref().map(|d| &d.error));
    let holes = h.app.session.doc.features.iter().filter(|f| matches!(f.kind, FeatureKind::Hole { .. })).count();
    assert_eq!(holes, 1, "one hole where it was clicked (the round face is picked whole, both halves)");
    let dir = last_hole_direction(&h);
    assert!(dir.dist(Vec3::new(0.0, 1.0, 0.0)) < 0.05, "the hole drills toward the axis: {dir:?}");
    assert!(!solid_at(&h, Vec3::new(at.x, -7.5, 20.0)), "the bore is 5 mm deep");
    assert!(solid_at(&h, Vec3::new(at.x, -4.5, 20.0)), "and stops there");
}

#[test]
fn flip_drills_holes_at_sketch_points_the_other_way() {
    let mut h = Harness::new();
    h.app.set_origin_shown(false);
    h.app.run("solid.box", json!({"length": 40, "width": 30, "height": 20})).unwrap();
    // A point on a plane through the middle of the block: by itself the hole goes down from the top.
    h.app.run("sketch.create", json!({"plane": {"origin": [0, 0, 10], "x_dir": [1, 0, 0], "y_dir": [0, 1, 0]}, "name": "P"})).unwrap();
    h.app.run("sketch.point", json!({"point": [10, 10], "id": "q"})).unwrap();
    h.app.run("sketch.finish", json!({})).unwrap();
    let sketch = h.app.session.doc.find_feature("P").unwrap().id;
    h.step(&json!({"start": "solid.hole"})).unwrap();
    if let Some(d) = h.app.dialog.as_mut() {
        if let crate::dialogs::Kind::Hole { depth, opts, .. } = &mut d.kind {
            opts.multiple = true;
            opts.all = false;
            opts.angled = false;
            *depth = "5 mm".into();
        }
        d.inputs = vec![crate::selection::SelInput::new("Points", crate::selection::POINTS, true)];
        d.inputs[0].items = vec![solidcraft_engine::Sel::SketchPoint { id: format!("{sketch}:q") }];
    }
    h.frames(2);
    // The arrow shows the way the hole goes: down from the top, and up once Flip is on.
    let (_, n) = arrow(&h);
    assert!(n.dist(Vec3::new(0.0, 0.0, 1.0)) < 1e-9, "unflipped, the hole goes down: the arrow stands on +z ({n:?})");
    h.app.dialog.as_mut().unwrap().extra.insert("flip".into(), json!(true));
    h.frames(2);
    let (_, n) = arrow(&h);
    assert!(n.dist(Vec3::new(0.0, 0.0, -1.0)) < 1e-9, "flipped, it goes up: {n:?}");
    h.step(&json!({"key": "Enter"})).unwrap();
    assert!(h.app.dialog.is_none(), "OK closed the dialog: {:?}", h.app.dialog.as_ref().map(|d| &d.error));
    assert!(!solid_at(&h, Vec3::new(10.0, 10.0, 2.5)), "flipped: the hole comes up from the bottom");
    assert!(solid_at(&h, Vec3::new(10.0, 10.0, 17.5)), "and not down from the top");
    // Edit Feature shows Flip on, and OK keeps it.
    let id = h.app.session.doc.features.last().unwrap().id;
    h.step(&json!({"call": "ui.editFeature", "params": {"feature": id}})).unwrap();
    h.frames(4);
    let d = h.app.dialog.as_ref().expect("the hole dialog");
    assert_eq!(d.extra.get("flip"), Some(&json!(true)));
    h.step(&json!({"key": "Enter"})).unwrap();
    assert!(!solid_at(&h, Vec3::new(10.0, 10.0, 2.5)) && solid_at(&h, Vec3::new(10.0, 10.0, 17.5)));
}
