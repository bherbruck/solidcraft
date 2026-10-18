//! Hole placement on the canvas. A Single hole picked on or near a sketch point on the face (a
//! circle's centre) goes to that point, as a Multiple pick of the same point does (#657); a pick
//! away from sketch points, or near one off the face under the cursor, stays where it was clicked.
//! Only points the sketch shows are snapped to (not lettering's outline points), and Select Other
//! looks the face up once for every point near the cursor.

use serde_json::{Value, json};
use solidcraft_engine::Sel;
use solidcraft_engine::doc::FeatureKind;
use solidcraft_engine::geom::{Aabb3, Vec2, Vec3};
use solidcraft_engine::sketch::{CurveKind, LinkKind, SPoint};

use crate::dialogs::Kind;
use crate::scenario::Harness;
use crate::viewport::Hit;

/// A 40 × 30 × 20 block, seen from the top, with a sketch on its top face: a circle of radius 4
/// about (12, 9). Returns the circle centre in the world.
fn block_with_circle() -> (Harness, Vec3) {
    let (h, centre) = block_and_circle_on(json!({"plane": {"face": [20, 15, 20]}}), [12.0, 9.0]);
    assert!((centre.z - 20.0).abs() < 1e-9, "the circle lies on the top face: {centre:?}");
    (h, centre)
}

/// The block, seen from the top, and a circle of radius 4 about `at` (sketch coordinates) in the
/// sketch `sketch` (`sketch.create`'s parameters) makes. Returns the circle centre in the world.
fn block_and_circle_on(sketch: Value, at: [f64; 2]) -> (Harness, Vec3) {
    let mut h = Harness::new();
    h.app.set_origin_shown(false);
    h.app.run("solid.box", json!({"length": 40, "width": 30, "height": 20})).unwrap();
    h.app.run("sketch.create", sketch).unwrap();
    h.app.run("sketch.circle.center", json!({"center": at, "radius": 4})).unwrap();
    h.app.run("sketch.finish", json!({})).unwrap();
    h.step(&json!({"call": "ui.view", "params": {"view": "top"}})).unwrap();
    h.settle();
    let st = h.app.session.world_state();
    let ss = st.sketches.first().unwrap();
    let centre = ss
        .sketch
        .curves
        .iter()
        .find_map(|c| match c.kind {
            CurveKind::Circle { c, .. } => Some(ss.plane.to_world(ss.sketch.points[c].pos)),
            _ => None,
        })
        .unwrap();
    (h, centre)
}

/// Where a world point is on screen.
fn screen(h: &mut Harness, p: Vec3) -> [f64; 2] {
    h.at(&json!({"world": [p.x, p.y, p.z]})).unwrap()
}

/// Five pixels off where `p` is on screen: within reach of a sketch point pick, not on `p`.
fn near(h: &mut Harness, p: Vec3) -> [f64; 2] {
    let c = screen(h, p);
    [c[0] + 4.0, c[1] + 3.0]
}

/// Everything under a screen point, as a click there sees it.
fn hits_at(h: &Harness, at: [f64; 2]) -> Vec<Hit> {
    let proj = crate::viewport::projection(&h.app, h.app.viewport.rect.unwrap());
    crate::viewport::pick(&h.app, &proj, egui::pos2(at[0] as f32, at[1] as f32))
}

/// The face the cursor is over, where the ray meets it, after hovering `at`.
fn hovered_face_point(h: &mut Harness, at: [f64; 2]) -> Vec3 {
    h.step(&json!({ "move": at })).unwrap();
    match &h.app.viewport.hover {
        Some(Hit::Face { point, .. }) => *point,
        other => panic!("the face lights up, not {other:?}"),
    }
}

/// The last feature's hole placement: its id, its position and whether it is at sketch points.
fn hole(h: &Harness) -> (u64, Vec3, bool) {
    let f = h.app.session.doc.features.last().unwrap();
    match &f.kind {
        FeatureKind::Hole { position, points, .. } => (f.id, position.0, points.is_some()),
        other => panic!("the last feature is not a hole: {}", other.type_name()),
    }
}

