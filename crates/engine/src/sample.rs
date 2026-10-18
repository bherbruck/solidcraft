//! A sample design (used by `solidcraft --sample` and demos): a parametric plate with rounded
//! corners, two holes and a boss.

use serde_json::{Value, json};

pub fn script() -> Value {
    json!({"commands": [
        {"command": "parameters.change", "params": {"name": "width", "expression": "80 mm", "comment": "plate width"}},
        {"command": "parameters.change", "params": {"name": "depth", "expression": "50 mm"}},
        {"command": "parameters.change", "params": {"name": "thickness", "expression": "8 mm"}},
        {"command": "sketch.create", "params": {"plane": "XY", "name": "Base"}},
        {"command": "sketch.rectangle.two_point", "params": {"p0": [0, 0], "p1": [80, 50]}},
        {"command": "sketch.constraint.coincident", "params": {"a": "p1", "b": "origin"}},
        {"command": "sketch.dimension", "params": {"entities": ["l1"], "value": "width"}},
        {"command": "sketch.dimension", "params": {"entities": ["l2"], "value": "depth"}},
        {"command": "sketch.finish", "params": {}},
        {"command": "solid.extrude", "params": {"distance": "thickness", "body_name": "Plate"}},
        {"command": "solid.fillet", "params": {"edges": [[0, 0, 4], [80, 0, 4], [80, 50, 4], [0, 50, 4]], "radius": "6 mm"}},
        {"command": "sketch.create", "params": {"plane": "XY", "name": "Holes"}},
        {"command": "sketch.circle.center", "params": {"center": [12, 25], "diameter": 8}},
        {"command": "sketch.circle.center", "params": {"center": [68, 25], "diameter": 8}},
        {"command": "sketch.finish", "params": {}},
        {"command": "solid.extrude", "params": {"distance": "thickness", "operation": "cut", "name": "Holes"}},
        {"command": "sketch.create", "params": {"plane": "XY", "offset": "thickness", "name": "Boss"}},
        {"command": "sketch.circle.center", "params": {"center": [40, 25], "diameter": 24}},
        {"command": "sketch.finish", "params": {}},
        {"command": "solid.extrude", "params": {"distance": 14, "operation": "join", "name": "Boss"}},
        {"command": "sketch.create", "params": {"plane": "XY", "offset": "thickness + 14 mm", "name": "Bore"}},
        {"command": "sketch.circle.center", "params": {"center": [40, 25], "diameter": 12}},
        {"command": "sketch.finish", "params": {}},
        {"command": "solid.extrude", "params": {"distance": "thickness + 14 mm", "direction": "negative", "operation": "cut", "name": "Bore"}},
    ]})
}

/// A built-in sample design: its name, a line about what it shows, and the script that builds it.
pub struct Sample {
    pub name: &'static str,
    pub about: &'static str,
    pub script: fn() -> Value,
}

/// The built-in samples (the start page lists them), each built through commands.
pub fn samples() -> Vec<Sample> {
    vec![
        Sample { name: "Sample Plate", about: "Parametric plate with fillets, holes and a bored boss", script },
        Sample { name: "Sheet Metal Bracket", about: "Base flange with edge flanges and a hem on a 2 mm rule", script: bracket },
        Sample { name: "Hinge Assembly", about: "Two components and a revolute joint, driven to 60°", script: hinge },
        Sample { name: "Plastic Enclosure", about: "Shelled box with a lip, screw bosses, a rest and a snap fit", script: enclosure },
        Sample { name: "Bolted Flange", about: "Hub and flange with a circular pattern driven by a parameter", script: flange },
        Sample { name: "Vent Plate", about: "Slots in a rectangular pattern: rows, columns and pitch are parameters", script: vents },
    ]
}

fn bracket() -> Value {
    json!({"commands": [
        {"command": "sheet.manage_rules", "params": {"name": "Steel 2mm", "thickness": "2 mm", "bend_radius": "Thickness", "active": true}},
        {"command": "sketch.create", "params": {"plane": "XY", "name": "Base"}},
        {"command": "sketch.rectangle.two_point", "params": {"p0": [0, 0], "p1": [120, 60]}},
        {"command": "sketch.finish", "params": {}},
        {"command": "sheet.flange", "params": {"sketch": "Base"}},
        {"command": "sheet.flange", "params": {"edges": [[60, 0, 2], [60, 60, 2]], "height": 30}},
        {"command": "sheet.flange", "params": {"edges": [[120, 30, 2]], "height": 20, "angle": "60 deg"}},
        {"command": "sheet.hem", "params": {"edges": [[0, 30, 2]], "length": 8}},
    ]})
}

