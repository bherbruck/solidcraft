//! Inline dimensions while drawing in a sketch. Once a shape's first point is down, value
//! boxes (line length and angle, rectangle width and height, circle diameter…) sit next to the
//! segment they measure and follow the cursor. Typing goes straight into the first box and
//! locks that value; Tab moves to the next box; Enter places the shape with the typed values,
//! which become dimension constraints. Values are expressions ("25", "1 in", "d1 * 2").

use egui::{Pos2, vec2};
use serde_json::{Value, json};
use solidcraft_engine::doc::expr::Kind as ValueKind;
use solidcraft_engine::geom::Vec2;

use crate::SolveApp;
use crate::theme::Tokens;
use crate::tools::Tool;
use crate::viewport::Proj;

/// One value box.
#[derive(Clone, Debug)]
pub struct DimBox {
    pub label: &'static str,
    pub kind: ValueKind,
    /// The expression shown (the live value while not locked).
    pub value: String,
    /// Typed by the user: the value no longer follows the cursor.
    pub locked: bool,
}

impl DimBox {
    fn new(label: &'static str, kind: ValueKind) -> DimBox {
        DimBox { label, kind, value: String::new(), locked: false }
    }
}

/// The boxes a tool shows after `n` points.
pub fn boxes_for(cmd: &str, n: usize) -> Vec<DimBox> {
    use ValueKind::*;
    let b = DimBox::new;
    match (cmd, n) {
        ("sketch.line", n) if n >= 1 => vec![b("Length", Length), b("Angle", Angle)],
        ("sketch.rectangle.two_point" | "sketch.rectangle.center", 1) => vec![b("Width", Length), b("Height", Length)],
        ("sketch.rectangle.rounded", 1) => vec![b("Width", Length), b("Height", Length), b("Radius", Length)],
        ("sketch.circle.center", 1) => vec![b("Diameter", Length)],
        ("sketch.arc.center_point", 1) => vec![b("Radius", Length)],
        ("sketch.arc.center_point", 2) => vec![b("Sweep", Angle)],
        ("sketch.polygon.inscribed" | "sketch.polygon.circumscribed", 1) => vec![b("Radius", Length), b("Sides", Unitless)],
        ("sketch.slot.center_to_center" | "sketch.slot.overall", 1) => vec![b("Length", Length)],
        ("sketch.conic", 2) => vec![b("Rho", Unitless)],
        ("sketch.slot.center_to_center" | "sketch.slot.overall", 2) => vec![b("Width", Length)],
        _ => Vec::new(),
    }
}

/// A locked box's value (mm or radians).
fn locked(app: &SolveApp, t: &Tool, label: &str) -> Option<f64> {
    let b = t.dims.iter().chain(&t.dim_history).find(|b| b.label == label && b.locked)?;
    app.session.doc.eval(&b.value, b.kind).ok().filter(|v| v.is_finite())
}

fn dir_or_x(v: Vec2) -> Vec2 {
    v.normalized().unwrap_or(Vec2::X)
}

fn sign(x: f64) -> f64 {
    if x < 0.0 { -1.0 } else { 1.0 }
}

