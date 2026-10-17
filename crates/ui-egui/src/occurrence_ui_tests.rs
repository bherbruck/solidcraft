use crate::context_menu::{self, Target};
use crate::scenario::Harness;
use serde_json::json;
use solidcraft_engine::Sel;

#[test]
fn replaced_body_name_uses_evaluated_owner_in_browser_and_component_actions() {
    let mut h = Harness::new();
    h.app.run("solid.box", json!({"length":10,"width":10,"height":10,"body_name":"Block"})).unwrap();
    let transferred = h.app.run("component.from_bodies", json!({"bodies":["Block"]})).unwrap();
    let owner = transferred["components"][0]["component"].as_u64().unwrap();
    assert_eq!(context_menu::component_bodies(&h.app, owner), ["Block"]);
    h.app.run("solid.remove", json!({"bodies":["Block"]})).unwrap();
    h.app.run("solid.box", json!({"corner":[30,0,0],"length":5,"width":5,"height":5,"body_name":"Block"})).unwrap();
    h.frames(2);
    let state = h.app.session.model.state();
    let body = state.body("Block").unwrap();
    assert_eq!(h.app.session.doc.model_body_component(body), 0);
    assert_eq!(context_menu::component_bodies(&h.app, 0), ["Block"]);
    assert!(context_menu::component_bodies(&h.app, owner).is_empty());
    let cache = h.app.tree.cache.as_ref().unwrap();
    assert_eq!(cache.root.folders[0].as_ref().unwrap().entries().map(|entry| entry.key.as_str()).collect::<Vec<_>>(), ["Block"]);
    let occurrence = h.app.session.doc.occurrence_of(owner).unwrap().id;
    assert!(cache.occurrences.get([occurrence].as_slice()).unwrap().folders[0].as_ref().unwrap().entries().next().is_none());
    let bounds = h.app.session.world_state().body("Block").unwrap().mesh().bounds();
    assert!((bounds.min.x - 30.0).abs() < 1e-9);
    assert!((bounds.max.x - 35.0).abs() < 1e-9);
}

fn fixture() -> (Harness, u64, u64, u64) {
    let mut h = Harness::new();
    let part = h.app.run("component.create", json!({"name":"Block"})).unwrap();
    let component = part["component"].as_u64().unwrap();
    let first = part["occurrence"].as_u64().unwrap();
    h.app.run("solid.box", json!({"length":10,"width":10,"height":10})).unwrap();
    h.app.run("component.activate", json!({"component":0})).unwrap();
    let second = h.app.run("occurrence.copy", json!({"occurrence":first,"translate":[50,0,0]})).unwrap()["occurrence"].as_u64().unwrap();
    h.frames(3);
    (h, component, first, second)
}

#[test]
fn browser_reuses_unchanged_rows_and_refreshes_live_state_and_replaced_documents() {
    let (mut h, _, first, second) = fixture();
    let second_body = h.app.session.occurrence_bodies(&[second], false).unwrap().pop().unwrap();
    let first_body = h.app.session.occurrence_bodies(&[first], false).unwrap().pop().unwrap();
    let row = |h: &Harness| {
        let cache = h.app.tree.cache.as_ref().unwrap();
        let entry = cache.occurrences.get([second].as_slice()).unwrap().folders[0].as_ref().unwrap().entries().next().unwrap();
        (entry.row_key.as_ptr(), entry.row_key.len())
    };
    let before = row(&h);
    assert_ne!(first_body, second_body);
    h.frames(3);
    assert_eq!(row(&h), before, "unchanged frames replaced cached row storage");
    h.app.run("select.set", json!({"items":[Sel::Body { name: second_body.clone() }]})).unwrap();
    h.app.session.visibility.hidden_bodies.push(second_body.clone());
    h.frames(2);
    let entry =
        h.app.tree.cache.as_ref().unwrap().occurrences.get([second].as_slice()).unwrap().folders[0].as_ref().unwrap().entries().next().unwrap();
    assert_eq!(entry.row_key.strip_prefix("b:"), Some(second_body.as_str()));
    assert!(entry.is_selected(&h.app));
    assert!(!entry.is_visible(&h.app));
    assert!(h.app.session.body_visible(&first_body));
    assert_eq!(row(&h), before, "selection or visibility rebuilt stable row data");

    let revision = h.app.session.revision;
    h.app.session = solidcraft_engine::Session::new(solidcraft_engine::doc::Document::new("Replacement"));
    h.app.session.revision = revision;
    h.frames(2);
    assert!(super::drawn_rows().iter().any(|row| row.0 == "Replacement"));
    assert!(super::drawn_occurrences().is_empty());
}

