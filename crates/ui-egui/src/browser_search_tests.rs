//! The browser's search (#1413): the field, the filtered tree, the chips and the actions.

use serde_json::{Value, json};
use solidcraft_engine::Sel;
use solidcraft_engine::browser_find::{Kind, Pattern, find};

use crate::scenario::{Harness, handle_at};

/// A base plate with a sketch and a plane; two columns with two M20 bolts each (a bolt has a
/// thread sketch); two more bolts and a washer on the plate, the washer held by a joint.
fn assembly() -> Harness {
    let mut h = Harness::new();
    h.app.home.open = false;
    let script = json!({"commands": [
        {"command": "solid.box", "params": {"length": 420, "width": 120, "height": 10, "corner": [-60, -40, -10], "body_name": "Base plate"}},
        {"command": "sketch.create", "params": {"plane": "XY", "name": "Bolt holes"}},
        {"command": "sketch.circle.center", "params": {"center": [40, 20], "diameter": 22}},
        {"command": "sketch.finish", "params": {}},
        {"command": "construct.plane.offset", "params": {"base": "XY", "offset": 150, "name": "Bolt line"}},
        {"command": "component.create", "params": {"name": "Column"}},
        {"command": "solid.box", "params": {"length": 40, "width": 40, "height": 300, "corner": [0, 0, 0], "body_name": "Column web"}},
        {"command": "component.create", "params": {"name": "Bolt M20", "parent": "Column"}},
        {"command": "solid.cylinder", "params": {"radius": 10, "height": 80, "base": [20, -20, 80], "axis": "Y", "body_name": "Bolt shank"}},
        {"command": "sketch.create", "params": {"plane": "XZ", "name": "Thread profile"}},
        {"command": "sketch.circle.center", "params": {"center": [20, 80], "diameter": 20}},
        {"command": "sketch.finish", "params": {}},
        {"command": "component.activate", "params": {"component": "Column"}},
        {"command": "occurrence.copy", "params": {"component": "Bolt M20", "translate": [0, 0, 120]}},
        {"command": "component.activate", "params": {"component": "root"}},
        {"command": "occurrence.copy", "params": {"component": "Column", "translate": [300, 0, 0]}},
        {"command": "occurrence.copy", "params": {"component": "Bolt M20", "parent": "root", "translate": [150, 0, -60]}},
        {"command": "occurrence.copy", "params": {"component": "Bolt M20", "parent": "root", "translate": [190, 0, -60]}},
        {"command": "component.create", "params": {"name": "Washer M20"}},
        {"command": "solid.cylinder", "params": {"radius": 18, "height": 3, "base": [20, -23, 80], "axis": "Y", "body_name": "Washer"}},
        {"command": "component.activate", "params": {"component": "root"}}
    ]});
    h.app.session.run_script(&script).unwrap();
    let (bolt, washer) = (occurrence(&h, 0, "Bolt M20:3"), occurrence(&h, 0, "Washer M20:1"));
    h.app.run("joint.as_built", json!({"type": "rigid", "a": bolt, "b": washer, "name": "Washer seat"})).unwrap();
    h.frames(3);
    h
}

fn occurrence(h: &Harness, parent: u64, name: &str) -> u64 {
    h.app.session.doc.occurrences.iter().find(|o| o.parent == parent && o.name == name).map(|o| o.id).unwrap()
}

fn rows() -> Vec<String> {
    crate::browser::drawn_rows().into_iter().map(|r| r.0).collect()
}

fn count(name: &str) -> usize {
    rows().iter().filter(|r| *r == name).count()
}

fn placements() -> Vec<Vec<u64>> {
    crate::browser::drawn_occurrences().into_iter().map(|o| o.0).collect()
}

fn search(h: &mut Harness, text: &str) {
    h.app.tree.search.text = text.to_string();
    h.frames(2);
}

fn click(h: &mut Harness, handle: &str) {
    let p = handle_at(handle).unwrap_or_else(|| panic!("no {handle}"));
    let r = h.call("ui.click", json!({"x": p.x, "y": p.y}));
    assert_eq!(r["ok"], true, "{r}");
    h.frames(2);
}

fn total(h: &Harness) -> usize {
    h.app.tree.search.hits().map_or(0, |hits| hits.total())
}