/// The next point the tool would place for the cursor at `h`, honouring locked values.
pub fn effective(app: &SolveApp, t: &Tool, h: Vec2) -> Vec2 {
    let Some(a) = t.pts.last().map(|p| p.0) else { return h };
    let lk = |l: &str| locked(app, t, l);
    match (t.cmd, t.pts.len()) {
        ("sketch.line", _) | ("sketch.slot.center_to_center" | "sketch.slot.overall", 1) => {
            let v = h - a;
            let len = lk("Length").unwrap_or(v.len());
            let ang = lk("Angle").unwrap_or_else(|| snap_axis(v.angle()));
            a + Vec2::from_angle(ang) * len
        }
        ("sketch.rectangle.two_point" | "sketch.rectangle.rounded", 1) => {
            let d = h - a;
            Vec2::new(a.x + lk("Width").map_or(d.x, |w| w * sign(d.x)), a.y + lk("Height").map_or(d.y, |hh| hh * sign(d.y)))
        }
        ("sketch.rectangle.center", 1) => {
            let d = h - a;
            Vec2::new(a.x + lk("Width").map_or(d.x, |w| w * 0.5 * sign(d.x)), a.y + lk("Height").map_or(d.y, |hh| hh * 0.5 * sign(d.y)))
        }
        ("sketch.circle.center", 1) => a + dir_or_x(h - a) * lk("Diameter").map_or((h - a).len(), |d| d * 0.5),
        ("sketch.arc.center_point", 1) => a + dir_or_x(h - a) * lk("Radius").unwrap_or((h - a).len()),
        ("sketch.arc.center_point", 2) => {
            let c = t.pts.first().map(|p| p.0).unwrap_or(a);
            let r = (a - c).len();
            let a0 = (a - c).angle();
            let sweep = lk("Sweep").unwrap_or_else(|| ((h - c).angle() - a0).rem_euclid(std::f64::consts::TAU));
            c + Vec2::from_angle(a0 + sweep) * r
        }
        ("sketch.polygon.inscribed" | "sketch.polygon.circumscribed", 1) => a + dir_or_x(h - a) * lk("Radius").unwrap_or((h - a).len()),
        ("sketch.slot.center_to_center" | "sketch.slot.overall", 2) => match lk("Width") {
            Some(w) => {
                let c0 = t.pts.first().map(|p| p.0).unwrap_or(a);
                let dir = dir_or_x(a - c0);
                let n = dir.perp();
                let side = sign(n.dot(h - c0));
                c0 + (a - c0) * 0.5 + n * (w * 0.5 * side)
            }
            None => h,
        },
        _ => h,
    }
}

/// The point a click places for the hover `p` (the preview draws this one): typed values win
/// over the cursor and drop its snap; a line's free end snaps to horizontal/vertical, one on
/// sketch geometry stays there, and one on a curve slides along it to where the axis crosses.
pub fn placed(app: &SolveApp, t: &Tool, p: &(Vec2, Option<String>)) -> (Vec2, Option<String>) {
    let line = t.cmd == "sketch.line";
    if t.dims.iter().any(|b| b.locked) || (line && p.1.is_none()) {
        return (effective(app, t, p.0), None);
    }
    if line && let Some(q) = on_curve_axis(app, t, p) {
        return q;
    }
    p.clone()
}

/// Where the preview and the value boxes put the tool's next point for the hover `p`: for a
/// line, exactly what a click places.
pub fn shown(app: &SolveApp, t: &Tool, p: &(Vec2, Option<String>)) -> Vec2 {
    if t.cmd == "sketch.line" { placed(app, t, p).0 } else { effective(app, t, p.0) }
}

/// A line end snapped onto a curve (`on:<curve>:x,y`) within the axis snap: the point where the
/// horizontal/vertical line from the start crosses that curve near the cursor (from a circle's
/// centre, its quadrant point), still on the curve. None: no axis snap there, or the axis misses
/// the curve close by (the curve snap wins as it is).
fn on_curve_axis(app: &SolveApp, t: &Tool, p: &(Vec2, Option<String>)) -> Option<(Vec2, Option<String>)> {
    let a = t.pts.last()?.0;
    let (curve, _) = p.1.as_deref()?.strip_prefix("on:")?.rsplit_once(':')?;
    let v = p.0 - a;
    let ang = snap_axis(v.angle());
    if ang == v.angle() {
        return None;
    }
    let st = app.session.world_state();
    let sk = &st.sketch(app.session.active_sketch?)?.sketch;
    let shape = sk.shape(sk.curve_index(curve)?)?;
    let dir = Vec2::from_angle(ang);
    let horizontal = dir.y.abs() < dir.x.abs();
    let axis = solidcraft_engine::sketch::Shape::Line { a, b: a + dir };
    let tol = 1e-9 * (1.0 + v.len());
    let q = solidcraft_engine::sketch::intersections(&axis, &shape)
        .into_iter()
        // Exactly on the axis, so the command infers the horizontal/vertical constraint.
        .map(|q| if horizontal { Vec2::new(q.x, a.y) } else { Vec2::new(a.x, q.y) })
        .filter(|q| (*q - a).dot(dir) > tol && shape.dist(*q) <= tol && q.dist(p.0) <= v.len() * 0.1)
        .min_by(|x, y| x.dist(p.0).total_cmp(&y.dist(p.0)))?;
    Some((q, Some(format!("on:{curve}:{},{}", q.x, q.y))))
}

