//! More sketch creation tools: midpoint line, tangent arc, tangent circles, center-point and
//! arc slots.

use serde_json::{Value, json};
use solidcraft_geom::Vec2;
use solidcraft_sketch::{ConstraintKind, CurveKind, LinkKind, LinkSource, Sketch, TextAlign, TextExprs, TextStyle};

/// The style parameters of `sketch.text` and `sketch.edit_text`.
macro_rules! text_style_doc {
    () => {
        "bold?, italic?: bool (an installed family's bold or italic face; italic without one is slanted), align?: left|center|right (the lines against `at`, or along the path), letter_spacing?: extra mm between letters, line_spacing?: times the font's line distance (default 1), path?: a curve id of the sketch the baseline follows (\"\": none; `at` and `angle` then do not place the text; the curve becomes construction, a guide the letters stand on), reverse?: bool (the other way along the path)"
    };
}

use super::sketch::{add_c, circumcircle, edit, ids_of, mark_construction, req_parg, result, slot};
use super::{CommandSpec, in_sketch};
use crate::params::{bad, num, req_vec2, str_, string_list};
use crate::{Result, Session};

#[cfg(test)]
#[path = "sketch_text_move_tests.rs"]
mod text_move_tests;

#[cfg(test)]
#[path = "sketch_text_expr_tests.rs"]
mod text_expr_tests;

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("sketch.line.midpoint", "Midpoint Line", midpoint_line)
        .at("SKETCH", "CREATE")
        .icon("line_mid")
        .enabled(in_sketch)
        .params(concat!("mid: [x,y] or point ref, end: [x,y]; construction?", created_doc!())),
    CommandSpec::new("sketch.arc.tangent", "Tangent Arc", arc_tangent).at("SKETCH", "CREATE").icon("arc_tangent").enabled(in_sketch).params(concat!(
        "start: end point of a line or arc (\"l1.end\"), end: [x,y]; curve?: the curve to be tangent to (default: the one ending at start)",
        created_doc!()
    )),
    CommandSpec::new("sketch.circle.two_tangent", "2-Tangent Circle", circle_tan_tan)
        .at("SKETCH", "CREATE")
        .icon("circle_tt")
        .enabled(in_sketch)
        .params(concat!("curves: [two lines/circles/arcs], radius | diameter: expr, near: [x,y] (which of the solutions)", created_doc!())),
    CommandSpec::new("sketch.circle.three_tangent", "3-Tangent Circle", circle_three_tan)
        .at("SKETCH", "CREATE")
        .icon("circle_ttt")
        .enabled(in_sketch)
        .params(concat!("curves: [three lines/circles/arcs], near: [x,y] (which of the solutions)", created_doc!())),
    CommandSpec::new("sketch.slot.center_point", "Center Point Slot", slot_center_point)
        .at("SKETCH", "CREATE")
        .icon("slot_center")
        .enabled(in_sketch)
        .params(concat!("center: slot centre, end: one arc centre, width", created_doc!())),
    CommandSpec::new("sketch.slot.arc_three_point", "Three Point Arc Slot", arc_slot_three)
        .at("SKETCH", "CREATE")
        .icon("arc_slot")
        .enabled(in_sketch)
        .params(concat!("start, through, end: points on the centre arc, width", created_doc!())),
    CommandSpec::new("sketch.slot.arc_center", "Center Point Arc Slot", arc_slot_center)
        .at("SKETCH", "CREATE")
        .icon("arc_slot_center")
        .enabled(in_sketch)
        .params(concat!("center, start: centre arc start, end: direction of the centre arc end (counter-clockwise), width", created_doc!())),
    CommandSpec::new("sketch.ellipse", "Ellipse", ellipse).at("SKETCH", "CREATE").icon("ellipse").enabled(in_sketch).params(concat!(
        "center, major: end of the major axis [x,y], minor: a point the ellipse passes through on the minor side [x,y] | minor_radius",
        created_doc!()
    )),
    CommandSpec::new("sketch.spline.fit_point", "Fit Point Spline", spline_fit)
        .at("SKETCH", "CREATE")
        .icon("spline")
        .enabled(in_sketch)
        .params(concat!("points: [[x,y] or point refs…] (2…500): the spline passes through them, periodic?: boolean (default false); periodic cubic fit closes smoothly through at least 3 distinct samples", created_doc!())),
    CommandSpec::new("sketch.spline.control_point", "Control Point Spline", spline_cv3)
        .at("SKETCH", "CREATE")
        .icon("spline_cv")
        .enabled(in_sketch)
        .params(concat!("points: control points (2…500), cubic", created_doc!())),
    CommandSpec::new("sketch.spline.control_point_5", "Control Point Spline (Degree 5)", spline_cv5)
        .at("SKETCH", "CREATE")
        .icon("spline_cv")
        .enabled(in_sketch)
        .params(concat!("points: control points (2…500), degree 5", created_doc!())),
    CommandSpec::new("sketch.conic", "Conic Curve", conic)
        .at("SKETCH", "CREATE")
        .icon("conic")
        .enabled(in_sketch)
        .params(concat!("start, end, apex: [x,y] or point refs; rho?: 0…1 (default 0.5: parabola)", created_doc!())),
    CommandSpec::new("sketch.blend_curve", "Blend Curve", blend)
        .at("SKETCH", "MODIFY")
        .icon("blend")
        .enabled(in_sketch)
        .params(concat!(
            "a, b: end points of two curves (\"l1.end\", \"a2.start\"), continuity?: g1|g2|g3|g4 (default g1: a tangent spline; g2…g4 match the reference curves' curvature, curvature rate and 4th derivative as well, as one Bézier of degree 5, 7 or 9)",
            created_doc!()
        )),
    CommandSpec::new("sketch.centerline", "Centerline", centerline)
        .at("SKETCH", "CREATE")
        .icon("centerline")
        .enabled(in_sketch)
        .params("curves: [line ids], value?: bool (default toggles): centerline line type (not in profiles; an axis)"),
    CommandSpec::new("sketch.infinite", "Infinite Line", infinite_line).icon("infinite")
        .enabled(in_sketch)
        .params("curves: [line ids], value?: bool (default toggles): infinite line type (#1359): construction geometry drawn across the whole view through the line's two points; off, a normal line"),
    CommandSpec::new("sketch.text", "Text", text).at("SKETCH", "CREATE").icon("sketch_text").enabled(in_sketch).params(concat!(
        "text: string (\\n breaks lines), at: baseline start [x,y], height?: capital height mm (default 5), angle?: deg; at's x and y, height and angle each take a number or an expression over the design's parameters as a string (\"plate_width / 2\", \"label_size\"): the text keeps the expression, follows the parameters, and returns it in `exprs`; an expression naming no parameter is the number it comes to, a bare number in one is in the design's units, \
         font?: an installed font's family or full name, or a .ttf/.otf file path (~ and %VAR% expand; default: the built-in Liberation Sans), ",
        text_style_doc!(),
        created_doc!()
    )),
    CommandSpec::new("sketch.edit_text", "Edit Text", edit_text).icon("edit")
        .enabled(in_sketch)
        .live(edit_text_live)
        .params(concat!(
            "link: the text's link id (or entity: one of its curves), text?, at?, height?, angle?: deg (numbers or expressions as in sketch.text; a number replaces an expression, and so does moving the text), font?: family or file path (empty: the built-in font), ",
            text_style_doc!(),
            " (each kept when left out); live?: bool: one frame of an interactive change (the sketch is solved, the features after it stay as they were until `sketch.drag_end`)",
            created_doc!()
        )),
];

/// The style a command's parameters give, over `old`.
fn text_style(p: &Value, cmd: &str, old: TextStyle) -> Result<TextStyle> {
    let mut style = old;
    for (key, slot) in [("bold", &mut style.bold), ("italic", &mut style.italic), ("reverse", &mut style.reverse)] {
        match p.get(key) {
            None | Some(Value::Null) => {}
            Some(Value::Bool(b)) => *slot = *b,
            Some(_) => return Err(bad(cmd, format!("`{key}` must be true or false"))),
        }
    }
    match p.get("align") {
        None | Some(Value::Null) => {}
        Some(Value::String(a)) => {
            style.align = match a.trim().to_ascii_lowercase().as_str() {
                "left" => TextAlign::Left,
                "center" | "centre" => TextAlign::Center,
                "right" => TextAlign::Right,
                _ => return Err(bad(cmd, "`align` must be left, center or right")),
            }
        }
        Some(_) => return Err(bad(cmd, "`align` must be left, center or right")),
    }
    if p.get("letter_spacing").is_some_and(|v| !v.is_null()) {
        style.letter_spacing =
            num(p, "letter_spacing").filter(|v| v.is_finite()).ok_or_else(|| bad(cmd, "`letter_spacing` must be a length in mm"))?;
    }
    if p.get("line_spacing").is_some_and(|v| !v.is_null()) {
        style.line_spacing =
            num(p, "line_spacing").filter(|v| v.is_finite() && *v > 0.0).ok_or_else(|| bad(cmd, "`line_spacing` must be a positive factor"))?;
    }
    match p.get("path") {
        None | Some(Value::Null) => {}
        Some(Value::String(c)) if c.trim().is_empty() => style.path = None,
        Some(Value::String(c)) => style.path = Some(c.trim().to_string()),
        Some(_) => return Err(bad(cmd, "`path` must be a curve id")),
    }
    Ok(style)
}

