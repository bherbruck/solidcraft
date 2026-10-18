//! URDF export (#719, docs/urdf.md): files, names, limits, mass and forward kinematics against
//! the joint solver.

use std::path::PathBuf;

use serde_json::{Value, json};
use solidcraft_doc::{Mat, joints, mat_mul};
use solidcraft_io::urdf::{Step, UrdfModel, UrdfOptions, UrdfPackage};

use crate::Session;

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    match s.execute(id, &p) {
        Ok(v) => v,
        Err(e) => panic!("{id} {p}: {e}"),
    }
}

fn occ(s: &Session, component: &str) -> u64 {
    let c = s.doc.find_component(component).unwrap_or(0);
    s.doc.occurrence_of(c).map(|o| o.id).unwrap_or(0)
}

fn ground(s: &mut Session, component: &str, grounded: bool) {
    let o = occ(s, component);
    run(s, "occurrence.ground", json!({"occurrence": o, "grounded": grounded}));
}

/// A hinge leaf: a 40 x 30 x 3 plate and an r4 knuckle along X at y = 33, z = 4.
fn leaf(s: &mut Session, name: &str) {
    run(s, "component.activate", json!({"component": "root"}));
    run(s, "component.create", json!({"name": name}));
    run(s, "solid.box", json!({"length": 40, "width": 30, "height": 3, "corner": [0, 0, 0]}));
    run(s, "solid.cylinder", json!({"base": [0, 33, 4], "axis": [1, 0, 0], "radius": 4, "height": 40, "operation": "new"}));
}

/// LeafA (grounded) and LeafB on a revolute joint "Hinge" without limits.
fn hinge() -> Session {
    let mut s = Session::default();
    leaf(&mut s, "LeafA");
    leaf(&mut s, "LeafB");
    ground(&mut s, "LeafA", true);
    run(
        &mut s,
        "joint.create",
        json!({"type": "revolute", "a": {"occurrence": "LeafA", "circle": [1, 37, 4]}, "b": {"occurrence": "LeafB", "circle": [1, 37, 4]}, "flip": true, "name": "Hinge"}),
    );
    s
}

/// 10 mm cubes in components A (grounded, at the origin) and B (at x = 20).
fn two_cubes() -> Session {
    let mut s = Session::default();
    for (name, x) in [("A", 0), ("B", 20)] {
        run(&mut s, "component.activate", json!({"component": "root"}));
        run(&mut s, "component.create", json!({"name": name}));
        run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10, "corner": [x, 0, 0]}));
    }
    ground(&mut s, "A", true);
    s
}

/// B's bottom face on A's top face.
fn joint(s: &mut Session, kind: &str, name: &str, limits: Value) {
    run(s, "joint.create", json!({"type": kind, "a": {"face": [5, 5, 10]}, "b": {"face": [25, 5, 0]}, "limits": limits, "name": name}));
}

fn package(s: &Session) -> UrdfPackage {
    match solidcraft_io::urdf(&s.doc, &s.doc.painted(&s.model.state()), "t", &UrdfOptions::default()) {
        Ok(p) => p,
        Err(e) => panic!("urdf: {e}"),
    }
}

fn close(a: &Mat, b: &Mat) -> bool {
    a.iter().flatten().zip(b.iter().flatten()).all(|(x, y)| (x - y).abs() < 1e-9)
}

/// Every placement's link, composed at the design's current joint values, sits where SolidCraft
/// places it.
fn assert_fk(m: &UrdfModel, s: &Session, what: &str) {
    let vals = joints::joint_values(&s.doc.assembly);
    let frames = m.link_frames(|j| j.source.and_then(|(id, k)| vals.get(&id)?.get(k).copied()).unwrap_or(0.0));
    let placements = s.doc.placements();
    let mut seen = 0;
    for (l, f) in m.links.iter().zip(&frames) {
        if l.path.is_empty() {
            continue;
        }
        let Some((_, _, want)) = placements.iter().find(|p| p.1 == l.path) else { panic!("{what}: {} has no placement", l.name) };
        let got = mat_mul(f, &l.mesh_origin);
        assert!(close(&got, want), "{what}: {} at {got:?}, SolidCraft has {want:?}", l.name);
        seen += 1;
    }
    assert_eq!(seen, placements.len() - 1, "{what}: a link per placement");
}

