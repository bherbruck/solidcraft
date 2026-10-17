//! Boundary surfaces (`surface.boundary`, issue #763): one patch through two to four sketch
//! curves in loop order, or one surface through a network of curves in two directions (sketch
//! curves, 3D sketch curves, a body's edges, or a point for a three-sided patch). The kernel
//! builds the face; this is the command's parameters and its checks, so that what it cannot hold
//! is refused before anything is built.

use serde_json::Value;
use solidcraft_doc::{BodyName, FeatureKind, NetworkCurve, Operation, Position, SketchId};
use solidcraft_kernel::Continuity;

use super::CommandSpec;
use super::features::{add_feature, feature_sketch, sketch_id};
use crate::params::{bad, str_, string_list, vec3};
use crate::{Result, Session};

/// The stations a boundary carries by default (the patch's resolution along the loop).
const DEFAULT_SAMPLES: usize = solidcraft_doc::BOUNDARY_SAMPLES;
/// The fewest stations a patch can hold the widest continuity pair with: a G3 side needs four rows
/// at each of two opposite sides, so eight.
const MIN_SAMPLES: usize = 8;
/// The most stations a boundary may carry (the kernel's own cap).
const MAX_SAMPLES: usize = 512;
/// The most curves one direction of a network may list (the kernel's own cap).
const MAX_NETWORK_CURVES: usize = solidcraft_geom::network::MAX_NETWORK_CURVES;
/// The most sketch curves one network curve may chain, and points picking one chain of edges
/// (the kernel's own caps).
use solidcraft_kernel::{MAX_CHAIN_CURVES as MAX_CHAIN, MAX_EDGE_PICKS as MAX_PICKS};

pub static COMMANDS: &[CommandSpec] = &[CommandSpec::new("surface.boundary", "Boundary Surface", boundary)
    .at("SURFACE", "CREATE")
    .icon("surface_boundary")
    .params(
        "Either a loop of one sketch's curves — sketch?: id|name (default: the sketch being edited); curves: [2 to 4 sketch curve ids in loop order] (each one ends where the next begins and the last where the first does; two: the patch spans between them and leaves each boundary along the tangent across it; three or four: the flat patch of the loop, three collapsing the side opposite the first curve to the corner there); continuity?: g0|g1|g2|g3 | [one per curve] (default g1: how much of the surface across each boundary the patch repeats — a sketch curve's own plane, so g1 and up hold exactly and g0 lets the middle bow); crown?: number -0.9…3 (default 0: how full the middle is, without touching a boundary; two curves only); samples?: 8…512 stations per boundary (default 25) — or a curve network — u: [curve] (two or more one way: the surface lofts through them), v?: [curve] (crossing every u-curve once: two each way give a Coons patch, more a Gordon surface through every curve; a curve that misses another by more than 0.01 mm is refused, naming the pair), each curve \"id\" (a curve of `sketch`) | {sketch?: id|name, curves: [ids meeting end to end]} (planar or 3D sketch curves) | {body: name, edges: [[x,y,z]] (points on a chain of its edges), continuity?: g0|g1|g2 (default g0; g1 the surface leaves tangent to the face beside a surface's open edges, g2 with its curvature too; on curves of one direction only)} | {point: [x,y,z]} (a side collapsed to a point: a three-sided patch); operation?: new|join (default new; join sews the surface into the body of its first edge curve, a solid when it closes the gap). name?, body_name? (a surface body)",
    )];

/// `surface.boundary`: a loop of sketch curves, or a network when `u` is given.
fn boundary(s: &mut Session, p: &Value) -> Result<Value> {
    if p.get("u").is_some_and(|v| !v.is_null()) {
        return network(s, p);
    }
    loop_patch(s, p)
}