/// What a text's `at` must be.
const AT_MUST: &str = "`at` must be the baseline start [x, y], each a number or an expression";

/// One number of a text, from a command's parameter `key`. A number is that number (mm, or
/// degrees for the angle). A string is an expression over the design's parameters: a bare number
/// in it is in the design's units (degrees for the angle), and it is kept as an expression when
/// it names a parameter, so that the text follows it; one that names none is the number it comes
/// to. Returns the value (mm or radians) and the expression kept. Anything else is refused with
/// `must`.
fn text_number(doc: &solidcraft_doc::Document, v: &Value, cmd: &str, key: &str, must: &str, angle: bool) -> Result<(f64, Option<String>)> {
    use solidcraft_doc::expr::{self, Kind};
    let kind = if angle { Kind::Angle } else { Kind::Length };
    match v {
        Value::Number(n) => {
            let x = n.as_f64().and_then(crate::params::finite).ok_or_else(|| bad(cmd, must))?;
            Ok((if angle { x.to_radians() } else { x }, None))
        }
        Value::String(e) if !e.trim().is_empty() && e.len() <= 4096 => {
            let e = doc.with_design_unit(e.trim(), kind);
            match doc.eval(&e, kind) {
                Ok(x) => {
                    let x = crate::params::finite(x).ok_or_else(|| bad(cmd, must))?;
                    Ok((x, (!expr::references(&e).is_empty()).then_some(e)))
                }
                // Say what is wrong with it (an unknown parameter, a length where an angle goes).
                Err(error) => Err(bad(cmd, format!("{key}: {error}"))),
            }
        }
        _ => Err(bad(cmd, must)),
    }
}

/// A text's values as `text_source` works on them: text, place, height, angle (radians), font,
/// style and the expressions behind the numbers.
type TextValues = (String, Vec2, f64, f64, Option<String>, TextStyle, TextExprs);

/// A text's source from a command's parameters: what is given replaces what `old` has (the
/// defaults, for a new text); what is left out stays, an expression with its number. A number
/// replaces an expression. An input that is not understood is refused, not ignored.
fn text_source(doc: &solidcraft_doc::Document, p: &Value, cmd: &str, old: Option<TextValues>) -> Result<LinkSource> {
    let (t0, mut at, mut height, mut angle, f0, s0, mut exprs) =
        old.unwrap_or((String::new(), Vec2::ZERO, 5.0, 0.0, None, TextStyle::default(), TextExprs::default()));
    let text = match p.get("text") {
        None | Some(Value::Null) => t0,
        Some(Value::String(t)) => t.clone(),
        Some(_) => return Err(bad(cmd, "`text` must be a string")),
    };
    if text.trim().is_empty() {
        return Err(bad(cmd, "`text` must not be empty"));
    }
    if text.chars().count() > solidcraft_sketch::MAX_TEXT_CHARS {
        return Err(bad(cmd, "text too long"));
    }
    match p.get("at") {
        None | Some(Value::Null) => {}
        Some(Value::Array(a)) => {
            let [x, y] = a.as_slice() else { return Err(bad(cmd, AT_MUST)) };
            (at.x, exprs.x) = text_number(doc, x, cmd, "`at` x", AT_MUST, false)?;
            (at.y, exprs.y) = text_number(doc, y, cmd, "`at` y", AT_MUST, false)?;
        }
        Some(v) => {
            at = crate::params::vec2(v).ok_or_else(|| bad(cmd, AT_MUST))?;
            (exprs.x, exprs.y) = (None, None);
        }
    }
    if let Some(v) = p.get("height").filter(|v| !v.is_null()) {
        (height, exprs.height) = text_number(doc, v, cmd, "`height`", "`height` must be a length in mm, or an expression", false)?;
    }
    if !(height > 1e-6 && height < 1e5) {
        return Err(bad(cmd, "`height` must be positive"));
    }
    if let Some(v) = p.get("angle").filter(|v| !v.is_null()) {
        (angle, exprs.angle) = text_number(doc, v, cmd, "`angle`", "`angle` must be an angle in degrees, or an expression", true)?;
    }
    let font = match p.get("font") {
        None | Some(Value::Null) => f0,
        Some(Value::String(f)) if f.trim().is_empty() => None,
        Some(Value::String(f)) => Some(f.trim().to_string()),
        Some(_) => return Err(bad(cmd, "`font` must be a font family name or a font file path")),
    };
    let style = text_style(p, cmd, s0)?;
    // An unknown font, an unreadable file or a style the font lacks is an error now, not a lost
    // link later; so is a font file outside an agent's folders (the font reader asks `vfs`).
    solidcraft_sketch::font_data_styled(font.as_deref(), style.bold, style.italic).map_err(|e| bad(cmd, e.to_string()))?;
    Ok(LinkSource::Text { text, at, height, angle, font, style, exprs })
}

/// The expressions behind a text's numbers, for a command's reply (`{"height": "plate_h / 10"}`).
fn text_exprs_json(src: &LinkSource) -> Option<Value> {
    let LinkSource::Text { exprs, .. } = src else { return None };
    let mut out = serde_json::Map::new();
    for (key, e) in [("x", &exprs.x), ("y", &exprs.y), ("height", &exprs.height), ("angle", &exprs.angle)] {
        if let Some(e) = e {
            out.insert(key.into(), json!(e));
        }
    }
    (!out.is_empty()).then_some(Value::Object(out))
}

fn text(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.text";
    if p.get("at").is_none_or(Value::is_null) {
        return Err(bad(cmd, AT_MUST));
    }
    let src = text_source(&s.doc, p, cmd, None)?;
    let id = s.active_sketch.ok_or_else(|| bad(cmd, "no sketch is being edited"))?;
    let mut doc = (*s.doc).clone();
    let mut sk = doc.sketch(id)?.clone();
    // A text needs nothing outside its sketch (a path is one of its curves).
    let geom = sk.text_source_geometry(&src).map_err(|e| bad(cmd, e.to_string()))?;
    if geom.is_empty() {
        return Err(bad(cmd, "the text makes no outline"));
    }
    let guide = guide(&mut sk, &src);
    let exprs = text_exprs_json(&src);
    let l = sk.add_link(LinkKind::Text, src, &geom)?;
    let curves = ids_of(&sk, &sk.link_curves(&l));
    *doc.sketch_mut(id)? = sk;
    *s.doc_mut() = doc;
    let mut out = json!({"link": l, "curves": curves, "guide": guide});
    if let Some(exprs) = exprs {
        out["exprs"] = exprs;
    }
    Ok(out)
}

/// A text's path is its guide: the curve it follows becomes construction. Left as it was, it
/// would cut the letters standing on it (and the ones hanging below it) into pieces, each
/// profile a piece of a letter and of the curve, and picking the text would pick those pieces.
/// Taking the text off the curve leaves the curve as it is. The curve made construction, if any.
fn guide(sk: &mut Sketch, src: &LinkSource) -> Option<String> {
    let LinkSource::Text { style, .. } = src else { return None };
    let id = style.path.as_ref()?;
    let curve = sk.curve_index(id).and_then(|c| sk.curves.get_mut(c))?;
    if curve.construction {
        return None;
    }
    curve.construction = true;
    curve.centerline = false;
    Some(id.clone())
}

/// The text source owning an outline or one of its linked points.
pub(super) fn text_link<'a>(sk: &'a Sketch, entity: &str) -> Option<&'a str> {
    let link = sk
        .curve_index(entity)
        .and_then(|i| sk.curves.get(i))
        .and_then(|c| c.link.as_deref())
        .or_else(|| sk.resolve_point(entity).and_then(|i| sk.points.get(i)).and_then(|p| p.link.as_deref()))?;
    sk.link(link).filter(|source| source.kind == LinkKind::Text).map(|source| source.id.as_str())
}

