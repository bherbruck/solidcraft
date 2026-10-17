//! Loose ends of a sketch's profile curves: the ones that touch nothing (open) and the ones that
//! touch another curve only by position (untied), and tying the untied ones.

use serde_json::{Value, json};
use solidcraft_sketch::{ConstraintKind, EndReport, EndTouch, Sketch, loose_ends};

use super::sketch::{add_c, edit, target_sketch};
use super::{CommandSpec, in_sketch};
use crate::params::{bad, string_list};
use crate::{EngineError, Result, Session};

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("sketch.loose_ends", "Loose Ends", loose_ends_cmd).icon("c_coincident").enabled(in_sketch).noundo().params(
        "sketch?: id|name (default active): the ends of profile curves (not construction, projected or text) that can make a profile fail. open: [{point, curve, at}] ends that touch no other profile curve (no profile runs through them); untied: [{point, curve, at, touches: {point}|{curve}}] ends that touch another curve only by position (the profile closes now, but nothing keeps it closed when a dimension changes)",
    ),
    CommandSpec::new("sketch.loose_ends.tie", "Tie Loose Ends", tie_cmd).icon("c_coincident").enabled(in_sketch).params(
        "sketch?: id|name (default active); points?: [point ids] (default: every untied end): ties each untied end to what it touches, coincident with another curve's end or on another curve. Returns constraints: [ids added], and the untied ends left",
    ),
];

fn point_id(sk: &Sketch, i: usize) -> Value {
    sk.points.get(i).map_or(Value::Null, |p| json!(p.id))
}

fn curve_id(sk: &Sketch, i: usize) -> Value {
    sk.curves.get(i).map_or(Value::Null, |c| json!(c.id))
}

fn at(sk: &Sketch, i: usize) -> Value {
    sk.point(i).map_or(Value::Null, |v| json!([v.x, v.y]))
}

/// The point that is an end of curve `c` (for `curve` in the report).
fn end_curve(sk: &Sketch, p: usize) -> Value {
    let c = sk.curves.iter().position(|c| c.kind.ends().is_some_and(|(a, b)| a == p || b == p) && !(c.construction || c.centerline || c.infinite));
    c.map_or(Value::Null, |c| curve_id(sk, c))
}

fn report_json(sk: &Sketch, r: &EndReport) -> Value {
    let open: Vec<Value> = r.open.iter().map(|&p| json!({"point": point_id(sk, p), "curve": end_curve(sk, p), "at": at(sk, p)})).collect();
    let untied: Vec<Value> = r
        .untied
        .iter()
        .map(|u| {
            let touches = match u.touches {
                EndTouch::Point(q) => json!({"point": point_id(sk, q)}),
                EndTouch::Curve(c) => json!({"curve": curve_id(sk, c)}),
            };
            json!({"point": point_id(sk, u.point), "curve": curve_id(sk, u.curve), "at": at(sk, u.point), "touches": touches})
        })
        .collect();
    json!({"open": open, "untied": untied})
}

fn loose_ends_cmd(s: &mut Session, p: &Value) -> Result<Value> {
    let id = target_sketch(s, p, "sketch.loose_ends")?;
    let st = s.model.state();
    let ss = st.sketch(id).ok_or_else(|| EngineError::Other("the sketch has not been evaluated".into()))?;
    Ok(report_json(&ss.sketch, &ss.ends))
}

fn tie_cmd(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.loose_ends.tie";
    let only = match p.get("points") {
        None | Some(Value::Null) => None,
        Some(Value::Array(a)) if a.iter().all(Value::is_string) => Some(string_list(p, "points")),
        Some(_) => return Err(bad(cmd, "`points` must be a list of point ids")),
    };
    let (added, info) = edit(s, p, cmd, true, |sk, _| {
        let wanted = match &only {
            None => None,
            Some(ids) => Some(
                ids.iter().map(|r| sk.resolve_point(r).ok_or_else(|| bad(cmd, format!("unknown point `{r}`")))).collect::<Result<Vec<usize>>>()?,
            ),
        };
        let report = loose_ends(sk);
        let mut added = Vec::new();
        for u in &report.untied {
            if wanted.as_ref().is_some_and(|w| !w.contains(&u.point)) {
                continue;
            }
            let kind = match u.touches {
                EndTouch::Point(q) => ConstraintKind::Coincident { p: u.point, q },
                EndTouch::Curve(c) => ConstraintKind::PointOnCurve { p: u.point, c },
            };
            added.push(add_c(sk, kind)?);
        }
        let left = report_json(sk, &loose_ends(sk)).get("untied").cloned().unwrap_or(Value::Null);
        Ok((added, left))
    })?;
    let (added, left) = added;
    Ok(json!({"constraints": added, "sketch": info, "untied": left}))
}

#[cfg(test)]
#[path = "sketch_ends_tests.rs"]
mod tests;
