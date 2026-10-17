//! Inspection: measure bodies, describe the document, sketches and model topology.

use serde_json::{Value, json};
use solidcraft_sketch::CurveKind;

use super::CommandSpec;
use crate::params::{bad, bool_, str_, string_list};
use crate::{EngineError, Result, Session};

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("inspect.measure", "Measure", measure)
        .at("SOLID", "INSPECT")
        .icon("measure")
        .key("I")
        .noundo()
        .params("bodies?: [names] (default all); or items: [1-2 selections] → length/area/volume; diameter, radius, circumference or arc_length/angle_deg of circles, arcs and cylinders; slot sizes; min (distance_mm, from, to), max, center_distance (centre to centre, or from an axis) and angle"),
    CommandSpec::new("document.inspect", "Inspect Design", inspect).icon("inspect").noundo().params("measure?: bool (include body measurements)"),
    CommandSpec::new("sketch.inspect", "Inspect Sketch", sketch_inspect).icon("inspect").noundo().params("sketch?: id|name (default active)"),
    CommandSpec::new("model.edges", "List Edges", model_edges).icon("inspect").noundo().params("body: name"),
    CommandSpec::new("model.faces", "List Faces", model_faces).icon("inspect").noundo().params(
        "body: name → per face: index, name (what face.delete, sheet.convert `face`, solid.shell … take in place of a point), exact, area, centroid (not always on the face), \
         plane_normal, point (a point on the face that picks it), cylinder? ({origin, axis, radius} of a cylindrical face)",
    ),
    CommandSpec::new("model.threads", "List Threads", model_threads).icon("inspect").noundo(),
    CommandSpec::new("material.assign", "Physical Material", physical_material)
        .at("SOLID", "MODIFY")
        .icon("material")
        .params("bodies: [names]; material: name (Steel, Aluminum, ABS Plastic, …, or one made with material.create; `material.list`)"),
    CommandSpec::new("appearance.assign", "Appearance", appearance).at("SOLID", "MODIFY").icon("appearance").key("A").params(
        "bodies?: [names]; faces?: [[x,y,z] on faces] (body?: which body, default the nearest); components?: [names or ids, or occurrence names]; \
             appearance?: library name (appearance.library) | color: \"#rrggbb\" or [r, g, b] (0–255), opacity? (0–1, default 1); \
             pbr?: {metallic:0..1, roughness:0.045..1, dielectric_ior:1..3} (requires color/appearance and opacity 1); omitted/null uses legacy scalar shading. Neither appearance nor color (or color: null): clear, so the face follows its body, the body its component, the component its material",
    ),
    CommandSpec::new("appearance.library", "Appearance Library", appearance_library).icon("library")
        .noundo()
        .params("→ the built-in appearances (name, colour, opacity)"),
    CommandSpec::new("appearance.list", "List Appearances", appearance_list).icon("list")
        .noundo()
        .params("body?: name → assigned appearances; with body, its resolved look and its faces' (face index, look)"),
    CommandSpec::new("material.list", "List Materials", material_list).icon("list")
        .noundo()
        .doc_only()
        .params("→ the built-in materials, then the design's own (user: true, with their look)"),
    CommandSpec::new("engine.commands", "List Commands", commands).icon("list").noundo(),
];

pub fn measure_json(b: &solidcraft_doc::ModelBody) -> Value {
    match solidcraft_kernel::measure(&b.body) {
        Ok(m) => json!({
            "name": b.name,
            "volume_mm3": m.volume,
            "area_mm2": m.area,
            "center_of_mass": m.centroid,
            "bbox": {"min": m.bbox.min, "max": m.bbox.max},
            "faces": m.merged.faces,
            "edges": m.merged.edges,
            "vertices": m.merged.vertices,
            "face_types": m.merged.face_types,
            "kernel": {"faces": m.faces, "edges": m.edges, "vertices": m.vertices, "shells": m.shells},
        }),
        Err(e) => json!({"name": b.name, "error": e.to_string()}),
    }
}

