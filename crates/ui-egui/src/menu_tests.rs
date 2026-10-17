//! Context menu tests: what the menus offer for each target, and that their items act.

use egui::pos2;
use serde_json::json;
use solidcraft_engine::Session;

use crate::context_menu::{self, Item, Target};
use crate::{Services, SolveApp};

fn sample_app() -> SolveApp {
    let mut s = Session::default();
    s.run_script(&solidcraft_engine::sample::script()).unwrap();
    SolveApp::new(s, Services::default())
}

#[test]
fn properties_leave_with_the_document_but_survive_a_failed_open() {
    let mut app = sample_app();
    app.menu.props = Some(("Prior body".into(), json!({"volume_mm3":42})));
    app.start("config.table");
    assert!(app.dialog.is_some());
    assert!(app.run("doc.open", json!({"path":"__missing_properties_regression__.solidcraft"})).is_err());
    assert!(app.menu.props.is_some());
    assert!(app.dialog.is_some());
    app.run("file.new", json!({"name":"New design"})).unwrap();
    assert!(app.menu.props.is_none());
    assert!(app.dialog.is_none());
    app.menu.props = Some(("New design".into(), json!({})));
    crate::documents::open_tab(&mut app, Session::default()).unwrap();
    assert!(app.menu.props.is_none());
    app.menu.props = Some(("Blank design".into(), json!({})));
    crate::documents::adopt(&mut app, Session::default());
    assert!(app.menu.props.is_none());
}

fn find(items: &[Item], label: &str) -> Item {
    items
        .iter()
        .find(|i| i.label == label)
        .cloned()
        .unwrap_or_else(|| panic!("no `{label}` in {:?}", items.iter().map(|i| &i.label).collect::<Vec<_>>()))
}

/// Run the item labelled `label` of the target's menu.
fn act(app: &mut SolveApp, t: &Target, label: &str) {
    let item = find(&context_menu::items(app, t), label);
    context_menu::run_item(app, &item, pos2(0.0, 0.0));
}

fn body(app: &SolveApp) -> String {
    app.session.model.state().bodies[0].name.clone()
}

#[test]
fn empty_space_offers_view_items_and_greys_out_undo() {
    let mut app = sample_app();
    app.session.undo.clear();
    let items = context_menu::items(&app, &Target::Viewport);
    assert!(!find(&items, "Undo").enabled, "nothing to undo");
    assert!(find(&items, "Fit").enabled);
    assert_eq!(context_menu::radial_items(&app).len(), 8);
    assert!(!context_menu::radial_items(&app)[0].enabled, "no command to repeat yet");
    // Hide Origin toggles.
    context_menu::run_item(&mut app, &find(&items, "Hide Origin"), pos2(0.0, 0.0));
    assert!(!app.ui.show_origin);
}

#[test]
fn appearance_browser_action_targets_the_clicked_body() {
    let mut app = sample_app();
    let b = body(&app);
    app.run("select.clear", json!({})).unwrap();
    act(&mut app, &Target::Body { name: b.clone() }, "Appearance");
    let dialog = app.dialog.as_ref().unwrap();
    assert!(matches!(dialog.kind, crate::dialogs::Kind::Appearance(_)));
    assert_eq!(dialog.inputs[0].items, vec![solidcraft_engine::Sel::Body { name: b }]);
}

#[test]
fn appearance_component_action_assigns_an_inherited_look() {
    let mut app = sample_app();
    let created = app.run("component.create", json!({"name": "Painted part"})).unwrap();
    let id = created["component"].as_u64().unwrap();
    act(&mut app, &Target::Component { id }, "Appearance");
    let d = app.dialog.clone().unwrap();
    let commands = crate::dialogs::apply_commands(&app, &d).unwrap();
    assert_eq!(commands[0].1["components"], json!(["Painted part"]));
    for (id, p) in commands {
        app.run(&id, p).unwrap();
    }
    assert!(app.session.doc.appearances.components.contains_key(&id));
}

/// A right-click on a sketch curve in the viewport reaches the sketch's own menu, the one the
/// browser row opens, Edit Sketch among it.
#[test]
fn a_sketch_curve_in_the_viewport_offers_the_sketch_menu() {
    let mut app = SolveApp::new(Session::default(), Services::default());
    app.run("sketch.create", json!({"plane": "XY", "name": "Base"})).unwrap();
    app.run("sketch.rectangle.two_point", json!({"p0": [0, 0], "p1": [40, 30]})).unwrap();
    app.run("sketch.finish", json!({})).unwrap();
    let (sketch, curve) = {
        let st = app.session.world_state();
        let ss = st.sketches.first().expect("the design has one sketch");
        (ss.feature, ss.sketch.curves.first().expect("the rectangle drew curves").id.clone())
    };
    app.run("select.set", json!({"items": [{"type": "external_sketch_curve", "sketch": sketch, "id": curve}]})).unwrap();
    let items = context_menu::items(&app, &Target::Viewport);
    // Item for item the browser row's menu, with the viewport's Clear Selection after it.
    let row = context_menu::items(&app, &Target::Sketch { id: sketch });
    let labels = |v: &[Item]| v.iter().map(|i| i.label.clone()).collect::<Vec<_>>();
    assert_eq!(labels(&items), [labels(&row), vec![String::new(), "Clear Selection".to_string()]].concat());
    let edit = find(&items, "Edit Sketch");
    assert_eq!(edit.id, "ui.editSketch");
    assert_eq!(edit.params, Some(json!({"sketch": sketch})));
    assert!(edit.enabled);
    context_menu::run_item(&mut app, &edit, pos2(0.0, 0.0));
    assert_eq!(app.session.active_sketch, Some(sketch), "Edit Sketch opens the sketch that was right-clicked");
    // The sketch is open now: the viewport offers the sketch-editing menu, not a second Edit Sketch.
    let editing = context_menu::items(&app, &Target::Viewport);
    assert!(find(&editing, "Finish Sketch").enabled);
    assert!(!editing.iter().any(|i| i.label == "Edit Sketch"), "{:?}", labels(&editing));
}

