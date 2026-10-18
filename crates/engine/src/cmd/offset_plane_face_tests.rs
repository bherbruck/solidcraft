#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use crate::Session;
use serde_json::{Value, json};
use solidcraft_doc::construct::ConstructGeom;
use solidcraft_doc::{Document, FeatureKind, PlaneRef};
use solidcraft_geom::Plane;

fn run(s: &mut Session, command: &str, params: Value) -> Value {
    s.execute(command, &params).unwrap_or_else(|e| panic!("{command} {params}: {e}"))
}

fn plane(s: &Session, name: &str) -> Plane {
    let state = s.model.state();
    match state.construct.iter().find(|c| c.name == name).map(|c| &c.geom) {
        Some(ConstructGeom::Plane(p)) => *p,
        other => panic!("construction plane `{name}`: {other:?}"),
    }
}

fn volume(s: &Session) -> f64 {
    s.model.state().bodies.iter().map(|b| solidcraft_kernel::measure(&b.body).unwrap().volume).sum()
}

#[test]
fn offset_plane_from_a_planar_face_follows_the_face() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 40, "width": 30, "height": 20, "name": "Base"}));

    // Top face (z = 20), 5 mm out along its normal.
    run(&mut s, "construct.plane.offset", json!({"base": {"face": [5, 5, 20]}, "offset": 5, "name": "Above"}));
    let above = plane(&s, "Above");
    assert!((above.origin.z - 25.0).abs() < 1e-9, "{above:?}");
    assert!((above.normal().z.abs() - 1.0).abs() < 1e-9, "{above:?}");
    // Stored as an exact face attachment, not a frozen copy of the plane.
    let feature = s.doc.find_feature("Above").unwrap();
    let FeatureKind::ConstructionPlane { plane: PlaneRef::Offset { base, .. } } = &feature.kind else { panic!("offset plane") };
    assert!(matches!(base.as_ref(), PlaneRef::Face { anchor: Some(_), .. }), "{base:?}");

    // Side face (x = 40), 3 mm out.
    run(&mut s, "construct.plane.offset", json!({"base": {"face": [40, 15, 10]}, "offset": 3, "name": "Side"}));
    let side = plane(&s, "Side");
    assert!((side.origin.x - 43.0).abs() < 1e-9, "{side:?}");
    assert!((side.normal().x.abs() - 1.0).abs() < 1e-9, "{side:?}");

    // A 10 × 10 × 10 pad sketched on the new plane sits 5 mm above the box.
    run(&mut s, "sketch.create", json!({"plane": "Above"}));
    run(&mut s, "sketch.rectangle.two_point", json!({"p0": [0, 0], "p1": [10, 10]}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.extrude", json!({"distance": 10, "operation": "new"}));
    s.refresh();
    assert!(s.model.results.iter().all(|r| r.error.is_none()), "{:?}", s.model.results);
    assert!((volume(&s) - (40.0 * 30.0 * 20.0 + 1000.0)).abs() < 1e-6, "{}", volume(&s));

    // Taller and longer box: both planes move with their faces.
    run(&mut s, "timeline.edit", json!({"feature": "Base", "set": {"height": "30", "length": "50"}}));
    assert!((plane(&s, "Above").origin.z - 35.0).abs() < 1e-9);
    assert!((plane(&s, "Side").origin.x - 53.0).abs() < 1e-9);
    assert!(s.model.results.iter().all(|r| r.error.is_none()), "{:?}", s.model.results);

    // Saved and reopened, the attachment still resolves.
    let reopened = Session::new(Document::from_json(&s.doc.to_json()).unwrap());
    assert!((plane(&reopened, "Above").origin.z - 35.0).abs() < 1e-9);
    assert!((plane(&reopened, "Side").origin.x - 53.0).abs() < 1e-9);
}

#[test]
fn offset_plane_rejects_bad_face_picks_without_panicking() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 40, "width": 30, "height": 20}));
    for base in [
        json!({"face": [100, 100, 100]}),
        json!({"face": [1e300, 0, 0]}),
        json!({"face": "top"}),
        json!({"face": [5, 5]}),
        json!({"face": [5, 5, 20], "index": 0}),
        json!({"face": [5, 5, 20], "body": 7, "index": 0}),
    ] {
        let features = s.doc.features.len();
        assert!(s.execute("construct.plane.offset", &json!({"base": base, "offset": 5})).is_err(), "{base}");
        assert_eq!(s.doc.features.len(), features, "{base}");
    }
}