fn measure(s: &mut Session, p: &Value) -> Result<Value> {
    if let Some(items) = p.get("items") {
        return super::measure_sel::measure_items(s, items);
    }
    let want = string_list(p, "bodies");
    let st = s.world_state();
    let selected: Vec<_> = st.bodies.iter().filter(|b| want.is_empty() || want.contains(&b.name)).collect();
    // Each body is measured on its own: on all cores, in body order.
    let mut bodies: Vec<Value> = solidcraft_kernel::map_on_threads(&selected, 16, |b| measure_json(b))
        .into_iter()
        .zip(&selected)
        .map(|(m, b)| m.unwrap_or_else(|| json!({"name": b.name, "error": "measuring this body failed"})))
        .collect();
    // Mass from each body's material.
    for (b, body) in bodies.iter_mut().zip(selected) {
        if let Some(v) = b["volume_mm3"].as_f64() {
            let source = body.definition_name();
            let rho = s.doc.density(source);
            b["material"] = json!(s.doc.materials.get(source).cloned().unwrap_or_else(|| "Default".into()));
            b["density_g_cm3"] = json!(rho);
            b["mass_g"] = json!(v * rho / 1000.0);
        }
    }
    if !want.is_empty() && bodies.len() != want.len() {
        return Err(bad("inspect.measure", "unknown body name"));
    }
    // Sheet metal keeps the edges where bends meet flat faces (as Fusion does): count its
    // B-rep faces as they are, without merging coplanar neighbours.
    let sheets = s.model.state().sheets.clone();
    for b in &mut bodies {
        if let Some(sh) = sheets.iter().find(|x| b["name"].as_str() == Some(x.body.as_str())) {
            let (df, de, dv) = sh.round_hole_correction();
            for (k, d) in [("faces", df), ("edges", de), ("vertices", dv)] {
                b[k] = json!(b["kernel"][k].as_i64().unwrap_or(0) + d);
            }
            b["sheet_metal"] = json!(true);
            // A perforation field (#732) is not cut into the B-rep: take its open area out of the volume, the mass
            // and the area (both faces minus the holes, plus the bores).
            let open = sh.perforated_area();
            if open > 0.0 {
                let v = b["volume_mm3"].as_f64().unwrap_or(0.0);
                let nv = v - open * sh.t;
                if let Some(m) = b["mass_g"].as_f64().filter(|_| v > 0.0) {
                    b["mass_g"] = json!(m * nv / v);
                }
                b["volume_mm3"] = json!(nv);
                let bores: f64 = sh
                    .perforations
                    .iter()
                    .map(|p| sh.perforation_holes(p).map_or(0.0, |h| h.len() as f64) * std::f64::consts::PI * p.diameter * sh.t)
                    .sum();
                b["area_mm2"] = json!(b["area_mm2"].as_f64().unwrap_or(0.0) - 2.0 * open + bores);
            }
        }
    }
    let tv: f64 = bodies.iter().filter_map(|b| b["volume_mm3"].as_f64()).sum();
    let ta: f64 = bodies.iter().filter_map(|b| b["area_mm2"].as_f64()).sum();
    let tf: u64 = bodies.iter().filter_map(|b| b["faces"].as_u64()).sum();
    let te: u64 = bodies.iter().filter_map(|b| b["edges"].as_u64()).sum();
    let tvx: u64 = bodies.iter().filter_map(|b| b["vertices"].as_u64()).sum();
    let tm: f64 = bodies.iter().filter_map(|b| b["mass_g"].as_f64()).sum();
    Ok(
        json!({"body_count": bodies.len(), "bodies": bodies, "total": {"volume_mm3": tv, "area_mm2": ta, "mass_g": tm, "faces": tf, "edges": te, "vertices": tvx}}),
    )
}