fn click(h: &mut Harness, path: &[u64], control: &str) {
    let point = h.call("ui.at", json!({"occurrence":path,"control":control}));
    assert_eq!(point["ok"], true, "{point}");
    let p = &point["result"];
    let result = h.call("ui.click", json!({"x":p[0],"y":p[1]}));
    assert_eq!(result["ok"], true);
}

#[test]
fn occurrence_second_row_selects_exact_path_and_menu_actions() {
    let (mut h, component, first, second) = fixture();
    click(&mut h, &[second], "row");
    assert_eq!(h.app.session.selection, vec![Sel::Occurrence { path: vec![second] }]);
    let bounds = crate::viewport::selection_bounds(&h.app).unwrap();
    assert!((bounds.center().x - 55.0).abs() < 1e-8);
    let target = Target::Occurrence { path: vec![second] };
    let items = context_menu::items(&h.app, &target);
    let independent = items.iter().find(|i| i.id == "component.paste_new").unwrap();
    assert_eq!(independent.label, "Paste New (Own Features)");
    assert_eq!(independent.params.as_ref().unwrap()["occurrence"], second);
    assert_eq!(independent.params.as_ref().unwrap()["path"], json!([second]));
    let properties = items.iter().find(|i| i.id == "ui.properties").unwrap();
    assert_eq!(properties.params.as_ref().unwrap()["occurrence"], second);
    context_menu::run_item(&mut h.app, properties, egui::Pos2::ZERO);
    assert!((h.app.menu.props.as_ref().unwrap().1["total"]["center_of_mass"][0].as_f64().unwrap() - 55.0).abs() < 1e-8);
    let movement = items.iter().find(|i| i.id == "ui.moveOccurrence").unwrap();
    context_menu::run_item(&mut h.app, movement, egui::Pos2::ZERO);
    assert_eq!(h.app.tree.occurrence_move.as_ref().unwrap().path, vec![second]);
    let ground = items.iter().find(|i| i.id == "occurrence.ground").unwrap();
    context_menu::run_item(&mut h.app, ground, egui::Pos2::ZERO);
    assert!(h.app.session.doc.occurrences.iter().find(|o| o.id == second).unwrap().grounded);
    assert!(!h.app.session.doc.occurrences.iter().find(|o| o.id == first).unwrap().grounded);
    h.app.tree.occurrence_move = None;
    click(&mut h, &[second], "activate");
    assert_eq!(h.app.session.active_component, component);
    assert_eq!(h.app.session.active_occurrence_path, Some(vec![second]));
}

#[test]
fn occurrence_eye_and_fold_are_independent_and_survive_rename() {
    let (mut h, component, first, second) = fixture();
    click(&mut h, &[second], "eye");
    assert!(h.app.session.occurrence_hidden(&[second]));
    assert!(h.app.session.occurrence_visible(&[first]));
    let second_body = h.app.session.occurrence_bodies(&[second], true).unwrap()[0].clone();
    assert!(!h.app.session.body_visible(&second_body));
    click(&mut h, &[second], "fold");
    assert!(h.app.tree.collapsed.contains(&format!("o:{second}")));
    assert!(!h.app.tree.collapsed.contains(&format!("o:{first}")));
    h.app.run("component.rename", json!({"component":component,"name":"Renamed Block"})).unwrap();
    h.frames(3);
    assert!(h.app.session.occurrence_hidden(&[second]));
    click(&mut h, &[second], "eye");
    assert!(h.app.session.occurrence_visible(&[second]));
}