/// `surface.boundary` with `curves`: one patch through two to four sketch curves in loop order.
fn loop_patch(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "surface.boundary";
    if p.get("v").is_some_and(|v| !v.is_null()) || p.get("operation").is_some_and(|v| !v.is_null()) {
        return Err(bad(cmd, "`v` and `operation` belong to a curve network: give its curves as `u` and `v`"));
    }
    let sketch = feature_sketch(s, p, cmd)?;
    let curves = string_list(p, "curves");
    if !(2..=4).contains(&curves.len()) {
        return Err(bad(cmd, "`curves` must list 2 to 4 sketch curves in loop order (or give a network as `u` and `v`)"));
    }
    // The curves must be the sketch's own, and they must run on into one another: the last station
    // of each is the first of the next, and the last boundary's end is the first one's start.
    let state = s.model.state();
    let sk = state.sketch(sketch).ok_or_else(|| bad(cmd, "the sketch has not been solved"))?;
    let mut ends: Vec<(solidcraft_geom::Vec2, solidcraft_geom::Vec2)> = Vec::with_capacity(curves.len());
    for id in &curves {
        let ci = sk.sketch.curve_index(id).ok_or_else(|| bad(cmd, format!("no sketch curve `{id}`")))?;
        let pieces = sk.sketch.exact_pieces(ci);
        let (Some(first), Some(last)) = (pieces.first(), pieces.last()) else {
            return Err(bad(cmd, format!("curve `{id}` has no exact geometry to build on")));
        };
        if last.end().dist(first.start()) < 1e-9 {
            return Err(bad(cmd, format!("curve `{id}` is closed: a boundary must be one side of the loop")));
        }
        ends.push((first.start(), last.end()));
    }
    // Two boundaries are the two sides of a bridge, which do not meet; three or four run round a
    // loop, so each has to end where the next begins.
    let scale = ends.iter().map(|(a, b)| a.dist(*b)).fold(1.0_f64, f64::max);
    let tol = scale * 1e-4 + 1e-9;
    for i in 0..if curves.len() >= 3 { ends.len() } else { 0 } {
        let next = (i + 1) % ends.len();
        let (Some(end), Some(start)) = (ends.get(i).map(|e| e.1), ends.get(next).map(|e| e.0)) else {
            return Err(bad(cmd, "a curve is missing its ends"));
        };
        let gap = end.dist(start);
        if gap > tol {
            return Err(bad(
                cmd,
                format!("curve `{}` ends {gap:.4} mm from where `{}` starts: list the curves in loop order", curves[i], curves[next]),
            ));
        }
    }
    drop(state);
    let orders = match p.get("continuity") {
        None | Some(Value::Null) => vec![Continuity::G1; curves.len()],
        Some(Value::String(w)) => {
            let one = Continuity::parse(w).ok_or_else(|| bad(cmd, "`continuity` must be g0, g1, g2 or g3"))?;
            vec![one; curves.len()]
        }
        Some(v) => {
            let list = v.as_array().filter(|a| a.len() == curves.len()).ok_or_else(|| bad(cmd, "`continuity` must be one word, or one per curve"))?;
            let mut out = Vec::with_capacity(list.len());
            for x in list {
                out.push(x.as_str().and_then(Continuity::parse).ok_or_else(|| bad(cmd, "`continuity` entries must be g0, g1, g2 or g3"))?);
            }
            out
        }
    };
    let crown = match p.get("crown") {
        None | Some(Value::Null) => 0.0,
        Some(v) => v
            .as_f64()
            .filter(|x| x.is_finite() && (solidcraft_geom::fill::MIN_CROWN..=solidcraft_geom::fill::MAX_CROWN).contains(x))
            .ok_or_else(|| bad(cmd, "`crown` must be a number between -0.9 and 3"))?,
    };
    let samples = match p.get("samples") {
        None | Some(Value::Null) => DEFAULT_SAMPLES,
        Some(v) => {
            let n = v.as_f64().filter(|x| x.is_finite() && x.fract() == 0.0).map(|x| x as usize);
            n.filter(|x| (MIN_SAMPLES..=MAX_SAMPLES).contains(x))
                .ok_or_else(|| bad(cmd, format!("`samples` must be a whole number of stations between {MIN_SAMPLES} and {MAX_SAMPLES}")))?
        }
    };
    if crown != 0.0 && curves.len() >= 3 {
        return Err(bad(cmd, "not supported yet: `crown` on a patch through three or four curves (it is the flat patch of its sides)"));
    }
    add_feature(s, p, FeatureKind::SurfaceBoundary { sketch: SketchId(sketch), curves, orders, crown, samples })
}

/// `surface.boundary` with `u` (and `v`): one surface through a curve network.
fn network(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "surface.boundary";
    for key in ["curves", "continuity", "crown", "samples"] {
        if p.get(key).is_some_and(|v| !v.is_null()) {
            return Err(bad(cmd, format!("`{key}` belongs to a loop of sketch curves: a network gives each curve its own continuity")));
        }
    }
    let operation = super::features::operation(p, cmd)?;
    if !matches!(operation, Operation::NewBody | Operation::Join) {
        return Err(bad(cmd, "`operation` must be new or join"));
    }
    // The sketch a bare curve id belongs to, found only when one is used.
    let mut default_sketch: Option<u64> = None;
    let u = network_curves(s, p, "u", &mut default_sketch)?;
    let v = network_curves(s, p, "v", &mut default_sketch)?;
    if u.is_empty() {
        return Err(bad(cmd, "`u` must list the curves of one direction"));
    }
    if u.len() + v.len() < 2 {
        return Err(bad(cmd, "a boundary surface needs two curves in one direction, or crossing curves"));
    }
    if operation == Operation::Join && !u.iter().chain(&v).any(|c| c.body.is_some()) {
        return Err(bad(cmd, "`operation` join needs a curve on a body's edges: the surface is sewn into that body"));
    }
    add_feature(s, p, FeatureKind::SurfaceNetwork { u, v, operation })
}