fn temp(tag: &str) -> PathBuf {
    let d = solidcraft_io::vfs::temp_dir().join(format!("solidcraft-urdf-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::create_dir_all(&d);
    d
}

/// Volume (m³) of an STL whose triangles share their corners (`None`: not closed).
fn stl_volume(bytes: &[u8]) -> Option<f64> {
    let mut index: std::collections::HashMap<[u32; 3], u32> = std::collections::HashMap::new();
    let mut mesh = solidcraft_geom::Mesh::default();
    for t in solidcraft_io::read_stl(bytes).ok()? {
        let mut tri = [0u32; 3];
        for (k, p) in t.iter().enumerate() {
            let n = mesh.positions.len() as u32;
            tri[k] = *index.entry(p.map(f32::to_bits)).or_insert_with(|| {
                mesh.positions.push(solidcraft_geom::Vec3::new(p[0] as f64, p[1] as f64, p[2] as f64));
                n
            });
        }
        mesh.triangles.push(tri);
    }
    mesh.mass_properties().map(|m| m.volume)
}

fn names(m: &UrdfModel) -> (Vec<&str>, Vec<&str>) {
    (m.links.iter().map(|l| l.name.as_str()).collect(), m.joints.iter().map(|j| j.name.as_str()).collect())
}

fn has_warning(p: &UrdfPackage, needle: &str) -> bool {
    p.warnings.iter().any(|w| w.contains(needle))
}

#[test]
fn urdf_of_a_hinge() {
    let mut s = hinge();
    run(&mut s, "joint.drive", json!({"joint": "Hinge", "value": 90}));
    let d = temp("hinge");
    let path = d.join("hinge.urdf");
    let r = run(&mut s, "file.export", json!({"path": path.to_string_lossy()}));
    assert_eq!(r["format"], "Urdf", "{r}");
    assert_eq!(r["links"], 3, "{r}");
    let mut files: Vec<String> = std::fs::read_dir(&d).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().to_string()).collect();
    files.sort();
    assert_eq!(files, ["hinge-LeafA.stl", "hinge-LeafB.stl", "hinge.urdf"]);
    let xml = std::fs::read_to_string(&path).unwrap();
    for want in [
        r#"<link name="base_link">"#,
        r#"<link name="LeafA">"#,
        r#"<link name="LeafB">"#,
        r#"<joint name="LeafA_fixed" type="fixed">"#,
        r#"<joint name="Hinge" type="continuous">"#,
        r#"<mesh filename="hinge-LeafB.stl"/>"#,
    ] {
        assert!(xml.contains(want), "{want} missing from\n{xml}");
    }
    // Every mesh is a closed solid in metres.
    for f in ["hinge-LeafA.stl", "hinge-LeafB.stl"] {
        let v = stl_volume(&std::fs::read(d.join(f)).unwrap());
        assert!(v.is_some_and(|v| v > 0.0 && v < 1e-5), "{f}: {v:?}");
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn urdf_forward_kinematics_match_the_assembly() {
    let cases = [
        ("revolute", json!([30]), json!([-50])),
        ("slider", json!([5]), json!([-3])),
        ("cylindrical", json!([30, 4]), json!([-70, -2])),
        ("pin_slot", json!([30, 3]), json!([100, -6])),
        ("planar", json!([2, 3, 40]), json!([-4, 7, -20])),
        ("ball", json!([20, 30, 40]), json!([-60, 10, 75])),
    ];
    for (kind, first, second) in cases {
        // The tree reaches the child on B, then (B grounded) on A.
        for b_grounded in [false, true] {
            let mut s = two_cubes();
            if b_grounded {
                ground(&mut s, "A", false);
                ground(&mut s, "B", true);
            }
            joint(&mut s, kind, "J", Value::Null);
            run(&mut s, "joint.drive", json!({"joint": "J", "values": first}));
            let p = package(&s);
            let what = format!("{kind} (B grounded: {b_grounded})");
            assert_fk(&p.model, &s, &what);
            // The same URDF follows SolidCraft to other values.
            run(&mut s, "joint.drive", json!({"joint": "J", "values": second}));
            assert_fk(&p.model, &s, &what);
            let moving = p.model.joints.iter().filter(|j| j.step != Step::Fixed).count();
            assert_eq!(moving, joints::JointKind::parse(kind).map(|k| k.dofs().len()).unwrap_or(0), "{what}");
        }
    }
}

#[test]
fn urdf_limits_convert_units() {
    let mut s = two_cubes();
    joint(&mut s, "slider", "Rail", json!([[10, 0]]));
    let x = package(&s).xml;
    assert!(x.contains(r#"<joint name="Rail" type="prismatic">"#), "{x}");
    assert!(x.contains(r#"<limit lower="0" upper="0.01" effort="1000" velocity="10"/>"#), "{x}");

    let mut s = two_cubes();
    joint(&mut s, "revolute", "Turn", json!([[-30, 45]]));
    let p = package(&s);
    let Some(j) = p.model.joints.iter().find(|j| j.name == "Turn") else { panic!("{:?}", names(&p.model)) };
    let Some((lo, hi)) = j.limit else { panic!("no limit") };
    assert!((lo + 30f64.to_radians()).abs() < 1e-12 && (hi - 45f64.to_radians()).abs() < 1e-12);
    assert!(p.xml.contains(r#"type="revolute">"#), "{}", p.xml);
    assert!(has_warning(&p, "effort and velocity"), "{:?}", p.warnings);

    // A prismatic step needs a limit: an unlimited slide gets ±1000 m and a warning.
    let mut s = two_cubes();
    joint(&mut s, "slider", "Free", Value::Null);
    let p = package(&s);
    assert!(p.xml.contains(r#"lower="-1000" upper="1000""#), "{}", p.xml);
    assert!(has_warning(&p, "Free"), "{:?}", p.warnings);
}

/// A limit that is not finite (a hand-edited file) counts as none, never as `0`.
#[test]
fn urdf_treats_infinite_limits_as_none() {
    let mut s = two_cubes();
    joint(&mut s, "revolute", "Turn", Value::Null);
    let mut doc = (*s.doc).clone();
    for j in &mut doc.assembly.joints {
        j.limits = vec![Some((f64::NEG_INFINITY, f64::INFINITY))];
    }
    let p = solidcraft_io::urdf(&doc, &s.doc.painted(&s.model.state()), "t", &UrdfOptions::default()).unwrap();
    assert!(p.xml.contains(r#"<joint name="Turn" type="continuous">"#), "{}", p.xml);
}

#[test]
fn urdf_effort_and_velocity_are_written_when_given() {
    let mut s = two_cubes();
    joint(&mut s, "revolute", "Turn", Value::Null);
    let d = temp("effort");
    let path = d.join("e.urdf");
    let r = run(&mut s, "file.export", json!({"path": path.to_string_lossy(), "effort": 12.5, "velocity": 0.5}));
    assert!(r["warnings"].as_array().unwrap().iter().all(|w| !w.as_str().unwrap_or("").contains("effort")), "{r}");
    let xml = std::fs::read_to_string(&path).unwrap();
    assert!(xml.contains(r#"<limit effort="12.5" velocity="0.5"/>"#), "{xml}");
    for bad in [json!(0), json!(-1), json!("x")] {
        assert!(s.execute("file.export", &json!({"path": path.to_string_lossy(), "effort": bad})).is_err(), "{bad}");
    }
    let _ = std::fs::remove_dir_all(&d);
}

/// A moved occurrence whose position is not captured yet exports where it is shown.
#[test]
fn urdf_exports_uncaptured_moves() {
    let mut s = two_cubes();
    let b = occ(&s, "B");
    run(&mut s, "occurrence.move", json!({"occurrence": b, "translate": [0, 0, 30], "capture": false}));
    assert!(!s.pending_moves.is_empty());
    let d = temp("pending");
    let path = d.join("m.urdf");
    run(&mut s, "file.export", json!({"path": path.to_string_lossy()}));
    let xml = std::fs::read_to_string(&path).unwrap();
    // B hangs on base_link where it is shown: 30 mm up.
    assert!(xml.contains(r#"<origin xyz="0 0 0.03" rpy="0 0 0"/>"#), "{xml}");
    let _ = std::fs::remove_dir_all(&d);
}

/// A jointed occurrence moved without capturing exports where its joint puts it: its joint
/// would put it back as soon as it moves, so the move must not become part of its link.
#[test]
fn urdf_ignores_uncaptured_moves_of_jointed_occurrences() {
    let mut s = two_cubes();
    joint(&mut s, "revolute", "Turn", Value::Null);
    let d = temp("pending-jointed");
    let path = d.join("j.urdf");
    run(&mut s, "file.export", json!({"path": path.to_string_lossy()}));
    let solved = std::fs::read_to_string(&path).unwrap();
    let b = occ(&s, "B");
    run(&mut s, "occurrence.move", json!({"occurrence": b, "translate": [0, 0, 30], "capture": false}));
    let r = run(&mut s, "file.export", json!({"path": path.to_string_lossy()}));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), solved);
    assert!(r["warnings"].as_array().unwrap().iter().any(|w| w.as_str().unwrap_or("").contains("not captured")), "{r}");
    let _ = std::fs::remove_dir_all(&d);
}

/// A joint whose origins no longer resolve does not place its occurrence, so that occurrence's
/// uncaptured move counts.
#[test]
fn urdf_keeps_uncaptured_moves_when_the_joint_is_broken() {
    let mut s = two_cubes();
    joint(&mut s, "revolute", "Turn", Value::Null);
    let Some(f) = s.doc.features.iter().rev().find(|f| matches!(f.kind, solidcraft_doc::FeatureKind::Box { .. })).map(|f| f.id) else {
        panic!("box")
    };
    run(&mut s, "timeline.suppress", json!({"feature": f, "suppressed": true}));
    let d = temp("pending-broken");
    let path = d.join("k.urdf");
    run(&mut s, "file.export", json!({"path": path.to_string_lossy()}));
    let before = std::fs::read_to_string(&path).unwrap();
    let b = occ(&s, "B");
    run(&mut s, "occurrence.move", json!({"occurrence": b, "translate": [0, 0, 30], "capture": false}));
    let r = run(&mut s, "file.export", json!({"path": path.to_string_lossy()}));
    assert_ne!(std::fs::read_to_string(&path).unwrap(), before, "B exports where it is shown");
    assert!(r["warnings"].as_array().unwrap().iter().all(|w| !w.as_str().unwrap_or("").contains("not captured")), "{r}");
    let _ = std::fs::remove_dir_all(&d);
}

/// A group the solver leaves where it is (grounded A elsewhere, C and D only joined to each
/// other) keeps an uncaptured move of C: no joint puts C back.
#[test]
fn urdf_keeps_uncaptured_moves_the_solver_does_not_undo() {
    let mut s = two_cubes();
    for (name, x) in [("C", 40), ("D", 60)] {
        run(&mut s, "component.activate", json!({"component": "root"}));
        run(&mut s, "component.create", json!({"name": name}));
        run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10, "corner": [x, 0, 0]}));
    }
    // A and B are joined, so the solver walks from grounded A and never anchors C.
    joint(&mut s, "revolute", "Turn", Value::Null);
    run(&mut s, "joint.create", json!({"type": "revolute", "a": {"face": [45, 5, 10]}, "b": {"face": [65, 5, 0]}, "name": "Pair"}));
    let d = temp("pending-floating");
    let path = d.join("f.urdf");
    run(&mut s, "file.export", json!({"path": path.to_string_lossy()}));
    let before = std::fs::read_to_string(&path).unwrap();
    let c = occ(&s, "C");
    run(&mut s, "occurrence.move", json!({"occurrence": c, "translate": [0, 0, 30], "capture": false}));
    let r = run(&mut s, "file.export", json!({"path": path.to_string_lossy()}));
    assert_ne!(std::fs::read_to_string(&path).unwrap(), before, "C exports where it is shown");
    assert!(r["warnings"].as_array().unwrap().iter().all(|w| !w.as_str().unwrap_or("").contains("not captured")), "{r}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn urdf_mesh_prefix_is_written_and_checked() {
    let mut s = hinge();
    let d = temp("prefix");
    let path = d.join("p.urdf");
    run(&mut s, "file.export", json!({"path": path.to_string_lossy(), "mesh_prefix": "package://robot/meshes/"}));
    let xml = std::fs::read_to_string(&path).unwrap();
    assert!(xml.contains(r#"<mesh filename="package://robot/meshes/p-LeafA.stl"/>"#), "{xml}");
    // XML cannot hold a control character, even escaped.
    let e = s.execute("file.export", &json!({"path": d.join("q.urdf").to_string_lossy(), "mesh_prefix": "a\u{1}b"}));
    assert!(e.is_err_and(|e| e.to_string().contains("mesh_prefix")));
    assert!(!d.join("q.urdf").exists());
    let _ = std::fs::remove_dir_all(&d);
}

/// The visual's colour keeps the body's opacity, and a too long prefix says so.
#[test]
fn urdf_colour_keeps_opacity() {
    let mut s = Session::default();
    run(&mut s, "component.create", json!({"name": "Glass"}));
    run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10}));
    let body = s.model.state().bodies.first().map(|b| b.name.clone()).unwrap_or_default();
    run(&mut s, "appearance.assign", json!({"bodies": [body], "color": [255, 0, 0], "opacity": 0.5}));
    let p = package(&s);
    assert!(p.xml.contains(r#"<color rgba="1 0 0 0.5"/>"#), "{}", p.xml);
    let d = temp("long-prefix");
    let e = s.execute("file.export", &json!({"path": d.join("l.urdf").to_string_lossy(), "mesh_prefix": "a".repeat(5000)}));
    assert!(e.is_err_and(|e| e.to_string().contains("longer than")));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn urdf_inertial_is_mass_weighted() {
    let mut s = Session::default();
    run(&mut s, "component.create", json!({"name": "Cube"}));
    run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10, "corner": [100, 0, 0]}));
    let body = s.model.state().bodies.first().map(|b| b.name.clone()).unwrap_or_default();
    run(&mut s, "material.assign", json!({"bodies": [body], "material": "Aluminum"}));
    let p = package(&s);
    let Some(l) = p.model.links.iter().find(|l| l.name == "Cube") else { panic!("{:?}", names(&p.model)) };
    let Some(i) = l.inertial else { panic!("no inertial") };
    let m = 0.0027;
    assert!((i.mass - m).abs() < 1e-12, "{}", i.mass);
    let ixx = m * (0.01f64.powi(2) * 2.0) / 12.0;
    for k in 0..3 {
        assert!((i.inertia[k][k] - ixx).abs() < 1e-12 * ixx.max(1.0) + 1e-15, "{:?}", i.inertia);
    }
    assert!(i.inertia[0][1].abs() < 1e-15);
    // The centre of mass is the cube's centre, in the link frame.
    assert!(i.com.dist(solidcraft_doc::apply_point(&l.mesh_origin, solidcraft_geom::Vec3::new(105.0, 5.0, 5.0))) < 1e-9, "{:?}", i.com);
}

#[test]
fn urdf_names_are_unique_across_links_joints_and_files() {
    let mut s = Session::default();
    for name in ["Arm", "base", "Arm_link1"] {
        run(&mut s, "component.activate", json!({"component": "root"}));
        run(&mut s, "component.create", json!({"name": name}));
        run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10, "corner": [0, 0, 0]}));
    }
    run(&mut s, "solid.box", json!({"length": 5, "width": 5, "height": 5, "corner": [50, 50, 0]}));
    ground(&mut s, "base", true);
    run(
        &mut s,
        "joint.create",
        json!({"type": "cylindrical", "a": {"occurrence": "base", "face": [5, 5, 10]}, "b": {"occurrence": "Arm", "face": [5, 5, 0]}, "name": "Arm"}),
    );
    let p = package(&s);
    let (links, joints) = names(&p.model);
    let mut all: Vec<&str> = links.iter().chain(&joints).copied().collect();
    all.extend(p.meshes.iter().map(|(f, _)| f.as_str()));
    let n = all.len();
    all.sort();
    all.dedup();
    assert_eq!(all.len(), n, "{links:?} {joints:?}");
    assert!(joints.contains(&"Arm"), "{joints:?}");
}

#[test]
fn urdf_collision_skips_open_bodies() {
    let mut s = Session::default();
    run(&mut s, "component.create", json!({"name": "Part"}));
    run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10, "corner": [0, 0, 0]}));
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "S"}));
    run(&mut s, "sketch.line", json!({"points": [[30, 0], [40, 0]], "ids": ["l1"]}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "surface.extrude", json!({"sketch": "S", "curves": ["l1"], "distance": 10, "body_name": "Wall"}));
    let p = package(&s);
    let Some(l) = p.model.links.iter().find(|l| l.name == "Part") else { panic!("{:?}", names(&p.model)) };
    let (Some(v), Some(c)) = (&l.visual, &l.collision) else { panic!("{l:?}") };
    assert_ne!(v, c, "a separate collision mesh");
    let bytes = &p.meshes.iter().find(|(f, _)| f == c).unwrap().1;
    assert!(stl_volume(bytes).is_some(), "the collision mesh is closed");
    assert!(has_warning(&p, "not a closed solid"), "{:?}", p.warnings);
    // Mass from the cube alone (default density).
    let rho = s.doc.density(&s.model.state().bodies.first().map(|b| b.name.clone()).unwrap_or_default());
    assert!(l.inertial.is_some_and(|i| (i.mass - 1000.0 * rho * 1e-6).abs() < 1e-12));
}

#[test]
fn urdf_open_only_component_is_drawn() {
    let mut s = Session::default();
    run(&mut s, "component.create", json!({"name": "Sheet"}));
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "S"}));
    run(&mut s, "sketch.line", json!({"points": [[30, 0], [40, 0]], "ids": ["l1"]}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "surface.extrude", json!({"sketch": "S", "curves": ["l1"], "distance": 10, "body_name": "Wall"}));
    let p = package(&s);
    let Some(l) = p.model.links.iter().find(|l| l.name == "Sheet") else { panic!("{:?}", names(&p.model)) };
    assert!(l.visual.is_some() && l.collision.is_none(), "{l:?}");
    assert!(l.inertial.is_some_and(|i| i.mass == 1e-6), "{l:?}");
}

#[test]
fn urdf_skips_a_joint_that_closes_a_loop() {
    let mut s = two_cubes();
    run(&mut s, "component.activate", json!({"component": "root"}));
    run(&mut s, "component.create", json!({"name": "C"}));
    run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10, "corner": [40, 0, 0]}));
    run(&mut s, "joint.rigid_group", json!({"occurrences": ["A", "B", "C"]}));
    run(&mut s, "joint.as_built", json!({"type": "rigid", "a": "B", "b": "C"}));
    let p = package(&s);
    assert!(has_warning(&p, "closes a loop"), "{:?}", p.warnings);
    assert_tree(&p.model);
    assert_fk(&p.model, &s, "loop");
}