fn hinge() -> Value {
    let mut cmds = Vec::new();
    for name in ["Leaf A", "Leaf B"] {
        cmds.extend([
            json!({"command": "component.activate", "params": {"component": "root"}}),
            json!({"command": "component.create", "params": {"name": name}}),
            json!({"command": "solid.box", "params": {"length": 40, "width": 30, "height": 3, "corner": [0, 0, 0]}}),
            json!({"command": "solid.cylinder", "params": {"base": [0, 33, 4], "axis": [1, 0, 0], "radius": 4, "height": 40, "operation": "new"}}),
        ]);
    }
    cmds.extend([
        json!({"command": "component.activate", "params": {"component": "root"}}),
        json!({"command": "occurrence.ground", "params": {"occurrence": "Leaf A:1", "grounded": true}}),
        json!({"command": "joint.create", "params": {"type": "revolute", "name": "Hinge",
            "a": {"occurrence": "Leaf A:1", "circle": [1, 37, 4]}, "b": {"occurrence": "Leaf B:1", "circle": [1, 37, 4]},
            "limits": [["0 deg", "180 deg"]]}}),
        json!({"command": "joint.drive", "params": {"joint": "Hinge", "value": 60}}),
    ]);
    json!({ "commands": cmds })
}

fn enclosure() -> Value {
    json!({"commands": [
        {"command": "solid.box", "params": {"length": 80, "width": 50, "height": 30, "body_name": "Enclosure"}},
        {"command": "solid.shell", "params": {"faces": [[40, 25, 30]], "thickness": 2}},
        {"command": "plastic.lip", "params": {"face": [1, 25, 30], "width": "1 mm", "height": "2 mm"}},
        {"command": "plastic.snap_fit", "params": {"position": [40, 44, 2], "hook": [0, -1, 0], "length": "14 mm", "thickness": "1.5 mm", "width": "6 mm", "catch_depth": "1 mm", "catch_length": "2 mm"}},
        {"command": "plastic.boss", "params": {"position": [10, 10, 2], "diameter": "7 mm", "height": "12 mm", "hole_diameter": "2.5 mm"}},
        {"command": "plastic.boss", "params": {"position": [70, 40, 2], "diameter": "7 mm", "height": "12 mm", "hole_diameter": "2.5 mm"}},
        {"command": "plastic.rest", "params": {"position": [40, 25, 2], "width": "16 mm", "length": "24 mm", "height": "3 mm"}},
    ]})
}

fn flange() -> Value {
    json!({"commands": [
        {"command": "parameters.change", "params": {"name": "bolts", "expression": "6", "unit": "", "comment": "number of bolt holes"}},
        {"command": "parameters.change", "params": {"name": "pcd", "expression": "70 mm", "comment": "bolt circle diameter"}},
        {"command": "sketch.create", "params": {"plane": "XY", "name": "Disc"}},
        {"command": "sketch.circle.center", "params": {"center": [0, 0], "diameter": 100}},
        {"command": "sketch.finish", "params": {}},
        {"command": "solid.extrude", "params": {"distance": 10, "body_name": "Flange", "name": "Disc"}},
        {"command": "sketch.create", "params": {"plane": {"face": [0, 0, 10]}, "name": "Hub"}},
        {"command": "sketch.circle.center", "params": {"center": [0, 0], "diameter": 44}},
        {"command": "sketch.finish", "params": {}},
        {"command": "solid.extrude", "params": {"distance": 16, "operation": "join", "name": "Hub"}},
        {"command": "sketch.create", "params": {"plane": "XY", "name": "Bore"}},
        {"command": "sketch.circle.center", "params": {"center": [0, 0], "diameter": 24}},
        {"command": "sketch.finish", "params": {}},
        {"command": "solid.extrude", "params": {"distance": 26, "operation": "cut", "name": "Bore"}},
        {"command": "sketch.create", "params": {"plane": "XY", "name": "Bolt"}},
        {"command": "sketch.circle.center", "params": {"center": [35, 0], "diameter": 9}},
        {"command": "sketch.finish", "params": {}},
        {"command": "solid.extrude", "params": {"distance": 10, "operation": "cut", "name": "BoltHole"}},
        {"command": "solid.pattern.circular", "params": {"features": ["BoltHole"], "axis": "Z", "count": "bolts"}},
    ]})
}