fn sketch_json(s: &Session, id: u64) -> Option<Value> {
    let state = s.model.state();
    let ss = state.sketch(id)?;
    // Where the sketch is in the world (its component's placement).
    let world = s.world_state();
    let plane = world.sketch(id).map(|w| w.plane).unwrap_or(ss.plane);
    let sk = &ss.sketch;
    let curves: Vec<Value> = sk
        .curves
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let det = ss.report.curve_determined.get(i).copied().unwrap_or(false);
            let pid = |k: usize| sk.points.get(k).map(|p| p.id.clone()).unwrap_or_default();
            let mut v = match c.kind {
                CurveKind::Line { a, b } => json!({"id": c.id, "type": "line", "start": pid(a), "end": pid(b), "start_at": sk.point(a), "end_at": sk.point(b), "construction": c.construction, "fixed": sk.curve_locked(i), "fully_constrained": det}),
                CurveKind::Circle { c: cc, r } => json!({"id": c.id, "type": "circle", "center": pid(cc), "center_at": sk.point(cc), "radius": r, "construction": c.construction, "fixed": sk.curve_locked(i), "fully_constrained": det}),
                CurveKind::Arc { c: cc, a, b } => json!({"id": c.id, "type": "arc", "center": pid(cc), "start": pid(a), "end": pid(b), "center_at": sk.point(cc), "start_at": sk.point(a), "end_at": sk.point(b), "radius": sk.radius(i), "construction": c.construction, "fixed": sk.curve_locked(i), "fully_constrained": det}),
                CurveKind::Ellipse { c: cc, m, r } => json!({"id": c.id, "type": "ellipse", "center": pid(cc), "major": pid(m), "center_at": sk.point(cc), "major_at": sk.point(m), "minor_radius": r, "construction": c.construction, "fixed": sk.curve_locked(i), "fully_constrained": det}),
                CurveKind::Spline { ref pts, control, degree, periodic } => json!({"id": c.id, "type": "spline", "points": pts.iter().map(|q| pid(*q)).collect::<Vec<_>>(), "points_at": pts.iter().map(|q| sk.point(*q)).collect::<Vec<_>>(), "control": control, "degree": degree, "periodic": periodic, "construction": c.construction, "fixed": sk.curve_locked(i), "fully_constrained": det}),
                CurveKind::Conic { a, b, apex, rho } => json!({"id": c.id, "type": "conic", "start": pid(a), "end": pid(b), "apex": pid(apex), "start_at": sk.point(a), "end_at": sk.point(b), "apex_at": sk.point(apex), "rho": rho, "construction": c.construction, "fixed": sk.curve_locked(i), "fully_constrained": det}),
            };
            if let (Some(l), Some(o)) = (&c.link, v.as_object_mut()) {
                o.insert("link".into(), json!(l));
            }
            v
        })
        .collect();
    let locked = sk.locked_points();
    let points: Vec<Value> = sk
        .points
        .iter()
        .enumerate()
        .map(|(i, p)| json!({"id": p.id, "at": p.pos, "world": plane.to_world(p.pos), "fixed": locked.get(i).copied().unwrap_or(false), "link": p.link, "fully_constrained": ss.report.point_determined.get(i).copied().unwrap_or(false)}))
        .collect();
    let constraints: Vec<Value> = sk
        .constraints
        .iter()
        .map(|c| {
            let mut v = serde_json::to_value(&c.kind).unwrap_or_default();
            if let Some(o) = v.as_object_mut() {
                o.insert("id".into(), json!(c.id));
                o.insert("name".into(), json!(c.kind.name()));
                if let Some(p) = &c.param {
                    o.insert("param".into(), json!(p));
                    o.insert("expression".into(), json!(s.doc.param(p).map(|x| x.expr.clone())));
                }
            }
            v
        })
        .collect();
    let profiles: Vec<Value> = ss
        .profiles
        .iter()
        .enumerate()
        .map(|(i, p)| json!({"index": i, "area_mm2": p.area, "centroid_sketch": p.centroid, "curves": p.outer_curves, "holes": p.hole_curves}))
        .collect();
    Some(json!({
        "id": id,
        "name": ss.name,
        "plane": ss.plane,
        "status": ss.report.status,
        "dof": ss.report.dof,
        "fully_constrained": ss.report.fully_constrained(),
        "failing": ss.report.failing,
        "curves": curves,
        "points": points,
        "constraints": constraints,
        "profiles": profiles,
        "links": sk.links,
        "wires": sk.wires.iter().map(|w| json!({"id": w.id, "points": w.pts.len(), "start": w.pts.first(), "end": w.pts.last(), "link": w.link})).collect::<Vec<_>>(),
        "visible": crate::view::sketch_shown(s, id),
    }))
}