/// Two grounded occurrences each reach C by a joint: the solver walks them in id order, so the
/// URDF keeps the joint from the one with the lower id, whatever order the file lists them in.
#[test]
fn urdf_walks_grounded_occurrences_in_id_order() {
    let mut s = two_cubes();
    run(&mut s, "component.activate", json!({"component": "root"}));
    run(&mut s, "component.create", json!({"name": "C"}));
    run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10, "corner": [40, 0, 0]}));
    ground(&mut s, "B", true);
    run(&mut s, "joint.create", json!({"type": "rigid", "a": {"face": [25, 5, 10]}, "b": {"face": [45, 5, 0]}, "name": "FromB"}));
    run(&mut s, "joint.create", json!({"type": "rigid", "a": {"face": [5, 5, 10]}, "b": {"occurrence": "C", "face": [25, 5, 20]}, "name": "FromA"}));
    let mut doc = (*s.doc).clone();
    doc.occurrences.reverse();
    let p = solidcraft_io::urdf(&doc, &s.doc.painted(&s.model.state()), "t", &UrdfOptions::default()).unwrap();
    assert!(p.model.joints.iter().any(|j| j.name == "FromA"), "{:?}", names(&p.model));
    assert!(has_warning(&p, "joint `FromB` closes a loop"), "{:?}", p.warnings);
}