/// Angles within a few degrees of horizontal or vertical snap to it (the line is drawn
/// horizontal or vertical, and the command infers the constraint).
pub fn snap_axis(ang: f64) -> f64 {
    let q = std::f64::consts::FRAC_PI_2;
    let k = (ang / q).round();
    if (ang - k * q).abs() < 2.5f64.to_radians() { k * q } else { ang }
}

/// Is the segment exactly horizontal or vertical (an inferred constraint)?
pub fn axis_aligned(a: Vec2, b: Vec2) -> Option<bool> {
    let d = b - a;
    if d.len() < 1e-9 {
        return None;
    }
    if d.y.abs() < 1e-9 * d.len() {
        Some(true)
    } else if d.x.abs() < 1e-9 * d.len() {
        Some(false)
    } else {
        None
    }
}

/// The value each box shows while it follows the cursor, and where it sits (sketch coordinates).
/// `px` is the size of a screen pixel in sketch units: boxes sit a little off the segment they
/// measure so they don't cover it.
fn live(app: &SolveApp, t: &Tool, e: Vec2, px: f64) -> Vec<(f64, Vec2)> {
    let Some(a) = t.pts.last().map(|p| p.0) else { return Vec::new() };
    let first = t.pts.first().map(|p| p.0).unwrap_or(a);
    let off = 20.0 * px;
    match (t.cmd, t.pts.len()) {
        ("sketch.line", _) | ("sketch.slot.center_to_center" | "sketch.slot.overall", 1) => {
            let v = e - a;
            let d = dir_or_x(v);
            let n = d.perp();
            vec![(v.len(), (a + e) * 0.5 + n * off), (v.angle(), a + d * (v.len() * 0.3).min(60.0 * px) - n * off)]
        }
        ("sketch.rectangle.two_point", 1) => {
            let (sx, sy) = (sign(e.x - a.x), sign(e.y - a.y));
            vec![
                ((e.x - a.x).abs(), Vec2::new((a.x + e.x) * 0.5, a.y - sy * off)),
                ((e.y - a.y).abs(), Vec2::new(e.x + sx * 2.0 * off, (a.y + e.y) * 0.5)),
            ]
        }
        ("sketch.rectangle.rounded", 1) => {
            let (sx, sy) = (sign(e.x - a.x), sign(e.y - a.y));
            let r = rounded_radius(app, t, a, e);
            vec![
                ((e.x - a.x).abs(), Vec2::new((a.x + e.x) * 0.5, a.y - sy * off)),
                ((e.y - a.y).abs(), Vec2::new(e.x + sx * 2.0 * off, (a.y + e.y) * 0.5)),
                // Outside the first corner, away from the cursor (a box there would take the click).
                (r, Vec2::new(a.x - sx * 2.0 * off, a.y - sy * off)),
            ]
        }
        ("sketch.rectangle.center", 1) => {
            let d = e - a;
            let (sx, sy) = (sign(d.x), sign(d.y));
            vec![(2.0 * d.x.abs(), Vec2::new(a.x, a.y - d.y - sy * off)), (2.0 * d.y.abs(), Vec2::new(a.x + d.x + sx * 2.0 * off, a.y))]
        }
        ("sketch.circle.center", 1) => vec![(2.0 * (e - a).len(), (a + e) * 0.5 + dir_or_x(e - a).perp() * off)],
        ("sketch.arc.center_point", 1) | ("sketch.polygon.inscribed" | "sketch.polygon.circumscribed", 1) => {
            vec![((e - a).len(), (a + e) * 0.5), (6.0, a + (e - a) * 0.5 + (e - a).perp() * 0.3)]
        }
        ("sketch.arc.center_point", 2) => {
            let a0 = (a - first).angle();
            // Out along the radius, clear of the cursor: a box on it would take the hover and
            // the click meant for a point there (#2201).
            vec![(((e - first).angle() - a0).rem_euclid(std::f64::consts::TAU), e + dir_or_x(e - first) * (2.5 * off))]
        }
        ("sketch.conic", 2) => {
            // `rho` is the curve's fullness, not a distance: the conic passes through
            // `chord midpoint + rho * (apex - chord midpoint)`, so the box sits on that point
            // (the part of the curve the value moves) with the default parabola's 0.5 shown
            // until a value is typed.
            let rho = locked(app, t, "Rho").unwrap_or(0.5);
            let mid = (first + a) * 0.5;
            let d = e - mid;
            vec![(rho, mid + d * rho + dir_or_x(d).perp() * off)]
        }
        ("sketch.slot.center_to_center" | "sketch.slot.overall", 2) => {
            let dir = dir_or_x(a - first);
            let side = if dir.cross(e - first) < 0.0 { -1.0 } else { 1.0 };
            // Beyond the cursor, away from the centre line, as for the arc (#2201).
            vec![(2.0 * dir.cross(e - first).abs(), e + dir.perp() * (side * 2.5 * off))]
        }
        _ => Vec::new(),
    }
}

