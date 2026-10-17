//! Many cuts through one sheet: a hole feature's drills machined in one pass, labels in one
//! sketch or one feature each, all keeping the geometry and flat pattern of cutting them one by one.

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

/// A `w` x `h` tray of 2.5 mm sheet: a base flange on XY with a 15 mm flange on each edge.
fn tray(s: &mut Session, w: f64, h: f64) {
    run(s, "sketch.create", json!({"plane": "XY", "name": "Base"}));
    run(s, "sketch.rectangle.two_point", json!({"p0": [0, 0], "p1": [w, h]}));
    run(s, "sketch.finish", json!({}));
    run(s, "sheet.flange", json!({"sketch": "Base"}));
    let edges = json!([[w / 2.0, h, 2.5], [w, h / 2.0, 2.5], [w / 2.0, 0, 2.5], [0, h / 2.0, 2.5]]);
    run(s, "sheet.flange", json!({"edges": edges, "height": 15}));
}

fn through_cut(s: &mut Session, sketch: &str) {
    run(s, "solid.extrude", json!({"sketch": sketch, "through_all": true, "direction": "symmetric", "operation": "cut"}));
}

/// A label cut on its own: one sketch, one cut.
fn label_cut(s: &mut Session, name: &str, text: &str, at: [f64; 2]) {
    run(s, "sketch.create", json!({"plane": "XY", "name": name}));
    run(s, "sketch.text", json!({"text": text, "at": at, "height": 6}));
    run(s, "sketch.finish", json!({}));
    through_cut(s, name);
}

/// The labels in one sketch, cut at once.
fn labels_cut(s: &mut Session, labels: &[(String, [f64; 2])]) {
    run(s, "sketch.create", json!({"plane": "XY", "name": "Labels"}));
    for (text, at) in labels {
        run(s, "sketch.text", json!({"text": text, "at": at, "height": 6}));
    }
    run(s, "sketch.finish", json!({}));
    through_cut(s, "Labels");
}

/// One hole feature drilling at every point (one sketch of points).
fn holes_at_once(s: &mut Session, points: &[[f64; 2]]) {
    run(s, "sketch.create", json!({"plane": "XY", "name": "HolePoints"}));
    for p in points {
        run(s, "sketch.point", json!({"point": p}));
    }
    run(s, "sketch.finish", json!({}));
    run(s, "solid.hole", json!({"sketch": "HolePoints", "diameter": 4}));
}

/// A hole feature for each point, drilled into the base's top face.
fn holes_one_by_one(s: &mut Session, points: &[[f64; 2]]) {
    for p in points {
        run(s, "solid.hole", json!({"position": [p[0], p[1], 2.5], "diameter": 4}));
    }
}

fn polyline(v: &Value) -> Vec<[f64; 2]> {
    v.as_array().map(|a| a.iter().map(|p| [p[0].as_f64().unwrap_or(f64::NAN), p[1].as_f64().unwrap_or(f64::NAN)]).collect()).unwrap_or_default()
}

fn area(p: &[[f64; 2]]) -> f64 {
    let n = p.len();
    (0..n).map(|i| p[i][0] * p[(i + 1) % n][1] - p[(i + 1) % n][0] * p[i][1]).sum::<f64>() / 2.0
}

fn sorted_loops(v: &Value) -> Vec<Vec<[f64; 2]>> {
    let mut loops: Vec<Vec<[f64; 2]>> = v.as_array().map(|a| a.iter().map(polyline).collect()).unwrap_or_default();
    loops.sort_by(|a, b| {
        let (a, b) = (a.first().copied().unwrap_or_default(), b.first().copied().unwrap_or_default());
        a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1]))
    });
    loops
}

/// The flat pattern's outline loops and cut-outs (each a polyline, ordered by where they start,
/// so loops made in another order compare alike) and the area they enclose.
fn flat(s: &mut Session) -> (Vec<Vec<[f64; 2]>>, f64) {
    let f = run(s, "sheet.flat_pattern", json!({}));
    let mut loops = sorted_loops(&f["outline"]);
    let cutouts = sorted_loops(&f["cutouts"]);
    let enclosed = loops.iter().map(|l| area(l).abs()).sum::<f64>() - cutouts.iter().map(|c| area(c).abs()).sum::<f64>();
    loops.extend(cutouts);
    (loops, enclosed)
}

fn cutout_count(s: &mut Session) -> usize {
    run(s, "sheet.flat_pattern", json!({}))["cutouts"].as_array().map_or(0, Vec::len)
}

/// The same volume (within a millionth) and the same flat pattern (its material area within a
/// millionth, every point of its outline and cut-outs within 1e-6 mm).
fn assert_same(a: &mut Session, b: &mut Session) {
    let (va, vb) = (volume(a), volume(b));
    assert!((va - vb).abs() <= 1e-6 * va.abs(), "volume {va} vs {vb}");
    let (la, ma) = flat(a);
    let (lb, mb) = flat(b);
    assert!(ma.is_finite() && (ma - mb).abs() <= 1e-6 * ma.abs().max(1.0), "flat area {ma} vs {mb}");
    let close = |p: &[[f64; 2]], q: &[[f64; 2]]| {
        p.len() == q.len() && p.iter().zip(q).all(|(x, y)| (x[0] - y[0]).abs() <= 1e-6 && (x[1] - y[1]).abs() <= 1e-6)
    };
    assert_eq!(la.len(), lb.len(), "flat loop count");
    for (x, y) in la.iter().zip(&lb) {
        assert!(close(x, y), "flat loop differs: {:?} vs {:?}", x.first(), y.first());
    }
}