/// The face places the open dialog's first input holds.
fn face_places(h: &Harness) -> Vec<(String, Vec3)> {
    h.app.dialog.as_ref().unwrap().inputs[0]
        .items
        .iter()
        .map(|x| match x {
            Sel::Face { body, point, .. } => (body.clone(), *point),
            other => panic!("a face place, not {other:?}"),
        })
        .collect()
}

/// Whether the model holds material at `p`.
fn solid_at(h: &Harness, p: Vec3) -> bool {
    h.app.session.model.state().bodies.iter().any(|b| b.body.contains(p))
}

#[test]
fn a_single_hole_picked_near_a_circle_centre_goes_to_the_centre() {
    let (mut h, centre) = block_with_circle();
    let click = near(&mut h, centre);
    h.step(&json!({"start": "solid.hole"})).unwrap();
    let d = h.app.dialog.as_ref().unwrap();
    assert!(matches!(&d.kind, Kind::Hole { opts, .. } if !opts.multiple), "a new hole starts with Single placement");
    h.step(&json!({"move": click})).unwrap();
    assert!(
        matches!(&h.app.viewport.hover, Some(Hit::SketchPoint { .. })),
        "the centre lights up as what the pick snaps to: {:?}",
        h.app.viewport.hover
    );
    h.step(&json!({"click": click})).unwrap();
    let places = face_places(&h);
    assert!(matches!(places.as_slice(), [(_, p)] if p.dist(centre) < 1e-9), "the face is picked at the centre {centre:?}, not {places:?}");
    h.step(&json!({"key": "Enter"})).unwrap();
    assert!(h.app.dialog.is_none(), "OK closed the dialog: {:?}", h.app.dialog.as_ref().map(|d| &d.error));
    let (id, position, at_points) = hole(&h);
    assert!(!at_points, "a Single hole is placed on its face");
    assert!(position.dist(centre) < 1e-9, "the hole is at the circle centre {centre:?}, not at {position:?}");
    assert!(h.app.session.model.result(id).is_some_and(|r| r.error.is_none() && r.warning.is_none()));
    // The bore (5 mm across, 10 mm deep) is round about the centre: empty on its axis and up to
    // 2.4 mm from it, solid 2.6 mm from it in every direction.
    let bore = |h: &Harness| {
        for k in 0..8 {
            let a = std::f64::consts::FRAC_PI_4 * f64::from(k);
            let dir = Vec3::new(a.cos(), a.sin(), 0.0);
            let at = |r: f64| centre + dir * r - Vec3::new(0.0, 0.0, 5.0);
            assert!(!solid_at(h, at(0.0)) && !solid_at(h, at(2.4)), "the bore is empty {a} rad from +x");
            assert!(solid_at(h, at(2.6)), "the wall is solid {a} rad from +x");
        }
    };
    bore(&h);
    // Edit Feature shows the Single placement on the top face at the centre, and OK keeps it.
    h.step(&json!({"call": "ui.editFeature", "params": {"feature": id}})).unwrap();
    h.frames(4);
    let d = h.app.dialog.as_ref().expect("the hole dialog");
    assert!(matches!(&d.kind, Kind::Hole { opts, .. } if !opts.multiple), "{:?}", d.kind);
    let places = face_places(&h);
    assert!(matches!(places.as_slice(), [(body, p)] if body == "Body1" && p.dist(centre) < 1e-6), "{places:?}");
    h.step(&json!({"key": "Enter"})).unwrap();
    assert!(h.app.dialog.is_none(), "OK closed the dialog: {:?}", h.app.dialog.as_ref().map(|d| &d.error));
    let (again, position, at_points) = hole(&h);
    assert_eq!(again, id, "the same feature");
    assert!(!at_points && position.dist(centre) < 1e-9, "{position:?}");
    bore(&h);
}