#[test]
fn typing_keeps_the_matches_with_their_parents() {
    let mut h = assembly();
    assert!(rows().iter().any(|r| r == "Document Settings"));
    click(&mut h, "browser:search");
    h.call("ui.text", json!({"text": "m20"}));
    h.frames(2);
    assert_eq!(h.app.tree.search.text, "m20");
    assert_eq!(total(&h), 7);
    let (c1, c2) = (occurrence(&h, 0, "Column:1"), occurrence(&h, 0, "Column:2"));
    let column = h.app.session.doc.find_component("Column").unwrap();
    let (b1, b2) = (occurrence(&h, column, "Bolt M20:1"), occurrence(&h, column, "Bolt M20:2"));
    let (b3, b4, w) = (occurrence(&h, 0, "Bolt M20:3"), occurrence(&h, 0, "Bolt M20:4"), occurrence(&h, 0, "Washer M20:1"));
    // The seven placements, and the two columns that hold four of them.
    assert_eq!(placements(), [vec![c1], vec![c1, b1], vec![c1, b2], vec![c2], vec![c2, b1], vec![c2, b2], vec![b3], vec![b4], vec![w]]);
    for gone in ["Document Settings", "Origin", "Named Views", "Base plate", "Bolt holes", "Bolt line", "Bolt shank", "Joints", "Bodies"] {
        assert_eq!(count(gone), 0, "{gone} doesn't match `m20`");
    }
    assert!(handle_at("browser:search:select-all").is_some());
}

/// The count is the rows that match: the browser and `browser.find` agree.
#[test]
fn the_count_is_the_matching_rows() {
    let mut h = assembly();
    search(&mut h, "shank");
    assert_eq!((total(&h), count("Bolt shank")), (6, 6));
    search(&mut h, "bolt");
    let bolts = placements().len() - 2; // less the two columns
    assert_eq!(total(&h), count("Bolt shank") + bolts + count("Bolt holes") + count("Bolt line"));
    assert_eq!(total(&h), 6 + 6 + 2);
    search(&mut h, "*:1");
    assert_eq!(total(&h), 4);
    search(&mut h, "thread");
    assert_eq!((total(&h), count("Thread profile")), (6, 6));
    search(&mut h, "seat");
    assert_eq!((total(&h), count("Washer seat"), count("Joints")), (1, 1, 1));
    for text in ["bolt", "m20", "*", "?olumn*", "washer", "e"] {
        search(&mut h, text);
        assert_eq!(total(&h), find(&h.app.session, Pattern::new(text).as_ref(), &[]).total, "{text}");
    }
}

#[test]
fn no_match_says_so_and_lists_nothing() {
    let mut h = assembly();
    search(&mut h, "zzz");
    assert_eq!(total(&h), 0);
    assert!(h.app.tree.search.on());
    assert!(placements().is_empty());
    assert_eq!(count(&h.app.session.doc.name.clone()), 0);
    assert!(handle_at("browser:search:select-all").is_none());
}

#[test]
fn chips_narrow_a_search_and_alone_list_a_kind() {
    let mut h = assembly();
    // Every kind the design has, and All.
    for kind in ["all", "component", "body", "sketch", "construction", "joint"] {
        assert!(handle_at(&format!("browser:search:kind:{kind}")).is_some(), "{kind}");
    }
    click(&mut h, "browser:search:kind:body");
    assert_eq!(h.app.tree.search.kind, Some(Kind::Body));
    assert_eq!(total(&h), 10);
    assert_eq!((count("Base plate"), count("Column web"), count("Bolt shank"), count("Washer")), (1, 2, 6, 1));
    assert_eq!(count("Thread profile") + count("Bolt holes"), 0);
    search(&mut h, "bolt");
    assert_eq!((total(&h), count("Bolt shank"), count("Bolt holes")), (6, 6, 0));
    click(&mut h, "browser:search:kind:sketch");
    assert_eq!((total(&h), count("Bolt holes"), count("Bolt shank")), (1, 1, 0));
    // All, with nothing typed, is no search: the tree is back.
    search(&mut h, "");
    click(&mut h, "browser:search:kind:all");
    assert!(!h.app.tree.search.on());
    assert!(rows().iter().any(|r| r == "Document Settings"));
}

