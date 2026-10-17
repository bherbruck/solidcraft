use std::f64::consts::{FRAC_PI_2, PI};

use serde_json::{Value, json};

use crate::Session;

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    match s.execute(id, &p) {
        Ok(v) => v,
        Err(e) => panic!("{id} {p}: {e}"),
    }
}

fn volume(s: &mut Session) -> f64 {
    run(s, "inspect.measure", json!({}))["total"]["volume_mm3"].as_f64().unwrap_or(f64::NAN)
}

fn rel(a: f64, b: f64) -> f64 {
    (a - b).abs() / b.abs().max(1e-12)
}

fn flat(s: &mut Session) -> Value {
    run(s, "sheet.flat_pattern", json!({}))
}

/// A w x h base flange on XY.
fn base(s: &mut Session, w: f64, h: f64) {
    run(s, "sketch.create", json!({"plane": "XY", "name": "Base"}));
    run(s, "sketch.rectangle.two_point", json!({"p0": [0, 0], "p1": [w, h]}));
    run(s, "sketch.finish", json!({}));
    run(s, "sheet.flange", json!({"sketch": "Base"}));
}

fn rectangle_tool(s: &mut Session, name: &str, p0: [f64; 2], p1: [f64; 2]) {
    run(s, "sketch.create", json!({"plane":"XY","name":name}));
    run(s, "sketch.rectangle.two_point", json!({"p0":p0,"p1":p1}));
    run(s, "sketch.finish", json!({}));
}

#[test]
fn shallow_sheet_pocket_preserves_material_depth_export_undo_and_reopen() {
    let mut s = Session::default();
    base(&mut s, 100.0, 60.0);
    rectangle_tool(&mut s, "Pocket", [20.0, 20.0], [30.0, 30.0]);
    run(&mut s, "solid.extrude", json!({"sketch":"Pocket","distance":1,"operation":"cut"}));
    assert!((volume(&mut s) - 14900.0).abs() < 1e-6);
    let f = flat(&mut s);
    assert_eq!(f["cutouts"], json!([]));
    assert_eq!(f["pockets"][0]["side"], "bottom");
    assert_eq!(f["pockets"][0]["depth_mm"], 1.0);
    let dxf = run(&mut s, "sheet.export_dxf", json!({}))["dxf"].as_str().unwrap().to_string();
    assert!(dxf.contains("POCKET_BOTTOM_Z0.000000_TO_1.000000_MM"));
    assert_eq!(solidcraft_io::read_dxf(&dxf).unwrap().len(), 8);
    run(&mut s, "edit.undo", json!({}));
    assert!((volume(&mut s) - 15000.0).abs() < 1e-6);
    run(&mut s, "edit.redo", json!({}));
    for command in ["sheet.unfold", "sheet.refold"] {
        run(&mut s, command, json!({}));
        assert!((volume(&mut s) - 14900.0).abs() < 1e-6);
        let state = s.model.state();
        let body = &state.bodies[0].body;
        assert!(!body.contains(solidcraft_geom::Vec3::new(25.0, 25.0, 0.5)));
        assert!(body.contains(solidcraft_geom::Vec3::new(25.0, 25.0, 1.5)));
    }
    let mut reopened = Session::new(solidcraft_doc::Document::from_json(&s.doc.to_json()).unwrap());
    assert!((volume(&mut reopened) - 14900.0).abs() < 1e-6);
    assert_eq!(flat(&mut reopened)["pockets"], f["pockets"]);
}

#[test]
fn planar_sheet_intersection_retains_exact_outline_and_islands() {
    let mut s = Session::default();
    base(&mut s, 100.0, 60.0);
    rectangle_tool(&mut s, "Retain", [20.0, 20.0], [30.0, 30.0]);
    run(&mut s, "solid.extrude", json!({"sketch":"Retain","through_all":true,"direction":"symmetric","operation":"intersect"}));
    for command in [None, Some("sheet.unfold"), Some("sheet.refold")] {
        if let Some(command) = command {
            run(&mut s, command, json!({}));
        }
        assert!((volume(&mut s) - 250.0).abs() < 1e-6);
        assert_eq!(flat(&mut s)["flat_size_mm"], json!([10.0, 10.0, 2.5]));
    }
    let dxf = run(&mut s, "sheet.export_dxf", json!({}))["dxf"].as_str().unwrap().to_string();
    assert_eq!(solidcraft_io::read_dxf(&dxf).unwrap().len(), 4);
    let mut imported = Session::default();
    run(&mut imported, "sketch.insert_dxf", json!({"name":"Retained","text":dxf}));
    run(&mut imported, "sketch.finish", json!({}));
    run(&mut imported, "solid.extrude", json!({"sketch":"Retained","distance":2.5}));
    assert!((volume(&mut imported) - 250.0).abs() < 1e-6);
    // An annular retained region keeps its central hole, with no first-solid shortcut.
    let mut annulus = Session::default();
    base(&mut annulus, 100.0, 60.0);
    run(&mut annulus, "sketch.create", json!({"plane":"XY","name":"Ring"}));
    run(&mut annulus, "sketch.circle.center", json!({"center":[50,30],"radius":10}));
    run(&mut annulus, "sketch.circle.center", json!({"center":[50,30],"radius":5}));
    run(&mut annulus, "sketch.finish", json!({}));
    run(
        &mut annulus,
        "solid.extrude",
        json!({"sketch":"Ring","profiles":[{"point":[57,30]}],"through_all":true,"direction":"symmetric","operation":"intersect"}),
    );
    run(&mut annulus, "sheet.unfold", json!({}));
    let area: f64 = annulus.model.state().sheets[0].flat_regions().unwrap().iter().map(|r| r.area()).sum();
    assert!((area - PI * 75.0).abs() < 1e-8, "exact retained area: {area}");
    // inspect.measure integrates a tessellation; exact arc area is checked above.
    let measured = volume(&mut annulus);
    assert!((measured - PI * 75.0 * 2.5).abs() < 0.1, "measured={measured}");
    assert_eq!(flat(&mut annulus)["cutouts"].as_array().unwrap().len(), 1);
}

#[test]
fn overlapping_opposite_side_pockets_become_true_through_material() {
    let mut s = Session::default();
    base(&mut s, 100.0, 60.0);
    rectangle_tool(&mut s, "Bottom", [20.0, 20.0], [30.0, 30.0]);
    run(&mut s, "solid.extrude", json!({"sketch":"Bottom","distance":1.5,"operation":"cut"}));
    rectangle_tool(&mut s, "Top", [25.0, 20.0], [35.0, 30.0]);
    run(&mut s, "solid.extrude", json!({"sketch":"Top","start_offset":2.5,"direction":"negative","distance":1.5,"operation":"cut"}));
    let expected = 15000.0 - 100.0 * 1.5 * 2.0 + 50.0 * 0.5;
    for command in [None, Some("sheet.unfold"), Some("sheet.refold")] {
        if let Some(command) = command {
            run(&mut s, command, json!({}));
        }
        assert!((volume(&mut s) - expected).abs() < 1e-6);
        assert_eq!(flat(&mut s)["cutouts"].as_array().unwrap().len(), 1);
        assert_eq!(flat(&mut s)["pockets"].as_array().unwrap().len(), 2);
    }
}

#[test]
fn pockets_survive_forming_away_from_bends_and_refuse_bend_overlap_atomically() {
    let mut s = Session::default();
    base(&mut s, 100.0, 60.0);
    rectangle_tool(&mut s, "Pocket", [20.0, 20.0], [30.0, 30.0]);
    run(&mut s, "solid.extrude", json!({"sketch":"Pocket","distance":1,"operation":"cut"}));
    run(&mut s, "sheet.flange", json!({"edges":[[50,0,2.5]],"height":20}));
    assert!((volume(&mut s) - (18972.62196081845 - 100.0)).abs() < 0.2);
    run(&mut s, "sheet.unfold", json!({}));
    let unfolded = volume(&mut s);
    run(&mut s, "sheet.refold", json!({}));
    assert!((volume(&mut s) - (18972.62196081845 - 100.0)).abs() < 0.2);
    assert!(unfolded > 14900.0);
    let mut cross = Session::default();
    base(&mut cross, 100.0, 60.0);
    rectangle_tool(&mut cross, "Pocket", [45.0, 20.0], [55.0, 30.0]);
    run(&mut cross, "solid.extrude", json!({"sketch":"Pocket","distance":1,"operation":"cut"}));
    let before = cross.doc.to_json();
    let undo = cross.undo.len();
    let error = cross.execute("sheet.fold", &json!({"points":[[50,0,2.5],[50,60,2.5]]})).unwrap_err().to_string();
    assert!(error.contains("pocket") && error.contains("bend"), "{error}");
    assert_eq!(cross.doc.to_json(), before);
    assert_eq!(cross.undo.len(), undo);
    assert!((volume(&mut cross) - 14900.0).abs() < 1e-6);
}

#[test]
fn unsupported_sheet_machining_is_atomic_but_true_no_change_is_allowed() {
    let mut s = Session::default();
    base(&mut s, 100.0, 60.0);
    rectangle_tool(&mut s, "Taper", [20.0, 20.0], [30.0, 30.0]);
    let before = s.doc.to_json();
    let undo = s.undo.len();
    let error = s.execute("solid.extrude", &json!({"sketch":"Taper","distance":1,"operation":"cut","taper":"5 deg"})).unwrap_err().to_string();
    assert!(error.contains("sheet") && error.contains("preserv"), "{error}");
    assert_eq!(s.doc.to_json(), before);
    assert_eq!(s.undo.len(), undo);
    rectangle_tool(&mut s, "Outside", [120.0, 20.0], [130.0, 30.0]);
    run(&mut s, "solid.extrude", json!({"sketch":"Outside","distance":1,"operation":"cut","taper":"5 deg"}));
    assert!((volume(&mut s) - 15000.0).abs() < 1e-6);
    rectangle_tool(&mut s, "All", [-20.0, -20.0], [120.0, 80.0]);
    run(&mut s, "solid.extrude", json!({"sketch":"All","through_all":true,"direction":"symmetric","operation":"intersect"}));
    run(&mut s, "sheet.unfold", json!({}));
    assert!((volume(&mut s) - 15000.0).abs() < 1e-6);
}

#[test]
fn empty_sheet_intersection_removes_geometry_and_metadata_together() {
    let mut s = Session::default();
    base(&mut s, 100.0, 60.0);
    rectangle_tool(&mut s, "Outside", [120.0, 20.0], [130.0, 30.0]);
    run(&mut s, "solid.extrude", json!({"sketch":"Outside","through_all":true,"direction":"symmetric","operation":"intersect"}));
    assert!(s.model.state().bodies.is_empty());
    assert!(s.model.state().sheets.is_empty());
    assert!(s.execute("sheet.unfold", &json!({})).is_err());
    run(&mut s, "edit.undo", json!({}));
    assert!((volume(&mut s) - 15000.0).abs() < 1e-6);
}

#[test]
fn intersect_preserves_both_material_regions_of_a_logical_ring_sheet() {
    let mut s = Session::default();
    base(&mut s, 100.0, 60.0);
    run(&mut s, "sketch.create", json!({"plane":"XY","name":"Ring"}));
    for radius in [10.0, 5.0] {
        run(&mut s, "sketch.circle.center", json!({"center":[50,30],"radius":radius}));
    }
    run(&mut s, "sketch.finish", json!({}));
    run(
        &mut s,
        "solid.extrude",
        json!({"sketch":"Ring","profiles":[{"point":[57,30]}],"through_all":true,"direction":"symmetric","operation":"cut"}),
    );
    rectangle_tool(&mut s, "Crop", [35.0, 15.0], [65.0, 45.0]);
    run(&mut s, "solid.extrude", json!({"sketch":"Crop","through_all":true,"direction":"symmetric","operation":"intersect"}));
    for command in [None, Some("sheet.unfold"), Some("sheet.refold")] {
        if let Some(command) = command {
            run(&mut s, command, json!({}));
        }
        let state = s.model.state();
        let regions = state.sheets[0].flat_regions().unwrap();
        assert_eq!(regions.len(), 2);
        let area: f64 = regions.iter().map(|r| r.area()).sum();
        assert!((area - (900.0 - PI * 75.0)).abs() < 1e-8, "area={area}");
        let body = &state.bodies[0].body;
        assert!(body.contains(solidcraft_geom::Vec3::new(50.0, 30.0, 1.0)));
        assert!(!body.contains(solidcraft_geom::Vec3::new(57.0, 30.0, 1.0)));
        assert!(body.contains(solidcraft_geom::Vec3::new(64.0, 30.0, 1.0)));
        assert!(!body.contains(solidcraft_geom::Vec3::new(70.0, 30.0, 1.0)));
        assert!((volume(&mut s) - (900.0 - PI * 75.0) * 2.5).abs() < 0.2);
    }
    let dxf = run(&mut s, "sheet.export_dxf", json!({}))["dxf"].as_str().unwrap().to_string();
    assert_eq!(solidcraft_io::read_dxf(&dxf).unwrap().len(), 6);
}