/// Transform a text source as a unit; generated outlines remain linked and editable.
pub(super) fn move_text(sk: &mut Sketch, link: &str, translate: Vec2, rotation: f64, center: Vec2, copy: bool) -> Result<Vec<usize>> {
    let Some(LinkSource::Text { text, at, height, angle, font, style, mut exprs }) = sk.link(link).map(|l| l.source.clone()) else {
        return Err(bad("sketch.move", "the selected source is not text"));
    };
    if style.path.is_some() {
        return Err(bad("sketch.move", "a text on a path follows its curve: move the curve, or take the text off it"));
    }
    let q = at - center;
    let (sin, cos) = rotation.sin_cos();
    let moved = center + Vec2::new(q.x * cos - q.y * sin, q.x * sin + q.y * cos) + translate;
    // What the move changes is a number now, not the expression it was; the rest stays.
    if (moved.x - at.x).abs() > 1e-12 {
        exprs.x = None;
    }
    if (moved.y - at.y).abs() > 1e-12 {
        exprs.y = None;
    }
    if rotation.abs() > 1e-12 {
        exprs.angle = None;
    }
    let (at, angle) = (moved, angle + rotation);
    let geom = solidcraft_sketch::text_geometry_styled(font.as_deref(), &text, at, height, angle, &style, None)?;
    let source = LinkSource::Text { text, at, height, angle, font, style, exprs };
    let link = if copy {
        sk.add_link(LinkKind::Text, source, &geom)?
    } else {
        sk.set_link_source(link, source);
        if sk.update_link(link, &geom)? {
            return Err(bad("sketch.move", "moving text must preserve its outline topology; edit the text source first"));
        }
        link.to_owned()
    };
    Ok(sk.link_curves(&link))
}

fn edit_text(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.edit_text";
    let id = s.active_sketch.ok_or_else(|| bad(cmd, "no sketch is being edited"))?;
    let mut doc = (*s.doc).clone();
    let mut sk = doc.sketch(id)?.clone();
    // The numbers its expressions give now (a parameter may have changed since the sketch was
    // last stored); one that cannot be evaluated is left as it was, to be replaced by this edit.
    let _ = doc.apply_text_values(&doc.param_values().0, &mut sk);
    let out = apply_text_edit(&mut sk, &doc, p)?;
    *doc.sketch_mut(id)? = sk;
    *s.doc_mut() = doc;
    Ok(out)
}

/// One frame of an interactive resize or rotation: the sketch is solved and shown, the
/// features after it keep their last result until `sketch.drag_end` evaluates the model.
fn edit_text_live(s: &mut Session, p: &Value) -> Result<(Value, Option<crate::LiveEdit>)> {
    let cmd = "sketch.edit_text";
    let id = s.active_sketch.ok_or_else(|| bad(cmd, "no sketch is being edited"))?;
    let (out, report) = super::sketch::edit_solved(s, id, false, |sk, doc| Ok((apply_text_edit(sk, doc, p)?, super::sketch::Left::Unsolved)))?;
    Ok((out, report.map(|report| crate::LiveEdit { sketch: id, report })))
}

/// `sketch.edit_text`'s change to its sketch: the text source's new values and its outlines
/// regenerated from them.
fn apply_text_edit(sk: &mut Sketch, doc: &solidcraft_doc::Document, p: &Value) -> Result<Value> {
    let cmd = "sketch.edit_text";
    let l = match (str_(p, "link"), str_(p, "entity")) {
        (Some(l), _) => l.to_string(),
        (None, Some(e)) => sk.curves.iter().find(|c| c.id == e).and_then(|c| c.link.clone()).ok_or_else(|| bad(cmd, format!("`{e}` is not text")))?,
        _ => return Err(bad(cmd, "give `link` or `entity`")),
    };
    let old = match sk.link(&l).map(|x| (x.kind, x.source.clone())) {
        Some((LinkKind::Text, LinkSource::Text { text, at, height, angle, font, style, exprs })) => (text, at, height, angle, font, style, exprs),
        _ => return Err(bad(cmd, format!("`{l}` is not text"))),
    };
    let src = text_source(doc, p, cmd, Some(old))?;
    let geom = sk.text_source_geometry(&src).map_err(|e| bad(cmd, e.to_string()))?;
    if geom.is_empty() {
        return Err(bad(cmd, "the text makes no outline"));
    }
    let guide = guide(sk, &src);
    let exprs = text_exprs_json(&src);
    sk.set_link_source(&l, src.clone());
    sk.update_link(&l, &geom)?;
    let curves = ids_of(sk, &sk.link_curves(&l));
    let mut out = json!({"link": l, "curves": curves, "guide": guide});
    if let Some(exprs) = exprs {
        out["exprs"] = exprs;
    }
    Ok(out)
}

/// The curve ending at point `pi` and the direction leaving it there.
fn end_of(sk: &Sketch, pi: usize) -> Option<(usize, Vec2)> {
    sk.curves.iter().enumerate().filter(|(_, c)| !c.construction).find_map(|(i, _)| leaving_dir(sk, i, pi).map(|d| (i, d)))
}

/// The continuity a blend asks for: the number of derivatives of each reference curve it
/// matches at both joints (g1 is the tangent construction the tool has always made).
fn blend_order(p: &Value, cmd: &str) -> Result<usize> {
    match p.get("continuity") {
        None | Some(Value::Null) => Ok(1),
        Some(Value::String(v)) => match v.trim().to_ascii_lowercase().as_str() {
            "" | "g1" => Ok(1),
            "g2" => Ok(2),
            "g3" => Ok(3),
            "g4" => Ok(4),
            _ => Err(bad(cmd, "`continuity` must be g1, g2, g3 or g4")),
        },
        Some(_) => Err(bad(cmd, "`continuity` must be g1, g2, g3 or g4")),
    }
}

/// Negate the odd-order derivatives: how a curve's derivatives change when it is read backwards.
fn flip_odd(d: &mut [Vec2]) {
    for (i, v) in d.iter_mut().enumerate() {
        if i % 2 == 0 {
            *v = -*v;
        }
    }
}

/// Derivatives 1…`order` of curve `ci`'s continuation through its end point `pi`, in the
/// parameterisation the blend needs at that joint: at the blend's start (`leaving`) the curve's
/// continuation runs away from it, at the blend's end the blend runs into the curve instead, which
/// is the same curve read backwards. The derivatives are those of the one exact piece that ends
/// at `pi`, evaluated at that piece's own end, so an arc gives its circular-motion terms (zero
/// curvature rate included) and a conic the quotient rule's terms. Errors when the curve has no
/// exact piece, when a derivative is not finite (a corrupt or degenerate reference), or when it
/// has no tangent at all: no blend can meet the last case.
fn joint_derivatives(sk: &Sketch, ci: usize, pi: usize, order: usize, leaving: bool, what: &str, cmd: &str) -> Result<Vec<Vec2>> {
    let id = sk.curves.get(ci).map(|c| c.id.as_str()).unwrap_or("?");
    let (start, end) = sk.curves.get(ci).and_then(|c| c.kind.ends()).ok_or_else(|| bad(cmd, format!("`{id}` has no end points")))?;
    let at_end = end == pi;
    if !at_end && start != pi {
        return Err(bad(cmd, format!("`{what}` is not an end point of `{id}`")));
    }
    let pieces = sk.exact_pieces(ci);
    let seg = if at_end { pieces.last() } else { pieces.first() }.ok_or_else(|| bad(cmd, format!("`{id}` has no exact geometry")))?;
    let mut d = seg
        .derivatives(if at_end { 1.0 } else { 0.0 }, order)
        .ok_or_else(|| bad(cmd, format!("`{id}` has no finite derivatives up to order {order} at `{what}`")))?;
    // Reading the piece against its own direction, or reading the continuation into the curve
    // rather than out of it, each reverse the parameterisation once and so flip the odd
    // derivatives: the blend's start wants the continuation away from the reference's end (no
    // flip) or backwards out of its start (flip); its end wants the mirror image of that, so a
    // joint needs a flip exactly when its two readings disagree.
    if at_end != leaving {
        flip_odd(&mut d);
    }
    if !d.first().is_some_and(|t| t.len() > 0.0) {
        return Err(bad(cmd, format!("`{id}` has no tangent at `{what}`")));
    }
    Ok(d)
}