#[test]
fn occurrence_move_dialog_keeps_other_pending_moves_and_reuses_initial_baseline() {
    let (mut h, _, first, second) = fixture();
    h.app.run("occurrence.move", json!({"occurrence":first,"translate":[0,10,0],"capture":false})).unwrap();
    h.app.run("occurrence.move", json!({"occurrence":second,"translate":[5,0,0],"capture":false})).unwrap();
    let first_pending = h.app.session.pending_moves[&first];
    let items = context_menu::items(&h.app, &Target::Occurrence { path: vec![second] });
    let movement = items.iter().find(|i| i.id == "ui.moveOccurrence").unwrap();
    context_menu::run_item(&mut h.app, movement, egui::Pos2::ZERO);
    let mut m = h.app.tree.occurrence_move.clone().unwrap();
    m.pivot =
        Some(solidcraft_engine::doc::apply_point(&h.app.session.occurrence_placement(&[second]).unwrap().world, solidcraft_engine::geom::Vec3::ZERO));
    m.translate = [10.0, 0.0, 0.0];
    crate::browser::apply_occurrence_move(&mut h.app, &m);
    let at = h.app.session.occurrence_placement(&[second]).unwrap();
    assert!((solidcraft_engine::doc::apply_point(&at.world, solidcraft_engine::geom::Vec3::ZERO).x - 65.0).abs() < 1e-8);
    m.translate = [3.0, 0.0, 0.0];
    crate::browser::apply_occurrence_move(&mut h.app, &m);
    let at = h.app.session.occurrence_placement(&[second]).unwrap();
    assert!((solidcraft_engine::doc::apply_point(&at.world, solidcraft_engine::geom::Vec3::ZERO).x - 58.0).abs() < 1e-8);
    assert_eq!(h.app.session.pending_moves[&first], first_pending);
    crate::browser::revert_occurrence_dialog(&mut h.app, &m);
    let restored = h.app.session.occurrence_placement(&[second]).unwrap();
    assert!((solidcraft_engine::doc::apply_point(&restored.world, solidcraft_engine::geom::Vec3::ZERO).x - 55.0).abs() < 1e-8);
    assert_eq!(h.app.session.pending_moves[&first], first_pending);
    h.app.run("component.revert_position", json!({"occurrences":[second]})).unwrap();
    context_menu::run_item(&mut h.app, movement, egui::Pos2::ZERO);
    let mut fresh = h.app.tree.occurrence_move.clone().unwrap();
    assert!(!fresh.had_pending);
    fresh.translate = [2.0, 0.0, 0.0];
    crate::browser::apply_occurrence_move(&mut h.app, &fresh);
    crate::browser::revert_occurrence_dialog(&mut h.app, &fresh);
    assert!(!h.app.session.pending_moves.contains_key(&second));
    assert_eq!(h.app.session.pending_moves[&first], first_pending);
}

#[test]
fn occurrence_delete_targets_second_copy_even_if_first_was_selected() {
    let (mut h, _, first, second) = fixture();
    click(&mut h, &[first], "row");
    let items = context_menu::items(&h.app, &Target::Occurrence { path: vec![second] });
    let delete = items.iter().find(|i| i.id == "ui.delete").unwrap();
    context_menu::run_item(&mut h.app, delete, egui::Pos2::ZERO);
    crate::delete::confirm(&mut h.app, true);
    assert!(h.app.session.doc.occurrences.iter().any(|o| o.id == first));
    assert!(!h.app.session.doc.occurrences.iter().any(|o| o.id == second));
    assert_eq!(h.app.session.world_state().bodies.len(), 1);
}

#[test]
fn occurrence_new_component_keeps_inactive_second_parent_authoring_context() {
    let (mut h, _, first, second) = fixture();
    assert_eq!(h.app.session.active_component, 0);
    let items = context_menu::items(&h.app, &Target::Occurrence { path: vec![second] });
    let create = items.iter().find(|i| i.id == "component.create").unwrap();
    assert_eq!(create.params.as_ref().unwrap()["parent_path"], json!([second]));
    context_menu::run_item(&mut h.app, create, egui::Pos2::ZERO);
    let active = h.app.session.active_occurrence_path.as_ref().unwrap();
    assert_eq!(active.len(), 2);
    assert_eq!(active[0], second);
    assert_ne!(active[0], first);
    h.app.run("solid.box", json!({"corner":[51,1,1],"length":2,"width":2,"height":2})).unwrap();
    let feature = h.app.session.model.state().bodies.last().unwrap().feature;
    let mut centers: Vec<f64> =
        h.app.session.world_state().bodies.iter().filter(|b| b.feature == feature).map(|b| b.mesh().bounds().center().x).collect();
    centers.sort_by(f64::total_cmp);
    assert_eq!(centers.len(), 2);
    assert!((centers[0] - 2.0).abs() < 1e-8 && (centers[1] - 52.0).abs() < 1e-8, "{centers:?}");
}