#[test]
fn a_single_hole_picked_away_from_sketch_points_stays_where_it_was_clicked() {
    let (mut h, centre) = block_with_circle();
    // Inside the circle, well away from its centre (and from every other sketch point).
    let away = centre + Vec3::new(3.0, 2.0, 0.0);
    let click = screen(&mut h, away);
    let c = screen(&mut h, centre);
    assert!(c.iter().zip(click).map(|(a, b)| (a - b).powi(2)).sum::<f64>().sqrt() > 16.0, "twice a point pick's reach from the centre");
    h.step(&json!({"start": "solid.hole"})).unwrap();
    let raw = hovered_face_point(&mut h, click);
    h.step(&json!({"click": click})).unwrap();
    h.step(&json!({"key": "Enter"})).unwrap();
    assert!(h.app.dialog.is_none());
    let (_, position, at_points) = hole(&h);
    assert!(!at_points);
    assert!(position.dist(raw) < 1e-9 && position.dist(away) < 1e-3, "the hole stays where it was clicked {raw:?}, not at {position:?}");
}

#[test]
fn a_single_hole_picked_near_a_sketch_point_off_the_face_under_it_stays_where_it_was_clicked() {
    // The circle 10 mm above the top face (its centre lies on no face), then on the bottom face
    // (hidden behind the top face): either way its centre is under the pick on screen, but not on
    // the face picked.
    // (The bottom face's sketch looks up through the block: its y runs along the world's −y.)
    let cases = [(json!({"plane": "XY", "offset": 30}), [12.0, 9.0], 30.0), (json!({"plane": {"face": [20, 15, 0]}}), [12.0, -9.0], 0.0)];
    for (sketch, at, z) in cases {
        let (mut h, centre) = block_and_circle_on(sketch, at);
        assert!(centre.dist(Vec3::new(12.0, 9.0, z)) < 1e-9, "{centre:?}");
        let click = near(&mut h, centre);
        h.step(&json!({"start": "solid.hole"})).unwrap();
        let hits = hits_at(&h, click);
        assert!(matches!(hits.first(), Some(Hit::SketchPoint { .. })), "the click reaches the centre: {hits:?}");
        let raw = hovered_face_point(&mut h, click);
        h.step(&json!({"click": click})).unwrap();
        h.step(&json!({"key": "Enter"})).unwrap();
        assert!(h.app.dialog.is_none());
        let (_, position, at_points) = hole(&h);
        assert!(!at_points);
        assert!(position.dist(raw) < 1e-9, "the hole stays where it was clicked {raw:?}, not at {position:?}");
        assert!((position.z - 20.0).abs() < 1e-9 && position.dist(Vec3::new(centre.x, centre.y, 20.0)) > 0.1, "{position:?}");
    }
}

#[test]
fn select_other_offers_the_centre_and_the_face_as_picked() {
    let (mut h, centre) = block_with_circle();
    let click = near(&mut h, centre);
    h.step(&json!({"start": "solid.hole"})).unwrap();
    // Where the click's ray meets the top face.
    let ray = hits_at(&h, click);
    let Some(Hit::Face { point: raw, .. }) = ray.iter().find(|x| matches!(x, Hit::Face { .. })) else { panic!("{ray:?}") };
    let raw = *raw;
    assert!(raw.dist(centre) > 0.1);
    for (row, want) in [(0, centre), (1, raw)] {
        let list = h.call("ui.selectOther", json!({"x": click[0], "y": click[1]}))["result"].clone();
        let labels: Vec<&str> = list["rows"].as_array().into_iter().flatten().filter_map(|r| r["label"].as_str()).collect();
        assert!(labels.len() == 2 && labels[0].contains("centre") && labels[1].starts_with("Face"), "the snapped centre, then the face: {labels:?}");
        h.call("ui.selectOther", json!({ "pick": row }));
        let places = face_places(&h);
        assert!(matches!(places.as_slice(), [(_, p)] if p.dist(want) < 1e-9), "row {row} places at {want:?}: {places:?}");
        // Start over for the other row (a second pick on the same face would take it out).
        h.step(&json!({"key": "Escape"})).unwrap();
        h.step(&json!({"start": "solid.hole"})).unwrap();
    }
}

