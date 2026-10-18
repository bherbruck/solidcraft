//! Section Analysis: a view-only cut through the model by a plane (what is on the plane's
//! positive side is hidden). It changes no geometry and is not an undo step.

use serde_json::{Value, json};
use solidcraft_geom::{Plane, Vec3};

use super::CommandSpec;
use crate::params::{bad, bool_, expr, vec3};
use crate::{Result, Session};

#[path = "section_curves.rs"]
mod curves;

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("inspect.section", "Section Analysis", section).at("SOLID", "INSPECT").icon("section").noundo().params(
        "plane: XY|XZ|YZ | construction plane name or id | {origin, normal}; offset?: expr along the plane's normal (XY: +Z, XZ: +Y, YZ: +X); \
         at?: expr, the cut's world coordinate on the named plane's axis (XY: Z, XZ: Y, YZ: X), instead of offset; \
         flip?: bool; or clear: true. Returns the cut's origin and normal, and for a named plane its axis and at",
    ),
    CommandSpec::new(curves::CMD, "Section Curves", curves::section_curves).icon("section").noundo().params(curves::PARAMS),
];

fn section(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "inspect.section";
    if bool_(p, "clear").unwrap_or(false) {
        s.section = None;
        s.revision += 1;
        return Ok(json!({"section": null}));
    }
    let (plane, axis) = cut_plane(s, p, cmd)?;
    let (origin, normal) = (plane.origin, plane.normal());
    s.section = Some((origin, normal));
    s.revision += 1;
    let mut section = json!({"origin": origin, "normal": normal});
    if let Some(i) = axis {
        section["axis"] = json!(["X", "Y", "Z"].get(i).copied().unwrap_or("?"));
        section["at"] = json!(axis_component(origin, i));
    }
    Ok(json!({"section": section}))
}

/// The cut plane of `plane`, `offset` | `at` and `flip`, with its in-plane axes (the named or
/// construction plane's own, else chosen from the normal; `flip` reverses y), and for a named
/// plane the index of its world axis.
pub(super) fn cut_plane(s: &Session, p: &Value, cmd: &str) -> Result<(Plane, Option<usize>)> {
    // A named plane's world axis: which component of a point lies along its normal.
    let (base, axis) = match p.get("plane") {
        Some(Value::String(n)) if Plane::named(n.trim()).is_some() => {
            // Apply the same ambiguity check as modelling commands before treating a name as an origin alias.
            super::features::plane_param(s, p.get("plane"), cmd)?;
            let pl = Plane::named(n.trim()).ok_or_else(|| bad(cmd, format!("unknown plane `{n}`")))?;
            let normal = pl.normal();
            let axis = [normal.x, normal.y, normal.z].iter().position(|c| c.abs() > 0.5);
            (pl, axis)
        }
        // A construction plane (name or id): where it is now.
        Some(v @ (Value::String(_) | Value::Number(_))) => {
            let r = super::features::plane_param(s, Some(v), cmd)?;
            let solidcraft_doc::PlaneRef::Construction { name } = &r else { return Err(bad(cmd, "`plane` must be a plane")) };
            let st = s.model.state();
            let pl = st
                .construct
                .iter()
                .find(|c| &c.name == name)
                .and_then(|c| if let solidcraft_doc::construct::ConstructGeom::Plane(pl) = &c.geom { Some(*pl) } else { None })
                .ok_or_else(|| bad(cmd, format!("construction plane `{name}` is not built (rolled back, suppressed or failed)")))?;
            (pl, None)
        }
        Some(o @ Value::Object(_)) => {
            let origin = o.get("origin").and_then(vec3).unwrap_or(Vec3::ZERO);
            let n = o.get("normal").and_then(vec3).and_then(|n| n.normalized()).ok_or_else(|| bad(cmd, "the plane needs a non-zero `normal`"))?;
            if !origin.is_finite() {
                return Err(bad(cmd, "the plane must be finite"));
            }
            (Plane::from_normal(origin, n).ok_or_else(|| bad(cmd, "the plane needs a non-zero `normal`"))?, None)
        }
        _ => return Err(bad(cmd, "`plane` must be XY, XZ, YZ, a construction plane name or id, or {origin, normal}")),
    };
    let (origin, normal) = (base.origin, base.normal());
    let length = |key: &str| -> Result<Option<f64>> {
        match expr(p, key) {
            Some(e) => s.doc.eval(&e, solidcraft_doc::expr::Kind::Length).map(Some).map_err(|e| bad(cmd, format!("{key}: {e}"))),
            None => Ok(None),
        }
    };
    let (offset, at) = (length("offset")?, length("at")?);
    let offset = match (offset, at, axis) {
        (Some(_), Some(_), _) => return Err(bad(cmd, "give either `offset` (along the normal) or `at` (a world coordinate), not both")),
        (_, Some(_), None) => return Err(bad(cmd, "`at` needs a named plane (XY, XZ, YZ); move an {origin, normal} plane by its origin")),
        // The named planes pass through the world origin, so the offset is `at` signed by the normal.
        (_, Some(at), Some(i)) => at * axis_component(normal, i).signum(),
        (offset, None, _) => offset.unwrap_or(0.0),
    };
    if !origin.is_finite() || !offset.is_finite() {
        return Err(bad(cmd, "the plane must be finite"));
    }
    let plane = Plane { origin: origin + normal * offset, ..base };
    Ok((if bool_(p, "flip").unwrap_or(false) { plane.flipped(false) } else { plane }, axis))
}

fn axis_component(v: Vec3, i: usize) -> f64 {
    match i {
        0 => v.x,
        1 => v.y,
        _ => v.z,
    }
}