fn blend(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.blend_curve";
    let order = blend_order(p, cmd)?;
    let (out, info) = edit(s, p, cmd, false, |sk, _| {
        let pa = req_parg(sk, p, "a", cmd)?.idx().ok_or_else(|| bad(cmd, "`a` must be the end point of a curve"))?;
        let pb = req_parg(sk, p, "b", cmd)?.idx().ok_or_else(|| bad(cmd, "`b` must be the end point of a curve"))?;
        let (ca, ta) = end_of(sk, pa).ok_or_else(|| bad(cmd, "no curve ends at `a`"))?;
        let (cb, tb) = end_of(sk, pb).ok_or_else(|| bad(cmd, "no curve ends at `b`"))?;
        let (qa, qb) = (sk.point(pa).unwrap_or_default(), sk.point(pb).unwrap_or_default());
        let l = qa.dist(qb);
        if l < 1e-9 || ca == cb && pa == pb {
            return Err(bad(cmd, "the ends must differ"));
        }
        let first_new = sk.points.len();
        let (sp, cons) = if order <= 1 {
            let c1 = sk.add_point(qa + ta * (l / 3.0), None)?;
            let c2 = sk.add_point(qb + tb * (l / 3.0), None)?;
            let sp = sk.add_curve(CurveKind::Spline { pts: vec![pa, c1, c2, pb], control: true, degree: 3, periodic: false }, None)?;
            let cons = vec![add_c(sk, ConstraintKind::Tangent { a: ca, b: sp })?, add_c(sk, ConstraintKind::Tangent { a: cb, b: sp })?];
            (sp, cons)
        } else {
            // One Bézier of degree 2·order + 1 whose controls are the Hermite interpolation of the
            // two references' derivatives, so its 1st…order-th derivatives are theirs at both
            // joints. Tangency comes from the construction, so it needs no tangent constraints.
            let da = joint_derivatives(sk, ca, pa, order, true, "a", cmd)?;
            let db = joint_derivatives(sk, cb, pb, order, false, "b", cmd)?;
            let mut pts: Vec<usize> = Vec::with_capacity(2 * order + 2);
            let cps = solidcraft_geom::hermite_bezier(qa, qb, &da, &db)
                .ok_or_else(|| bad(cmd, "the blend curve could not be built from the derivatives of `a` and `b`"))?;
            for (i, q) in cps.iter().enumerate() {
                pts.push(if i == 0 {
                    pa
                } else if i + 1 == cps.len() {
                    pb
                } else {
                    sk.add_point(*q, None)?
                });
            }
            let degree = u8::try_from(cps.len().saturating_sub(1)).map_err(|_| bad(cmd, "the blend curve needs too many control points"))?;
            let sp = sk.add_curve(CurveKind::Spline { pts, control: true, degree, periodic: false }, None)?;
            (sp, Vec::new())
        };
        solve_new_only(sk, first_new);
        Ok((ids_of(sk, &[sp]), cons))
    })?;
    Ok(result(out, info))
}

fn centerline(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.centerline";
    let ids = string_list(p, "curves");
    if ids.is_empty() {
        return Err(bad(cmd, "`curves` must list lines"));
    }
    let want = crate::params::bool_(p, "value");
    let (n, info) = edit(s, p, cmd, false, |sk, _| {
        for id in &ids {
            let c = sk.curve_index(id).ok_or_else(|| bad(cmd, format!("unknown curve `{id}`")))?;
            let cu = sk.curves.get_mut(c).ok_or_else(|| bad(cmd, "curve"))?;
            if !matches!(cu.kind, CurveKind::Line { .. }) {
                return Err(bad(cmd, format!("`{id}` is not a line")));
            }
            cu.centerline = want.unwrap_or(!cu.centerline);
            if cu.centerline {
                cu.construction = false;
                cu.infinite = false;
            }
        }
        Ok(ids.len())
    })?;
    Ok(json!({"changed": n, "sketch": info}))
}

fn infinite_line(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.infinite";
    let ids = string_list(p, "curves");
    if ids.is_empty() {
        return Err(bad(cmd, "`curves` must list lines"));
    }
    let want = crate::params::bool_(p, "value");
    let (n, info) = edit(s, p, cmd, false, |sk, _| {
        for id in &ids {
            let c = sk.curve_index(id).ok_or_else(|| bad(cmd, format!("unknown curve `{id}`")))?;
            let cu = sk.curves.get_mut(c).ok_or_else(|| bad(cmd, "curve"))?;
            if !matches!(cu.kind, CurveKind::Line { .. }) {
                return Err(bad(cmd, format!("`{id}` is not a line")));
            }
            // Infinite lines are construction geometry (never in a profile); off, a normal line.
            cu.infinite = want.unwrap_or(!cu.infinite);
            cu.construction = cu.infinite;
            cu.centerline = false;
        }
        Ok(ids.len())
    })?;
    Ok(json!({"changed": n, "sketch": info}))
}

fn ellipse(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.ellipse";
    let (c, m) = (req_vec2(cmd, p, "center")?, req_vec2(cmd, p, "major")?);
    let u = (m - c).normalized().ok_or_else(|| bad(cmd, "the major axis end must differ from the centre"))?;
    let a = c.dist(m);
    let r = match (num(p, "minor_radius"), p.get("minor").and_then(crate::params::vec2)) {
        (Some(r), _) => r,
        (None, Some(q)) => {
            // Minor radius so the ellipse passes through q.
            let d = q - c;
            let (x, y) = (d.dot(u), d.cross(u).abs());
            let k = 1.0 - (x / a).powi(2);
            if k <= 1e-12 {
                return Err(bad(cmd, "the minor point must lie within the major axis span"));
            }
            y / k.sqrt()
        }
        _ => return Err(bad(cmd, "needs `minor` [x,y] or `minor_radius`")),
    };
    if !(r > 1e-9 && r < 1e8) {
        return Err(bad(cmd, "the minor radius must be positive"));
    }
    let (out, info) = edit(s, p, cmd, false, |sk, _| {
        let ci = sk.add_point(c, None)?;
        let mi = sk.add_point(m, None)?;
        let e = sk.add_curve(CurveKind::Ellipse { c: ci, m: mi, r }, None)?;
        mark_construction(sk, &[e], p);
        Ok((ids_of(sk, &[e]), Vec::new()))
    })?;
    Ok(result(out, info))
}

fn point_list(sk: &mut Sketch, p: &Value, cmd: &str) -> Result<Vec<usize>> {
    let list = p.get("points").and_then(Value::as_array).ok_or_else(|| bad(cmd, "`points` must be a list"))?;
    if list.len() < 2 || list.len() > solidcraft_sketch::MAX_SPLINE_POINTS {
        return Err(bad(cmd, "a spline needs 2 to 500 points"));
    }
    let mut out = Vec::new();
    for v in list {
        let a = super::sketch::parg(sk, v, cmd)?;
        let i = match a.idx() {
            Some(i) => i,
            None => sk.add_point(a.pos(), None)?,
        };
        if out.last() == Some(&i) || out.last().and_then(|l| sk.point(*l)).is_some_and(|q| q.dist(a.pos()) < 1e-9) {
            return Err(bad(cmd, "consecutive points must differ"));
        }
        out.push(i);
    }
    Ok(out)
}

fn spline(s: &mut Session, p: &Value, cmd: &str, control: bool, degree: u8) -> Result<Value> {
    let periodic = match p.get("periodic") {
        None => false,
        Some(Value::Bool(v)) => *v,
        _ => return Err(bad(cmd, "periodic must be boolean")),
    };
    if periodic && (control || degree != 3) {
        return Err(bad(cmd, "periodic mode requires a cubic fit spline"));
    }
    let p = &super::sketch_project::snap_points(s, p, cmd)?;
    let (out, info) = edit(s, p, cmd, false, |sk, _| {
        let mut pts = point_list(sk, p, cmd)?;
        if periodic
            && sk.point(*pts.first().ok_or_else(|| bad(cmd, "missing first point"))?)
                != sk.point(*pts.last().ok_or_else(|| bad(cmd, "missing last point"))?)
        {
            pts.push(*pts.first().ok_or_else(|| bad(cmd, "missing first point"))?);
        }
        let c = sk.add_curve(CurveKind::Spline { pts, control, degree, periodic }, None)?;
        mark_construction(sk, &[c], p);
        Ok((ids_of(sk, &[c]), Vec::new()))
    })?;
    Ok(result(out, info))
}

fn spline_fit(s: &mut Session, p: &Value) -> Result<Value> {
    spline(s, p, "sketch.spline.fit_point", false, 3)
}
fn spline_cv3(s: &mut Session, p: &Value) -> Result<Value> {
    spline(s, p, "sketch.spline.control_point", true, 3)
}
fn spline_cv5(s: &mut Session, p: &Value) -> Result<Value> {
    spline(s, p, "sketch.spline.control_point_5", true, 5)
}

fn conic(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.conic";
    let rho = num(p, "rho").unwrap_or(0.5);
    if !(rho > 1e-6 && rho < 1.0 - 1e-6) {
        return Err(bad(cmd, "`rho` must be between 0 and 1"));
    }
    let p = &super::sketch_project::snap_points(s, p, cmd)?;
    let (out, info) = edit(s, p, cmd, false, |sk, _| {
        let mut idx = Vec::new();
        for k in ["start", "end", "apex"] {
            let a = req_parg(sk, p, k, cmd)?;
            idx.push(match a.idx() {
                Some(i) => i,
                None => sk.add_point(a.pos(), None)?,
            });
        }
        let (a, b, x) = (idx[0], idx[1], idx[2]);
        if a == b || a == x || b == x {
            return Err(bad(cmd, "start, end and apex must differ"));
        }
        let c = sk.add_curve(CurveKind::Conic { a, b, apex: x, rho }, None)?;
        mark_construction(sk, &[c], p);
        Ok((ids_of(sk, &[c]), Vec::new()))
    })?;
    Ok(result(out, info))
}