#[test]
fn select_other_keeps_the_face_as_picked_beside_many_snapped_points() {
    // More points under the cursor than Select Other lists of one kind: the snapped places are
    // capped as points, and the face as picked keeps its row.
    let mut h = Harness::new();
    h.app.set_origin_shown(false);
    h.app.run("solid.box", json!({"length": 40, "width": 30, "height": 20})).unwrap();
    h.app.run("sketch.create", json!({"plane": {"face": [20, 15, 20]}})).unwrap();
    let many = crate::select_other::MAX_PER_KIND + 4;
    for i in 0..many {
        let a = i as f64 * 0.3;
        h.app.run("sketch.point", json!({"point": [12.0 + 0.1 * a.cos(), 9.0 + 0.1 * a.sin()]})).unwrap();
    }
    h.app.run("sketch.finish", json!({})).unwrap();
    h.step(&json!({"call": "ui.view", "params": {"view": "top"}})).unwrap();
    h.settle();
    let click = near(&mut h, Vec3::new(12.0, 9.0, 20.0));
    h.step(&json!({"start": "solid.hole"})).unwrap();
    let list = h.call("ui.selectOther", json!({"x": click[0], "y": click[1]}))["result"].clone();
    let rows = list["rows"].as_array().cloned().unwrap_or_default();
    let count = |prefix: &str| rows.iter().filter(|r| r["label"].as_str().is_some_and(|l| l.starts_with(prefix))).count();
    assert_eq!((count("Point"), count("Face")), (crate::select_other::MAX_PER_KIND, 1), "{list}");
    assert_eq!(list["total"], json!(many + 1), "{list}");
}

/// The block, seen from the top, with an "L" 10 mm high written on its top face from (5, 5), and
/// with a circle of radius 0.5 about `circle` (millimetres right of the foot's lower right corner)
/// when given. Returns the corner, then the circle centre, in the world.
fn block_with_lettering(circle: Option<f64>) -> (Harness, Vec3, Option<Vec3>) {
    let mut h = Harness::new();
    h.app.set_origin_shown(false);
    h.app.run("solid.box", json!({"length": 40, "width": 30, "height": 20})).unwrap();
    h.app.run("sketch.create", json!({"plane": {"face": [20, 15, 20]}})).unwrap();
    h.app.run("sketch.text", json!({"text": "L", "at": [5, 5], "height": 10})).unwrap();
    let outline = |h: &Harness| {
        let st = h.app.session.world_state();
        let ss = st.sketches.first().unwrap();
        let sk = &ss.sketch;
        let text = |p: &&SPoint| p.link.as_deref().and_then(|l| sk.link(l)).is_some_and(|l| l.kind == LinkKind::Text);
        sk.points.iter().filter(text).map(|p| p.pos).collect::<Vec<Vec2>>()
    };
    let corner = outline(&h).into_iter().max_by(|a, b| (a.x - a.y).total_cmp(&(b.x - b.y))).unwrap();
    let centre = circle.map(|dx| Vec2::new(corner.x + dx, corner.y));
    if let Some(c) = centre {
        h.app.run("sketch.circle.center", json!({"center": [c.x, c.y], "radius": 0.5})).unwrap();
    }
    h.app.run("sketch.finish", json!({})).unwrap();
    h.step(&json!({"call": "ui.view", "params": {"view": "top"}})).unwrap();
    h.settle();
    let st = h.app.session.world_state();
    let plane = st.sketches.first().unwrap().plane;
    assert_eq!(outline(&h).len(), 6, "the L's outline has six corners");
    (h, plane.to_world(corner), centre.map(|c| plane.to_world(c)))
}

/// Where the ray through a screen point meets the nearest face.
fn face_point_at(h: &Harness, at: [f64; 2]) -> Vec3 {
    let ray = hits_at(h, at);
    match ray.iter().find(|x| matches!(x, Hit::Face { .. })) {
        Some(Hit::Face { point, .. }) => *point,
        _ => panic!("no face under {at:?}: {ray:?}"),
    }
}