/// Every link but `base_link` has exactly one parent joint.
fn assert_tree(m: &UrdfModel) {
    for (i, l) in m.links.iter().enumerate().skip(1) {
        assert_eq!(m.joints.iter().filter(|j| j.child == i).count(), 1, "{}", l.name);
    }
    assert_eq!(m.joints.iter().filter(|j| j.child == 0).count(), 0);
}

#[test]
fn urdf_fixes_an_unjointed_occurrence_to_base() {
    let s = two_cubes();
    let p = package(&s);
    assert!(has_warning(&p, "`B` has no joint"), "{:?}", p.warnings);
    assert_tree(&p.model);
    assert_fk(&p.model, &s, "loose");
}

#[test]
fn urdf_of_an_ungrounded_assembly_keeps_its_joints() {
    let mut s = two_cubes();
    ground(&mut s, "A", false);
    joint(&mut s, "revolute", "Turn", Value::Null);
    run(&mut s, "joint.drive", json!({"joint": "Turn", "value": 20}));
    let p = package(&s);
    assert!(has_warning(&p, "`A` is not grounded"), "{:?}", p.warnings);
    assert!(p.model.joints.iter().any(|j| j.name == "Turn" && j.step == Step::Revolute));
    assert_fk(&p.model, &s, "ungrounded");
    run(&mut s, "joint.drive", json!({"joint": "Turn", "value": -70}));
    assert_fk(&p.model, &s, "ungrounded, driven");
}