fn fmt(app: &SolveApp, label: &str, kind: ValueKind, v: f64) -> String {
    match kind {
        ValueKind::Angle => format!("{:.1} deg", v.to_degrees()),
        // A count rounds to a whole number; a ratio keeps its decimals (0.5, not 1).
        ValueKind::Unitless if label == "Rho" => {
            let text = format!("{v:.3}");
            let text = text.trim_end_matches('0').trim_end_matches('.');
            if text.is_empty() { "0".to_string() } else { text.to_string() }
        }
        ValueKind::Unitless => format!("{}", v.round()),
        ValueKind::Length => crate::prefs::show_mm_bare(app, v),
    }
}

/// Make the tool's boxes match its stage (new boxes when a point was placed).
pub fn sync(t: &mut Tool) {
    if t.dims_stage != t.pts.len() {
        if t.dims_stage == 1
            && t.pts.len() == 2
            && matches!(t.cmd, "sketch.arc.center_point" | "sketch.slot.center_to_center" | "sketch.slot.overall")
        {
            t.dim_history.extend(t.dims.iter().filter(|dimension| dimension.locked).cloned());
        } else if t.pts.len() <= t.dims_stage || t.cmd == "sketch.line" {
            t.dim_history.clear();
        }
        t.dims = boxes_for(t.cmd, t.pts.len());
        t.dims_stage = t.pts.len();
        t.dims_focus = !t.dims.is_empty();
    }
}

/// Clear a shape's input immediately, including when another first point arrives before redraw.
pub fn reset(t: &mut Tool) {
    t.dims.clear();
    t.dim_history.clear();
    t.dims_stage = usize::MAX;
    t.dims_focus = false;
}

pub fn has_locked(t: &Tool, label: &str) -> bool {
    t.dims.iter().chain(&t.dim_history).any(|dimension| dimension.locked && dimension.label == label)
}

/// Original expressions, rather than sampled numbers, become the shape's permanent dimensions.
pub fn dimensions(t: &Tool) -> Value {
    let values = t
        .dim_history
        .iter()
        .chain(&t.dims)
        // A rounded rectangle's radius is its own parameter (sent with the shape), not a dimension.
        .filter(|dimension| {
            dimension.locked && !matches!(dimension.label, "Sides" | "Rho") && !(t.cmd == "sketch.rectangle.rounded" && dimension.label == "Radius")
        })
        .map(|dimension| (dimension.label.to_ascii_lowercase(), json!(dimension.value)))
        .collect::<serde_json::Map<_, _>>();
    Value::Object(values)
}

/// Invalid values stay in the current field, so users can correct them before another stage.
pub fn validate(app: &SolveApp, t: &Tool) -> Result<(), String> {
    for dimension in t.dim_history.iter().chain(&t.dims).filter(|dimension| dimension.locked) {
        if dimension.value.len() > 4096 {
            return Err(format!("{} expression is too long", dimension.label));
        }
        let value = app.session.doc.eval(&dimension.value, dimension.kind).map_err(|error| format!("{}: {error}", dimension.label))?;
        if !value.is_finite() || value.abs() > 1e9 {
            return Err(format!("{} must be finite and within the model's size limits", dimension.label));
        }
        match dimension.label {
            "Angle" => {}
            "Sweep" if value.abs() > 1e-9 && value.abs() < std::f64::consts::TAU - 1e-9 => {}
            "Sweep" => return Err("Sweep must lie strictly between -360 and 360 degrees, and cannot be zero".into()),
            "Sides" if (3.0..=200.0).contains(&value) && value.fract() == 0.0 => {}
            "Sides" => return Err("Sides must be an integer from 3 to 200".into()),
            "Rho" if value > 1e-6 && value < 1.0 - 1e-6 => {}
            "Rho" => return Err("Rho must lie strictly between 0 and 1 (below 0.5 an ellipse, 0.5 a parabola, above 0.5 a hyperbola)".into()),
            _ if value > 1e-9 => {}
            _ => return Err(format!("{} must be positive", dimension.label)),
        }
    }
    if t.cmd == "sketch.slot.overall"
        && let (Some(length), Some(width)) = (locked(app, t, "Length"), locked(app, t, "Width"))
        && length <= width
    {
        return Err("Overall slot Length must exceed Width".into());
    }
    Ok(())
}