fn sketch_inspect(s: &mut Session, p: &Value) -> Result<Value> {
    let id = match p.get("sketch") {
        Some(Value::Number(n)) => s.doc.find_feature(&n.to_string()).map(|f| f.id),
        Some(Value::String(x)) => s.doc.find_feature(x).map(|f| f.id),
        _ => s.active_sketch,
    }
    .ok_or_else(|| bad("sketch.inspect", "no such sketch (and none is active)"))?;
    sketch_json(s, id).ok_or_else(|| EngineError::Other("that sketch is not evaluated (rolled back or not a sketch)".into()))
}

fn inspect(s: &mut Session, p: &Value) -> Result<Value> {
    let st = s.model.state();
    let (vals, perr) = s.doc.param_values();
    let params: Vec<Value> = s
        .doc
        .params
        .iter()
        .map(|p| json!({"name": p.name, "expression": p.expr, "unit": p.unit, "value": vals.get(&p.name).map(|v| v.v), "model": p.model, "comment": p.comment, "error": perr.get(&p.name)}))
        .collect();
    let marker = s.doc.marker.unwrap_or(s.doc.features.len());
    let timeline: Vec<Value> = s
        .doc
        .features
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let r = s.model.result(f.id);
            json!({
                "id": f.id, "name": f.name, "type": f.kind.type_name(), "suppressed": f.suppressed, "rolled_back": i >= marker,
                "error": r.and_then(|r| r.error.clone()), "warning": r.and_then(|r| r.warning.clone()), "ms": r.map(|r| r.ms),
                "boolean_diagnostics": r.map(|r| &r.boolean_diagnostics),
            })
        })
        .collect();
    let with_measure = bool_(p, "measure").unwrap_or(false);
    let bodies: Vec<Value> = st
        .bodies
        .iter()
        .map(|b| {
            let mut v = if with_measure { measure_json(b) } else { json!({"name": b.name, "feature": b.feature}) };
            if let Some(o) = v.as_object_mut() {
                o.insert("visible".into(), json!(s.body_visible(&b.name)));
                if s.body_removed_later(&b.name) {
                    // Editing a sketch rolls the timeline back; this body is deleted further on.
                    o.insert("removed_later".into(), json!(true));
                }
            }
            v
        })
        .collect();
    let sketches: Vec<Value> = st
        .sketches
        .iter()
        .map(|ss| json!({"id": ss.feature, "name": ss.name, "dof": ss.report.dof, "status": ss.report.status, "curves": ss.sketch.curves.len(), "profiles": ss.profiles.len(), "visible": crate::view::sketch_shown(s, ss.feature)}))
        .collect();
    Ok(json!({
        "name": s.doc.name, "units": s.doc.units, "metadata": s.doc.metadata, "path": s.path, "dirty": s.is_dirty(), "revision": s.revision,
        "active_sketch": s.active_sketch, "marker": marker,
        "params": params, "timeline": timeline, "bodies": bodies, "sketches": sketches,
        "selection": s.selection, "undo": s.undo.len(), "redo": s.redo.len(), "hidden_origin": s.visibility.hidden_origin,
    }))
}

fn body_of<'a>(st: &'a solidcraft_doc::ModelState, p: &Value, cmd: &str) -> Result<&'a solidcraft_doc::ModelBody> {
    let n = str_(p, "body").ok_or_else(|| bad(cmd, "`body` is required"))?;
    st.body(n).ok_or_else(|| bad(cmd, format!("no body `{n}`")))
}