#[test]
fn urdf_skips_a_joint_with_an_error() {
    let mut s = two_cubes();
    joint(&mut s, "revolute", "Turn", Value::Null);
    // Snap the joint to a face that then goes away.
    let Some(f) = s.doc.features.iter().rev().find(|f| matches!(f.kind, solidcraft_doc::FeatureKind::Box { .. })).map(|f| f.id) else {
        panic!("box")
    };
    run(&mut s, "timeline.suppress", json!({"feature": f, "suppressed": true}));
    let p = package(&s);
    assert!(has_warning(&p, "joint `Turn` is skipped"), "{:?}", p.warnings);
    assert_tree(&p.model);
}

#[test]
fn urdf_nested_occurrence_moves_with_its_parent() {
    let mut s = hinge();
    let b = s.doc.find_component("LeafB").unwrap_or(0);
    run(&mut s, "component.activate", json!({"component": "LeafB"}));
    run(&mut s, "component.create", json!({"name": "Pin", "parent": b}));
    run(&mut s, "solid.box", json!({"length": 4, "width": 4, "height": 4, "corner": [10, 10, 3]}));
    run(&mut s, "joint.drive", json!({"joint": "Hinge", "value": 30}));
    let p = package(&s);
    assert!(!has_warning(&p, "Pin"), "{:?}", p.warnings);
    let pin = p.model.links.iter().position(|l| l.name == "Pin").unwrap_or(0);
    let leaf_b = p.model.links.iter().position(|l| l.name == "LeafB").unwrap_or(0);
    assert!(p.model.joints.iter().any(|j| j.child == pin && j.parent == leaf_b), "Pin hangs on LeafB");
    assert_fk(&p.model, &s, "nested");
    run(&mut s, "joint.drive", json!({"joint": "Hinge", "value": 120}));
    assert_fk(&p.model, &s, "nested, driven");
}

