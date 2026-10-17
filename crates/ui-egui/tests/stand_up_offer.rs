//! The "Stand this part up?" offer can be answered by handle (#1905): its buttons are published
//! as `stand_up:stand`, `stand_up:leave` and `stand_up:never`, it stays up through a camera move
//! and a command until answered, and `ui.prefs {"stand_up": false}` leaves the file as it is.
use serde_json::json;
use solidcraft_ui_egui::scenario::Harness;

/// A harness with a 10 × 40 × 5 mm box (Y up) just opened from STEP: the offer is up.
fn opened() -> Harness {
    let mut src = Harness::new();
    let path = src.dir.join("tall_in_y.step").to_string_lossy().into_owned();
    src.app.run("solid.box", json!({"length": 10, "width": 40, "height": 5})).unwrap();
    src.app.run("file.export", json!({"path": path})).unwrap();
    let mut h = Harness::new();
    h.app.open_path(&path);
    h.frames(3);
    assert_eq!(h.app.stand_up.as_ref().map(Vec::len), Some(1), "the file's body is offered");
    h
}

fn height(h: &Harness) -> f64 {
    let b = solidcraft_engine::view::bounds(&h.app.session);
    b.max.z - b.min.z
}

#[test]
fn the_offer_buttons_are_handles() {
    let mut h = opened();
    for name in ["stand_up:stand", "stand_up:leave", "stand_up:never"] {
        assert!(h.at(&json!({"handle": name})).is_ok(), "{name} is published");
    }
    // Unanswered, it stays up through a camera move and a command.
    h.run(&json!([{"camera": {"yaw": 0.6, "pitch": 0.4}}, {"call": "engine.execute", "params": {"command": "solid.box", "params": {"length": 1, "width": 1, "height": 1}}}]))
        .unwrap();
    h.frames(3);
    assert!(h.app.stand_up.is_some(), "the offer waits for an answer");
    h.run(&json!([{"click": {"handle": "stand_up:stand"}}])).unwrap();
    h.frames(3);
    assert!(h.app.stand_up.is_none());
    assert!((height(&h) - 40.0).abs() < 1e-6, "stood up: {}", height(&h));
}

#[test]
fn leave_it_keeps_the_file_and_the_preference() {
    let mut h = opened();
    h.run(&json!([{"click": {"handle": "stand_up:leave"}}])).unwrap();
    h.frames(3);
    assert!(h.app.stand_up.is_none());
    assert!(h.app.preferences.offer_stand_up);
    assert!((height(&h) - 5.0).abs() < 1e-6);
    assert!(h.at(&json!({"handle": "stand_up:leave"})).is_err(), "the offer is gone");
}

#[test]
fn dont_ask_again_turns_the_offer_off() {
    let mut h = opened();
    h.run(&json!([{"click": {"handle": "stand_up:never"}}])).unwrap();
    h.frames(3);
    assert!(h.app.stand_up.is_none());
    assert!(!h.app.preferences.offer_stand_up);
    assert!((height(&h) - 5.0).abs() < 1e-6);
}

#[test]
fn prefs_stand_up_false_leaves_the_offer() {
    let mut h = opened();
    let r = h.call("ui.prefs", json!({"stand_up": false}));
    assert_eq!(r["result"]["stand_up_offer"], json!(null), "{r}");
    assert_eq!(r["result"]["offer_stand_up"], json!(true), "{r}");
    assert!((height(&h) - 5.0).abs() < 1e-6);
}
