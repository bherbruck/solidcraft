//! Completed distances must reach the viewport before a continuously moving pointer stops.
use super::*;
use serde_json::json;

fn colors() -> Colors {
    Colors { body: [1, 1, 1, 255], added: [2, 2, 2, 255], new_body: [2, 2, 2, 255], removed: [3, 3, 3, 100], edge: [0, 0, 0, 255] }
}

fn fixture() -> Session {
    let mut s = Session::new(solidcraft_engine::doc::Document::new("Preview drag"));
    s.execute("sketch.create", &json!({"plane":"XY", "name":"Profile"})).unwrap();
    s.execute("sketch.rectangle.two_point", &json!({"p0":[0,0], "p1":[10,10]})).unwrap();
    s.execute("sketch.finish", &json!({})).unwrap();
    s
}

fn extrude(distance: i32) -> Vec<(String, Value)> {
    vec![("solid.extrude".into(), json!({"sketch":"Profile", "distance":distance, "operation":"new"}))]
}

fn check_drag(s: &Session, commands: impl Fn(i32) -> Vec<(String, Value)>, base_z: f64) {
    let original = serde_json::to_value(&*s.doc).unwrap();
    let undo = s.undo.len();
    let mut pv = PreviewState::default();
    for (finished, wanted) in [(10, 20), (20, 30), (30, 30)] {
        let old = commands(finished);
        let new = commands(wanted);
        let old_key = job_key(&old, s.revision, colors());
        let key = job_key(&new, s.revision, colors());
        let (tx, rx) = std::sync::mpsc::channel();
        pv.running = Some((old_key, rx));
        pv.running_tool = as_new_body(&old).is_some();
        pv.running_stream_key = stream_key(&old, s.revision, colors());
        let built = compute_tool(s, &old, colors()).unwrap();
        tx.send(Ok(built)).unwrap();
        pv.poll(key, stream_key(&new, s.revision, colors()));
        assert!(pv.active && pv.error.is_none(), "completed geometry must show during the drag: {:?}", pv.error);
        assert!((pv.bounds.max.z - base_z - f64::from(finished)).abs() < 1e-8);
        assert_eq!(pv.shown_key, Some(old_key));
        assert_eq!(pv.shown_key == Some(key), finished == wanted, "only the final distance satisfies the latest request");
        assert!(pv.running.is_none());
    }
    assert_eq!(serde_json::to_value(&*s.doc).unwrap(), original);
    assert_eq!(s.undo.len(), undo);
}

#[test]
fn new_body_extrude_keeps_moving_during_a_drag() {
    check_drag(&fixture(), extrude, 0.0);
}

#[test]
fn edited_new_body_extrude_keeps_moving_during_a_drag() {
    let mut s = fixture();
    let feature = s.execute("solid.extrude", &extrude(5)[0].1).unwrap()["feature"].as_u64().unwrap();
    check_drag(
        &s,
        |distance| vec![("timeline.redefine".into(), json!({"feature":feature,"command":"solid.extrude","params":extrude(distance)[0].1}))],
        0.0,
    );
}

#[test]
fn face_offset_keeps_moving_during_a_drag() {
    let mut s = Session::new(solidcraft_engine::doc::Document::new("Face offset drag"));
    s.execute("solid.box", &json!({"length":10,"width":10,"height":10})).unwrap();
    check_drag(&s, |distance| vec![("solid.offset_face".into(), json!({"faces":[[5,5,10]],"distance":distance}))], 10.0);
}

#[test]
fn join_tool_keeps_moving_without_waiting_for_its_boolean() {
    let mut s = fixture();
    s.execute("solid.box", &json!({"length":10,"width":10,"height":5})).unwrap();
    check_drag(&s, |distance| vec![("solid.extrude".into(), json!({"sketch":"Profile","distance":distance,"operation":"join"}))], 0.0);
}

#[test]
fn distance_stream_keeps_feature_selection_and_extent_context() {
    let old = extrude(10);
    let key = stream_key(&old, 7, colors());
    assert!(key.is_some());
    assert_eq!(key, stream_key(&extrude(20), 7, colors()));
    assert_ne!(key, stream_key(&old, 8, colors()));
    let mut recolored = colors();
    recolored.added = [40, 50, 60, 255];
    assert_ne!(key, stream_key(&old, 7, recolored));
    for (field, value) in [
        ("sketch", json!("Other")),
        ("profiles", json!([1])),
        ("face", json!([0, 0, 5])),
        ("operation", json!("cut")),
        ("direction", json!("negative")),
        ("distance2", json!(5)),
        ("to", json!([0, 0, 50])),
        ("taper", json!(5)),
        ("start_offset", json!(3)),
        ("participants", json!(["Other"])),
    ] {
        let mut changed = old.clone();
        changed[0].1[field] = value;
        assert_ne!(key, stream_key(&changed, 7, colors()), "changed {field} cannot reuse the old result");
    }
    for field in ["distance2", "to_offset"] {
        let mut from = old.clone();
        from[0].1[field] = json!(5);
        let mut to = from.clone();
        to[0].1[field] = json!(9);
        assert_eq!(stream_key(&from, 7, colors()), stream_key(&to, 7, colors()));
    }
    let edit =
        |feature, distance| vec![("timeline.redefine".into(), json!({"feature":feature,"command":"solid.extrude","params":extrude(distance)[0].1}))];
    assert_eq!(stream_key(&edit(2, 10), 7, colors()), stream_key(&edit(2, 20), 7, colors()));
    assert_ne!(stream_key(&edit(2, 10), 7, colors()), stream_key(&edit(3, 20), 7, colors()));
    assert_ne!(key, stream_key(&edit(2, 10), 7, colors()));
    let offset = |distance| vec![("solid.offset_face".into(), json!({"faces":[[5,5,10]],"distance":distance}))];
    assert!(stream_key(&offset(1), 7, colors()).is_some());
    assert_eq!(stream_key(&offset(1), 7, colors()), stream_key(&offset(5), 7, colors()));
    assert!(stream_key(&[], 7, colors()).is_none());
    assert!(stream_key(&[("solid.fillet".into(), json!({"radius":5}))], 7, colors()).is_none());
    let mut mixed = old;
    mixed.push(("select.clear".into(), json!({})));
    assert!(stream_key(&mixed, 7, colors()).is_none());
}

#[test]
fn distance_results_reject_other_setups_and_obsolete_errors() {
    let s = fixture();
    let old = extrude(10);
    let current = extrude(20);
    for (revision, failed) in [(s.revision + 1, false), (s.revision, true)] {
        let mut built = compute_tool(&s, &old, colors()).unwrap();
        if failed {
            built.error = Some("old distance failed".into());
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let mut pv = PreviewState {
            running: Some((job_key(&old, s.revision, colors()), rx)),
            running_stream_key: stream_key(&old, s.revision, colors()),
            ..Default::default()
        };
        tx.send(Ok(built)).unwrap();
        pv.poll(job_key(&current, revision, colors()), stream_key(&current, revision, colors()));
        assert!(!pv.active && pv.error.is_none() && pv.shown_key.is_none());
    }
    let mut join = current.clone();
    join[0].1["operation"] = json!("join");
    assert!(as_new_body(&join).is_some());
    assert!(stream_key(&join, s.revision, colors()).is_none(), "tool previews retain their upstream streaming path");
}