fn model_edges(s: &mut Session, p: &Value) -> Result<Value> {
    let st = s.world_state();
    let b = body_of(&st, p, "model.edges")?;
    let tol = (b.body.size() * 2e-3).max(1e-3);
    let names = solidcraft_doc::naming::edge_names(b);
    let edges: Vec<Value> = b
        .body
        .edges(tol)?
        .iter()
        .map(|e| json!({"index": e.index, "name": names.get(e.index), "mid": e.mid, "length": e.length, "start": e.points.first(), "end": e.points.last()}))
        .collect();
    Ok(json!({"body": b.name, "edges": edges}))
}

fn model_faces(s: &mut Session, p: &Value) -> Result<Value> {
    let st = s.world_state();
    let b = body_of(&st, p, "model.faces")?;
    let names = solidcraft_doc::naming::face_names(b);
    let tol = (b.body.size() * 2e-3).max(1e-3);
    let spots = solidcraft_doc::naming::face_spots(b);
    let faces: Vec<Value> = b
        .body
        .faces(tol)?
        .iter()
        .map(|f| {
            let spot = spots.get(f.index).copied().flatten();
            let mut v = json!({"index": f.index, "name": names.get(f.index), "exact": solidcraft_doc::naming::face_name_exact(b, f.index),
                   "area": f.area, "centroid": f.centroid, "plane_normal": f.plane_normal, "point": spot.map(|x| x.point)});
            if let (Some(solidcraft_kernel::FaceSurface::Cylinder { origin, axis, radius }), Some(o)) =
                (spot.and_then(|x| x.surface), v.as_object_mut())
            {
                o.insert("cylinder".into(), json!({"origin": origin, "axis": axis, "radius": radius}));
            }
            v
        })
        .collect();
    Ok(json!({"body": b.name, "faces": faces}))
}

fn commands(s: &mut Session, _p: &Value) -> Result<Value> {
    Ok(Value::Array(crate::command_specs().iter().map(|c| serde_json::to_value(c.info(s)).unwrap_or_default()).collect()))
}

fn model_threads(s: &mut Session, _p: &Value) -> Result<Value> {
    let st = s.world_state();
    Ok(json!({ "threads": st.threads }))
}

fn physical_material(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "material.assign";
    let bodies = string_list(p, "bodies");
    let material = str_(p, "material").ok_or_else(|| bad(cmd, "`material` must be a material name"))?;
    let canonical = s.doc.material_name(material).ok_or_else(|| bad(cmd, format!("unknown material `{material}` (see material.list)")))?;
    let st = s.model.state();
    if bodies.is_empty() || bodies.iter().any(|b| st.body(b).is_none()) {
        return Err(bad(cmd, "`bodies` must list existing bodies"));
    }
    for b in &bodies {
        if canonical == "Default" {
            s.doc_mut().materials.remove(b);
        } else {
            s.doc_mut().materials.insert(b.clone(), canonical.clone());
        }
    }
    Ok(json!({"bodies": bodies, "material": canonical}))
}

/// A colour parameter: `"#rrggbb"` or `[r, g, b]` (0–255).
pub(super) fn color_param(v: &Value) -> Option<[u8; 3]> {
    if let Some(t) = v.as_str() {
        let h = t.trim().trim_start_matches('#');
        if h.len() != 6 || !h.is_ascii() {
            return None;
        }
        let byte = |i: usize| h.get(i..i + 2).and_then(|x| u8::from_str_radix(x, 16).ok());
        return Some([byte(0)?, byte(2)?, byte(4)?]);
    }
    let a = v.as_array().filter(|a| a.len() == 3)?;
    let c = |i: usize| a.get(i).and_then(Value::as_f64).filter(|x| x.is_finite() && (0.0..=255.0).contains(x)).map(|x| x.round() as u8);
    Some([c(0)?, c(1)?, c(2)?])
}