fn midpoint_line(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.line.midpoint";
    let end = req_vec2(cmd, p, "end")?;
    let (out, info) = edit(s, p, cmd, false, |sk, _| {
        let m = req_parg(sk, p, "mid", cmd)?;
        let start = m.pos() * 2.0 - end;
        if start.dist(end) < 1e-9 {
            return Err(bad(cmd, "the end must differ from the midpoint"));
        }
        let l = sk.add_line(start, end, None, None, None)?;
        let mp = match m.idx() {
            Some(i) => i,
            None => sk.add_point(m.pos(), None)?,
        };
        let k = add_c(sk, ConstraintKind::Midpoint { p: mp, l })?;
        mark_construction(sk, &[l], p);
        Ok((ids_of(sk, &[l]), vec![k]))
    })?;
    Ok(result(out, info))
}

/// Direction leaving curve `ci` at its end point `pi` (continuing the curve past that end).
fn leaving_dir(sk: &Sketch, ci: usize, pi: usize) -> Option<Vec2> {
    match sk.curves.get(ci)?.kind {
        CurveKind::Line { a, b } => {
            let (pa, pb) = (sk.point(a)?, sk.point(b)?);
            if pi == b {
                (pb - pa).normalized()
            } else if pi == a {
                (pa - pb).normalized()
            } else {
                None
            }
        }
        CurveKind::Arc { c, a, b } => {
            let (pc, q) = (sk.point(c)?, sk.point(pi)?);
            let ccw = (q - pc).perp().normalized()?;
            if pi == b {
                Some(ccw)
            } else if pi == a {
                Some(-ccw)
            } else {
                None
            }
        }
        CurveKind::Spline { ref pts, control, periodic, .. } => {
            let q: Vec<Vec2> = pts.iter().filter_map(|i| sk.point(*i)).collect();
            if pts.last() == Some(&pi) {
                solidcraft_sketch::spline_end_tangent_mode(&q, control, periodic, true)
            } else if pts.first() == Some(&pi) {
                solidcraft_sketch::spline_end_tangent_mode(&q, control, periodic, false).map(|t| -t)
            } else {
                None
            }
        }
        CurveKind::Conic { a, b, apex, .. } => {
            let x = sk.point(apex)?;
            if pi == b {
                (sk.point(b)? - x).normalized()
            } else if pi == a {
                (sk.point(a)? - x).normalized()
            } else {
                None
            }
        }
        _ => None,
    }
}

fn arc_tangent(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.arc.tangent";
    let end = req_vec2(cmd, p, "end")?;
    let (out, info) = edit(s, p, cmd, false, |sk, _| {
        let st = req_parg(sk, p, "start", cmd)?;
        let picked = st.idx().ok_or_else(|| bad(cmd, "`start` must be the end point of a line or arc"))?;
        // The picked point, then any other point at the same place: a trimmed arc's end is its own
        // point, lying on the end of the curve that trimmed it, which is the one a click picks (#1485).
        let at = st.pos();
        let tol = 1e-6 * at.len().max(1.0);
        let candidates: Vec<usize> =
            std::iter::once(picked).chain((0..sk.points.len()).filter(|&q| q != picked && sk.point(q).is_some_and(|x| x.dist(at) <= tol))).collect();
        let (ci, pi) = match str_(p, "curve") {
            Some(id) => {
                let ci = sk.curve_index(id).ok_or_else(|| bad(cmd, format!("unknown curve `{id}`")))?;
                let pi = candidates
                    .iter()
                    .copied()
                    .find(|&q| leaving_dir(sk, ci, q).is_some())
                    .ok_or_else(|| bad(cmd, "`start` is not an end of that curve"))?;
                (ci, pi)
            }
            None => candidates
                .iter()
                .find_map(|&q| sk.curves.iter().enumerate().find(|(i, c)| !c.construction && leaving_dir(sk, *i, q).is_some()).map(|(i, _)| (i, q)))
                .ok_or_else(|| bad(cmd, "no line or arc ends at `start`"))?,
        };
        let t = leaving_dir(sk, ci, pi).ok_or_else(|| bad(cmd, "`start` is not an end of that curve"))?;
        let s0 = st.pos();
        let n = t.perp();
        let d = end - s0;
        let den = 2.0 * n.dot(d);
        if den.abs() < 1e-9 * d.len().max(1e-9) || d.len() < 1e-9 {
            return Err(bad(cmd, "the end is straight ahead: draw a line instead"));
        }
        let center = s0 + n * (d.len2() / den);
        let a = if den > 0.0 {
            sk.add_arc(center, s0, end, [None, Some(pi), None], None)?
        } else {
            let a = sk.add_arc(center, end, s0, [None, None, Some(pi)], None)?;
            if let Some(c) = sk.curves.get_mut(a) {
                c.reversed = true;
            }
            a
        };
        let k = add_c(sk, ConstraintKind::Tangent { a: ci, b: a })?;
        Ok((ids_of(sk, &[a]), vec![k]))
    })?;
    Ok(result(out, info))
}

/// Initial radius for a circle near `at` touching the curves: the mean distance to them.
fn guess_radius(sk: &Sketch, curves: &[usize], at: Vec2) -> f64 {
    let ds: Vec<f64> = curves.iter().filter_map(|c| sk.shape(*c)).map(|sh| sh.project(at).dist(at)).collect();
    let r = ds.iter().sum::<f64>() / ds.len().max(1) as f64;
    if r.is_finite() && r > 1e-6 { r } else { 1.0 }
}

fn tangent_curves(sk: &Sketch, p: &Value, cmd: &str, n: usize) -> Result<Vec<usize>> {
    let ids = string_list(p, "curves");
    if ids.len() != n {
        return Err(bad(cmd, format!("`curves` must list {n} curves")));
    }
    ids.iter().map(|id| sk.curve_index(id).ok_or_else(|| bad(cmd, format!("unknown curve `{id}`")))).collect()
}

/// A circle tangent to `curves`, solved from a start near `near`.
fn tangent_circle(s: &mut Session, p: &Value, cmd: &str, n: usize, radius: Option<String>) -> Result<Value> {
    let near = req_vec2(cmd, p, "near")?;
    let (out, info) = edit(s, p, cmd, true, |sk, doc| {
        let cs = tangent_curves(sk, p, cmd, n)?;
        let r0 = match &radius {
            Some(e) => doc.eval(e, solidcraft_doc::expr::Kind::Length).map_err(|e| bad(cmd, format!("radius: {e}")))?,
            None => guess_radius(sk, &cs, near),
        };
        if !(r0 > 1e-9 && r0 < 1e8) {
            return Err(bad(cmd, "radius must be positive"));
        }
        // Start where the circle already nearly touches: move the centre so the guessed circle
        // sits between the curves.
        let first_new = sk.points.len();
        let c = sk.add_circle(near, r0, None, None)?;
        let mut cons = Vec::new();
        for k in &cs {
            cons.push(add_c(sk, ConstraintKind::Tangent { a: *k, b: c })?);
        }
        if let Some(e) = &radius {
            let pname = doc.new_model_param(e, "mm");
            let k = sk.add_constraint(ConstraintKind::Radius { c, value: r0 }, Some(pname))?;
            cons.push(k);
        }
        solve_new_only(sk, first_new);
        Ok((ids_of(sk, &[c]), cons))
    })?;
    if info["solved"] != json!(true) {
        return Err(bad(cmd, "no tangent circle there"));
    }
    Ok(result(out, info))
}

fn circle_tan_tan(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.circle.two_tangent";
    let r = match (crate::params::expr(p, "radius"), num(p, "diameter")) {
        (Some(r), _) => r.into_string(),
        (None, Some(d)) => format!("{}", d / 2.0),
        _ => return Err(bad(cmd, "needs `radius` or `diameter`")),
    };
    tangent_circle(s, p, cmd, 2, Some(r))
}

fn circle_three_tan(s: &mut Session, p: &Value) -> Result<Value> {
    tangent_circle(s, p, "sketch.circle.three_tangent", 3, None)
}

fn width(p: &Value, cmd: &str) -> Result<f64> {
    num(p, "width").filter(|w| *w > 1e-9).ok_or_else(|| bad(cmd, "`width` must be positive"))
}

fn slot_center_point(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.slot.center_point";
    let (c, e) = (req_vec2(cmd, p, "center")?, req_vec2(cmd, p, "end")?);
    let w = width(p, cmd)?;
    slot(s, p, cmd, c * 2.0 - e, e, w)
}