#[test]
fn a_single_hole_picked_beside_lettering_stays_where_it_was_clicked() {
    // Text outlines' points are not drawn and not picked on their own (#718 review): a click 5 px
    // off a letter's corner keeps the face point it was clicked at.
    let (mut h, corner, _) = block_with_lettering(None);
    assert!((corner.z - 20.0).abs() < 1e-9, "the lettering lies on the top face: {corner:?}");
    let click = near(&mut h, corner);
    h.step(&json!({"start": "solid.hole"})).unwrap();
    let raw = face_point_at(&h, click);
    h.step(&json!({"move": click})).unwrap();
    assert!(matches!(&h.app.viewport.hover, Some(Hit::Face { .. })), "the face lights up, not a letter's corner: {:?}", h.app.viewport.hover);
    h.step(&json!({"click": click})).unwrap();
    let places = face_places(&h);
    assert!(matches!(places.as_slice(), [(_, p)] if p.dist(raw) < 1e-9), "the face is picked where clicked {raw:?}, not {places:?}");
    h.step(&json!({"key": "Enter"})).unwrap();
    assert!(h.app.dialog.is_none(), "OK closed the dialog: {:?}", h.app.dialog.as_ref().map(|d| &d.error));
    let (_, position, at_points) = hole(&h);
    assert!(!at_points && position.dist(raw) < 1e-9 && position.dist(corner) > 0.5, "{position:?}");
}

#[test]
fn a_single_hole_picked_beside_lettering_snaps_to_a_shown_point_in_reach() {
    // A circle centre 1.2 mm (8 px) right of a letter's corner. The click is nearer the corner
    // (2.4 px) but within reach of the centre (5.6 px): the centre is the point the pick snaps to.
    let (mut h, corner, centre) = block_with_lettering(Some(1.2));
    let centre = centre.unwrap();
    let (c, p) = (screen(&mut h, corner), screen(&mut h, centre));
    let d = ((p[0] - c[0]).powi(2) + (p[1] - c[1]).powi(2)).sqrt();
    assert!(d > 6.0 && d < 10.0, "the centre is {d} px from the corner");
    let click = [c[0] + (p[0] - c[0]) * 0.3, c[1] + (p[1] - c[1]) * 0.3];
    h.step(&json!({"start": "solid.hole"})).unwrap();
    h.step(&json!({"click": click})).unwrap();
    let places = face_places(&h);
    assert!(matches!(places.as_slice(), [(_, p)] if p.dist(centre) < 1e-9), "the face is picked at the centre {centre:?}, not {places:?}");
}

