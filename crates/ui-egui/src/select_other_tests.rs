//! Select Other: press and hold, the viewport menu's entry, click cycling and dialog inputs.

use serde_json::{Value, json};
use solidcraft_engine::Sel;

use crate::scenario::Harness;

/// A sketch with two lines drawn on top of each other (l1, then l2), off the origin axes.
fn two_lines() -> (Harness, Value) {
    let mut h = Harness::new();
    h.app.set_origin_shown(false);
    h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
    for _ in 0..2 {
        h.app.run("sketch.line", json!({"points":[[0,10],[20,10]],"infer":false,"connect":false})).unwrap();
    }
    h.app.cam.distance = 60.0;
    h.settle();
    let at = h.call("ui.at", json!({"sketch":[10,10]}))["result"].clone();
    (h, at)
}

fn labels(h: &Harness) -> Vec<String> {
    crate::select_other::describe(&h.app)["rows"].as_array().into_iter().flatten().filter_map(|r| r["label"].as_str().map(String::from)).collect()
}

fn curves(h: &Harness) -> Vec<String> {
    h.app.session.selection.iter().filter_map(|s| if let Sel::SketchCurve { id } = s { Some(id.clone()) } else { None }).collect()
}

#[test]
fn holding_still_lists_both_coincident_lines_and_picks_the_one_underneath() {
    let (mut h, at) = two_lines();
    let before = h.app.session.doc.to_json();
    h.step(&json!({"hold": at})).unwrap();
    assert!(crate::select_other::is_open(&h.app), "a held press opens Select Other");
    let rows = labels(&h);
    assert!(rows.len() >= 2, "{rows:?}");
    assert_eq!(&rows[..2], ["Line 1", "Line 2"], "the sketch's own curves come first");
    // Letting go of the hold neither selected nor dragged anything.
    assert!(h.app.session.selection.is_empty(), "{:?}", h.app.session.selection);
    assert_eq!(h.app.session.doc.to_json(), before);
    // Hovering a row lights it up in the viewport.
    h.step(&json!({"move": {"handle": "select_other:1"}})).unwrap();
    assert!(matches!(&h.app.viewport.hover, Some(crate::viewport::Hit::SketchCurve { id, .. }) if id == "l2"), "{:?}", h.app.viewport.hover);
    h.step(&json!({"click": {"handle": "select_other:1"}})).unwrap();
    assert!(!crate::select_other::is_open(&h.app));
    assert_eq!(curves(&h), ["l2"]);
    // Shift-picking from the list adds, like a Shift-click.
    h.step(&json!({"hold": at})).unwrap();
    h.step(&json!({"click": {"handle": "select_other:0"}, "shift": true})).unwrap();
    let mut got = curves(&h);
    got.sort();
    assert_eq!(got, ["l1", "l2"]);
}

#[test]
fn esc_and_a_click_elsewhere_close_the_list_without_selecting() {
    let (mut h, at) = two_lines();
    h.step(&json!({"hold": at})).unwrap();
    assert!(crate::select_other::is_open(&h.app));
    h.step(&json!({"key": "Escape"})).unwrap();
    assert!(!crate::select_other::is_open(&h.app));
    assert!(h.app.session.active_sketch.is_some(), "Esc only closed the list");
    h.step(&json!({"hold": at})).unwrap();
    h.step(&json!({"click": [at[0].as_f64().unwrap() + 200.0, at[1].as_f64().unwrap() + 150.0]})).unwrap();
    assert!(!crate::select_other::is_open(&h.app));
    assert!(h.app.session.selection.is_empty(), "{:?}", h.app.session.selection);
}

#[test]
fn clicking_again_on_the_same_spot_selects_the_next_thing_under_it() {
    let (mut h, at) = two_lines();
    h.step(&json!({"click": at})).unwrap();
    assert_eq!(curves(&h), ["l1"]);
    // A deliberate second click (not part of a double-click).
    h.frames(30);
    h.step(&json!({"click": at})).unwrap();
    assert_eq!(curves(&h), ["l2"]);
    h.frames(30);
    h.step(&json!({"click": at})).unwrap();
    assert_eq!(curves(&h), ["l1"], "it wraps around");
    // A click somewhere else starts over.
    let x = at[0].as_f64().unwrap();
    let y = at[1].as_f64().unwrap();
    h.frames(30);
    h.step(&json!({"click": [x + 40.0, y]})).unwrap();
    h.frames(30);
    h.step(&json!({"click": at})).unwrap();
    assert_eq!(curves(&h), ["l1"]);
}

#[test]
fn a_double_click_does_not_cycle() {
    let (mut h, at) = two_lines();
    h.step(&json!({"click": at, "double": true})).unwrap();
    // The double-click's chain starts from the line under the cursor.
    assert!(curves(&h).contains(&"l1".to_string()), "{:?}", h.app.session.selection);
}