/// Solid geometry in the selection keeps the body menu: a right-click on a face offers the
/// face's commands even when a sketch curve is selected as well.
#[test]
fn a_selected_face_keeps_the_body_menu_beside_a_sketch_curve() {
    let mut app = sample_app();
    let b = body(&app);
    let sketch = app.session.world_state().sketches.first().expect("the sample design has sketches").feature;
    let items = json!([
        {"type": "face", "body": b, "index": 0, "point": [0, 0, 0]},
        {"type": "external_sketch_curve", "sketch": sketch, "id": "l1"},
    ]);
    app.run("select.set", json!({"items": items})).unwrap();
    let items = context_menu::items(&app, &Target::Viewport);
    assert!(find(&items, "Press Pull").enabled);
    assert!(find(&items, "Edit Profile Sketch").enabled);
    assert!(!items.iter().any(|i| i.label == "Redefine Sketch Plane"), "the face menu, not the sketch menu");
}

#[test]
fn face_menu_hides_and_finds_the_body() {
    let mut app = sample_app();
    let b = body(&app);
    app.run("select.set", json!({"items": [{"type": "face", "body": b, "index": 0, "point": [0, 0, 0]}]})).unwrap();
    let items = context_menu::items(&app, &Target::Viewport);
    for l in ["Create Sketch", "Offset Plane", "Press Pull", "Extrude", "Measure", "Find in Browser", "Find in Timeline"] {
        assert!(find(&items, l).enabled, "{l}");
    }
    assert!(find(&items, "Appearance").enabled);
    context_menu::run_item(&mut app, &find(&items, "Find in Timeline"), pos2(0.0, 0.0));
    assert!(matches!(app.session.selection.as_slice(), [solidcraft_engine::Sel::Feature { .. }]));
    context_menu::run_item(&mut app, &find(&items, "Show/Hide"), pos2(0.0, 0.0));
    assert_eq!(app.session.visibility.hidden_bodies, vec![b]);
}

#[test]
fn body_menu_isolates_renames_locks_and_deletes() {
    let mut app = sample_app();
    app.run("solid.box", json!({"length": 5, "width": 5, "height": 5, "corner": [200, 0, 0]})).unwrap();
    let b = body(&app);
    let t = Target::Body { name: b.clone() };
    let items = context_menu::items(&app, &t);
    context_menu::run_item(&mut app, &find(&items, "Isolate"), pos2(0.0, 0.0));
    assert_eq!(app.session.visibility.hidden_bodies.len(), 1, "the other body is hidden");
    assert!(!app.session.visibility.hidden_bodies.contains(&b));
    // Rename through the rename box.
    context_menu::run_item(&mut app, &find(&items, "Rename"), pos2(10.0, 10.0));
    assert!(context_menu::finish_rename(&mut app, Some("Base"), true));
    assert!(app.session.model.state().body("Base").is_some());
    // Locked bodies can't be moved or deleted.
    let t = Target::Body { name: "Base".into() };
    act(&mut app, &t, "Lock");
    let items = context_menu::items(&app, &t);
    assert!(!find(&items, "Delete").enabled && !find(&items, "Move/Copy").enabled);
    context_menu::run_item(&mut app, &find(&items, "Unlock"), pos2(0.0, 0.0));
    let n = app.session.model.state().bodies.len();
    act(&mut app, &t, "Delete");
    assert_eq!(app.session.model.state().bodies.len(), n - 1);
}

#[test]
fn properties_measure_the_body() {
    let mut app = sample_app();
    let b = body(&app);
    let items = context_menu::items(&app, &Target::Body { name: b });
    context_menu::run_item(&mut app, &find(&items, "Properties"), pos2(0.0, 0.0));
    let (_, v) = app.menu.props.clone().unwrap();
    assert!(v["total"]["volume_mm3"].as_f64().unwrap() > 1000.0);
    assert!(v["total"]["mass_g"].as_f64().unwrap() > 0.0);
}

#[test]
fn component_properties_measure_one_assembly_placement() {
    let mut session = Session::default();
    let component = session.execute("component.create", &json!({"name": "Subassembly"})).unwrap()["component"].as_u64().unwrap();
    session.execute("component.create", &json!({"name": "Block"})).unwrap();
    session.execute("solid.box", &json!({"length": 10, "width": 10, "height": 10})).unwrap();
    session.execute("occurrence.copy", &json!({"component": component, "translate": [100, 0, 0]})).unwrap();
    let mut app = SolveApp::new(session, Services::default());
    act(&mut app, &Target::Component { id: component }, "Properties");
    let (title, properties) = app.menu.props.as_ref().unwrap();
    assert_eq!(title, "Subassembly");
    assert_eq!(properties["body_count"], 1);
    assert!((properties["total"]["volume_mm3"].as_f64().unwrap() - 1000.0).abs() < 1e-6);
    assert!((properties["total"]["center_of_mass"][0].as_f64().unwrap() - 5.0).abs() < 1e-6);
    act(&mut app, &Target::Component { id: 0 }, "Properties");
    assert_eq!(app.menu.props.as_ref().unwrap().1["body_count"], 2);
}