#[test]
fn a_single_hole_snaps_only_to_points_its_sketch_shows() {
    // A face sketch projects the face's edges, so the face's corners are points of the sketch: a
    // click 5 px inside a corner snaps to it while the sketch shows projected geometry, and stays
    // where it was clicked while the sketch hides it (Show Projected off).
    for shown in [true, false] {
        let mut h = Harness::new();
        h.app.set_origin_shown(false);
        h.app.run("solid.box", json!({"length": 40, "width": 30, "height": 20})).unwrap();
        h.app.run("sketch.create", json!({"plane": {"face": [20, 15, 20]}})).unwrap();
        h.app.run("sketch.options", json!({"show_projected": shown})).unwrap();
        h.app.run("sketch.finish", json!({})).unwrap();
        h.step(&json!({"call": "ui.view", "params": {"view": "top"}})).unwrap();
        h.settle();
        let corner = Vec3::new(40.0, 30.0, 20.0);
        let c = screen(&mut h, corner);
        let click = [c[0] - 4.0, c[1] + 3.0];
        h.step(&json!({"start": "solid.hole"})).unwrap();
        let raw = face_point_at(&h, click);
        assert!(raw.dist(corner) > 0.5 && (raw.z - 20.0).abs() < 1e-9, "{raw:?}");
        h.step(&json!({"click": click})).unwrap();
        let want = if shown { corner } else { raw };
        let places = face_places(&h);
        assert!(matches!(places.as_slice(), [(_, p)] if p.dist(want) < 1e-9), "projected shown: {shown}; at {want:?}, not {places:?}");
    }
    // A hidden sketch offers no points: its circle's centre is not snapped to.
    let (mut h, centre) = block_with_circle();
    let name = h.app.session.doc.features.iter().find(|f| matches!(f.kind, FeatureKind::Sketch { .. })).unwrap().name.clone();
    h.app.run("browser.visibility", json!({"items": [name], "visible": false})).unwrap();
    let click = near(&mut h, centre);
    h.step(&json!({"start": "solid.hole"})).unwrap();
    let raw = face_point_at(&h, click);
    h.step(&json!({"click": click})).unwrap();
    let places = face_places(&h);
    assert!(matches!(places.as_slice(), [(_, p)] if p.dist(raw) < 1e-9 && p.dist(centre) > 0.1), "{places:?}");
    // Show Points off only hides the markers of the sketch being edited (a finished sketch draws
    // none): its circle's centre is still snapped to.
    let mut h = Harness::new();
    h.app.set_origin_shown(false);
    h.app.run("solid.box", json!({"length": 40, "width": 30, "height": 20})).unwrap();
    h.app.run("sketch.create", json!({"plane": {"face": [20, 15, 20]}})).unwrap();
    h.app.run("sketch.options", json!({"show_points": false})).unwrap();
    h.app.run("sketch.circle.center", json!({"center": [12, 9], "radius": 4})).unwrap();
    h.app.run("sketch.finish", json!({})).unwrap();
    h.step(&json!({"call": "ui.view", "params": {"view": "top"}})).unwrap();
    h.settle();
    let centre = Vec3::new(12.0, 9.0, 20.0);
    let click = near(&mut h, centre);
    h.step(&json!({"start": "solid.hole"})).unwrap();
    h.step(&json!({"click": click})).unwrap();
    let places = face_places(&h);
    assert!(matches!(places.as_slice(), [(_, p)] if p.dist(centre) < 1e-9), "Show Points off: at the centre, not {places:?}");
}

/// Three Select Other lists' worth of sketch points under the cursor, with a Single hole started.
struct Cluster {
    h: Harness,
    /// How many points.
    many: usize,
    /// 5 px off them.
    click: [f64; 2],
    /// Triangles of the body under the click, of the face under it, and of that face's triangles
    /// the ones whose bounding box comes within a micrometre of the points' bounding box.
    body_tris: usize,
    face_tris: usize,
    near_tris: usize,
}

impl Cluster {
    /// Select Other at the click: the list, then (planar face lookups, triangles the snap looked
    /// at) while making it.
    fn select_other(&mut self) -> (Value, (usize, usize)) {
        crate::dialogs::take_snap_work();
        let list = self.h.call("ui.selectOther", json!({"x": self.click[0], "y": self.click[1]}))["result"].clone();
        (list, crate::dialogs::take_snap_work())
    }
}

/// How many rows of a Select Other list are labelled `prefix`….
fn rows_named(list: &Value, prefix: &str) -> usize {
    list["rows"].as_array().into_iter().flatten().filter(|r| r["label"].as_str().is_some_and(|l| l.starts_with(prefix))).count()
}