#[test]
fn urdf_copies_nested_occurrences_of_each_placement() {
    let mut s = Session::default();
    run(&mut s, "component.create", json!({"name": "Leg"}));
    run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 30, "corner": [0, 0, 0]}));
    let leg = s.doc.find_component("Leg").unwrap_or(0);
    run(&mut s, "component.create", json!({"name": "Foot", "parent": leg}));
    run(&mut s, "solid.box", json!({"length": 12, "width": 12, "height": 2, "corner": [-1, -1, -2]}));
    run(&mut s, "component.activate", json!({"component": "root"}));
    run(&mut s, "occurrence.copy", json!({"component": "Leg", "translate": [50, 0, 0]}));
    assert_eq!(s.doc.placements().len(), 5, "root, two legs, two feet");
    let p = package(&s);
    assert_fk(&p.model, &s, "copies");
    assert_tree(&p.model);
}

/// Deeper than `placements()` walks (a file can hold it; commands stop at 64 levels): the
/// export says copies past the limit are left out.
#[test]
fn urdf_warns_when_placements_are_cut_off() {
    let s = Session::default();
    let mut doc = (*s.doc).clone();
    for k in 1..=66u64 {
        doc.components.push(solidcraft_doc::Component {
            id: 1000 + k,
            name: format!("C{k}"),
            parent: if k == 1 { 0 } else { 999 + k },
            ..Default::default()
        });
        doc.occurrences.push(solidcraft_doc::Occurrence {
            id: 2000 + k,
            component: 1000 + k,
            parent: if k == 1 { 0 } else { 999 + k },
            name: format!("C{k}:1"),
            transform: solidcraft_doc::IDENTITY,
            grounded: false,
        });
    }
    // A component placed twice with a part nested in it, next to the deep chain: its copy is
    // still there, since only the branch past the limit stops.
    doc.components.push(solidcraft_doc::Component { id: 3001, name: "Leg".into(), parent: 0, ..Default::default() });
    doc.components.push(solidcraft_doc::Component { id: 3002, name: "Foot".into(), parent: 3001, ..Default::default() });
    // Listed first, so the walk (a stack) goes down the chain before reaching them.
    for (id, component, parent, name) in [(4003, 3002, 3001, "Foot:1"), (4002, 3001, 0, "Leg:2"), (4001, 3001, 0, "Leg:1")] {
        let o = solidcraft_doc::Occurrence { id, component, parent, name: name.into(), transform: solidcraft_doc::IDENTITY, grounded: false };
        doc.occurrences.insert(0, o);
    }
    let p = solidcraft_io::urdf(&doc, &s.model.state(), "t", &UrdfOptions::default()).unwrap();
    assert!(has_warning(&p, "left out"), "{:?}", p.warnings);
    assert!(p.model.links.iter().any(|l| l.path == [4002, 4003]), "the copy of Foot under Leg:2");
    assert_tree(&p.model);
}