#[test]
fn sketch_menu_hides_and_moves_to_another_plane() {
    let mut app = sample_app();
    let id = app.session.doc.features.iter().find(|f| f.name == "Holes").unwrap().id;
    let t = Target::Sketch { id };
    let items = context_menu::items(&app, &t);
    let vis = crate::browser::sketch_visible(&app, id);
    context_menu::run_item(&mut app, &find(&items, "Show/Hide"), pos2(0.0, 0.0));
    assert_eq!(crate::browser::sketch_visible(&app, id), !vis);
    context_menu::run_item(&mut app, &find(&items, "Show Dimension"), pos2(0.0, 0.0));
    assert_eq!(app.ui.shown_dims, vec![id]);
    assert!(find(&context_menu::items(&app, &t), "Hide Dimension").enabled);
    context_menu::run_item(&mut app, &find(&items, "Edit Sketch"), pos2(0.0, 0.0));
    assert_eq!(app.session.active_sketch, Some(id));
    app.finish_sketch();
    context_menu::run_item(&mut app, &find(&items, "Redefine Sketch Plane"), pos2(0.0, 0.0));
    assert_eq!(app.tree.redefine, Some(id));
}

#[test]
fn sketch_entities_offer_the_constraints_that_fit() {
    let mut app = sample_app();
    app.run("sketch.create", json!({"plane": "XY"})).unwrap();
    app.run("sketch.line", json!({"points": [[0, 0], [10, 1]]})).unwrap();
    app.run("sketch.line", json!({"points": [[0, 5], [10, 7]]})).unwrap();
    let ids: Vec<String> = {
        let st = app.session.model.state();
        let ss = st.sketch(app.session.active_sketch.unwrap()).unwrap();
        ss.sketch.curves.iter().map(|c| c.id.clone()).collect()
    };
    app.run("select.set", json!({"items": ids.iter().map(|i| json!({"type": "sketch_curve", "id": i})).collect::<Vec<_>>()})).unwrap();
    let items = context_menu::items(&app, &Target::Viewport);
    let par = find(&items, "Parallel");
    assert!(find(&items, "Finish Sketch").enabled);
    context_menu::run_item(&mut app, &par, pos2(0.0, 0.0));
    let st = app.session.model.state();
    let ss = st.sketch(app.session.active_sketch.unwrap()).unwrap();
    assert!(ss.sketch.constraints.iter().any(|c| format!("{:?}", c.kind).contains("Parallel")), "{:?}", ss.sketch.constraints);
}

#[test]
fn component_menu_activates_grounds_and_pastes() {
    let mut app = sample_app();
    let b = body(&app);
    app.run("component.from_bodies", json!({"bodies": [b]})).unwrap();
    let c = app.session.doc.components[0].id;
    let t = Target::Component { id: c };
    act(&mut app, &t, "Activate");
    assert_eq!(app.session.active_component, c);
    act(&mut app, &t, "Ground");
    assert!(app.session.doc.occurrence_of(c).unwrap().grounded);
    assert!(!find(&context_menu::items(&app, &t), "Move/Copy").enabled, "grounded");
    act(&mut app, &t, "Paste New");
    assert_eq!(app.session.doc.components.len(), 2);
}

#[test]
fn pick_runs_an_item_of_the_open_menu() {
    let mut app = sample_app();
    assert!(context_menu::pick(&mut app, "Fit").is_err(), "no menu open");
    context_menu::open_for(&mut app, pos2(500.0, 400.0), Target::Viewport);
    assert!(context_menu::pick(&mut app, "Nope").is_err());
    app.session.undo.clear();
    assert!(context_menu::pick(&mut app, "Undo").is_err(), "disabled");
    assert!(context_menu::pick(&mut app, "ui.origin").is_ok());
    assert!(app.viewport.context_menu.is_none(), "a pick closes the menu");
    assert!(!app.ui.show_origin);
}

#[test]
fn folder_and_group_menus_make_rename_and_drop_groups() {
    let mut app = sample_app();
    app.run("solid.box", json!({"length": 5, "width": 5, "height": 5, "corner": [200, 0, 0]})).unwrap();
    let names: Vec<String> = app.session.model.state().bodies.iter().map(|b| b.name.clone()).collect();
    let items: Vec<_> = names.iter().map(|n| json!({"type": "body", "name": n})).collect();
    app.run("select.set", json!({ "items": items })).unwrap();
    let folder = Target::Folder { component: 0, folder: "bodies".into() };
    act(&mut app, &folder, "Group Selected");
    let g = app.session.doc.browser_groups[0].clone();
    assert_eq!(g.items.len(), 2);
    // A body's menu offers grouping too while several are selected.
    assert!(context_menu::items(&app, &Target::Body { name: names[0].clone() }).iter().any(|i| i.label == "Group Selected"));
    let t = Target::Group { id: g.id };
    act(&mut app, &t, "Rename");
    assert!(context_menu::finish_rename(&mut app, Some("Parts"), true));
    assert_eq!(app.session.doc.browser_groups[0].name, "Parts");
    act(&mut app, &t, "Show/Hide");
    assert_eq!(app.session.visibility.hidden_bodies.len(), 2);
    act(&mut app, &t, "Ungroup");
    assert!(app.session.doc.browser_groups.is_empty());
    act(&mut app, &folder, "New Group");
    assert_eq!(app.session.doc.browser_groups[0].items.len(), 0);
}

#[test]
fn browser_folds_are_kept_in_the_preferences() {
    let mut app = sample_app();
    app.tree.collapsed.insert("c0/sketches".into());
    app.tree.expanded.insert("origin".into());
    let prefs = app.prefs();
    let mut other = sample_app();
    other.load_prefs(&prefs);
    assert!(other.tree.collapsed.contains("c0/sketches") && other.tree.expanded.contains("origin"));
}

