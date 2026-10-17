//! Handles on a selected sketch text: a frame around its outlines, a square on each corner
//! that resizes the text (the opposite corner stays where it is) and a round knob above it
//! that turns the text about its centre (Shift: steps of 15°). Each frame of a drag runs
//! `sketch.edit_text` live; the whole gesture is one undo step and Esc cancels it (the drag
//! shares [`crate::viewport::Viewport::text_drag`] with moving a text, so it ends and cancels
//! the same way).
use std::cell::RefCell;

use egui::{Align2, CursorIcon, FontId, Painter, Pos2, Rect, Shape, Stroke, StrokeKind, vec2};
use serde_json::json;
use solidcraft_engine::geom::Vec2;
use solidcraft_engine::sketch::{LinkKind, LinkSource};

use crate::SolveApp;
use crate::theme::Tokens;
use crate::viewport::Proj;

/// Pixels from a handle's centre that still grab it.
const GRAB_PX: f32 = 8.0;
/// How far above the frame the rotation knob sits (pixels).
const KNOB_PX: f32 = 26.0;
/// Smallest and largest factor one resize may apply, so the text stays a usable size.
const SCALE_RANGE: (f64, f64) = (0.02, 50.0);
/// Rotation steps with Shift held.
const ANGLE_STEP_DEG: f64 = 15.0;

/// A text's frame: its source and the box around its outlines in the text's own frame (x
/// along the baseline from `at`, y up from it).
#[derive(Clone, Debug, PartialEq)]
struct Frame {
    link: String,
    at: Vec2,
    height: f64,
    angle: f64,
    min: Vec2,
    max: Vec2,
}

impl Frame {
    /// A point of the text's frame in sketch coordinates.
    fn to_sketch(&self, local: Vec2) -> Vec2 {
        self.at + rotate(local, self.angle)
    }

    /// A sketch point in the text's frame.
    fn to_local(&self, p: Vec2) -> Vec2 {
        rotate(p - self.at, -self.angle)
    }

    /// The corners, counter-clockwise from the bottom left (text frame).
    fn corners(&self) -> [Vec2; 4] {
        [self.min, Vec2::new(self.max.x, self.min.y), self.max, Vec2::new(self.min.x, self.max.y)]
    }

    fn center(&self) -> Vec2 {
        (self.min + self.max) * 0.5
    }
}

fn rotate(v: Vec2, angle: f64) -> Vec2 {
    let (s, c) = angle.sin_cos();
    Vec2::new(v.x * c - v.y * s, v.x * s + v.y * c)
}

/// What a drag holds.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Grab {
    /// A corner (index into [`Frame::corners`]).
    Corner(usize),
    Rotate,
}

#[derive(Clone, Debug)]
struct Drag {
    grab: Grab,
    /// The text as it was when the drag started.
    start: Frame,
    /// Undo depth at the start: every frame folds into one step.
    depth: usize,
    /// Where the pointer went down (sketch coordinates).
    press: Vec2,
}

thread_local! {
    static DRAG: RefCell<Option<Drag>> = const { RefCell::new(None) };
}

/// The text the handles are for: the one selected text, while nothing else (a tool, a
/// dialog) is going on.
fn target(app: &SolveApp) -> Option<String> {
    if let Some(d) = DRAG.with(|d| d.borrow().clone()) {
        return Some(d.start.link);
    }
    if app.tool.is_some() || app.dialog.is_some() {
        return None;
    }
    let entity = crate::sketch_text::selected_entity(app)?;
    let sk = app.session.doc.sketch(app.session.active_sketch?).ok()?;
    sk.curves.iter().find(|c| c.id == entity)?.link.clone()
}