/// Arc slot around `o` (centre-line radius `r`) from angle `t0` sweeping `sweep` (> 0).
fn arc_slot(s: &mut Session, p: &Value, cmd: &str, o: Vec2, r: f64, t0: f64, sweep: f64, w: f64) -> Result<Value> {
    let h = w / 2.0;
    if h >= r {
        return Err(bad(cmd, "the slot is wider than its centre arc's diameter"));
    }
    if !(sweep > 1e-6 && sweep < std::f64::consts::TAU - 1e-6) {
        return Err(bad(cmd, "the centre arc must have a length"));
    }
    let t1 = t0 + sweep;
    let at = |t: f64, rr: f64| o + Vec2::from_angle(t) * rr;
    let (out, info) = edit(s, p, cmd, false, |sk, _| {
        let oc = sk.add_point(o, None)?;
        let ps = sk.add_point(at(t0, r), None)?;
        let pe = sk.add_point(at(t1, r), None)?;
        let (os, is) = (sk.add_point(at(t0, r + h), None)?, sk.add_point(at(t0, r - h), None)?);
        let (oe, ie) = (sk.add_point(at(t1, r + h), None)?, sk.add_point(at(t1, r - h), None)?);
        let outer = sk.add_arc(o, at(t0, r + h), at(t1, r + h), [Some(oc), Some(os), Some(oe)], None)?;
        let inner = sk.add_arc(o, at(t0, r - h), at(t1, r - h), [Some(oc), Some(is), Some(ie)], None)?;
        let cap_e = sk.add_arc(at(t1, r), at(t1, r + h), at(t1, r - h), [Some(pe), Some(oe), Some(ie)], None)?;
        let cap_s = sk.add_arc(at(t0, r), at(t0, r - h), at(t0, r + h), [Some(ps), Some(is), Some(os)], None)?;
        let centre = sk.add_arc(o, at(t0, r), at(t1, r), [Some(oc), Some(ps), Some(pe)], None)?;
        if let Some(c) = sk.curves.get_mut(centre) {
            c.construction = true;
        }
        let mut cons = Vec::new();
        for (a, b) in [(outer, cap_e), (inner, cap_e), (outer, cap_s), (inner, cap_s)] {
            cons.push(add_c(sk, ConstraintKind::Tangent { a, b })?);
        }
        cons.push(add_c(sk, ConstraintKind::Equal { a: cap_s, b: cap_e })?);
        Ok((ids_of(sk, &[outer, cap_e, inner, cap_s]), cons))
    })?;
    Ok(result(out, info))
}

fn arc_slot_three(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.slot.arc_three_point";
    let (a, t, b) = (req_vec2(cmd, p, "start")?, req_vec2(cmd, p, "through")?, req_vec2(cmd, p, "end")?);
    let w = width(p, cmd)?;
    let (o, r) = circumcircle(a, t, b).ok_or_else(|| bad(cmd, "the points are collinear"))?;
    let ang = |q: Vec2| (q - o).angle();
    let span = |from: f64, to: f64| (to - from).rem_euclid(std::f64::consts::TAU);
    // Counter-clockwise from start must pass the through point; otherwise go from the end.
    let (t0, sweep) = if span(ang(a), ang(t)) < span(ang(a), ang(b)) { (ang(a), span(ang(a), ang(b))) } else { (ang(b), span(ang(b), ang(a))) };
    arc_slot(s, p, cmd, o, r, t0, sweep, w)
}

fn arc_slot_center(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.slot.arc_center";
    let (o, a, b) = (req_vec2(cmd, p, "center")?, req_vec2(cmd, p, "start")?, req_vec2(cmd, p, "end")?);
    let w = width(p, cmd)?;
    let r = o.dist(a);
    if r < 1e-9 || o.dist(b) < 1e-9 {
        return Err(bad(cmd, "the points must differ from the centre"));
    }
    let t0 = (a - o).angle();
    let sweep = ((b - o).angle() - t0).rem_euclid(std::f64::consts::TAU);
    arc_slot(s, p, cmd, o, r, t0, sweep, w)
}

/// Solve moving only the points from `first_new` on (the geometry a tool just made), so the
/// existing drawing stays put when the new entities alone can satisfy the constraints. Falls
/// back to leaving everything to the normal solve.
pub(super) fn solve_new_only(sk: &mut Sketch, first_new: usize) {
    let mut trial = sk.clone();
    for (i, p) in trial.points.iter_mut().enumerate() {
        if i < first_new {
            p.fixed = true;
        }
    }
    if solidcraft_sketch::solve(&mut trial).ok() {
        for (p, t) in sk.points.iter_mut().zip(&trial.points).skip(first_new) {
            p.pos = t.pos;
        }
        for (c, t) in sk.curves.iter_mut().zip(&trial.curves) {
            if let (CurveKind::Circle { r, .. }, CurveKind::Circle { r: tr, .. }) = (&mut c.kind, &t.kind) {
                *r = *tr;
            }
        }
    }
}

#[cfg(test)]
mod blend_tests {
    use serde_json::{Value, json};
    use solidcraft_geom::{Seg2, Vec2};
    use solidcraft_sketch::CurveKind;

    use crate::Session;

    fn run(s: &mut Session, id: &str, p: Value) -> Value {
        match s.execute(id, &p) {
            Ok(v) => v,
            Err(e) => panic!("{id} {p}: {e}"),
        }
    }

    fn ids(v: &Value) -> Vec<String> {
        v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default()
    }

    fn new_sketch() -> Session {
        let mut s = Session::default();
        run(&mut s, "sketch.create", json!({"plane": "XY"}));
        s
    }

    fn sketch(s: &Session) -> solidcraft_sketch::Sketch {
        let id = s.active_sketch.unwrap();
        s.model.state().sketch(id).unwrap().sketch.clone()
    }

    fn factorial(n: usize) -> f64 {
        (1..=n).fold(1.0, |a, b| a * b as f64)
    }

    /// The derivatives 1…`order` of exact piece `seg` at its end (`at_end`), from the piece's own
    /// geometry: what a blend must match, computed here independently of `Seg2::derivatives`.
    fn piece_derivatives(seg: &Seg2, at_end: bool, order: usize) -> Vec<Vec2> {
        let t = if at_end { 1.0 } else { 0.0 };
        let p = seg.point_at(t);
        let mut d = match *seg {
            Seg2::Line { a, b } => vec![b - a, Vec2::ZERO, Vec2::ZERO, Vec2::ZERO],
            Seg2::Arc { center, sweep, .. } => {
                let (v, s2) = (seg.derivative(t), sweep * sweep);
                vec![v, (p - center) * -s2, v * -s2, (p - center) * (s2 * s2)]
            }
            Seg2::Cubic { p0, p1, p2, p3 } => {
                let third = (p3 - p2 * 3.0 + p1 * 3.0 - p0) * 6.0;
                if at_end {
                    vec![(p3 - p2) * 3.0, (p3 - p2 * 2.0 + p1) * 6.0, third, Vec2::ZERO]
                } else {
                    vec![(p1 - p0) * 3.0, (p0 - p1 * 2.0 + p2) * 6.0, third, Vec2::ZERO]
                }
            }
            Seg2::Conic { a, apex, b, w } => {
                // The rational quadratic's numerator N and weight D as quadratics in the power
                // basis; dividing N's series by D's (constant term 1 at both ends) has to give the
                // Taylor coefficients of the curve.
                let (n0, n1, n2) = (a, apex * (2.0 * w) - a * 2.0, a - apex * (2.0 * w) + b);
                let (e1, e2) = (2.0 * (w - 1.0), 2.0 * (1.0 - w));
                let h = 1e-3;
                let series = |f: &dyn Fn(f64) -> Vec2| [f(t), (f(t + h) - f(t - h)) / (2.0 * h), (f(t + h) - f(t) * 2.0 + f(t - h)) / (2.0 * h * h)];
                let n = series(&|x| n0 + n1 * x + n2 * (x * x));
                let den = [1.0 + e1 * t + e2 * (t * t), e1 + e2 * (2.0 * t), e2];
                let mut x = vec![n[0] / den[0]];
                for j in 1..=order {
                    let mut v = n.get(j).copied().unwrap_or(Vec2::ZERO);
                    for i in 1..=j {
                        v -= x[j - i] * den.get(i).copied().unwrap_or(0.0);
                    }
                    x.push(v / den[0]);
                }
                x.remove(0);
                x.into_iter().enumerate().map(|(i, v)| v * factorial(i + 1)).collect()
            }
            Seg2::Bezier { .. } => panic!("a test piece is not a high-degree Bézier"),
        };
        d.truncate(order);
        d
    }

    /// The derivatives 1…`order` the blend's own control polygon carries at an end: the j-th
    /// difference of the controls, scaled by `n!/(n − j)!` (the Bézier endpoint identity).
    fn blend_derivatives(ctrl: &[Vec2], at_end: bool, order: usize) -> Vec<Vec2> {
        let c: Vec<Vec2> = if at_end { ctrl.iter().rev().copied().collect() } else { ctrl.to_vec() };
        let n = c.len() - 1;
        let (mut diffs, mut scale) = (c, 1.0);
        let mut out = Vec::new();
        for j in 1..=order {
            diffs = diffs.windows(2).map(|w| w[1] - w[0]).collect();
            scale *= (n + 1 - j) as f64;
            out.push(diffs.first().copied().unwrap_or_default() * scale);
        }
        if at_end {
            for (i, v) in out.iter_mut().enumerate() {
                if i % 2 == 0 {
                    *v = -*v;
                }
            }
        }
        out
    }