/// The corner radius a rounded rectangle from `a` to `b` gets: the typed Radius, or a fifth of
/// the shorter side.
pub fn rounded_radius(app: &SolveApp, t: &Tool, a: Vec2, b: Vec2) -> f64 {
    locked(app, t, "Radius").unwrap_or_else(|| 0.2 * (b.x - a.x).abs().min((b.y - a.y).abs()))
}

/// The radius to send with the rounded rectangle: the typed expression (it becomes the radius
/// parameter as written, "d1 / 2" included), or the default as a number.
pub fn rounded_radius_value(app: &SolveApp, t: &Tool, a: Vec2, b: Vec2) -> Value {
    match t.dims.iter().chain(&t.dim_history).find(|d| d.label == "Radius" && d.locked) {
        Some(d) => json!(d.value),
        None => json!(rounded_radius(app, t, a, b)),
    }
}

/// A locked box value by label (for the polygon side count).
pub fn locked_value(app: &SolveApp, t: &Tool, label: &str) -> Option<f64> {
    locked(app, t, label)
}

/// Draw the boxes of the active tool next to what they measure. Enter places the point.
pub fn show(app: &mut SolveApp, ui: &mut egui::Ui, proj: &Proj) {
    // Esc in a box ends the shape (the box had the keyboard, so the shortcut didn't see it).
    let in_box =
        app.tool.as_ref().is_some_and(|t| (0..t.dims.len()).any(|i| ui.ctx().memory(|m| m.had_focus_last_frame(egui::Id::new(("sc_dim", i))))));
    if in_box && !app.esc_handled && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.esc_handled = true;
        crate::tools::finish(app);
        return;
    }
    let Some(mut t) = app.tool.take() else { return };
    sync(&mut t);
    let place = if t.dims.is_empty() { None } else { boxes(app, ui, proj, &mut t) };
    app.tool = Some(t);
    if let Some(p) = place {
        crate::tools::place(app, (p, None));
    }
}