#[test]
fn delete_key_handles_every_kind_and_asks_about_dependents() {
    use solidcraft_engine::Sel;
    let mut app = sample_app();
    app.run("construct.plane.offset", json!({"base": "XY", "offset": 10})).unwrap();
    app.run("solid.box", json!({"length": 5, "width": 5, "height": 5, "corner": [200, 0, 0]})).unwrap();
    let n = app.session.doc.features.len();
    // A body and a construction plane together: one undo step, no question.
    let b = app.session.model.state().bodies.last().unwrap().name.clone();
    app.run("select.set", json!({"items": [{"type": "body", "name": b}, {"type": "plane", "name": "Plane1"}]})).unwrap();
    crate::delete::delete_selection(&mut app);
    assert!(app.menu.confirm.is_none());
    assert!(app.session.model.state().body(&b).is_none());
    assert!(app.session.doc.find_feature("Plane1").is_none());
    app.run("edit.undo", json!({})).unwrap();
    assert_eq!(app.session.doc.features.len(), n);
    // A sketch with features built on it: the confirmation lists them first.
    let sk = app.session.doc.features.iter().find(|f| f.name == "Base").unwrap().id;
    app.run("select.set", json!({"items": [Sel::Feature { id: sk }]})).unwrap();
    crate::delete::delete_selection(&mut app);
    let pending = app.menu.confirm.clone().expect("asks first");
    assert!(pending.report["deleted"].as_array().unwrap().len() > 1);
    assert!(crate::delete::confirm(&mut app, false));
    assert_eq!(app.session.doc.features.len(), n, "cancel keeps everything");
    crate::delete::delete_selection(&mut app);
    crate::delete::confirm(&mut app, true);
    assert!(app.session.doc.features.len() < n);
    app.run("edit.undo", json!({})).unwrap();
    assert_eq!(app.session.doc.features.len(), n);
    // A face its neighbours can't close (a side of the plate): refused, nothing changes.
    let body = &app.session.model.state().bodies[0];
    let face = body.body.faces(0.01).unwrap().into_iter().find(|f| f.plane_normal.is_some_and(|n| n.x < -0.99)).unwrap();
    let b0 = body.name.clone();
    app.run("select.set", json!({"items": [{"type": "face", "body": b0, "index": face.index, "point": face.centroid}]})).unwrap();
    crate::delete::delete_selection(&mut app);
    assert!(app.status.as_ref().is_some_and(|s| s.0.contains("Delete Face")), "{:?}", app.status);
    assert_eq!(app.session.doc.features.len(), n);
}

#[test]
fn menu_delete_items_use_the_same_path() {
    let mut app = sample_app();
    let b = body(&app);
    let original_volume = solidcraft_engine::kernel::measure(&app.session.world_state().body(&b).unwrap().body).unwrap().volume;
    let created = app.run("component.from_bodies", json!({"bodies": [b]})).unwrap();
    let c = app.session.doc.components[0].id;
    let transfer = created["components"][0]["feature"].as_u64().unwrap();
    let transfer_name = app.session.doc.feature(transfer).unwrap().name.clone();
    let saved = app.session.doc.to_json();
    act(&mut app, &Target::Component { id: c }, "Delete");
    let confirmation = app.menu.confirm.as_ref().expect("recorded transfer history needs confirmation");
    assert!(confirmation.report["deleted"].as_array().unwrap().iter().any(|name| name.as_str() == Some(transfer_name.as_str())));
    assert!(crate::delete::confirm(&mut app, false));
    assert_eq!(app.session.doc.to_json(), saved, "cancel keeps component and transfer history");
    act(&mut app, &Target::Component { id: c }, "Delete");
    assert!(crate::delete::confirm(&mut app, true));
    assert!(app.session.doc.components.is_empty());
    assert!(app.session.doc.feature(transfer).is_none());
    let restored = app.session.world_state();
    assert_eq!(restored.bodies.len(), 1, "the original joined plate stays intact");
    let plate = restored.body(&b).unwrap();
    assert_eq!(plate.component, 0);
    assert!((solidcraft_engine::kernel::measure(&plate.body).unwrap().volume - original_volume).abs() < 1e-6);
    assert!(plate.mesh().contains(solidcraft_engine::geom::Vec3::new(40.0, 35.0, 12.0)));
    let mut reopened = solidcraft_engine::Session::new(solidcraft_engine::doc::Document::from_json(&app.session.doc.to_json()).unwrap());
    reopened.refresh();
    let reopened_state = reopened.world_state();
    assert_eq!(reopened_state.bodies.len(), 1);
    assert_eq!(reopened_state.body(&b).unwrap().component, 0);
    assert!((solidcraft_engine::kernel::measure(&reopened_state.body(&b).unwrap().body).unwrap().volume - original_volume).abs() < 1e-6);
    // A sketch from the browser: asks, since its features go too.
    let id = app.session.doc.features.iter().find(|f| f.name == "Base").unwrap().id;
    act(&mut app, &Target::Sketch { id }, "Delete");
    assert!(app.menu.confirm.is_some());
}