/// One direction of a network (`key` is `u` or `v`), checked: each curve sketch curves that exist
/// (planar or 3D), a body's edges with points on them, or a point; a continuity only on edges.
fn network_curves(s: &Session, p: &Value, key: &str, default_sketch: &mut Option<u64>) -> Result<Vec<NetworkCurve>> {
    let cmd = "surface.boundary";
    let list = match p.get(key) {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(Value::Array(a)) => a,
        Some(_) => return Err(bad(cmd, format!("`{key}` must list curves"))),
    };
    if list.len() > MAX_NETWORK_CURVES {
        return Err(bad(cmd, format!("`{key}` may list at most {MAX_NETWORK_CURVES} curves")));
    }
    let mut out = Vec::with_capacity(list.len());
    for (k, item) in list.iter().enumerate() {
        let name = format!("{key}[{k}]");
        let mut curve = NetworkCurve { sketch: None, curves: Vec::new(), body: None, edges: Vec::new(), point: None, continuity: Continuity::G0 };
        let obj = match item {
            Value::String(id) => {
                curve.curves = vec![id.clone()];
                None
            }
            Value::Object(_) => Some(item),
            _ => return Err(bad(cmd, format!("{name} must be a curve id or {{sketch?, curves}}, {{body, edges}} or {{point}}"))),
        };
        if let Some(o) = obj {
            let has = |k: &str| o.get(k).is_some_and(|v| !v.is_null());
            let kinds = [has("curves"), has("body") || has("edges"), has("point")];
            if kinds.iter().filter(|x| **x).count() != 1 {
                return Err(bad(cmd, format!("{name} must be one of {{sketch?, curves}}, {{body, edges}} or {{point}}")));
            }
            if has("point") {
                curve.point = Some(Position(o.get("point").and_then(vec3).ok_or_else(|| bad(cmd, format!("{name}.point must be [x, y, z]")))?));
            } else if kinds[1] {
                let body = str_(o, "body").ok_or_else(|| bad(cmd, format!("{name}.body must name a body")))?;
                if s.model.state().body(body).is_none() {
                    return Err(bad(cmd, format!("no body `{body}`")));
                }
                let edges =
                    o.get("edges").and_then(Value::as_array).ok_or_else(|| bad(cmd, format!("{name}.edges must list points on the edges")))?;
                if edges.is_empty() || edges.len() > MAX_PICKS {
                    return Err(bad(cmd, format!("{name}.edges must list 1 to {MAX_PICKS} points on a chain of edges")));
                }
                let pts: Vec<Position> = edges
                    .iter()
                    .map(|e| vec3(e).map(Position).ok_or_else(|| bad(cmd, format!("{name}.edges: a point must be [x, y, z]"))))
                    .collect::<Result<_>>()?;
                curve.body = Some(BodyName::from(body.to_string()));
                curve.edges = pts;
            } else {
                curve.curves = string_list(o, "curves");
                if o.get("sketch").is_some_and(|v| !v.is_null()) {
                    curve.sketch = Some(SketchId(sketch_id(s, o.get("sketch"), cmd, &format!("{name}.sketch"))?));
                }
            }
            if let Some(g) = o.get("continuity").filter(|v| !v.is_null()) {
                let g = g.as_str().and_then(Continuity::parse).ok_or_else(|| bad(cmd, format!("{name}.continuity must be g0, g1 or g2")))?;
                if g == Continuity::G3 {
                    return Err(bad(cmd, format!("not supported yet: G3 on {name} (a curve network holds up to g2)")));
                }
                if g != Continuity::G0 && curve.body.is_none() {
                    return Err(bad(cmd, format!("{name}: g1 and g2 apply to a curve on a body's edges (a surface's open edges)")));
                }
                curve.continuity = g;
            }
        }
        if curve.point.is_none() && curve.body.is_none() {
            if curve.curves.is_empty() || curve.curves.len() > MAX_CHAIN {
                return Err(bad(cmd, format!("{name} must list 1 to {MAX_CHAIN} sketch curves")));
            }
            let sketch = match curve.sketch {
                Some(id) => id.0,
                None => match *default_sketch {
                    Some(id) => id,
                    None => {
                        let id = feature_sketch(s, p, cmd)?;
                        *default_sketch = Some(id);
                        id
                    }
                },
            };
            let state = s.model.state();
            let sk = state.sketch(sketch).ok_or_else(|| bad(cmd, "the sketch has not been solved"))?;
            for id in &curve.curves {
                if sk.sketch.curve_index(id).is_none() && sk.sketch.wire_index(id).is_none() {
                    return Err(bad(cmd, format!("{name}: no sketch curve `{id}`")));
                }
            }
            curve.sketch = Some(SketchId(sketch));
        }
        out.push(curve);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

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

    /// Two parallel 20 mm lines 10 mm apart, and the patch between them.
    fn two_walls(s: &mut Session) -> Vec<String> {
        let a = ids(&run(s, "sketch.line", json!({"points": [[0, 0], [20, 0]]}))["created"]["curves"])[0].clone();
        let b = ids(&run(s, "sketch.line", json!({"points": [[0, 10], [20, 10]]}))["created"]["curves"])[0].clone();
        vec![a, b]
    }

    #[test]
    fn a_boundary_patch_between_two_sketch_curves_is_the_sheet_they_span() {
        for continuity in [json!("g0"), json!("g1"), json!("g2"), json!(["g1", "g2"])] {
            let mut s = new_sketch();
            let curves = two_walls(&mut s);
            let r = run(&mut s, "surface.boundary", json!({"curves": curves, "continuity": continuity}));
            let body = ids(&r["created_bodies"])[0].clone();
            // It is one face, its boundary is the two lines themselves, and it covers exactly the
            // 20 × 10 sheet between them — all four frames lie in the sketch's plane.
            let f = run(&mut s, "model.faces", json!({"body": body}));
            let faces = f["faces"].as_array().cloned().unwrap_or_default();
            assert_eq!(faces.len(), 1, "{continuity}: {f}");
            assert!((faces[0]["area"].as_f64().unwrap_or(f64::NAN) - 200.0).abs() < 1e-4, "{continuity}: {f}");
            let normal = faces[0]["plane_normal"].as_array().cloned().unwrap_or_default();
            assert_eq!(normal.len(), 3, "{continuity}: the patch is planar: {f}");
            assert!(normal[2].as_f64().unwrap_or(0.0).abs() > 1.0 - 1e-9, "{continuity}: it lies in the sketch's plane: {f}");
            let m = run(&mut s, "inspect.measure", json!({}));
            let area = m["total"]["area_mm2"].as_f64().unwrap_or(f64::NAN);
            assert!((area - 200.0).abs() < 1e-4, "{continuity}: {area}");
        }
    }

    #[test]
    fn a_boundary_patch_refuses_values_it_cannot_hold() {
        let mut s = new_sketch();
        let curves = two_walls(&mut s);
        let extra = ids(&run(&mut s, "sketch.line", json!({"points": [[30, 30], [40, 40]]}))["created"]["curves"])[0].clone();
        // A loop of three: the crown is the bridge's, not the flat loop patch's.
        let a = ids(&run(&mut s, "sketch.line", json!({"points": [[0, 0], [20, 0]]}))["created"]["curves"])[0].clone();
        let b = ids(&run(&mut s, "sketch.line", json!({"points": [[20, 0], [10, 15]]}))["created"]["curves"])[0].clone();
        let c = ids(&run(&mut s, "sketch.line", json!({"points": [[10, 15], [0, 0]]}))["created"]["curves"])[0].clone();
        let e = s.execute("surface.boundary", &json!({"curves": [a, b, c], "crown": 1})).unwrap_err().to_string();
        assert!(e.contains("not supported yet") && e.contains("crown"), "three boundaries with a crown: {e}");
        // Curves out of loop order.
        let e = s.execute("surface.boundary", &json!({"curves": [curves[0], curves[1], extra]})).unwrap_err().to_string();
        assert!(e.contains("loop order"), "{e}");
        // Parameters and curves that make no sense.
        let cases: Vec<(Value, &str)> = vec![
            (json!({"curves": [curves[0]]}), "2 to 4"),
            (json!({"curves": [curves[0], curves[1], extra.clone(), extra.clone(), extra.clone()]}), "2 to 4"),
            (json!({"curves": [curves[0], "nope"]}), "no sketch curve"),
            (json!({"curves": curves, "continuity": "g9"}), "continuity"),
            (json!({"curves": curves, "continuity": ["g1", "g2", "g3"]}), "continuity"),
            (json!({"curves": curves, "crown": 5}), "crown"),
            (json!({"curves": curves, "crown": "full"}), "crown"),
            (json!({"curves": curves, "samples": 3}), "samples"),
            (json!({"curves": curves, "samples": 2.5}), "samples"),
            (json!({"curves": curves, "operation": "join"}), "network"),
        ];
        for (params, want) in cases {
            let e = s.execute("surface.boundary", &params).unwrap_err().to_string();
            assert!(e.contains(want), "{params}: {e}");
        }
        // Nothing was built by any of them.
        assert!(s.model.state().bodies.is_empty(), "a refused patch left a body behind");
    }
}

#[cfg(test)]
#[path = "surface_boundary_tests.rs"]
mod network_tests;