/// The frame of text `link` in the active sketch, as shown (solved).
fn frame(app: &SolveApp, link: &str) -> Option<Frame> {
    let sid = app.session.active_sketch?;
    let state = app.session.world_state();
    let sk = &state.sketch(sid)?.sketch;
    let source = sk.link(link).filter(|l| l.kind == LinkKind::Text)?;
    let LinkSource::Text { at, height, angle, style, .. } = &source.source else { return None };
    // A text on a path follows its curve: it is not placed by hand.
    if style.path.is_some() {
        return None;
    }
    let mut f = Frame { link: link.to_owned(), at: *at, height: *height, angle: *angle, min: Vec2::ZERO, max: Vec2::ZERO };
    let mut first = true;
    for p in sk.link_curves(link).into_iter().flat_map(|ci| sk.polyline(ci)) {
        let q = f.to_local(p);
        if !q.is_finite() {
            continue;
        }
        if first {
            (f.min, f.max, first) = (q, q, false);
        } else {
            f.min = Vec2::new(f.min.x.min(q.x), f.min.y.min(q.y));
            f.max = Vec2::new(f.max.x.max(q.x), f.max.y.max(q.y));
        }
    }
    // A frame needs some width and height to grab (a text of dots or dashes is flat).
    let pad = height * 0.05;
    if f.max.x - f.min.x < pad {
        f.max.x += pad;
        f.min.x -= pad;
    }
    if f.max.y - f.min.y < pad {
        f.max.y += pad;
        f.min.y -= pad;
    }
    (!first).then_some(f)
}

/// The handles on screen: the four corners, then the rotation knob and the point of the frame
/// it hangs from.
struct Screen {
    corners: [Pos2; 4],
    knob: Pos2,
    knob_base: Pos2,
}

fn screen(app: &SolveApp, proj: &Proj, f: &Frame) -> Option<Screen> {
    let sid = app.session.active_sketch?;
    let state = app.session.world_state();
    let plane = state.sketch(sid)?.plane;
    let to_screen = |local: Vec2| proj.to_screen(plane.to_world(f.to_sketch(local)));
    let [a, b, c, d] = f.corners();
    let corners = [to_screen(a)?, to_screen(b)?, to_screen(c)?, to_screen(d)?];
    let top = Vec2::new((f.min.x + f.max.x) * 0.5, f.max.y);
    let knob_base = to_screen(top)?;
    // "Up" for the text, as it looks on screen.
    let up = to_screen(top + Vec2::new(0.0, f.height.max(1e-3)))? - knob_base;
    let up = if up.length() > 1e-3 { up.normalized() } else { vec2(0.0, -1.0) };
    Some(Screen { corners, knob: knob_base + up * KNOB_PX, knob_base })
}

/// The handle under a screen position.
fn hit(app: &SolveApp, proj: &Proj, pos: Pos2) -> Option<(Frame, Grab)> {
    let f = frame(app, &target(app)?)?;
    let s = screen(app, proj, &f)?;
    if s.knob.distance(pos) <= GRAB_PX {
        return Some((f, Grab::Rotate));
    }
    let corner = (0..4).find(|&i| s.corners.get(i).is_some_and(|c| c.distance(pos) <= GRAB_PX))?;
    Some((f, Grab::Corner(corner)))
}

/// Is a handle under the pointer? (A click there does not change the selection.)
pub(crate) fn over(app: &SolveApp, proj: &Proj, pos: Pos2) -> bool {
    hit(app, proj, pos).is_some()
}

/// Where the pointer is on the active sketch's plane.
fn on_plane(app: &SolveApp, proj: &Proj, pos: Pos2) -> Option<Vec2> {
    let state = app.session.world_state();
    let ss = state.sketch(app.session.active_sketch?)?;
    let (o, d) = proj.ray(pos);
    ss.plane.intersect_ray(o, d).map(|w| ss.plane.to_local(w))
}

/// A drag starting on a handle (true: the handles took it).
pub(crate) fn drag_start(app: &mut SolveApp, proj: &Proj, press: Pos2) -> bool {
    let Some((start, grab)) = hit(app, proj, press) else { return false };
    let Some(at) = on_plane(app, proj, press) else { return false };
    let Ok(result) = app.session.execute("edit.checkpoint", &json!({})) else { return false };
    let Some(checkpoint) = result["checkpoint"].as_u64() else { return false };
    app.viewport.text_drag = Some((checkpoint, start.link.clone()));
    DRAG.with(|d| *d.borrow_mut() = Some(Drag { grab, start, depth: app.session.undo.len(), press: at }));
    true
}