#[test]
fn canvas_menu_hides_renames_calibrates_and_deletes() {
    let mut app = sample_app();
    let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
    png.extend(400u32.to_be_bytes());
    png.extend(200u32.to_be_bytes());
    png.extend([8, 6, 0, 0, 0, 0, 0, 0, 0]);
    let data = solidcraft_engine::doc::canvas::base64_encode(&png);
    let id = app.run("canvas.insert", json!({"data": data, "plane": "XY", "width": 80})).unwrap()["canvas"].as_u64().unwrap();
    let t = Target::Canvas { id };
    act(&mut app, &t, "Show/Hide");
    assert!(!app.session.doc.canvases[0].visible);
    act(&mut app, &t, "Rename");
    assert!(context_menu::finish_rename(&mut app, Some("Photo"), true));
    assert_eq!(app.session.doc.canvases[0].name, "Photo");
    act(&mut app, &t, "Calibrate");
    let p = app.tree.canvas_panel.clone().unwrap();
    assert!(p.calibrate && (p.distance - 80.0).abs() < 1e-9, "starts from the image's width");
    act(&mut app, &t, "Delete");
    assert!(app.session.doc.canvases.is_empty());
    app.run("edit.undo", json!({})).unwrap();
    assert_eq!(app.session.doc.canvases.len(), 1);
    // A canvas picked in the browser goes with the Delete key.
    app.tree.picked_canvases = vec![id];
    crate::delete::delete_selection(&mut app);
    assert!(app.session.doc.canvases.is_empty());
}

/// The canvas and browser menus keep Fusion's items in Fusion's order (the ones we have).
#[test]
fn menus_follow_fusion_order() {
    let mut app = sample_app();
    let b = body(&app);
    let order = |items: &[Item], want: &[&str]| {
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        let pos: Vec<usize> = want.iter().map(|w| labels.iter().position(|l| l == w).unwrap_or_else(|| panic!("no `{w}` in {labels:?}"))).collect();
        assert!(pos.windows(2).all(|p| p[0] < p[1]), "{want:?} out of order in {labels:?}");
    };
    app.run("select.set", json!({"items": [{"type": "face", "body": b, "index": 0, "point": [0, 0, 0]}]})).unwrap();
    let face = context_menu::items(&app, &Target::Viewport);
    order(
        &face,
        &[
            "Create Sketch",
            "Extrude",
            "Offset Plane",
            "Shell",
            "Edit Feature",
            "Edit Profile Sketch",
            "Appearance",
            "Properties",
            "Delete",
            "Show/Hide",
            "Find in Browser",
        ],
    );
    assert_eq!(find(&face, "Extrude").shortcut, "E");
    assert!(find(&face, "Edit Profile Sketch").enabled, "the plate's extrude has a sketch");
    let body_menu = context_menu::items(&app, &Target::Body { name: b.clone() });
    order(
        &body_menu,
        &[
            "Move/Copy",
            "Create Components from Bodies",
            "Physical Material",
            "Appearance",
            "Properties",
            "Save As Mesh",
            "Delete",
            "Remove",
            "Rename",
            "Show/Hide",
            "Isolate",
        ],
    );
    let id = app.session.doc.features.iter().find(|f| f.name == "Base").unwrap().id;
    let sketch = context_menu::items(&app, &Target::Sketch { id });
    order(
        &sketch,
        &[
            "Extrude",
            "Edit Sketch",
            "Redefine Sketch Plane",
            "Export DXF…",
            "Delete",
            "Rename",
            "Look At",
            "Hide Profile",
            "Show Dimension",
            "Show/Hide",
            "Find in Timeline",
        ],
    );
    let folder = context_menu::items(&app, &Target::Folder { component: 0, folder: "bodies".into() });
    order(&folder, &["Create Components from Bodies", "New Group", "Show/Hide", "Show All"]);
    let origin = context_menu::items(&app, &Target::Origin);
    order(&origin, &["Show/Hide", "Show All", "Hide Planes", "Hide Axes"]);
    context_menu::run_item(&mut app, &find(&origin, "Hide Planes"), pos2(0.0, 0.0));
    assert!(["XY", "XZ", "YZ"].iter().all(|p| app.session.visibility.hidden_origin.iter().any(|h| h == p)));
    app.run("select.clear", json!({})).unwrap();
    let empty = context_menu::items(&app, &Target::Viewport);
    order(&empty, &["Pan", "Zoom", "Orbit", "Show All", "Unisolate", "Extrude", "Fillet"]);
    context_menu::run_item(&mut app, &find(&empty, "Pan"), pos2(0.0, 0.0));
    assert!(matches!(app.viewport.nav, Some(crate::viewport::NavMode::Pan)));
}

#[test]
fn v_toggles_what_is_selected() {
    let mut app = sample_app();
    let b = body(&app);
    let sk = app.session.doc.features.iter().find(|f| f.name == "Base").unwrap().id;
    app.run("select.set", json!({"items": [{"type": "body", "name": b}, {"type": "plane", "name": "XY"}, {"type": "feature", "id": sk}]})).unwrap();
    crate::browser::set_sketch_visible(&mut app, sk, true);
    context_menu::toggle_visibility(&mut app);
    assert!(app.session.visibility.hidden_bodies.contains(&b) && app.session.visibility.hidden_origin.iter().any(|h| h == "XY"));
    assert!(!crate::browser::sketch_visible(&app, sk));
    context_menu::toggle_visibility(&mut app);
    assert!(app.session.visibility.hidden_bodies.is_empty() && app.session.visibility.hidden_origin.is_empty());
    assert!(crate::browser::sketch_visible(&app, sk));
    // Components picked in the browser hide their bodies.
    app.run("component.from_bodies", json!({"bodies": [b.clone()]})).unwrap();
    app.run("select.clear", json!({})).unwrap();
    app.tree.picked_components = vec![app.session.doc.components[0].id];
    context_menu::toggle_visibility(&mut app);
    assert_eq!(app.session.visibility.hidden_bodies, vec![b]);
    assert_eq!(find(&context_menu::items(&app, &Target::Body { name: body(&app) }), "Show/Hide").shortcut, "V");
}