#[test]
fn urdf_robot_name_is_xml_safe() {
    let s = hinge();
    for stem in ["a--b", "x\u{1}y", "<&>"] {
        let p = match solidcraft_io::urdf(&s.doc, &s.doc.painted(&s.model.state()), stem, &UrdfOptions::default()) {
            Ok(p) => p,
            Err(e) => panic!("{e}"),
        };
        assert!(well_formed(&p.xml).is_ok(), "{stem}: {:?}\n{}", well_formed(&p.xml), p.xml);
        assert!(p.xml.chars().all(|c| c == '\n' || !c.is_control()), "{stem}");
    }
}

/// The XML is well formed: every element closes, in order.
fn well_formed(xml: &str) -> Result<(), String> {
    let mut stack: Vec<String> = Vec::new();
    let mut rest = xml;
    while let Some(i) = rest.find('<') {
        rest = &rest[i + 1..];
        let Some(end) = rest.find('>') else { return Err("unclosed tag".into()) };
        let tag = &rest[..end];
        rest = &rest[end + 1..];
        if tag.starts_with('?') || tag.starts_with('!') || tag.ends_with('/') {
            continue;
        }
        if let Some(name) = tag.strip_prefix('/') {
            if stack.pop().as_deref() != Some(name) {
                return Err(format!("</{name}> out of order"));
            }
        } else {
            stack.push(tag.split_whitespace().next().unwrap_or("").to_string());
        }
    }
    if stack.is_empty() { Ok(()) } else { Err(format!("unclosed {stack:?}")) }
}