fn appearance(s: &mut Session, p: &Value) -> Result<Value> {
    use solidcraft_doc::appearance::{FaceLook, Look, library};
    let cmd = "appearance.assign";
    let mut look: Option<Look> = match (str_(p, "appearance"), p.get("color")) {
        // A colour with a name: that colour, called that (a name not in the library is fine).
        (Some(n), Some(v)) if !v.is_null() => {
            let c = color_param(v).ok_or_else(|| bad(cmd, "`color` must be \"#rrggbb\" or [r, g, b] with values 0–255"))?;
            let o = p.get("opacity").and_then(Value::as_f64).filter(|x| x.is_finite()).unwrap_or(1.0);
            let mut l = Look::custom(c, o);
            l.name = n.chars().take(128).collect();
            Some(l)
        }
        (Some(n), _) => {
            let mut l = library(n).ok_or_else(|| bad(cmd, format!("no appearance `{n}` in the library (appearance.library)")))?;
            if let Some(o) = p.get("opacity").and_then(Value::as_f64).filter(|x| x.is_finite()) {
                l.opacity = o.clamp(0.0, 1.0);
            }
            Some(l)
        }
        (None, None | Some(Value::Null)) => None,
        (None, Some(v)) => {
            let c = color_param(v).ok_or_else(|| bad(cmd, "`color` must be \"#rrggbb\" or [r, g, b] with values 0–255"))?;
            let o = match p.get("opacity") {
                Some(o) => o.as_f64().filter(|x| x.is_finite() && (0.0..=1.0).contains(x)).ok_or_else(|| bad(cmd, "`opacity` must be 0…1"))?,
                None => 1.0,
            };
            Some(Look::custom(c, o))
        }
    };
    if let Some(l) = &mut look {
        attach_pbr(cmd, p, l)?;
    } else if p.get("pbr").is_some_and(|v| !v.is_null()) {
        return Err(bad(cmd, "`pbr` requires a color or appearance"));
    }
    let bodies = string_list(p, "bodies");
    let st = s.model.state();
    if bodies.iter().any(|b| st.body(b).is_none()) {
        return Err(bad(cmd, "`bodies` must list existing bodies"));
    }
    // Faces: a point on each, on the named body or the nearest one.
    let mut faces: Vec<(String, solidcraft_geom::Vec3)> = Vec::new();
    if let Some(list) = p.get("faces") {
        let list = list.as_array().filter(|a| a.len() <= 10_000).ok_or_else(|| bad(cmd, "`faces` must list points [x, y, z]"))?;
        for v in list {
            let pt = crate::params::vec3(v)
                .or_else(|| v.get("point").and_then(crate::params::vec3))
                .ok_or_else(|| bad(cmd, "`faces` must list points [x, y, z]"))?;
            let on = |b: &solidcraft_doc::ModelBody| solidcraft_doc::appearance::face_index_at(b, pt).is_some();
            let body = match v.get("body").and_then(Value::as_str).or_else(|| str_(p, "body")) {
                Some(n) => st.body(n).filter(|b| on(b)).map(|b| b.name.clone()),
                None => st.bodies.iter().filter(|b| on(b)).min_by(|a, b| dist_to(a, pt).total_cmp(&dist_to(b, pt))).map(|b| b.name.clone()),
            };
            faces.push((body.ok_or_else(|| bad(cmd, format!("no face at {pt:?}")))?, pt));
        }
    }
    let mut comps: Vec<u64> = Vec::new();
    for c in string_list(p, "components") {
        let id = s
            .doc
            .find_component(&c)
            .or_else(|| s.doc.occurrences.iter().find(|o| o.name == c).map(|o| o.component))
            .filter(|id| *id != 0)
            .ok_or_else(|| bad(cmd, format!("no component `{c}`")))?;
        comps.push(id);
    }
    if bodies.is_empty() && faces.is_empty() && comps.is_empty() {
        return Err(bad(cmd, "give `bodies`, `faces` or `components`"));
    }
    // Persistent names of the picked faces.
    let st_names: Vec<(String, solidcraft_geom::Vec3, Option<String>)> = faces
        .iter()
        .map(|(b, pt)| {
            let n = st.body(b).and_then(|mb| {
                solidcraft_doc::appearance::face_index_at(mb, *pt).and_then(|i| solidcraft_doc::naming::face_names(mb).get(i).cloned())
            });
            (b.clone(), *pt, n)
        })
        .collect();
    let a = &mut s.doc_mut().appearances;
    for b in &bodies {
        match &look {
            Some(l) => a.bodies.insert(b.clone(), l.clone()),
            None => a.bodies.remove(b),
        };
    }
    for (body, pt) in &faces {
        // One entry per face: a new one replaces any at the same spot.
        a.faces.retain(|f| !(f.body == *body && f.point.dist(*pt) < 1e-9));
        if let Some(l) = &look {
            if a.faces.len() >= 100_000 {
                return Err(bad(cmd, "too many face appearances"));
            }
            let name = st_names.iter().find(|(b, p, _)| b == body && p.dist(*pt) < 1e-12).and_then(|(_, _, n)| n.clone());
            a.faces.push(FaceLook { body: body.clone(), point: *pt, look: l.clone(), name, strict_name: false, certificate: None });
        }
    }
    for c in &comps {
        match &look {
            Some(l) => a.components.insert(*c, l.clone()),
            None => a.components.remove(c),
        };
    }
    // Clearing a face: drop every entry on that face, not just at the same point.
    if look.is_none() && !faces.is_empty() {
        let st = s.model.state();
        let doc = s.doc_mut();
        let picked: Vec<(String, Option<usize>, Option<String>)> = faces
            .iter()
            .map(|(b, pt)| {
                (
                    b.clone(),
                    st.body(b).and_then(|mb| solidcraft_doc::appearance::face_index_at(mb, *pt)),
                    st_names.iter().find(|(name, p, _)| name == b && p.dist(*pt) < 1e-12).and_then(|(_, _, name)| name.clone()),
                )
            })
            .collect();
        doc.appearances.faces.retain(|f| {
            let idx = st.body(&f.body).and_then(|mb| solidcraft_doc::appearance::face_index_at(mb, f.point));
            !picked.iter().any(|(b, i, name)| {
                *b == f.body
                    && (f
                        .name
                        .as_ref()
                        .zip(name.as_ref())
                        .is_some_and(|(a, b)| solidcraft_doc::naming::strip_piece(a) == solidcraft_doc::naming::strip_piece(b))
                        || (!f.strict_name && i.is_some() && *i == idx))
            })
        });
    }
    Ok(json!({
        "bodies": bodies,
        "faces": faces.iter().map(|(b, p)| json!({"body": b, "point": p})).collect::<Vec<_>>(),
        "components": comps,
        "appearance": look.as_ref().map(look_json),
    }))
}