#[test]
fn the_viewport_menu_offers_select_other() {
    let (mut h, at) = two_lines();
    h.step(&json!({"click": at, "button": "right"})).unwrap();
    let menu = h.call("ui.menu", json!({}))["result"].clone();
    let items: Vec<&str> = menu["items"].as_array().into_iter().flatten().filter_map(|i| i["id"].as_str()).collect();
    assert!(items.contains(&"ui.selectOther"), "{items:?}");
    let r = h.call("ui.menuPick", json!({"item": "ui.selectOther"}));
    assert_eq!(r["ok"], true, "{r}");
    assert!(labels(&h).starts_with(&["Line 1".to_string(), "Line 2".to_string()]), "{:?}", labels(&h));
    // The control channel picks rows too.
    let r = h.call("ui.selectOther", json!({"pick": 1}));
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!(curves(&h), ["l2"]);
    // Over empty space there is nothing to offer.
    h.step(&json!({"click": [40, 400], "button": "right"})).unwrap();
    let menu = h.call("ui.menu", json!({}))["result"].clone();
    assert!(!menu["items"].as_array().into_iter().flatten().any(|i| i["id"] == "ui.selectOther"), "{menu}");
}

#[test]
fn overlapping_bodies_list_both_and_the_hidden_one_can_be_picked() {
    let mut h = Harness::new();
    h.app.set_origin_shown(false);
    h.app.run("solid.box", json!({"length":10,"width":10,"height":10,"body_name":"Inner"})).unwrap();
    h.app.run("solid.box", json!({"length":10,"width":10,"height":10,"corner":[5,5,5],"body_name":"Outer"})).unwrap();
    h.call("ui.view", json!({"view":"top"}));
    h.call("ui.view", json!({"view":"fit"}));
    h.step(&json!({"hold": {"world": [7, 7, 15]}})).unwrap();
    let rows = labels(&h);
    assert!(rows.iter().any(|r| r == "Body · Outer") && rows.iter().any(|r| r == "Body · Inner"), "{rows:?}");
    let faces = rows.iter().filter(|r| r.starts_with("Face ")).count();
    assert_eq!(faces, 2, "the nearest face of each body: {rows:?}");
    let i = rows.iter().position(|r| r == "Body · Inner").unwrap();
    h.call("ui.selectOther", json!({"pick": i}));
    assert_eq!(h.app.session.selection, [Sel::Body { name: "Inner".into() }]);
}

#[test]
fn a_dialog_selection_input_takes_its_pick_from_the_list() {
    let mut h = Harness::new();
    h.app.set_origin_shown(false);
    h.app.run("solid.box", json!({"length":10,"width":10,"height":10,"body_name":"Block"})).unwrap();
    h.call("ui.view", json!({"view":"home"}));
    h.call("ui.view", json!({"view":"fit"}));
    h.step(&json!({"start": "solid.fillet"})).unwrap();
    h.frames(2);
    h.step(&json!({"hold": {"world": [5, 0, 10]}})).unwrap();
    let rows = crate::select_other::describe(&h.app)["rows"].clone();
    let sels: Vec<&Value> = rows.as_array().into_iter().flatten().map(|r| &r["sel"]).collect();
    assert!(!sels.is_empty(), "{rows}");
    // The edge in front, then the face behind it; no bodies, which the fillet doesn't take.
    assert_eq!(sels[0]["type"], "edge", "{rows}");
    assert!(sels.iter().all(|s| s["type"] == "edge" || s["type"] == "face"), "only what the fillet's input accepts: {rows}");
    h.call("ui.selectOther", json!({"pick": 0}));
    let d = h.app.dialog.as_ref().expect("the fillet dialog stays open");
    assert!(d.inputs.iter().any(|i| i.items.iter().any(|x| matches!(x, Sel::Edge { .. }))), "{:?}", d.inputs);
    assert!(h.app.session.selection.is_empty(), "the pick went to the dialog, not the selection");
}

#[test]
fn rows_name_points_by_their_curves() {
    let mut h = Harness::new();
    h.app.set_origin_shown(false);
    h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
    h.app.run("sketch.circle.center", json!({"center":[10,10],"radius":3})).unwrap();
    h.app.cam.distance = 60.0;
    h.settle();
    let at = h.call("ui.at", json!({"sketch":[10,10]}))["result"].clone();
    h.call("ui.selectOther", json!({"x": at[0], "y": at[1]}));
    let rows = labels(&h);
    assert!(rows.iter().any(|r| r.starts_with("Point (") && r.contains("centre")), "{rows:?}");
}

#[test]
fn a_dense_spot_is_capped_per_kind_and_reports_the_total() {
    let mut h = Harness::new();
    h.app.set_origin_shown(false);
    h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
    // Thirty lines drawn on top of each other: one kind of thing, far beyond the per-kind cap.
    for _ in 0..30 {
        h.app.run("sketch.line", json!({"points":[[0,10],[20,10]],"infer":false,"connect":false})).unwrap();
    }
    h.app.cam.distance = 60.0;
    h.settle();
    let at = h.call("ui.at", json!({"sketch":[10,10]}))["result"].clone();
    h.step(&json!({"hold": at})).unwrap();
    assert!(crate::select_other::is_open(&h.app));
    let d = crate::select_other::describe(&h.app);
    let rows = d["rows"].as_array().expect("rows");
    let lines = rows.iter().filter(|r| r["label"].as_str().is_some_and(|l| l.starts_with("Line "))).count();
    assert_eq!(lines, 20, "at most 20 rows of one kind, nearest first: {rows:?}");
    assert_eq!(rows[0]["label"], json!("Line 1"), "the nearest still leads");
    let total = d["total"].as_u64().expect("total");
    assert!(total >= 30, "the total counts every candidate, not just the rows: {total}");
    assert!((total as usize) < rows.len() + 30, "the list is truncated: {total} total, {} rows", rows.len());
}