#[test]
fn named_views_save_restore_rename_and_delete() {
    let mut app = sample_app();
    act(&mut app, &Target::NamedViews, "New Named View");
    assert_eq!(app.session.doc.named_views.len(), 1);
    let saved = app.cam;
    app.cam.set_view(solidcraft_engine::render::StandardView::Top);
    let t = Target::NamedView { name: "Named View1".into() };
    act(&mut app, &t, "Restore");
    assert_eq!(app.cam_anim.map(|a| a.to), Some(saved), "animates back to the saved camera");
    act(&mut app, &t, "Rename");
    assert!(context_menu::finish_rename(&mut app, Some("Front detail"), true));
    let t = Target::NamedView { name: "Front detail".into() };
    act(&mut app, &t, "Delete");
    assert!(app.session.doc.named_views.is_empty());
    // Standard views restore but can't be renamed or deleted.
    let top = context_menu::items(&app, &Target::NamedView { name: "Top".into() });
    assert!(!find(&top, "Delete").enabled);
    context_menu::run_item(&mut app, &find(&top, "Restore"), pos2(0.0, 0.0));
    assert!(app.cam_anim.is_some());
}

#[test]
fn units_menu_changes_the_design_units_and_new_designs_take_the_preference() {
    let mut app = sample_app();
    let items = context_menu::items(&app, &Target::Units);
    assert_eq!(items.len(), 5);
    act(&mut app, &Target::Units, "in");
    assert_eq!(app.session.doc.units, "in");
    assert!(crate::prefs::show_mm(&app, 25.4, 1).starts_with("1.000 in"));
    app.preferences.default_units = "cm".into();
    crate::documents::new_design(&mut app);
    assert_eq!(app.session.doc.units, "cm");
    assert!(!app.session.is_dirty(), "a fresh design stays unmodified");
}

#[test]
fn an_uncaptured_move_asks_before_the_next_command() {
    let mut app = sample_app();
    let b = body(&app);
    app.run("component.from_bodies", json!({"bodies": [b]})).unwrap();
    let occ = app.session.doc.occurrences[0].id;
    app.run("occurrence.move", json!({"occurrence": occ, "translate": [10, 0, 0], "capture": false})).unwrap();
    app.start("sketch.create");
    assert_eq!(app.tree.capture_prompt.as_deref(), Some("sketch.create"), "asks first");
    assert!(app.dialog.is_none());
    crate::browser::answer_capture(&mut app, "cancel").unwrap();
    assert!(app.tree.capture_prompt.is_none() && !app.session.pending_moves.is_empty());
    // Revert, then the command goes ahead.
    app.start("sketch.create");
    crate::browser::answer_capture(&mut app, "revert").unwrap();
    assert!(app.session.pending_moves.is_empty());
    assert!(app.dialog.is_some(), "Create Sketch started");
    assert_eq!(app.session.doc.occurrences[0].transform, solidcraft_engine::doc::IDENTITY);
    // Capture keeps the move.
    app.dialog = None;
    app.run("occurrence.move", json!({"occurrence": occ, "translate": [10, 0, 0], "capture": false})).unwrap();
    app.start("solid.extrude");
    crate::browser::answer_capture(&mut app, "capture").unwrap();
    assert_eq!(app.session.doc.occurrences[0].transform[3][0], 10.0);
    assert!(crate::browser::answer_capture(&mut app, "capture").is_err(), "nothing waits now");
}

#[test]
fn f2_renames_the_selected_item() {
    let mut app = sample_app();
    let b = body(&app);
    app.run("select.set", json!({"items": [{"type": "body", "name": b}]})).unwrap();
    assert!(context_menu::rename_selection(&mut app, pos2(0.0, 0.0)));
    assert!(context_menu::finish_rename(&mut app, Some("Base"), true));
    assert!(app.session.model.state().body("Base").is_some());
    app.run("select.clear", json!({})).unwrap();
    assert!(!context_menu::rename_selection(&mut app, pos2(0.0, 0.0)), "nothing selected");
}

#[test]
fn occurrence_moves_turn_about_their_origin_then_move() {
    let mut app = sample_app();
    let b = body(&app);
    app.run("component.from_bodies", json!({"bodies": [b]})).unwrap();
    let occ = app.session.doc.occurrences[0].id;
    let mut m = crate::browser::OccMove::new(occ);
    m.pivot = Some(solidcraft_engine::geom::Vec3::new(5.0, 0.0, 0.0));
    m.translate = [10.0, 0.0, 0.0];
    m.angles = [0.0, 0.0, 90.0];
    crate::browser::apply_occurrence_move(&mut app, &m);
    let t = *app.session.pending_moves.get(&occ).unwrap();
    // The pivot lands 10 mm along X; X turns into Y.
    let p = solidcraft_engine::doc::apply_point(&t, solidcraft_engine::geom::Vec3::new(5.0, 0.0, 0.0));
    let x = solidcraft_engine::doc::apply_vector(&t, solidcraft_engine::geom::Vec3::X);
    assert!((p.x - 15.0).abs() < 1e-9 && p.y.abs() < 1e-9, "{p:?}");
    assert!((x.y - 1.0).abs() < 1e-9, "{x:?}");
    // Changing the values again starts from where it was, not from the last move.
    m.translate = [0.0, 0.0, 0.0];
    m.angles = [0.0; 3];
    crate::browser::apply_occurrence_move(&mut app, &m);
    assert_eq!(*app.session.pending_moves.get(&occ).unwrap(), solidcraft_engine::doc::IDENTITY);
}

