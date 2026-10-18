use serde_json::{Value, json};
use solidcraft_geom::Vec3;

use crate::Session;

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    match s.execute(id, &p) {
        Ok(v) => v,
        Err(e) => panic!("{id} {p}: {e}"),
    }
}

/// A tray: a base `w` x 200 (its width a sketch dimension: the parameter returned), 1 mm thick
/// with 1 mm bends, front and back flanges 30 high in one feature, left and right in another,
/// and a 6.6 mm hole in the base: the two flange features' ids.
fn tray(s: &mut Session, w: f64) -> (String, u64, u64) {
    run(s, "sheet.manage_rules", json!({"name": "R1", "thickness": "1 mm", "bend_radius": "1 mm", "active": true}));
    run(s, "sketch.create", json!({"plane": "XY", "name": "Base"}));
    run(s, "sketch.rectangle.two_point", json!({"p0": [0, 0], "p1": [w, 200]}));
    let d = run(s, "sketch.dimension", json!({"entities": ["l1"], "value": w}));
    run(s, "sketch.finish", json!({}));
    run(s, "sheet.flange", json!({"sketch": "Base"}));
    run(s, "sheet.flange", json!({"edges": [[100, 0, 1], [100, 200, 1]], "height": 30}));
    let front_back = s.doc.features.last().map_or(0, |f| f.id);
    run(s, "sheet.flange", json!({"edges": [[0, 100, 1], [w, 100, 1]], "height": 30}));
    let sides = s.doc.features.last().map_or(0, |f| f.id);
    run(s, "solid.hole", json!({"position": [100, 100, 1], "diameter": 6.6}));
    let param = d["param"].as_str().or_else(|| d["name"].as_str()).map(str::to_string).unwrap_or_else(|| "d1".into());
    (param, front_back, sides)
}

/// Each face's readable sheet name, with its centroid and cylinder radius.
fn faces(s: &mut Session) -> Vec<(Option<String>, Vec3, Option<f64>)> {
    let body = s.model.state().bodies.first().map(|b| b.name.clone()).unwrap_or_default();
    let r = run(s, "model.faces", json!({"body": body}));
    r["faces"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|f| {
            let c = &f["centroid"];
            let at = Vec3::new(c[0].as_f64().unwrap_or(f64::NAN), c[1].as_f64().unwrap_or(f64::NAN), c[2].as_f64().unwrap_or(f64::NAN));
            (f["sheet_face"].as_str().map(str::to_string), at, f["cylinder"]["radius"].as_f64())
        })
        .collect()
}

fn named<'a>(faces: &'a [(Option<String>, Vec3, Option<f64>)], name: &str) -> Vec<&'a (Option<String>, Vec3, Option<f64>)> {
    faces.iter().filter(|f| f.0.as_deref() == Some(name)).collect()
}

/// The point on the face with this name, as a command taking a face name finds it.
fn point(s: &Session, name: &str) -> Vec3 {
    solidcraft_doc::naming::named_face_point(&s.model.state(), None, name).unwrap_or_else(|e| panic!("{name}: {e}"))
}

#[test]
fn a_trays_faces_have_readable_sheet_names_that_follow_its_size() {
    let mut s = Session::default();
    let (width, fb, sd) = tray(&mut s, 320.0);
    let all = faces(&mut s);
    // Every face but the hole's wall (two halves) has a sheet name, and no two faces share one.
    let unnamed: Vec<_> = all.iter().filter(|f| f.0.is_none()).collect();
    assert!(unnamed.len() == 2 && unnamed.iter().all(|f| f.2.is_some_and(|r| (r - 3.3).abs() < 1e-6)), "only the hole's wall: {unnamed:?}");
    let mut names: Vec<&str> = all.iter().filter_map(|f| f.0.as_deref()).collect();
    names.sort_unstable();
    let n = names.len();
    names.dedup();
    assert_eq!(names.len(), n, "{names:?}");
    for (name, at) in [
        ("sheet:Body1:base:top".to_string(), Vec3::new(160.0, 100.0, 1.0)),
        ("sheet:Body1:base:bottom".to_string(), Vec3::new(160.0, 100.0, 0.0)),
        (format!("sheet:Body1:F{sd}.1:top"), Vec3::new(319.0, 100.0, 16.0)),
        (format!("sheet:Body1:F{sd}.1:bottom"), Vec3::new(320.0, 100.0, 16.0)),
        (format!("sheet:Body1:F{sd}.1:rim.far"), Vec3::new(319.5, 100.0, 30.0)),
        (format!("sheet:Body1:F{fb}.0:bottom"), Vec3::new(160.0, 0.0, 16.0)),
        // The front flange runs on past its bend to the right one, under its corner.
        (format!("sheet:Body1:F{fb}.0:rim.ext1"), Vec3::new(319.0, 0.5, 2.0)),
    ] {
        let f = named(&all, &name);
        assert!(f.len() == 1 && f[0].1.dist(at) < 2.5, "{name}: {f:?}");
    }
    // A bend's two curved faces: its radius and the radius plus the thickness.
    let radius = |name: &str| named(&all, name).first().and_then(|f| f.2);
    assert_eq!(radius(&format!("sheet:Body1:F{fb}.0:bend.inner")).map(|r| (r * 1e6).round() / 1e6), Some(1.0));
    assert_eq!(radius(&format!("sheet:Body1:F{fb}.0:bend.outer")).map(|r| (r * 1e6).round() / 1e6), Some(2.0));
    // 80 mm wider: the same names, each on the same face where it now is.
    let right = point(&s, &format!("sheet:Body1:F{sd}.1:top"));
    assert!((right.x - 319.0).abs() < 1e-3, "{right:?}");
    run(&mut s, "parameters.change", json!({"name": width, "expression": "400"}));
    let wider = faces(&mut s);
    let mut again: Vec<&str> = wider.iter().filter_map(|f| f.0.as_deref()).collect();
    again.sort_unstable();
    assert_eq!(again, names);
    let right = point(&s, &format!("sheet:Body1:F{sd}.1:top"));
    assert!((right.x - 399.0).abs() < 1e-3, "{right:?}");
    let far = point(&s, &format!("sheet:Body1:F{fb}.1:rim.far"));
    assert!((far.y - 199.5).abs() < 1e-3 && (far.z - 30.0).abs() < 1e-3, "{far:?}");
}

#[test]
fn a_flanges_sheet_names_hold_when_another_flange_goes() {
    let mut s = Session::default();
    let (_, fb, sd) = tray(&mut s, 320.0);
    let right = format!("sheet:Body1:F{sd}.1:top");
    let before = point(&s, &right);
    run(&mut s, "timeline.suppress", json!({"feature": fb, "suppressed": true}));
    let after = point(&s, &right);
    assert!((after.x - 319.0).abs() < 1e-3 && (after.z - before.z).abs() < 16.0, "{before:?} {after:?}");
    // The names of the flanges that went are gone; asking for one says so.
    let gone = format!("sheet:Body1:F{fb}.0:top");
    assert!(solidcraft_doc::naming::named_face_point(&s.model.state(), None, &gone).is_err());
    assert!(solidcraft_doc::naming::named_face_point(&s.model.state(), Some("Body2"), &right).is_err());
}