#[test]
fn select_all_selects_what_was_found() {
    let mut h = assembly();
    search(&mut h, "m20:1");
    click(&mut h, "browser:search:select-all");
    assert_eq!(h.app.session.selection.len(), 3);
    assert!(h.app.session.selection.iter().all(|s| matches!(s, Sel::Occurrence { .. })));
    // A component's sketch is listed in each of its six placements, and is one item.
    search(&mut h, "thread");
    click(&mut h, "browser:search:select-all");
    let thread = h.app.session.doc.find_feature("Thread profile").unwrap().id;
    assert_eq!(h.app.session.selection, [Sel::Feature { id: thread }]);
    // Joints are picked in the tree.
    search(&mut h, "seat");
    click(&mut h, "browser:search:select-all");
    assert!(h.app.session.selection.is_empty());
    assert_eq!(h.app.tree.picked_joints, [h.app.session.doc.assembly.joints[0].id]);
}

#[test]
fn hide_others_leaves_only_what_was_found_shown() {
    let mut h = assembly();
    let world: Vec<String> = h.app.session.world_state().bodies.iter().map(|b| b.name.clone()).collect();
    assert_eq!(world.len(), 10);
    search(&mut h, "washer m20");
    click(&mut h, "browser:search:hide-others");
    let washer = h.app.session.occurrence_bodies(&[occurrence(&h, 0, "Washer M20:1")], true).unwrap();
    assert_eq!(washer.len(), 1);
    let mut hidden = h.app.session.visibility.hidden_bodies.clone();
    hidden.sort();
    let mut others: Vec<String> = world.iter().filter(|b| !washer.contains(b)).cloned().collect();
    others.sort();
    assert_eq!(hidden, others);
    // A search that found no body or placement has nothing to keep: the button is off.
    h.app.session.visibility.hidden_bodies.clear();
    search(&mut h, "holes");
    click(&mut h, "browser:search:hide-others");
    assert!(h.app.session.visibility.hidden_bodies.is_empty());
}

#[test]
fn a_search_opens_closed_rows_and_ending_it_restores_the_tree() {
    let mut h = assembly();
    let c1 = occurrence(&h, 0, "Column:1");
    h.app.tree.collapsed.insert(format!("o:{c1}"));
    h.frames(2);
    assert!(!placements().iter().any(|p| p.len() == 2 && p[0] == c1), "Column:1 starts closed");
    let folds = h.app.tree.collapsed.clone();
    search(&mut h, "m20");
    assert_eq!(placements().iter().filter(|p| p.len() == 2 && p[0] == c1).count(), 2, "the search opens it");
    // Closing it in the search hides its bolts, and leaves the tree's own folds alone.
    click(&mut h, &format!("fold:occurrence:{c1}"));
    assert!(!placements().iter().any(|p| p.len() == 2 && p[0] == c1));
    assert_eq!(h.app.tree.collapsed, folds);
    // Another search opens it again.
    search(&mut h, "m20:");
    assert_eq!(placements().iter().filter(|p| p.len() == 2 && p[0] == c1).count(), 2);
    // Esc in the field ends it: the tree as it was, Column:1 still closed.
    click(&mut h, "browser:search");
    h.call("ui.key", json!({"key": "Escape"}));
    h.frames(2);
    assert!(!h.app.tree.search.on());
    assert!(h.app.tree.search.text.is_empty());
    assert_eq!(h.app.tree.collapsed, folds);
    assert!(rows().iter().any(|r| r == "Document Settings"));
    assert!(!placements().iter().any(|p| p.len() == 2 && p[0] == c1));
}

#[test]
fn the_clear_button_ends_a_search() {
    let mut h = assembly();
    search(&mut h, "bolt");
    h.app.tree.search.kind = Some(Kind::Body);
    h.frames(2);
    click(&mut h, "browser:search:clear");
    assert!(!h.app.tree.search.on());
    assert_eq!((h.app.tree.search.text.as_str(), h.app.tree.search.kind), ("", None));
    assert!(handle_at("browser:search:clear").is_none());
}

#[test]
fn the_search_is_in_both_browser_layouts() {
    let mut h = assembly();
    h.app.preferences.timeline_left = true;
    h.frames(2);
    assert!(handle_at("left-browser-toggle").is_some(), "the browser is below the timeline");
    search(&mut h, "washer");
    assert_eq!((total(&h), count("Washer seat"), count("Washer")), (3, 1, 1));
    assert!(handle_at("browser:search:select-all").is_some());
}