/// Is a handle being dragged? A drag the text move machinery ended (Esc, another command, a
/// document switch) is over here too.
pub(crate) fn dragging(app: &SolveApp) -> bool {
    let on = DRAG.with(|d| d.borrow().is_some());
    if on && app.viewport.text_drag.is_none() {
        DRAG.with(|d| *d.borrow_mut() = None);
        return false;
    }
    on
}

/// The text's source after the pointer moved to `to` (sketch coordinates): `at`, `height`
/// and `angle` (radians).
fn follow(d: &Drag, to: Vec2, shift: bool) -> Option<(Vec2, f64, f64)> {
    let f = &d.start;
    match d.grab {
        Grab::Corner(i) => {
            let corners = f.corners();
            let anchor = *corners.get((i + 2) % 4)?;
            let diagonal = *corners.get(i)? - anchor;
            let len2 = diagonal.dot(diagonal);
            if len2 <= 1e-18 {
                return None;
            }
            // The pointer's progress along the diagonal: the text keeps its proportions.
            let s = ((f.to_local(to) - anchor).dot(diagonal) / len2).clamp(SCALE_RANGE.0, SCALE_RANGE.1);
            let pivot = f.to_sketch(anchor);
            Some((pivot + (f.at - pivot) * s, f.height * s, f.angle))
        }
        Grab::Rotate => {
            let center = f.to_sketch(f.center());
            let (a, b) = (d.press - center, to - center);
            if a.len() < 1e-9 || b.len() < 1e-9 {
                return None;
            }
            let mut turn = b.y.atan2(b.x) - a.y.atan2(a.x);
            if shift {
                let step = ANGLE_STEP_DEG.to_radians();
                turn = ((f.angle + turn) / step).round() * step - f.angle;
            }
            Some((center + rotate(f.at - center, turn), f.height, f.angle + turn))
        }
    }
}

/// Follow the pointer: the text's source changes live; one undo step for the whole drag.
pub(crate) fn drag_to(app: &mut SolveApp, proj: &Proj, pos: Pos2, shift: bool) {
    let Some(d) = DRAG.with(|d| d.borrow().clone()) else { return };
    let Some(to) = on_plane(app, proj, pos) else { return };
    let Some((at, height, angle)) = follow(&d, to, shift) else { return };
    let mut params = json!({"link": d.start.link, "at": [at.x, at.y], "live": true});
    // A resize keeps the exact saved angle; a rotation keeps the height (and the expression
    // behind it, if it has one).
    if d.grab == Grab::Rotate {
        params["angle"] = json!(angle.to_degrees());
    } else {
        params["height"] = json!(height);
    }
    if app.session.execute("sketch.edit_text", &params).is_ok() {
        app.session.undo.truncate(d.depth + 1);
    }
}

/// The drag is over: the model follows the text once, and the gesture is kept.
pub(crate) fn drag_end(app: &mut SolveApp) {
    let Some(d) = DRAG.with(|d| d.borrow_mut().take()) else { return };
    let _ = app.session.execute("sketch.drag_end", &json!({}));
    app.session.undo.truncate(d.depth + 1);
    crate::sketch_text_move::end_drag(app);
}