fn boxes(app: &SolveApp, ui: &mut egui::Ui, proj: &Proj, t: &mut Tool) -> Option<Vec2> {
    let tk = Tokens::get();
    let sid = app.session.active_sketch?;
    let st = app.session.world_state();
    let plane = st.sketch(sid)?.plane;
    let e = shown(app, t, t.hover.as_ref()?);
    let px = 2.0 * app.cam.half_height() / f64::from(proj.rect.height().max(1.0));
    let lv = live(app, t, e, px);
    let focus = std::mem::take(&mut t.dims_focus);
    // Egui processes ordinary Tab navigation before these floating text edits are drawn.
    // Remember the box that owned the keyboard and restore a focus within this tool after
    // drawing, so the last dimension wraps to the first rather than reaching the panels.
    let tab = ui.input(|input| {
        input.raw.events.iter().find_map(|event| match event {
            egui::Event::Key { key: egui::Key::Tab, pressed: true, modifiers, .. } if !modifiers.ctrl && !modifiers.alt && !modifiers.command => {
                Some(modifiers.shift)
            }
            _ => None,
        })
    });
    let source = tab.and_then(|_| {
        ui.ctx().memory(|memory| {
            (0..t.dims.len())
                .find(|i| memory.had_focus_last_frame(egui::Id::new(("sc_dim", i))))
                .or_else(|| (0..t.dims.len()).find(|i| memory.has_focus(egui::Id::new(("sc_dim", i)))))
        })
    });
    if source.is_some() {
        // Also cancel navigation on the first focused frame, before the text edit's focus
        // filter has been installed (e.g. immediately repeated keyboard input).
        ui.ctx().memory_mut(|memory| memory.move_focus(egui::FocusDirection::None));
    }
    let mut completed = false;
    let mut any_focused = false;
    let view = app.viewport.layout.as_ref().map(|l| l.drawable).filter(|r| r.is_finite() && r.is_positive());
    for (i, b) in t.dims.iter_mut().enumerate() {
        let Some((v, at)) = lv.get(i).copied() else { continue };
        if !b.locked {
            b.value = fmt(app, b.label, b.kind, v);
        }
        let Some(sp) = proj.to_screen(plane.to_world(at)) else { continue };
        let id = egui::Id::new(("sc_dim", i));
        let has_focus = ui.ctx().memory(|m| m.has_focus(id));
        // While a box follows the cursor its whole text is selected, so typing replaces it.
        if (has_focus || (focus && i == 0)) && !b.locked {
            let mut s = egui::text_edit::TextEditState::load(ui.ctx(), id).unwrap_or_default();
            let n = b.value.chars().count();
            s.cursor.set_char_range(Some(egui::text_selection::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(n))));
            s.store(ui.ctx(), id);
        }
        let pos = in_view(sp + vec2(-34.0, -11.0), view, ui.ctx().pointer_latest_pos());
        crate::frame::area(egui::Id::new(("sc_dim_area", i))).fixed_pos(pos).order(egui::Order::Foreground).show(ui.ctx(), |ui| {
            let fill = if b.locked { tk.accent_soft } else { tk.panel };
            egui::Frame::popup(ui.style()).fill(fill).inner_margin(egui::Margin::symmetric(3, 1)).show(ui, |ui| {
                let r = ui
                    // The drawing tool handles Enter. Keep the text edit focused when a
                    // rejected value needs correction, instead of surrendering the keyboard.
                    .add(egui::TextEdit::singleline(&mut b.value).id(id).desired_width(66.0).return_key(None).event_filter(egui::EventFilter {
                        tab: true,
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        ..Default::default()
                    }))
                    .on_hover_text(b.label);
                // A completion gets first use of forward Tab. Shift+Tab always moves back;
                // the box reached by egui's generic traversal must not complete its text.
                let changed = (source.is_none() || (source == Some(i) && tab != Some(true))) && crate::params_dialog::complete(ui, &r, &mut b.value);
                completed |= changed && source == Some(i);
                if r.changed() || changed {
                    b.locked = true;
                }
                if focus && i == 0 {
                    r.request_focus();
                }
                any_focused |= r.has_focus() || r.lost_focus();
            });
        });
    }
    if let Some(source) = source
        && !completed
    {
        let next = if tab == Some(true) { (source + t.dims.len() - 1) % t.dims.len() } else { (source + 1) % t.dims.len() };
        let id = egui::Id::new(("sc_dim", next));
        if let Some(dimension) = t.dims.get(next) {
            let mut state = egui::TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
            state.cursor.set_char_range(Some(egui::text::CCursorRange::two(
                egui::text::CCursor::new(0),
                egui::text::CCursor::new(dimension.value.chars().count()),
            )));
            state.store(ui.ctx(), id);
        }
        ui.ctx().memory_mut(|memory| memory.request_focus(id));
    }
    let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
    let nothing = ui.ctx().memory(|m| m.focused().is_none());
    (enter && (any_focused || nothing)).then_some(e)
}

/// About the size of a value box, for keeping it in view.
const BOX: egui::Vec2 = egui::vec2(76.0, 24.0);

/// Where a value box whose top left would be `pos` goes so it stays in the drawing area `view`:
/// as it is when it fits; else mirrored through the pointer (a box set off the cursor, past the
/// view's edge, goes to the cursor's other side, still clear of it, #2201); else pushed inside.
fn in_view(pos: Pos2, view: Option<egui::Rect>, pointer: Option<Pos2>) -> Pos2 {
    let Some(view) = view else { return pos };
    let fits = |p: Pos2| view.contains_rect(egui::Rect::from_min_size(p, BOX));
    if fits(pos) {
        return pos;
    }
    if let Some(c) = pointer {
        let mirrored = c + (c - (pos + BOX * 0.5)) - BOX * 0.5;
        if fits(mirrored) {
            return mirrored;
        }
    }
    let max = (view.max - BOX).max(view.min);
    pos.clamp(view.min, max)
}

#[cfg(test)]
#[path = "sketch_dims_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "sketch_typed_intent_tests.rs"]
mod typed_intent_tests;

#[cfg(test)]
#[path = "sketch_clover_ui_tests.rs"]
mod clover_ui_tests;