fn vents() -> Value {
    json!({"commands": [
        {"command": "parameters.change", "params": {"name": "cols", "expression": "6", "unit": ""}},
        {"command": "parameters.change", "params": {"name": "rows", "expression": "3", "unit": ""}},
        {"command": "parameters.change", "params": {"name": "pitch", "expression": "16 mm", "comment": "slot spacing"}},
        {"command": "solid.box", "params": {"length": 120, "width": 80, "height": 4, "body_name": "Plate"}},
        {"command": "solid.fillet", "params": {"edges": [[0, 0, 2], [120, 0, 2], [120, 80, 2], [0, 80, 2]], "radius": "8 mm"}},
        {"command": "sketch.create", "params": {"plane": "XY", "name": "Slot"}},
        {"command": "sketch.rectangle.two_point", "params": {"p0": [18, 12], "p1": [24, 26]}},
        {"command": "sketch.finish", "params": {}},
        {"command": "solid.extrude", "params": {"distance": 4, "operation": "cut", "name": "Vent"}},
        {"command": "solid.pattern.rectangular", "params": {"features": ["Vent"], "dir1": [1, 0, 0], "count1": "cols", "spacing1": "pitch",
            "dir2": [0, 1, 0], "count2": "rows", "spacing2": "22 mm"}},
    ]})
}

#[cfg(test)]
mod tests {
    #[test]
    fn plate_boss_and_bore_planes_follow_thickness_without_weak_face_references() {
        let mut session = crate::Session::default();
        session.run_script(&super::script()).unwrap();
        let boss = session
            .doc
            .features
            .iter()
            .find(|feature| feature.name == "Boss" && matches!(feature.kind, solidcraft_doc::FeatureKind::Sketch { .. }))
            .unwrap()
            .id;
        let bore = session
            .doc
            .features
            .iter()
            .find(|feature| feature.name == "Bore" && matches!(feature.kind, solidcraft_doc::FeatureKind::Sketch { .. }))
            .unwrap()
            .id;
        session.execute("parameters.change", &serde_json::json!({"name":"thickness","expression":"12 mm"})).unwrap();
        assert!(session.model.results.iter().all(|result| result.error.is_none()));
        assert!((session.model.state().sketch(boss).unwrap().plane.origin.z - 12.).abs() < 1e-6);
        assert!((session.model.state().sketch(bore).unwrap().plane.origin.z - 26.).abs() < 1e-6);
        let world = session.world_state();
        let plate = world.body("Plate").unwrap().mesh();
        assert!((plate.bounds().max.z - 26.).abs() < 1e-6);
        assert!(plate.contains(solidcraft_geom::Vec3::new(5., 25., 10.)));
        assert!(plate.contains(solidcraft_geom::Vec3::new(40., 35., 24.)));
        assert!(!plate.contains(solidcraft_geom::Vec3::new(40., 25., 24.)));
        assert!(!plate.contains(solidcraft_geom::Vec3::new(40., 25., 2.)));
        assert!(plate.contains(solidcraft_geom::Vec3::new(40., 35., 2.)));
        let reopened = crate::Session::new(solidcraft_doc::Document::from_json(&session.doc.to_json()).unwrap());
        assert!((reopened.model.state().sketch(bore).unwrap().plane.origin.z - 26.).abs() < 1e-6);
        let reopened_world = reopened.world_state();
        let reopened_plate = reopened_world.body("Plate").unwrap().mesh();
        assert!(!reopened_plate.contains(solidcraft_geom::Vec3::new(40., 25., 2.)));
        assert!(reopened_plate.contains(solidcraft_geom::Vec3::new(40., 35., 2.)));
    }

    /// Every built-in sample builds without a failing feature or joint.
    #[test]
    fn samples_build_cleanly() {
        for s in super::samples() {
            let mut session = crate::Session::default();
            if let Err(e) = session.run_script(&(s.script)()) {
                panic!("{}: {e}", s.name);
            }
            let failed: Vec<String> = session.model.results.iter().filter_map(|r| r.error.as_ref().map(|e| format!("{}: {e}", r.name))).collect();
            assert!(failed.is_empty(), "{}: {failed:?}", s.name);
            assert!(!session.model.state().bodies.is_empty(), "{}", s.name);
        }
    }
}