/// Draw the frame and its handles (and, while dragging, the value being changed).
pub(crate) fn show(app: &SolveApp, ui: &egui::Ui, painter: &Painter, proj: &Proj, hover: Option<Pos2>) {
    let Some(link) = target(app) else { return };
    let Some(f) = frame(app, &link) else { return };
    let Some(s) = screen(app, proj, &f) else { return };
    let tk = Tokens::get();
    let line = Stroke::new(1.0, tk.accent);
    let mut outline = s.corners.to_vec();
    outline.push(s.corners[0]);
    painter.add(Shape::dashed_line(&outline, line, 5.0, 4.0));
    painter.line_segment([s.knob_base, s.knob], line);
    let drag = DRAG.with(|d| d.borrow().as_ref().map(|d| d.grab));
    let under = hover.and_then(|p| hit(app, proj, p)).map(|(_, g)| g);
    let lit = |g: Grab| drag == Some(g) || (drag.is_none() && under == Some(g));
    for (i, c) in s.corners.iter().enumerate() {
        let r = Rect::from_center_size(*c, vec2(8.0, 8.0));
        painter.rect_filled(r, 1.0, if lit(Grab::Corner(i)) { tk.accent } else { tk.panel });
        painter.rect_stroke(r, 1.0, Stroke::new(1.5, tk.accent), StrokeKind::Middle);
        crate::scenario::publish_handle(&format!("text_corner_{i}"), *c);
    }
    painter.circle(s.knob, 5.0, if lit(Grab::Rotate) { tk.accent } else { tk.panel }, Stroke::new(1.5, tk.accent));
    crate::scenario::publish_handle("text_rotate", s.knob);
    match drag.or(under) {
        Some(Grab::Rotate) => ui.ctx().set_cursor_icon(if drag.is_some() { CursorIcon::Grabbing } else { CursorIcon::Grab }),
        // The corner's diagonal as it looks on screen.
        Some(Grab::Corner(i)) => {
            let opposite = s.corners[(i + 2) % 4];
            let v = s.corners[i] - opposite;
            let down_right = (v.x >= 0.0) == (v.y >= 0.0);
            ui.ctx().set_cursor_icon(if down_right { CursorIcon::ResizeNwSe } else { CursorIcon::ResizeNeSw });
        }
        None => {}
    }
    // While dragging, the value that changes, next to the pointer.
    if let (Some(grab), Some(p)) = (drag, hover) {
        let label = match grab {
            Grab::Corner(_) => {
                let (scale, unit) = crate::prefs::unit_scale(&app.session.doc.units);
                format!("{:.2} {unit}", f.height * scale)
            }
            Grab::Rotate => format!("{:.1}°", f.angle.to_degrees()),
        };
        painter.text(p + vec2(14.0, -14.0), Align2::LEFT_BOTTOM, label, FontId::proportional(12.0), tk.accent);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame_at(at: Vec2, angle: f64) -> Frame {
        Frame { link: "t".into(), at, height: 10.0, angle, min: Vec2::new(0.0, 0.0), max: Vec2::new(20.0, 10.0) }
    }

    fn drag(grab: Grab, start: Frame, press: Vec2) -> Drag {
        Drag { grab, start, depth: 0, press }
    }

    #[test]
    fn a_corner_scales_about_the_opposite_corner() {
        let f = frame_at(Vec2::new(5.0, 5.0), 0.0);
        // The top right corner pulled to twice the box: the bottom left stays at `at`.
        let d = drag(Grab::Corner(2), f.clone(), Vec2::new(25.0, 15.0));
        let (at, height, angle) = follow(&d, Vec2::new(45.0, 25.0), false).unwrap();
        assert!(at.dist(Vec2::new(5.0, 5.0)) < 1e-12);
        assert!((height - 20.0).abs() < 1e-12);
        assert_eq!(angle, 0.0);
        // The bottom left pulled in: the top right corner stays put.
        let d = drag(Grab::Corner(0), f, Vec2::new(5.0, 5.0));
        let (at, height, _) = follow(&d, Vec2::new(15.0, 10.0), false).unwrap();
        assert!((height - 5.0).abs() < 1e-12);
        let after = Frame { at, height, min: Vec2::ZERO, max: Vec2::new(10.0, 5.0), ..frame_at(at, 0.0) };
        assert!(after.to_sketch(after.max).dist(Vec2::new(25.0, 15.0)) < 1e-12);
    }

    #[test]
    fn a_corner_of_a_turned_text_scales_along_its_own_diagonal() {
        let f = frame_at(Vec2::new(0.0, 0.0), std::f64::consts::FRAC_PI_2);
        let far = f.to_sketch(Vec2::new(30.0, 15.0));
        let d = drag(Grab::Corner(2), f, Vec2::ZERO);
        let (at, height, angle) = follow(&d, far, false).unwrap();
        assert!(at.dist(Vec2::ZERO) < 1e-12);
        assert!((height - 15.0).abs() < 1e-12);
        assert_eq!(angle, std::f64::consts::FRAC_PI_2);
    }

    #[test]
    fn a_resize_never_flips_or_vanishes_the_text() {
        let f = frame_at(Vec2::ZERO, 0.0);
        let d = drag(Grab::Corner(2), f, Vec2::new(20.0, 10.0));
        for to in [Vec2::new(-50.0, -50.0), Vec2::ZERO, Vec2::new(1e9, 1e9)] {
            let (_, height, _) = follow(&d, to, false).unwrap();
            assert!(height.is_finite() && height >= 10.0 * SCALE_RANGE.0 && height <= 10.0 * SCALE_RANGE.1);
        }
    }

    #[test]
    fn the_knob_turns_about_the_centre_and_snaps_with_shift() {
        let f = frame_at(Vec2::ZERO, 0.0);
        let center = f.to_sketch(f.center());
        let d = drag(Grab::Rotate, f, center + Vec2::new(0.0, 10.0));
        // A quarter turn to the left.
        let (at, height, angle) = follow(&d, center + Vec2::new(-10.0, 0.0), false).unwrap();
        assert!((angle - std::f64::consts::FRAC_PI_2).abs() < 1e-12);
        assert_eq!(height, 10.0);
        let turned = Frame { at, angle, ..frame_at(at, angle) };
        assert!(turned.to_sketch(turned.center()).dist(center) < 1e-9, "the centre stays put");
        // 40° snaps to 45° with Shift.
        let to = center + rotate(Vec2::new(0.0, 10.0), 40_f64.to_radians());
        let (_, _, snapped) = follow(&d, to, true).unwrap();
        assert!((snapped - 45_f64.to_radians()).abs() < 1e-12);
        // At the centre itself there is no direction to follow.
        assert!(follow(&d, center, false).is_none());
    }

    use crate::scenario::Harness;
    use solidcraft_engine::Sel;

    /// A sketch with "II" (10 mm) seen from the top, the text selected: its handles are up.
    fn setup() -> (Harness, String) {
        let mut h = Harness::new();
        h.app.home.open = false;
        h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
        let result = h.app.run("sketch.text", json!({"text":"II","at":[0,0],"height":10})).unwrap();
        h.call("ui.view", json!({"view":"top"}));
        h.app.cam.distance = 60.0;
        h.app.session.selection = vec![Sel::SketchCurve { id: result["curves"][0].as_str().unwrap().into() }];
        h.frames(3);
        (h, result["link"].as_str().unwrap().into())
    }

    fn source(h: &Harness, link: &str) -> (Vec2, f64, f64) {
        let sk = h.app.session.doc.sketch(h.app.session.active_sketch.unwrap()).unwrap();
        let LinkSource::Text { at, height, angle, .. } = &sk.link(link).unwrap().source else { panic!("text") };
        (*at, *height, *angle)
    }

    #[test]
    fn double_clicking_a_letter_opens_its_text_for_editing() {
        let (mut h, link) = setup();
        h.app.session.selection.clear();
        h.frames(2);
        let sid = h.app.session.active_sketch.unwrap();
        let inside = h.app.session.world_state().sketch(sid).unwrap().profiles[0].centroid;
        h.step(&json!({"click":{"sketch":[inside.x,inside.y]},"double":true})).unwrap();
        h.frames(2);
        match h.app.dialog.as_ref().map(|d| &d.kind) {
            Some(crate::dialogs::Kind::Text(tx)) => assert_eq!(tx.link, link),
            other => panic!("Edit Text is not open: {other:?}"),
        }
    }

    #[test]
    fn a_point_of_a_letter_stands_for_its_text() {
        let (mut h, link) = setup();
        let sid = h.app.session.active_sketch.unwrap();
        let sk = h.app.session.doc.sketch(sid).unwrap();
        let point = sk.points.iter().find(|p| p.link.as_deref() == Some(link.as_str())).unwrap().id.clone();
        h.app.session.selection = vec![Sel::SketchPoint { id: point }];
        h.frames(2);
        assert!(crate::sketch_text::selected_entity(&h.app).is_some());
        assert!(target(&h.app).is_some_and(|l| l == link), "its handles are up too");
    }

    #[test]
    fn dragging_a_corner_resizes_the_text_in_one_undo_step() {
        let (mut h, link) = setup();
        // Both the initial handle and the doubled text must stay below the floating ribbon.
        h.app.cam.distance = 90.0;
        h.frames(3);
        let before = (h.app.session.doc.to_json(), h.app.session.undo.len());
        let f = frame(&h.app, &link).unwrap();
        // Pull the top right corner out to twice the frame: the bottom left stays put.
        let pivot = f.to_sketch(f.min);
        let target = pivot + (f.to_sketch(f.max) - pivot) * 2.0;
        h.step(&json!({"drag":[{"handle":"text_corner_2"},{"sketch":[target.x,target.y]}],"steps":6})).unwrap();
        h.frames(2);
        let (_, height, angle) = source(&h, &link);
        assert!((height - 20.0).abs() < 0.5, "height {height}");
        assert_eq!(angle, 0.0);
        let after = frame(&h.app, &link).unwrap();
        assert!(after.to_sketch(after.min).dist(pivot) < 0.3, "the opposite corner stays where it was");
        assert!(!h.app.session.model.is_stale(), "the model follows when the drag ends");
        assert!(h.app.viewport.text_drag.is_none());
        assert_eq!(h.app.session.undo.len(), before.1 + 1);
        h.app.run("edit.undo", json!({})).unwrap();
        assert_eq!(h.app.session.doc.to_json(), before.0);
    }

    #[test]
    fn turning_a_text_keeps_the_expression_of_its_height_and_resizing_replaces_it() {
        let mut h = Harness::new();
        h.app.home.open = false;
        h.app.run("parameters.add", json!({"name":"tall","expression":"10 mm"})).unwrap();
        h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
        let result = h.app.run("sketch.text", json!({"text":"II","at":[0,0],"height":"tall"})).unwrap();
        let link = result["link"].as_str().unwrap().to_owned();
        h.call("ui.view", json!({"view":"top"}));
        // Keep the rotated resize target clear of the floating ribbon too.
        h.app.cam.distance = 90.0;
        h.app.session.selection = vec![Sel::SketchCurve { id: result["curves"][0].as_str().unwrap().into() }];
        h.frames(3);
        let height_expr = |h: &Harness| {
            let sk = h.app.session.doc.sketch(h.app.session.active_sketch.unwrap()).unwrap();
            let LinkSource::Text { exprs, .. } = &sk.link(&link).unwrap().source else { panic!("text") };
            exprs.height.clone()
        };
        assert_eq!(height_expr(&h).as_deref(), Some("tall"));
        let f = frame(&h.app, &link).unwrap();
        let center = f.to_sketch(f.center());
        let to = center + rotate(Vec2::new(0.0, 15.0), 40_f64.to_radians());
        h.step(&json!({"drag":[{"handle":"text_rotate"},{"sketch":[to.x,to.y]}],"steps":6})).unwrap();
        h.frames(2);
        let (_, height, angle) = source(&h, &link);
        assert!(angle.abs() > 0.5 && height == 10.0, "turned: {angle} {height}");
        assert_eq!(height_expr(&h).as_deref(), Some("tall"), "turning leaves the height alone");
        let f = frame(&h.app, &link).unwrap();
        let pivot = f.to_sketch(f.min);
        let target = pivot + (f.to_sketch(f.max) - pivot) * 2.0;
        h.step(&json!({"drag":[{"handle":"text_corner_2"},{"sketch":[target.x,target.y]}],"steps":6})).unwrap();
        h.frames(2);
        assert!((source(&h, &link).1 - 20.0).abs() < 0.5);
        assert_eq!(height_expr(&h), None, "a height set by hand is a number");
    }

    #[test]
    fn the_rotation_knob_turns_the_text_in_steps_with_shift() {
        let (mut h, link) = setup();
        let f = frame(&h.app, &link).unwrap();
        let center = f.to_sketch(f.center());
        // From above the centre to its upper left: about 40°, 45° with Shift.
        let to = center + rotate(Vec2::new(0.0, 15.0), 40_f64.to_radians());
        h.step(&json!({"drag":[{"handle":"text_rotate"},{"sketch":[to.x,to.y]}],"steps":6,"shift":true})).unwrap();
        h.frames(2);
        let (_, height, angle) = source(&h, &link);
        assert!((angle - 45_f64.to_radians()).abs() < 1e-9, "angle {}", angle.to_degrees());
        assert_eq!(height, 10.0);
        let after = frame(&h.app, &link).unwrap();
        assert!(after.to_sketch(after.center()).dist(center) < 0.05, "it turns about its centre");
    }
}