#[test]
fn ctrl_f_over_the_browser_takes_the_keyboard() {
    let mut h = assembly();
    let row = handle_at("row:Document Settings").unwrap();
    h.call("ui.move", json!({"x": row.x, "y": row.y}));
    h.frames(1);
    h.call("ui.key", json!({"key": "F", "cmd": true}));
    h.frames(2);
    h.call("ui.text", json!({"text": "web"}));
    h.frames(2);
    assert_eq!(h.app.tree.search.text, "web");
    assert_eq!(count("Column web"), 2);
}

#[test]
fn find_in_browser_ends_the_search() {
    let mut h = assembly();
    search(&mut h, "m20");
    h.app.tree.reveal = Some("Base plate".into());
    h.frames(3);
    assert!(!h.app.tree.search.on());
    assert_eq!(count("Base plate"), 1);
}

#[test]
fn the_design_changing_updates_the_search() {
    let mut h = assembly();
    search(&mut h, "washer");
    h.app.tree.search.kind = Some(Kind::Body);
    h.frames(2);
    assert_eq!((total(&h), count("Washer")), (1, 1));
    h.app.run("body.rename", json!({"body": "Washer", "name": "Spacer"})).unwrap();
    h.frames(2);
    assert_eq!((total(&h), count("Spacer")), (0, 0));
    h.app.run("edit.undo", json!({})).unwrap();
    h.frames(2);
    assert_eq!((total(&h), count("Washer")), (1, 1));
}

/// Whatever is typed, the browser draws and its count is the engine's.
#[test]
fn hostile_text_in_the_field_never_panics() {
    let mut h = assembly();
    let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
    let alphabet: Vec<char> = "ab*?:M2 0\u{0}\u{FFFF}é\u{10FFFF}".chars().collect();
    for _ in 0..150 {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let len = (seed % 12) as usize;
        let text: String = (0..len).map(|i| alphabet[((seed >> (i * 5)) as usize) % alphabet.len()]).collect();
        search(&mut h, &text);
        let want = Pattern::new(&text).map(|p| find(&h.app.session, Some(&p), &[]).total);
        assert_eq!(h.app.tree.search.hits().map(|hits| hits.total()), want, "{text:?}");
    }
    // Longer than the field takes: the field keeps the first 256 characters.
    click(&mut h, "browser:search");
    h.app.tree.search.text.clear();
    h.call("ui.text", json!({"text": "b".repeat(300)}));
    h.frames(2);
    assert!(h.app.tree.search.text.chars().count() <= solidcraft_engine::browser_find::MAX_PATTERN_CHARS);
    let _: Value = h.call("ui.key", json!({"key": "Escape"}));
}

/// A search shows only some of a folder: dragging there would reorder the folder from those
/// alone, so rows don't drag until it ends.
#[test]
fn rows_do_not_drag_during_a_search() {
    let mut h = Harness::new();
    h.app.home.open = false;
    h.app.session.run_script(&solidcraft_engine::sample::script()).unwrap();
    h.frames(3);
    search(&mut h, "bo");
    assert_eq!((count("Boss"), count("Bore"), count("Base")), (1, 1, 0));
    let order = h.app.session.doc.browser_order.clone();
    let (bore, boss) = (handle_at("row:Bore").unwrap(), handle_at("row:Boss").unwrap());
    h.call("ui.drag", json!({"x0": bore.x + 40.0, "y0": bore.y, "x1": boss.x + 40.0, "y1": boss.y - 8.0, "steps": 12}));
    h.frames(4);
    assert_eq!(h.app.session.doc.browser_order, order);
}

/// During a search only the rows with something listed beneath them fold: a matching
/// placement with nothing under it is a leaf, not an open row with nothing in it.
#[test]
fn only_rows_with_something_beneath_fold_during_a_search() {
    let mut h = assembly();
    let folds = |name: &str| crate::browser::drawn_rows().into_iter().find(|r| r.0 == name).map(|r| r.3.is_some());
    search(&mut h, "m20");
    assert_eq!((folds("Column:1"), folds("Bolt M20:3"), folds("Washer M20:1")), (Some(true), Some(false), Some(false)));
    // The washer's body matches too: its placement folds to show it.
    search(&mut h, "washer");
    assert_eq!((folds("Washer M20:1"), folds("Washer")), (Some(true), Some(false)));
    // Without a search, every placement folds as before (the columns closed, so it is in view).
    for column in ["Column:1", "Column:2"] {
        h.app.tree.collapsed.insert(format!("o:{}", occurrence(&h, 0, column)));
    }
    search(&mut h, "");
    assert_eq!(folds("Bolt M20:3"), Some(true));
}