#[test]
fn plane_at_angle_from_a_planar_face_follows_the_face() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 40, "width": 30, "height": 20, "name": "Base"}));

    // Top face (z = 20), 45 degrees about X axis.
    run(&mut s, "construct.plane.at_angle", json!({"base": {"face": [5, 5, 20]}, "axis": "X", "angle": "45 deg", "name": "Slanted"}));
    let slanted = plane(&s, "Slanted");
    // Base plane has normal Z and its origin on the top face, (0, 0, 20). Rotated 45 degrees
    // about X: the origin is (0, -20/sqrt(2), 20/sqrt(2)) and the normal [0, -sin(45), cos(45)].
    let half = 1.0_f64 / 2.0_f64.sqrt();
    assert!(slanted.origin.x.abs() < 1e-9, "{slanted:?}");
    assert!((slanted.origin.y - -20.0 * half).abs() < 1e-9, "{slanted:?}");
    assert!((slanted.origin.z - 20.0 * half).abs() < 1e-9, "{slanted:?}");
    assert!((slanted.normal().y - (-half)).abs() < 1e-9, "{slanted:?}");
    assert!((slanted.normal().z - half).abs() < 1e-9, "{slanted:?}");

    let feature = s.doc.find_feature("Slanted").unwrap();
    let FeatureKind::ConstructionPlane { plane: PlaneRef::AtAngle { base, .. } } = &feature.kind else { panic!("at_angle plane") };
    assert!(matches!(base.as_ref(), PlaneRef::Face { anchor: Some(_), .. }), "{base:?}");

    // Height changes: the slanted plane moves with the top face (0, 0, 30).
    run(&mut s, "timeline.edit", json!({"feature": "Base", "set": {"height": "30"}}));
    let slanted2 = plane(&s, "Slanted");
    assert!(slanted2.origin.x.abs() < 1e-9, "{slanted2:?}");
    assert!((slanted2.origin.y - -30.0 * half).abs() < 1e-9, "{slanted2:?}");
    assert!((slanted2.origin.z - 30.0 * half).abs() < 1e-9, "{slanted2:?}");
    assert!((slanted2.normal().y - (-half)).abs() < 1e-9, "{slanted2:?}");
    assert!((slanted2.normal().z - half).abs() < 1e-9, "{slanted2:?}");

    // Saved and reopened, the attachment still resolves.
    let reopened = Session::new(Document::from_json(&s.doc.to_json()).unwrap());
    let reopened_slanted = plane(&reopened, "Slanted");
    assert!((reopened_slanted.origin.y - -30.0 * half).abs() < 1e-9, "{reopened_slanted:?}");
    assert!((reopened_slanted.origin.z - 30.0 * half).abs() < 1e-9, "{reopened_slanted:?}");
    assert!((reopened_slanted.normal().y - (-half)).abs() < 1e-9, "{reopened_slanted:?}");
}

#[test]
fn plane_at_angle_rejects_bad_face_picks_without_panicking() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 40, "width": 30, "height": 20}));
    for base in [
        json!({"face": [100, 100, 100]}),
        json!({"face": [1e300, 0, 0]}),
        json!({"face": "top"}),
        json!({"face": [5, 5]}),
        json!({"face": [5, 5, 20], "index": 0}),
        json!({"face": [5, 5, 20], "body": 7, "index": 0}),
    ] {
        let features = s.doc.features.len();
        assert!(s.execute("construct.plane.at_angle", &json!({"base": base, "axis": "X", "angle": "45 deg"})).is_err(), "{base}");
        assert_eq!(s.doc.features.len(), features, "{base}");
    }
}