    /// The blend curve's control points, its degree and the one exact piece they make.
    fn blend_of(s: &Session, id: &str) -> (Vec<Vec2>, usize, Seg2) {
        let sk = sketch(s);
        let ci = sk.curve_index(id).unwrap();
        let ctrl: Vec<Vec2> = sk.curves[ci].kind.point_ids().iter().map(|i| sk.point(*i).unwrap()).collect();
        let degree = match sk.curves[ci].kind {
            CurveKind::Spline { degree, .. } => usize::from(degree),
            ref other => panic!("{other:?}"),
        };
        let pieces = sk.exact_pieces(ci);
        assert_eq!(pieces.len(), 1, "{pieces:?}");
        (ctrl, degree, pieces[0])
    }

    /// The reference pair the tests build: a line `(0, 0)`–`(6, 0)` ending at `a` and a three-point
    /// control spline starting at `b`, so the blend runs out of the first and into the second with
    /// no reversal at either joint.
    fn two_references(s: &mut Session) -> (String, String, [Vec2; 4]) {
        let a = ids(&run(s, "sketch.line", json!({"points": [[0, 0], [6, 0]]}))["curves"])[0].clone();
        let bctrl = [Vec2::new(20.0, 10.0), Vec2::new(24.0, 14.0), Vec2::new(30.0, 9.0), Vec2::new(34.0, 6.0)];
        let b = ids(&run(s, "sketch.spline.control_point", json!({"points": [[20, 10], [24, 14], [30, 9], [34, 6]]}))["curves"])[0].clone();
        (a, b, bctrl)
    }

    #[test]
    fn blend_curve_matches_both_references_through_the_requested_order() {
        for order in 1..=4usize {
            let mut s = new_sketch();
            let (a, b, bctrl) = two_references(&mut s);
            let r = run(
                &mut s,
                "sketch.blend_curve",
                json!({
                    "a": format!("{a}.end"),
                    "b": format!("{b}.start"),
                    "continuity": format!("g{order}"),
                }),
            );
            let sp = ids(&r["curves"])[0].clone();
            let (ctrl, degree, piece) = blend_of(&s, &sp);
            assert_eq!(degree, 2 * order + 1, "g{order} is one Bézier of degree 2k+1");
            assert_eq!(ctrl.len(), 2 * order + 2, "g{order} control count");
            assert!(r["sketch"]["solved"].as_bool().unwrap_or(false), "{r}");
            assert!(piece.start().dist(Vec2::new(6.0, 0.0)) < 1e-12 && piece.end().dist(bctrl[0]) < 1e-12, "the blend joins `a` and `b`");

            // What each reference supplies at the joint: the line's own derivative at its end (and
            // nothing higher) and the cubic's at its start.
            let line = piece_derivatives(&Seg2::Line { a: Vec2::ZERO, b: Vec2::new(6.0, 0.0) }, true, 4);
            let cubic = piece_derivatives(&Seg2::Cubic { p0: bctrl[0], p1: bctrl[1], p2: bctrl[2], p3: bctrl[3] }, false, 4);
            let got0 = blend_derivatives(&ctrl, false, 4);
            let got1 = blend_derivatives(&ctrl, true, 4);
            // g1 is the tangent construction of the tool: the first derivative is parallel to the
            // reference's (that is what G1 is), with the tool's own chord-scaled magnitude. g2 and
            // up match the reference's derivative vectors themselves.
            let tan_line = line[0].normalized().unwrap();
            let tan_cubic = cubic[0].normalized().unwrap();
            assert!(got0[0].cross(tan_line).abs() < 1e-9 && got0[0].dot(tan_line) > 0.0, "g{order} leaves `a` along {tan_line:?}, not {:?}", got0[0]);
            assert!(
                got1[0].cross(tan_cubic).abs() < 1e-9 && got1[0].dot(tan_cubic) > 0.0,
                "g{order} enters `b` along {tan_cubic:?}, not {:?}",
                got1[0]
            );
            for j in 1..order {
                assert!(got0[j].dist(line[j]) < 1e-9, "g{order} at a, order {}: {:?} vs {:?}", j + 1, got0[j], line[j]);
                assert!(got1[j].dist(cubic[j]) < 1e-9, "g{order} at b, order {}: {:?} vs {:?}", j + 1, got1[j], cubic[j]);
            }
            if order >= 2 {
                for (got, want) in [(got0[0], line[0]), (got1[0], cubic[0])] {
                    assert!(got.dist(want) < 1e-9, "g{order} matches the reference's own derivative: {got:?} vs {want:?}");
                }
            }
            // The next order up is not met: the line has no derivative there at all, while a blend of
            // degree 2k+1 has.
            if order < 4 {
                let next = got0[order];
                assert!(next.dist(line[order]) > 1e-6, "g{order} matched one order too many at a: {next:?}");
            }
            // Curvature itself (orientation-free, from finite differences of the samples) agrees at
            // both joints once the order asks for it.
            if order >= 2 {
                let curve = |x: f64| piece.point_at(x.clamp(0.0, 1.0));
                let line_f = |x: f64| Vec2::new(6.0 * x, 0.0);
                let cubic_f = |x: f64| Seg2::Cubic { p0: bctrl[0], p1: bctrl[1], p2: bctrl[2], p3: bctrl[3] }.point_at(x);
                let k0 = solidcraft_sketch::end_curvature(&curve, false);
                let ka = solidcraft_sketch::end_curvature(&line_f, true);
                let k1 = solidcraft_sketch::end_curvature(&curve, true);
                let kb = solidcraft_sketch::end_curvature(&cubic_f, false);
                assert!(k0.dist(ka) < 1e-3, "g{order} curvature at a: {k0:?} vs {ka:?}");
                assert!(k1.dist(kb) < 1e-3, "g{order} curvature at b: {k1:?} vs {kb:?}");
            }
        }
    }

    #[test]
    fn blend_curve_g1_is_the_tangent_construction_with_its_constraints() {
        let mut s = new_sketch();
        let (a, b, bctrl) = two_references(&mut s);
        let r = run(&mut s, "sketch.blend_curve", json!({"a": format!("{a}.end"), "b": format!("{b}.start")}));
        let sp = ids(&r["curves"])[0].clone();
        let (ctrl, degree, _) = blend_of(&s, &sp);
        assert_eq!(degree, 3, "the default is the cubic tangent spline");
        let (qa, qb) = (Vec2::new(6.0, 0.0), bctrl[0]);
        let l = qa.dist(qb);
        let tb = (qb - bctrl[1]).normalized().unwrap();
        let want = [qa, qa + Vec2::X * (l / 3.0), qb + tb * (l / 3.0), qb];
        for (i, q) in want.iter().enumerate() {
            assert!(ctrl[i].dist(*q) < 1e-9, "g1 control {i}: {:?} vs {q:?}", ctrl[i]);
        }
        assert_eq!(ids(&r["constraints"]).len(), 2, "the two tangents are kept: {r}");
        // g2…g4 meet the derivatives by construction and need no constraint of their own.
        for order in 2..=4usize {
            let mut s = new_sketch();
            let (a, b, _) = two_references(&mut s);
            let r = run(
                &mut s,
                "sketch.blend_curve",
                json!({
                    "a": format!("{a}.end"),
                    "b": format!("{b}.start"),
                    "continuity": format!("g{order}"),
                }),
            );
            assert!(ids(&r["constraints"]).is_empty(), "g{order}: {r}");
        }
    }

    #[test]
    fn blend_curve_between_two_lines_is_curvature_free_to_the_requested_order() {
        // Two lines meeting at a right angle: from G2 on, the blend has the lines' own zero
        // curvature at both joints (a corner with no curvature jump).
        for order in 2..=4usize {
            let mut s = new_sketch();
            let a = ids(&run(&mut s, "sketch.line", json!({"points": [[0, 0], [2, 0]]}))["curves"])[0].clone();
            let b = ids(&run(&mut s, "sketch.line", json!({"points": [[20, 10], [20, 12]]}))["curves"])[0].clone();
            let r = run(
                &mut s,
                "sketch.blend_curve",
                json!({
                    "a": format!("{a}.end"),
                    "b": format!("{b}.start"),
                    "continuity": format!("g{order}"),
                }),
            );
            let sp = ids(&r["curves"])[0].clone();
            let (ctrl, degree, piece) = blend_of(&s, &sp);
            assert_eq!(degree, 2 * order + 1);
            let got0 = blend_derivatives(&ctrl, false, 4);
            let got1 = blend_derivatives(&ctrl, true, 4);
            assert!(got0[0].dist(Vec2::new(2.0, 0.0)) < 1e-12, "g{order} leaves `a` along +x: {:?}", got0[0]);
            assert!(got1[0].dist(Vec2::new(0.0, 2.0)) < 1e-12, "g{order} enters `b` along +y: {:?}", got1[0]);
            for j in 1..order {
                assert!(got0[j].len() < 1e-12, "g{order}: a line has no order-{} derivative", j + 1);
                assert!(got1[j].len() < 1e-12, "g{order}: a line has no order-{} derivative", j + 1);
            }
            if order < 4 {
                assert!(got0[order].len() > 1e-6, "g{order}: the blend curves, the line does not");
            }
            assert!(piece.start().dist(Vec2::new(2.0, 0.0)) < 1e-12 && piece.end().dist(Vec2::new(20.0, 10.0)) < 1e-12);
        }
    }