#[test]
fn urdf_needs_the_whole_assembly() {
    let mut s = hinge();
    let d = temp("bodies");
    let e = s.execute("file.export", &json!({"path": d.join("x.urdf").to_string_lossy(), "bodies": ["Body1"]}));
    assert!(e.is_err_and(|e| e.to_string().contains("whole assembly")));
    assert_eq!(std::fs::read_dir(&d).map(|r| r.count()).unwrap_or(0), 0);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn urdf_files_stay_inside_allow_paths() {
    let mut s = hinge();
    let inside = temp("scope-in");
    let outside = temp("scope-out");
    let refused = solidcraft_io::vfs::scoped(Some(solidcraft_io::vfs::PathScope::new([inside.clone()])), || {
        s.execute("file.export", &json!({"path": outside.join("r.urdf").to_string_lossy()}))
    });
    assert!(refused.is_err());
    assert_eq!(std::fs::read_dir(&outside).map(|r| r.count()).unwrap_or(0), 0, "nothing written");
    let ok = solidcraft_io::vfs::scoped(Some(solidcraft_io::vfs::PathScope::new([inside.clone()])), || {
        s.execute("file.export", &json!({"path": inside.join("r.urdf").to_string_lossy()}))
    });
    assert!(ok.is_ok(), "{ok:?}");
    let _ = std::fs::remove_dir_all(&inside);
    let _ = std::fs::remove_dir_all(&outside);
}