#[test]
fn machining_a_folded_parallel_panel_preserves_its_pocket_on_unfold() {
    use solidcraft_doc::{apply_point, apply_vector};
    use solidcraft_geom::Vec3;
    let mut s = Session::default();
    base(&mut s, 100.0, 60.0);
    run(&mut s, "sheet.flange", json!({"edges":[[50,0,2.5]],"height":20}));
    let sh = s.model.state().sheets[0].clone();
    let points = sh.panel(&sh.flanges[0]).outer.polyline(1e-3);
    let c = points.iter().copied().fold(solidcraft_geom::Vec2::ZERO, |sum, p| sum + p) / (points.len() as f64);
    let w = sh.world(Some(0));
    let origin = apply_point(&w, Vec3::ZERO);
    let x = apply_vector(&w, Vec3::X);
    let y = apply_vector(&w, Vec3::Y);
    run(
        &mut s,
        "sketch.create",
        json!({"name":"PanelPocket","plane":{"origin":[origin.x,origin.y,origin.z],"x_dir":[x.x,x.y,x.z],"y_dir":[y.x,y.y,y.z]}}),
    );
    run(&mut s, "sketch.rectangle.two_point", json!({"p0":[c.x-2.0,c.y-2.0],"p1":[c.x+2.0,c.y+2.0]}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.extrude", json!({"sketch":"PanelPocket","distance":1,"operation":"cut"}));
    assert!((volume(&mut s) - (18972.62196081845 - 16.0)).abs() < 0.2);
    run(&mut s, "sheet.unfold", json!({}));
    let state = s.model.state();
    let body = &state.bodies[0].body;
    assert!(!body.contains(Vec3::new(c.x, c.y, 0.5)));
    assert!(body.contains(Vec3::new(c.x, c.y, 1.5)));
    assert_eq!(flat(&mut s)["pockets"][0]["depth_mm"], 1.0);
    run(&mut s, "sheet.refold", json!({}));
    assert!((volume(&mut s) - (18972.62196081845 - 16.0)).abs() < 0.2);
}

#[test]
fn flipped_sheet_pocket_keeps_depth_in_the_local_thickness_direction() {
    let mut s = Session::default();
    rectangle_tool(&mut s, "Base", [0.0, 0.0], [100.0, 60.0]);
    run(&mut s, "sheet.flange", json!({"sketch":"Base","flip":true}));
    rectangle_tool(&mut s, "Pocket", [20.0, 20.0], [30.0, 30.0]);
    run(&mut s, "solid.extrude", json!({"sketch":"Pocket","distance":1,"direction":"negative","operation":"cut"}));
    for command in [None, Some("sheet.unfold"), Some("sheet.refold")] {
        if let Some(command) = command {
            run(&mut s, command, json!({}));
        }
        assert!((volume(&mut s) - 14900.0).abs() < 1e-6);
        let state = s.model.state();
        assert!(!state.bodies[0].body.contains(solidcraft_geom::Vec3::new(25.0, 25.0, -0.5)));
        assert!(state.bodies[0].body.contains(solidcraft_geom::Vec3::new(25.0, 25.0, -1.5)));
    }
}

#[test]
fn multiple_profile_intersect_keeps_tool_union_for_solids_and_sheet_groups() {
    for sheet in [false, true] {
        let mut s = Session::default();
        if sheet {
            base(&mut s, 100.0, 60.0);
        } else {
            run(&mut s, "solid.box", json!({"length":100,"width":60,"height":2.5,"body_name":"Blank"}));
        }
        let target = s.model.state().bodies[0].name.clone();
        run(&mut s, "solid.box", json!({"corner":[200,0,0],"length":10,"width":10,"height":10,"body_name":"Control"}));
        run(&mut s, "sketch.create", json!({"plane":"XY","name":"TwoIslands"}));
        for x in [20.0, 60.0] {
            run(&mut s, "sketch.rectangle.two_point", json!({"p0":[x,20],"p1":[x+10.0,30]}));
        }
        run(&mut s, "sketch.finish", json!({}));
        run(&mut s, "solid.extrude", json!({"sketch":"TwoIslands","operation":"intersect","distance":3,"targets":[target]}));
        assert!((volume(&mut s) - 1500.0).abs() < 1e-6);
        if sheet {
            for command in ["sheet.unfold", "sheet.refold"] {
                run(&mut s, command, json!({"body":target}));
                assert!((volume(&mut s) - 1500.0).abs() < 1e-6);
                assert_eq!(flat(&mut s)["outline"].as_array().unwrap().len(), 2);
            }
        }
        let state = s.model.state();
        for x in [25.0, 65.0] {
            assert!(state.bodies.iter().any(|b| b.body.contains(solidcraft_geom::Vec3::new(x, 25.0, 1.0))));
        }
        assert!(!state.bodies.iter().any(|b| b.body.contains(solidcraft_geom::Vec3::new(50.0, 25.0, 1.0))));
        assert!(state.bodies.iter().any(|b| b.name == "Control" && b.body.contains(solidcraft_geom::Vec3::new(205.0, 5.0, 5.0))));
    }
}

#[test]
fn deleting_sheet_material_prunes_cosmetic_threads_and_undo_restores_them() {
    let mut s = Session::default();
    run(&mut s, "sketch.create", json!({"plane":"XY","name":"ThreadPlate"}));
    run(&mut s, "sketch.rectangle.two_point", json!({"p0":[0,0],"p1":[100,60]}));
    run(&mut s, "sketch.circle.center", json!({"center":[50,30],"radius":5}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "sheet.flange", json!({"sketch":"ThreadPlate","profiles":[{"point":[10,10]}]}));
    run(&mut s, "solid.thread", json!({"face":[55,30,1],"designation":"M10"}));
    assert_eq!(s.model.state().threads.len(), 1);
    rectangle_tool(&mut s, "Outside", [120.0, 20.0], [130.0, 30.0]);
    run(&mut s, "solid.extrude", json!({"sketch":"Outside","through_all":true,"direction":"symmetric","operation":"intersect"}));
    assert!(s.model.state().bodies.is_empty());
    assert!(s.model.state().sheets.is_empty());
    assert!(s.model.state().threads.is_empty());
    assert!(solidcraft_doc::world_state(&s.doc, &s.model.state()).threads.is_empty());
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(s.model.state().threads.len(), 1);
}

#[test]
fn sheet_pocket_transfer_keeps_local_depth_world_pose_and_history() {
    use solidcraft_geom::Vec3;
    fn check(s: &Session, name: &str, owner: u64, local_origin: Vec3, local_y: Vec3, transferred: bool) {
        let state = s.model.state();
        let mb = state.body(name).unwrap();
        assert_eq!(mb.component, owner);
        assert_eq!(mb.transferred, transferred);
        let sh = state.sheets.iter().find(|sh| sh.body == name).unwrap();
        assert!(sh.frame.origin.dist(local_origin) < 1e-8, "{:?}", sh.frame);
        assert!(sh.frame.x.dist(Vec3::X) < 1e-8);
        assert!(sh.frame.y.dist(local_y) < 1e-8);
        assert_eq!(sh.pockets.len(), 1);
        assert_eq!((sh.pockets[0].z0, sh.pockets[0].z1), (0.0, 1.0));
        let cut = sh.frame.to_world(solidcraft_geom::Vec2::new(25.0, 25.0)) + sh.frame.normal() * 0.5;
        let floor = sh.frame.to_world(solidcraft_geom::Vec2::new(25.0, 25.0)) + sh.frame.normal() * 1.5;
        assert!(!mb.body.contains(cut));
        assert!(mb.body.contains(floor));
        let world = s.world_state();
        let body = &world.body(name).unwrap().body;
        let bounds = body.tessellate(1e-3).unwrap().bounds();
        assert!(bounds.min.dist(Vec3::ZERO) < 1e-6, "{bounds:?}");
        assert!(bounds.max.dist(Vec3::new(100.0, 60.0, 2.5)) < 1e-6, "{bounds:?}");
        assert!(!body.contains(Vec3::new(25.0, 25.0, 0.5)));
        assert!(body.contains(Vec3::new(25.0, 25.0, 1.5)));
        assert!((solidcraft_kernel::measure(body).unwrap().volume - 14900.0).abs() < 1e-6);
    }
    let mut s = Session::default();
    base(&mut s, 100.0, 60.0);
    rectangle_tool(&mut s, "Pocket", [20.0, 20.0], [30.0, 30.0]);
    run(&mut s, "solid.extrude", json!({"sketch":"Pocket","distance":1,"operation":"cut"}));
    let name = s.model.state().sheets[0].body.clone();
    check(&s, &name, 0, Vec3::ZERO, Vec3::Y, false);
    let destination = run(&mut s, "component.create", json!({"name":"Machined","parent":0}));
    let component = destination["component"].as_u64().unwrap();
    let occurrence = destination["occurrence"].as_u64().unwrap();
    run(&mut s, "occurrence.move", json!({"occurrence":occurrence,"translate":[30,20,10],"axis":[1,0,0],"angle":"90 deg","capture":true}));
    run(&mut s, "component.move_bodies", json!({"component":component,"bodies":[name]}));
    assert!(!s.doc.body_offsets.contains_key(&name), "recorded transfer must not leave a legacy placement offset");
    let origin = Vec3::new(-30.0, -10.0, 20.0);
    let y = Vec3::new(0.0, 0.0, -1.0);
    check(&s, &name, component, origin, y, true);
    run(&mut s, "edit.undo", json!({}));
    check(&s, &name, 0, Vec3::ZERO, Vec3::Y, false);
    run(&mut s, "edit.redo", json!({}));
    check(&s, &name, component, origin, y, true);
    run(&mut s, "component.activate", json!({"component":component}));
    for command in ["sheet.unfold", "sheet.refold"] {
        run(&mut s, command, json!({"body":name}));
        check(&s, &name, component, origin, y, true);
        let f = run(&mut s, "sheet.flat_pattern", json!({"body":name}));
        assert_eq!(f["pockets"][0]["side"], "bottom");
        assert_eq!(f["pockets"][0]["depth_mm"], 1.0);
    }
    let mut reopened = Session::new(solidcraft_doc::Document::from_json(&s.doc.to_json()).unwrap());
    check(&reopened, &name, component, origin, y, true);
    run(&mut reopened, "component.activate", json!({"component":component}));
    run(&mut reopened, "sheet.unfold", json!({"body":name}));
    check(&reopened, &name, component, origin, y, true);
}

#[test]
fn base_and_edge_flange_follow_the_k_factor() {
    let mut s = Session::default();
    base(&mut s, 100.0, 60.0);
    assert!(rel(volume(&mut s), 15000.0) < 1e-9);
    run(&mut s, "sheet.flange", json!({"edges": [[50, 0, 2.5]], "height": 20}));
    // Oracle 68: flat = 60 − 5 + (π/2)(2.5 + 0.44·2.5) + 20 − 5.
    let want = 60.0 - 5.0 + FRAC_PI_2 * (2.5 + 0.44 * 2.5) + 15.0;
    let f = flat(&mut s);
    assert!((f["flat_size_mm"][1].as_f64().unwrap_or(0.0) - want).abs() < 1e-6, "{f}");
    assert_eq!(f["bends"][0]["direction"], "up");
    assert!(rel(volume(&mut s), 18972.622) < 1e-4, "{}", volume(&mut s));
    let m = run(&mut s, "inspect.measure", json!({}));
    assert_eq!(m["total"]["faces"], 14, "{m}");
    // The angle is a parameter: 45° (oracle 70 uses height 15).
    let names = s.doc.features.last().map(|f| f.param_names.clone()).unwrap_or_default();
    run(&mut s, "parameters.change", json!({"name": names[0], "expression": "15"}));
    run(&mut s, "parameters.change", json!({"name": names[1], "expression": "45 deg"}));
    assert!(rel(volume(&mut s), 18450.777) < 1e-4, "{}", volume(&mut s));
    assert!((flat(&mut s)["flat_size_mm"][1].as_f64().unwrap_or(0.0) - 73.685298).abs() < 1e-5);
    // DXF: outline and an up bend line.
    let d = run(&mut s, "sheet.export_dxf", json!({}));
    let text = d["dxf"].as_str().unwrap_or_default();
    assert!(text.contains("BEND_UP") && text.contains("OUTLINE"), "{text}");
    assert!(s.execute("sheet.flange", &json!({"edges": [[50, 30, 40]], "height": 20})).is_err(), "not on an edge");
}

#[test]
fn rules_are_parameters() {
    let mut s = Session::default();
    run(&mut s, "parameters.change", json!({"name": "Thick", "expression": "1.5 mm"}));
    run(&mut s, "sheet.manage_rules", json!({"name": "Custom", "thickness": "Thick", "k_factor": 0.33, "bend_radius": "3 mm"}));
    base(&mut s, 80.0, 50.0);
    run(&mut s, "sheet.flange", json!({"edges": [[40, 50, 1.5], [40, 0, 1.5]], "height": 20}));
    // Oracle 71: 50 − 9 + 2·(π/2)(3 + 0.495) + 2·15.5 = 82.980.
    let want = 50.0 - 2.0 * 4.5 + 2.0 * FRAC_PI_2 * (3.0 + 0.33 * 1.5) + 2.0 * 15.5;
    assert!((flat(&mut s)["flat_size_mm"][1].as_f64().unwrap_or(0.0) - want).abs() < 1e-6);
    assert!(rel(volume(&mut s), 10053.717) < 1e-4, "{}", volume(&mut s));
    // Thicker: everything follows.
    run(&mut s, "parameters.change", json!({"name": "Thick", "expression": "2 mm"}));
    let want = 50.0 - 2.0 * 5.0 + 2.0 * FRAC_PI_2 * (3.0 + 0.33 * 2.0) + 2.0 * 15.0;
    assert!((flat(&mut s)["flat_size_mm"][1].as_f64().unwrap_or(0.0) - want).abs() < 1e-6);
    let l = run(&mut s, "sheet.rules", json!({}));
    assert_eq!(l["active"], "Custom", "{l}");
    assert!(s.execute("sheet.manage_rules", &json!({"name": "Bad", "k_factor": 3})).is_err());
}

#[test]
fn contour_flange_hem_and_tray() {
    // Oracle 72.
    let mut s = Session::default();
    run(&mut s, "sketch.create", json!({"plane": {"origin": [0, 0, 0], "x_dir": [1, 0, 0], "y_dir": [0, 0, -1]}, "name": "Contour"}));
    run(&mut s, "sketch.line", json!({"points": [[0, 0], [40, 0], [40, -30], [70, -30]], "ids": ["l1", "l2", "l3"]}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "sheet.flange", json!({"sketch": "Contour", "curves": ["l1", "l2", "l3"], "distance": 50}));
    assert!(rel(volume(&mut s), 12097.622) < 1e-4, "{}", volume(&mut s));
    assert!((flat(&mut s)["flat_size_mm"][0].as_f64().unwrap_or(0.0) - 96.309734).abs() < 1e-5);
    // Oracle 73: flange, then a flat hem of 8.
    let mut s = Session::default();
    base(&mut s, 100.0, 60.0);
    run(&mut s, "sheet.flange", json!({"edges": [[50, 0, 2.5]], "height": 20}));
    run(&mut s, "sheet.hem", json!({"edges": [[50, 0, 20]], "length": 8}));
    assert!(rel(volume(&mut s), 21334.723) < 1e-4, "{}", volume(&mut s));
    let b = flat(&mut s)["bends"].clone();
    assert!(b.as_array().is_some_and(|a| a.iter().any(|x| (x["angle_deg"].as_f64().unwrap_or(0.0) - 180.0).abs() < 1e-9)), "{b}");
    // A tray: four flanges at once stop at each other's bends.
    let mut s = Session::default();
    base(&mut s, 100.0, 60.0);
    run(&mut s, "sheet.flange", json!({"edges": [[50, 60, 2.5], [100, 30, 2.5], [50, 0, 2.5], [0, 30, 2.5]], "height": 25}));
    let f = flat(&mut s);
    // Oracle 69: 100 − 10 + 2·5.655 + 2·20 = 141.310 across.
    assert!((f["flat_size_mm"][0].as_f64().unwrap_or(0.0) - (90.0 + 2.0 * FRAC_PI_2 * 3.6 + 40.0)).abs() < 1e-6, "{f}");
    let v = volume(&mut s);
    // Each flange runs on past its bend toward the corner until the inner corners are the
    // rule's gap apart (diagonally): 5 − (2.5 + 2.5/√2) = 0.732 at each of 8 ends.
    let section = PI / 4.0 * (25.0 - 6.25) + 2.5 * 20.0;
    let ext = 5.0 - (2.5 + 2.5 / std::f64::consts::SQRT_2);
    let want = 90.0 * 50.0 * 2.5 + section * (2.0 * 90.0 + 2.0 * 50.0) + 8.0 * ext * 2.5 * 20.0;
    assert!(rel(v, want) < 1e-4 && rel(v, 29666.234) < 1e-4, "{v} {want}");
}

#[test]
fn unfold_cut_refold_and_convert() {
    // Oracle 74: two flanges, unfold, a slot across the bend, refold.
    let mut s = Session::default();
    base(&mut s, 80.0, 50.0);
    run(&mut s, "sheet.flange", json!({"edges": [[40, 50, 2.5], [40, 0, 2.5]], "height": 20}));
    let folded = volume(&mut s);
    run(&mut s, "sheet.unfold", json!({}));
    let m = run(&mut s, "inspect.measure", json!({}));
    assert!((m["bodies"][0]["bbox"]["max"][2].as_f64().unwrap_or(0.0) - 2.5).abs() < 1e-6, "{m}");
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Slot"}));
    run(&mut s, "sketch.rectangle.two_point", json!({"p0": [35, -12], "p1": [45, 10]}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.extrude", json!({"sketch": "Slot", "through_all": true, "operation": "cut", "direction": "symmetric"}));
    run(&mut s, "sheet.refold", json!({}));
    let v = volume(&mut s);
    assert!(v < folded - 300.0, "{v} {folded}");
    assert!(rel(v, 15800.304) < 1e-4, "{v}");
    assert_eq!(flat(&mut s)["cutouts"].as_array().map(Vec::len), Some(1));
    // Convert a plate.
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 100, "width": 60, "height": 2.5, "body_name": "Plate"}));
    run(&mut s, "sheet.convert", json!({"body": "Plate", "face": [50, 30, 2.5]}));
    run(&mut s, "sheet.flange", json!({"edges": [[50, 0, 2.5]], "height": 20}));
    assert!(rel(volume(&mut s), 18972.622) < 1e-4, "{}", volume(&mut s));
    // A rod picked on its end is a round plate as thick as the rod is long (as a 20 x 20 x 30 bar
    // is a square one); a block thicker at one end is no plate.
    let mut t = Session::default();
    run(&mut t, "solid.cylinder", json!({"radius": 10, "height": 30, "body_name": "Rod"}));
    run(&mut t, "sheet.convert", json!({"body": "Rod", "face": [0, 0, 30]}));
    let f = flat(&mut t);
    assert!(rel(f["thickness"].as_f64().unwrap_or(0.0), 30.0) < 1e-9 && rel(f["flat_size_mm"][0].as_f64().unwrap_or(0.0), 20.0) < 1e-4, "{f}");
    let mut t = Session::default();
    run(&mut t, "solid.box", json!({"length": 100, "width": 60, "height": 2.5, "body_name": "Block"}));
    run(&mut t, "solid.box", json!({"corner": [0, 0, 0], "length": 20, "width": 60, "height": 10, "operation": "join"}));
    assert!(t.execute("sheet.convert", &json!({"body": "Block", "face": [50, 30, 2.5]})).is_err());
}

/// A plate (15, -20) (0, 0) (-100, 0) (-100, -20) with a 20 mm inside flange on its slanted edge
/// (2 mm sheet, R 2, K 0.5: the folded sheet holds its flat area times the thickness), unfolded,
/// and a triangle cut out with a corner on the bend line: where the line ends on the plate's
/// edge at (-5, 0), the triangle on the bend or on the base, or in the line's middle. The refold
/// failed ("sew: face 10: This wire is not simple.").
#[test]
fn a_cut_out_with_a_corner_on_a_bend_line_refolds() {
    // The base (10, -20) (-5, 0) (-100, 0) (-100, -20): 2050; the bend 25 x 3π/2; the flange 25 x 16.
    let area = 2450.0 + 37.5 * PI;
    let cases = [
        ([[-5.0, 0.0], [-3.0, -1.0], [-3.0, 1.0]], 2.0),
        ([[-5.0, 0.0], [-7.0, -1.0], [-6.0, -3.0]], 2.5),
        ([[2.5, -10.0], [2.1, -12.8], [-0.3, -9.6]], 4.0),
        ([[2.5, -10.0], [4.7, -9.6], [3.5, -8.0]], 2.0),
    ];
    for (triangle, cut) in cases {
        let mut s = Session::default();
        run(&mut s, "sheet.manage_rules", json!({"name": "Rule", "active": true, "thickness": "2 mm", "k_factor": "0.5", "bend_radius": "2 mm"}));
        run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Outline"}));
        run(&mut s, "sketch.line", json!({"points": [[15, -20], [0, 0], [-100, 0], [-100, -20]], "closed": true}));
        run(&mut s, "sketch.finish", json!({}));
        run(&mut s, "sheet.flange", json!({"sketch": "Outline", "rule": "Rule"}));
        run(&mut s, "sheet.flange", json!({"edges": [[7.5, -10, 2]], "height": 20, "radius": 2, "position": "inside"}));
        // The measure follows the bend by facets.
        let folded = volume(&mut s);
        assert!(rel(folded, area * 2.0) < 1e-4, "{folded} vs {}", area * 2.0);
        run(&mut s, "sheet.unfold", json!({}));
        run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Cutout"}));
        run(&mut s, "sketch.line", json!({"points": triangle, "closed": true}));
        run(&mut s, "sketch.finish", json!({}));
        run(&mut s, "solid.extrude", json!({"sketch": "Cutout", "through_all": true, "direction": "symmetric", "operation": "cut"}));
        assert!(rel(volume(&mut s), (area - cut) * 2.0) < 1e-9, "{triangle:?} flat: {}", volume(&mut s));
        run(&mut s, "sheet.refold", json!({}));
        let refolded = volume(&mut s);
        assert!((folded - refolded - cut * 2.0).abs() < 0.01, "{triangle:?} refolded: {refolded}, {} less", folded - refolded);
    }
}

/// Plates with an inside flange on a slanted edge (2 mm sheet, R 2, K 0.5: the folded sheet
/// holds its flat area times the thickness), unfolded, and a triangle cut out across the bend
/// line with a corner 0.05 µm beside the line, or on it at x = 84.0764335, a border between
/// two cells of the 1e-6 mm grid the sew finds vertices on. The refold failed ("sew: a
/// degenerate edge", "sew: face 13: This wire is not closed."); it loses the triangle.
#[test]
fn a_cut_corner_a_hair_from_a_bend_line_refolds() {
    let root820 = 820f64.sqrt();
    let cases = [
        // Base 1750, bend 25 x 3π/2, flange 25 x 26; a triangle of 4 mm².
        (
            [[0.0, 0.0], [100.0, 0.0], [85.0, 20.0], [0.0, 20.0]],
            [92.5, 10.0, 2.0],
            30.0,
            [[87.49999996, 9.99999997], [85.49999996, 7.99999997], [89.49999996, 7.99999997]],
            2400.0 + 37.5 * PI,
            4.0,
        ),
        // The edge is √820 long; the base loses 4 mm along its normal: 2444 − 4√820; the bend
        // √820 x 3π/2, the flange √820 x 36; a triangle of 3 mm².
        (
            [[0.0, 0.0], [100.0, 0.0], [88.0, 26.0], [0.0, 26.0]],
            [94.0, 13.0, 2.0],
            40.0,
            [[84.0764335, 24.9558467078], [82.0764335, 22.9558467078], [86.0764335, 23.9558467078]],
            2444.0 + root820 * (32.0 + 3.0 * PI / 2.0),
            3.0,
        ),
    ];
    for (outline, edge, height, triangle, area, cut) in cases {
        let mut s = Session::default();
        run(&mut s, "sheet.manage_rules", json!({"name": "Rule", "active": true, "thickness": "2 mm", "k_factor": "0.5", "bend_radius": "2 mm"}));
        run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Outline"}));
        run(&mut s, "sketch.line", json!({"points": outline, "closed": true}));
        run(&mut s, "sketch.finish", json!({}));
        run(&mut s, "sheet.flange", json!({"sketch": "Outline", "rule": "Rule"}));
        run(&mut s, "sheet.flange", json!({"edges": [edge], "height": height, "radius": 2, "position": "inside"}));
        // The measure follows the bend by facets.
        let folded = volume(&mut s);
        assert!(rel(folded, area * 2.0) < 1e-4, "{folded} vs {}", area * 2.0);
        run(&mut s, "sheet.unfold", json!({}));
        run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Cutout"}));
        run(&mut s, "sketch.line", json!({"points": triangle, "closed": true}));
        run(&mut s, "sketch.finish", json!({}));
        run(&mut s, "solid.extrude", json!({"sketch": "Cutout", "through_all": true, "direction": "symmetric", "operation": "cut"}));
        assert!(rel(volume(&mut s), (area - cut) * 2.0) < 1e-9, "flat: {}", volume(&mut s));
        run(&mut s, "sheet.refold", json!({}));
        let refolded = volume(&mut s);
        assert!((folded - refolded - cut * 2.0).abs() < 0.01, "refolded: {refolded}, {} less", folded - refolded);
    }
}

/// A 100 x 60 plate (2 mm sheet, R 2, K 0.5) with a 42 mm flange (R 1) outside its bottom edge,
/// unfolded, and a notch cut off the flat flange's free corner: an arc from the side edge,
/// through (0.6, 0.6) from the corner, to the free edge, closed by lines through a point outside
/// the sheet. The arc leaves the side edge 1.7997505 mm from the corner, in the folded flange at
/// a height of 42 − 1.7997505 = 40.2002495 mm: a border between two cells of the 1e-6 mm grid the
/// sew finds vertices on, and the arc's end and the edge's, worked out apart, differ by 7e-13
/// mm. The refold failed ("sew: face 5: This wire is not closed."); it loses the notch.
#[test]
fn a_notch_whose_arc_ends_on_a_grid_border_refolds() {
    let t = 1.7997505;
    // The flat flange ends at 39 + the bend allowance (π/2)(R + K T) = 39 + π below the plate.
    let free = -(42.0 - 1.0 - 2.0 + FRAC_PI_2 * (1.0 + 0.5 * 2.0));
    let flat = 6000.0 + 100.0 * PI + 100.0 * 39.0;
    // The notch is the triangle corner, end, end less the circle's segment over its chord; the
    // circle's centre (a, a) from the corner is as far from (0.6, 0.6) as from the arc's ends.
    let a = (t * t - 0.72) / (2.0 * t - 2.4);
    let r2 = 2.0 * (a - 0.6) * (a - 0.6);
    let angle = 2.0 * (t * 2f64.sqrt() / (2.0 * r2.sqrt())).asin();
    let notch = t * t / 2.0 - r2 / 2.0 * (angle - angle.sin());
    assert!((notch - 0.88421).abs() < 1e-5, "{notch}");
    let mut s = Session::default();
    run(&mut s, "sheet.manage_rules", json!({"name": "Rule", "active": true, "thickness": "2 mm", "k_factor": "0.5", "bend_radius": "2 mm"}));
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Outline"}));
    run(&mut s, "sketch.line", json!({"points": [[0, 0], [100, 0], [100, 60], [0, 60]], "closed": true}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "sheet.flange", json!({"sketch": "Outline", "rule": "Rule"}));
    run(&mut s, "sheet.flange", json!({"edges": [[50, 0, 2]], "height": 42, "radius": 1, "position": "outside"}));
    // The measure follows the bend by facets.
    let folded = volume(&mut s);
    assert!(rel(folded, flat * 2.0) < 1e-4, "{folded} vs {}", flat * 2.0);
    run(&mut s, "sheet.unfold", json!({}));
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Cutout"}));
    run(&mut s, "sketch.arc.three_point", json!({"start": [t, free], "through": [0.6, free + 0.6], "end": [0.0, free + t]}));
    run(&mut s, "sketch.line", json!({"points": [[0.0, free + t], [-1.0, -44.0]]}));
    run(&mut s, "sketch.line", json!({"points": [[-1.0, -44.0], [t, free]]}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.extrude", json!({"sketch": "Cutout", "through_all": true, "direction": "symmetric", "operation": "cut"}));
    assert!(rel(volume(&mut s), (flat - notch) * 2.0) < 1e-6, "flat: {}", volume(&mut s));
    run(&mut s, "sheet.refold", json!({}));
    let refolded = volume(&mut s);
    assert!((folded - refolded - notch * 2.0).abs() < 0.01, "refolded: {refolded}, {} less", folded - refolded);
}

/// An inside flange on a 100 x 60 plate (2 mm sheet, R 1, K 0.5) closes at every whole-degree
/// turn of the plate. The flange's setback trims the plate at corners rounded to the nanometre,
/// and the bend zone's corners beside them are not, so the two copies of a corner differ by a
/// fraction of a nanometre. At 62° the trimmed corner lies on a border of the 1e-6 mm grid the
/// sew finds vertices on (x = 100 cos 62° - 3 sin 62° = 44.2983135); its copies fell on either
/// side, and the flange failed ("sew: the faces do not close"). The sew's weld of edge ends a
/// hair apart joins them.
#[test]
fn an_inside_flange_on_a_turned_plate_closes_at_every_turn() {
    // The plate less the 3 mm setback, a quarter annulus (radii 1 and 3) and a 17 mm leg, 100 long.
    let want = 100.0 * 57.0 * 2.0 + PI * 2.0 * 100.0 + 17.0 * 2.0 * 100.0;
    let mut failed = Vec::new();
    for k in 0..90 {
        let turn = (k as f64).to_radians();
        let at = |x: f64, y: f64| [x * turn.cos() - y * turn.sin(), x * turn.sin() + y * turn.cos()];
        let mut s = Session::default();
        run(&mut s, "sheet.manage_rules", json!({"name": "R", "thickness": "2 mm", "k_factor": "0.5", "bend_radius": "1 mm", "active": true}));
        run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Outline"}));
        run(&mut s, "sketch.line", json!({"points": [at(0.0, 0.0), at(100.0, 0.0), at(100.0, 60.0), at(0.0, 60.0)], "closed": true}));
        run(&mut s, "sketch.finish", json!({}));
        run(&mut s, "sheet.flange", json!({"sketch": "Outline"}));
        let [x, y] = at(50.0, 0.0);
        let flange = json!({"edges": [[x, y, 2]], "height": 20, "angle": 90, "radius": 1, "position": "inside"});
        if let Err(e) = s.execute("sheet.flange", &flange) {
            failed.push(format!("{k}°: {e}"));
            continue;
        }
        assert!(rel(volume(&mut s), want) < 1e-4, "{k}°: {} vs {want}", volume(&mut s));
    }
    assert!(failed.is_empty(), "{} of 90 turns: {failed:?}", failed.len());
}

/// A sketch line across a 100 x 60 plate's top face at x = 40.
fn fold_line(s: &mut Session) {
    run(s, "sketch.create", json!({"plane": "XY", "offset": 2.5, "name": "FoldLine"}));
    run(s, "sketch.line", json!({"points": [[40, 0], [40, 60]], "ids": ["fl"]}));
    run(s, "sketch.finish", json!({}));
}

fn bbox(s: &mut Session) -> Value {
    run(s, "inspect.measure", json!({}))["bodies"][0]["bbox"].clone()
}

#[test]
fn fold_takes_the_bend_out_of_the_flat_sheet() {
    let (t, r, k) = (2.5, 2.5, 0.44);
    let ba = FRAC_PI_2 * (r + k * t);
    let mut s = Session::default();
    base(&mut s, 100.0, 60.0);
    fold_line(&mut s);
    run(&mut s, "sheet.fold", json!({"sketch": "FoldLine", "curve": "fl"}));
    // The flat pattern keeps its size; the bend's centre line is the sketch line.
    let f = flat(&mut s);
    assert!((f["flat_size_mm"][0].as_f64().unwrap_or(0.0) - 100.0).abs() < 1e-6, "{f}");
    assert!((f["flat_size_mm"][1].as_f64().unwrap_or(0.0) - 60.0).abs() < 1e-6, "{f}");
    assert!((f["bends"][0]["allowance"].as_f64().unwrap_or(0.0) - ba).abs() < 1e-9, "{f}");
    // Folded: the zone's flat volume BA·T·w becomes the bend's θ·T·(R + T/2)·w.
    let want = 15000.0 - ba * t * 60.0 + FRAC_PI_2 * t * (r + t / 2.0) * 60.0;
    assert!(rel(volume(&mut s), want) < 1e-4, "{} {want}", volume(&mut s));
    // The smaller side (x < 40) stands up: outer face at the zone start less R + T, leg 40 − BA/2
    // tall above the bend.
    let b = bbox(&mut s);
    assert!((b["min"][0].as_f64().unwrap_or(0.0) - (40.0 + ba / 2.0 - r - t)).abs() < 1e-6, "{b}");
    assert!((b["max"][2].as_f64().unwrap_or(0.0) - (t + r + 40.0 - ba / 2.0)).abs() < 1e-6, "{b}");
    assert!((b["max"][0].as_f64().unwrap_or(0.0) - 100.0).abs() < 1e-6, "{b}");
}

#[test]
fn fold_positions_fixed_side_and_flip() {
    let (t, r, k) = (2.5, 2.5, 0.44);
    let ba = FRAC_PI_2 * (r + k * t);
    let case = |p: Value| {
        let mut s = Session::default();
        base(&mut s, 100.0, 60.0);
        fold_line(&mut s);
        let mut q = json!({"sketch": "FoldLine", "curve": "fl"});
        for (k, v) in p.as_object().into_iter().flatten() {
            q[k] = v.clone();
        }
        run(&mut s, "sheet.fold", q);
        bbox(&mut s)
    };
    let x0 = |b: &Value| b["min"][0].as_f64().unwrap_or(f64::NAN);
    // Start: the bend starts at the line (on the fixed side), so the folded outer face is R + T
    // short of it. End: the bend ends there. Mould: the outer face lands on the line.
    assert!((x0(&case(json!({"position": "start"}))) - (40.0 - r - t)).abs() < 1e-6);
    assert!((x0(&case(json!({"position": "end"}))) - (40.0 + ba - r - t)).abs() < 1e-6);
    assert!((x0(&case(json!({"position": "mould"}))) - 40.0).abs() < 1e-6);
    // Keep the small side: the large one (x > 40) stands up instead.
    let b = case(json!({"fixed": [10, 30, 2.5]}));
    assert!((b["max"][0].as_f64().unwrap_or(0.0) - (40.0 - ba / 2.0 + r + t)).abs() < 1e-6, "{b}");
    assert!((b["max"][2].as_f64().unwrap_or(0.0) - (t + r + 60.0 - ba / 2.0)).abs() < 1e-6, "{b}");
    // Flipped: folds down, below the sheet.
    let b = case(json!({"flip": true}));
    assert!((b["min"][2].as_f64().unwrap_or(0.0) + (r + 40.0 - ba / 2.0)).abs() < 1e-6, "{b}");
    // A 45° fold: the leg leans.
    let b = case(json!({"angle": "45 deg"}));
    assert!(b["max"][2].as_f64().unwrap_or(0.0) < t + r + 40.0 - ba / 2.0);
}

#[test]
fn fold_carries_flanges_on_the_moving_side() {
    let mut s = Session::default();
    base(&mut s, 100.0, 60.0);
    // A flange on the left edge, then a fold between it and the rest.
    run(&mut s, "sheet.flange", json!({"edges": [[0, 30, 2.5]], "height": 20}));
    let v0 = volume(&mut s);
    fold_line(&mut s);
    run(&mut s, "sheet.fold", json!({"sketch": "FoldLine", "curve": "fl", "fixed": [90, 30, 2.5]}));
    // The flange rode along: it is above the plate now, nothing left at z < 0.
    let b = bbox(&mut s);
    assert!(b["min"][2].as_f64().unwrap_or(-1.0) > -1e-6, "{b}");
    let ba = FRAC_PI_2 * (2.5 + 0.44 * 2.5);
    let want = v0 - ba * 2.5 * 60.0 + FRAC_PI_2 * 2.5 * (2.5 + 1.25) * 60.0;
    assert!(rel(volume(&mut s), want) < 1e-4, "{} {want}", volume(&mut s));
    assert!(flat(&mut s)["bends"].as_array().map(Vec::len) == Some(2));
    // Errors: off the sheet, through the flange's bend.
    let mut e = Session::default();
    base(&mut e, 100.0, 60.0);
    assert!(e.execute("sheet.fold", &json!({"points": [[40, 0, 30], [40, 60, 30]]})).is_err());
    assert!(e.execute("sheet.fold", &json!({"points": [[40, 0, 2.5], [40, 60, 2.5]], "position": "sideways"})).is_err());
    assert!(e.execute("sheet.fold", &json!({"points": [[0.5, 0, 2.5], [0.5, 60, 2.5]]})).is_err(), "no material past the bend");
}

#[test]
fn fold_on_a_flange_panel() {
    let (t, r, k) = (2.5, 2.5, 0.44);
    let ba = FRAC_PI_2 * (r + k * t);
    let mut s = Session::default();
    base(&mut s, 100.0, 60.0);
    run(&mut s, "sheet.flange", json!({"edges": [[50, 0, 2.5]], "height": 30}));
    let flat0 = flat(&mut s)["flat_size_mm"].clone();
    let v0 = volume(&mut s);
    // The flange stands up at the plate's y = 0 edge: its outer face is the body's min y.
    let y = bbox(&mut s)["min"][1].as_f64().unwrap_or(f64::NAN);
    run(&mut s, "sheet.fold", json!({"points": [[10, y, 18], [90, y, 18]]}));
    let f = flat(&mut s);
    for i in 0..2 {
        assert!(
            (f["flat_size_mm"][i].as_f64().unwrap_or(0.0) - flat0[i].as_f64().unwrap_or(1.0)).abs() < 1e-6,
            "the flat pattern keeps its size: {f}"
        );
    }
    assert_eq!(f["bends"].as_array().map(Vec::len), Some(2));
    let len = f["bends"][1]["length"].as_f64().unwrap_or(0.0);
    assert!(len > 90.0, "{f}");
    let want = v0 - ba * t * len + FRAC_PI_2 * t * (r + t / 2.0) * len;
    assert!(rel(volume(&mut s), want) < 1e-4, "{} {want}", volume(&mut s));
    // The top part now leans over the plate (folded up, toward its top face).
    let b = bbox(&mut s);
    assert!(b["max"][2].as_f64().unwrap_or(99.0) < 25.0, "{b}");
    // Folding across the flange's own bend is refused.
    assert!(s.execute("sheet.fold", &json!({"points": [[50, y, 2], [50, y, 25]]})).is_err());
}

#[test]
fn fold_through_cut_outs() {
    let mut s = Session::default();
    base(&mut s, 100.0, 60.0);
    // A slot across the fold line (edges along and across the bend).
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Slot"}));
    run(&mut s, "sketch.rectangle.two_point", json!({"p0": [30, 20], "p1": [50, 40]}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.extrude", json!({"sketch": "Slot", "through_all": true, "operation": "cut", "direction": "symmetric"}));
    let mut plain = Session::default();
    base(&mut plain, 100.0, 60.0);
    fold_line(&mut plain);
    run(&mut plain, "sheet.fold", json!({"sketch": "FoldLine", "curve": "fl"}));
    fold_line(&mut s);
    run(&mut s, "sheet.fold", json!({"sketch": "FoldLine", "curve": "fl"}));
    let f = flat(&mut s);
    assert_eq!(f["cutouts"].as_array().map(Vec::len), Some(1), "{f}");
    let (v, vp) = (volume(&mut s), volume(&mut plain));
    // The slot takes 20 x 20 of the flat sheet; across the bend it removes the bend's share.
    assert!(v < vp - 900.0 && v > vp - 1100.0, "{v} {vp}");
    run(&mut s, "sheet.unfold", json!({}));
    assert!((volume(&mut s) - (15000.0 - 1000.0)).abs() < 1e-3, "{}", volume(&mut s));
    // A round hole across the bend folds too: its edge runs over the bend as a curve on the
    // bend's cylinder, and unfolds to the same round hole.
    let mut c = Session::default();
    base(&mut c, 100.0, 60.0);
    run(&mut c, "sketch.create", json!({"plane": "XY", "name": "Hole"}));
    run(&mut c, "sketch.circle.center", json!({"center": [40, 30], "radius": 5}));
    run(&mut c, "sketch.finish", json!({}));
    run(&mut c, "solid.extrude", json!({"sketch": "Hole", "through_all": true, "operation": "cut", "direction": "symmetric"}));
    fold_line(&mut c);
    run(&mut c, "sheet.fold", json!({"sketch": "FoldLine", "curve": "fl"}));
    let f = flat(&mut c);
    assert_eq!(f["cutouts"].as_array().map(Vec::len), Some(1), "{f}");
    let folded = volume(&mut c);
    assert!(folded < 15000.0 - 150.0, "{folded}");
    run(&mut c, "sheet.unfold", json!({}));
    let want = 15000.0 - std::f64::consts::PI * 25.0 * 2.5;
    assert!((volume(&mut c) - want).abs() < 0.05, "{} vs {want}", volume(&mut c));
}

#[test]
fn flange_edges_are_named_and_follow_the_base_sketch() {
    let mut s = Session::default();
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Base"}));
    run(&mut s, "sketch.rectangle.two_point", json!({"p0": [0, 0], "p1": [100, 60]}));
    let d = run(&mut s, "sketch.dimension", json!({"entities": ["l1"], "value": 100}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "sheet.flange", json!({"sketch": "Base"}));
    run(&mut s, "sheet.flange", json!({"edges": [[100, 30, 2.5]], "height": 20, "name": "Side"}));
    let side = s.doc.find_feature("Side").cloned().unwrap();
    assert_eq!(side.edge_names, vec!["base:l2:top".to_string()]);
    let v0 = volume(&mut s);
    // Narrower base: the flange stays on the right edge (now at x = 40).
    let name = d["param"].as_str().or_else(|| d["name"].as_str()).map(str::to_string).unwrap_or_else(|| "d1".into());
    run(&mut s, "parameters.change", json!({"name": name, "expression": "40"}));
    let b = run(&mut s, "inspect.measure", json!({}))["bodies"][0]["bbox"].clone();
    let w = b["max"][0].as_f64().unwrap_or(0.0) - b["min"][0].as_f64().unwrap_or(0.0);
    assert!((w - 40.0).abs() < 1e-3 && b["max"][2].as_f64().unwrap_or(0.0) > 19.0, "{b}");
    assert!(volume(&mut s) < v0);
    let id = side.id;
    assert_eq!(s.model.result(id).and_then(|r| r.warning.clone()), None);
    // A hem on the flange's far edge is named after the flange.
    let top = b["max"][2].as_f64().unwrap_or(0.0);
    let x = b["max"][0].as_f64().unwrap_or(0.0);
    run(&mut s, "sheet.hem", json!({"edges": [[x, 30, top]], "length": 6, "name": "H"}));
    let h = s.doc.find_feature("H").cloned().unwrap();
    assert!(h.edge_names[0].starts_with(&format!("F{id}.0:far")), "{:?}", h.edge_names);
}

/// With no fixed side, the larger side stays whichever side of the line it is on (QA s22).
#[test]
fn fold_keeps_the_larger_side_either_way() {
    for (w, line, moving_right) in [(60.0, 40.0, true), (100.0, 40.0, false)] {
        let mut s = Session::default();
        base(&mut s, w, 40.0);
        run(&mut s, "sheet.fold", json!({"points": [[line, 0, 2.5], [line, 40, 2.5]]}));
        let b = bbox(&mut s);
        let (x0, x1) = (b["min"][0].as_f64().unwrap_or(0.0), b["max"][0].as_f64().unwrap_or(0.0));
        if moving_right {
            assert!(x0.abs() < 1e-6 && x1 < line + 5.0, "the left (larger) part stays: {b}");
        } else {
            assert!((x1 - w).abs() < 1e-6 && x0 > line - 5.0, "the right (larger) part stays: {b}");
        }
    }
}

/// A 100 x 60 plate with a 90° flange up from its y = 0 edge (steel: 2.5 thick, R = T, K 0.44).
/// The flange's inner face is the plane y = 2.5 and its outer face y = 0; it stands from z = 5.
fn plate_with_flange(s: &mut Session) {
    base(s, 100.0, 60.0);
    run(s, "sheet.flange", json!({"edges": [[50, 0, 2.5]], "height": 20}));
}

/// A circle sketch on `plane`, finished.
fn circle_sketch(s: &mut Session, name: &str, plane: Value, center: [f64; 2], diameter: f64) -> Value {
    let made = run(s, "sketch.create", json!({"plane": plane, "name": name}));
    run(s, "sketch.circle.center", json!({"center": center, "diameter": diameter}));
    run(s, "sketch.finish", json!({}));
    made
}

fn cut_through(s: &mut Session, sketch: &str) {
    run(s, "solid.extrude", json!({"sketch": sketch, "through_all": true, "direction": "negative", "operation": "cut"}));
}

fn contains(s: &Session, p: [f64; 3]) -> bool {
    s.model.state().bodies.first().is_some_and(|b| b.body.contains(solidcraft_geom::Vec3::new(p[0], p[1], p[2])))
}

#[test]
fn a_sketch_on_a_flange_face_attaches_to_its_panel() {
    // Sketch coordinates on a sheet face are the flat pattern's: the flange panel starts at
    // y = 5 − BA and runs on toward −y, so a hole 7 mm up the face is at y = 5 − BA − 7.
    let ba = FRAC_PI_2 * (2.5 + 0.44 * 2.5);
    let hole = PI * 1.8 * 1.8 * 2.5;
    let mut s = Session::default();
    plate_with_flange(&mut s);
    let v0 = volume(&mut s);
    let made = circle_sketch(&mut s, "Holes", json!({"face": [50, 2.5, 12]}), [30.0, 5.0 - ba - 7.0], 3.6);
    assert_eq!(made["attachment"], "sheet_face");
    // The sketch keeps the face's name: the body, the panel (its flange) and the side of the sheet.
    let sketch = s.doc.find_feature("Holes").cloned().unwrap();
    let solidcraft_doc::FeatureKind::Sketch { plane, .. } = &sketch.kind else { panic!("{:?}", sketch.kind) };
    let name = solidcraft_doc::sheet::plane_sheet_face(plane).unwrap_or_default();
    assert!(name.starts_with("sheet:Body1:F") && name.ends_with(":top"), "{name}");
    // A cut through the flange from that sketch is a round hole 7 mm up the face, at x = 30.
    run(&mut s, "solid.extrude", json!({"sketch": "Holes", "distance": 2.5, "direction": "negative", "operation": "cut"}));
    assert!((v0 - volume(&mut s) - hole).abs() < 0.1, "{}", v0 - volume(&mut s));
    assert!(!contains(&s, [30.0, 1.25, 12.0]) && contains(&s, [30.0, 1.25, 14.5]) && contains(&s, [33.0, 1.25, 12.0]));
    let f = flat(&mut s);
    let cut = f["cutouts"][0].as_array().cloned().unwrap_or_default();
    assert!(!cut.is_empty(), "{f}");
    for p in &cut {
        let (x, y) = (p[0].as_f64().unwrap_or(f64::NAN), p[1].as_f64().unwrap_or(f64::NAN));
        assert!(((x - 30.0).hypot(y - (5.0 - ba - 7.0)) - 1.8).abs() < 1e-3, "({x}, {y})");
    }
    // `fixed: true` still freezes the picked plane instead of attaching to the panel.
    let frozen = circle_sketch(&mut s, "Frozen", json!({"face": [50, 2.5, 12], "fixed": true}), [0.0, 0.0], 1.0);
    assert_eq!(frozen["attachment"], "fixed");
}

/// The thickness of a sheet comes from a parameter: the flange's face moves with it, and a
/// sketch frozen on the old plane cuts a blind pocket in the thicker sheet. The attached
/// sketch cuts the same hole as one drawn on the thicker sheet in the first place.
#[test]
fn a_sketch_on_a_sheet_face_follows_the_sheet_when_its_thickness_changes() {
    let build = |t: f64, then: Option<f64>| {
        let mut s = Session::default();
        run(&mut s, "parameters.add", json!({"name": "T", "expression": format!("{t} mm")}));
        run(&mut s, "sheet.manage_rules", json!({"name": "AL", "thickness": "T", "k_factor": "0.5", "bend_radius": "1 mm", "active": true}));
        base(&mut s, 100.0, 60.0);
        run(&mut s, "sheet.flange", json!({"edges": [[50, 0, t]], "height": 20}));
        // The hole is drawn 10 mm up the 2 mm flange, in the flat pattern: its bend ends at
        // y = T + R = 3 and the bend allowance is (R + T/2)·π/2 = π. The coordinates stay as they are.
        let flat_y = 3.0 - PI - 10.0;
        circle_sketch(&mut s, "Holes", json!({"face": [50, t, 12]}), [30.0, flat_y], 6.0);
        run(&mut s, "sketch.create", json!({"plane": {"face": [10, 30, t]}, "offset": "5 mm", "name": "Above"}));
        run(&mut s, "sketch.finish", json!({}));
        let v0 = volume(&mut s);
        cut_through(&mut s, "Holes");
        if let Some(t2) = then {
            let r = run(&mut s, "parameters.change", json!({"name": "T", "expression": format!("{t2} mm")}));
            assert_eq!(r["errors"], json!([]), "{r}");
        }
        (s, v0)
    };
    let (mut grown, _) = build(2.0, Some(3.0));
    let (mut direct, v_direct) = build(3.0, None);
    let (vg, vd) = (volume(&mut grown), volume(&mut direct));
    // The hole is the cylinder Ø6 × 3 through the thicker sheet, as if drawn there at first.
    assert!((v_direct - vd - PI * 9.0 * 3.0).abs() < 0.1, "{}", v_direct - vd);
    assert!(rel(vg, vd) < 1e-9, "{vg} {vd}");
    let (fg, fd) = (flat(&mut grown), flat(&mut direct));
    assert_eq!(fg["pockets"], json!([]), "a blind pocket instead of the hole: {fg}");
    assert_eq!(fg["cutouts"], fd["cutouts"]);
    // The thicker flange's panel starts at flat y = 4 − 3.927 and z = T + R = 4, so the hole is
    // 10.215 mm up it: through the whole 3 mm.
    assert!(!contains(&grown, [30.0, 1.5, 14.2]) && !contains(&grown, [30.0, 0.2, 14.2]) && contains(&grown, [30.0, 1.5, 19.0]));
    // The sketch offset from the base's top face moved up with it: 3 + 5.
    let plane = run(&mut grown, "sketch.inspect", json!({"sketch": "Above"}))["plane"]["origin"][2].as_f64().unwrap_or(f64::NAN);
    assert!((plane - 8.0).abs() < 1e-9, "{plane}");
}

#[test]
fn sketches_on_the_top_and_bottom_faces_of_a_sheet_survive_unfold_refold_and_reopen() {
    let mut s = Session::default();
    base(&mut s, 100.0, 60.0);
    // The top face's coordinates are the base sketch's; the bottom face's mirror y, as seen from below.
    circle_sketch(&mut s, "Top", json!({"face": [30, 20, 2.5]}), [30.0, 20.0], 4.0);
    circle_sketch(&mut s, "Bottom", json!({"face": [60, 20, 0]}), [60.0, -20.0], 4.0);
    cut_through(&mut s, "Top");
    cut_through(&mut s, "Bottom");
    let want = 15000.0 - 2.0 * PI * 4.0 * 2.5;
    assert!((volume(&mut s) - want).abs() < 0.1, "{}", volume(&mut s));
    let holes = |f: &Value| {
        let mut centres: Vec<(f64, f64)> = f["cutouts"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|c| {
                let pts: Vec<(f64, f64)> = c
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .iter()
                    .map(|p| (p[0].as_f64().unwrap_or(f64::NAN), p[1].as_f64().unwrap_or(f64::NAN)))
                    .collect();
                let n = pts.len().max(1) as f64;
                (pts.iter().map(|p| p.0).sum::<f64>() / n, pts.iter().map(|p| p.1).sum::<f64>() / n)
            })
            .collect();
        centres.sort_by(|a, b| a.0.total_cmp(&b.0));
        centres
    };
    for command in [None, Some("sheet.unfold"), Some("sheet.refold")] {
        if let Some(command) = command {
            run(&mut s, command, json!({}));
        }
        let centres = holes(&flat(&mut s));
        assert_eq!(centres.len(), 2, "{centres:?}");
        assert!((centres[0].0 - 30.0).abs() < 0.01 && (centres[0].1 - 20.0).abs() < 0.01, "{centres:?}");
        assert!((centres[1].0 - 60.0).abs() < 0.01 && (centres[1].1 - 20.0).abs() < 0.01, "{centres:?}");
        assert!((volume(&mut s) - want).abs() < 0.1);
    }
    let mut reopened = Session::new(solidcraft_doc::Document::from_json(&s.doc.to_json()).unwrap());
    assert!((volume(&mut reopened) - want).abs() < 0.1);
    assert_eq!(holes(&flat(&mut reopened)), holes(&flat(&mut s)));
}

#[test]
fn a_sketch_made_on_the_unfolded_sheet_lands_on_the_flange_after_refolding() {
    let ba = FRAC_PI_2 * (2.5 + 0.44 * 2.5);
    let mut s = Session::default();
    plate_with_flange(&mut s);
    let v0 = volume(&mut s);
    run(&mut s, "sheet.unfold", json!({}));
    // Unfolded, the flange lies in the sheet's plane: the one flat face covers it too.
    let y = 5.0 - ba - 7.0;
    circle_sketch(&mut s, "Holes", json!({"face": [30, y, 2.5]}), [30.0, y], 3.6);
    cut_through(&mut s, "Holes");
    run(&mut s, "sheet.refold", json!({}));
    assert!((v0 - volume(&mut s) - PI * 1.8 * 1.8 * 2.5).abs() < 0.1, "{}", v0 - volume(&mut s));
    assert!(!contains(&s, [30.0, 1.25, 12.0]) && contains(&s, [30.0, 1.25, 14.5]));
}

#[test]
fn a_sketch_on_a_sheet_face_reports_a_lost_attachment_instead_of_moving_to_another_face() {
    let mut s = Session::default();
    plate_with_flange(&mut s);
    circle_sketch(&mut s, "Holes", json!({"face": [50, 2.5, 12]}), [30.0, -8.0], 3.6);
    let id = s.doc.find_feature("Holes").map(|f| f.id).unwrap_or(0);
    let status = |s: &Session| s.placed_document().sketch_face_attachment(id, &s.model);
    assert!(matches!(status(&s), solidcraft_doc::FaceAttachmentStatus::Sheet { plane: Some(_) }));
    // With the flange suppressed there is no such face; the base's top face is not a substitute.
    run(&mut s, "timeline.suppress", json!({"feature": "EdgeFlange1", "suppressed": true}));
    let error = run(&mut s, "timeline.inspect", json!({"feature": "Holes"}))["error"].as_str().unwrap_or_default().to_string();
    assert!(error.contains("lost face attachment") && error.contains("reattach"), "{error}");
    let solidcraft_doc::FaceAttachmentStatus::Lost { message, repair_plane } = status(&s) else { panic!("{:?}", status(&s)) };
    assert!(message.contains("sheet face") && repair_plane.is_some(), "{message}");
    // Redefine Sketch Plane repairs it on another face, here the base's top (its sketch keeps its geometry).
    let repaired = run(&mut s, "sketch.redefine", json!({"sketch": "Holes", "plane": {"face": [10, 30, 2.5]}}));
    assert_eq!(repaired["attachment"], "sheet_face");
    assert!(run(&mut s, "timeline.inspect", json!({"feature": "Holes"}))["error"].is_null());
    assert!(matches!(status(&s), solidcraft_doc::FaceAttachmentStatus::Sheet { .. }));
    // The flange is back, and a sketch attached to it again is healthy.
    run(&mut s, "timeline.suppress", json!({"feature": "EdgeFlange1", "suppressed": false}));
    let again = run(&mut s, "sketch.redefine", json!({"sketch": "Holes", "plane": {"face": [50, 2.5, 12]}}));
    assert_eq!(again["attachment"], "sheet_face");
    assert!(run(&mut s, "timeline.inspect", json!({"feature": "Holes"}))["error"].is_null());
    assert!(matches!(status(&s), solidcraft_doc::FaceAttachmentStatus::Sheet { .. }));
}

#[test]
fn renaming_a_sheet_body_keeps_its_sketches_on_their_faces() {
    let mut s = Session::default();
    plate_with_flange(&mut s);
    let v0 = volume(&mut s);
    circle_sketch(&mut s, "Holes", json!({"face": [50, 2.5, 12]}), [30.0, -8.0], 3.6);
    run(&mut s, "sketch.create", json!({"plane": {"face": [10, 30, 2.5]}, "offset": "5 mm", "name": "Above"}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "body.rename", json!({"body": "Body1", "name": "Bracket"}));
    for (sketch, face) in [("Holes", "sheet:Bracket:F"), ("Above", "sheet:Bracket:base:top")] {
        let feature = s.doc.find_feature(sketch).cloned().unwrap();
        let solidcraft_doc::FeatureKind::Sketch { plane, .. } = &feature.kind else { panic!("{:?}", feature.kind) };
        assert!(solidcraft_doc::sheet::plane_sheet_face(plane).is_some_and(|name| name.starts_with(face)), "{sketch}: {:?}", feature.kind);
        assert!(run(&mut s, "timeline.inspect", json!({"feature": sketch}))["error"].is_null(), "{sketch}");
    }
    cut_through(&mut s, "Holes");
    assert!((v0 - volume(&mut s) - PI * 1.8 * 1.8 * 2.5).abs() < 0.1, "{}", v0 - volume(&mut s));
}

/// A tray shrunk by a parameter change unfolds like the same tray built at that size. The
/// sketch solved again leaves its corners ~1e-13 mm off, and the flat pattern's planar
/// arrangement used to drop a bend zone through a gap that small: "planar region arrangement
/// failed", for W alone or both sizes, at 72° or 90°, while growing the tray worked.
#[test]
fn tray_shrunk_by_a_parameter_unfolds_like_one_built_at_that_size() {
    for angle in [72.0, 90.0] {
        let tray = |s: &mut Session, w: f64, h: f64| {
            run(s, "parameters.add", json!({"name": "W", "expression": format!("{w} mm")}));
            run(s, "parameters.add", json!({"name": "H", "expression": format!("{h} mm")}));
            run(s, "sheet.manage_rules", json!({"name": "AL2", "thickness": "2 mm", "k_factor": "0.5", "bend_radius": "1 mm", "active": true}));
            run(s, "sketch.create", json!({"plane": "XY", "name": "Tray"}));
            run(s, "sketch.rectangle.two_point", json!({"p0": [0, 0], "p1": [w, h]}));
            run(s, "sketch.constraint.fix", json!({"entities": ["p1"]}));
            run(s, "sketch.dimension", json!({"entities": ["l1"], "value": "W"}));
            run(s, "sketch.dimension", json!({"entities": ["l2"], "value": "H"}));
            run(s, "sketch.finish", json!({}));
            run(s, "sheet.flange", json!({"type": "base", "sketch": "Tray"}));
            let edges = json!([[w / 2.0, 0, 2], [w, h / 2.0, 2], [w / 2.0, h, 2], [0, h / 2.0, 2]]);
            run(s, "sheet.flange", json!({"type": "edge", "edges": edges, "height": 38.6, "angle": angle}));
            run(s, "sheet.unfold", json!({}));
        };
        let mut direct = Session::default();
        tray(&mut direct, 400.0, 300.0);
        let want = flat(&mut direct)["flat_size_mm"].clone();
        let mut shrunk = Session::default();
        tray(&mut shrunk, 2000.0, 2500.0);
        for (name, expr) in [("W", "400 mm"), ("H", "300 mm")] {
            let r = run(&mut shrunk, "parameters.change", json!({"name": name, "expression": expr}));
            assert_eq!(r["errors"], json!([]), "{angle}° {name} = {expr}: {r}");
        }
        let got = flat(&mut shrunk)["flat_size_mm"].clone();
        for k in 0..3 {
            assert!((got[k].as_f64().unwrap() - want[k].as_f64().unwrap()).abs() < 1e-9, "{angle}°: {got} {want}");
        }
        assert!(rel(volume(&mut shrunk), volume(&mut direct)) < 1e-9, "{angle}°");
    }
}

/// A 55 x 30 plate whose bottom edge bends up by 1 in 100 over its last 5 mm, with a flange on
/// the straight 50 mm, unfolds at any turn. The flange keeps its start rounded to the
/// nanometre, so its bend zone meets the plate a fraction of a nanometre off the corner, and
/// the bent piece of edge, a hair from the zone's edge, crosses it a few hundredths of a
/// micrometre before the corner. Split there, the piece left a sliver shorter than the
/// tolerance, which the flat pattern's union took for a second way out of the corner and
/// closed as a loop of its own: "planar region arrangement failed" at 46 of these 90 turns.
#[test]
fn a_flange_beside_a_slightly_bent_edge_unfolds_at_any_turn() {
    // Base 55 x 30 less the 5 x 0.05 triangle under the bent piece; bend π and leg 17 along 50.
    let area = 55.0 * 30.0 - 5.0 * 0.05 / 2.0 + 50.0 * (PI + 17.0);
    let mut failed = Vec::new();
    for k in 0..90 {
        let turn = (k as f64).to_radians();
        let at = |x: f64, y: f64| [x * turn.cos() - y * turn.sin(), x * turn.sin() + y * turn.cos()];
        let mut s = Session::default();
        run(&mut s, "sheet.manage_rules", json!({"name": "R", "thickness": "2 mm", "k_factor": "0.5", "bend_radius": "1 mm", "active": true}));
        run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Outline"}));
        let outline = [(95.0, 50.05), (100.0, 50.0), (150.0, 50.0), (150.0, 80.0), (95.0, 80.0)].map(|(x, y)| at(x, y));
        run(&mut s, "sketch.line", json!({"points": outline, "closed": true}));
        run(&mut s, "sketch.finish", json!({}));
        run(&mut s, "sheet.flange", json!({"sketch": "Outline"}));
        let [x, y] = at(125.0, 50.0);
        run(&mut s, "sheet.flange", json!({"edges": [[x, y, 2]], "height": 20, "angle": 90, "radius": 1, "position": "outside"}));
        if let Err(e) = s.execute("sheet.unfold", &json!({})) {
            failed.push(format!("{k}°: {e}"));
            continue;
        }
        let f = flat(&mut s);
        assert_eq!(f["outline"].as_array().map(Vec::len), Some(1), "{k}°: {f}");
        assert!((flat_area(&f) - area).abs() < 1e-6, "{k}°: {} vs {area}", flat_area(&f));
        assert!(rel(volume(&mut s), area * 2.0) < 1e-4, "{k}°: {} vs {}", volume(&mut s), area * 2.0);
    }
    assert!(failed.is_empty(), "{} of 90 turns: {failed:?}", failed.len());
}

#[test]
fn the_bend_table_and_the_dxf_follow_what_flat_cuts_leave_of_a_bend() {
    // A 200 x 100 plate with a 40 mm flange along y = 0: the bend's centre line runs at y = -pi/2
    // (R 1, T 2, K 0.5: a bend allowance of pi) from x = 0 to 200.
    let mut s = Session::default();
    run(&mut s, "sheet.manage_rules", json!({"name": "R", "thickness": "2 mm", "k_factor": "0.5", "bend_radius": "2 mm", "active": true}));
    base(&mut s, 200.0, 100.0);
    run(&mut s, "sheet.flange", json!({"edges": [[100, 0, 2]], "height": 40, "angle": 90, "radius": 1, "position": "outside"}));
    // (bend number, length, start x, end x) of every row; all on the centre line.
    let rows = |s: &mut Session| -> Vec<(u64, f64, f64, f64)> {
        let f = flat(s);
        let num = |v: &Value| v.as_f64().unwrap_or(f64::NAN);
        f["bends"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|b| {
                assert!((num(&b["start"][1]) + FRAC_PI_2).abs() < 1e-9 && (num(&b["end"][1]) + FRAC_PI_2).abs() < 1e-9, "{b}");
                (b["bend"].as_u64().unwrap_or(0), num(&b["length"]), num(&b["start"][0]), num(&b["end"][0]))
            })
            .collect()
    };
    let close = |got: &[(u64, f64, f64, f64)], want: &[(u64, f64, f64, f64)]| {
        got.len() == want.len()
            && got.iter().zip(want).all(|(g, w)| g.0 == w.0 && (g.1 - w.1).abs() < 1e-9 && (g.2 - w.2).abs() < 1e-9 && (g.3 - w.3).abs() < 1e-9)
    };
    // The DXF's bend lines: both ends' x of every LINE along the centre line, left to right; the
    // BEND_UP layer holds nothing else.
    let bend_lines = |s: &mut Session| -> Vec<(f64, f64)> {
        let text = run(s, "sheet.export_dxf", json!({}))["dxf"].as_str().unwrap_or_default().to_string();
        let on_layer = text.lines().collect::<Vec<_>>().windows(2).filter(|w| w[0].trim() == "8" && w[1].trim() == "BEND_UP").count();
        let mut found: Vec<(f64, f64)> = solidcraft_io::read_dxf(&text)
            .unwrap()
            .iter()
            .filter_map(|g| match g {
                solidcraft_io::Geom2::Line(a, b) if (a.y + FRAC_PI_2).abs() < 1e-6 && (b.y + FRAC_PI_2).abs() < 1e-6 => {
                    Some((a.x.min(b.x), a.x.max(b.x)))
                }
                _ => None,
            })
            .collect();
        found.sort_by(|a, b| a.0.total_cmp(&b.0));
        assert_eq!(found.len(), on_layer, "{text}");
        found
    };
    assert!(close(&rows(&mut s), &[(1, 200.0, 0.0, 200.0)]), "{:?}", rows(&mut s));
    assert_eq!(bend_lines(&mut s).len(), 1);
    // A flat cut takes the flange's left half, its bend and a strip of the base.
    run(&mut s, "sheet.unfold", json!({}));
    rectangle_tool(&mut s, "Half", [-10.0, -60.0], [100.0, 10.0]);
    run(&mut s, "solid.extrude", json!({"sketch": "Half", "through_all": true, "direction": "symmetric", "operation": "cut"}));
    run(&mut s, "sheet.refold", json!({}));
    assert!(close(&rows(&mut s), &[(1, 100.0, 100.0, 200.0)]), "{:?}", rows(&mut s));
    let lines = bend_lines(&mut s);
    assert!(lines.len() == 1 && (lines[0].0 - 100.0).abs() < 1e-6 && (lines[0].1 - 200.0).abs() < 1e-6, "{lines:?}");
    let csv = run(&mut s, "sheet.export_bend_table", json!({}));
    let text = csv["text"].as_str().unwrap_or_default();
    assert_eq!((text.lines().count(), csv["bends"].as_u64()), (2, Some(1)), "{text}");
    assert!(text.lines().nth(1).is_some_and(|l| l.contains(",1,90.000000000,up,1.000000000,3.141592654,100.000000000,100.000000000,")), "{text}");
    // A slot across what is left splits the bend: a row and a line for each piece, both bend 1.
    run(&mut s, "sheet.unfold", json!({}));
    rectangle_tool(&mut s, "Slot", [140.0, -20.0], [160.0, 10.0]);
    run(&mut s, "solid.extrude", json!({"sketch": "Slot", "through_all": true, "direction": "symmetric", "operation": "cut"}));
    run(&mut s, "sheet.refold", json!({}));
    assert!(close(&rows(&mut s), &[(1, 40.0, 100.0, 140.0), (1, 40.0, 160.0, 200.0)]), "{:?}", rows(&mut s));
    let lines = bend_lines(&mut s);
    assert!(lines.len() == 2 && (lines[0].1 - 140.0).abs() < 1e-6 && (lines[1].0 - 160.0).abs() < 1e-6, "{lines:?}");
    let csv = run(&mut s, "sheet.export_bend_table", json!({}));
    assert_eq!((csv["text"].as_str().unwrap_or_default().lines().count(), csv["bends"].as_u64()), (3, Some(2)));
    // Unfolded, the flat pattern lists the same pieces.
    run(&mut s, "sheet.unfold", json!({}));
    assert!(close(&rows(&mut s), &[(1, 40.0, 100.0, 140.0), (1, 40.0, 160.0, 200.0)]), "{:?}", rows(&mut s));
}

/// The flat pattern's area: its outline loops less its cut-outs.
fn flat_area(f: &Value) -> f64 {
    let loops = |k: &str| -> f64 {
        let all = f[k].as_array().cloned().unwrap_or_default();
        all.iter()
            .map(|l| {
                let pts: Vec<(f64, f64)> = l
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .iter()
                    .map(|q| (q[0].as_f64().unwrap_or(f64::NAN), q[1].as_f64().unwrap_or(f64::NAN)))
                    .collect();
                polygon_area(&pts)
            })
            .sum()
    };
    loops("outline") - loops("cutouts")
}

#[test]
fn an_edge_flange_made_after_a_cut_runs_along_the_edge_the_cut_left() {
    // A 200 x 100 plate, its bottom left 100 x 60 cut away, then a flange picked on what is left of
    // the bottom edge (x 100 … 200): it used to span the whole original edge, x 0 … 200, its left
    // half standing beside nothing.
    let mut s = Session::default();
    run(&mut s, "sheet.manage_rules", json!({"name": "R", "thickness": "2 mm", "k_factor": "0.5", "bend_radius": "2 mm", "active": true}));
    base(&mut s, 200.0, 100.0);
    rectangle_tool(&mut s, "Corner", [-10.0, -10.0], [100.0, 60.0]);
    run(&mut s, "solid.extrude", json!({"sketch": "Corner", "through_all": true, "direction": "symmetric", "operation": "cut"}));
    assert!((volume(&mut s) - 14_000.0 * 2.0).abs() < 1e-6);
    run(&mut s, "sheet.flange", json!({"edges": [[150, 0, 2]], "height": 40, "angle": 90, "radius": 1, "position": "outside"}));
    assert!(no_errors_or_warnings(&s), "{:?}", s.model.results);
    let f = flat(&mut s);
    let bends = f["bends"].as_array().cloned().unwrap_or_default();
    assert_eq!(bends.len(), 1, "{f}");
    let x = |k: &str| bends[0][k][0].as_f64().unwrap_or(f64::NAN);
    assert!(
        (x("start") - 100.0).abs() < 1e-9 && (x("end") - 200.0).abs() < 1e-9 && (bends[0]["length"].as_f64().unwrap_or(0.0) - 100.0).abs() < 1e-9,
        "{f}"
    );
    assert_eq!(f["outline"].as_array().map(Vec::len), Some(1), "{f}");
    // The flat pattern: the plate less the cut, and the flange (π of bend, 37 of panel) on 100 mm.
    assert!((flat_area(&f) - (14_000.0 + 100.0 * (PI + 37.0))).abs() < 1e-6, "{}", flat_area(&f));
    // Folded: the bend keeps its volume (K 0.5); the flange stands on x 100 … 200 only.
    let want = (14_000.0 + 100.0 * (PI + 37.0)) * 2.0;
    assert!((volume(&mut s) - want).abs() < 1e-3 * want, "{} vs {want}", volume(&mut s));
    assert!(contains(&s, [150.0, -2.0, 20.0]) && !contains(&s, [50.0, -2.0, 20.0]));
    let state = s.model.state();
    let body = &state.bodies[0].body;
    assert!(body.validity().is_empty() && body.shell_count() == 1);
    for command in ["sheet.unfold", "sheet.refold"] {
        run(&mut s, command, json!({}));
        assert!((volume(&mut s) - want).abs() < 1e-3 * want, "{command}: {}", volume(&mut s));
    }
}

/// A 200 x 100 plate (2 mm, K 0.5) with `cut`, a rectangle between two corners, cut through it.
fn plate_with_a_cut(s: &mut Session, cut: [[f64; 2]; 2]) {
    run(s, "sheet.manage_rules", json!({"name": "R", "thickness": "2 mm", "k_factor": "0.5", "bend_radius": "1 mm", "active": true}));
    base(s, 200.0, 100.0);
    rectangle_tool(s, "Cut", cut[0], cut[1]);
    run(s, "solid.extrude", json!({"sketch": "Cut", "through_all": true, "direction": "symmetric", "operation": "cut"}));
}

#[test]
fn edge_flanges_on_either_side_of_a_notch_span_the_side_they_were_picked_on() {
    // A notch 40 wide and 30 deep in the middle of the bottom edge leaves x 0 … 80 and 120 … 200 of
    // it. A 90° flange 40 high set inside (setback 3, BA π, panel 37) spans the side it was picked
    // on and takes its setback off that side alone; picked on both sides, each has its own.
    for (edges, sides) in
        [(json!([[160, 0, 2]]), [false, true]), (json!([[40, 0, 2]]), [true, false]), (json!([[40, 0, 2], [160, 0, 2]]), [true, true])]
    {
        let mut s = Session::default();
        plate_with_a_cut(&mut s, [[80.0, -10.0], [120.0, 30.0]]);
        run(&mut s, "sheet.flange", json!({"edges": edges, "height": 40, "angle": 90}));
        assert!(no_errors_or_warnings(&s), "{edges}: {:?}", s.model.results);
        let f = flat(&mut s);
        let want: Vec<(f64, f64)> = [(0.0, 80.0), (120.0, 200.0)].into_iter().zip(sides).filter_map(|(span, on)| on.then_some(span)).collect();
        let bends: Vec<(f64, f64)> = f["bends"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|b| (b["start"][0].as_f64().unwrap_or(f64::NAN), b["end"][0].as_f64().unwrap_or(f64::NAN)))
            .collect();
        assert!(
            bends.len() == want.len() && bends.iter().zip(&want).all(|(b, w)| (b.0 - w.0).abs() < 1e-9 && (b.1 - w.1).abs() < 1e-9),
            "{edges}: {f}"
        );
        assert_eq!(f["outline"].as_array().map(Vec::len), Some(1), "{edges}: {f}");
        // The plate less the notch, and for each flange 80 × (π + 37) less the 80 × 3 it set back.
        let area = 20_000.0 - 40.0 * 30.0 + want.len() as f64 * 80.0 * (PI + 37.0 - 3.0);
        assert!((flat_area(&f) - area).abs() < 1e-6, "{edges}: {}", flat_area(&f));
        assert!(rel(volume(&mut s), area * 2.0) < 1e-4, "{edges}: {} vs {}", volume(&mut s), area * 2.0);
        // A flange stands with its outer face on the edge (y 0 … 2); a side without one keeps the
        // base up to the edge, which the bend's round outer corner leaves free.
        for (x, on) in [40.0, 160.0].into_iter().zip(sides) {
            assert_eq!(contains(&s, [x, 1.0, 20.0]), on, "{edges}: a flange at x {x}");
            assert_eq!(contains(&s, [x, 0.2, 0.2]), !on, "{edges}: the base's own edge at x {x}");
        }
        let state = s.model.state();
        let body = &state.bodies[0].body;
        assert!(body.validity().is_empty() && body.shell_count() == 1, "{edges}");
        for command in ["sheet.unfold", "sheet.refold"] {
            run(&mut s, command, json!({}));
            assert!(rel(volume(&mut s), area * 2.0) < 1e-4, "{edges}, {command}: {}", volume(&mut s));
        }
    }
    // An edge that a cut has taken whole is refused.
    let mut s = Session::default();
    plate_with_a_cut(&mut s, [[-10.0, -10.0], [210.0, 30.0]]);
    let e = s.execute("sheet.flange", &json!({"edges": [[100, 0, 2]], "height": 40})).map(|_| ()).unwrap_err().to_string();
    assert!(e.contains("a cut-out has taken that edge away"), "{e}");
}

#[test]
fn an_edge_flange_beside_a_ledge_as_tall_as_its_setback_or_a_little_taller_folds() {
    // A 100 x 60 plate (2 mm, K 0.5) with a ledge 3 wide beside the right end of its bottom edge,
    // and a 90° flange 20 high on that edge, radius 1: setback 3 inside, half of it in the
    // middle; BA π, panel 17. A ledge as tall as the setback goes with the strip the setback
    // takes, and the bend runs along what the base keeps, x 0 … 97. One 0.05 taller keeps that
    // sliver above the bend line, and the bend runs on under it, x 0 … 100. Both used to fail
    // with "sew: a degenerate edge": the first square to the axes, the second turned off them
    // (a 5-12-13 slope) in the middle.
    for turn in [0.0, (5.0f64 / 13.0).asin()] {
        let at = |x: f64, y: f64| [x * turn.cos() - y * turn.sin(), x * turn.sin() + y * turn.cos()];
        for (position, setback) in [("inside", 3.0), ("middle", 3.0 / 2.0)] {
            for (taller, len) in [(0.0, 97.0), (0.05, 100.0)] {
                let what = format!("{position}, ledge {setback} + {taller}, turned {turn:.4}");
                let ledge = setback + taller;
                let mut s = Session::default();
                run(
                    &mut s,
                    "sheet.manage_rules",
                    json!({"name": "R", "thickness": "2 mm", "k_factor": "0.5", "bend_radius": "1 mm", "active": true}),
                );
                run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Outline"}));
                let outline = [(0.0, 0.0), (100.0, 0.0), (100.0, ledge), (97.0, ledge), (97.0, 60.0), (0.0, 60.0)].map(|(x, y)| at(x, y));
                run(&mut s, "sketch.line", json!({"points": outline, "closed": true}));
                run(&mut s, "sketch.finish", json!({}));
                run(&mut s, "sheet.flange", json!({"sketch": "Outline"}));
                let [x, y] = at(50.0, 0.0);
                run(&mut s, "sheet.flange", json!({"edges": [[x, y, 2]], "height": 20, "angle": 90, "radius": 1, "position": position}));
                assert!(no_errors_or_warnings(&s), "{what}: {:?}", s.model.results);
                let f = flat(&mut s);
                let bends = f["bends"].as_array().cloned().unwrap_or_default();
                assert!(bends.len() == 1 && (bends[0]["length"].as_f64().unwrap_or(0.0) - len).abs() < 1e-6, "{what}: {f}");
                assert_eq!(f["outline"].as_array().map(Vec::len), Some(1), "{what}: {f}");
                // The base 97 wide above the bend line (and the sliver), and len × (π + 17) of bend
                // and flange.
                let area = 97.0 * (60.0 - setback) + 3.0 * taller + len * (PI + 17.0);
                assert!((flat_area(&f) - area).abs() < 1e-6, "{what}: {}", flat_area(&f));
                assert!(rel(volume(&mut s), area * 2.0) < 1e-4, "{what}: {} vs {}", volume(&mut s), area * 2.0);
                // The flange's outer face lies 3 − setback outside the edge (y setback − 3 … − 1);
                // it stands over the ledge only where the bend runs on. The base stops at x = 97.
                let here = |x: f64, y: f64, z: f64| {
                    let [x, y] = at(x, y);
                    contains(&s, [x, y, z])
                };
                assert!(here(50.0, setback - 2.0, 10.0) && here(95.0, 10.0, 1.0) && !here(98.5, 10.0, 1.0), "{what}");
                assert_eq!(here(98.5, setback - 2.0, 10.0), taller > 0.0, "{what}");
                let state = s.model.state();
                let body = &state.bodies[0].body;
                assert!(body.validity().is_empty() && body.shell_count() == 1, "{what}");
                for command in ["sheet.unfold", "sheet.refold"] {
                    run(&mut s, command, json!({}));
                    assert!(rel(volume(&mut s), area * 2.0) < 1e-4, "{what}, {command}: {}", volume(&mut s));
                }
            }
        }
    }
}

#[test]
fn an_edge_flange_on_a_flange_runs_along_what_a_flat_cut_left_of_its_far_edge() {
    // A 40 high wall on the plate's bottom edge (outside: BA π, panel 37), its far edge at
    // y = -(π + 37) in the flat. Unfolded, a cut takes x 0 … 60 of that edge, up to y = -30. A
    // 15 high flange picked on the rest of the edge (the wall's inner face is y = -1, its top
    // z = 40) spans x 60 … 200.
    let mut s = Session::default();
    run(&mut s, "sheet.manage_rules", json!({"name": "R", "thickness": "2 mm", "k_factor": "0.5", "bend_radius": "1 mm", "active": true}));
    base(&mut s, 200.0, 100.0);
    run(&mut s, "sheet.flange", json!({"edges": [[100, 0, 2]], "height": 40, "angle": 90, "position": "outside"}));
    run(&mut s, "sheet.unfold", json!({}));
    rectangle_tool(&mut s, "Corner", [-10.0, -50.0], [60.0, -30.0]);
    run(&mut s, "solid.extrude", json!({"sketch": "Corner", "through_all": true, "direction": "symmetric", "operation": "cut"}));
    run(&mut s, "sheet.refold", json!({}));
    run(&mut s, "sheet.flange", json!({"edges": [[130, -1, 40]], "height": 15, "angle": 90, "position": "outside"}));
    assert!(no_errors_or_warnings(&s), "{:?}", s.model.results);
    let f = flat(&mut s);
    let bends = f["bends"].as_array().cloned().unwrap_or_default();
    let spans: Vec<(f64, f64)> =
        bends.iter().map(|b| (b["start"][0].as_f64().unwrap_or(f64::NAN), b["end"][0].as_f64().unwrap_or(f64::NAN))).collect();
    let close = |a: (f64, f64), b: (f64, f64)| (a.0 - b.0).abs() < 1e-9 && (a.1 - b.1).abs() < 1e-9;
    assert!(spans.len() == 2 && close(spans[0], (0.0, 200.0)) && close(spans[1], (60.0, 200.0)), "{f}");
    // The plate, the wall less the cut's 60 × (π + 7), and 140 × (π + 12) of the flange on it.
    let area = 20_000.0 + 200.0 * (PI + 37.0) - 60.0 * (PI + 7.0) + 140.0 * (PI + 12.0);
    assert!((flat_area(&f) - area).abs() < 1e-6, "{}", flat_area(&f));
    assert!(rel(volume(&mut s), area * 2.0) < 1e-4, "{} vs {}", volume(&mut s), area * 2.0);
    // It lies over the plate from the wall (z 41 … 43, y 0 … 12), on x 60 … 200 only.
    assert!(contains(&s, [130.0, 6.0, 42.0]) && !contains(&s, [30.0, 6.0, 42.0]));
}

#[test]
fn a_flange_bent_inside_keeps_the_base_sketchs_arcs_in_the_flat_pattern_and_the_dxf() {
    // A 200 x 100 plate with its corners rounded 8, drawn as arcs, and a 40 high flange on its
    // bottom edge (2 mm, R 1, K 0.5: setback 3 inside, half of it in the middle; BA π, panel
    // 37). With the bend inside or in the middle the base is cut back through the two lower
    // arcs: the DXF's outline was about 150 LINEs then, every arc as chords a quarter
    // millimetre long, where the bend outside gives 8 LINEs and 4 ARCs.
    let h = 8.0 * (1.0 - 0.5f64.sqrt());
    // ∫ √(64 − z²): the area under an arc of radius 8, from its centre's height on.
    let under = |z: f64| z / 2.0 * (64.0 - z * z).sqrt() + 32.0 * (z / 8.0).asin();
    for (position, setback) in [("outside", 0.0f64), ("middle", 3.0 / 2.0), ("inside", 3.0)] {
        let mut s = Session::default();
        run(&mut s, "sheet.manage_rules", json!({"name": "R", "thickness": "2 mm", "k_factor": "0.5", "bend_radius": "1 mm", "active": true}));
        run(&mut s, "sketch.create", json!({"plane": "XY", "name": "S"}));
        run(&mut s, "sketch.line", json!({"points": [[8, 0], [192, 0]]}));
        run(&mut s, "sketch.arc.three_point", json!({"start": [192, 0], "end": [200, 8], "through": [200.0 - h, h]}));
        run(&mut s, "sketch.line", json!({"points": [[200, 8], [200, 92]]}));
        run(&mut s, "sketch.arc.three_point", json!({"start": [200, 92], "end": [192, 100], "through": [200.0 - h, 100.0 - h]}));
        run(&mut s, "sketch.line", json!({"points": [[192, 100], [8, 100]]}));
        run(&mut s, "sketch.arc.three_point", json!({"start": [8, 100], "end": [0, 92], "through": [h, 100.0 - h]}));
        run(&mut s, "sketch.line", json!({"points": [[0, 92], [0, 8]]}));
        run(&mut s, "sketch.arc.three_point", json!({"start": [0, 8], "end": [8, 0], "through": [h, h]}));
        run(&mut s, "sketch.finish", json!({}));
        run(&mut s, "sheet.flange", json!({"sketch": "S"}));
        run(&mut s, "sheet.flange", json!({"edges": [[100, 0, 2]], "height": 40, "angle": 90, "position": position}));
        assert!(no_errors_or_warnings(&s), "{position}: {:?}", s.model.results);
        let text = run(&mut s, "sheet.export_dxf", json!({}))["dxf"].as_str().unwrap_or_default().to_string();
        let shapes = solidcraft_io::read_dxf(&text).unwrap();
        let lines = shapes.iter().filter(|g| matches!(g, solidcraft_io::Geom2::Line(..))).count();
        let radii: Vec<f64> =
            shapes.iter().filter_map(|g| if let solidcraft_io::Geom2::Arc { c, a, .. } = g { Some(c.dist(*a)) } else { None }).collect();
        // Eight lines of outline and the bend's centre line; the four corners.
        assert_eq!((lines, radii.len(), shapes.len()), (9, 4, 13), "{position}");
        assert!(radii.iter().all(|r| (r - 8.0).abs() < 1e-9), "{position}: {radii:?}");
        // The plate less what lies below the bend line, ∫ 184 + 2·√(64 − (8 − y)²) from 0 to the
        // setback, and the flange along the trimmed edge, which runs from arc to arc.
        let below = 184.0 * setback + 2.0 * (under(8.0) - under(8.0 - setback));
        let edge = 184.0 + 2.0 * (64.0 - (8.0 - setback) * (8.0 - setback)).sqrt();
        let want = 20_000.0 - (4.0 - PI) * 64.0 - below + edge * (PI + 37.0);
        let regions = s.model.state().sheets[0].flat_regions().unwrap();
        assert!(regions.len() == 1 && (regions[0].area() - want).abs() < 1e-6, "{position}: {} vs {want}", regions[0].area());
        let f = flat(&mut s);
        assert!((f["bends"][0]["length"].as_f64().unwrap_or(f64::NAN) - edge).abs() < 1e-9, "{position}: {}", f["bends"]);
        // Folded it holds the same (K 0.5), and so it does unfolded and folded again.
        for command in [None, Some("sheet.unfold"), Some("sheet.refold")] {
            if let Some(command) = command {
                run(&mut s, command, json!({}));
            }
            assert!(rel(volume(&mut s), want * 2.0) < 1e-3, "{position} {command:?}: {} vs {}", volume(&mut s), want * 2.0);
        }
    }
}

#[test]
fn bend_table_exports_shop_dimensions_and_tracks_parameter_changes() {
    let mut s = Session::default();
    run(&mut s, "sheet.manage_rules", json!({"name": "Shop, \"steel\"", "thickness": 2, "bend_radius": 3, "k_factor": 0.4}));
    base(&mut s, 100.0, 60.0);
    run(&mut s, "sheet.flange", json!({"edges": [[50, 0, 2]], "height": 20}));
    let undo = s.undo.len();
    let csv = run(&mut s, "sheet.export_bend_table", json!({}));
    let text = csv["text"].as_str().unwrap();
    assert!(text.starts_with("body,rule,thickness_mm,k_factor,bend,angle_deg,direction,"));
    assert!(text.contains("\"Shop, \"\"steel\"\"\""), "{text}");
    assert_eq!(text.lines().count(), 2);
    assert_eq!(csv["bends"], 1);
    let exported = run(&mut s, "sheet.export_bend_table", json!({"format": "JSON"}));
    let table: Value = serde_json::from_str(exported["text"].as_str().unwrap()).unwrap();
    assert_eq!(table["thickness"], 2.0);
    assert_eq!(table["bends"][0]["direction"], "up");
    assert!((table["bends"][0]["allowance"].as_f64().unwrap() - FRAC_PI_2 * 3.8).abs() < 1e-9);
    assert_eq!(s.undo.len(), undo, "export does not create an undo step");
    run(&mut s, "sheet.manage_rules", json!({"name": "Shop, \"steel\"", "k_factor": 0.5}));
    let updated = run(&mut s, "sheet.export_bend_table", json!({"format": "json"}));
    let table: Value = serde_json::from_str(updated["text"].as_str().unwrap()).unwrap();
    assert!((table["bends"][0]["allowance"].as_f64().unwrap() - FRAC_PI_2 * 4.0).abs() < 1e-9);
    assert!(s.execute("sheet.export_bend_table", &json!({"format": "xml"})).is_err());
    assert!(s.execute("sheet.export_bend_table", &json!({"body": "Missing"})).is_err());
    let mut empty = Session::default();
    assert!(empty.execute("sheet.export_bend_table", &json!({})).is_err());
}

#[test]
fn malformed_fold_points_are_rejected_atomically() {
    let mut s = Session::default();
    base(&mut s, 100.0, 60.0);
    let before = serde_json::to_value(&s.doc).unwrap();
    for points in [
        json!([[40, 0, 2.5], null, [40, 60, 2.5]]),
        json!([[40, 0, 2.5], [40, 60, 2.5], "junk"]),
        json!([[40, 0, 2.5], [40, 60]]),
        json!([[40, 0, 2.5]]),
        json!("not points"),
    ] {
        assert!(s.execute("sheet.fold", &json!({"points": points})).is_err());
        assert_eq!(serde_json::to_value(&s.doc).unwrap(), before);
    }
}

#[test]
fn flat_dxf_preserves_exact_cutout_entities() {
    use solidcraft_geom::{Loop2, Region2, Seg2, Vec2};
    let mut s = Session::default();
    base(&mut s, 100.0, 60.0);
    let mut sh = s.model.state().sheets[0].clone();
    sh.holes = vec![Region2 {
        outer: Loop2 { segs: vec![Seg2::Arc { center: Vec2::new(40.0, 30.0), radius: 5.0, start: -PI, sweep: -2.0 * PI }] },
        holes: Vec::new(),
    }];
    let dxf = super::flat_dxf(&sh, &super::DxfOptions::default()).unwrap();
    assert_eq!(dxf.matches("0\nCIRCLE\n").count(), 1, "{dxf}");
    assert!(!dxf.contains("0\nARC\n"), "a full circle must not become a zero-length ARC");
    let read = solidcraft_io::read_dxf(&dxf).unwrap();
    assert_eq!(read.len(), 5);
    assert!(read.iter().any(|g| matches!(g, solidcraft_io::Geom2::Circle(c, r) if *c == Vec2::new(40.0, 30.0) && (*r - 5.0).abs() < 1e-9)));
    sh.holes[0].outer = Loop2 {
        segs: vec![
            Seg2::Line { a: Vec2::new(10.0, 10.0), b: Vec2::new(30.0, 10.0) },
            Seg2::Arc { center: Vec2::new(20.0, 10.0), radius: 10.0, start: 0.0, sweep: PI },
        ],
    };
    let dxf = super::flat_dxf(&sh, &super::DxfOptions::default()).unwrap();
    assert_eq!(dxf.matches("0\nLINE\n").count(), 5, "four outline lines and one exact cutout line");
    assert_eq!(dxf.matches("0\nARC\n").count(), 1);
    assert!(dxf.contains(" 50\n0.0\n 51\n180.0\n"), "{dxf}");
    let read = solidcraft_io::read_dxf(&dxf).unwrap();
    assert_eq!(read.len(), 6);
    assert!(read.iter().any(|g| matches!(g, solidcraft_io::Geom2::Arc { c, a, b } if *c == Vec2::new(20.0, 10.0) && a.dist(Vec2::new(30.0, 10.0)) < 1e-9 && b.dist(Vec2::new(10.0, 10.0)) < 1e-9)));
}

// A profile chosen by a point outside its inner loop must keep that inner loop through
// sheet construction, flattening and manufacturing export.
#[test]
fn inherited_base_profile_holes_survive_every_sheet_state() {
    for flip in [false, true] {
        let mut s = Session::default();
        run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Perforated"}));
        run(&mut s, "sketch.rectangle.two_point", json!({"p0": [0, 0], "p1": [100, 60]}));
        run(&mut s, "sketch.rectangle.two_point", json!({"p0": [40, 20], "p1": [60, 40]}));
        run(&mut s, "sketch.finish", json!({}));
        run(&mut s, "sheet.flange", json!({"sketch": "Perforated", "profiles": [{"point": [10, 10]}], "flip": flip}));
        assert!((volume(&mut s) - 14000.0).abs() < 1e-6);
        let f = flat(&mut s);
        assert_eq!(f["cutouts"].as_array().map(Vec::len), Some(1));
        let dxf = run(&mut s, "sheet.export_dxf", json!({}))["dxf"].as_str().unwrap().to_string();
        assert_eq!(solidcraft_io::read_dxf(&dxf).unwrap().len(), 8);
        let mut reopened = Session::default();
        run(&mut reopened, "sketch.insert_dxf", json!({"text": dxf, "name": "Blank"}));
        run(&mut reopened, "sketch.finish", json!({}));
        let pick = if flip { [10, -10] } else { [10, 10] };
        run(&mut reopened, "solid.extrude", json!({"sketch": "Blank", "profiles": [{"point": pick}], "distance": 2.5}));
        assert!((volume(&mut reopened) - 14000.0).abs() < 1e-5);
        run(&mut s, "sheet.unfold", json!({}));
        assert!((volume(&mut s) - 14000.0).abs() < 1e-6);
        run(&mut s, "sheet.refold", json!({}));
        assert!((volume(&mut s) - 14000.0).abs() < 1e-6);
    }
}

#[test]
fn edge_notch_uses_the_same_material_boundary_in_solid_flat_and_dxf() {
    let mut s = Session::default();
    base(&mut s, 100.0, 60.0);
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Notch"}));
    run(&mut s, "sketch.rectangle.two_point", json!({"p0": [-5, 20], "p1": [10, 40]}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.extrude", json!({"sketch": "Notch", "through_all": true, "operation": "cut", "direction": "symmetric"}));
    assert!((volume(&mut s) - 14500.0).abs() < 1e-6);
    let f = flat(&mut s);
    assert_eq!(f["outline"].as_array().map(Vec::len), Some(1));
    assert_eq!(f["outline"][0].as_array().map(Vec::len), Some(8));
    assert_eq!(f["cutouts"].as_array().map(Vec::len), Some(0));
    let dxf = run(&mut s, "sheet.export_dxf", json!({}))["dxf"].as_str().unwrap().to_string();
    assert_eq!(solidcraft_io::read_dxf(&dxf).unwrap().len(), 8);
    let mut reopened = Session::default();
    run(&mut reopened, "sketch.insert_dxf", json!({"text": dxf, "name": "Blank"}));
    run(&mut reopened, "sketch.finish", json!({}));
    run(&mut reopened, "solid.extrude", json!({"sketch": "Blank", "distance": 2.5}));
    assert!((volume(&mut reopened) - 14500.0).abs() < 1e-5);
    run(&mut s, "sheet.unfold", json!({}));
    assert!((volume(&mut s) - 14500.0).abs() < 1e-6);
    run(&mut s, "sheet.refold", json!({}));
    assert!((volume(&mut s) - 14500.0).abs() < 1e-6);
}

#[test]
fn inherited_round_hole_survives_flange_and_unfold_in_flipped_frames() {
    for flip in [false, true] {
        let mut s = Session::default();
        run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Perforated"}));
        run(&mut s, "sketch.rectangle.two_point", json!({"p0": [0, 0], "p1": [100, 60]}));
        run(&mut s, "sketch.circle.center", json!({"center": [50, 30], "radius": 5}));
        run(&mut s, "sketch.finish", json!({}));
        run(&mut s, "sheet.flange", json!({"sketch": "Perforated", "profiles": [{"point": [10, 10]}], "flip": flip}));
        run(&mut s, "sheet.flange", json!({"edges": [[50, 0, if flip { -2.5 } else { 2.5 }]], "height": 20}));
        let folded = volume(&mut s);
        assert!((folded - (18972.62196081845 - PI * 25.0 * 2.5)).abs() < 0.2, "{folded}");
        let f = flat(&mut s);
        assert_eq!(f["cutouts"].as_array().map(Vec::len), Some(1));
        let sh = s.model.state().sheets[0].clone();
        let regions = sh.flat_regions().unwrap();
        assert_eq!(regions[0].holes[0].segs.len(), 2);
        assert!(regions[0].holes[0].segs.iter().all(|s| matches!(s, solidcraft_geom::Seg2::Arc { radius, .. } if (*radius - 5.0).abs() < 1e-9)));
        let expected = regions.iter().map(solidcraft_geom::Region2::area).sum::<f64>() * 2.5;
        run(&mut s, "sheet.unfold", json!({}));
        assert!((volume(&mut s) - expected).abs() < 0.1);
        run(&mut s, "sheet.refold", json!({}));
        assert!((volume(&mut s) - folded).abs() < 0.1);
    }
}

fn faces(s: &mut Session) -> (u64, u64) {
    let body = run(s, "inspect.measure", json!({}))["bodies"][0].clone();
    (body["faces"].as_u64().unwrap(), body["face_types"]["cylinder"].as_u64().unwrap_or(0))
}

#[test]
fn elliptic_cut_out_stays_exact_in_the_flat_pattern_and_follows_arcs_when_folded() {
    use solidcraft_geom::Seg2;
    let mut s = Session::default();
    base(&mut s, 100.0, 60.0);
    run(&mut s, "sheet.flange", json!({"edges": [[50, 0, 2.5]], "height": 20}));
    let (uncut, blank) = (volume(&mut s), s.model.state().sheets[0].flat_regions().unwrap()[0].area());
    let (plain_faces, plain_cylinders) = faces(&mut s);
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Cut"}));
    run(&mut s, "sketch.ellipse", json!({"center": [40, 30], "major": [55, 30], "minor_radius": 8}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.extrude", json!({"sketch": "Cut", "through_all": true, "operation": "cut", "direction": "symmetric"}));
    // The ellipse takes π·15·8 of the 2.5 mm sheet (a body's volume is measured on its mesh).
    let cut = PI * 15.0 * 8.0;
    let folded = volume(&mut s);
    assert!((uncut - folded - cut * 2.5).abs() < 0.5, "{}", uncut - folded);
    // The flat pattern holds the ellipse itself, so its area is the blank less π·15·8.
    let regions = s.model.state().sheets[0].flat_regions().unwrap();
    let hole = &regions[0].holes[0];
    assert!(hole.segs.len() == 4 && hole.segs.iter().all(|x| matches!(x, Seg2::Conic { .. })), "{} segments", hole.segs.len());
    assert!((regions[0].area() - (blank - cut)).abs() < 1e-9, "{}", regions[0].area());
    // The flat pattern JSON lists it as those four curves, one `polyline` each, with every
    // point on the ellipse ((x − 40)/15)² + ((y − 30)/8)² = 1.
    let shapes = flat(&mut s)["cutout_shapes"].as_array().cloned().unwrap_or_default();
    let curves: Vec<Vec<Value>> = shapes[0]["segments"].as_array().into_iter().flatten().filter_map(|x| x["polyline"].as_array().cloned()).collect();
    assert!(shapes.len() == 1 && curves.len() == 4 && shapes[0]["segments"].as_array().is_some_and(|a| a.len() == 4), "{} curves", curves.len());
    let off = |p: &Value| ((p[0].as_f64().unwrap_or(0.0) - 40.0) / 15.0).powi(2) + ((p[1].as_f64().unwrap_or(0.0) - 30.0) / 8.0).powi(2) - 1.0;
    assert!(curves.iter().all(|c| c.len() > 2 && c.iter().all(|p| off(p).abs() < 1e-9)));
    // Folded, the cut-out's wall is cylinders over tangent arcs: fewer than half the 264
    // chords of 0.001 mm (four quarters of √(2·17/0.008) = 66 each).
    let (cut_faces, cut_cylinders) = faces(&mut s);
    let wall = cut_faces - plain_faces;
    assert!(wall < 132 && cut_cylinders - plain_cylinders + 4 >= wall, "{wall} wall faces, {} cylinders", cut_cylinders - plain_cylinders);
    // Unfolded, it is the four conic faces of the ellipse.
    run(&mut s, "sheet.unfold", json!({}));
    assert!((volume(&mut s) - (blank - cut) * 2.5).abs() < 0.5);
    assert_eq!(faces(&mut s).0, regions[0].outer.segs.len() as u64 + 4 + 2);
    run(&mut s, "sheet.refold", json!({}));
    assert!((volume(&mut s) - folded).abs() < 0.1);
}

#[test]
fn flipped_base_flange_keeps_its_elliptic_outline() {
    for flip in [false, true] {
        let mut s = Session::default();
        run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Oval"}));
        run(&mut s, "sketch.ellipse", json!({"center": [0, 0], "major": [25, 0], "minor_radius": 12}));
        run(&mut s, "sketch.finish", json!({}));
        run(&mut s, "sheet.flange", json!({"sketch": "Oval", "flip": flip}));
        let regions = s.model.state().sheets[0].flat_regions().unwrap();
        let outline = &regions[0].outer.segs;
        assert!(
            outline.len() == 4 && outline.iter().all(|x| matches!(x, solidcraft_geom::Seg2::Conic { .. })),
            "flip {flip}: {} segments",
            outline.len()
        );
        // π·25·12 of the 2.5 mm sheet: exactly in the flat pattern; within 0.5 mm³ in the body,
        // which follows the outline by arcs and is measured on its mesh (a 64-sided outline
        // would be 3.8 mm³ short).
        assert!((regions[0].area() - PI * 300.0).abs() < 1e-9, "flip {flip}: {}", regions[0].area());
        assert!((volume(&mut s) - PI * 300.0 * 2.5).abs() < 0.5, "flip {flip}: {}", volume(&mut s));
    }
}

#[test]
fn crossing_only_disconnecting_cut_preserves_all_material_states() {
    let mut s = Session::default();
    base(&mut s, 100.0, 60.0);
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Split"}));
    run(&mut s, "sketch.rectangle.two_point", json!({"p0": [45, -5], "p1": [55, 65]}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.extrude", json!({"sketch": "Split", "through_all": true, "operation": "cut", "direction": "symmetric"}));
    assert!((volume(&mut s) - 13500.0).abs() < 1e-6);
    assert_eq!(flat(&mut s)["outline"].as_array().map(Vec::len), Some(2));
    let dxf = run(&mut s, "sheet.export_dxf", json!({}))["dxf"].as_str().unwrap().to_string();
    let mut reopened = Session::default();
    run(&mut reopened, "sketch.insert_dxf", json!({"text": dxf, "name": "SplitBlank"}));
    run(&mut reopened, "sketch.finish", json!({}));
    run(&mut reopened, "solid.extrude", json!({"sketch": "SplitBlank", "distance": 2.5}));
    assert!((volume(&mut reopened) - 13500.0).abs() < 1e-6);
    run(&mut s, "sheet.unfold", json!({}));
    assert!((volume(&mut s) - 13500.0).abs() < 1e-6);
    run(&mut s, "sheet.refold", json!({}));
    assert!((volume(&mut s) - 13500.0).abs() < 1e-6);
}

#[test]
fn splitting_an_unfolded_flanged_sheet_preserves_both_bent_pieces() {
    let mut s = Session::default();
    base(&mut s, 100.0, 60.0);
    run(&mut s, "sheet.flange", json!({"edges": [[50,0,2.5]], "height": 20}));
    let folded = volume(&mut s);
    run(&mut s, "sheet.unfold", json!({}));
    let unfolded = volume(&mut s);
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "BlankSplit"}));
    run(&mut s, "sketch.rectangle.two_point", json!({"p0": [45,-100], "p1": [55,100]}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.extrude", json!({"sketch": "BlankSplit", "through_all": true, "operation": "cut", "direction": "symmetric"}));
    assert!((volume(&mut s) - unfolded * 0.9).abs() < 0.1);
    assert_eq!(flat(&mut s)["outline"].as_array().map(Vec::len), Some(2));
    run(&mut s, "sheet.refold", json!({}));
    assert!((volume(&mut s) - folded * 0.9).abs() < 0.2);
    run(&mut s, "sheet.unfold", json!({}));
    assert!((volume(&mut s) - unfolded * 0.9).abs() < 0.1);
}

#[test]
fn repeated_and_nested_sheet_cuts_are_redundant() {
    let mut s = Session::default();
    base(&mut s, 100.0, 60.0);
    for (i, radius) in [3.0, 5.0, 5.0].into_iter().enumerate() {
        let name = format!("Hole{i}");
        run(&mut s, "sketch.create", json!({"plane": "XY", "name": name}));
        run(&mut s, "sketch.circle.center", json!({"center": [20, 30], "radius": radius}));
        run(&mut s, "sketch.finish", json!({}));
        run(&mut s, "solid.extrude", json!({"sketch": name, "through_all": true, "operation": "cut", "direction": "symmetric"}));
    }
    assert!((volume(&mut s) - (6000.0 - PI * 25.0) * 2.5).abs() < 0.1);
    assert_eq!(flat(&mut s)["cutouts"].as_array().map(Vec::len), Some(1));
}

#[test]
fn overlapping_round_cuts_keep_exact_union_in_export_and_roundtrip() {
    let mut s = Session::default();
    base(&mut s, 100.0, 60.0);
    for (i, x) in [20.0, 22.0].into_iter().enumerate() {
        let name = format!("Overlap{i}");
        run(&mut s, "sketch.create", json!({"plane": "XY", "name": name}));
        run(&mut s, "sketch.circle.center", json!({"center": [x, 30], "radius": 5}));
        run(&mut s, "sketch.finish", json!({}));
        run(&mut s, "solid.extrude", json!({"sketch": name, "through_all": true, "operation": "cut", "direction": "symmetric"}));
    }
    let intersection = 50.0 * (0.2f64).acos() - (96.0f64).sqrt();
    let expected = (6000.0 - (50.0 * PI - intersection)) * 2.5;
    assert!((volume(&mut s) - expected).abs() < 0.1);
    assert_eq!(flat(&mut s)["cutouts"].as_array().map(Vec::len), Some(1));
    let dxf = run(&mut s, "sheet.export_dxf", json!({}))["dxf"].as_str().unwrap().to_string();
    assert!(dxf.contains("0\nARC\n"));
    let mut reopened = Session::default();
    run(&mut reopened, "sketch.insert_dxf", json!({"text": dxf, "name": "Blank"}));
    run(&mut reopened, "sketch.finish", json!({}));
    run(&mut reopened, "solid.extrude", json!({"sketch": "Blank", "profiles": [{"point": [10, 10]}], "distance": 2.5}));
    assert!((volume(&mut reopened) - expected).abs() < 0.1);
    run(&mut s, "sheet.unfold", json!({}));
    assert!((volume(&mut s) - expected).abs() < 0.1);
    run(&mut s, "sheet.refold", json!({}));
    assert!((volume(&mut s) - expected).abs() < 0.1);
}

#[test]
fn cut_across_a_concave_notch_uses_crossings_not_just_contained_corners() {
    let mut s = Session::default();
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "U"}));
    run(&mut s, "sketch.line", json!({"points": [[0,0],[100,0],[100,60],[70,60],[70,20],[30,20],[30,60],[0,60],[0,0]]}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "sheet.flange", json!({"sketch": "U"}));
    assert!((volume(&mut s) - 11000.0).abs() < 1e-6);
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "ArmNotches"}));
    run(&mut s, "sketch.rectangle.two_point", json!({"p0": [20,30], "p1": [80,40]}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.extrude", json!({"sketch": "ArmNotches", "through_all": true, "operation": "cut", "direction": "symmetric"}));
    assert!((volume(&mut s) - 10500.0).abs() < 1e-6);
    let f = flat(&mut s);
    assert_eq!(f["cutouts"].as_array().map(Vec::len), Some(0));
    run(&mut s, "sheet.unfold", json!({}));
    assert!((volume(&mut s) - 10500.0).abs() < 1e-6);
}

#[test]
fn tiny_cut_inside_exact_circle_survives_unfold_refold_and_dxf() {
    for initially_flat in [false, true] {
        let mut s = Session::default();
        run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Disc"}));
        run(&mut s, "sketch.circle.center", json!({"center": [0,0], "radius": 10}));
        run(&mut s, "sketch.finish", json!({}));
        run(&mut s, "sheet.flange", json!({"sketch": "Disc"}));
        let circle = s.model.state().sheets[0].base.outer.clone();
        let displayed = circle.polyline(1e-3);
        let at = (displayed[0] + displayed[1]).normalized().unwrap() * 9.9998;
        assert!(!circle.contains(at));
        if initially_flat {
            run(&mut s, "sheet.unfold", json!({}));
        }
        run(&mut s, "sketch.create", json!({"plane": "XY", "name": "TinyCut"}));
        run(&mut s, "sketch.rectangle.two_point", json!({"p0": [at.x-0.000025,at.y-0.000025], "p1": [at.x+0.000025,at.y+0.000025]}));
        run(&mut s, "sketch.finish", json!({}));
        run(&mut s, "solid.extrude", json!({"sketch": "TinyCut", "through_all": true, "operation": "cut", "direction": "symmetric"}));
        for stage in ["sheet.unfold", "sheet.refold"] {
            let regions = s.model.state().sheets[0].flat_regions().unwrap();
            assert_eq!(regions[0].holes.len(), 1);
            assert!((regions[0].holes[0].signed_area().abs() - 2.5e-9).abs() < 1e-13);
            assert_eq!(s.model.state().bodies.len(), 1);
            let dxf = run(&mut s, "sheet.export_dxf", json!({}))["dxf"].as_str().unwrap().to_string();
            assert_eq!(solidcraft_io::read_dxf(&dxf).unwrap().len(), 5);
            if stage == "sheet.unfold" && initially_flat {
                run(&mut s, "sheet.refold", json!({}));
            }
            run(&mut s, stage, json!({}));
        }
        assert_eq!(s.model.state().sheets[0].flat_regions().unwrap()[0].holes.len(), 1);
    }
}

#[test]
fn unrelated_box_cut_preserves_logical_ring_sheet_compound() {
    let mut s = Session::default();
    base(&mut s, 100.0, 60.0);
    let sheet_name = s.model.state().sheets[0].body.clone();
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Ring"}));
    for radius in [10, 5] {
        run(&mut s, "sketch.circle.center", json!({"center": [50,30], "radius": radius}));
    }
    run(&mut s, "sketch.finish", json!({}));
    run(
        &mut s,
        "solid.extrude",
        json!({"sketch": "Ring", "profiles": [{"point": [57,30]}], "through_all": true, "operation": "cut", "direction": "symmetric"}),
    );
    let material = s.model.state().sheets[0].flat_regions().unwrap();
    run(&mut s, "solid.box", json!({"length": 20, "width": 20, "height": 10, "corner": [200,0,0], "body_name": "SeparateBox"}));
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "BoxCut"}));
    run(&mut s, "sketch.rectangle.two_point", json!({"p0": [205,5], "p1": [210,10]}));
    run(&mut s, "sketch.finish", json!({}));
    run(
        &mut s,
        "solid.extrude",
        json!({"sketch": "BoxCut", "participants": ["SeparateBox"], "through_all": true, "operation": "cut", "direction": "symmetric"}),
    );
    assert_eq!(s.model.state().bodies.len(), 2);
    assert_eq!(s.model.state().sheets[0].flat_regions().unwrap(), material);
    for stage in ["sheet.unfold", "sheet.refold"] {
        run(&mut s, stage, json!({"body": sheet_name}));
        assert_eq!(s.model.state().bodies.len(), 2);
        let state = s.model.state();
        let body = &state.bodies.iter().find(|b| b.name == sheet_name).unwrap().body;
        assert!(body.contains(solidcraft_geom::Vec3::new(50.0, 30.0, 1.25)));
        assert!(!body.contains(solidcraft_geom::Vec3::new(57.0, 30.0, 1.25)));
    }
}

#[test]
fn specialized_sheet_cuts_respect_component_and_explicit_participants() {
    for explicit in [false, true] {
        let mut s = Session::default();
        base(&mut s, 100.0, 60.0);
        let name = s.model.state().sheets[0].body.clone();
        run(&mut s, "component.create", json!({"name": "Other"}));
        run(&mut s, "solid.box", json!({"length": 100, "width": 60, "height": 10, "body_name": "OtherBox"}));
        if explicit {
            run(&mut s, "component.activate", json!({"component": "root"}));
        }
        run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Cut"}));
        run(&mut s, "sketch.rectangle.two_point", json!({"p0": [20,20], "p1": [30,30]}));
        run(&mut s, "sketch.finish", json!({}));
        let mut params = json!({"sketch": "Cut", "through_all": true, "operation": "cut", "direction": "symmetric"});
        if explicit {
            params["participants"] = json!(["OtherBox"]);
        }
        run(&mut s, "solid.extrude", params);
        let m = run(&mut s, "inspect.measure", json!({"bodies": [name]}));
        assert!((m["total"]["volume_mm3"].as_f64().unwrap() - 15000.0).abs() < 1e-6);
        assert!(s.model.state().sheets[0].holes.is_empty());
        let m = run(&mut s, "inspect.measure", json!({"bodies": ["OtherBox"]}));
        assert!((m["total"]["volume_mm3"].as_f64().unwrap() - 59000.0).abs() < 1e-6);
    }
}

#[test]
fn ring_cut_preserves_retained_island_through_unfold_and_refold() {
    fn material_probes(s: &Session) {
        let body = &s.model.state().bodies[0].body;
        assert!(body.contains(solidcraft_geom::Vec3::new(50.0, 30.0, 1.25)));
        assert!(!body.contains(solidcraft_geom::Vec3::new(57.0, 30.0, 1.25)));
        assert!(body.contains(solidcraft_geom::Vec3::new(65.0, 30.0, 1.25)));
    }
    let mut s = Session::default();
    base(&mut s, 100.0, 60.0);
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Ring"}));
    run(&mut s, "sketch.circle.center", json!({"center": [50,30], "radius": 10}));
    run(&mut s, "sketch.circle.center", json!({"center": [50,30], "radius": 5}));
    run(&mut s, "sketch.finish", json!({}));
    run(
        &mut s,
        "solid.extrude",
        json!({"sketch": "Ring", "profiles": [{"point": [57,30]}], "through_all": true, "operation": "cut", "direction": "symmetric"}),
    );
    let expected = (6000.0 - PI * 75.0) * 2.5;
    assert!((volume(&mut s) - expected).abs() < 0.2);
    material_probes(&s);
    assert_eq!(flat(&mut s)["outline"].as_array().map(Vec::len), Some(2));
    let regions = s.model.state().sheets[0].flat_regions().unwrap();
    assert!(regions.iter().any(|r| r.contains(solidcraft_geom::Vec2::new(50.0, 30.0))));
    assert!(!regions.iter().any(|r| r.contains(solidcraft_geom::Vec2::new(57.0, 30.0))));
    assert!(regions.iter().any(|r| r.contains(solidcraft_geom::Vec2::new(65.0, 30.0))));
    let dxf = run(&mut s, "sheet.export_dxf", json!({}))["dxf"].as_str().unwrap().to_string();
    let mut reopened = Session::default();
    run(&mut reopened, "sketch.insert_dxf", json!({"text": dxf, "name": "RingBlank"}));
    run(&mut reopened, "sketch.finish", json!({}));
    run(&mut reopened, "solid.extrude", json!({"sketch": "RingBlank", "profiles": [{"point": [10,10]}, {"point": [50,30]}], "distance": 2.5}));
    assert!((volume(&mut reopened) - expected).abs() < 0.2);
    run(&mut s, "sheet.unfold", json!({}));
    assert!((volume(&mut s) - expected).abs() < 0.2);
    material_probes(&s);
    run(&mut s, "sheet.refold", json!({}));
    assert!((volume(&mut s) - expected).abs() < 0.2);
    material_probes(&s);
}

fn retained_depth_check(s: &mut Session, want: f64, z0: f64, z1: f64) {
    use solidcraft_geom::Vec3;
    assert!((volume(s) - want).abs() < 1e-6, "{} vs {want}", volume(s));
    let state = s.model.state();
    assert_eq!(state.sheets.len(), 1);
    assert_eq!(state.sheets[0].t, 2.5, "rule stock gauge is preserved");
    assert!(state.bodies.iter().any(|b| b.body.contains(Vec3::new(25.0, 25.0, (z0 + z1) * 0.5))));
    assert!(!state.bodies.iter().any(|b| b.body.contains(Vec3::new(25.0, 25.0, z0 - 0.1))));
    assert!(!state.bodies.iter().any(|b| b.body.contains(Vec3::new(25.0, 25.0, z1 + 0.1))));
    assert!(!state.bodies.iter().any(|b| b.body.contains(Vec3::new(10.0, 25.0, (z0 + z1) * 0.5))));
    let bounds = state.bodies[0].mesh().bounds();
    assert!((bounds.min.z - z0).abs() < 1e-8 && (bounds.max.z - z1).abs() < 1e-8, "{bounds:?}");
}

fn depth_step_reopen(s: &mut Session, suffix: &str, want: f64, inside: solidcraft_geom::Vec3, outside: solidcraft_geom::Vec3) {
    let path = std::env::temp_dir().join(format!("solidcraft-depth-intersect-{suffix}-{}.step", std::process::id()));
    run(s, "file.export", json!({"path":path.to_string_lossy()}));
    let mut imported = Session::default();
    run(&mut imported, "doc.open", json!({"path":path.to_string_lossy()}));
    assert!((volume(&mut imported) - want).abs() < 1e-6);
    assert!(imported.model.state().bodies.iter().any(|b| b.body.contains(inside)));
    assert!(!imported.model.state().bodies.iter().any(|b| b.body.contains(outside)));
    std::fs::remove_file(path).unwrap();
}

/// The generic sketch importer intentionally drops DXF layers/depth. Reimport only the
/// material outline at a separately known depth, never pretend all entities reconstruct3D.
fn dxf_outline_only(text: &str) -> String {
    let lines: Vec<_> = text.lines().collect();
    let pairs: Vec<_> = lines.as_chunks::<2>().0.iter().filter_map(|p| Some((p.first()?.trim(), *p.get(1)?))).collect();
    let mut out = String::from("0\nSECTION\n2\nENTITIES\n");
    let mut start = 0;
    for end in 1..=pairs.len() {
        if end == pairs.len() || pairs.get(end).is_some_and(|p| p.0 == "0") {
            let entity = &pairs[start..end];
            if entity.first().is_some_and(|p| matches!(p.1, "LINE" | "ARC" | "CIRCLE"))
                && entity.iter().any(|p| p.0 == "8" && matches!(p.1, "OUTLINE" | "CUT"))
            {
                for (code, value) in entity {
                    out.push_str(&format!("{code}\n{value}\n"));
                }
            }
            start = end;
        }
    }
    out.push_str("0\nENDSEC\n0\nEOF\n");
    out
}

#[test]
fn partial_depth_intersect_retains_bottom_top_and_middle_material_through_history_and_exports() {
    use solidcraft_geom::Vec3;
    for (offset, z0, z1, layers) in [
        (0.0, 0.0, 1.0, vec![("top", 1.0, 2.5)]),
        (1.5, 1.5, 2.5, vec![("bottom", 0.0, 1.5)]),
        (0.5, 0.5, 1.5, vec![("bottom", 0.0, 0.5), ("top", 1.5, 2.5)]),
    ] {
        let mut s = Session::default();
        base(&mut s, 100.0, 60.0);
        rectangle_tool(&mut s, "RetainDepth", [20.0, 20.0], [30.0, 30.0]);
        run(&mut s, "solid.extrude", json!({"sketch":"RetainDepth","distance":1,"start_offset":offset,"operation":"intersect","name":"RetainSlab"}));
        retained_depth_check(&mut s, 100.0, z0, z1);
        let exported = flat(&mut s);
        assert_eq!(exported["thickness"], 2.5);
        assert_eq!(exported["pockets"].as_array().unwrap().len(), layers.len());
        let dxf = run(&mut s, "sheet.export_dxf", json!({}))["dxf"].as_str().unwrap().to_string();
        for (side, a, b) in &layers {
            assert!(
                exported["pockets"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|p| p["side"] == *side && p["z0_mm"] == *a && p["z1_mm"] == *b && p["depth_mm"] == b - a)
            );
            assert!(dxf.contains(&format!("POCKET_{}_Z{a:.6}_TO_{b:.6}_MM", side.to_uppercase())));
        }
        let outline = dxf_outline_only(&dxf);
        assert_eq!(solidcraft_io::read_dxf(&outline).unwrap().len(), 4);
        let mut outline_import = Session::default();
        run(&mut outline_import, "sketch.insert_dxf", json!({"text":outline,"name":"Outline"}));
        run(&mut outline_import, "sketch.finish", json!({}));
        run(&mut outline_import, "solid.extrude", json!({"distance":1}));
        assert!((volume(&mut outline_import) - 100.0).abs() < 1e-6);
        run(&mut s, "edit.undo", json!({}));
        assert!((volume(&mut s) - 15000.0).abs() < 1e-6);
        run(&mut s, "edit.redo", json!({}));
        for command in ["sheet.unfold", "sheet.refold"] {
            run(&mut s, command, json!({}));
            retained_depth_check(&mut s, 100.0, z0, z1);
            assert_eq!(flat(&mut s)["pockets"], exported["pockets"]);
        }
        let mut reopened = Session::new(solidcraft_doc::Document::from_json(&s.doc.to_json()).unwrap());
        retained_depth_check(&mut reopened, 100.0, z0, z1);
        depth_step_reopen(&mut s, &format!("offset{offset}"), 100.0, Vec3::new(25.0, 25.0, (z0 + z1) * 0.5), Vec3::new(25.0, 25.0, z1 + 0.1));
    }
}

#[test]
fn partial_depth_intersect_preserves_prior_pockets_and_actual_interval_intersections() {
    use solidcraft_geom::Vec3;
    let mut s = Session::default();
    base(&mut s, 100.0, 60.0);
    rectangle_tool(&mut s, "OldPocket", [20.0, 20.0], [25.0, 30.0]);
    run(&mut s, "solid.extrude", json!({"sketch":"OldPocket","distance":1,"operation":"cut"}));
    rectangle_tool(&mut s, "RetainDepth", [20.0, 20.0], [30.0, 30.0]);
    run(&mut s, "solid.extrude", json!({"sketch":"RetainDepth","distance":1,"start_offset":0.5,"operation":"intersect"}));
    for command in [None, Some("sheet.unfold"), Some("sheet.refold")] {
        if let Some(command) = command {
            run(&mut s, command, json!({}));
        }
        assert!((volume(&mut s) - 75.0).abs() < 1e-6);
        let state = s.model.state();
        let body = &state.bodies[0].body;
        assert!(!body.contains(Vec3::new(22.0, 25.0, 0.75)));
        assert!(body.contains(Vec3::new(22.0, 25.0, 1.25)));
        assert!(body.contains(Vec3::new(27.0, 25.0, 0.75)));
        assert!(!body.contains(Vec3::new(27.0, 25.0, 0.25)));
    }
    let records = s.model.state().sheets[0].pockets.clone();
    run(&mut s, "solid.extrude", json!({"sketch":"RetainDepth","distance":1.5,"start_offset":0.25,"operation":"intersect"}));
    assert!((volume(&mut s) - 75.0).abs() < 1e-6);
    assert_eq!(s.model.state().sheets[0].pockets, records, "no-change leaves replay work unchanged");
    run(&mut s, "solid.extrude", json!({"sketch":"RetainDepth","distance":0.25,"start_offset":1.0,"operation":"intersect"}));
    assert!((volume(&mut s) - 25.0).abs() < 1e-6);
    run(&mut s, "sheet.unfold", json!({}));
    assert!((volume(&mut s) - 25.0).abs() < 1e-6);
    let mut cold = Session::new(solidcraft_doc::Document::from_json(&s.doc.to_json()).unwrap());
    assert!((volume(&mut cold) - 25.0).abs() < 1e-6);
    depth_step_reopen(&mut s, "prior", 25.0, Vec3::new(22.0, 25.0, 1.125), Vec3::new(22.0, 25.0, 0.75));
}

#[test]
fn partial_depth_intersect_keeps_disconnected_tool_islands_and_annular_curves() {
    use solidcraft_geom::Vec3;
    let mut s = Session::default();
    base(&mut s, 100.0, 60.0);
    rectangle_tool(&mut s, "Islands", [20.0, 20.0], [30.0, 30.0]);
    run(&mut s, "sketch.edit", json!({"sketch":"Islands"}));
    run(&mut s, "sketch.rectangle.two_point", json!({"p0":[60,20],"p1":[70,30]}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.extrude", json!({"sketch":"Islands","distance":1,"operation":"intersect"}));
    for command in [None, Some("sheet.unfold"), Some("sheet.refold")] {
        if let Some(command) = command {
            run(&mut s, command, json!({}));
        }
        assert!((volume(&mut s) - 200.0).abs() < 1e-6);
        assert_eq!(s.model.state().sheets[0].flat_regions().unwrap().len(), 2);
        for x in [25.0, 65.0] {
            assert!(s.model.state().bodies[0].body.contains(Vec3::new(x, 25.0, 0.5)));
        }
        assert!(!s.model.state().bodies[0].body.contains(Vec3::new(45.0, 25.0, 0.5)));
    }
    depth_step_reopen(&mut s, "islands", 200.0, Vec3::new(65.0, 25.0, 0.5), Vec3::new(45.0, 25.0, 0.5));
    let mut ring = Session::default();
    base(&mut ring, 100.0, 60.0);
    run(&mut ring, "sketch.create", json!({"plane":"XY","name":"Ring"}));
    for radius in [10.0, 5.0] {
        run(&mut ring, "sketch.circle.center", json!({"center":[50,30],"radius":radius}));
    }
    run(&mut ring, "sketch.finish", json!({}));
    run(&mut ring, "solid.extrude", json!({"sketch":"Ring","profiles":[{"point":[57,30]}],"distance":1,"operation":"intersect"}));
    for command in [None, Some("sheet.unfold"), Some("sheet.refold")] {
        if let Some(command) = command {
            run(&mut ring, command, json!({}));
        }
        let state = ring.model.state();
        let area: f64 = state.sheets[0].flat_regions().unwrap().iter().map(|r| r.area()).sum();
        assert!((area - 75.0 * PI).abs() < 1e-8);
        assert!((volume(&mut ring) - 75.0 * PI).abs() < 0.1);
        assert!(state.bodies[0].body.contains(Vec3::new(57.0, 30.0, 0.5)));
        assert!(!state.bodies[0].body.contains(Vec3::new(50.0, 30.0, 0.5)));
        assert!(!state.bodies[0].body.contains(Vec3::new(57.0, 30.0, 1.5)));
    }
    let dxf = run(&mut ring, "sheet.export_dxf", json!({}))["dxf"].as_str().unwrap().to_string();
    assert!(dxf.contains("POCKET_TOP_Z1.000000_TO_2.500000_MM"));
    assert_eq!(solidcraft_io::read_dxf(&dxf_outline_only(&dxf)).unwrap().len(), 2);
}

#[test]
fn partial_depth_intersect_deletes_material_already_removed_by_a_pocket_atomically() {
    let mut s = Session::default();
    base(&mut s, 100.0, 60.0);
    rectangle_tool(&mut s, "Removed", [20.0, 20.0], [30.0, 30.0]);
    run(&mut s, "solid.extrude", json!({"sketch":"Removed","distance":1,"operation":"cut"}));
    run(&mut s, "solid.extrude", json!({"sketch":"Removed","distance":1,"operation":"intersect"}));
    assert!(s.model.state().bodies.is_empty());
    assert!(s.model.state().sheets.is_empty());
    run(&mut s, "edit.undo", json!({}));
    assert!((volume(&mut s) - 14900.0).abs() < 1e-6);
    run(&mut s, "edit.redo", json!({}));
    assert!(s.model.state().bodies.is_empty());
}

#[test]
fn partial_depth_intersect_flipped_tool_and_transferred_sheet_keep_depth_side_and_world_pose() {
    use solidcraft_geom::Vec3;
    let mut flipped = Session::default();
    base(&mut flipped, 100.0, 60.0);
    run(&mut flipped, "sketch.create", json!({"name":"FlippedTool","plane":{"origin":[0,0,2.5],"x_dir":[1,0,0],"y_dir":[0,-1,0]}}));
    run(&mut flipped, "sketch.rectangle.two_point", json!({"p0":[20,-30],"p1":[30,-20]}));
    run(&mut flipped, "sketch.finish", json!({}));
    run(&mut flipped, "solid.extrude", json!({"sketch":"FlippedTool","distance":1,"operation":"intersect"}));
    retained_depth_check(&mut flipped, 100.0, 1.5, 2.5);
    assert_eq!(flat(&mut flipped)["pockets"][0]["side"], "bottom");
    assert_eq!(flat(&mut flipped)["pockets"][0]["depth_mm"], 1.5);
    let mut s = Session::default();
    base(&mut s, 100.0, 60.0);
    rectangle_tool(&mut s, "RetainDepth", [20.0, 20.0], [30.0, 30.0]);
    run(&mut s, "solid.extrude", json!({"sketch":"RetainDepth","distance":1,"start_offset":0.5,"operation":"intersect","name":"RetainSlab"}));
    let before = s.world_state().bodies[0].mesh().bounds();
    let name = s.model.state().sheets[0].body.clone();
    let destination = run(&mut s, "component.create", json!({"name":"Destination","parent":0}));
    let owner = destination["component"].as_u64().unwrap();
    let occurrence = destination["occurrence"].as_u64().unwrap();
    run(&mut s, "occurrence.move", json!({"occurrence":occurrence,"translate":[30,20,10],"axis":[1,0,0],"angle":"90 deg","capture":true}));
    run(&mut s, "component.move_bodies", json!({"component":owner,"bodies":[name]}));
    for command in [None, Some("sheet.unfold"), Some("sheet.refold")] {
        if let Some(command) = command {
            run(&mut s, command, json!({}));
        }
        let state = s.model.state();
        let sh = &state.sheets[0];
        assert_eq!(sh.t, 2.5);
        assert!(sh.frame.origin.dist(Vec3::new(-30.0, -10.0, 20.0)) < 1e-8);
        assert!(sh.frame.normal().dist(Vec3::Y) < 1e-8);
        assert_eq!(sh.pockets.len(), 2);
        assert_eq!(state.body(&name).unwrap().component, owner);
        assert!(state.body(&name).unwrap().transferred);
        let world = s.world_state();
        let body = &world.body(&name).unwrap().body;
        assert!(body.contains(Vec3::new(25.0, 25.0, 1.0)));
        assert!(!body.contains(Vec3::new(25.0, 25.0, 0.25)));
        assert!(!body.contains(Vec3::new(25.0, 25.0, 2.0)));
        let bounds = world.body(&name).unwrap().mesh().bounds();
        assert!(before.min.dist(bounds.min) < 1e-8 && before.max.dist(bounds.max) < 1e-8);
        assert!((solidcraft_kernel::measure(body).unwrap().volume - 100.0).abs() < 1e-6);
    }
    let mut reopened = Session::new(solidcraft_doc::Document::from_json(&s.doc.to_json()).unwrap());
    let reopened_world = reopened.world_state();
    let body = &reopened_world.body(&name).unwrap().body;
    assert!(body.contains(Vec3::new(25.0, 25.0, 1.0)));
    assert!((solidcraft_kernel::measure(body).unwrap().volume - 100.0).abs() < 1e-6);
    run(&mut reopened, "timeline.edit", json!({"feature":"RetainSlab","set":{"extent":{"distance":"1.5"}}}));
    let world = reopened.world_state();
    let body = &world.body(&name).unwrap().body;
    assert!((solidcraft_kernel::measure(body).unwrap().volume - 150.0).abs() < 1e-6);
    assert!(body.contains(Vec3::new(25.0, 25.0, 1.75)));
    assert!(!body.contains(Vec3::new(25.0, 25.0, 2.25)));
}

#[test]
fn partial_depth_intersect_full_gauge_in_rotated_frames_does_not_invent_pockets() {
    use solidcraft_geom::Vec3;
    for (angle, translate) in [(37.0, [30.0, 20.0, 10.0]), (121.0, [12345.678, 98765.432, 4321.123])] {
        let mut s = Session::default();
        base(&mut s, 100.0, 60.0);
        let name = s.model.state().sheets[0].body.clone();
        let destination = run(&mut s, "component.create", json!({"name":"Destination","parent":0}));
        let owner = destination["component"].as_u64().unwrap();
        run(
            &mut s,
            "occurrence.move",
            json!({"occurrence":destination["occurrence"],"translate":translate,"axis":[1,2,3],"angle":format!("{angle} deg"),"capture":true}),
        );
        run(&mut s, "component.move_bodies", json!({"component":owner,"bodies":[name]}));
        run(&mut s, "component.activate", json!({"component":owner}));
        run(&mut s, "sketch.create", json!({"name":"WorldTool","plane":{"origin":[0,0,0],"x_dir":[1,0,0],"y_dir":[0,1,0]}}));
        run(&mut s, "sketch.rectangle.two_point", json!({"p0":[20,20],"p1":[30,30]}));
        run(&mut s, "sketch.finish", json!({}));
        let mut tool_session = Session::new(solidcraft_doc::Document::from_json(&s.doc.to_json()).unwrap());
        // The witness is built in the sketch's own component, so its caps lie in the sketch's frame.
        run(&mut tool_session, "component.activate", json!({"component":owner}));
        run(&mut tool_session, "solid.extrude", json!({"sketch":"WorldTool","distance":2.5,"body_name":"CapWitness"}));
        let witness_state = tool_session.model.state();
        let witness = &witness_state.body("CapWitness").unwrap().body;
        let tool_plane = witness_state.sketches.last().unwrap().plane;
        let sources = witness.face_sources().unwrap();
        assert_eq!(sources.len(), witness.face_count());
        let mut cap_count = 0;
        for (index, source) in sources.iter().enumerate() {
            if let Some(solidcraft_kernel::FaceSource::CertifiedExtrudeCap { high, plane, .. }) = source {
                cap_count += 1;
                let current = witness.planar_face(index).unwrap();
                assert!(current.normal().cross(tool_plane.normal()).len() < 1e-10);
                assert!((current.origin - plane.origin).dot(tool_plane.normal()).abs() < 1e-8);
                assert!(((plane.origin - tool_plane.origin).dot(tool_plane.normal()) - if *high { 2.5 } else { 0.0 }).abs() < 1e-8);
            }
        }
        assert_eq!(cap_count, 2);
        run(&mut s, "solid.extrude", json!({"sketch":"WorldTool","distance":2.5,"operation":"intersect","name":"PostTransferRetain"}));
        for command in [None, Some("sheet.unfold"), Some("sheet.refold")] {
            if let Some(command) = command {
                run(&mut s, command, json!({}));
            }
            assert!(s.model.state().sheets[0].pockets.is_empty(), "full gauge must not acquire roundoff-depth pockets");
            let world = s.world_state();
            let body = &world.body(&name).unwrap().body;
            assert!((solidcraft_kernel::measure(body).unwrap().volume - 250.0).abs() < 1e-5);
            assert!(body.contains(Vec3::new(25.0, 25.0, 0.1)));
            assert!(body.contains(Vec3::new(25.0, 25.0, 2.4)));
            assert!(!body.contains(Vec3::new(10.0, 25.0, 1.0)));
        }
        let mut cold = Session::new(solidcraft_doc::Document::from_json(&s.doc.to_json()).unwrap());
        assert!(cold.model.state().sheets[0].pockets.is_empty());
        assert!(flat(&mut cold)["pockets"].as_array().unwrap().is_empty());
        run(&mut s, "timeline.edit", json!({"feature":"PostTransferRetain","set":{"extent":{"distance":"1","start_offset":"0.5"}}}));
        for command in [None, Some("sheet.unfold"), Some("sheet.refold")] {
            if let Some(command) = command {
                run(&mut s, command, json!({}));
            }
            assert_eq!(s.model.state().sheets[0].pockets.len(), 2);
            let world = s.world_state();
            let body = &world.body(&name).unwrap().body;
            let measured = solidcraft_kernel::measure(body).unwrap().volume;
            assert!((measured - 100.0).abs() < 1e-5, "retained volume {measured}");
            assert!(body.contains(Vec3::new(25.0, 25.0, 1.0)));
            assert!(!body.contains(Vec3::new(25.0, 25.0, 0.1)));
            assert!(!body.contains(Vec3::new(25.0, 25.0, 2.4)));
        }
        let cold = Session::new(solidcraft_doc::Document::from_json(&s.doc.to_json()).unwrap());
        let world = cold.world_state();
        let body = &world.body(&name).unwrap().body;
        assert!((solidcraft_kernel::measure(body).unwrap().volume - 100.0).abs() < 1e-5);
        assert!(body.contains(Vec3::new(25.0, 25.0, 1.0)));
        assert!(!body.contains(Vec3::new(25.0, 25.0, 0.1)));
    }
}

#[test]
fn partial_depth_intersect_planar_to_object_retains_current_target_depth() {
    use solidcraft_geom::Vec3;
    let mut s = Session::default();
    base(&mut s, 100.0, 60.0);
    let name = s.model.state().sheets[0].body.clone();
    run(&mut s, "solid.box", json!({"corner":[200,0,1.5],"length":10,"width":10,"height":1,"body_name":"Roof"}));
    rectangle_tool(&mut s, "Tool", [20.0, 20.0], [30.0, 30.0]);
    run(&mut s, "solid.extrude", json!({"sketch":"Tool","to":[205,5,1.5],"start_offset":0.5,"operation":"intersect","participants":[name]}));
    for command in [None, Some("sheet.unfold"), Some("sheet.refold")] {
        if let Some(command) = command {
            run(&mut s, command, json!({"body":name}));
        }
        let state = s.model.state();
        let body = &state.body(&name).unwrap().body;
        assert!((solidcraft_kernel::measure(body).unwrap().volume - 100.0).abs() < 1e-6);
        assert!(body.contains(Vec3::new(25.0, 25.0, 1.0)));
        assert!(!body.contains(Vec3::new(25.0, 25.0, 0.1)));
        assert!(!body.contains(Vec3::new(25.0, 25.0, 2.0)));
        assert_eq!(state.sheets[0].pockets.len(), 2);
        assert!((solidcraft_kernel::measure(&state.body("Roof").unwrap().body).unwrap().volume - 100.0).abs() < 1e-6);
    }
    let cold = Session::new(solidcraft_doc::Document::from_json(&s.doc.to_json()).unwrap());
    assert!((solidcraft_kernel::measure(&cold.model.state().body(&name).unwrap().body).unwrap().volume - 100.0).abs() < 1e-6);
}

#[test]
fn partial_depth_intersect_curved_to_object_refuses_unrepresentable_depth_atomically() {
    let mut s = Session::default();
    base(&mut s, 100.0, 60.0);
    let name = s.model.state().sheets[0].body.clone();
    run(&mut s, "solid.cylinder", json!({"base":[25,20,12],"axis":[0,1,0],"radius":11,"height":10,"body_name":"CurvedRoof"}));
    rectangle_tool(&mut s, "Tool", [24.0, 23.0], [26.0, 27.0]);
    let before = s.doc.to_json();
    let error =
        s.execute("solid.extrude", &json!({"sketch":"Tool","to":[36,25,12],"operation":"intersect","participants":[name]})).unwrap_err().to_string();
    assert!(error.contains("sheet machining"), "{error}");
    assert_eq!(s.doc.to_json(), before);
    for command in [None, Some("sheet.unfold"), Some("sheet.refold")] {
        if let Some(command) = command {
            run(&mut s, command, json!({"body":name}));
        }
        let state = s.model.state();
        assert!((solidcraft_kernel::measure(&state.body(&name).unwrap().body).unwrap().volume - 15000.0).abs() < 1e-6);
        assert!(state.sheets[0].pockets.is_empty());
    }
}

#[test]
fn partial_depth_intersect_curved_to_object_preserves_current_trimmed_cap_footprint() {
    use solidcraft_geom::Vec3;
    let mut s = Session::default();
    base(&mut s, 100.0, 60.0);
    let name = s.model.state().sheets[0].body.clone();
    run(&mut s, "solid.cylinder", json!({"base":[25,25,0],"axis":[0,0,1],"radius":2,"height":20,"body_name":"CurvedRoof"}));
    run(&mut s, "solid.box", json!({"corner":[19,19,1],"length":12,"width":12,"height":1,"body_name":"StopPlate"}));
    run(&mut s, "solid.combine", json!({"target":"CurvedRoof","tools":["StopPlate"],"operation":"join"}));
    rectangle_tool(&mut s, "Tool", [20.0, 20.0], [30.0, 30.0]);
    let mut tool_session = Session::new(solidcraft_doc::Document::from_json(&s.doc.to_json()).unwrap());
    run(&mut tool_session, "solid.extrude", json!({"sketch":"Tool","to":[27,25,10],"body_name":"TrimWitness"}));
    let ts = tool_session.model.state();
    let witness = &ts.body("TrimWitness").unwrap().body;
    assert!((solidcraft_kernel::measure(witness).unwrap().volume - (100.0 - 4.0 * std::f64::consts::PI)).abs() < 0.01);
    assert!(witness.contains(Vec3::new(21.0, 21.0, 0.5)));
    assert!(!witness.contains(Vec3::new(25.0, 25.0, 0.5)));
    run(&mut s, "solid.extrude", json!({"sketch":"Tool","to":[27,25,10],"operation":"intersect","participants":[name]}));
    for command in [None, Some("sheet.unfold"), Some("sheet.refold")] {
        if let Some(command) = command {
            run(&mut s, command, json!({"body":name}));
        }
        let state = s.model.state();
        let body = &state.body(&name).unwrap().body;
        let measured = solidcraft_kernel::measure(body).unwrap().volume;
        assert!((measured - (100.0 - 4.0 * std::f64::consts::PI)).abs() < 0.01);
        let area: f64 = state.sheets[0].flat_regions().unwrap().iter().map(|r| r.area()).sum();
        assert!((area - (100.0 - 4.0 * std::f64::consts::PI)).abs() < 1e-8);
        assert!(body.contains(Vec3::new(21.0, 21.0, 0.5)));
        assert!(!body.contains(Vec3::new(25.0, 25.0, 0.5)));
        assert!(!body.contains(Vec3::new(21.0, 21.0, 1.5)));
    }
    let cold = Session::new(solidcraft_doc::Document::from_json(&s.doc.to_json()).unwrap());
    assert!(!cold.model.state().body(&name).unwrap().body.contains(Vec3::new(25.0, 25.0, 0.5)));
    let dxf = run(&mut s, "sheet.export_dxf", json!({"body":name}))["dxf"].as_str().unwrap().to_string();
    assert!(dxf.contains("POCKET_TOP_Z1.000000_TO_2.500000_MM"));
    let outline = dxf_outline_only(&dxf);
    assert_eq!(solidcraft_io::read_dxf(&outline).unwrap().len(), 5);
    let mut outline_import = Session::default();
    run(&mut outline_import, "sketch.insert_dxf", json!({"text":outline,"name":"Outline"}));
    run(&mut outline_import, "sketch.finish", json!({}));
    run(&mut outline_import, "solid.extrude", json!({"distance":1,"profiles":[{"point":[21,21]}]}));
    assert!((volume(&mut outline_import) - (100.0 - 4.0 * std::f64::consts::PI)).abs() < 0.01);
    assert!(!outline_import.model.state().bodies[0].body.contains(Vec3::new(25.0, 25.0, 0.5)));
    let path = std::env::temp_dir().join(format!("solidcraft-current-trim-cap-{}.step", std::process::id()));
    run(&mut s, "file.export", json!({"path":path.to_string_lossy(),"bodies":[name]}));
    let mut imported = Session::default();
    run(&mut imported, "doc.open", json!({"path":path.to_string_lossy()}));
    assert!((volume(&mut imported) - (100.0 - 4.0 * std::f64::consts::PI)).abs() < 0.01);
    assert!(!imported.model.state().bodies[0].body.contains(Vec3::new(25.0, 25.0, 0.5)));
    std::fs::remove_file(path).unwrap();
}

#[test]
fn partial_depth_intersect_sub_extrusion_resolution_layers_refuse_atomically() {
    for (distance, offset) in [(2.5 - 1e-8, 1e-8), (2.5 - 1e-8, 0.0), (1e-8, 0.0)] {
        let mut s = Session::default();
        base(&mut s, 100.0, 60.0);
        let name = s.model.state().sheets[0].body.clone();
        let destination = run(&mut s, "component.create", json!({"name":"Far","parent":0}));
        let owner = destination["component"].as_u64().unwrap();
        run(
            &mut s,
            "occurrence.move",
            json!({"occurrence":destination["occurrence"],"translate":[12345.678,98765.432,4321.123],"axis":[1,2,3],"angle":"121 deg","capture":true}),
        );
        run(&mut s, "component.move_bodies", json!({"component":owner,"bodies":[name]}));
        run(&mut s, "component.activate", json!({"component":owner}));
        run(&mut s, "sketch.create", json!({"name":"Tool","plane":{"origin":[0,0,0],"x_dir":[1,0,0],"y_dir":[0,1,0]}}));
        run(&mut s, "sketch.rectangle.two_point", json!({"p0":[20,20],"p1":[30,30]}));
        run(&mut s, "sketch.finish", json!({}));
        let before = s.doc.to_json();
        let error = s
            .execute("solid.extrude", &json!({"sketch":"Tool","distance":distance,"start_offset":offset,"operation":"intersect"}))
            .unwrap_err()
            .to_string();
        assert!(error.contains("resolution") || error.contains("distance is zero") || error.contains("sheet machining"), "{error}");
        assert_eq!(s.doc.to_json(), before);
        assert!(s.model.state().sheets[0].pockets.is_empty());
        assert!((solidcraft_kernel::measure(&s.world_state().body(&name).unwrap().body).unwrap().volume - 15000.0).abs() < 1e-5);
    }
}

/// A 0.8 mm steel rule and a base flange from a closed outline on XY (the sheet spans z 0..0.8).
fn outline_blank(s: &mut Session, outline: Value) {
    run(s, "sheet.manage_rules", json!({"name": "Steel 0.8", "thickness": "0.8", "active": true}));
    run(s, "sketch.create", json!({"plane": "XY", "name": "Blank"}));
    run(s, "sketch.line", json!({"points": outline, "closed": true}));
    run(s, "sketch.finish", json!({}));
    run(s, "sheet.flange", json!({"sketch": "Blank"}));
}

/// A 100 x 40 blank with two 15 mm tabs on its right edge (y 5..12 and y 28..35), on one bend line.
fn two_tab_blank(s: &mut Session) {
    outline_blank(
        s,
        json!([[0, 0], [100, 0], [100, 5], [115, 5], [115, 12], [100, 12], [100, 28], [115, 28], [115, 35], [100, 35], [100, 40], [0, 40]]),
    );
}

/// Folding one tab must not be refused because another tab lies on the same line (#264).
#[test]
fn fold_one_of_two_collinear_tabs() {
    let mut s = Session::default();
    two_tab_blank(&mut s);
    let flat = volume(&mut s);
    run(&mut s, "sheet.fold", json!({"points": [[107, 5, 0.8], [107, 12, 0.8]], "fixed": [50, 20, 0.8]}));
    assert!(rel(volume(&mut s), flat) < 1e-3, "a fold keeps the material");
}

/// #685: a `fixed` point exactly on the sheet's top face is "on the sheet" (0.8.0 refused it, while a point 0.4 mm
/// above the sheet was accepted). A frame blank 507.4 .. 508.2 with two flange tabs, folded on one of them.
#[test]
fn fold_accepts_a_fixed_point_on_the_top_face() {
    for fixed_z in [508.2, 507.8, 507.4] {
        let mut s = Session::default();
        run(&mut s, "sheet.manage_rules", json!({"name": "SGCC 0.8", "thickness": "0.8", "k_factor": "0.44", "bend_radius": "0.8", "active": true}));
        run(&mut s, "sketch.create", json!({"plane": {"origin": [0, 0, 507.4], "x_dir": [1, 0, 0], "y_dir": [0, 1, 0]}, "name": "Blank"}));
        run(
            &mut s,
            "sketch.line",
            json!({"points": [[1.7, 10.75], [30, 10.75], [30, 0.64], [440, 0.64], [440, 10.75], [465.5, 10.75], [465.5, 229.25],
                              [440, 229.25], [440, 239.36], [30, 239.36], [30, 229.25], [1.7, 229.25]], "closed": true}),
        );
        run(&mut s, "sketch.finish", json!({}));
        run(&mut s, "sheet.flange", json!({"base": {"sketch": "Blank"}, "rule": "SGCC 0.8"}));
        let flat = volume(&mut s);
        run(&mut s, "sheet.fold", json!({"points": [[30, 230.255, 508.2], [440, 230.255, 508.2]], "fixed": [420, 120, fixed_z], "flip": true}));
        assert!(rel(volume(&mut s), flat) < 1e-3, "fixed z {fixed_z}: a fold keeps the material");
    }
}

/// One fold line across both tabs bends both (#264).
#[test]
fn fold_across_two_collinear_tabs() {
    let mut s = Session::default();
    two_tab_blank(&mut s);
    let flat = volume(&mut s);
    run(&mut s, "sheet.fold", json!({"points": [[107, 5, 0.8], [107, 35, 0.8]], "fixed": [50, 20, 0.8]}));
    assert!(rel(volume(&mut s), flat) < 1e-3);
    let bb = &run(&mut s, "inspect.measure", json!({}))["bodies"][0]["bbox"];
    assert!(bb["max"][2].as_f64().unwrap() > 7.0, "both tabs stand up: {bb}");
}

/// After one of two collinear tabs is folded, the other one folds in its own feature too (#638, follow-up to #264).
#[test]
fn fold_second_collinear_tab_after_the_first() {
    let mut s = Session::default();
    two_tab_blank(&mut s);
    let flat = volume(&mut s);
    run(&mut s, "sheet.fold", json!({"points": [[107, 5, 0.8], [107, 12, 0.8]], "fixed": [50, 20, 0.8]}));
    run(&mut s, "sheet.fold", json!({"points": [[107, 28, 0.8], [107, 35, 0.8]], "fixed": [50, 20, 0.8]}));
    assert!(rel(volume(&mut s), flat) < 1e-3, "folds keep the material");
    let bb = &run(&mut s, "inspect.measure", json!({}))["bodies"][0]["bbox"];
    assert!(bb["max"][2].as_f64().unwrap() > 7.0, "the tabs stand up: {bb}");
    // The tabs stand at the bend, x 106.1 .. 107.9 (centerline at x 107).
    assert!(bb["max"][0].as_f64().unwrap() < 110.0, "neither tab still lies flat out to x 115: {bb}");
}

/// An L blank (100 x 50 bottom leg, 40 x 50 upright leg on the right) with a tab x 45..60, y 60..90 on the inside
/// edge x 60. The fold line x 59, extended, runs into the bottom leg.
fn l_blank_with_inside_tab(s: &mut Session) {
    outline_blank(s, json!([[0, 0], [100, 0], [100, 100], [60, 100], [60, 90], [45, 90], [45, 60], [60, 60], [60, 50], [0, 50]]));
}

/// The tab on the inside edge folds wherever on the L sheet `fixed` lies: also when it is on the leg the extended
/// fold line runs into (#639: "folding tabs that join again beyond the bend").
#[test]
fn fold_tab_on_inside_edge_of_l_blank_with_fixed_on_the_far_leg() {
    let mut near = Session::default();
    l_blank_with_inside_tab(&mut near);
    run(&mut near, "sheet.fold", json!({"points": [[59, 60, 0.8], [59, 90, 0.8]], "fixed": [80, 20, 0.8]}));
    let mut far = Session::default();
    l_blank_with_inside_tab(&mut far);
    let flat = volume(&mut far);
    run(&mut far, "sheet.fold", json!({"points": [[59, 60, 0.8], [59, 90, 0.8]], "fixed": [40, 30, 0.8]}));
    assert!(rel(volume(&mut far), flat) < 1e-3, "a fold keeps the material");
    let a = &run(&mut near, "inspect.measure", json!({}))["bodies"][0]["bbox"];
    let b = &run(&mut far, "inspect.measure", json!({}))["bodies"][0]["bbox"];
    assert!(b["max"][2].as_f64().unwrap() > 7.0, "the tab stands up: {b}");
    assert_eq!(a, b, "both fixed points are on the same L sheet, so the fold must give the same part");
}

/// Edge flanges on both tab ends in one feature (two separate features already work) (#264).
#[test]
fn edge_flanges_on_two_collinear_tab_ends_in_one_feature() {
    let mut s = Session::default();
    two_tab_blank(&mut s);
    run(&mut s, "sheet.flange", json!({"edges": [[115, 8, 0.8], [115, 31, 0.8]], "height": "10"}));
    let mut separate = Session::default();
    two_tab_blank(&mut separate);
    run(&mut separate, "sheet.flange", json!({"edges": [[115, 8, 0.8]], "height": "10"}));
    run(&mut separate, "sheet.flange", json!({"edges": [[115, 31, 0.8]], "height": "10"}));
    assert!(rel(volume(&mut s), volume(&mut separate)) < 1e-6);
}

/// A 160 x 30 bar with a 20 x 30 leg standing past its top edge at the left end (2 mm, R 1,
/// K 0.5, so folding keeps the volume): a base flange, or a plate converted to sheet metal.
fn bar_with_leg(s: &mut Session, converted: bool) {
    run(s, "sheet.manage_rules", json!({"name": "AL2", "thickness": "2 mm", "k_factor": 0.5, "bend_radius": "1 mm", "active": true}));
    run(s, "sketch.create", json!({"plane": "XY", "name": "Bar"}));
    run(s, "sketch.line", json!({"points": [[0, 0], [160, 0], [160, 30], [20, 30], [20, 60], [0, 60]], "closed": true}));
    run(s, "sketch.finish", json!({}));
    if converted {
        run(s, "solid.extrude", json!({"sketch": "Bar", "distance": 2, "body_name": "Plate"}));
        run(s, "sheet.convert", json!({"body": "Plate", "face": [80, 15, 2]}));
    } else {
        run(s, "sheet.flange", json!({"sketch": "Bar"}));
    }
}

/// An edge flange takes its setback off its own edge only: the leg standing past the edge's end
/// stays, and the rule's bend relief frees the bend and the flange from it.
#[test]
fn an_edge_flange_beside_a_leg_of_the_base_trims_its_own_edge_and_relieves_its_bend() {
    // Bend allowance (π/2)·(R + K·T) = π; the rule's relief is Thickness wide, half of it deep.
    let (ba, width, depth) = (PI, 2.0, 1.0);
    for (converted, position, setback) in [(false, "inside", 3.0), (false, "outside", 0.0), (true, "inside", 3.0)] {
        let what = format!("converted {converted}, {position}");
        let mut s = Session::default();
        bar_with_leg(&mut s, converted);
        // Where the sheet's bottom face lies.
        let z0 = bbox(&mut s)["min"][2].as_f64().unwrap_or(f64::NAN);
        // A 90° flange 15 high on the bar's top edge (x 20…160); the leg stands past its left end.
        run(&mut s, "sheet.flange", json!({"edges": [[90, 30, z0 + 2.0]], "height": 15, "angle": 90, "position": position}));
        assert!(no_errors_or_warnings(&s), "{what}: {:?}", s.model.results);
        // By hand: the base loses the setback under that edge alone, and the relief beside the
        // bend's end at x = 20, from `depth` before the bend line to `width` past the flange's
        // far edge in the flat (BA and the 12 mm panel past the line).
        let (line, panel) = (30.0 - setback, 12.0);
        let relief = width * (depth + ba + panel + width);
        let base = 160.0 * 30.0 + 20.0 * 30.0 - 140.0 * setback - relief;
        let want = 2.0 * (base + 140.0 * (ba + panel));
        assert!(rel(volume(&mut s), want) < 1e-5, "{what}: {} vs {want}", volume(&mut s));
        assert!(contains(&s, [10.0, 50.0, z0 + 1.0]), "{what}: the leg is kept");
        assert!(!contains(&s, [19.0, line + 0.5, z0 + 1.0]), "{what}: the relief is cut");
        assert!(contains(&s, [17.0, line + 0.5, z0 + 1.0]), "{what}: the leg beside the relief stays");
        assert!(contains(&s, [19.0, 50.0, z0 + 1.0]), "{what}: the leg past the relief's end stays");
        // The flange's mid plane is R + T/2 = 2 past the bend line; it ends at the leg.
        assert!(contains(&s, [90.0, line + 2.0, z0 + 10.0]), "{what}: the flange stands");
        assert!(!contains(&s, [10.0, line + 2.0, z0 + 10.0]), "{what}: the flange ends at the leg");
        let f = flat(&mut s);
        assert_eq!(f["outline"].as_array().map(Vec::len), Some(1), "{what}: {f}");
        assert_eq!(f["cutouts"].as_array().map(Vec::len), Some(0), "{what}: {f}");
        assert!((f["bends"][0]["length"].as_f64().unwrap_or(f64::NAN) - 140.0).abs() < 1e-9, "{what}: {f}");
        run(&mut s, "sheet.unfold", json!({}));
        assert!(rel(volume(&mut s), want) < 1e-9, "{what}: flat {} vs {want}", volume(&mut s));
    }
}

/// A bend relief the rule makes too narrow to cut is left out, and the flange says where.
#[test]
fn a_bend_relief_left_out_is_reported_on_the_flange() {
    let mut s = Session::default();
    bar_with_leg(&mut s, false);
    run(&mut s, "sheet.manage_rules", json!({"name": "AL2", "relief_width": "0 mm"}));
    run(&mut s, "sheet.flange", json!({"edges": [[90, 30, 2]], "height": 15, "angle": 90}));
    let w = s.model.results.last().and_then(|r| r.warning.clone()).unwrap_or_default();
    assert!(w.contains("no bend relief at (20.000, 27.000, 0.000)") && w.contains("relief width"), "{w}");
    assert!(contains(&s, [10.0, 50.0, 1.0]), "the leg is kept");
}

/// `fixed` inside a cut-out touches no material: the fold must refuse it or say which side it kept (#263).
#[test]
fn fold_fixed_point_off_the_sheet_is_not_silently_ignored() {
    let mut s = Session::default();
    outline_blank(&mut s, json!([[0, 0], [100, 0], [100, 60], [0, 60]]));
    run(&mut s, "sketch.create", json!({"plane": "XY", "offset": "2", "name": "Window"}));
    run(&mut s, "sketch.rectangle.two_point", json!({"p0": [10, 15], "p1": [40, 45]}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.extrude", json!({"sketch": "Window", "distance": "4", "direction": "negative", "operation": "cut"}));
    match s.execute("sheet.fold", &json!({"points": [[80, 0, 0.8], [80, 60, 0.8]], "fixed": [25, 30, 0.8]})) {
        Err(e) => assert!(e.to_string().contains("fixed"), "{e}"),
        Ok(r) => {
            let warned =
                ["warning", "warnings"].iter().any(|k| r.get(*k).is_some_and(|w| !w.is_null() && w != &json!([]) && w.to_string().contains("fixed")));
            assert!(warned, "fixed [25, 30] is inside the window, yet the fold succeeded silently: {r}");
        }
    }
}

/// A tab whose fold line sits exactly at its root: the bend zone reaches the blank outline beside
/// the tab. Every edge is straight, so the error must not blame a slanted or curved edge (#262).
#[test]
fn fold_at_a_tab_root_names_the_real_cause() {
    let mut s = Session::default();
    outline_blank(&mut s, json!([[0, 0], [100, 0], [100, 50], [60, 50], [60, 70], [40, 70], [40, 50], [0, 50]]));
    if let Err(e) = s.execute("sheet.fold", &json!({"points": [[40, 50, 0.8], [60, 50, 0.8]], "fixed": [50, 25, 0.8]})) {
        let e = e.to_string();
        assert!(!e.contains("slanted or curved"), "all edges are straight: {e}");
        assert!(e.contains("bend zone"), "name the bend zone reaching the outline: {e}");
    }
    // 1 mm into the tab the same fold works.
    let mut ok = Session::default();
    outline_blank(&mut ok, json!([[0, 0], [100, 0], [100, 50], [60, 50], [60, 70], [40, 70], [40, 50], [0, 50]]));
    run(&mut ok, "sheet.fold", json!({"points": [[40, 51, 0.8], [60, 51, 0.8]], "fixed": [50, 25, 0.8]}));
}

/// #296: two bends meeting at an open corner relief touch only along an edge, so the folded
/// material the machining guard checks must not be sewn into one solid.
#[test]
fn hole_beside_two_adjacent_edge_flanges_cuts_and_bends_stay_guarded() {
    let build = |s: &mut Session| {
        base(s, 80.0, 40.0);
        run(s, "sheet.flange", json!({"edges":[[40.0, 0.0, 2.5]],"height":30,"angle":90}));
        run(s, "sheet.flange", json!({"edges":[[80.0, 22.5, 2.5]],"height":15,"angle":90}));
    };
    let hole = |s: &mut Session, name: &str, centre: [f64; 2]| {
        run(s, "sketch.create", json!({"plane":"XY","name":name}));
        run(s, "sketch.circle.center", json!({"center":centre,"radius":3}));
        run(s, "sketch.finish", json!({}));
        s.execute("solid.extrude", &json!({"sketch":name,"through_all":true,"operation":"cut"}))
    };
    let mut s = Session::default();
    build(&mut s);
    let before = volume(&mut s);
    assert!((before - 14057.2).abs() < 0.1, "{before}");
    if let Err(e) = hole(&mut s, "Hole", [25.0, 20.0]) {
        panic!("hole beside two flanges: {e}");
    }
    let after = volume(&mut s);
    assert!((before - after - 9.0 * PI * 2.5).abs() < 0.05, "{before} -> {after}");
    let mut bend = Session::default();
    build(&mut bend);
    let error = hole(&mut bend, "Bend", [78.5, 20.0]).unwrap_err().to_string();
    assert!(error.contains("bend"), "{error}");
}

/// The guard of #296 sews every bend on its own. A bend that carries a folded round cut-out
/// (curved edges on both of its cylinders, a ruled wall between them) sews like any other: a hole
/// beside it cuts, and a hole in the bend is still refused.
#[test]
fn hole_beside_a_bend_with_a_round_cut_out_cuts_and_the_bend_stays_guarded() {
    let build = |s: &mut Session| {
        base(s, 80.0, 40.0);
        run(s, "sheet.flange", json!({"edges":[[40.0, 0.0, 2.5]],"height":30,"angle":90}));
        run(s, "sheet.unfold", json!({}));
        run(s, "sketch.create", json!({"plane":"XY","name":"Across"}));
        run(s, "sketch.circle.center", json!({"center":[60.0, 0.0],"radius":4}));
        run(s, "sketch.finish", json!({}));
        run(s, "solid.extrude", json!({"sketch":"Across","through_all":true,"operation":"cut","direction":"symmetric"}));
        run(s, "sheet.refold", json!({}));
    };
    let hole = |s: &mut Session, name: &str, centre: [f64; 2]| {
        run(s, "sketch.create", json!({"plane":"XY","name":name}));
        run(s, "sketch.circle.center", json!({"center":centre,"radius":3}));
        run(s, "sketch.finish", json!({}));
        s.execute("solid.extrude", &json!({"sketch":name,"through_all":true,"operation":"cut"}))
    };
    let mut s = Session::default();
    build(&mut s);
    let before = volume(&mut s);
    if let Err(e) = hole(&mut s, "Hole", [25.0, 20.0]) {
        panic!("hole beside a bend with a round cut-out: {e}");
    }
    let after = volume(&mut s);
    assert!((before - after - 9.0 * PI * 2.5).abs() < 0.05, "{before} -> {after}");
    let mut bend = Session::default();
    build(&mut bend);
    let error = hole(&mut bend, "Bend", [25.0, 1.5]).unwrap_err().to_string();
    assert!(error.contains("bend"), "{error}");
}

/// Whether the flat pattern has a cut-out whose points all lie on the circle of radius `r` about `c`.
fn has_round_cutout(f: &Value, c: [f64; 2], r: f64) -> bool {
    f["cutouts"].as_array().into_iter().flatten().any(|cut| {
        let on_circle = |p: &Value| ((p[0].as_f64().unwrap_or(f64::NAN) - c[0]).hypot(p[1].as_f64().unwrap_or(f64::NAN) - c[1]) - r).abs() < 1e-3;
        cut.as_array().is_some_and(|points| !points.is_empty() && points.iter().all(on_circle))
    })
}

/// Whether the flat DXF holds the circle of radius `r` about `c` as a `CIRCLE`.
fn dxf_has_circle(s: &mut Session, c: [f64; 2], r: f64) -> bool {
    let dxf = run(s, "sheet.export_dxf", json!({}))["dxf"].as_str().unwrap_or_default().to_string();
    let centre = solidcraft_geom::Vec2::new(c[0], c[1]);
    solidcraft_io::read_dxf(&dxf)
        .unwrap_or_default()
        .iter()
        .any(|g| matches!(g, solidcraft_io::Geom2::Circle(at, radius) if at.dist(centre) < 1e-5 && (radius - r).abs() < 1e-5))
}

/// A hole on a flange lands in the flat pattern where the bend allowance puts its panel.
#[test]
fn a_hole_on_a_flange_panel_unfolds_with_the_flange() {
    // The flange's panel begins 5 mm (R + T) above the base's underside, and at y = 5 − BA in
    // the flat pattern: a hole 7 mm up the panel is at y = 5 − BA − 7 there, one 10 mm up at
    // y = 5 − BA − 10.
    let ba = FRAC_PI_2 * (2.5 + 0.44 * 2.5);
    let mut s = Session::default();
    plate_with_flange(&mut s);
    let v0 = volume(&mut s);
    // Ø4 from the flange's inner face (y = 2.5), Ø3 from its outer face (y = 0), Ø6 on the base.
    run(&mut s, "solid.hole", json!({"position": [30, 2.5, 12], "diameter": 4}));
    run(&mut s, "solid.hole", json!({"position": [70, 0, 15], "diameter": 3}));
    run(&mut s, "solid.hole", json!({"position": [50, 30, 2.5], "diameter": 6}));
    let removed = PI * 2.5 * (4.0 + 2.25 + 9.0);
    assert!((v0 - volume(&mut s) - removed).abs() < 0.05, "{}", v0 - volume(&mut s));
    assert!(!contains(&s, [30.0, 1.25, 12.0]) && contains(&s, [33.0, 1.25, 12.0]));
    assert!(!contains(&s, [70.0, 1.25, 15.0]) && !contains(&s, [50.0, 30.0, 1.25]));
    let holes = [([30.0, 5.0 - ba - 7.0], 2.0), ([70.0, 5.0 - ba - 10.0], 1.5), ([50.0, 30.0], 3.0)];
    let f = flat(&mut s);
    assert_eq!(f["cutouts"].as_array().map(Vec::len), Some(3));
    for (centre, radius) in holes {
        assert!(has_round_cutout(&f, centre, radius), "{centre:?}: {}", f["cutouts"]);
        assert!(dxf_has_circle(&mut s, centre, radius), "{centre:?}");
    }
    // Unfolded, the sheet is its flat pattern 2.5 thick: (55 + BA + 15) x 100, less the holes.
    run(&mut s, "sheet.unfold", json!({}));
    assert!((volume(&mut s) - ((70.0 + ba) * 250.0 - removed)).abs() < 0.05, "{}", volume(&mut s));
    run(&mut s, "sheet.refold", json!({}));
    assert!((v0 - volume(&mut s) - removed).abs() < 0.05, "{}", v0 - volume(&mut s));
    // A hole made on the unfolded sheet where the flange lies is on the flange once refolded.
    let mut flat_first = Session::default();
    plate_with_flange(&mut flat_first);
    run(&mut flat_first, "sheet.unfold", json!({}));
    run(&mut flat_first, "solid.hole", json!({"position": [30, 5.0 - ba - 7.0, 2.5], "diameter": 4}));
    run(&mut flat_first, "sheet.refold", json!({}));
    assert!((v0 - volume(&mut flat_first) - PI * 4.0 * 2.5).abs() < 0.05, "{}", v0 - volume(&mut flat_first));
    assert!(!contains(&flat_first, [30.0, 1.25, 12.0]) && contains(&flat_first, [33.0, 1.25, 12.0]));
}

/// On a folded sheet a counterbore is the same pocket around the same hole, on the base and on a
/// flange, and stays through unfold and refold.
#[test]
fn a_counterbore_on_a_folded_sheet_is_a_pocket_around_its_cut_out() {
    let ba = FRAC_PI_2 * (2.5 + 0.44 * 2.5);
    let mut s = Session::default();
    plate_with_flange(&mut s);
    let v0 = volume(&mut s);
    // Ø4 with Ø8 x 1 on the base: 22π mm³. Ø3 with Ø6 x 1 from the flange's inner face, 7 mm up
    // its panel: π · (1.5² · 2.5 + (3² − 1.5²) · 1) = 12.375π mm³.
    run(&mut s, "solid.hole", json!({"position": [80, 40, 2.5], "diameter": 4, "type": "counterbore", "cb_diameter": 8, "cb_depth": 1}));
    run(&mut s, "solid.hole", json!({"position": [30, 2.5, 12], "diameter": 3, "type": "counterbore", "cb_diameter": 6, "cb_depth": 1}));
    let removed = (22.0 + 12.375) * PI;
    assert!((v0 - volume(&mut s) - removed).abs() < 0.05, "{}", v0 - volume(&mut s));
    assert!(!contains(&s, [83.0, 40.0, 2.0]) && contains(&s, [83.0, 40.0, 1.0]) && !contains(&s, [80.0, 40.0, 1.0]));
    // The flange's pocket is in its inner face (y from 1.5 to 2.5), around the hole.
    assert!(!contains(&s, [32.0, 2.0, 12.0]) && contains(&s, [32.0, 1.0, 12.0]) && !contains(&s, [30.0, 1.0, 12.0]));
    let f = flat(&mut s);
    assert_eq!(f["cutouts"].as_array().map(Vec::len), Some(2));
    assert!(has_round_cutout(&f, [80.0, 40.0], 2.0) && has_round_cutout(&f, [30.0, 5.0 - ba - 7.0], 1.5), "{}", f["cutouts"]);
    assert_eq!(f["pockets"].as_array().map(Vec::len), Some(2));
    run(&mut s, "sheet.unfold", json!({}));
    assert!((volume(&mut s) - ((70.0 + ba) * 250.0 - removed)).abs() < 0.05, "{}", volume(&mut s));
    run(&mut s, "sheet.refold", json!({}));
    assert!((v0 - volume(&mut s) - removed).abs() < 0.05, "{}", v0 - volume(&mut s));
}

/// The pocket's tool spans a cut-out inside it: extrude cuts of a pocket and of a hole in its
/// floor, in either order, rebuild on the base of a folded sheet, where the pocket's own wall
/// along the hole's used to make the cut fail.
#[test]
fn a_pocket_around_a_cut_out_rebuilds_on_a_folded_sheet() {
    for pocket_first in [true, false] {
        let mut s = Session::default();
        plate_with_flange(&mut s);
        let v0 = volume(&mut s);
        let cuts = [("Pocket", 6.0, 1.0), ("Bore", 3.0, 2.5)];
        for (name, diameter, depth) in if pocket_first { cuts } else { [cuts[1], cuts[0]] } {
            circle_sketch(&mut s, name, json!({"face": [50, 30, 2.5]}), [40.0, 30.0], diameter);
            run(&mut s, "solid.extrude", json!({"sketch": name, "distance": depth, "direction": "negative", "operation": "cut"}));
        }
        // π · (1.5² · 2.5 + (3² − 1.5²) · 1) = 12.375π mm³.
        assert!((v0 - volume(&mut s) - 12.375 * PI).abs() < 0.05, "{pocket_first}: {}", v0 - volume(&mut s));
        let f = flat(&mut s);
        assert_eq!(f["cutouts"].as_array().map(Vec::len), Some(1));
        assert_eq!(f["pockets"].as_array().map(Vec::len), Some(1));
        for command in ["sheet.unfold", "sheet.refold"] {
            run(&mut s, command, json!({}));
        }
        assert!((v0 - volume(&mut s) - 12.375 * PI).abs() < 0.05, "{pocket_first}: {}", v0 - volume(&mut s));
    }
}

/// Engraving a long channel leaves its corner rounds round. The channel (1200 long, 1 thick) has
/// 2 mm rounds where its legs end, and two 5.95 x 5 pockets 0.1 apart are cut 0.1 deep into a leg
/// halfway along. Their walls in one plane leave the second pocket to the exact planar fallback,
/// whose mesh (a thousandth of the body, 1.2 mm) is flat across a 2 mm round: taken for a plane,
/// the rounds came back flat chamfers beside a round hole, and without one the cut failed.
#[test]
fn engraving_a_long_channel_keeps_its_corner_rounds() {
    for hole in [false, true] {
        let mut s = Session::default();
        run(&mut s, "sheet.manage_rules", json!({"name": "Thin", "active": true, "thickness": "1 mm", "bend_radius": "1 mm", "k_factor": "0.44"}));
        // Along z, 20 wide, its legs 65 and 25 high from the web at y = 65.
        run(&mut s, "sketch.create", json!({"plane": {"origin": [0, 0, -600], "x_dir": [1, 0, 0], "y_dir": [0, 1, 0]}, "name": "Profile"}));
        run(&mut s, "sketch.line", json!({"points": [[-10, 0], [-10, 65], [10, 65], [10, 40]], "ids": ["l1", "l2", "l3"]}));
        run(&mut s, "sketch.finish", json!({}));
        run(&mut s, "sheet.flange", json!({"sketch": "Profile", "curves": ["l1", "l2", "l3"], "distance": 1200}));
        // Each leg's free edge rounded at both ends: the corner outside a quarter circle, cut through.
        let legs = [(-10.0_f64, 0.0_f64), (10.0, 40.0)];
        for (k, (x, y)) in legs.iter().enumerate() {
            let name = format!("Rounds{k}");
            run(&mut s, "sketch.create", json!({"plane": {"origin": [x, 0, 0], "x_dir": [0, 0, 1], "y_dir": [0, 1, 0]}, "name": name}));
            for end in [-600.0, 600.0_f64] {
                let c = end - 2.0 * end.signum();
                let far = end + 2.0 * end.signum();
                run(&mut s, "sketch.line", json!({"points": [[c, y], [c, y - 2.0], [far, y - 2.0], [far, y + 2.0], [end, y + 2.0]]}));
                run(&mut s, "sketch.arc.center_point", json!({"center": [c, y + 2.0], "start": [end, y + 2.0], "sweep": -90.0 * end.signum()}));
            }
            run(&mut s, "sketch.finish", json!({}));
            run(&mut s, "solid.extrude", json!({"sketch": name, "distance": 4, "direction": "symmetric", "operation": "cut"}));
        }
        if hole {
            run(&mut s, "sketch.create", json!({"plane": {"origin": [-10, 0, 0], "x_dir": [0, 0, 1], "y_dir": [0, 1, 0]}, "name": "Hole"}));
            run(&mut s, "sketch.circle.center", json!({"center": [360, 57], "diameter": 4}));
            run(&mut s, "sketch.finish", json!({}));
            run(&mut s, "solid.extrude", json!({"sketch": "Hole", "distance": 4, "direction": "symmetric", "operation": "cut"}));
        }
        let v0 = volume(&mut s);
        // Into the outer face of the leg at x = -10 (its material from -10 to -9).
        run(&mut s, "sketch.create", json!({"plane": {"origin": [-10, 0, 0], "x_dir": [0, 0, 1], "y_dir": [0, 1, 0]}, "name": "Label"}));
        run(&mut s, "sketch.rectangle.two_point", json!({"p0": [-6, 30], "p1": [-0.05, 35]}));
        run(&mut s, "sketch.rectangle.two_point", json!({"p0": [0.05, 30], "p1": [6, 35]}));
        run(&mut s, "sketch.finish", json!({}));
        run(&mut s, "solid.extrude", json!({"sketch": "Label", "distance": 0.1, "direction": "negative", "operation": "cut"}));
        // 2 x 5.95 x 5 x 0.1.
        let removed = v0 - volume(&mut s);
        assert!((removed - 5.95).abs() < 1e-3, "hole {hole}: removed {removed}");
        // A point on each round, halfway through the leg: 2 - 2 cos 45 deg in from both edges.
        let d = 2.0 - std::f64::consts::SQRT_2;
        let faces: Vec<[f64; 3]> =
            legs.iter().flat_map(|(x, y)| [-600.0, 600.0_f64].map(|end| [x + 0.5 * -x.signum(), y + d, end - d * end.signum()])).collect();
        let radii = run(&mut s, "inspect.minimum_radius", json!({"faces": faces}));
        let items = radii["items"].as_array().unwrap();
        assert_eq!(items.len(), 4);
        for item in items {
            assert!(item["min_radius"].as_f64().is_some_and(|r| (r - 2.0).abs() < 1e-6), "hole {hole}: {item}");
        }
    }
}

/// A fold whose stored `fixed` point ends up off the sheet after a parameter change keeps working:
/// the point still says which side stays (#370). Only a new pick off the sheet is refused (#263).
#[test]
fn fold_keeps_working_when_a_parameter_moves_the_sheet_off_its_fixed_point() {
    let mut s = Session::default();
    run(&mut s, "parameters.change", json!({"name": "w", "expression": "100 mm"}));
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Blank"}));
    run(&mut s, "sketch.rectangle.two_point", json!({"p0": [0, 0], "p1": [100, 60]}));
    run(&mut s, "sketch.dimension", json!({"entities": ["l1"], "value": "w"}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "sheet.flange", json!({"sketch": "Blank"}));
    run(&mut s, "sheet.fold", json!({"points": [[40, 0, 2.5], [40, 60, 2.5]], "fixed": [90, 30, 2.5], "name": "Fold1"}));
    run(&mut s, "parameters.change", json!({"name": "w", "expression": "80 mm"}));
    let fold = run(&mut s, "timeline.inspect", json!({"feature": "Fold1"}));
    assert!(fold["error"].is_null(), "the fold fails after the width change: {}", fold["error"]);
    // The +x side (where the point is) stays flat out to the new width; the -x side stands up.
    let bb = &run(&mut s, "inspect.measure", json!({}))["bodies"][0]["bbox"];
    assert!((bb["max"][0].as_f64().unwrap_or(0.0) - 80.0).abs() < 1e-6, "{bb}");
    assert!(bb["max"][2].as_f64().unwrap_or(0.0) > 30.0, "{bb}");
}

/// `fixed` 20 mm above a 2.5 mm sheet touches no material: refuse it or say so, like a point in a
/// cut-out (#371).
#[test]
fn fold_fixed_point_above_the_sheet_is_not_silently_accepted() {
    let mut s = Session::default();
    base(&mut s, 100.0, 60.0);
    match s.execute("sheet.fold", &json!({"points": [[40, 0, 2.5], [40, 60, 2.5]], "fixed": [90, 30, 20]})) {
        Err(e) => assert!(e.to_string().contains("fixed"), "{e}"),
        Ok(r) => {
            let warned = ["warning", "warnings"].iter().any(|k| r.get(*k).is_some_and(|w| w.to_string().contains("fixed")));
            assert!(warned, "fixed [90, 30, 20] is 17.5 mm above the sheet, yet the fold succeeded silently: {r}");
        }
    }
}

/// A 120 x 90 tray in 2 mm aluminium (R 1, K 0.5): four 38.6 mm flanges at `deg` in one feature.
fn alu_tray(s: &mut Session, deg: f64, rule: Value) {
    let mut r = json!({"name": "AL2", "thickness": "2 mm", "k_factor": 0.5, "bend_radius": "1 mm", "gap": "0.5 mm", "active": true});
    if let (Some(r), Some(extra)) = (r.as_object_mut(), rule.as_object()) {
        r.extend(extra.clone());
    }
    run(s, "sheet.manage_rules", r);
    base(s, 120.0, 90.0);
    run(s, "sheet.flange", json!({"edges": [[60, 0, 2], [120, 45, 2], [60, 90, 2], [0, 45, 2]], "height": 38.6, "angle": format!("{deg} deg")}));
}

#[test]
fn closed_corners_reach_the_flat_pattern_and_the_dxf() {
    use std::f64::consts::SQRT_2;
    // 60° and 105° are angles whose flat pattern was lost to a rounding slip in the planar arrangement.
    for deg in [60.0f64, 72.0, 90.0, 105.0] {
        let mut s = Session::default();
        alu_tray(&mut s, deg, json!({}));
        assert!(s.model.results.iter().all(|r| r.error.is_none() && r.warning.is_none()), "{deg}: {:?}", s.model.results);
        // The end of the flange on y = 0 at the corner at the origin, by hand (see the doc tests):
        // x(w) = g/√2 + T·sinθ − w·cosθ at flat y = s0 − BA − (w − s0), for w = s0 and w = 38.6.
        let th = deg.to_radians();
        let (s0, ba) = (3.0 * (th / 2.0).tan(), th * 2.0);
        let x_end = |w: f64| 0.5 / SQRT_2 + 2.0 * th.sin() - w * th.cos();
        let f = flat(&mut s);
        let outline: Vec<(f64, f64)> = f["outline"][0]
            .as_array()
            .map(|l| l.iter().map(|p| (p[0].as_f64().unwrap_or(f64::NAN), p[1].as_f64().unwrap_or(f64::NAN))).collect())
            .unwrap_or_default();
        assert_eq!(f["outline"].as_array().map(Vec::len), Some(1), "{deg}: {f}");
        for (x, y) in [(x_end(s0), s0 - ba), (x_end(38.6), s0 - ba - (38.6 - s0))] {
            assert!(outline.iter().any(|p| (p.0 - x).abs() < 1e-6 && (p.1 - y).abs() < 1e-6), "{deg}: ({x}, {y}) not in {outline:?}");
        }
        // The folded part holds the flat pattern's material (K = 0.5 keeps the volume).
        let n = outline.len();
        let area = (0..n).map(|i| outline[i].0 * outline[(i + 1) % n].1 - outline[(i + 1) % n].0 * outline[i].1).sum::<f64>().abs() / 2.0;
        let v = volume(&mut s);
        assert!((v - area * 2.0).abs() < 1e-3 * v, "{deg}: {v} vs {}", area * 2.0);
        // The DXF: the closed outline in straight lines, four bend lines.
        let dxf = run(&mut s, "sheet.export_dxf", json!({}))["dxf"].as_str().unwrap_or_default().to_string();
        assert_eq!(dxf.matches("0\nLINE\n").count(), n + 4, "{deg}: {n} outline points");
        // Unfold and refold keep it.
        run(&mut s, "sheet.unfold", json!({}));
        run(&mut s, "sheet.refold", json!({}));
        assert!((volume(&mut s) - v).abs() < 1e-6 * v, "{deg}");
        // A cut made folded still goes through with the corners closed: a Ø20 hole in the base.
        run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Hole"}));
        run(&mut s, "sketch.circle.center", json!({"center": [60, 45], "diameter": 20}));
        run(&mut s, "sketch.finish", json!({}));
        run(&mut s, "solid.extrude", json!({"sketch": "Hole", "distance": 10, "operation": "cut", "through_all": true}));
        let want = PI * 100.0 * 2.0;
        assert!((v - volume(&mut s) - want).abs() < 1e-3 * want, "{deg}: {v} - {} vs {want}", volume(&mut s));
        assert_eq!(flat(&mut s)["cutouts"].as_array().map(Vec::len), Some(1), "{deg}");
    }
}

#[test]
fn corner_reliefs_of_the_rule_reach_the_part_the_flat_pattern_and_the_dxf() {
    use solidcraft_geom::Vec2;
    // The tray at 72°: its bends meet at the corners of the base, s0 = 3·tan 36° in from the sheet's
    // edges, and are BA = 2θ wide in the flat. A relief 8 mm across reaches r = 4 mm along both edges
    // and takes, at each corner, r²·π/4 (round) or r² (rectangular) of the base and r of two bends.
    let th = 72f64.to_radians();
    let (s0, ba) = (3.0 * (th / 2.0).tan(), th * 2.0);
    let corners = [(s0, s0), (120.0 - s0, s0), (120.0 - s0, 90.0 - s0), (s0, 90.0 - s0)];
    let export = |s: &mut Session| run(s, "sheet.export_dxf", json!({}))["dxf"].as_str().unwrap_or_default().to_string();
    let mut notch = Session::default();
    alu_tray(&mut notch, 72.0, json!({"corner_relief": "8 mm"}));
    let v0 = volume(&mut notch);
    assert_eq!(export(&mut notch).matches("0\nARC\n").count(), 0);
    run(&mut notch, "sheet.unfold", json!({}));
    let flat0 = volume(&mut notch);
    for (shape, of_the_base) in [("round", 4.0 * PI), ("rectangular", 16.0)] {
        let mut s = Session::default();
        alu_tray(&mut s, 72.0, json!({"corner_relief_shape": shape, "corner_relief": "8 mm"}));
        assert!(s.model.results.iter().all(|r| r.error.is_none() && r.warning.is_none()), "{shape}: {:?}", s.model.results);
        let listed = run(&mut s, "sheet.rules", json!({}));
        assert_eq!(listed["rules"][0]["rule"]["corner_relief_shape"], shape);
        assert_eq!(listed["rules"][0]["values"]["corner_relief_shape"], shape);
        // The folded part loses what the flat pattern loses (K = 0.5 keeps the volume of a bend).
        let (v, want) = (volume(&mut s), 4.0 * (of_the_base + 8.0 * ba) * 2.0);
        assert!((v0 - v - want).abs() < 0.1, "{shape}: {v0} - {v} vs {want}");
        // The bend table: every bend is 8 mm shorter.
        let f = flat(&mut s);
        let lengths: Vec<f64> =
            f["bends"].as_array().map(|b| b.iter().map(|b| b["length"].as_f64().unwrap_or(f64::NAN)).collect()).unwrap_or_default();
        for (got, edge) in lengths.iter().zip([120.0, 90.0, 120.0, 90.0]) {
            assert!(lengths.len() == 4 && (got - (edge - 2.0 * s0 - 8.0)).abs() < 1e-6, "{shape}: {lengths:?}");
        }
        assert_eq!(f["outline"].as_array().map(Vec::len), Some(1), "{shape}");
        // The DXF: the round reliefs are arcs of radius 4 about the four corners, the square ones lines.
        let dxf = export(&mut s);
        let arcs: Vec<(Vec2, f64)> = solidcraft_io::read_dxf(&dxf)
            .unwrap_or_default()
            .iter()
            .filter_map(|g| if let solidcraft_io::Geom2::Arc { c, a, .. } = g { Some((*c, a.dist(*c))) } else { None })
            .collect();
        if shape == "round" {
            assert_eq!(arcs.len(), 4, "{arcs:?}");
            for (x, y) in corners {
                assert!(arcs.iter().any(|(c, r)| c.dist(Vec2::new(x, y)) < 1e-6 && (r - 4.0).abs() < 1e-6), "({x}, {y}): {arcs:?}");
            }
        } else {
            let corner = Vec2::new(s0 + 4.0, s0 + 4.0);
            let read = solidcraft_io::read_dxf(&dxf).unwrap_or_default();
            let at_corner =
                |g: &solidcraft_io::Geom2| matches!(g, solidcraft_io::Geom2::Line(a, b) if a.dist(corner) < 1e-6 || b.dist(corner) < 1e-6);
            assert!(arcs.is_empty() && read.iter().any(at_corner), "{arcs:?}");
        }
        // Unfold and refold keep it, and a cut made folded still goes through: a Ø20 hole in the base.
        run(&mut s, "sheet.unfold", json!({}));
        assert!((flat0 - volume(&mut s) - want).abs() < 0.1, "{shape}: unfolded {flat0} - {} vs {want}", volume(&mut s));
        run(&mut s, "sheet.refold", json!({}));
        assert!((volume(&mut s) - v).abs() < 1e-6 * v, "{shape}");
        run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Hole"}));
        run(&mut s, "sketch.circle.center", json!({"center": [60, 45], "diameter": 20}));
        run(&mut s, "sketch.finish", json!({}));
        run(&mut s, "solid.extrude", json!({"sketch": "Hole", "distance": 10, "operation": "cut", "through_all": true}));
        assert!((v - volume(&mut s) - PI * 100.0 * 2.0).abs() < 0.5, "{shape}: {v} - {}", volume(&mut s));
        let mut reopened = Session::new(solidcraft_doc::Document::from_json(&s.doc.to_json()).unwrap());
        assert!((volume(&mut reopened) - volume(&mut s)).abs() < 1e-6, "{shape}");
    }
}

#[test]
fn a_tear_of_the_rule_reaches_the_part_the_flat_pattern_and_the_dxf() {
    use std::f64::consts::SQRT_2;
    // The tray at 90° and 72° (see the doc tests): its bends meet at (s0, s0), s0 = 3·tan(θ/2), and
    // are BA = 2θ wide in the flat. The flange on y = 0 is closed to x_end(w) = g/√2 + T·sinθ − w·cosθ.
    // Torn, its bend's end is cut from (s0, s0) straight to (x_end(s0), s0 − BA), where the flange's
    // end starts: grow = s0 − x_end(s0) = sinθ − 0.5/√2 past the corner, instead of square across the
    // bend and back along the flange's edge (the notch's step).
    for deg in [90.0f64, 72.0] {
        let th = deg.to_radians();
        let (s0, ba) = (3.0 * (th / 2.0).tan(), th * 2.0);
        let x_end = |w: f64| 0.5 / SQRT_2 + 2.0 * th.sin() - w * th.cos();
        let grow = s0 - x_end(s0);
        assert!((grow - (th.sin() - 0.5 / SQRT_2)).abs() < 1e-12);
        let points = |f: &Value| -> Vec<(f64, f64)> {
            let line = f["outline"][0].as_array();
            line.map(|l| l.iter().map(|p| (p[0].as_f64().unwrap_or(f64::NAN), p[1].as_f64().unwrap_or(f64::NAN))).collect()).unwrap_or_default()
        };
        let mut notch = Session::default();
        alu_tray(&mut notch, deg, json!({}));
        let (v0, stepped) = (volume(&mut notch), points(&flat(&mut notch)));
        run(&mut notch, "sheet.unfold", json!({}));
        let flat0 = volume(&mut notch);
        let mut s = Session::default();
        alu_tray(&mut s, deg, json!({"corner_relief_shape": "tear"}));
        assert!(s.model.results.iter().all(|r| r.error.is_none() && r.warning.is_none()), "{deg}: {:?}", s.model.results);
        let listed = run(&mut s, "sheet.rules", json!({}));
        assert_eq!(listed["rules"][0]["rule"]["corner_relief_shape"], "tear");
        assert_eq!(listed["rules"][0]["values"]["corner_relief_shape"], "tear");
        // The flat pattern, at the corner at the origin: the flange's end, the cut, the corner of the
        // bends, and the same on the flange on x = 0. The step is gone: a corner less at each of the
        // eight bend ends.
        let f = flat(&mut s);
        let outline = points(&f);
        let at = |pts: &[(f64, f64)], x: f64, y: f64| pts.iter().any(|p| (p.0 - x).abs() < 1e-6 && (p.1 - y).abs() < 1e-6);
        assert_eq!(f["outline"].as_array().map(Vec::len), Some(1), "{deg}: {f}");
        for (x, y) in [(x_end(38.6), s0 - ba - (38.6 - s0)), (x_end(s0), s0 - ba), (s0, s0)] {
            assert!(at(&outline, x, y) && at(&outline, y, x), "{deg}: ({x}, {y}) not in {outline:?}");
        }
        assert!(at(&stepped, s0, s0 - ba) && !at(&outline, s0, s0 - ba) && !at(&outline, s0 - ba, s0), "{deg}");
        assert_eq!(outline.len() + 8, stepped.len(), "{deg}");
        // The bend table: every centre line runs between the middles of its bend's two ends.
        let lengths: Vec<f64> =
            f["bends"].as_array().map(|b| b.iter().map(|b| b["length"].as_f64().unwrap_or(f64::NAN)).collect()).unwrap_or_default();
        for (got, edge) in lengths.iter().zip([120.0, 90.0, 120.0, 90.0]) {
            assert!(lengths.len() == 4 && (got - (edge - 2.0 * s0 + grow)).abs() < 1e-6, "{deg}: {lengths:?}");
        }
        // The folded part gains what the flat pattern gains: the triangle BA·grow/2 at each of the
        // eight bend ends (K = 0.5 keeps the volume of a bend).
        let (v, want) = (volume(&mut s), 8.0 * ba * grow / 2.0 * 2.0);
        assert!((v - v0 - want).abs() < 0.05, "{deg}: {v} - {v0} vs {want}");
        // The DXF: the outline in straight lines, each cut one of them, and four bend lines.
        let dxf = run(&mut s, "sheet.export_dxf", json!({}))["dxf"].as_str().unwrap_or_default().to_string();
        assert_eq!(dxf.matches("0\nLINE\n").count(), outline.len() + 4, "{deg}");
        let cut_end = solidcraft_geom::Vec2::new(x_end(s0), s0 - ba);
        let read = solidcraft_io::read_dxf(&dxf).unwrap_or_default();
        let at_cut_end = |g: &solidcraft_io::Geom2| matches!(g, solidcraft_io::Geom2::Line(a, b) if a.dist(cut_end) < 1e-6 || b.dist(cut_end) < 1e-6);
        assert!(read.iter().any(at_cut_end), "{deg}");
        // Unfold and refold keep it, and a cut made folded still goes through: a Ø20 hole in the base.
        run(&mut s, "sheet.unfold", json!({}));
        assert!((volume(&mut s) - flat0 - want).abs() < 1e-6, "{deg}: unfolded {} - {flat0} vs {want}", volume(&mut s));
        run(&mut s, "sheet.refold", json!({}));
        assert!((volume(&mut s) - v).abs() < 1e-6 * v, "{deg}");
        run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Hole"}));
        run(&mut s, "sketch.circle.center", json!({"center": [60, 45], "diameter": 20}));
        run(&mut s, "sketch.finish", json!({}));
        run(&mut s, "solid.extrude", json!({"sketch": "Hole", "distance": 10, "operation": "cut", "through_all": true}));
        assert!((v - volume(&mut s) - PI * 100.0 * 2.0).abs() < 0.5, "{deg}: {v} - {}", volume(&mut s));
        let mut reopened = Session::new(solidcraft_doc::Document::from_json(&s.doc.to_json()).unwrap());
        assert!((volume(&mut reopened) - volume(&mut s)).abs() < 1e-6, "{deg}");
    }
}

#[test]
fn the_corner_relief_shape_is_a_rule_setting_checked_and_undone() {
    let mut s = Session::default();
    alu_tray(&mut s, 72.0, json!({}));
    let shape = |s: &mut Session| run(s, "sheet.rules", json!({}))["rules"][0]["rule"]["corner_relief_shape"].clone();
    let v0 = volume(&mut s);
    assert_eq!(shape(&mut s), "notch");
    // Only the four names are shapes; a refused one changes nothing.
    for wrong in [json!("star"), json!("square"), json!(""), json!(2)] {
        let e = s.execute("sheet.manage_rules", &json!({"name": "AL2", "corner_relief_shape": wrong})).map(|_| ()).unwrap_err().to_string();
        assert!(e.contains("`corner_relief_shape` must be notch, round, rectangular or tear"), "{e}");
    }
    assert!(shape(&mut s) == "notch" && volume(&mut s) == v0);
    // Changing the rule changes the parts made with it; other settings leave the shape alone.
    let r = run(&mut s, "sheet.manage_rules", json!({"name": "AL2", "corner_relief_shape": "Round"}));
    assert_eq!((&r["rule"]["corner_relief_shape"], &r["values"]["corner_relief_shape"]), (&json!("round"), &json!("round")));
    let round = volume(&mut s);
    assert!(round < v0 - 200.0, "{v0} -> {round}");
    run(&mut s, "sheet.manage_rules", json!({"name": "AL2", "hem_gap": "0.02 mm"}));
    assert!(shape(&mut s) == "round" && volume(&mut s) == round);
    run(&mut s, "sheet.manage_rules", json!({"name": "AL2", "corner_relief_shape": null, "corner_relief": "6 mm"}));
    assert!(shape(&mut s) == "round" && volume(&mut s) > round);
    for _ in 0..3 {
        run(&mut s, "edit.undo", json!({}));
    }
    assert!(shape(&mut s) == "notch" && volume(&mut s) == v0);
    // A relief longer than the bends is left out and reported on the flange; the part is the notched one.
    run(&mut s, "sheet.manage_rules", json!({"name": "AL2", "corner_relief_shape": "rectangular", "corner_relief": "200 mm"}));
    let w = s.model.results.last().and_then(|r| r.warning.clone()).unwrap_or_default();
    assert_eq!(w.matches("no corner relief at (").count(), 4, "{w}");
    assert!(w.contains("would take a whole bend away") && volume(&mut s) == v0, "{w}");
}

/// A 120 x 90 base in 2 mm aluminium (R 1, K 0.5, gap 0.5; `rule` adds to its settings) with two edge
/// flange features, 20 high at 90°: `near` on the bottom and left edges, which meet at the corner at
/// the origin, and `far` on the top and right edges, which meet at the corner at (120, 90). The two
/// objects hold what each feature is given besides its edges.
fn two_flange_groups(rule: Value, near: Value, far: Value) -> Session {
    let mut s = Session::default();
    let mut r = json!({"name": "AL2", "thickness": "2 mm", "k_factor": 0.5, "bend_radius": "1 mm", "gap": "0.5 mm", "active": true});
    if let (Some(r), Some(extra)) = (r.as_object_mut(), rule.as_object()) {
        r.extend(extra.clone());
    }
    run(&mut s, "sheet.manage_rules", r);
    base(&mut s, 120.0, 90.0);
    for (edges, own) in [(json!([[60, 0, 2], [0, 45, 2]]), near), (json!([[60, 90, 2], [120, 45, 2]]), far)] {
        let mut p = json!({"edges": edges, "height": 20, "angle": "90 deg"});
        if let (Some(p), Some(own)) = (p.as_object_mut(), own.as_object()) {
            p.extend(own.clone());
        }
        run(&mut s, "sheet.flange", p);
    }
    s
}

/// What the DXF of the flat pattern draws of the corner reliefs: the arcs (centre, radius) and the
/// lines' end points.
fn relief_drawing(s: &mut Session) -> (Vec<(solidcraft_geom::Vec2, f64)>, Vec<solidcraft_geom::Vec2>) {
    let dxf = run(s, "sheet.export_dxf", json!({}))["dxf"].as_str().unwrap_or_default().to_string();
    let read = solidcraft_io::read_dxf(&dxf).unwrap_or_default();
    let arcs = read.iter().filter_map(|g| if let solidcraft_io::Geom2::Arc { c, a, .. } = g { Some((*c, a.dist(*c))) } else { None }).collect();
    let ends = read.iter().flat_map(|g| if let solidcraft_io::Geom2::Line(a, b) = g { vec![*a, *b] } else { Vec::new() }).collect();
    (arcs, ends)
}

#[test]
fn each_edge_flange_feature_chooses_the_corner_relief_of_its_own_corners() {
    use solidcraft_geom::Vec2;
    // At 90° the bends of a corner meet at (s0, s0) in from the sheet's edges, s0 = (R + T)·tan 45° = 3,
    // and are BA = π/2·(R + K·T) = π wide. A relief d across reaches r = d/2 along both bends and takes,
    // at its corner, r²·π/4 (round) or r² (rectangular) of the base and r of each bend, 2 mm thick.
    let (s0, ba) = (3.0, PI);
    let at = |ends: &[Vec2], x: f64, y: f64| ends.iter().any(|p| p.dist(Vec2::new(x, y)) < 1e-6);
    // The rule says notch, as it does by default: the corners are not relieved.
    let mut plain = two_flange_groups(json!({}), json!({}), json!({}));
    assert!(no_errors_or_warnings(&plain), "{:?}", plain.model.results);
    let v0 = volume(&mut plain);
    assert!(relief_drawing(&mut plain).0.is_empty());
    // The flanges at the origin ask for a round 6 mm across, those at (120, 90) for a rectangular 3 mm.
    let near = json!({"corner_relief_shape": "round", "corner_relief": "6 mm"});
    let far = json!({"corner_relief_shape": "Rectangular", "corner_relief": "3 mm"});
    let mut s = two_flange_groups(json!({}), near, far);
    assert!(no_errors_or_warnings(&s), "{:?}", s.model.results);
    let listed = run(&mut s, "sheet.rules", json!({}));
    let rule = &listed["rules"][0];
    assert_eq!((&rule["rule"]["corner_relief_shape"], &rule["values"]["corner_relief_shape"]), (&json!("notch"), &json!("notch")));
    // The folded part loses what the two reliefs take.
    let (v, want) = (volume(&mut s), ((PI * 9.0 / 4.0 + 6.0 * ba) + (1.5 * 1.5 + 3.0 * ba)) * 2.0);
    assert!((v0 - v - want).abs() < 0.1, "{v0} - {v} vs {want}");
    // The DXF: one arc of radius 3 about the corner at the origin and nothing round at the other corner;
    // there the relief is a kite ending 1.5 mm in from the corner of the bends, and none at the origin.
    let (arcs, ends) = relief_drawing(&mut s);
    assert!(arcs.len() == 1 && arcs[0].0.dist(Vec2::new(s0, s0)) < 1e-6 && (arcs[0].1 - 3.0).abs() < 1e-6, "{arcs:?}");
    assert!(at(&ends, 120.0 - s0 - 1.5, 90.0 - s0 - 1.5), "the corner at (120, 90) is relieved with a rectangular relief");
    assert!(!at(&ends, s0 + 1.5, s0 + 1.5), "the corner at the origin is not");
    // Unfolding, refolding and reopening the design keep both.
    run(&mut s, "sheet.unfold", json!({}));
    run(&mut s, "sheet.refold", json!({}));
    assert!((volume(&mut s) - v).abs() < 1e-6 * v);
    let mut reopened = Session::new(solidcraft_doc::Document::from_json(&s.doc.to_json()).unwrap());
    assert!((volume(&mut reopened) - v).abs() < 1e-6 * v);
    assert_eq!(relief_drawing(&mut reopened).0.len(), 1);
    // The size is an input of the feature like its height: another size moves the relief.
    let near_feature = s.doc.features.iter().find(|f| matches!(f.kind, solidcraft_doc::FeatureKind::SheetFlange { .. }));
    let names = near_feature.map(|f| f.param_names.clone()).unwrap_or_default();
    assert_eq!(names.len(), 3, "height, angle and the relief: {names:?}");
    run(&mut s, "parameters.change", json!({"name": names[2], "expression": "4 mm"}));
    let arcs = relief_drawing(&mut s).0;
    assert!(arcs.len() == 1 && (arcs[0].1 - 2.0).abs() < 1e-6, "{arcs:?}");
}

#[test]
fn an_edge_flange_feature_follows_the_rule_for_the_relief_it_leaves_out() {
    use solidcraft_geom::Vec2;
    let s0 = 3.0;
    let at = |ends: &[Vec2], x: f64, y: f64| ends.iter().any(|p| p.dist(Vec2::new(x, y)) < 1e-6);
    let round8 = json!({"corner_relief_shape": "round", "corner_relief": "8 mm"});
    // The rule says a round 8 mm across. A feature that asks for nothing gets it, one that asks for a
    // size alone keeps the rule's shape.
    let mut s = two_flange_groups(round8.clone(), json!({"corner_relief": "6 mm"}), json!({}));
    assert!(no_errors_or_warnings(&s), "{:?}", s.model.results);
    let mut arcs = relief_drawing(&mut s).0;
    arcs.sort_by(|a, b| a.1.total_cmp(&b.1));
    assert_eq!(arcs.len(), 2, "{arcs:?}");
    assert!(arcs[0].0.dist(Vec2::new(s0, s0)) < 1e-6 && (arcs[0].1 - 3.0).abs() < 1e-6, "{arcs:?}");
    assert!(arcs[1].0.dist(Vec2::new(120.0 - s0, 90.0 - s0)) < 1e-6 && (arcs[1].1 - 4.0).abs() < 1e-6, "{arcs:?}");
    // A shape alone keeps the rule's size.
    let mut s = two_flange_groups(round8, json!({"corner_relief_shape": "rectangular"}), json!({}));
    assert!(no_errors_or_warnings(&s), "{:?}", s.model.results);
    let (arcs, ends) = relief_drawing(&mut s);
    assert!(arcs.len() == 1 && arcs[0].0.dist(Vec2::new(120.0 - s0, 90.0 - s0)) < 1e-6 && (arcs[0].1 - 4.0).abs() < 1e-6, "{arcs:?}");
    assert!(at(&ends, s0 + 4.0, s0 + 4.0), "a kite 8 mm across at the origin");
    // A change of the rule still reaches the feature that asks for nothing.
    run(&mut s, "sheet.manage_rules", json!({"name": "AL2", "corner_relief": "6 mm"}));
    let arcs = relief_drawing(&mut s).0;
    assert!(arcs.len() == 1 && (arcs[0].1 - 3.0).abs() < 1e-6, "{arcs:?}");
}

#[test]
fn an_edge_flange_feature_can_tear_its_corners_under_a_notch_rule() {
    use std::f64::consts::SQRT_2;
    // Torn, the two bends of a corner run to the corner of the bends and each is cut straight to its
    // flange's end: at 90° it is grow = sin 90° − gap/√2 past the corner, a triangle BA·grow/2 at each of
    // the two bend ends, and a corner less at each (see a_tear_of_the_rule_reaches_the_part_the_flat_pattern_and_the_dxf).
    let (ba, grow) = (PI, 1.0 - 0.5 / SQRT_2);
    let mut notch = two_flange_groups(json!({}), json!({}), json!({}));
    let (v0, stepped) = (volume(&mut notch), outline_points(&flat(&mut notch)));
    let mut s = two_flange_groups(json!({}), json!({"corner_relief_shape": "tear"}), json!({}));
    assert!(no_errors_or_warnings(&s), "{:?}", s.model.results);
    let torn = s.model.state().sheets.last().map(|b| b.flanges.iter().map(|f| f.tear).collect::<Vec<_>>()).unwrap_or_default();
    assert_eq!(torn.iter().filter(|t| **t != (false, false)).count(), 2, "{torn:?}");
    let want = 2.0 * ba * grow / 2.0 * 2.0;
    assert!((volume(&mut s) - v0 - want).abs() < 0.05, "{} - {v0} vs {want}", volume(&mut s));
    assert_eq!(outline_points(&flat(&mut s)).len() + 2, stepped.len());
}

#[test]
fn only_an_edge_flange_takes_a_corner_relief_of_its_own() {
    let mut s = Session::default();
    run(&mut s, "sheet.manage_rules", json!({"name": "AL2", "thickness": "2 mm", "k_factor": 0.5, "bend_radius": "1 mm", "active": true}));
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Base"}));
    run(&mut s, "sketch.rectangle.two_point", json!({"p0": [0, 0], "p1": [120, 90]}));
    run(&mut s, "sketch.finish", json!({}));
    let features = s.doc.features.len();
    // A base flange and a contour flange take their relief from the rule, as before: refused, and
    // the message names the key.
    for (params, key) in [
        (json!({"sketch": "Base", "corner_relief_shape": "round"}), "corner_relief_shape"),
        (json!({"sketch": "Base", "corner_relief": "6 mm"}), "corner_relief"),
        (json!({"type": "base", "sketch": "Base", "corner_relief_shape": null}), "corner_relief_shape"),
        (json!({"sketch": "Base", "curves": ["l1"], "distance": 10, "corner_relief": "6 mm"}), "corner_relief"),
        (json!({"type": "contour", "sketch": "Base", "curves": ["l1"], "distance": 10, "corner_relief_shape": "tear"}), "corner_relief_shape"),
        (json!({"base": {"sketch": "Base", "corner_relief": "6 mm"}}), "corner_relief"),
    ] {
        let e = s.execute("sheet.flange", &params).map(|_| ()).unwrap_err().to_string();
        let today = format!(
            "`{key}` is not a sheet.flange parameter: the sheet metal rule sets it. Make a rule with sheet.manage_rules {{name, {key}, …}} and pass its name as `rule`"
        );
        assert!(e.contains(&today), "{params}: {e}");
    }
    assert_eq!(s.doc.features.len(), features, "a refused flange changes nothing");
    run(&mut s, "sheet.flange", json!({"sketch": "Base"}));
    // The rest of the rule stays with the rule for an edge flange as well, and a relief that is no shape
    // or no length is refused before anything changes.
    let features = s.doc.features.len();
    let edge = |extra: Value| {
        let mut p = json!({"edges": [[60, 0, 2], [0, 45, 2]], "height": 20});
        if let (Some(p), Some(extra)) = (p.as_object_mut(), extra.as_object()) {
            p.extend(extra.clone());
        }
        p
    };
    for (extra, want) in [
        (json!({"thickness": "3 mm"}), "`thickness` is not a sheet.flange parameter"),
        (json!({"relief_width": "1 mm"}), "`relief_width` is not a sheet.flange parameter"),
        (json!({"corner_relief_shape": "star"}), "`corner_relief_shape` must be notch, round, rectangular or tear"),
        (json!({"corner_relief_shape": 2}), "`corner_relief_shape` must be notch, round, rectangular or tear"),
        (json!({"corner_relief": "wide"}), "corner_relief"),
        (json!({"corner_relief": "30 deg"}), "corner_relief"),
    ] {
        let e = s.execute("sheet.flange", &edge(extra.clone())).map(|_| ()).unwrap_err().to_string();
        assert!(e.contains(want), "{extra}: {e}");
    }
    assert_eq!(s.doc.features.len(), features);
    // Null is not given: the corners follow the rule. A shape is stored by its name.
    run(&mut s, "sheet.flange", edge(json!({"corner_relief_shape": null})));
    let mut torn = edge(json!({"corner_relief_shape": " Tear ", "corner_relief": 5}));
    torn["edges"] = json!([[60, 90, 2], [120, 45, 2]]);
    run(&mut s, "sheet.flange", torn);
    let saved: Value = serde_json::from_str(&s.doc.to_json()).unwrap_or_default();
    let own: Vec<(Value, Value)> = saved["features"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter(|f| f["type"] == "sheet_flange")
        .map(|f| (f["corner_relief_shape"].clone(), f["corner_relief"].clone()))
        .collect();
    assert_eq!(own, [(Value::Null, Value::Null), (json!("tear"), json!("5"))]);
}

#[test]
fn a_corner_relief_of_an_edge_flange_is_saved_with_it_and_designs_without_one_save_as_before() {
    let flange_features = |s: &Session| -> Vec<Value> {
        let saved: Value = serde_json::from_str(&s.doc.to_json()).unwrap_or_default();
        saved["features"].as_array().cloned().unwrap_or_default().into_iter().filter(|f| f["type"] == "sheet_flange").collect()
    };
    // Without a relief of its own the feature saves what it always did: neither key.
    let plain = two_flange_groups(json!({}), json!({}), json!({}));
    let features = flange_features(&plain);
    assert_eq!(features.len(), 2);
    assert!(features.iter().all(|f| f.get("corner_relief_shape").is_none() && f.get("corner_relief").is_none()), "{features:?}");
    // With one, the file has it and a design read from it builds the same part.
    let near = json!({"corner_relief_shape": "round", "corner_relief": "6 mm"});
    let mut s = two_flange_groups(json!({}), near, json!({"corner_relief_shape": "tear"}));
    let features = flange_features(&s);
    assert_eq!((&features[0]["corner_relief_shape"], &features[0]["corner_relief"]), (&json!("round"), &json!("6 mm")));
    assert_eq!(features[1]["corner_relief_shape"], "tear");
    assert!(features[1].get("corner_relief").is_none(), "{}", features[1]);
    let saved = s.doc.to_json();
    let mut reopened = Session::new(solidcraft_doc::Document::from_json(&saved).unwrap());
    assert_eq!(reopened.doc.features, s.doc.features);
    assert!((volume(&mut reopened) - volume(&mut s)).abs() < 1e-6);
    assert_eq!(relief_drawing(&mut reopened), relief_drawing(&mut s));
    // timeline.edit changes the relief of a saved feature, and null takes it back to the rule's.
    let id = s.doc.features.iter().rev().nth(1).map_or(0, |f| f.id);
    run(&mut s, "timeline.edit", json!({"feature": id, "set": {"corner_relief_shape": "rectangular"}}));
    assert!(relief_drawing(&mut s).0.is_empty());
    run(&mut s, "timeline.edit", json!({"feature": id, "set": {"corner_relief_shape": null, "corner_relief": null}}));
    assert!(flange_features(&s)[0].get("corner_relief_shape").is_none());
    assert!(flange_features(&s)[0].get("corner_relief").is_none());
}

#[test]
fn corners_left_open_are_reported_on_the_flange() {
    let mut s = Session::default();
    alu_tray(&mut s, 72.0, json!({"gap": "0 mm"}));
    let w = s.model.results.last().and_then(|r| r.warning.clone()).unwrap_or_default();
    assert_eq!(w.matches("left open").count(), 4, "{w}");
    // Open corners still fold, unfold and take cuts.
    run(&mut s, "sheet.unfold", json!({}));
    run(&mut s, "sheet.refold", json!({}));
    assert!(volume(&mut s) > 0.0);
}

/// 2 mm aluminium (R 1, K 0.5, gap 0.5) and a 120 x 90 base flange.
fn alu_base(s: &mut Session) {
    run(
        s,
        "sheet.manage_rules",
        json!({"name": "AL2", "thickness": "2 mm", "k_factor": 0.5, "bend_radius": "1 mm", "gap": "0.5 mm", "active": true}),
    );
    base(s, 120.0, 90.0);
}

/// The base's four edges (bottom, right, top, left) in one feature: 75° and 38.6 high from the
/// feature, then 90°, 105° at 30 high and 120° at 25 high from the edges themselves.
fn four_angles_in_one_feature() -> Session {
    let mut s = Session::default();
    alu_base(&mut s);
    let edges = json!([
        [60, 0, 2],
        {"point": [120, 45, 2], "angle": "90 deg"},
        {"point": [60, 90, 2], "angle": 105, "height": "30 mm"},
        {"point": {"x": 0, "y": 45, "z": 2}, "angle": "120 deg", "height": 25}
    ]);
    run(&mut s, "sheet.flange", json!({"edges": edges, "height": 38.6, "angle": "75 deg"}));
    s
}

/// The flat outline's points.
fn outline_points(f: &Value) -> Vec<(f64, f64)> {
    let loops = f["outline"].as_array().cloned().unwrap_or_default();
    loops
        .iter()
        .flat_map(|l| l.as_array().cloned().unwrap_or_default())
        .map(|q| (q[0].as_f64().unwrap_or(f64::NAN), q[1].as_f64().unwrap_or(f64::NAN)))
        .collect()
}

/// Two point lists are the same set of points, to 1e-6.
fn same_points(a: &[(f64, f64)], b: &[(f64, f64)]) -> bool {
    a.len() == b.len() && a.iter().all(|p| b.iter().any(|q| (p.0 - q.0).abs() < 1e-6 && (p.1 - q.1).abs() < 1e-6))
}

fn bend_angles(s: &mut Session) -> Vec<f64> {
    flat(s)["bends"].as_array().cloned().unwrap_or_default().iter().map(|b| b["angle_deg"].as_f64().unwrap_or(f64::NAN).round()).collect()
}

fn no_errors_or_warnings(s: &Session) -> bool {
    s.model.results.iter().all(|r| r.error.is_none() && r.warning.is_none())
}

/// The flanges of [`four_angles_in_one_feature`] (bottom, right, top, left): angles in degrees
/// and heights.
const FOUR_ANGLES: [f64; 4] = [75.0, 90.0, 105.0, 120.0];
const FOUR_HEIGHTS: [f64; 4] = [38.6, 38.6, 30.0, 25.0];

/// That tray's panels in the flat pattern by hand (T 2, R 1, K 0.5, gap g 0.5; the derivation
/// is with the doc tests). Folded, a flange at θ whose neighbour at a corner is at φ ends
/// x(w) = ((g/2)·√(2 − 2·cosθ·cosφ) + T·(1 − cosθ·cosφ) − w·sinθ·cosφ) / sinφ along its edge from
/// that corner, w up its outer face from the virtual sharp. In the flat that point lies
/// s0 − BA − (w − s0) into the base from the edge, with s0 = (R + T)·tan(θ/2) the setback and
/// BA = θ·(R + K·T) the bend allowance; the panel spans w = s0 … the flange's height.
fn hand_panels() -> Vec<Vec<(f64, f64)>> {
    // Each edge's start corner, its direction, the direction into the base, and its length.
    let frames: [((f64, f64), (f64, f64), (f64, f64), f64); 4] = [
        ((0.0, 0.0), (1.0, 0.0), (0.0, 1.0), 120.0),
        ((120.0, 0.0), (0.0, 1.0), (-1.0, 0.0), 90.0),
        ((120.0, 90.0), (-1.0, 0.0), (0.0, -1.0), 120.0),
        ((0.0, 90.0), (0.0, -1.0), (1.0, 0.0), 90.0),
    ];
    (0..4)
        .map(|i| {
            let (o, d, v, len) = frames[i];
            let (th, h) = (FOUR_ANGLES[i].to_radians(), FOUR_HEIGHTS[i]);
            let (s0, ba) = (3.0 * (th / 2.0).tan(), th * 2.0);
            let x_end = |phi: f64, w: f64| {
                let c = th.cos() * phi.cos();
                (0.25 * (2.0 - 2.0 * c).sqrt() + 2.0 * (1.0 - c) - w * th.sin() * phi.cos()) / phi.sin()
            };
            let (prev, next) = (FOUR_ANGLES[(i + 3) % 4].to_radians(), FOUR_ANGLES[(i + 1) % 4].to_radians());
            let at = |u: f64, w: f64| {
                let into = s0 - ba - (w - s0);
                (o.0 + d.0 * u + v.0 * into, o.1 + d.1 * u + v.1 * into)
            };
            vec![at(x_end(prev, s0), s0), at(x_end(prev, h), h), at(len - x_end(next, h), h), at(len - x_end(next, s0), s0)]
        })
        .collect()
}

/// A polygon's area.
fn polygon_area(p: &[(f64, f64)]) -> f64 {
    let n = p.len();
    (0..n).map(|i| p[i].0 * p[(i + 1) % n].1 - p[(i + 1) % n].0 * p[i].1).sum::<f64>().abs() / 2.0
}

#[test]
fn edge_flanges_take_their_own_angle_and_height_per_edge() {
    let mut one = four_angles_in_one_feature();
    assert!(no_errors_or_warnings(&one), "{:?}", one.model.results);
    let f1 = flat(&mut one);
    // The bends: each edge's own angle, as long as the base's edge between its neighbours' bend
    // start lines (each s0 in from the original edge).
    assert_eq!(bend_angles(&mut one), FOUR_ANGLES);
    let s0 = FOUR_ANGLES.map(|a| 3.0 * (a.to_radians() / 2.0).tan());
    let ba = FOUR_ANGLES.map(|a| a.to_radians() * 2.0);
    let (w, h) = (120.0 - s0[3] - s0[1], 90.0 - s0[0] - s0[2]);
    let lengths: Vec<f64> = f1["bends"].as_array().cloned().unwrap_or_default().iter().map(|b| b["length"].as_f64().unwrap_or(f64::NAN)).collect();
    assert_eq!(lengths.len(), 4, "{f1}");
    for (got, want) in lengths.iter().zip([w, h, w, h]) {
        assert!((got - want).abs() < 1e-6, "{lengths:?} vs {w} and {h}");
    }
    // The flat pattern is one outline through every hand-derived panel corner.
    let (outline, panels) = (outline_points(&f1), hand_panels());
    assert_eq!(f1["outline"].as_array().map(Vec::len), Some(1), "{f1}");
    for (i, panel) in panels.iter().enumerate() {
        for &(x, y) in panel {
            assert!(outline.iter().any(|p| (p.0 - x).abs() < 1e-6 && (p.1 - y).abs() < 1e-6), "flange {i}: ({x}, {y}) not in {outline:?}");
        }
    }
    // It holds the base, the four bend zones and the four panels, nothing more or less, and its
    // size is the panels' extent.
    let area = w * h + (ba[0] + ba[2]) * w + (ba[1] + ba[3]) * h + panels.iter().map(|p| polygon_area(p)).sum::<f64>();
    assert!(rel(polygon_area(&outline), area) < 1e-9, "{} vs {area}", polygon_area(&outline));
    let all = panels.concat();
    let span = |k: fn(&(f64, f64)) -> f64| all.iter().map(k).fold(f64::MIN, f64::max) - all.iter().map(k).fold(f64::MAX, f64::min);
    for (k, want) in [span(|p| p.0), span(|p| p.1), 2.0].into_iter().enumerate() {
        assert!((f1["flat_size_mm"][k].as_f64().unwrap_or(f64::NAN) - want).abs() < 1e-6, "{}: {want}", f1["flat_size_mm"]);
    }
    // Folded, the part holds that material (K = 0.5 keeps the volume).
    let v1 = volume(&mut one);
    assert!(rel(v1, area * 2.0) < 1e-3, "{v1} vs {}", area * 2.0);
    // The DXF: the outline in straight lines, and four bend lines.
    let dxf = run(&mut one, "sheet.export_dxf", json!({}))["dxf"].as_str().unwrap_or_default().to_string();
    assert_eq!(dxf.matches("0\nLINE\n").count(), outline.len() + 4, "{} outline points", outline.len());
    // Unfold and refold keep it.
    run(&mut one, "sheet.unfold", json!({}));
    assert!(same_points(&outline_points(&flat(&mut one)), &outline_points(&f1)));
    run(&mut one, "sheet.refold", json!({}));
    assert!(rel(volume(&mut one), v1) < 1e-9);
    // Saved, an edge with its own values is an object and one without is the point it always was.
    let text = one.doc.to_json();
    let saved: Value = serde_json::from_str(&text).unwrap();
    let flange = saved["features"]
        .as_array()
        .and_then(|f| f.iter().find(|f| f["type"] == "sheet_flange" && f["edges"].as_array().is_some_and(|e| e.len() == 4)));
    assert_eq!(
        flange.map(|f| f["edges"].clone()),
        Some(json!([
            [60.0, 0.0, 2.0],
            {"point": [120.0, 45.0, 2.0], "angle": "90 deg"},
            {"point": [60.0, 90.0, 2.0], "angle": "105", "height": "30 mm"},
            {"point": [0.0, 45.0, 2.0], "angle": "120 deg", "height": "25"}
        ])),
        "{saved}"
    );
    // Reopened, it is the same part.
    let mut again = Session::new(solidcraft_doc::Document::from_json(&text).unwrap());
    assert!(no_errors_or_warnings(&again), "{:?}", again.model.results);
    assert!(rel(volume(&mut again), v1) < 1e-9);
}

#[test]
fn per_edge_values_are_expressions_and_feature_inputs() {
    let build = |angle: &str, height: &str| {
        let mut s = Session::default();
        alu_base(&mut s);
        run(&mut s, "parameters.add", json!({"name": "wall_angle", "expression": "105 deg"}));
        run(&mut s, "parameters.add", json!({"name": "wall_height", "expression": "30 mm"}));
        let edges = json!([[60, 0, 2], {"point": [120, 45, 2], "angle": "90 deg"}, {"point": [60, 90, 2], "angle": angle, "height": height}]);
        run(&mut s, "sheet.flange", json!({"edges": edges, "height": 38.6, "angle": "75 deg"}));
        assert!(no_errors_or_warnings(&s), "{:?}", s.model.results);
        s
    };
    let part = |s: &mut Session| (volume(s), outline_points(&flat(s)));
    let mut s = build("wall_angle", "wall_height");
    // The feature's inputs: its own, then each edge's, in the order of the edges.
    let labels: Vec<String> =
        s.doc.features.last().map(|f| f.kind.inputs().into_iter().map(|(l, e, _)| format!("{l}={e}")).collect()).unwrap_or_default();
    assert_eq!(labels, ["Height=38.6", "Angle=75 deg", "Edge angle=90 deg", "Edge angle=wall_angle", "Edge height=wall_height"]);
    // Changing the user parameters moves that flange.
    run(&mut s, "parameters.change", json!({"name": "wall_angle", "expression": "110 deg"}));
    run(&mut s, "parameters.change", json!({"name": "wall_height", "expression": "34 mm"}));
    let (v, outline) = part(&mut build("110 deg", "34 mm"));
    assert!(rel(volume(&mut s), v) < 1e-9, "{} vs {v}", volume(&mut s));
    assert!(same_points(&outline_points(&flat(&mut s)), &outline));
    // So does the feature's own input for an edge, by its parameter name (the last two).
    let names = s.doc.features.last().map(|f| f.param_names.clone()).unwrap_or_default();
    assert_eq!(names.len(), 5, "{names:?}");
    run(&mut s, "parameters.change", json!({"name": names[3], "expression": "100 deg"}));
    run(&mut s, "parameters.change", json!({"name": names[4], "expression": "36 mm"}));
    let (v, outline) = part(&mut build("100 deg", "36 mm"));
    assert!(rel(volume(&mut s), v) < 1e-9, "{} vs {v}", volume(&mut s));
    assert!(same_points(&outline_points(&flat(&mut s)), &outline));
    assert!(no_errors_or_warnings(&s), "{:?}", s.model.results);
}

/// The bends' lengths in the flat pattern, in the order the bends were made.
fn bend_lengths(s: &mut Session) -> Vec<f64> {
    flat(s)["bends"].as_array().cloned().unwrap_or_default().iter().map(|b| b["length"].as_f64().unwrap_or(f64::NAN)).collect()
}

/// A tray on a 120 wide, 90 deep base whose top edge is `top` wide (120: a rectangle), in 2 mm
/// aluminium (R 1, K 0.5) with the flange gap `gap`: walls 20 high at `deg` in one feature and,
/// in the next feature, a return 12 high at 180° − `deg` on the far edge of each of the walls
/// `returns` (0…3: bottom, right, top, left; none: no feature), picked on the walls' inner faces,
/// so that the returns turn in over the base and lie level.
fn tray_with_returns(top: f64, deg: f64, gap: &str, returns: &[usize]) -> Session {
    let mut s = Session::default();
    run(
        &mut s,
        "sheet.manage_rules",
        json!({"name": "AL2", "thickness": "2 mm", "k_factor": 0.5, "bend_radius": "1 mm", "gap": gap, "active": true}),
    );
    let inset = (120.0 - top) / 2.0;
    let pts = [(0.0, 0.0), (120.0, 0.0), (120.0 - inset, 90.0), (inset, 90.0)];
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Base"}));
    run(&mut s, "sketch.line", json!({"points": pts.map(|p| [p.0, p.1]), "closed": true}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "sheet.flange", json!({"sketch": "Base"}));
    // A wall's inner face 20 up its outer face from the base's edge: that far in from the edge
    // and that high (see the doc tests).
    let th = deg.to_radians();
    let (into, up) = (-20.0 * th.cos() + 2.0 * th.sin(), 20.0 * th.sin() + 2.0 * th.cos());
    let pick = |k: usize, into: f64, z: f64| {
        let (a, b) = (pts[k], pts[(k + 1) % 4]);
        let l = (b.0 - a.0).hypot(b.1 - a.1);
        json!([(a.0 + b.0) / 2.0 - (b.1 - a.1) / l * into, (a.1 + b.1) / 2.0 + (b.0 - a.0) / l * into, z])
    };
    let edges: Vec<Value> = (0..4).map(|k| pick(k, 0.0, 2.0)).collect();
    run(&mut s, "sheet.flange", json!({"edges": edges, "height": 20, "angle": format!("{deg} deg")}));
    if !returns.is_empty() {
        let edges: Vec<Value> = returns.iter().map(|&k| pick(k, into, up)).collect();
        run(&mut s, "sheet.flange", json!({"edges": edges, "height": 12, "angle": format!("{} deg", 180.0 - deg)}));
    }
    s
}

#[test]
fn returns_on_a_tray_close_at_its_corners() {
    use std::f64::consts::SQRT_2;
    // Walls at 90° with returns at 90°. The returns' feature used to fail here ("sew: nonmanifold
    // edge contact"): each return took its wall's bend length and ran through its neighbours.
    let mut s = tray_with_returns(120.0, 90.0, "0.5 mm", &[0, 1, 2, 3]);
    assert!(no_errors_or_warnings(&s), "{:?}", s.model.results);
    // By hand (the derivation is with the doc tests; e = g/√2). The part fills 120 x 90 x 20.
    let e = 0.5 / SQRT_2;
    let b = bbox(&mut s);
    for (k, want) in [120.0, 90.0, 20.0].into_iter().enumerate() {
        let (lo, hi) = (b["min"][k].as_f64().unwrap_or(f64::NAN), b["max"][k].as_f64().unwrap_or(f64::NAN));
        assert!(lo.abs() < 1e-6 && (hi - want).abs() < 1e-6, "{b}");
    }
    // The walls' bends are the base's trimmed edges; the returns' bends stop g/√2 short of the
    // mitre x = y through (3, 3) at both ends.
    let f = flat(&mut s);
    let lengths = bend_lengths(&mut s);
    assert_eq!(lengths.len(), 8, "{f}");
    for (got, want) in lengths.iter().zip([114.0, 84.0, 114.0, 84.0, 114.0 - 2.0 * e, 84.0 - 2.0 * e, 114.0 - 2.0 * e, 84.0 - 2.0 * e]) {
        assert!((got - want).abs() < 1e-6, "{lengths:?}");
    }
    // The flat pattern is one outline through the corners of the return's panel on the wall on
    // y = 0: 9 high past two bend zones π wide and the wall's 14, mitred at 45° at both ends.
    let outline = outline_points(&f);
    assert_eq!(f["outline"].as_array().map(Vec::len), Some(1), "{f}");
    let (y0, y1) = (-11.0 - 2.0 * PI, -20.0 - 2.0 * PI);
    for (x, y) in [(3.0 + e, y0), (117.0 - e, y0), (108.0 - e, y1), (12.0 + e, y1)] {
        assert!(outline.iter().any(|p| (p.0 - x).abs() < 1e-6 && (p.1 - y).abs() < 1e-6), "({x}, {y}) not in {outline:?}");
    }
    // It holds the base, the walls' bends and panels and the returns' bends and panels, nothing
    // more or less, and is 20 + 2π wider than the base on every side.
    let area = 114.0 * 84.0 + 396.0 * PI + 28.0 * (202.0 - 4.0 * e) + 2.0 * PI * (198.0 - 4.0 * e) + 18.0 * (180.0 - 4.0 * e);
    assert!(rel(polygon_area(&outline), area) < 1e-9, "{} vs {area}", polygon_area(&outline));
    for (k, want) in [160.0 + 4.0 * PI, 130.0 + 4.0 * PI, 2.0].into_iter().enumerate() {
        assert!((f["flat_size_mm"][k].as_f64().unwrap_or(f64::NAN) - want).abs() < 1e-6, "{}: {want}", f["flat_size_mm"]);
    }
    // Folded, the part holds that material (K = 0.5 keeps the volume).
    let v = volume(&mut s);
    assert!(rel(v, area * 2.0) < 1e-3, "{v} vs {}", area * 2.0);
    // The DXF: the outline in straight lines, and eight bend lines.
    let dxf = run(&mut s, "sheet.export_dxf", json!({}))["dxf"].as_str().unwrap_or_default().to_string();
    assert_eq!(dxf.matches("0\nLINE\n").count(), outline.len() + 8, "{} outline points", outline.len());
    // Unfold and refold keep it.
    run(&mut s, "sheet.unfold", json!({}));
    assert!(same_points(&outline_points(&flat(&mut s)), &outline));
    run(&mut s, "sheet.refold", json!({}));
    assert!(rel(volume(&mut s), v) < 1e-9);
    // Reopened, it is the same part.
    let mut again = Session::new(solidcraft_doc::Document::from_json(&s.doc.to_json()).unwrap());
    assert!(no_errors_or_warnings(&again), "{:?}", again.model.results);
    assert!(rel(volume(&mut again), v) < 1e-9);
    assert!(same_points(&outline_points(&flat(&mut again)), &outline));
}

#[test]
fn returns_on_leaning_walls_stay_inside_the_walls() {
    // A trapezoid (corners of 77.5° and 102.5°) with walls at 110° and level returns at 70°. Each
    // return used to run its wall's bend length from where the wall's far edge started, out
    // through the neighbouring wall: the part reached x = 120.588 where its walls end at 117.587.
    let mut s = tray_with_returns(80.0, 110.0, "0.5 mm", &[0, 1, 2, 3]);
    assert!(no_errors_or_warnings(&s), "{:?}", s.model.results);
    // The returns lie inside the walls and no higher than their tips: the part's box is the
    // walls' own.
    let (with, without) = (bbox(&mut s), bbox(&mut tray_with_returns(80.0, 110.0, "0.5 mm", &[])));
    for end in ["min", "max"] {
        for k in 0..3 {
            let (a, b) = (with[end][k].as_f64().unwrap_or(f64::NAN), without[end][k].as_f64().unwrap_or(f64::NAN));
            assert!((a - b).abs() < 1e-6, "{with} vs {without}");
        }
    }
    // By hand (the doc tests): a return's panel starts v = 3·sin 110° + (20 − 3·tan 35°)·cos 70°
    // in from its wall's edge, and at a base corner of angle α its bend stops
    // x_b = (g/2 + v·cos(α/2)) / sin(α/2) along the edge from the corner.
    let th = 110f64.to_radians();
    let v = 3.0 * th.sin() - (20.0 - 3.0 * 35f64.to_radians().tan()) * th.cos();
    let x_b = |alpha: f64| (0.25 + v * (alpha / 2.0).cos()) / (alpha / 2.0).sin();
    let (acute, side) = ((90.0f64 / 20.0).atan(), 20f64.hypot(90.0));
    let want = [120.0 - 2.0 * x_b(acute), side - x_b(acute) - x_b(PI - acute), 80.0 - 2.0 * x_b(PI - acute), side - x_b(acute) - x_b(PI - acute)];
    let lengths = bend_lengths(&mut s);
    assert_eq!(lengths.len(), 8, "{lengths:?}");
    for (got, want) in lengths[4..].iter().zip(want) {
        assert!((got - want).abs() < 1e-6, "{lengths:?} vs {want:?}");
    }
    // One outline in the flat, and the folded part holds its material.
    let f = flat(&mut s);
    assert_eq!(f["outline"].as_array().map(Vec::len), Some(1), "{f}");
    let (outline, vol) = (outline_points(&f), volume(&mut s));
    assert!(rel(vol, polygon_area(&outline) * 2.0) < 1e-3, "{vol} vs {}", polygon_area(&outline) * 2.0);
    run(&mut s, "sheet.unfold", json!({}));
    assert!(same_points(&outline_points(&flat(&mut s)), &outline));
    run(&mut s, "sheet.refold", json!({}));
    assert!(rel(volume(&mut s), vol) < 1e-9);
}

#[test]
fn a_return_alone_on_a_leaning_wall_keeps_out_of_the_walls_beside_it() {
    use std::f64::consts::SQRT_2;
    // The tray with walls at 110° and a level return on the wall on y = 0 only. It used to take
    // that wall's bend length (120 − 2·3·tan 55° = 111.431) from where the wall's far edge starts
    // and came out through the wall on x = 120: the part reached x = 120.504 where its walls end
    // at 118.716.
    let mut s = tray_with_returns(120.0, 110.0, "0.5 mm", &[0]);
    assert!(no_errors_or_warnings(&s), "{:?}", s.model.results);
    let (with, without) = (bbox(&mut s), bbox(&mut tray_with_returns(120.0, 110.0, "0.5 mm", &[])));
    for end in ["min", "max"] {
        for k in 0..3 {
            let (a, b) = (with[end][k].as_f64().unwrap_or(f64::NAN), without[end][k].as_f64().unwrap_or(f64::NAN));
            assert!((a - b).abs() < 1e-6, "{with} vs {without}");
        }
    }
    // By hand (the doc tests): the walls beside it lean in over it and come nearest at its top,
    // 20·sin 110° up, where their inner faces are 2/sin 110° + 20·cos 70° in from the base's edges.
    // The return stops square g/√2 short of them at both ends: 120 − 2·9.322312 = 101.355376.
    let th = 110f64.to_radians();
    let want = 120.0 - 2.0 * (0.5 / SQRT_2 + 2.0 / th.sin() - 20.0 * th.cos());
    let lengths = bend_lengths(&mut s);
    assert!(lengths.len() == 5 && (lengths[4] - want).abs() < 1e-6 && (want - 101.355376).abs() < 1e-6, "{lengths:?} vs {want}");
    // One outline in the flat, and the folded part holds its material.
    let f = flat(&mut s);
    assert_eq!(f["outline"].as_array().map(Vec::len), Some(1), "{f}");
    let (outline, vol) = (outline_points(&f), volume(&mut s));
    assert!(rel(vol, polygon_area(&outline) * 2.0) < 1e-3, "{vol} vs {}", polygon_area(&outline) * 2.0);
}

#[test]
fn returns_follow_the_rules_gap_and_stay_open_with_a_warning_when_it_is_zero() {
    use std::f64::consts::SQRT_2;
    // The mitre keeps the rule's gap: at 90° the returns' bends are 114 − 2·g/√2 and 84 − 2·g/√2.
    let mut s = tray_with_returns(120.0, 90.0, "0.5 mm", &[0, 1, 2, 3]);
    for g in [0.5, 1.0, 0.2] {
        run(&mut s, "sheet.manage_rules", json!({"name": "AL2", "gap": format!("{g} mm")}));
        assert!(no_errors_or_warnings(&s), "{g}: {:?}", s.model.results);
        let lengths = bend_lengths(&mut s);
        assert_eq!(lengths.len(), 8, "{g}: {lengths:?}");
        for (got, want) in lengths[4..].iter().zip([114.0, 84.0, 114.0, 84.0]) {
            assert!((got - (want - 2.0 * g / SQRT_2)).abs() < 1e-6, "{g}: {lengths:?}");
        }
    }
    // A gap of zero can't be kept: the walls' corners and the returns' corners are left open,
    // each feature with its warning, and each return spans its wall's far edge (here the wall's
    // bend, 120 − 2·3·tan 55°).
    let mut s = tray_with_returns(120.0, 110.0, "0 mm", &[0, 1, 2, 3]);
    let warnings: Vec<String> = s.model.results.iter().filter_map(|r| r.warning.clone()).collect();
    assert!(s.model.results.iter().all(|r| r.error.is_none()) && warnings.len() == 2, "{:?}", s.model.results);
    assert!(warnings.iter().all(|w| w.matches("left open").count() == 4), "{warnings:?}");
    let lengths = bend_lengths(&mut s);
    assert!((lengths[4] - lengths[0]).abs() < 1e-9 && (lengths[0] - (120.0 - 6.0 * 55f64.to_radians().tan())).abs() < 1e-6, "{lengths:?}");
}

#[test]
fn bad_per_edge_input_is_refused_and_plain_edges_stay_plain() {
    let mut s = Session::default();
    alu_base(&mut s);
    let err = |s: &mut Session, edges: Value| {
        s.execute("sheet.flange", &json!({"edges": edges, "height": 38.6, "angle": "90 deg"})).err().map(|e| e.to_string()).unwrap_or_default()
    };
    // The wrong kind of value, or no expression at all: the edge and the key are named.
    let e = err(&mut s, json!([[60, 0, 2], {"point": [120, 45, 2], "angle": "10 mm"}]));
    assert!(e.contains("edges[1].angle"), "{e}");
    let e = err(&mut s, json!([{"point": [60, 0, 2], "height": "30 deg"}]));
    assert!(e.contains("edges[0].height"), "{e}");
    let e = err(&mut s, json!([{"point": [60, 0, 2], "angle": "nope"}]));
    assert!(e.contains("edges[0].angle"), "{e}");
    let e = err(&mut s, json!([{"point": [60, 0, 2], "angle": [1, 2]}]));
    assert!(e.contains("edges[0]") && e.contains("`angle` must be a number or an expression"), "{e}");
    // An unknown key, a point that is none, an angle without `point`, something that is no edge.
    let e = err(&mut s, json!([{"point": [60, 0, 2], "twist": 3}]));
    assert!(e.contains("edges[0]") && e.contains("unknown key `twist`"), "{e}");
    let e = err(&mut s, json!([{"point": [60, 0], "angle": "60 deg"}]));
    assert!(e.contains("edges[0]") && e.contains("`point` must be [x, y, z]"), "{e}");
    let e = err(&mut s, json!([{"x": 60, "y": 0, "z": 2, "angle": "60 deg"}]));
    assert!(e.contains("edges[0]") && e.contains("unknown key"), "{e}");
    let e = err(&mut s, json!([{"angle": "60 deg"}]));
    assert!(e.contains("edges[0]") && e.contains("`point` must be [x, y, z]"), "{e}");
    let e = err(&mut s, json!(["edge"]));
    assert!(e.contains("edges[0]"), "{e}");
    let e = err(&mut s, json!([]));
    assert!(e.contains("1…1000"), "{e}");
    // Evaluated: an angle outside 0…180° and a height under the bend's, each naming its edge.
    let e = err(&mut s, json!([[60, 0, 2], {"point": [120, 45, 2], "angle": "200 deg"}]));
    assert!(e.contains("edge 2") && e.contains("angle must be between 0 and 180"), "{e}");
    let e = err(&mut s, json!([{"point": [60, 0, 2], "height": "2 mm"}, [120, 45, 2]]));
    assert!(e.contains("edge 1") && e.contains("height must be at least"), "{e}");
    // None of it added a feature.
    assert_eq!(s.doc.features.len(), 2, "{:?}", s.doc.features.iter().map(|f| f.name.clone()).collect::<Vec<_>>());
    // Plain points, `{x, y, z}` points and `{point}` without values are saved as the points they were.
    run(&mut s, "sheet.flange", json!({"edges": [[60, 0, 2], {"x": 120, "y": 45, "z": 2}, {"point": [60, 90, 2]}], "height": 20, "angle": "90 deg"}));
    let edges = serde_json::to_value(s.doc.features.last().map(|f| f.kind.clone())).unwrap()["edges"].clone();
    assert_eq!(edges, json!([[60.0, 0.0, 2.0], [120.0, 45.0, 2.0], [60.0, 90.0, 2.0]]));
}

#[test]
fn editing_a_flange_keeps_each_edges_own_values_with_its_edge() {
    let mut s = four_angles_in_one_feature();
    let id = s.doc.features.last().map(|f| f.id).unwrap_or(0);
    // Give the first edge 80° and take the 90° off the second.
    let edges = json!([
        {"point": [60, 0, 2], "angle": "80 deg"},
        [120, 45, 2],
        {"point": [60, 90, 2], "angle": "105 deg", "height": "30 mm"},
        {"point": [0, 45, 2], "angle": "120 deg", "height": "25 mm"}
    ]);
    run(&mut s, "timeline.edit", json!({"feature": id, "set": {"edges": edges}}));
    assert!(no_errors_or_warnings(&s), "{:?}", s.model.results);
    assert_eq!(bend_angles(&mut s), [80.0, 75.0, 105.0, 120.0]);
    let kind = |s: &Session| serde_json::to_value(s.doc.features.last().map(|f| f.kind.clone())).unwrap();
    assert_eq!(kind(&s)["edges"][1], json!([120.0, 45.0, 2.0]), "no value of its own: a plain point again");
    let (v, outline) = (volume(&mut s), outline_points(&flat(&mut s)));
    // The same edges in another order are the same part: each value follows its edge.
    let edges = json!([
        {"point": [0, 45, 2], "angle": "120 deg", "height": "25 mm"},
        {"point": [60, 90, 2], "angle": "105 deg", "height": "30 mm"},
        {"point": [60, 0, 2], "angle": "80 deg"},
        [120, 45, 2]
    ]);
    run(&mut s, "timeline.edit", json!({"feature": id, "set": {"edges": edges}}));
    assert!(no_errors_or_warnings(&s), "{:?}", s.model.results);
    assert_eq!(bend_angles(&mut s), [120.0, 105.0, 80.0, 75.0]);
    assert!(rel(volume(&mut s), v) < 1e-9, "{} vs {v}", volume(&mut s));
    assert!(same_points(&outline_points(&flat(&mut s)), &outline));
    // A bad edit is refused and leaves the feature as it was.
    let before = kind(&s);
    assert!(s.execute("timeline.edit", &json!({"feature": id, "set": {"edges": [{"point": [60, 0, 2], "angle": "80 deg", "colour": 1}]}})).is_err());
    assert!(s.execute("timeline.edit", &json!({"feature": id, "set": {"edges": [{"point": [60, 0, 2], "angle": "250 deg"}]}})).is_err());
    assert_eq!(kind(&s), before);
    assert_eq!(bend_angles(&mut s), [120.0, 105.0, 80.0, 75.0]);
}

#[test]
fn designs_without_per_edge_values_are_read_and_written_as_before() {
    let mut s = Session::default();
    alu_base(&mut s);
    run(&mut s, "sheet.flange", json!({"edges": [[60, 0, 2], [120, 45, 2], [60, 90, 2], [0, 45, 2]], "height": 38.6, "angle": "72 deg"}));
    let v = volume(&mut s);
    let text = s.doc.to_json();
    let mut design: Value = serde_json::from_str(&text).unwrap();
    let flange = design["features"].as_array().and_then(|f| f.iter().position(|f| f["type"] == "sheet_flange")).unwrap();
    assert_eq!(design["features"][flange]["edges"], json!([[60.0, 0.0, 2.0], [120.0, 45.0, 2.0], [60.0, 90.0, 2.0], [0.0, 45.0, 2.0]]));
    let mut same = Session::new(solidcraft_doc::Document::from_json(&text).unwrap());
    assert!(rel(volume(&mut same), v) < 1e-12);
    // A design file whose last edge carries its own angle is the part the command makes.
    design["features"][flange]["edges"][3] = json!({"point": [0.0, 45.0, 2.0], "angle": "100 deg"});
    let mut opened = Session::new(solidcraft_doc::Document::from_json(&design.to_string()).unwrap());
    assert!(no_errors_or_warnings(&opened), "{:?}", opened.model.results);
    let mut made = Session::default();
    alu_base(&mut made);
    let edges = json!([[60, 0, 2], [120, 45, 2], [60, 90, 2], {"point": [0, 45, 2], "angle": "100 deg"}]);
    run(&mut made, "sheet.flange", json!({"edges": edges, "height": 38.6, "angle": "72 deg"}));
    let (vo, vm) = (volume(&mut opened), volume(&mut made));
    assert!(rel(vo, vm) < 1e-9 && rel(vo, v) > 1e-4, "{vo} vs {vm} vs {v}");
    // A key this build doesn't know is refused, not dropped.
    design["features"][flange]["edges"][3] = json!({"point": [0.0, 45.0, 2.0], "twist": "3 deg"});
    assert!(solidcraft_doc::Document::from_json(&design.to_string()).is_err());
}

/// Names of the hole's side faces (curved, smaller than the bend faces) in `Body1`.
fn hole_face_names(s: &mut Session) -> Vec<String> {
    let faces = run(s, "model.faces", json!({"body": "Body1"}));
    let faces = faces["faces"].as_array().cloned().unwrap_or_default();
    faces
        .iter()
        .filter(|f| f["plane_normal"].is_null() && f["area"].as_f64().is_some_and(|a| a < 100.0))
        .map(|f| f["name"].as_str().unwrap_or_default().to_string())
        .collect()
}

#[test]
fn a_hole_on_a_folded_panel_keeps_the_name_of_its_sketch_circle() {
    // A Ø10 hole at x = 20 on a 100 x 60 plate; the fold at x = 40 stands that (smaller) side up.
    let mut s = Session::default();
    base(&mut s, 100.0, 60.0);
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Hole"}));
    let circle = run(&mut s, "sketch.circle.center", json!({"center": [20, 30], "radius": 5}));
    let curve = circle["curves"][0].as_str().unwrap_or("c1").to_string();
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.extrude", json!({"sketch": "Hole", "through_all": true, "operation": "cut", "direction": "symmetric"}));
    let side = format!(":side:{curve}#");
    let flat = hole_face_names(&mut s);
    assert_eq!(flat.len(), 2, "{flat:?}");
    assert!(flat.iter().all(|n| n.contains(&side)), "before the fold: {flat:?}");
    fold_line(&mut s);
    run(&mut s, "sheet.fold", json!({"sketch": "FoldLine", "curve": "fl"}));
    assert!(bbox(&mut s)["max"][2].as_f64().unwrap_or(0.0) > 20.0, "the hole's panel stands up");
    let folded = hole_face_names(&mut s);
    assert_eq!(folded.len(), 2, "{folded:?}");
    assert!(folded.iter().all(|n| n.contains(&side)), "after the fold the hole is still circle {curve}: {folded:?} (before: {flat:?})");
}

#[test]
fn a_box_cut_straight_through_a_flat_sheet_is_a_cutout_like_the_extrude_cut() {
    // A 10 x 10 box perpendicular to the sheet and through its whole thickness removes exactly
    // what a sketch rectangle extruded through all removes, which is accepted today.
    let mut s = Session::default();
    base(&mut s, 100.0, 60.0);
    run(&mut s, "solid.box", json!({"corner": [20, 20, -1], "length": 10, "width": 10, "height": 5, "operation": "cut"}));
    let mut extruded = Session::default();
    base(&mut extruded, 100.0, 60.0);
    rectangle_tool(&mut extruded, "Hole", [20.0, 20.0], [30.0, 30.0]);
    run(&mut extruded, "solid.extrude", json!({"sketch": "Hole", "through_all": true, "direction": "symmetric", "operation": "cut"}));
    for session in [&mut s, &mut extruded] {
        assert!((volume(session) - 14750.0).abs() < 1e-6);
        assert_eq!(flat(session)["cutouts"].as_array().map(Vec::len), Some(1));
    }
    run(&mut s, "sheet.unfold", json!({}));
    run(&mut s, "sheet.refold", json!({}));
    assert!((volume(&mut s) - 14750.0).abs() < 1e-6);
    // A cylinder along the sheet normal is a round hole, exact in the flat pattern and DXF.
    run(&mut s, "solid.cylinder", json!({"base": [70, 30, -1], "radius": 5, "height": 5, "operation": "cut"}));
    assert_eq!(flat(&mut s)["cutouts"].as_array().map(Vec::len), Some(2));
    let dxf = run(&mut s, "sheet.export_dxf", json!({}))["dxf"].as_str().unwrap_or_default().to_string();
    assert!(dxf.contains("CIRCLE"), "the hole stays a circle");
    assert!((volume(&mut s) - (14750.0 - PI * 25.0 * 2.5)).abs() < 0.5);
    // A box that only dents the top face is a pocket of constant depth.
    run(&mut s, "solid.box", json!({"corner": [50, 10, 1.5], "length": 10, "width": 10, "height": 5, "operation": "cut"}));
    let f = flat(&mut s);
    assert_eq!(f["pockets"].as_array().map(Vec::len), Some(1));
    assert_eq!(f["pockets"][0]["depth_mm"], 1.0);
    // A box across a folded bend, or a cylinder lying in the sheet, is still 3D machining
    // and still refused atomically.
    let mut tilted = Session::default();
    base(&mut tilted, 100.0, 60.0);
    run(&mut tilted, "sheet.flange", json!({"edges": [[50, 0, 2.5]], "height": 20}));
    let before = tilted.doc.to_json();
    let error = tilted
        .execute("solid.box", &json!({"corner": [40, -10, -5], "length": 20, "width": 20, "height": 40, "operation": "cut"}))
        .unwrap_err()
        .to_string();
    assert!(error.contains("bend") || error.contains("preserv"), "{error}");
    assert_eq!(tilted.doc.to_json(), before);
    let error = s
        .execute("solid.cylinder", &json!({"base": [10, 50, 1.25], "axis": "X", "radius": 1, "height": 20, "operation": "cut"}))
        .unwrap_err()
        .to_string();
    assert!(error.contains("preserv"), "{error}");
}

#[test]
fn flat_pattern_reports_a_round_hole_as_a_circle() {
    // The DXF export already writes this hole as a CIRCLE; the JSON flat pattern should say
    // "circle, centre (50, 30), radius 1.6" too, not only a sampled polyline: any object in the
    // cut-out's shape that has a `center` and a `radius` counts.
    fn circles(v: &Value, out: &mut Vec<([f64; 2], f64)>) {
        match v {
            Value::Object(m) => {
                let center = m.get("center").and_then(Value::as_array).and_then(|c| Some([c.first()?.as_f64()?, c.get(1)?.as_f64()?]));
                if let (Some(center), Some(radius)) = (center, m.get("radius").and_then(Value::as_f64)) {
                    out.push((center, radius));
                }
                m.values().for_each(|v| circles(v, out));
            }
            Value::Array(a) => a.iter().for_each(|v| circles(v, out)),
            _ => {}
        }
    }
    let mut s = Session::default();
    base(&mut s, 100.0, 60.0);
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Hole"}));
    run(&mut s, "sketch.circle.center", json!({"center": [50, 30], "radius": 1.6}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.extrude", json!({"sketch": "Hole", "through_all": true, "direction": "symmetric", "operation": "cut"}));
    let dxf = run(&mut s, "sheet.export_dxf", json!({}))["dxf"].as_str().unwrap_or_default().to_string();
    assert!(dxf.contains("CIRCLE"), "the DXF keeps the circle");
    let f = flat(&mut s);
    let cutouts = f["cutouts"].as_array().cloned().unwrap_or_default();
    assert_eq!(cutouts.len(), 1);
    // The point list stays for existing readers; `cutout_shapes` carries the exact shape.
    assert!(cutouts[0].as_array().is_some_and(|p| p.len() > 8));
    let shapes = f["cutout_shapes"].as_array().cloned().unwrap_or_default();
    assert_eq!(shapes.len(), 1);
    let mut found = Vec::new();
    circles(&shapes[0], &mut found);
    let shown: String = shapes[0].to_string().chars().take(120).collect();
    assert!(
        found.iter().any(|(c, r)| (c[0] - 50.0).abs() < 1e-9 && (c[1] - 30.0).abs() < 1e-9 && (r - 1.6).abs() < 1e-9),
        "the round hole is not reported as a circle: {shown} ..."
    );
    // A slot is its two lines and two half-circle arcs, exactly.
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Slot"}));
    run(&mut s, "sketch.slot.overall", json!({"p0": [10, 10], "p1": [30, 10], "width": 4}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "solid.extrude", json!({"sketch": "Slot", "through_all": true, "direction": "symmetric", "operation": "cut"}));
    let f = flat(&mut s);
    let shapes = f["cutout_shapes"].as_array().cloned().unwrap_or_default();
    assert_eq!(shapes.len(), f["cutouts"].as_array().map_or(0, Vec::len));
    let slot = shapes.iter().find(|s| s.get("segments").is_some()).cloned().unwrap_or_default();
    let segments = slot["segments"].as_array().cloned().unwrap_or_default();
    assert_eq!(segments.iter().filter(|s| s.get("line").is_some()).count(), 2, "{slot}");
    let arcs: Vec<&Value> = segments.iter().filter_map(|s| s.get("arc")).collect();
    assert_eq!(arcs.len(), 2, "{slot}");
    assert!(
        arcs.iter()
            .all(|a| (a["radius"].as_f64().unwrap_or(0.0) - 2.0).abs() < 1e-9 && (a["sweep_deg"].as_f64().unwrap_or(0.0).abs() - 180.0).abs() < 1e-6)
    );
}

#[test]
fn a_flange_asked_for_0_8_mm_is_0_8_mm_or_refused_never_silently_2_5_mm() {
    // `thickness` on sheet.flange may become an inline override of the rule, or be refused as
    // an unknown parameter; either is fine. Building the default 2.5 mm sheet silently is not.
    let mut s = Session::default();
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Base"}));
    run(&mut s, "sketch.rectangle.two_point", json!({"p0": [0, 0], "p1": [100, 60]}));
    run(&mut s, "sketch.finish", json!({}));
    match s.execute("sheet.flange", &json!({"sketch": "Base", "thickness": 0.8})) {
        Ok(_) => {
            assert!((flat(&mut s)["thickness"].as_f64().unwrap_or(f64::NAN) - 0.8).abs() < 1e-9);
            assert!((volume(&mut s) - 100.0 * 60.0 * 0.8).abs() < 1e-6, "built at {} mm3", volume(&mut s));
        }
        Err(e) => assert!(e.to_string().contains("thickness"), "{e}"),
    }
}

/// A new fold still refuses a `fixed` point in a cut-out or past the outline, leaving the design
/// untouched (#263); a stored one that drifts off the sheet keeps its side with a warning (#370).
#[test]
fn fold_refuses_a_new_fixed_point_off_the_sheet_but_warns_on_recompute() {
    let mut s = Session::default();
    run(&mut s, "parameters.change", json!({"name": "w", "expression": "100 mm"}));
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Blank"}));
    run(&mut s, "sketch.rectangle.two_point", json!({"p0": [0, 0], "p1": [100, 60]}));
    run(&mut s, "sketch.dimension", json!({"entities": ["l1"], "value": "w"}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "sheet.flange", json!({"sketch": "Blank"}));
    rectangle_tool(&mut s, "Window", [10.0, 15.0], [30.0, 45.0]);
    run(&mut s, "solid.extrude", json!({"sketch": "Window", "through_all": true, "direction": "symmetric", "operation": "cut"}));
    let before = s.doc.to_json();
    let undo = s.undo.len();
    for fixed in [json!([20, 30, 2.5]), json!([150, 30, 2.5])] {
        let e = s.execute("sheet.fold", &json!({"points": [[40, 0, 2.5], [40, 60, 2.5]], "fixed": fixed})).unwrap_err().to_string();
        assert!(e.contains("fixed") && e.contains("not on the sheet"), "{fixed}: {e}");
        assert_eq!(s.doc.to_json(), before);
        assert_eq!(s.undo.len(), undo);
    }
    run(&mut s, "sheet.fold", json!({"points": [[40, 0, 2.5], [40, 60, 2.5]], "fixed": [90, 30, 2.5], "name": "Fold1"}));
    assert!(run(&mut s, "timeline.inspect", json!({"feature": "Fold1"}))["warning"].is_null());
    run(&mut s, "parameters.change", json!({"name": "w", "expression": "80 mm"}));
    let fold = run(&mut s, "timeline.inspect", json!({"feature": "Fold1"}));
    assert!(fold["error"].is_null(), "{fold}");
    assert!(fold["warning"].as_str().is_some_and(|w| w.contains("fixed point is not on the sheet") && w.contains("stays")), "{fold}");
}

/// Points away from the sheet are refused atomically; points on any face of it, on a flange's
/// leg, or a hair off a face still pick the side that stays (#371).
#[test]
fn fold_fixed_point_must_touch_the_sheet_but_any_face_will_do() {
    let mut s = Session::default();
    base(&mut s, 100.0, 60.0);
    let before = s.doc.to_json();
    let undo = s.undo.len();
    for fixed in [json!([90, 30, 20]), json!([90, 30, -10])] {
        let e = s.execute("sheet.fold", &json!({"points": [[40, 0, 2.5], [40, 60, 2.5]], "fixed": fixed})).unwrap_err().to_string();
        assert!(e.contains("fixed") && e.contains("not on the sheet"), "{fixed}: {e}");
        assert_eq!(s.doc.to_json(), before);
        assert_eq!(s.undo.len(), undo);
    }
    // The left (smaller) side stays: the right part stands up.
    for fixed in [json!([10, 30, 2.5]), json!([10, 30, 0]), json!([0, 30, 1.25]), json!([10, 30, 2.500001])] {
        let mut s = Session::default();
        base(&mut s, 100.0, 60.0);
        run(&mut s, "sheet.fold", json!({"points": [[40, 0, 2.5], [40, 60, 2.5]], "fixed": fixed, "name": "Fold1"}));
        let bb = bbox(&mut s);
        assert!(bb["min"][0].as_f64().unwrap_or(f64::NAN).abs() < 1e-6 && bb["max"][0].as_f64().unwrap_or(f64::NAN) < 50.0, "{fixed}: {bb}");
        assert!(run(&mut s, "timeline.inspect", json!({"feature": "Fold1"}))["warning"].is_null(), "{fixed}");
    }
    // A point on an edge flange's leg (far above the base panel's slab) is on the sheet.
    let mut f = Session::default();
    run(&mut f, "parameters.change", json!({"name": "h", "expression": "20 mm"}));
    base(&mut f, 100.0, 60.0);
    run(&mut f, "sheet.flange", json!({"edges": [[100, 30, 2.5]], "height": "h"}));
    let bb = bbox(&mut f);
    let x = bb["max"][0].as_f64().unwrap_or(f64::NAN);
    run(&mut f, "sheet.fold", json!({"points": [[40, 0, 2.5], [40, 60, 2.5]], "fixed": [x, 30, 15], "name": "Fold1"}));
    assert!(run(&mut f, "timeline.inspect", json!({"feature": "Fold1"}))["warning"].is_null());
    // A shorter leg leaves the stored point in the air: the fold recomputes, keeps its side and warns.
    run(&mut f, "parameters.change", json!({"name": "h", "expression": "10 mm"}));
    let fold = run(&mut f, "timeline.inspect", json!({"feature": "Fold1"}));
    assert!(fold["error"].is_null(), "{fold}");
    assert!(fold["warning"].as_str().is_some_and(|w| w.contains("fixed point is not on the sheet")), "{fold}");
    assert!((bbox(&mut f)["max"][0].as_f64().unwrap_or(f64::NAN) - x).abs() < 1e-6, "the right side stays");
}

/// The middle of the largest triangle of each flat face of the first body: well inside it.
fn flat_face_points(s: &Session) -> Vec<(usize, solidcraft_geom::Vec3, solidcraft_geom::Vec3)> {
    let state = s.model.state();
    let body = &state.bodies[0].body;
    let mesh = body.tessellate(0.01).unwrap();
    (0..body.face_count())
        .filter_map(|index| {
            let normal = body.planar_face(index)?.normal();
            let point = mesh
                .triangles
                .iter()
                .zip(&mesh.tri_face)
                .filter(|(_, f)| **f as usize == index)
                .filter_map(|(t, _)| mesh.tri(t))
                .max_by(|[a, b, c], [d, e, f]| (*b - *a).cross(*c - *a).len().total_cmp(&(*e - *d).cross(*f - *d).len()))
                .map(|[a, b, c]| (a + b + c) * (1.0 / 3.0))?;
            Some((index, point, normal))
        })
        .collect()
}

/// Every flat face of a sheet takes an attached sketch (#495): its panels, the rims along the
/// panels' straight edges (named by the base sketch line or the flange's far edge and sides) and
/// the flat ends of a bend. Each lies on its face, facing out of the sheet, and a rim's sketch
/// follows the rim when the base grows. A press-pull on a rim pushes it out, and a plain hole into
/// a panel is kept as a cut-out of the flat pattern (#560).
#[test]
fn every_flat_face_of_a_sheet_takes_an_attached_sketch() {
    let mut s = Session::default();
    run(&mut s, "parameters.add", json!({"name": "W", "expression": "100 mm"}));
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Base"}));
    run(&mut s, "sketch.rectangle.two_point", json!({"p0": [0, 0], "p1": [100, 60]}));
    run(&mut s, "sketch.constraint.fix", json!({"entities": ["p1"]}));
    run(&mut s, "sketch.dimension", json!({"entities": ["l1"], "value": "W"}));
    run(&mut s, "sketch.finish", json!({}));
    run(&mut s, "sheet.flange", json!({"sketch": "Base"}));
    run(&mut s, "sheet.flange", json!({"edges": [[50, 0, 2.5]], "height": 20}));
    let faces = flat_face_points(&s);
    // The base: its two faces and three rims (the fourth edge bends); the flange: its two faces,
    // its far rim and two sides; the two ends of the bend.
    assert_eq!(faces.len(), 12);
    let mut on_side = None;
    for (index, point, normal) in faces {
        let made = run(&mut s, "sketch.create", json!({"plane": {"face": point, "body": "Body1", "index": index}}));
        run(&mut s, "sketch.finish", json!({}));
        assert_eq!(made["attachment"], "sheet_face", "face {index} at {point:?}");
        let id = made["sketch"].as_u64().unwrap();
        let solidcraft_doc::FaceAttachmentStatus::Sheet { plane: Some(plane) } = s.placed_document().sketch_face_attachment(id, &s.model) else {
            panic!("face {index} at {point:?}")
        };
        assert!(plane.normal().dot(normal) > 1.0 - 1e-9 && (point - plane.origin).dot(normal).abs() < 1e-6, "face {index}: {plane:?}");
        if (point.x - 100.0).abs() < 1e-6 && point.y > 10.0 {
            on_side = Some(id);
        }
    }
    // The base's rim along x = W (the sketch's l2) moves with W.
    let side = on_side.expect("the base rim on x = 100");
    run(&mut s, "parameters.change", json!({"name": "W", "expression": "120 mm"}));
    let solidcraft_doc::FaceAttachmentStatus::Sheet { plane: Some(plane) } = s.placed_document().sketch_face_attachment(side, &s.model) else {
        panic!("the rim sketch after the edit")
    };
    assert!((plane.origin.x - 120.0).abs() < 1e-9 && (plane.normal().x - 1.0).abs() < 1e-9, "{plane:?}");
    // Press-pull the rim 5 mm out: the base grows by 5 x 60 x 2.5 less what the flange covers.
    let v0 = volume(&mut s);
    run(&mut s, "solid.press_pull", json!({"face": [120, 30, 1.25], "distance": 5}));
    assert!(volume(&mut s) > v0 + 5.0 * 50.0 * 2.5, "{} {v0}", volume(&mut s));
    // A plain hole into the base is a cut-out of the flat pattern.
    let mut t = Session::default();
    plate_with_flange(&mut t);
    let v0 = volume(&mut t);
    run(&mut t, "solid.hole", json!({"position": [50, 30, 2.5], "direction": [0, 0, -1], "diameter": 6}));
    assert!((v0 - volume(&mut t) - PI * 9.0 * 2.5).abs() < 0.1, "{}", v0 - volume(&mut t));
    let f = flat(&mut t);
    assert_eq!(f["cutouts"].as_array().map(Vec::len), Some(1), "{f}");
}

/// A 60 x 40 base with a 30 mm flange up its far long edge (y = 40); its thickness.
fn l_bracket(s: &mut Session) -> f64 {
    base(s, 60.0, 40.0);
    run(s, "sheet.flange", json!({"type": "edge", "edges": [[30, 40, 0]], "height": 30}));
    flat(s)["thickness"].as_f64().unwrap()
}

#[test]
fn a_hole_drilled_into_a_rim_is_a_bore_the_sheet_keeps_when_it_unfolds() {
    let mut s = Session::default();
    let t = l_bracket(&mut s);
    let before = volume(&mut s);
    let flat_before = unfolded_volume(&mut s);
    let (d, y) = (t * 0.6, 20.0);
    // Into the base's end rim (x = 60) along the base, through all: out at x = 0.
    run(&mut s, "solid.hole", json!({"position": [60, y, t / 2.0], "direction": [-1, 0, 0], "diameter": d}));
    let removed = PI * (d / 2.0).powi(2) * 60.0;
    let folded = volume(&mut s);
    assert!(rel(before - folded, removed) < 0.02, "removed {} of {removed}", before - folded);
    let f = flat(&mut s);
    assert_eq!(f["bores"].as_array().map(Vec::len), Some(1), "{f}");
    assert_eq!(f["cutouts"], json!([]));
    let inside = |s: &Session, p: [f64; 3]| s.model.state().bodies[0].body.contains(solidcraft_geom::Vec3::new(p[0], p[1], p[2]));
    // The flat pattern loses the same bore (the bends unroll to their own length).
    let flat_after = unfolded_volume(&mut s);
    assert!(rel(flat_before - flat_after, removed) < 0.02, "flat: removed {} of {removed}", flat_before - flat_after);
    assert!(rel(volume(&mut s), folded) < 1e-6);
    // Folded again: the bore is where it was drilled, the material beside it stays.
    assert!(!inside(&s, [30.0, y, t / 2.0]));
    assert!(inside(&s, [30.0, y + d, t / 2.0]));
    let mut reopened = Session::new(solidcraft_doc::Document::from_json(&s.doc.to_json()).unwrap());
    assert!(rel(volume(&mut reopened), folded) < 1e-3);
    assert!(rel(unfolded_volume(&mut reopened), flat_after) < 1e-6);
}

/// The volume of the sheet unfolded; it is folded back after.
fn unfolded_volume(s: &mut Session) -> f64 {
    run(s, "sheet.unfold", json!({}));
    let v = volume(s);
    run(s, "sheet.refold", json!({}));
    v
}

#[test]
fn a_rim_hole_in_a_flange_follows_it_into_the_flat_pattern() {
    let mut s = Session::default();
    let t = l_bracket(&mut s);
    let before = volume(&mut s);
    let flat_before = unfolded_volume(&mut s);
    let inside = |s: &Session, p: [f64; 3]| s.model.state().bodies[0].body.contains(solidcraft_geom::Vec3::new(p[0], p[1], p[2]));
    // Halfway up the flange, in the middle of its thickness (whichever way the rule put it).
    let mid = [[40.0 - t / 2.0, 15.0], [40.0 - t / 2.0, -15.0], [40.0 + t / 2.0, 15.0], [40.0 + t / 2.0, -15.0]]
        .into_iter()
        .find(|[y, z]| inside(&s, [30.0, *y, *z]))
        .expect("the flange's material");
    let d = t * 0.5;
    // Into the flange's end rim (x = 60), through all.
    run(&mut s, "solid.hole", json!({"position": [60.0, mid[0], mid[1]], "direction": [-1, 0, 0], "diameter": d}));
    let folded = volume(&mut s);
    assert!(rel(before - folded, PI * (d / 2.0).powi(2) * 60.0) < 0.02, "{before} -> {folded}");
    assert!(!inside(&s, [30.0, mid[0], mid[1]]));
    let flat_after = unfolded_volume(&mut s);
    assert!(rel(flat_before - flat_after, before - folded) < 0.02, "flat: {flat_before} -> {flat_after}");
    assert!(rel(volume(&mut s), folded) < 1e-6);
    assert!(!inside(&s, [30.0, mid[0], mid[1]]));
}

#[test]
fn a_rim_hole_that_would_run_into_a_bend_is_refused_and_changes_nothing() {
    let mut s = Session::default();
    let t = l_bracket(&mut s);
    let before = volume(&mut s);
    // Into the near rim (y = 0) toward the bend at y = 40.
    let r = s.execute("solid.hole", &json!({"position": [30, 0, t / 2.0], "direction": [0, 1, 0], "diameter": t * 0.6}));
    assert!(r.is_err(), "{r:?}");
    assert!(rel(volume(&mut s), before) < 1e-9);
    assert!(flat(&mut s).get("bores").is_none());
}

/// #732: a perforated panel. A 400 x 470 x 0.8 sheet with Ø1 holes on a 1.5 mm 60° staggered grid (DIN 24041
/// Rv 1.0-1.5), cut in two sketches of 500 holes; 0.8.0 refuses the second cut ("sheet metal flat geometry exceeds
/// the bounded processing budget"). A real mesh panel needs ~94 000 such holes; 1000 is the first step.
#[test]
fn perforated_sheet_takes_a_thousand_round_holes() {
    let mut s = Session::default();
    outline_blank(&mut s, json!([[0, 0], [400, 0], [400, 470], [0, 470]]));
    let flat = volume(&mut s);
    let (p, dy) = (1.5_f64, 1.5_f64 * 3f64.sqrt() / 2.0);
    let mut holes = Vec::new();
    'grid: for r in 0..(450.0 / dy) as usize {
        for k in 0..(380.0 / p) as usize {
            let x = 10.0 + k as f64 * p + if r % 2 == 1 { p / 2.0 } else { 0.0 };
            holes.push([x, 10.0 + r as f64 * dy]);
            if holes.len() == 1000 {
                break 'grid;
            }
        }
    }
    for (g, chunk) in holes.chunks(500).enumerate() {
        let name = format!("Perforation {g}");
        run(&mut s, "sketch.create", json!({"plane": {"origin": [0, 0, 2], "x_dir": [1, 0, 0], "y_dir": [0, 1, 0]}, "name": name}));
        for c in chunk {
            run(&mut s, "sketch.circle.center", json!({"center": c, "radius": 0.5}));
        }
        run(&mut s, "sketch.finish", json!({}));
        run(&mut s, "solid.extrude", json!({"sketch": name, "distance": "4", "direction": "negative", "operation": "cut"}));
    }
    let removed = flat - volume(&mut s);
    let expected = 1000.0 * std::f64::consts::PI * 0.5 * 0.5 * 0.8;
    assert!((removed - expected).abs() / expected < 0.05, "removed {removed:.1} mm³, 1000 holes are {expected:.1} mm³");
}

// ---- #732 sheet.perforate: a perforation field stored on the sheet, not cut into the B-rep ----------------------

/// Hole centres of a staggered (60 deg) perforation over a w x h rectangle at z 0, as `sheet.perforate` places them:
/// rows along x every pitch * sqrt(3) / 2, odd rows shifted half a pitch, starting at margin + radius from the
/// lower-left corner; a hole is kept when it stays margin away from the outline.
fn staggered_count(w: f64, h: f64, d: f64, pitch: f64, margin: f64) -> usize {
    let (r, dy) = (d / 2.0, pitch * 3f64.sqrt() / 2.0);
    let (lo, hix, hiy) = (margin + r, w - margin - r, h - margin - r);
    let mut n = 0;
    let mut row = 0;
    while lo + row as f64 * dy <= hiy + 1e-9 {
        let mut x = lo + if row % 2 == 1 { pitch / 2.0 } else { 0.0 };
        while x <= hix + 1e-9 {
            n += 1;
            x += pitch;
        }
        row += 1;
    }
    n
}

fn perforation_circles(dxf: &str) -> Vec<(f64, f64, f64)> {
    let lines: Vec<&str> = dxf.lines().map(str::trim).collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < lines.len() {
        if lines[i] == "0" && lines[i + 1] == "CIRCLE" {
            let (mut layer, mut x, mut y, mut r) = ("", 0.0, 0.0, 0.0);
            let mut j = i + 2;
            while j + 1 < lines.len() && lines[j] != "0" {
                match lines[j] {
                    "8" => layer = lines[j + 1],
                    "10" => x = lines[j + 1].parse().unwrap_or(0.0),
                    "20" => y = lines[j + 1].parse().unwrap_or(0.0),
                    "40" => r = lines[j + 1].parse().unwrap_or(0.0),
                    _ => {}
                }
                j += 2;
            }
            if layer == "PERFORATION" {
                out.push((x, y, r));
            }
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

/// #732: Rv 1.0-1.5 over a 100 x 60 x 0.8 sheet: every hole is in the flat pattern and in the DXF (layer
/// PERFORATION), placed on the documented grid, and the B-rep is not cut (the field is stored like a cosmetic thread).
#[test]
fn perforation_field_exports_every_hole_to_the_flat_pattern_and_dxf() {
    let mut s = Session::default();
    outline_blank(&mut s, json!([[0, 0], [100, 0], [100, 60], [0, 60]]));
    let faces = run(&mut s, "inspect.measure", json!({}))["bodies"][0]["faces"].clone();
    run(&mut s, "sheet.perforate", json!({"diameter": "1", "pitch": "1.5", "margin": "1"}));
    let expected = staggered_count(100.0, 60.0, 1.0, 1.5, 1.0);
    let f = flat(&mut s);
    assert_eq!(f["perforations"][0]["count"].as_u64(), Some(expected as u64), "{}", f["perforations"]);
    assert_eq!(f["perforations"][0]["pattern"], "staggered");
    let open = f["perforations"][0]["open_area_mm2"].as_f64().unwrap();
    assert!(rel(open, expected as f64 * PI * 0.25) < 1e-9);
    let dxf = run(&mut s, "sheet.export_dxf", json!({}))["dxf"].as_str().unwrap_or_default().to_string();
    let holes = perforation_circles(&dxf);
    assert_eq!(holes.len(), expected);
    assert!(holes.iter().all(|&(x, y, r)| (r - 0.5).abs() < 1e-9 && (1.5..=98.5).contains(&x) && (1.5..=58.5).contains(&y)));
    assert!((holes[0].0 - 1.5).abs() < 1e-9 && (holes[0].1 - 1.5).abs() < 1e-9, "first hole at margin + radius: {:?}", holes[0]);
    assert_eq!(run(&mut s, "inspect.measure", json!({}))["bodies"][0]["faces"], faces, "the B-rep is not cut");
}

/// #732: the measured volume and mass drop by the open area times the thickness.
#[test]
fn perforation_reduces_volume_and_mass_by_the_open_area() {
    let mut s = Session::default();
    outline_blank(&mut s, json!([[0, 0], [100, 0], [100, 60], [0, 60]]));
    let before = run(&mut s, "inspect.measure", json!({}));
    run(&mut s, "sheet.perforate", json!({"diameter": "1", "pitch": "1.5", "margin": "1"}));
    let after = run(&mut s, "inspect.measure", json!({}));
    let n = staggered_count(100.0, 60.0, 1.0, 1.5, 1.0) as f64;
    let removed = before["total"]["volume_mm3"].as_f64().unwrap() - after["total"]["volume_mm3"].as_f64().unwrap();
    assert!(rel(removed, n * PI * 0.25 * 0.8) < 1e-9, "removed {removed}");
    let (m0, m1) = (before["bodies"][0]["mass_g"].as_f64().unwrap(), after["bodies"][0]["mass_g"].as_f64().unwrap());
    let ratio = after["total"]["volume_mm3"].as_f64().unwrap() / before["total"]["volume_mm3"].as_f64().unwrap();
    assert!(rel(m1 / m0, ratio) < 1e-9);
}

/// #732: holes stay out of bend zones, also when the fold comes after the perforation; the field follows the sheet
/// through unfold / refold and through save and reopen.
#[test]
fn perforation_skips_bend_zones_and_survives_fold_unfold_and_reopen() {
    let mut s = Session::default();
    outline_blank(&mut s, json!([[0, 0], [100, 0], [100, 60], [0, 60]]));
    run(&mut s, "sheet.perforate", json!({"diameter": "1", "pitch": "1.5", "margin": "1"}));
    let flat_count = flat(&mut s)["perforations"][0]["count"].as_u64().unwrap();
    run(&mut s, "sheet.fold", json!({"points": [[80, 0, 0.8], [80, 60, 0.8]], "fixed": [40, 30, 0.4]}));
    let folded = flat(&mut s);
    let count = folded["perforations"][0]["count"].as_u64().unwrap();
    assert!(count < flat_count, "holes in the new bend zone are dropped: {count} of {flat_count}");
    let zone = &folded["bend_zones"][0]; // [min x, max x, min y, max y] of the bend zone, flat coordinates
    let (z0, z1) = (zone[0].as_f64().unwrap_or(f64::NAN), zone[1].as_f64().unwrap_or(f64::NAN));
    let dxf = run(&mut s, "sheet.export_dxf", json!({}))["dxf"].as_str().unwrap_or_default().to_string();
    assert!(
        perforation_circles(&dxf).iter().all(|&(x, _, r)| x + r + 1.0 <= z0 + 1e-9 || x - r - 1.0 >= z1 - 1e-9),
        "no hole within margin of the bend zone x {z0}..{z1}"
    );
    let volume_folded = volume(&mut s);
    run(&mut s, "sheet.unfold", json!({}));
    assert_eq!(flat(&mut s)["perforations"][0]["count"].as_u64(), Some(count));
    run(&mut s, "sheet.refold", json!({}));
    assert!(rel(volume(&mut s), volume_folded) < 1e-9);
    let mut reopened = Session::new(solidcraft_doc::Document::from_json(&s.doc.to_json()).unwrap());
    assert_eq!(flat(&mut reopened)["perforations"][0]["count"].as_u64(), Some(count));
    assert!(rel(volume(&mut reopened), volume_folded) < 1e-9);
}

/// #732: a real mesh panel - 400 x 470 with Rv 1.0-1.5 is about 94 000 holes - is quick to perforate, measure and
/// export (the holes never enter the flat-geometry budget).
#[test]
fn perforating_a_mesh_panel_with_ninety_thousand_holes_is_fast() {
    let mut s = Session::default();
    outline_blank(&mut s, json!([[0, 0], [400, 0], [400, 470], [0, 470]]));
    let t = std::time::Instant::now();
    run(&mut s, "sheet.perforate", json!({"diameter": "1", "pitch": "1.5", "margin": "1"}));
    let count = flat(&mut s)["perforations"][0]["count"].as_u64().unwrap();
    let dxf = run(&mut s, "sheet.export_dxf", json!({}))["dxf"].as_str().unwrap_or_default().len();
    volume(&mut s);
    assert!(count > 90_000, "{count}");
    assert!(dxf > 90_000 * 30, "every hole is in the DXF");
    assert!(t.elapsed().as_secs_f64() < 20.0, "took {:?}", t.elapsed());
}

/// #732: a square pattern, and values that cannot be punched are refused.
#[test]
fn perforate_supports_a_square_pattern_and_refuses_impossible_values() {
    let mut s = Session::default();
    outline_blank(&mut s, json!([[0, 0], [30, 0], [30, 20], [0, 20]]));
    run(&mut s, "sheet.perforate", json!({"diameter": "2", "pitch": "4", "margin": "0", "pattern": "square"}));
    assert_eq!(flat(&mut s)["perforations"][0]["count"].as_u64(), Some(8 * 5));
    for bad in [
        json!({"diameter": "0", "pitch": "2"}),
        json!({"diameter": "2", "pitch": "2"}),
        json!({"diameter": "1", "pitch": "2", "pattern": "hexagon"}),
        json!({"diameter": "1", "pitch": "2", "margin": "-1"}),
    ] {
        assert!(s.execute("sheet.perforate", &bad).is_err(), "{bad} must be refused");
    }
}

/// A 100 x 60 tray with one 45° flange 40 high on its y = 0 edge (2.5 mm, R 2.5, K 0.44), and on
/// it a return 10 high when asked. Returns the far edge of the last panel in the flat pattern:
/// the base starts at the setback s = (t + R)·tan 22.5°, each bend's zone is (π/4)·(R + K·t) long,
/// and a panel h high is h − s long, or h − 2s with a return on its far edge.
fn tray_with_a_45_degree_flange(s: &mut Session, with_return: bool) -> f64 {
    base(s, 100.0, 60.0);
    run(s, "sheet.flange", json!({"edges": [[50, 0, 2.5]], "height": 40, "angle": 45}));
    let (setback, ba) = (5.0 * (PI / 8.0).tan(), PI / 4.0 * (2.5 + 0.44 * 2.5));
    if with_return {
        run(s, "sheet.flange", json!({"edges": [[50, -27.5, 26.5]], "height": 10, "angle": 45}));
        return setback - ba - (40.0 - 2.0 * setback) - ba - (10.0 - setback);
    }
    setback - ba - (40.0 - setback)
}

/// A point on that flange's inside face, the plane y + z = 2.5·√2 (tangent to the bend's inside
/// radius, 2.5 about y = 5·tan 22.5°, z = 5).
const FLANGE_FACE: [f64; 3] = [50.0, 2.5 * std::f64::consts::SQRT_2 - 15.0, 15.0];

fn last_warning(s: &Session) -> String {
    s.model.results.last().and_then(|r| r.warning.clone()).unwrap_or_default()
}

fn parted_note(area: f64, at: [f64; 2]) -> String {
    format!("1 piece of `Body1` is no longer joined to its base ({area:.2} mm², near ({:.1}, {:.1}) in the flat pattern); it is kept", at[0], at[1])
}

fn flat_pieces(s: &Session) -> Vec<f64> {
    s.model.state().sheets[0].flat_regions().unwrap().iter().map(solidcraft_geom::Region2::area).collect()
}

#[test]
fn a_cut_that_parts_a_flange_from_the_base_keeps_it_and_says_so_folded_or_flat() {
    // A slot across the flange beside its bend (flat y −3 to −1.5) leaves the flange's panel past
    // it, with any return on it, joined to nothing: 100 × (−3 − far). Cut on the folded flange's
    // face or in the unfolded sheet, the piece stays, and the cut names it in its reply and on the
    // timeline, with the middle of the piece in the flat pattern.
    for (with_return, unfolded) in [(false, false), (false, true), (true, false), (true, true)] {
        let what = format!("return {with_return}, unfolded {unfolded}");
        let mut s = Session::default();
        let far = tray_with_a_45_degree_flange(&mut s, with_return);
        let out = if unfolded {
            run(&mut s, "sheet.unfold", json!({}));
            rectangle_tool(&mut s, "Slot", [-5.0, -3.0], [105.0, -1.5]);
            run(&mut s, "solid.extrude", json!({"sketch": "Slot", "through_all": true, "direction": "symmetric", "operation": "cut"}))
        } else {
            run(&mut s, "sketch.create", json!({"plane": {"face": FLANGE_FACE}, "name": "Slot"}));
            run(&mut s, "sketch.rectangle.two_point", json!({"p0": [-5, -3], "p1": [105, -1.5]}));
            run(&mut s, "sketch.finish", json!({}));
            run(&mut s, "solid.extrude", json!({"sketch": "Slot", "through_all": true, "direction": "negative", "operation": "cut"}))
        };
        let note = parted_note(100.0 * (-3.0 - far), [50.0, (far - 3.0) / 2.0]);
        assert!(last_warning(&s).contains(&note), "{what}: {}", last_warning(&s));
        assert_eq!(out["warning"], json!(last_warning(&s)), "{what}: {out}");
        // Both pieces stay, folded and flat: the base with its bend left, and the flange.
        let pieces = flat_pieces(&s);
        assert!(pieces.len() == 2 && pieces.iter().any(|a| (a - 100.0 * (-3.0 - far)).abs() < 1e-6), "{what}: {pieces:?}");
        for command in if unfolded { ["sheet.refold", "sheet.unfold"] } else { ["sheet.unfold", "sheet.refold"] } {
            run(&mut s, command, json!({}));
            assert_eq!(flat(&mut s)["outline"].as_array().map(Vec::len), Some(2), "{what} {command}");
            assert_eq!(flat_pieces(&s), pieces, "{what} {command}");
            if command == "sheet.unfold" {
                assert!((volume(&mut s) - pieces.iter().sum::<f64>() * 2.5).abs() < 1e-3, "{what}: {}", volume(&mut s));
            }
        }
    }
}

#[test]
fn a_ring_over_a_flanges_edge_names_the_piece_of_its_inside_on_the_flange_but_not_an_island() {
    // A ring Ø16 / Ø10 cut on the folded flange's face, centred on its far edge: the half of its
    // inside on the flange (π·5²/2, its centroid 4·5/3π inside the edge) has nothing round it,
    // and is named. Centred on the flange, its inside is an island, and nothing is named.
    for (y, named) in [(None, true), (Some(-20.0), false)] {
        let mut s = Session::default();
        let far = tray_with_a_45_degree_flange(&mut s, false);
        let y = y.unwrap_or(far);
        run(&mut s, "sketch.create", json!({"plane": {"face": FLANGE_FACE}, "name": "Ring"}));
        run(&mut s, "sketch.circle.center", json!({"center": [50, y], "diameter": 16}));
        run(&mut s, "sketch.circle.center", json!({"center": [50, y], "diameter": 10}));
        run(&mut s, "sketch.finish", json!({}));
        let out = run(
            &mut s,
            "solid.extrude",
            json!({"sketch": "Ring", "profiles": [{"point": [56.5, y]}], "through_all": true, "direction": "negative", "operation": "cut"}),
        );
        let warning = out["warning"].as_str().unwrap_or_default().to_string();
        let note = parted_note(12.5 * PI, [50.0, far + 20.0 / (3.0 * PI)]);
        assert_eq!(warning.contains(&note), named, "{y}: {warning}");
        assert!(named || !warning.contains("no longer joined"), "{y}: {warning}");
        assert_eq!(flat_pieces(&s).len(), 2, "{y}");
    }
}

#[test]
fn cuts_that_split_a_sheet_or_cut_it_beside_the_base_name_only_the_pieces_they_part() {
    // Unfolded, a strip across tray and flange leaves two pieces that each keep their part of
    // the base: nothing to name. A box over the smaller piece's base and bend then leaves its
    // flange panel, 26 × (−3 − far), named; a hole in the base after it names nothing again.
    let mut s = Session::default();
    let far = tray_with_a_45_degree_flange(&mut s, false);
    run(&mut s, "sheet.unfold", json!({}));
    rectangle_tool(&mut s, "Strip", [70.0, -60.0], [74.0, 70.0]);
    let out = run(&mut s, "solid.extrude", json!({"sketch": "Strip", "through_all": true, "direction": "symmetric", "operation": "cut"}));
    assert!(out["warning"].is_null() && last_warning(&s).is_empty(), "{out}");
    rectangle_tool(&mut s, "Box", [72.0, -3.0], [101.0, 61.0]);
    let out = run(&mut s, "solid.extrude", json!({"sketch": "Box", "through_all": true, "direction": "symmetric", "operation": "cut"}));
    assert_eq!(out["warning"], json!(parted_note(26.0 * (-3.0 - far), [87.0, (far - 3.0) / 2.0])), "{out}");
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Hole"}));
    run(&mut s, "sketch.circle.center", json!({"center": [30, 30], "diameter": 8}));
    run(&mut s, "sketch.finish", json!({}));
    let out = run(&mut s, "solid.extrude", json!({"sketch": "Hole", "through_all": true, "direction": "symmetric", "operation": "cut"}));
    assert!(out["warning"].is_null(), "{out}");
    run(&mut s, "sheet.refold", json!({}));
    assert_eq!(flat(&mut s)["outline"].as_array().map(Vec::len), Some(2));
    // Folded, a slot across the base beside the bend leaves the flange with a strip of the base;
    // a plain plate cut in two halves. Neither names anything.
    let mut s = Session::default();
    tray_with_a_45_degree_flange(&mut s, false);
    rectangle_tool(&mut s, "Slot", [-5.0, 3.0], [105.0, 6.0]);
    let out = run(&mut s, "solid.extrude", json!({"sketch": "Slot", "through_all": true, "direction": "symmetric", "operation": "cut"}));
    assert!(out["warning"].is_null() && flat_pieces(&s).len() == 2, "{out}");
    let mut s = Session::default();
    base(&mut s, 100.0, 60.0);
    rectangle_tool(&mut s, "Strip", [45.0, -5.0], [55.0, 65.0]);
    let out = run(&mut s, "solid.extrude", json!({"sketch": "Strip", "through_all": true, "direction": "symmetric", "operation": "cut"}));
    assert!(out["warning"].is_null() && flat_pieces(&s).len() == 2, "{out}");
}

#[test]
fn a_hole_that_parts_a_flange_from_the_base_names_it() {
    // A 10 x 60 plate with a 90° flange 40 high on its y = 0 edge: the flange's panel runs from
    // flat y = 5 − BA (world z = 5) for 35 mm. A Ø12 hole drilled into it at z = 15 (flat
    // y = 5 − BA − 10) takes its whole width, and leaves the 25 mm below the hole's centre less
    // the half disc there: 10 × 25 − ∫ √(36 − u²) du over |u| ≤ 5 = 250 − (5√11 + 36·asin(5/6)).
    let mut s = Session::default();
    base(&mut s, 10.0, 60.0);
    run(&mut s, "sheet.flange", json!({"edges": [[5, 0, 2.5]], "height": 40}));
    let out = run(&mut s, "solid.hole", json!({"position": [5, 0, 15], "diameter": 12}));
    let area = 250.0 - (5.0 * 11f64.sqrt() + 36.0 * (5.0f64 / 6.0).asin());
    let warning = out["warning"].as_str().unwrap_or_default();
    assert!(warning.starts_with(&format!("1 piece of `Body1` is no longer joined to its base ({area:.2} mm², near (5.0, ")), "{warning}");
    assert_eq!(out["warning"], json!(last_warning(&s)));
    assert_eq!(flat_pieces(&s).len(), 2);
}
