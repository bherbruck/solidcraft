//! `sketch.loose_ends` and `sketch.loose_ends.tie`: ends that touch nothing or only touch by
//! position, and tying them so the profile stays closed when the sketch changes.

use serde_json::{Value, json};

use crate::Session;

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    match s.execute(id, &p) {
        Ok(v) => v,
        Err(e) => panic!("{id} {p}: {e}"),
    }
}

/// A square of four separate lines: closed by position only.
fn loose_square(s: &mut Session) -> Vec<String> {
    run(s, "sketch.create", json!({"plane": "XY"}));
    let c = [[0, 0], [10, 0], [10, 10], [0, 10]];
    (0..4)
        .map(|i| {
            let r = run(s, "sketch.line", json!({"points": [c[i], c[(i + 1) % 4]], "infer": false, "connect": false}));
            r["curves"][0].as_str().unwrap().to_string()
        })
        .collect()
}

#[test]
fn a_square_closed_by_position_has_four_untied_corners() {
    let mut s = Session::default();
    loose_square(&mut s);
    assert_eq!(run(&mut s, "sketch.solve", json!({}))["profiles"], 1);
    let r = run(&mut s, "sketch.loose_ends", json!({}));
    assert_eq!(r["open"].as_array().unwrap().len(), 0, "{r}");
    let untied = r["untied"].as_array().unwrap();
    assert_eq!(untied.len(), 4, "{r}");
    assert!(untied.iter().all(|u| u["touches"]["point"].is_string() && u["at"].is_array()), "{r}");
}

#[test]
fn tying_keeps_the_profile_closed_when_a_side_moves() {
    let mut s = Session::default();
    let lines = loose_square(&mut s);
    let r = run(&mut s, "sketch.loose_ends.tie", json!({}));
    assert_eq!(r["constraints"].as_array().unwrap().len(), 4, "{r}");
    assert_eq!(r["untied"].as_array().unwrap().len(), 0, "{r}");
    assert!(run(&mut s, "sketch.loose_ends", json!({}))["untied"].as_array().unwrap().is_empty());
    // Lengthen the bottom side: the corners follow it and the square stays one profile.
    run(&mut s, "sketch.dimension", json!({"entities": [lines[0]], "type": "length", "value": 14}));
    assert_eq!(run(&mut s, "sketch.solve", json!({}))["profiles"], 1);
    // Tying is one undo step.
    run(&mut s, "edit.undo", json!({}));
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(run(&mut s, "sketch.loose_ends", json!({}))["untied"].as_array().unwrap().len(), 4);
}

#[test]
fn tie_takes_only_the_points_asked_for() {
    let mut s = Session::default();
    loose_square(&mut s);
    let r = run(&mut s, "sketch.loose_ends", json!({}));
    let first = r["untied"][0]["point"].clone();
    let t = run(&mut s, "sketch.loose_ends.tie", json!({"points": [first]}));
    assert_eq!(t["constraints"].as_array().unwrap().len(), 1, "{t}");
    assert_eq!(t["untied"].as_array().unwrap().len(), 3, "{t}");
}

#[test]
fn a_gap_shows_two_open_ends() {
    let mut s = Session::default();
    run(&mut s, "sketch.create", json!({"plane": "XY"}));
    run(&mut s, "sketch.line", json!({"points": [[0, 0], [10, 0], [10, 10], [0, 9.9]], "infer": false}));
    let r = run(&mut s, "sketch.loose_ends", json!({}));
    assert_eq!(r["open"].as_array().unwrap().len(), 2, "{r}");
    assert!(r["untied"].as_array().unwrap().is_empty(), "{r}");
}

#[test]
fn hostile_tie_params_are_errors() {
    let mut s = Session::default();
    loose_square(&mut s);
    for p in [json!({"points": 3}), json!({"points": ["nope"]}), json!({"points": [null, 1e308]}), json!({"sketch": "missing"})] {
        assert!(s.execute("sketch.loose_ends.tie", &p).is_err(), "{p}");
    }
    assert!(s.execute("sketch.loose_ends", &json!({"sketch": "missing"})).is_err());
    // Nothing changed.
    assert_eq!(run(&mut s, "sketch.loose_ends", json!({}))["untied"].as_array().unwrap().len(), 4);
}

#[test]
fn outside_a_sketch_they_are_refused() {
    let mut s = Session::default();
    assert!(s.execute("sketch.loose_ends", &json!({})).is_err());
    assert!(s.execute("sketch.loose_ends.tie", &json!({})).is_err());
}