#[test]
fn occurrence_body_appearance_editor_inherits_definition_precision() {
    let (mut h, _, _, second) = fixture();
    h.app.run("appearance.assign", json!({"bodies":["Body1"],"color":"#123456","appearance":"Tinted source","opacity":0.123456})).unwrap();
    let name = h.app.session.occurrence_bodies(&[second], false).unwrap()[0].clone();
    h.app.session.selection = vec![Sel::Body { name }];
    let (kind, _) = crate::dialogs_appearance::start(&h.app, "appearance.assign").unwrap();
    let crate::dialogs::Kind::Appearance(look) = kind else { panic!("appearance editor") };
    assert_eq!(look.color, [0x12, 0x34, 0x56]);
    assert_eq!(look.opacity, 0.123456);
    assert_eq!(look.look.as_deref(), Some("Tinted source"));
}

#[test]
fn occurrence_nested_shared_child_has_distinct_rows_and_scope() {
    let mut h = Harness::new();
    let parent = h.app.run("component.create", json!({"name":"Assembly"})).unwrap();
    let parent_component = parent["component"].as_u64().unwrap();
    let parent_occurrence = parent["occurrence"].as_u64().unwrap();
    let child = h.app.run("component.create", json!({"name":"Child"})).unwrap()["occurrence"].as_u64().unwrap();
    h.app.run("solid.box", json!({"length":10,"width":10,"height":10})).unwrap();
    h.app.run("component.activate", json!({"component":0})).unwrap();
    let other = h.app.run("occurrence.copy", json!({"component":parent_component,"translate":[80,0,0]})).unwrap()["occurrence"].as_u64().unwrap();
    h.frames(3);
    click(&mut h, &[other, child], "row");
    assert_eq!(h.app.session.selection, vec![Sel::Occurrence { path: vec![other, child] }]);
    assert!((crate::viewport::selection_bounds(&h.app).unwrap().center().x - 85.0).abs() < 1e-8);
    let items = context_menu::items(&h.app, &Target::Occurrence { path: vec![other, child] });
    assert!(items.iter().any(|i| i.label.starts_with("Shared child:")));
    click(&mut h, &[other, child], "fold");
    assert!(h.app.tree.collapsed.contains(&format!("o:{other}/{child}")));
    assert!(!h.app.tree.collapsed.contains(&format!("o:{parent_occurrence}/{child}")));
    click(&mut h, &[other, child], "eye");
    assert!(h.app.session.occurrence_visible(&[parent_occurrence, child]));
    assert!(!h.app.session.occurrence_visible(&[other, child]));
    let delete = items.iter().find(|i| i.id == "ui.delete").unwrap();
    context_menu::run_item(&mut h.app, delete, egui::Pos2::ZERO);
    assert!(h.app.menu.confirm.is_some(), "shared child deletion must communicate scope before removing both renderings");
}

#[test]
fn occurrence_distance_editor_reads_inches_and_keeps_four_digits_in_model_space() {
    let (mut h, _, first, _) = fixture();
    h.app.run("document.units", json!({"units":"in"})).unwrap();
    let target = Target::Occurrence { path: vec![first] };
    let items = context_menu::items(&h.app, &target);
    let movement = items.iter().find(|i| i.id == "ui.moveOccurrence").unwrap();
    context_menu::run_item(&mut h.app, movement, egui::Pos2::ZERO);
    h.settle();
    let original = h.app.session.doc.to_json();
    let point = h.call("ui.at", json!({"handle":"occurrence-move:distance:0"}));
    let at = &point["result"];
    h.call("ui.click", json!({"x":at[0],"y":at[1],"double":true}));
    h.settle();
    h.step(&json!({"key":"A","cmd":true})).unwrap();
    h.step(&json!({"text":"1.2345"})).unwrap();
    h.step(&json!({"key":"Enter"})).unwrap();
    let m = h.app.tree.occurrence_move.as_ref().unwrap();
    assert!((m.translate[0] - 31.3563).abs() < 1e-8, "{:?}", m.translate);
    assert_eq!(h.app.session.doc.to_json(), original, "pending movement does not alter the stored design");
    h.app.session.selection = vec![Sel::Occurrence { path: vec![first] }];
    let bounds = crate::viewport::selection_bounds(&h.app).unwrap();
    assert!((bounds.min.x - 31.3563).abs() < 1e-6);
    let movement = h.app.tree.occurrence_move.clone().unwrap();
    crate::browser::revert_occurrence_dialog(&mut h.app, &movement);
    assert!(h.app.session.pending_moves.is_empty());
}