fn dist_to(b: &solidcraft_doc::ModelBody, p: solidcraft_geom::Vec3) -> f64 {
    let bb = b.mesh().bounds();
    let c = solidcraft_geom::Vec3::new(p.x.clamp(bb.min.x, bb.max.x), p.y.clamp(bb.min.y, bb.max.y), p.z.clamp(bb.min.z, bb.max.z));
    c.dist(p)
}

pub(super) fn look_json(l: &solidcraft_doc::appearance::Look) -> Value {
    let mut value = json!({"name": l.name, "color": l.hex(), "opacity": l.opacity});
    if let Some(pbr) = l.pbr {
        value["pbr"] = json!(pbr);
    }
    value
}

#[cfg(test)]
#[path = "pbr_appearance_tests.rs"]
mod pbr_tests;

pub(super) fn attach_pbr(cmd: &str, params: &Value, look: &mut solidcraft_doc::appearance::Look) -> Result<()> {
    let Some(value) = params.get("pbr") else { return Ok(()) };
    if value.is_null() {
        look.pbr = None;
        return Ok(());
    }
    if let Some(opacity) = params.get("opacity")
        && opacity.as_f64().filter(|v| v.is_finite() && *v == 1.0).is_none()
    {
        return Err(bad(cmd, "opaque PBR requires numeric opacity 1"));
    }
    let object = value
        .as_object()
        .filter(|o| o.len() == 3 && o.keys().all(|k| matches!(k.as_str(), "metallic" | "roughness" | "dielectric_ior")))
        .ok_or_else(|| bad(cmd, "`pbr` requires exactly metallic, roughness and dielectric_ior"))?;
    let number = |key: &str| object.get(key).and_then(Value::as_f64).ok_or_else(|| bad(cmd, format!("`pbr.{key}` must be a finite number")));
    let response = solidcraft_doc::pbr_appearance::PbrParameters::new(number("metallic")?, number("roughness")?, number("dielectric_ior")?)
        .map_err(|e| bad(cmd, e.to_string()))?;
    if look.opacity != 1.0 {
        return Err(bad(cmd, "opaque PBR requires opacity 1; refraction/transmission is not supported"));
    }
    look.pbr = Some(response);
    Ok(())
}