fn hole_grid(n: usize, columns: usize, at: [f64; 2], pitch: f64) -> Vec<[f64; 2]> {
    (0..n).map(|i| [at[0] + (i % columns) as f64 * pitch, at[1] + (i / columns) as f64 * pitch]).collect()
}

#[test]
fn a_hole_feature_drills_a_sheet_in_one_pass_like_holes_one_by_one() {
    let points = hole_grid(12, 6, [15.0, 20.0], 15.0);
    let (mut once, mut each) = (Session::default(), Session::default());
    tray(&mut once, 120.0, 80.0);
    tray(&mut each, 120.0, 80.0);
    let before = volume(&mut once);
    holes_at_once(&mut once, &points);
    holes_one_by_one(&mut each, &points);
    // Each hole takes π r² t from the base (the measure follows the walls' facets).
    let want = before - 12.0 * std::f64::consts::PI * 4.0 * 2.5;
    assert!((volume(&mut once) - want).abs() < 1e-5 * want, "{} {want}", volume(&mut once));
    assert_eq!(cutout_count(&mut once), 12);
    assert_same(&mut once, &mut each);
}

/// A second cut after a text cut was dropped without a word, and a hole after it missed the flat
/// pattern: the 3D test whether their tools met the sheet came out empty against the sheet
/// rebuilt with the letters' cut-outs.
#[test]
fn a_cut_after_a_text_cut_still_cuts() {
    let mut s = Session::default();
    tray(&mut s, 120.0, 80.0);
    label_cut(&mut s, "Label", "AB", [15.0, 50.0]);
    let before = volume(&mut s);
    run(&mut s, "sketch.create", json!({"plane": "XY", "name": "Slot"}));
    run(&mut s, "sketch.rectangle.two_point", json!({"p0": [70, 20], "p1": [90, 30]}));
    run(&mut s, "sketch.finish", json!({}));
    let r = run(&mut s, "solid.extrude", json!({"sketch": "Slot", "through_all": true, "direction": "symmetric", "operation": "cut"}));
    assert_eq!(r["changed_bodies"], json!(["Body1"]), "{r}");
    let want = before - 20.0 * 10.0 * 2.5;
    assert!((volume(&mut s) - want).abs() < 1e-6 * want, "{} {want}", volume(&mut s));
    assert_eq!(cutout_count(&mut s), 3, "A, B and the slot");
    // A hole drilled after it is a cut-out of the flat pattern too (it was cut into the solid
    // only, so unfolding would have lost it).
    run(&mut s, "solid.hole", json!({"position": [100, 60, 2.5], "diameter": 4}));
    assert_eq!(cutout_count(&mut s), 4, "A, B, the slot and the hole");
}

#[test]
fn labels_in_one_sketch_and_holes_in_one_feature_match_cutting_them_one_by_one() {
    let labels: Vec<(String, [f64; 2])> = (0..3).map(|i| (format!("A{i}"), [15.0 + 30.0 * i as f64, 55.0])).collect();
    let points = hole_grid(6, 6, [15.0, 20.0], 15.0);
    let (mut once, mut each) = (Session::default(), Session::default());
    tray(&mut once, 120.0, 80.0);
    tray(&mut each, 120.0, 80.0);
    labels_cut(&mut once, &labels);
    holes_at_once(&mut once, &points);
    for (i, (text, at)) in labels.iter().enumerate() {
        label_cut(&mut each, &format!("Label{i}"), text, *at);
    }
    holes_one_by_one(&mut each, &points);
    assert_same(&mut once, &mut each);
}

/// The full size: a flanged sheet with 10 labels and 50 holes, cut at once and one by one.
/// `cargo test --release -p solidcraft-engine --lib sheet_many_cuts_benchmark -- --ignored --nocapture`
#[test]
#[ignore = "benchmark: run with --release --ignored --nocapture"]
fn sheet_many_cuts_benchmark() {
    let labels: Vec<(String, [f64; 2])> =
        (0..10).map(|i| (format!("AB{i}"), [20.0 + 55.0 * (i % 5) as f64, if i < 5 { 150.0 } else { 170.0 }])).collect();
    let points = hole_grid(50, 10, [20.0, 30.0], 20.0);
    let time = |f: &mut dyn FnMut()| {
        let t = std::time::Instant::now();
        f();
        t.elapsed().as_secs_f64() * 1000.0
    };
    let (mut once, mut each) = (Session::default(), Session::default());
    tray(&mut once, 300.0, 200.0);
    tray(&mut each, 300.0, 200.0);
    let labels_once = time(&mut || labels_cut(&mut once, &labels));
    let holes_once = time(&mut || holes_at_once(&mut once, &points));
    let labels_each = time(&mut || {
        for (i, (text, at)) in labels.iter().enumerate() {
            label_cut(&mut each, &format!("Label{i}"), text, *at);
        }
    });
    let holes_each = time(&mut || holes_one_by_one(&mut each, &points));
    println!("labels: one sketch {labels_once:.0} ms, one feature each {labels_each:.0} ms");
    println!("holes: one feature {holes_once:.0} ms, one feature each {holes_each:.0} ms");
    assert_same(&mut once, &mut each);
}
