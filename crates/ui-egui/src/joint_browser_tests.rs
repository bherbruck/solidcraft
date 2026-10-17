//! Joints in the browser (#1160): an eye per joint and on the Joints folder hides and shows
//! joint glyphs; Show/Hide in a joint's menu and the V key do the same. Hiding only hides the
//! drawing: the joint still constrains and drives. A hidden joint picked in the browser draws
//! highlighted until the pick goes away.

use egui::pos2;
use serde_json::json;

use crate::context_menu::{self, Target};
use crate::scenario::Harness;

/// Components A (grounded) and B joined by a revolute "Hinge", and C held by a rigid "Glue".
fn harness() -> Harness {
    let mut h = Harness::new();
    h.app.home.open = false;
    h.app.ui.show_browser = true;
    for (name, x) in [("A", 0), ("B", 30), ("C", 60)] {
        h.app.run("component.activate", json!({"component": "root"})).unwrap();
        h.app.run("component.create", json!({ "name": name })).unwrap();
        h.app.run("solid.box", json!({"length": 20, "width": 20, "height": 10, "corner": [x, 0, 0]})).unwrap();
    }
    h.app.run("component.activate", json!({"component": "root"})).unwrap();
    h.app.run("occurrence.ground", json!({"occurrence": "A:1", "grounded": true})).unwrap();
    h.app
        .run("joint.create", json!({"type": "revolute", "name": "Hinge", "a": {"occurrence": "A:1", "face": [10, 10, 10]}, "b": {"occurrence": "B:1", "face": [40, 10, 0]}}))
        .unwrap();
    h.app
        .run("joint.create", json!({"type": "rigid", "name": "Glue", "a": {"occurrence": "A:1", "face": [20, 10, 5]}, "b": {"occurrence": "C:1", "face": [60, 10, 5]}}))
        .unwrap();
    h.frames(3);
    h
}

fn id(h: &Harness, name: &str) -> u64 {
    h.app.session.doc.assembly.joints.iter().find(|j| j.name == name).unwrap().id
}

fn click(h: &mut Harness, handle: &str) {
    assert!(crate::scenario::handle_at(handle).is_some(), "{handle} is not drawn: {:?}", crate::scenario::handles(""));
    h.step(&json!({"click": {"handle": handle}})).unwrap();
    h.frames(3);
}

fn drawn(h: &Harness) -> Vec<(u64, bool)> {
    crate::dialogs_assembly::drawn_joints(&h.app)
}

#[test]
fn eyes_hide_and_show_joints_and_the_folder_toggles_all() {
    let mut h = harness();
    let (hinge, glue) = (id(&h, "Hinge"), id(&h, "Glue"));
    assert_eq!(drawn(&h), [(hinge, false), (glue, false)], "every joint draws by default");

    click(&mut h, "eye:Hinge");
    assert!(!h.app.session.joint_visible(hinge) && h.app.session.joint_visible(glue));
    assert_eq!(drawn(&h), [(glue, false)], "a hidden joint isn't drawn");
    assert!(crate::scenario::handle_at("row:Hinge").is_some(), "a hidden joint stays listed");

    // The folder's eye: some shown → all hidden; none shown → all shown.
    click(&mut h, "eye:Joints");
    assert!(drawn(&h).is_empty());
    click(&mut h, "eye:Joints");
    assert_eq!(drawn(&h), [(hinge, false), (glue, false)]);
}

#[test]
fn a_hidden_joint_still_drives_and_shows_while_picked() {
    let mut h = harness();
    let hinge = id(&h, "Hinge");
    click(&mut h, "eye:Hinge");
    let b = |h: &Harness| {
        let c = h.app.session.doc.find_component("B").unwrap();
        h.app.session.doc.occurrence_of(c).unwrap().transform
    };
    let before = b(&h);
    h.app.run("joint.drive", json!({"joint": "Hinge", "value": 90})).unwrap();
    h.frames(3);
    assert_ne!(before, b(&h), "the hidden hinge still turns B");
    assert!(!h.app.session.joint_visible(hinge));

    // Picked in the browser, the hidden joint draws highlighted, and only while it is picked.
    click(&mut h, "row:Hinge");
    assert_eq!(h.app.tree.picked_joints, [hinge]);
    assert!(drawn(&h).contains(&(hinge, true)), "{:?}", drawn(&h));
    assert!(!h.app.session.joint_visible(hinge), "picking doesn't show it");
    crate::viewport::select(&mut h.app, None, false, false);
    assert!(h.app.tree.picked_joints.is_empty());
    assert!(!drawn(&h).iter().any(|(j, _)| *j == hinge));
}

#[test]
fn the_menu_and_the_v_key_hide_and_show_picked_joints() {
    let mut h = harness();
    let (hinge, glue) = (id(&h, "Hinge"), id(&h, "Glue"));
    let items = context_menu::items(&h.app, &Target::Joint { id: hinge });
    let item = items.iter().find(|i| i.label == "Show/Hide").unwrap_or_else(|| panic!("no Show/Hide in {items:?}")).clone();
    assert_eq!(item.shortcut, "V");
    context_menu::run_item(&mut h.app, &item, pos2(0.0, 0.0));
    assert!(!h.app.session.joint_visible(hinge) && h.app.session.joint_visible(glue));
    let items = context_menu::items(&h.app, &Target::Joint { id: hinge });
    let item = items.iter().find(|i| i.label == "Show/Hide").unwrap().clone();
    context_menu::run_item(&mut h.app, &item, pos2(0.0, 0.0));
    assert!(h.app.session.joint_visible(hinge));

    // V on the joints picked in the browser.
    click(&mut h, "row:Hinge");
    h.app.tree.picked_joints.push(glue);
    context_menu::toggle_visibility(&mut h.app);
    assert!(!h.app.session.joint_visible(hinge) && !h.app.session.joint_visible(glue));
    context_menu::toggle_visibility(&mut h.app);
    assert!(h.app.session.joint_visible(hinge) && h.app.session.joint_visible(glue));
}