#[test]
fn toolbar_pins_removes_reorders_and_keeps_its_layout() {
    use crate::toolbar_custom as tc;
    let mut app = sample_app();
    app.ui.tab = "MODEL".into();
    let ids = |app: &SolveApp, panel: &str| -> Vec<&'static str> {
        let lists = tc::panel_lists("MODEL");
        let (_, cmds, n) = lists.iter().find(|(p, ..)| *p == panel).unwrap();
        tc::buttons(app, "MODEL", panel, cmds, *n).iter().map(|c| c.id).collect()
    };
    let create = ids(&app, "CREATE");
    // Pin a command that isn't a button yet: it lands at the end of its panel.
    let extra =
        tc::panel_lists("MODEL").iter().find(|(p, ..)| *p == "CREATE").unwrap().1.iter().map(|c| c.id).find(|id| !create.contains(id)).unwrap();
    act(&mut app, &Target::ToolbarCommand { id: extra.into() }, "Pin to Toolbar");
    assert_eq!(ids(&app, "CREATE").last(), Some(&extra));
    // Remove a default button.
    act(&mut app, &Target::ToolbarCommand { id: create[0].into() }, "Remove from Toolbar");
    assert!(!ids(&app, "CREATE").contains(&create[0]));
    // Reorder: the pinned one before the second default; then into another panel.
    tc::reorder(&mut app, "MODEL", extra, "CREATE", Some(create[1]));
    assert_eq!(ids(&app, "CREATE")[0], extra);
    tc::reorder(&mut app, "MODEL", extra, "MODIFY", None);
    assert!(!ids(&app, "CREATE").contains(&extra) && ids(&app, "MODIFY").last() == Some(&extra));
    // Pin to Shortcuts goes to the S box's favourites.
    act(&mut app, &Target::ToolbarCommand { id: "solid.extrude".into() }, "Pin to Shortcuts");
    assert!(app.sbox.pinned.iter().any(|p| p == "solid.extrude"));
    // Kept between runs, and exported and imported with the shortcuts.
    let mut other = sample_app();
    other.load_prefs(&app.prefs());
    assert_eq!(other.toolbar_custom, app.toolbar_custom);
    let file = tc::export(&app);
    let mut third = sample_app();
    tc::import(&mut third, &file).unwrap();
    assert_eq!(third.toolbar_custom, app.toolbar_custom);
    assert!(tc::import(&mut third, &json!({"format": "nope"})).is_err());
    // Reset Toolbar puts the tab back.
    act(&mut app, &Target::ToolbarCommand { id: extra.into() }, "Reset Toolbar");
    assert_eq!(ids(&app, "CREATE"), create);
}

#[test]
fn the_workspace_menu_lists_design_and_render_and_greys_future_workspaces() {
    let app = sample_app();
    let items = context_menu::items(&app, &Target::Workspace);
    assert!(find(&items, "Design").enabled);
    assert!(find(&items, "Render").enabled);
    assert!(!find(&items, "Animation").enabled);
    assert!(!items.iter().any(|i| i.label.contains("Generative")));
}

/// A right-click on a construction plane's browser row opens its menu, like a body's or a
/// sketch's row, and the menu's items act on that plane.
#[test]
fn construction_plane_row_opens_its_menu() {
    let mut h = crate::scenario::Harness::new();
    h.app.run("solid.box", json!({"length": 40, "width": 40, "height": 10, "body_name": "Block"})).unwrap();
    h.app.run("construct.plane.offset", json!({"base": "XY", "offset": 25, "name": "Cut"})).unwrap();
    let id = h.app.session.doc.find_feature("Cut").map(|f| f.id).unwrap();
    h.app.run("select.clear", json!({})).unwrap();
    h.frames(3);
    let at = crate::scenario::handle_at("row:Cut").expect("the plane has a browser row");
    h.call("ui.click", json!({"x": at.x, "y": at.y, "button": "right"}));
    h.frames(2);
    let menu = h.call("ui.menu", json!({}));
    let menu = &menu["result"];
    assert_eq!(menu["open"], true, "{menu}");
    assert_eq!(menu["target"], json!({"type": "plane", "id": id}), "{menu}");
    let labels: Vec<&str> = menu["items"].as_array().unwrap().iter().filter_map(|i| i["label"].as_str()).filter(|l| !l.is_empty()).collect();
    assert_eq!(labels, ["Create Sketch", "Edit Feature", "Delete", "Rename", "Show/Hide", "Find in Timeline"]);
    assert_eq!(h.app.session.selection, vec![solidcraft_engine::Sel::Plane { name: "Cut".into() }], "the right-click picks the plane");
    context_menu::close(&mut h.app);

    let t = Target::Plane { id };
    act(&mut h.app, &t, "Show/Hide");
    assert!(h.app.session.visibility.hidden_origin.contains(&"Cut".to_string()));
    act(&mut h.app, &t, "Show/Hide");
    assert!(!h.app.session.visibility.hidden_origin.contains(&"Cut".to_string()));
    act(&mut h.app, &t, "Rename");
    assert!(context_menu::finish_rename(&mut h.app, Some("Section"), true));
    assert!(h.app.session.doc.find_feature("Section").is_some_and(|f| f.id == id));
    act(&mut h.app, &t, "Find in Timeline");
    assert_eq!(h.app.session.selection, vec![solidcraft_engine::Sel::Feature { id }]);
    act(&mut h.app, &t, "Edit Feature");
    assert!(h.app.dialog.is_some(), "Edit Feature opens the plane's dialog");
    h.app.dialog = None;
    act(&mut h.app, &t, "Delete");
    assert!(h.app.session.doc.feature(id).is_none(), "nothing depends on the plane: it goes at once");
}