    #[test]
    fn blend_curve_matches_a_conic_reference() {
        for order in 2..=4usize {
            let mut s = new_sketch();
            let a = ids(&run(&mut s, "sketch.line", json!({"points": [[0, 0], [2, 0]]}))["curves"])[0].clone();
            let b = ids(&run(&mut s, "sketch.conic", json!({"start": [20, 10], "apex": [26, 16], "end": [30, 10], "rho": 0.7}))["curves"])[0].clone();
            let r = run(
                &mut s,
                "sketch.blend_curve",
                json!({
                    "a": format!("{a}.end"),
                    "b": format!("{b}.start"),
                    "continuity": format!("g{order}"),
                }),
            );
            let sp = ids(&r["curves"])[0].clone();
            let (ctrl, degree, _) = blend_of(&s, &sp);
            assert_eq!(degree, 2 * order + 1);
            // The conic's own derivatives at its start: `w` comes from the sketch's `rho`, and the
            // series division in `piece_derivatives` is the expectation.
            let w = 0.7 / (1.0 - 0.7);
            let conic = Seg2::Conic { a: Vec2::new(20.0, 10.0), apex: Vec2::new(26.0, 16.0), b: Vec2::new(30.0, 10.0), w };
            let want = piece_derivatives(&conic, false, order);
            let got = blend_derivatives(&ctrl, true, order);
            for j in 0..order {
                assert!(got[j].dist(want[j]) < 1e-8 * want[j].len().max(1.0), "g{order} order {}: {:?} vs {:?}", j + 1, got[j], want[j]);
            }
        }
    }

    #[test]
    fn blend_curve_refuses_hostile_orders_and_degenerate_references() {
        let mut s = new_sketch();
        let (a, b, _) = two_references(&mut s);
        let ends = json!({"a": format!("{a}.end"), "b": format!("{b}.start")});
        for (order, want) in [("g0", true), ("g9", true), ("G9", true), ("curvature", true), ("", false), ("g2", false), ("G4", false)] {
            let mut p = ends.clone();
            p["continuity"] = json!(order);
            let r = s.execute("sketch.blend_curve", &p);
            assert_eq!(r.is_err(), want, "continuity {order:?}: {r:?}");
            assert!(r.is_ok() || r.unwrap_err().to_string().contains("continuity"), "continuity {order:?} needs a clear error");
        }
        let mut numbered = ends.clone();
        numbered["continuity"] = json!(3);
        assert!(s.execute("sketch.blend_curve", &numbered).is_err(), "a number is not a continuity");
        // A point that is not an end of a curve, and the same end twice.
        let p = ids(&run(&mut s, "sketch.point", json!({"point": [50, 50]}))["created"]["points"])[0].clone();
        let e = s.execute("sketch.blend_curve", &json!({"a": p, "b": format!("{b}.start")})).unwrap_err().to_string();
        assert!(e.contains("no curve ends at `a`"), "{e}");
        let e = s.execute("sketch.blend_curve", &json!({"a": format!("{a}.end"), "b": format!("{a}.end")})).unwrap_err().to_string();
        assert!(e.contains("ends must differ"), "{e}");
        // The helper behind the command refuses a corrupt (non-finite) reference and a reference
        // with no tangent at all, never guessing at them.
        let mut broken = solidcraft_sketch::Sketch::new();
        let p0 = broken.add_point(Vec2::new(1.0, 1.0), None).unwrap();
        let p1 = broken.add_point(Vec2::ZERO, None).unwrap();
        let l = broken.add_curve(CurveKind::Spline { pts: vec![p1, p0], control: true, degree: 1, periodic: false }, None).unwrap();
        broken.points[p0].pos = Vec2::new(f64::NAN, 4.0);
        assert!(super::joint_derivatives(&broken, l, p1, 2, true, "a", "sketch.blend_curve").is_err(), "non-finite geometry");
        let mut flat = solidcraft_sketch::Sketch::new();
        let q0 = flat.add_point(Vec2::new(3.0, 3.0), None).unwrap();
        let q1 = flat.add_point(Vec2::new(3.0, 3.0), None).unwrap();
        let d = flat.add_curve(CurveKind::Spline { pts: vec![q0, q1], control: true, degree: 1, periodic: false }, None).unwrap();
        let e = super::joint_derivatives(&flat, d, q0, 2, true, "a", "sketch.blend_curve").unwrap_err().to_string();
        assert!(e.contains("no tangent"), "{e}");
        assert!(solidcraft_geom::hermite_bezier(Vec2::ZERO, Vec2::X, &[Vec2::Y; 5], &[Vec2::Y; 5]).is_none(), "no order above g4");
    }

    #[test]
    fn blend_curve_with_g4_closes_a_profile_that_measures_exactly() {
        // A quarter-turn blend from (2, 0) into a line at (20, 10), closed by the chord between its
        // ends: the region's area comes from the degree-9 curve, not from its chord.
        let mut s = new_sketch();
        let a = ids(&run(&mut s, "sketch.line", json!({"points": [[0, 0], [2, 0]]}))["curves"])[0].clone();
        let b = ids(&run(&mut s, "sketch.line", json!({"points": [[20, 10], [20, 12]]}))["curves"])[0].clone();
        let r = run(
            &mut s,
            "sketch.blend_curve",
            json!({
                "a": format!("{a}.end"),
                "b": format!("{b}.start"),
                "continuity": "g4",
            }),
        );
        let sp = ids(&r["curves"])[0].clone();
        let (ctrl, degree, piece) = blend_of(&s, &sp);
        assert_eq!(degree, 9, "G4 is one degree-9 Bézier");
        assert_eq!(ctrl.len(), 10);
        // The command still reports what it added, with stable ids: ten controls of which the two
        // joints are the references' own points, so eight new points, and no new constraint.
        assert_eq!(ids(&r["created"]["curves"]), vec![sp.clone()], "{r}");
        assert_eq!(ids(&r["created"]["points"]).len(), 8, "{r}");
        assert!(ids(&r["created"]["constraints"]).is_empty() && ids(&r["created"]["dimensions"]).is_empty(), "{r}");
        run(&mut s, "sketch.line", json!({"points": [[20, 10], [2, 0]], "id": "chord"}));
        // Drop the two references: what is left is the blend and its chord, one closed region.
        run(&mut s, "sketch.delete", json!({"entities": [a, b]}));
        let id = s.active_sketch.unwrap();
        let state = s.model.state();
        let profiles = &state.sketch(id).unwrap().profiles;
        assert_eq!(profiles.len(), 1, "the blend and the chord close one region");
        let area = profiles[0].area;
        // A fine sampling of the exact piece plus the straight chord reproduces the area; the chord
        // alone (what a polyline approximation would give) is exactly zero.
        let n = 20_000;
        let mut sampled = 0.0;
        for i in 0..n {
            let (t0, t1) = (i as f64 / n as f64, (i + 1) as f64 / n as f64);
            sampled += 0.5 * piece.point_at(t0).cross(piece.point_at(t1));
        }
        // The chord back from (20, 10) to (2, 0) contributes its own shoelace term; together they
        // are the region.
        sampled += 0.5 * Vec2::new(20.0, 10.0).cross(Vec2::new(2.0, 0.0));
        assert!((area - sampled.abs()).abs() < 1e-4, "{area} vs {sampled}");
        assert!(area > 1e-2, "the blend curves away from the chord: {area}");
        // And the degree-9 profile still extrudes to its area times the height (the kernel's
        // volume of a free-form body is a numerical integration, so this is the same 2e-4 the
        // neighbouring free-form extrusion test allows).
        run(&mut s, "solid.extrude", json!({"distance": 5}));
        let m = run(&mut s, "inspect.measure", json!({}));
        let volume = m["total"]["volume_mm3"].as_f64().unwrap_or(f64::NAN);
        assert!((volume - area * 5.0).abs() / (area * 5.0) < 2e-4, "volume {volume} vs {}", area * 5.0);
        assert!(m["bodies"][0]["faces"].as_u64().unwrap_or(0) >= 3, "a solid with the blend as a side: {m}");
    }
}