/// A cylinder of radius 15 and height 20 standing on the origin, seen from `view`, with the
/// points within 0.1 mm of `at` in the sketch `sketch.create` makes with `sketch`.
fn cylinder_with_cluster(sketch: Value, at: [f64; 2], view: &str) -> Cluster {
    let mut h = Harness::new();
    h.app.set_origin_shown(false);
    h.app.run("solid.cylinder", json!({"radius": 15, "height": 20})).unwrap();
    h.app.run("sketch.create", sketch).unwrap();
    let many = 3 * crate::select_other::MAX_PER_KIND;
    let local: Vec<Vec2> = (0..many).map(|i| i as f64 * 0.3).map(|a| Vec2::new(at[0] + 0.1 * a.cos(), at[1] + 0.1 * a.sin())).collect();
    for p in &local {
        h.app.run("sketch.point", json!({"point": [p.x, p.y]})).unwrap();
    }
    h.app.run("sketch.finish", json!({})).unwrap();
    h.step(&json!({"call": "ui.view", "params": {"view": view}})).unwrap();
    h.settle();
    let plane = h.app.session.world_state().sketches.first().unwrap().plane;
    let centre = plane.to_world(Vec2::new(at[0], at[1]));
    let mut points = Aabb3::EMPTY;
    local.iter().for_each(|p| points.add(plane.to_world(*p)));
    let click = near(&mut h, centre);
    h.step(&json!({"start": "solid.hole"})).unwrap();
    let ray = hits_at(&h, click);
    let Some(Hit::Face { body, index, .. }) = ray.iter().find(|x| matches!(x, Hit::Face { .. })) else { panic!("no face under {centre:?}: {ray:?}") };
    let m = h.app.session.world_state().body(body).unwrap().mesh();
    let face: Vec<[Vec3; 3]> = m.triangles.iter().zip(&m.tri_face).filter(|(_, f)| **f as usize == *index).filter_map(|(t, _)| m.tri(t)).collect();
    let near_points = |t: &[Vec3; 3]| {
        let mut b = Aabb3::EMPTY;
        t.iter().for_each(|v| b.add(*v));
        let gap = (b.min - points.max).max(points.min - b.max);
        gap.x.max(gap.y).max(gap.z) <= 1e-3
    };
    let (body_tris, face_tris, near_tris) = (m.triangles.len(), face.len(), face.iter().filter(|t| near_points(t)).count());
    assert!(body_tris >= 2 * face_tris, "the body has other faces: {body_tris} and {face_tris} triangles");
    Cluster { h, many, click, body_tris, face_tris, near_tris }
}

#[test]
fn select_other_over_a_dense_cluster_looks_the_face_up_once() {
    // The points on the cylinder's top face, a disc of a small part of the body's triangles. Select
    // Other looks the face under the cursor up once for all of them, not once per point (#718
    // review): one planarity lookup, which tessellates the body, and one pass over the body's
    // triangles. Each point is then tested against the face's triangles near the points only,
    // not the face's or the body's; every point on the face is still counted.
    let mut c = cylinder_with_cluster(json!({"plane": {"face": [0, 0, 20]}}), [5.0, 4.0], "top");
    let (many, body, face, near) = (c.many, c.body_tris, c.face_tris, c.near_tris);
    assert!(body >= 4 * face && 4 * near <= face, "{body} triangles in the body, {face} in the face, {near} of them near the points");
    let (list, (lookups, tested)) = c.select_other();
    assert_eq!((rows_named(&list, "Point"), rows_named(&list, "Face")), (crate::select_other::MAX_PER_KIND, 1), "{list}");
    assert_eq!(list["total"], json!(many + 1), "every point on the face is counted: {list}");
    assert_eq!(lookups, 1, "the face is looked up once for {many} points");
    assert!(tested <= body + many * near, "{tested} triangles looked at for {many} points ({body} in the body, {near} near the points)");
}

#[test]
fn select_other_over_a_dense_cluster_off_the_face_tests_no_triangles() {
    // The points 10 mm above the cylinder's top face, seen from the top: under the cursor, but
    // off the face's plane. Each is turned down without a triangle test, and the face as picked
    // is all that is offered.
    let mut c = cylinder_with_cluster(json!({"plane": "XY", "offset": 30}), [5.0, 4.0], "top");
    let (many, body) = (c.many, c.body_tris);
    let (list, (lookups, tested)) = c.select_other();
    assert_eq!((rows_named(&list, "Point"), rows_named(&list, "Face")), (0, 1), "{list}");
    assert_eq!(list["total"], json!(1), "{list}");
    assert_eq!(lookups, 1, "the face is looked up once for {many} points");
    assert_eq!(tested, body, "one pass over the body's {body} triangles, and no test for the {many} points off the face");
}