/// A right-click on a plane offers a sketch on it (#1354): an origin plane in the viewport (it
/// stays unselected), an origin plane's browser row and a construction plane's row. Picking it
/// starts the sketch on that plane.
#[test]
fn right_click_on_a_plane_offers_a_sketch_on_it() {
    use solidcraft_engine::geom::Vec3;
    let normal = |h: &crate::scenario::Harness| h.app.session.world_state().sketches.first().map(|s| s.plane.normal()).unwrap_or_default();
    for (plane, axis) in [("XY", Vec3::Z), ("XZ", Vec3::Y), ("YZ", Vec3::X)] {
        let mut h = crate::scenario::Harness::new();
        h.app.home.open = false;
        h.settle();
        let p = h.at(&json!({"plane": plane})).unwrap();
        h.call("ui.menu", json!({"x": p[0], "y": p[1]}));
        let menu = h.call("ui.menu", json!({}))["result"].clone();
        let items = menu["items"].as_array().unwrap();
        let labels: Vec<&str> = items.iter().filter_map(|i| i["label"].as_str()).filter(|l| !l.is_empty()).collect();
        assert_eq!(labels[..3], ["Select Other…", "Create Sketch", "Pan"], "{plane}: {labels:?}");
        let sketch = items.iter().find(|i| i["label"] == "Create Sketch").unwrap();
        assert_eq!(sketch["params"], json!({"plane": plane}));
        assert!(h.app.session.selection.is_empty(), "the right-click leaves the plane unselected");
        assert_eq!(h.call("ui.menuPick", json!({"item": "Create Sketch"}))["ok"], true);
        assert!(h.app.session.active_sketch.is_some(), "{plane}: picking it starts a sketch");
        assert!(normal(&h).dot(axis).abs() > 0.999, "{plane}: sketched on {:?}", normal(&h));
    }

    let mut h = crate::scenario::Harness::new();
    h.app.home.open = false;
    crate::browser::toggle(&mut h.app, "origin", false);
    h.frames(3);
    let at = crate::scenario::handle_at("row:XZ").expect("the Origin folder lists XZ");
    h.call("ui.click", json!({"x": at.x, "y": at.y, "button": "right"}));
    h.frames(2);
    let menu = h.call("ui.menu", json!({}))["result"].clone();
    assert_eq!(menu["target"], json!({"type": "origin_plane", "name": "XZ"}), "{menu}");
    let t = Target::OriginPlane { name: "XZ".into() };
    act(&mut h.app, &t, "Show/Hide");
    assert!(h.app.session.visibility.hidden_origin.contains(&"XZ".to_string()));
    act(&mut h.app, &t, "Show/Hide");
    assert!(!h.app.session.visibility.hidden_origin.contains(&"XZ".to_string()));
    context_menu::close(&mut h.app);
    // The toolbar's Create Sketch is already waiting for a plane: the menu's replaces it.
    h.app.start("sketch.create");
    assert!(h.app.dialog.is_some());
    act(&mut h.app, &t, "Create Sketch");
    h.frames(3);
    assert!(h.app.dialog.is_none(), "no Create Sketch dialog is left waiting for a plane");
    assert!(normal(&h).dot(Vec3::Y).abs() > 0.999);

    let mut h = crate::scenario::Harness::new();
    h.app.run("construct.plane.offset", json!({"base": "XY", "offset": 25, "name": "Cut"})).unwrap();
    let id = h.app.session.doc.find_feature("Cut").map(|f| f.id).unwrap();
    act(&mut h.app, &Target::Plane { id }, "Create Sketch");
    h.frames(3);
    let sketch = h.app.session.active_sketch.and_then(|s| h.app.session.world_state().sketch(s).map(|s| s.plane.origin.z));
    assert_eq!(sketch, Some(25.0), "the sketch is on the construction plane");
}

/// Menus opened near the window's bottom right corner, in the viewport or the browser, open up
/// or to the left where they would not fit, ending at the cursor and staying in the window, at
/// any UI scale (#1354).
#[test]
fn menus_near_the_window_corner_open_inward() {
    for scale in [0.7, 1.0, 1.5] {
        let mut h = crate::scenario::Harness::new();
        h.app.home.open = false;
        h.call("ui.prefs", json!({"ui_scale": scale}));
        let screen = h.ctx.content_rect();
        let vp = h.app.viewport.rect.unwrap();
        let at = vp.right_bottom() - egui::vec2(40.0, 40.0);
        for target in [None, Some(json!({"type": "origin"}))] {
            let mut params = json!({"x": at.x, "y": at.y});
            if let Some(t) = &target {
                params["target"] = t.clone();
            }
            h.call("ui.menu", params);
            assert_eq!(h.call("ui.menu", json!({}))["result"]["open"], true, "{scale} {target:?}");
            // An auto-sized menu takes a few frames to settle its size.
            h.frames(3);
            let r = h.ctx.memory(|m| m.area_rect(egui::Id::new("sc_context_menu"))).unwrap();
            assert!(screen.contains_rect(r), "{scale} {target:?}: {r:?} outside {screen:?}");
            // Below and right of the cursor where it fits, else ending at it.
            let x = if at.x + r.width() <= screen.right() { at.x } else { at.x - r.width() };
            let y = if at.y + r.height() <= screen.bottom() { at.y } else { at.y - r.height() };
            assert!((r.left() - x).abs() <= 2.0 && (r.top() - y).abs() <= 2.0, "{scale} {target:?}: {r:?} for {at:?} in {screen:?}");
            if target.is_none() {
                assert!((r.bottom() - at.y).abs() <= 2.0, "{scale}: the empty-space menu is too tall for below and opens above");
            }
            context_menu::close(&mut h.app);
            h.frames(2);
        }
    }
}