fn appearance_library(_s: &mut Session, _p: &Value) -> Result<Value> {
    let list: Vec<Value> = solidcraft_doc::appearance::LIBRARY
        .iter()
        .map(|(n, c, o)| {
            let category = n.split(" - ").next().unwrap_or(n);
            json!({"name": n, "category": category, "color": format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]), "opacity": o})
        })
        .collect();
    Ok(json!({"appearances": list}))
}

fn appearance_list(s: &mut Session, p: &Value) -> Result<Value> {
    let a = &s.doc.appearances;
    // "In this design": each look used, with where.
    let mut used: Vec<(solidcraft_doc::appearance::Look, Vec<Value>, Vec<Value>, Vec<Value>)> = Vec::new();
    fn slot(used: &mut Vec<(solidcraft_doc::appearance::Look, Vec<Value>, Vec<Value>, Vec<Value>)>, l: &solidcraft_doc::appearance::Look) -> usize {
        match used.iter().position(|(u, ..)| u == l) {
            Some(i) => i,
            None => {
                used.push((l.clone(), Vec::new(), Vec::new(), Vec::new()));
                used.len() - 1
            }
        }
    }
    for (b, l) in &a.bodies {
        let i = slot(&mut used, l);
        if let Some(u) = used.get_mut(i) {
            u.1.push(json!(b));
        }
    }
    for f in &a.faces {
        let i = slot(&mut used, &f.look);
        if let Some(u) = used.get_mut(i) {
            u.2.push(json!({"body": f.body, "point": f.point}));
        }
    }
    for (c, l) in &a.components {
        let i = slot(&mut used, l);
        if let Some(u) = used.get_mut(i) {
            u.3.push(json!(c));
        }
    }
    let in_design: Vec<Value> = used
        .iter()
        .map(|(l, b, f, c)| {
            let mut value = look_json(l);
            value["bodies"] = json!(b);
            value["faces"] = json!(f);
            value["components"] = json!(c);
            value
        })
        .collect();
    let mut out = json!({
        "in_design": in_design,
        "bodies": a.bodies.iter().map(|(b, l)| (b.clone(), look_json(l))).collect::<serde_json::Map<_, _>>(),
        "faces": a.faces.iter().map(|f| json!({"body": f.body, "point": f.point, "appearance": look_json(&f.look)})).collect::<Vec<_>>(),
        "components": a.components.iter().map(|(c, l)| json!({"component": c, "appearance": look_json(l)})).collect::<Vec<_>>(),
    });
    if let Some(name) = str_(p, "body") {
        let st = s.model.state();
        let b = st.body(name).ok_or_else(|| bad("appearance.list", format!("no body `{name}`")))?;
        out["body"] = json!({
            "name": name,
            "appearance": s.doc.model_body_look(b).map(|l| look_json(&l)),
            "color": s.doc.body_color(b),
            "faces": s.doc.face_colors(b).iter().map(|(i, l)| json!({"face": i, "appearance": look_json(l)})).collect::<Vec<_>>(),
        });
    }
    Ok(out)
}

fn material_list(s: &mut Session, _p: &Value) -> Result<Value> {
    let built_in = solidcraft_doc::MATERIALS.iter().map(|(n, d)| json!({"name": n, "density_g_cm3": d}));
    let user = s.doc.user_materials.iter().map(
        |m| json!({"name": m.name, "density_g_cm3": m.density, "appearance": s.doc.material_look(&m.name).as_ref().map(look_json), "user": true}),
    );
    Ok(json!({"materials": built_in.chain(user).collect::<Vec<_>>()}))
}