#[test]
fn select_other_over_a_dense_cluster_by_a_curved_face_looks_it_up_once() {
    // The points in a sketch through the cylinder's axis, seen from the front: the face under the
    // cursor is the curved side. A hole goes on it where it is picked (sketch points snap only
    // on flat faces): it is looked up once, its triangles not at all, and only the face is offered.
    let mut c = cylinder_with_cluster(json!({"plane": "XZ"}), [5.0, -10.0], "front");
    let many = c.many;
    let (list, (lookups, tested)) = c.select_other();
    assert_eq!((rows_named(&list, "Point"), rows_named(&list, "Face")), (0, 1), "the curved face alone: {list}");
    assert_eq!(lookups, 1, "the curved face is looked up once for {many} points");
    assert_eq!(tested, 0, "no triangle of a curved face is looked at");
}

#[test]
fn select_other_snaps_no_more_points_than_the_budget_allows() {
    // With the snap's budget cut to 30 point-in-triangle tests, at most 30 of the points on the
    // face are offered (each takes one test at least), and the face as picked keeps its row.
    let mut c = cylinder_with_cluster(json!({"plane": {"face": [0, 0, 20]}}), [5.0, 4.0], "top");
    let (many, body) = (c.many, c.body_tris);
    crate::dialogs::set_snap_budget(Some(30));
    let (list, (_, tested)) = c.select_other();
    crate::dialogs::set_snap_budget(None);
    assert!(many > 30, "more points than the budget");
    assert_eq!(rows_named(&list, "Face"), 1, "{list}");
    let (points, total) = (rows_named(&list, "Point"), list["total"].as_u64().unwrap() as usize);
    assert!(points >= 1 && total > points && total <= 30 + 1, "some points within the budget, and the face: {list}");
    assert!(tested <= body + 30, "{tested} triangles looked at");
}

#[test]
fn a_face_place_snaps_to_the_points_of_shown_geometry_only() {
    // Which points a Single hole snaps to, against the rule the viewport draws them by: not a
    // text outline's points (`view::sketch_point_visible`), and no linked point while Show
    // Projected is off. An id names its first link; a point whose link is gone counts as shown.
    let (h, _, _) = block_with_lettering(None);
    let mut sk = h.app.session.world_state().sketches.first().unwrap().sketch.clone();
    let text = sk.links.iter().find(|l| l.kind == LinkKind::Text).unwrap().clone();
    let edges = sk.links.iter().find(|l| l.kind != LinkKind::Text).unwrap().clone();
    // Later links reusing the ids, with the other kind: the first link of an id is the one.
    let (mut not_text, mut now_text) = (text.clone(), edges.clone());
    not_text.kind = edges.kind;
    now_text.kind = LinkKind::Text;
    sk.links.extend([not_text, now_text]);
    let mut lost = sk.points.last().unwrap().clone();
    lost.id = "lost".into();
    lost.link = Some("gone".into());
    sk.points.push(lost);
    let linked = |l: &str| sk.points.iter().filter(|p| p.link.as_deref() == Some(l)).count();
    assert!(linked(&text.id) == 6 && linked(&edges.id) == 4, "the L's corners and the face's");
    for hide in [false, true] {
        sk.view.hide_projected = hide;
        let takes = crate::dialogs::SnapPoints::of(&sk);
        for (i, p) in sk.points.iter().enumerate() {
            let want = solidcraft_engine::view::sketch_point_visible(&sk, i) && !(hide && p.link.is_some());
            assert_eq!(takes.takes(p), want, "point {} (link {:?}), Show Projected off: {hide}", p.id, p.link);
        }
        let taken = sk.points.iter().filter(|p| takes.takes(p)).count();
        assert_eq!(taken, if hide { 1 } else { 1 + 4 + 1 }, "the origin, and unless hidden the face's corners and the lost point");
    }
}
