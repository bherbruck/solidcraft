//! Surface analyses: zebra stripes, draft angle and curvature map shading of the model's faces.
//! View state only: no geometry changes and no undo step.

use serde_json::{Value, json};
use solidcraft_geom::Vec3;

use super::CommandSpec;
use crate::params::{bad, bool_, num, str_, vec3};
use crate::{ContinuitySeam, Result, Session, SurfaceAnalysis, kernel};

/// Stations along each edge when the call does not say (`samples`).
const DEFAULT_SAMPLES: usize = 33;
/// Tolerances a seam is judged against when the call does not say: a micron of position gap, a
/// degree of normal angle, and a curvature jump of 1/100 mm.
const DEFAULT_GAP_MM: f64 = 1e-3;
const DEFAULT_ANGLE_DEG: f64 = 1.0;
const DEFAULT_CURVATURE_PER_MM: f64 = 0.01;
/// Most edges one call may pick, and most bodies one call may analyse at once.
const MAX_PICKS: usize = 512;
const MAX_BODIES: usize = 64;

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("inspect.zebra", "Zebra Analysis", zebra)
        .at("SKETCH", "INSPECT")
        .icon("zebra")
        .noundo()
        .params("stripes?: number across the view (default 12); clear?: true turns the analysis off"),
    CommandSpec::new("inspect.draft", "Draft Analysis", draft)
        .at("SKETCH", "INSPECT")
        .icon("draft_analysis")
        .noundo()
        .params("pull?: [x,y,z] direction (default Z); angle?: degrees (default 1); clear?: true turns the analysis off"),
    CommandSpec::new("inspect.environment_map", "Environment Map Analysis", |s, p| set(s, p, SurfaceAnalysis::Environment))
        .at("SKETCH", "INSPECT")
        .icon("environment_map")
        .noundo()
        .params("clear?: true turns the analysis off"),
    CommandSpec::new("inspect.accessibility", "Accessibility Analysis", accessibility)
        .at("SKETCH", "INSPECT")
        .icon("accessibility")
        .noundo()
        .params("direction?: [x,y,z] the tool comes from (default Z, from above); clear?: true turns the analysis off"),
    CommandSpec::new("inspect.curvature_map", "Curvature Map Analysis", curvature_map)
        .at("SKETCH", "INSPECT")
        .icon("curvature_map")
        .noundo()
        .params("radius?: mm shown red (default: a tenth of the model's size); clear?: true turns the analysis off"),
    CommandSpec::new("inspect.continuity", "Continuity Analysis", continuity).at("SURFACE", "INSPECT").icon("continuity").noundo().params(
        "body?: name (default: the visible bodies); edges?: [x,y,z] points or {\"index\": i} (default: every edge of the body; \
             a pick must match one body); samples?: 3…257 stations along each edge, ends included (default 33); \
             gap_tolerance_mm? (default 0.001), angle_tolerance_deg? (default 1), curvature_tolerance_per_mm? (default 0.01); \
             add?: true keeps the seams already shaded and adds these (measuring an edge again replaces it); \
             clear?: true turns the analysis off. → each edge's worst gap (G0), normal angle (G1) and curvature jump (G2) as \
             {value, at: [x,y,z], edge_fraction}, its verdict (smooth, curvature_jump, sharp or gap) and a summary; \
             the seams are shaded in the viewport",
    ),
];

fn set(s: &mut Session, p: &Value, a: SurfaceAnalysis) -> Result<Value> {
    if bool_(p, "clear").unwrap_or(false) {
        s.analysis = None;
    } else {
        s.analysis = Some(a);
    }
    s.revision += 1;
    Ok(json!({ "analysis": s.analysis }))
}

fn zebra(s: &mut Session, p: &Value) -> Result<Value> {
    let stripes = num(p, "stripes").unwrap_or(12.0);
    if !(stripes.is_finite() && (1.0..=200.0).contains(&stripes)) {
        return Err(bad("inspect.zebra", "`stripes` must be 1…200"));
    }
    set(s, p, SurfaceAnalysis::Zebra { stripes })
}

fn draft(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "inspect.draft";
    let pull = match p.get("pull") {
        Some(v) => vec3(v).and_then(Vec3::normalized).ok_or_else(|| bad(cmd, "`pull` must be a non-zero [x,y,z]"))?,
        None => Vec3::Z,
    };
    let angle = num(p, "angle").unwrap_or(1.0);
    if !(angle.is_finite() && (0.0..90.0).contains(&angle)) {
        return Err(bad(cmd, "`angle` must be 0…90 degrees"));
    }
    set(s, p, SurfaceAnalysis::Draft { pull, angle })
}

fn accessibility(s: &mut Session, p: &Value) -> Result<Value> {
    let dir = match p.get("direction") {
        Some(v) => vec3(v).and_then(Vec3::normalized).ok_or_else(|| bad("inspect.accessibility", "`direction` must be a non-zero [x,y,z]"))?,
        None => Vec3::Z,
    };
    set(s, p, SurfaceAnalysis::Access { dir })
}

fn curvature_map(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "inspect.curvature_map";
    let radius = match num(p, "radius") {
        Some(r) if r.is_finite() && r > 1e-6 && r < 1e9 => r,
        Some(_) => return Err(bad(cmd, "`radius` must be positive")),
        None => {
            let st = s.model.state();
            let size = st.bodies.iter().map(|b| b.mesh().bounds().diagonal()).fold(0.0_f64, f64::max);
            if size > 0.0 { size / 10.0 } else { 10.0 }
        }
    };
    set(s, p, SurfaceAnalysis::Curvature { radius })
}

/// `inspect.continuity`: measure how far the two faces meeting at each picked edge (or at every
/// edge of a body) depart from sharing a curve (G0), a tangent plane (G1) and a curvature (G2),
/// and keep the seams so the viewport can shade them. View state only: the design and its undo
/// history are untouched (`noundo`), as with the other analyses.
fn continuity(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "inspect.continuity";
    if bool_(p, "clear").unwrap_or(false) {
        s.continuity.clear();
        s.revision += 1;
        return Ok(json!({ "analysis": "continuity", "cleared": true }));
    }
    let samples = match p.get("samples") {
        None => DEFAULT_SAMPLES,
        Some(v) => match v.as_f64().filter(|n| n.is_finite() && n.fract() == 0.0 && (3.0..=257.0).contains(n)) {
            Some(n) => n as usize,
            None => return Err(bad(cmd, "`samples` must be 3…257 stations along each edge")),
        },
    };
    let tolerances = kernel::ContinuityTolerances {
        gap_mm: tolerance(p, "gap_tolerance_mm", DEFAULT_GAP_MM)?,
        angle_deg: tolerance(p, "angle_tolerance_deg", DEFAULT_ANGLE_DEG)?,
        curvature_per_mm: tolerance(p, "curvature_tolerance_per_mm", DEFAULT_CURVATURE_PER_MM)?,
    };
    let picks = edge_picks(p)?;
    let st = s.world_state();
    let named = str_(p, "body");
    let candidates: Vec<&solidcraft_doc::ModelBody> = match named {
        Some(name) => vec![st.body(name).ok_or_else(|| bad(cmd, format!("no body `{name}`")))?],
        None => st.bodies.iter().filter(|b| s.body_visible(&b.name)).collect(),
    };
    if candidates.is_empty() {
        return Err(bad(cmd, "there are no visible bodies to analyse: name a `body`"));
    }
    if candidates.len() > MAX_BODIES {
        return Err(bad(cmd, format!("more than {MAX_BODIES} bodies are shown: name the `body` to analyse")));
    }
    let mut targets: Vec<(&solidcraft_doc::ModelBody, Vec<usize>)> = Vec::new();
    if picks.is_empty() {
        targets.extend(candidates.into_iter().map(|b| (b, Vec::new())));
    } else {
        // Picks mean edges of one body (a design may stack bodies at the same place): a list
        // that lands on one body's edges only is analysed, and anything else is refused.
        let mut hits = Vec::new();
        let mut refused = None;
        for b in candidates {
            match kernel::pick_edges(&b.body, &picks) {
                Ok(edges) => hits.push((b, edges)),
                Err(e) => refused = refused.or(Some(e)),
            }
        }
        if hits.is_empty() {
            return Err(match (named, refused) {
                (Some(_), Some(e)) => e.into(),
                _ => bad(cmd, "no visible body has an edge matching `edges`"),
            });
        }
        if hits.len() > 1 {
            return Err(bad(cmd, "`edges` matches edges of more than one body: name the `body` to analyse"));
        }
        targets.extend(hits);
    }
    let mut seams: Vec<ContinuitySeam> = Vec::new();
    let mut bodies = Vec::new();
    let mut summary = Summary::default();
    for (body, edges) in &targets {
        let report = kernel::seam_diagnostics(&body.body, edges, samples, tolerances)?;
        let mut rows = Vec::new();
        for seam in &report.seams {
            summary.count(seam.verdict);
            seams.push(ContinuitySeam { body: body.name.clone(), edge: seam.edge, points: seam.points.clone(), verdict: seam.verdict });
            rows.push(json!({
                "edge": seam.edge,
                "length_mm": seam.length,
                "samples": seam.samples,
                "gap_mm": seam.gap_mm,
                "normal_angle_deg": seam.normal_angle_deg,
                "curvature_jump_per_mm": seam.curvature_jump_per_mm,
                "verdict": seam.verdict,
            }));
        }
        summary.skipped += report.skipped;
        summary.unmeasured += report.unmeasured;
        bodies.push(json!({
            "body": body.name,
            "edges": rows,
            "skipped": report.skipped,
            "unmeasured": report.unmeasured,
        }));
    }
    if bool_(p, "add").unwrap_or(false) {
        // The seams shown stay; an edge measured again takes its new reading.
        s.continuity.retain(|kept| !seams.iter().any(|new| new.body == kept.body && new.edge == kept.edge));
        s.continuity.extend(seams);
    } else {
        s.continuity = seams;
    }
    s.revision += 1;
    Ok(json!({
        "analysis": "continuity",
        "samples": samples,
        "tolerances": {
            "gap_mm": tolerances.gap_mm,
            "angle_deg": tolerances.angle_deg,
            "curvature_per_mm": tolerances.curvature_per_mm,
        },
        "bodies": bodies,
        "summary": {
            "edges": summary.edges,
            "smooth": summary.smooth,
            "curvature_jump": summary.curvature_jump,
            "sharp": summary.sharp,
            "gap": summary.gap,
            "skipped": summary.skipped,
            "unmeasured": summary.unmeasured,
            "worst": summary.worst(),
        },
    }))
}

/// A tolerance parameter. A value that is given but unusable (NaN, infinity, negative, a
/// string, a bare `null`) is refused: `num` would fold it into "not given".
fn tolerance(p: &Value, key: &str, default: f64) -> Result<f64> {
    match p.get(key) {
        None => Ok(default),
        Some(v) => match v.as_f64() {
            Some(x) if x.is_finite() && (0.0..1e9).contains(&x) => Ok(x),
            _ => Err(bad("inspect.continuity", format!("`{key}` must be a finite number, 0 or more"))),
        },
    }
}

/// The `edges` parameter: points on the edges to measure, or `{"index": i}` entries in the
/// display mesh's edge order. Absent (or an empty list) means every edge of the body.
fn edge_picks(p: &Value) -> Result<Vec<kernel::EdgePick>> {
    let Some(value) = p.get("edges") else { return Ok(Vec::new()) };
    let Some(list) = value.as_array() else {
        return Err(bad("inspect.continuity", "`edges` must be a list of [x,y,z] points or {\"index\": i} entries"));
    };
    if list.len() > MAX_PICKS {
        return Err(bad("inspect.continuity", format!("pick at most {MAX_PICKS} edges")));
    }
    let mut out = Vec::new();
    for item in list {
        if let Some(index) = item.get("index").and_then(Value::as_u64) {
            let index = usize::try_from(index).map_err(|_| bad("inspect.continuity", "`index` is too large"))?;
            out.push(kernel::EdgePick::Index(index));
        } else if let Some(point) = vec3(item) {
            out.push(kernel::EdgePick::Point(point));
        } else {
            return Err(bad("inspect.continuity", "each `edges` entry is a point [x,y,z] or {\"index\": i}"));
        }
    }
    Ok(out)
}

/// Counts of a continuity reply, and the worst verdict over every edge measured.
#[derive(Clone, Copy, Debug, Default)]
struct Summary {
    edges: usize,
    smooth: usize,
    curvature_jump: usize,
    sharp: usize,
    gap: usize,
    skipped: usize,
    unmeasured: usize,
}

impl Summary {
    fn count(&mut self, verdict: kernel::SeamVerdict) {
        self.edges += 1;
        match verdict {
            kernel::SeamVerdict::Smooth => self.smooth += 1,
            kernel::SeamVerdict::CurvatureJump => self.curvature_jump += 1,
            kernel::SeamVerdict::Sharp => self.sharp += 1,
            kernel::SeamVerdict::Gap => self.gap += 1,
        }
    }

    /// The worst verdict over the edges measured (`None` when there was nothing to measure).
    /// [`kernel::SeamVerdict`] runs from the best continuity to the worst.
    fn worst(&self) -> Option<kernel::SeamVerdict> {
        [
            (self.smooth, kernel::SeamVerdict::Smooth),
            (self.curvature_jump, kernel::SeamVerdict::CurvatureJump),
            (self.sharp, kernel::SeamVerdict::Sharp),
            (self.gap, kernel::SeamVerdict::Gap),
        ]
        .into_iter()
        .filter(|(count, _)| *count > 0)
        .map(|(_, verdict)| verdict)
        .max()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use crate::{Session, SurfaceAnalysis};

    fn boxed(s: &mut Session, size: f64) {
        s.execute("solid.box", &json!({"length": size, "width": size, "height": size})).unwrap();
    }

    /// The edges of the first body of a continuity reply.
    fn edges(v: &Value) -> &Vec<Value> {
        v["bodies"][0]["edges"].as_array().unwrap_or_else(|| panic!("no edges in {v}"))
    }

    /// The seams that read as tangent (within 0.01° of the neighbouring face).
    fn tangent(edges: &[Value]) -> Vec<&Value> {
        edges.iter().filter(|e| e["normal_angle_deg"]["value"].as_f64().unwrap_or(f64::NAN) < 0.01).collect()
    }

    #[test]
    fn surface_analyses_are_view_state() {
        let mut s = Session::default();
        s.execute("solid.box", &json!({"length": 10, "width": 10, "height": 10})).unwrap();
        let undo = s.undo.len();
        s.execute("inspect.zebra", &json!({"stripes": 20})).unwrap();
        assert_eq!(s.analysis, Some(SurfaceAnalysis::Zebra { stripes: 20.0 }));
        s.execute("inspect.draft", &json!({"pull": [0, 0, 2], "angle": 3})).unwrap();
        assert!(matches!(s.analysis, Some(SurfaceAnalysis::Draft { angle, .. }) if angle == 3.0));
        s.execute("inspect.curvature_map", &json!({})).unwrap();
        assert!(matches!(s.analysis, Some(SurfaceAnalysis::Curvature { radius }) if (radius - 300f64.sqrt() / 10.0).abs() < 1e-6));
        s.execute("inspect.environment_map", &json!({})).unwrap();
        assert_eq!(s.analysis, Some(SurfaceAnalysis::Environment));
        s.execute("inspect.accessibility", &json!({"direction": [0, 0, -3]})).unwrap();
        assert_eq!(s.analysis, Some(SurfaceAnalysis::Access { dir: solidcraft_geom::Vec3::new(0.0, 0.0, -1.0) }));
        s.execute("inspect.curvature_map", &json!({"clear": true})).unwrap();
        assert_eq!(s.analysis, None);
        assert_eq!(s.undo.len(), undo, "no undo steps");
        assert!(s.execute("inspect.zebra", &json!({"stripes": 0})).is_err());
        assert!(s.execute("inspect.draft", &json!({"pull": [0, 0, 0]})).is_err());
    }

    /// A box edge: the faces meet exactly (G0) and square to each other (G1), so the analysis
    /// must read 0 gap and 90°, and must keep its seams as view state without an undo step.
    #[test]
    fn continuity_reports_a_box_edge_as_a_gap_free_right_angle() {
        let mut s = Session::default();
        boxed(&mut s, 20.0);
        let undo = s.undo.len();
        let v = s.execute("inspect.continuity", &json!({})).unwrap();
        assert_eq!(edges(&v).len(), 12);
        for e in edges(&v) {
            let gap = e["gap_mm"]["value"].as_f64().unwrap();
            let angle = e["normal_angle_deg"]["value"].as_f64().unwrap();
            let jump = e["curvature_jump_per_mm"]["value"].as_f64().unwrap();
            assert!(gap < 1e-6, "gap {gap} in {e}");
            assert!((angle - 90.0).abs() < 1e-9, "angle {angle} in {e}");
            assert!(jump < 1e-9, "curvature jump {jump} in {e}");
            assert_eq!(e["verdict"], json!("sharp"), "{e}");
            assert_eq!(e["samples"], json!(33), "{e}");
            assert!((e["length_mm"].as_f64().unwrap() - 20.0).abs() < 1e-9, "{e}");
            assert_eq!(e["normal_angle_deg"]["at"].as_array().map(Vec::len), Some(3), "{e}");
            let f = e["normal_angle_deg"]["edge_fraction"].as_f64().unwrap();
            assert!((0.0..=1.0).contains(&f), "{e}");
        }
        assert_eq!(v["analysis"], json!("continuity"));
        assert_eq!(v["samples"], json!(33));
        assert_eq!(v["tolerances"], json!({"gap_mm": 1e-3, "angle_deg": 1.0, "curvature_per_mm": 0.01}));
        assert_eq!(v["summary"]["edges"], json!(12));
        assert_eq!(v["summary"]["sharp"], json!(12));
        assert_eq!(v["summary"]["worst"], json!("sharp"));
        assert_eq!(s.continuity.len(), 12, "the seams are kept for the viewport");
        assert_eq!(s.undo.len(), undo, "no undo step");
        assert_eq!(s.analysis, None, "the other analyses are left alone");
    }

    /// A rolling-ball fillet is tangent to the faces it joins but not curvature continuous: its
    /// two seams read ~0° with the fillet's 1 / radius curvature jump against the planes.
    #[test]
    fn continuity_finds_the_fillet_seams_tangent_and_their_curvature_jump() {
        let mut s = Session::default();
        boxed(&mut s, 20.0);
        s.execute("solid.fillet", &json!({"edges": [[20, 20, 10]], "radius": 4})).unwrap();
        let v = s.execute("inspect.continuity", &json!({})).unwrap();
        assert_eq!(edges(&v).len(), 15, "the rounded edge leaves three new edges behind: {v}");
        let seams = tangent(edges(&v));
        assert_eq!(seams.len(), 2, "the fillet meets its two neighbours tangentially: {v}");
        for e in seams {
            assert!(e["gap_mm"]["value"].as_f64().unwrap() < 1e-6, "{e}");
            let jump = e["curvature_jump_per_mm"]["value"].as_f64().unwrap();
            assert!((jump - 0.25).abs() < 1e-6, "1 / radius 4: {e}");
            assert_eq!(e["verdict"], json!("curvature_jump"), "{e}");
        }
        assert_eq!(v["summary"]["curvature_jump"], json!(2));
        assert_eq!(v["summary"]["sharp"], json!(13));
        assert_eq!(v["summary"]["smooth"], json!(0));
    }

    /// A slot's straight wall runs tangentially into its arc wall: the join is G1 but its
    /// curvature jumps by the arc's 1 / radius, which is exactly what a Class-A check looks for.
    #[test]
    fn continuity_reports_one_over_the_radius_where_a_line_meets_an_arc() {
        let mut s = Session::default();
        s.execute("sketch.create", &json!({"plane": "XY"})).unwrap();
        s.execute("sketch.slot.center_point", &json!({"center": [0, 0], "end": [20, 0], "width": 10})).unwrap();
        s.execute("solid.extrude", &json!({"distance": 5})).unwrap();
        let v = s.execute("inspect.continuity", &json!({"angle_tolerance_deg": 0.01})).unwrap();
        let seams = tangent(edges(&v));
        assert_eq!(seams.len(), 4, "a slot has four line-to-arc joins: {v}");
        for e in seams {
            assert!(e["gap_mm"]["value"].as_f64().unwrap() < 1e-6, "{e}");
            let jump = e["curvature_jump_per_mm"]["value"].as_f64().unwrap();
            assert!((jump - 0.2).abs() < 1e-6, "1 / radius 5: {e}");
            assert_eq!(e["verdict"], json!("curvature_jump"), "{e}");
        }
        assert_eq!(v["summary"]["curvature_jump"], json!(4));
    }

    /// `clear` takes the shading away, and every continuity call stays view state.
    #[test]
    fn continuity_is_view_state_and_clears() {
        let mut s = Session::default();
        boxed(&mut s, 10.0);
        let undo = s.undo.len();
        s.execute("inspect.continuity", &json!({"samples": 5})).unwrap();
        assert_eq!(s.continuity.len(), 12);
        assert_eq!(s.undo.len(), undo, "no undo step");
        let cleared = s.execute("inspect.continuity", &json!({"clear": true})).unwrap();
        assert_eq!(cleared["cleared"], json!(true));
        assert!(s.continuity.is_empty(), "clear leaves nothing shaded");
        assert_eq!(s.undo.len(), undo);
    }

    /// `add` keeps what is shaded and adds the picked edges: an edge measured again replaces its
    /// seam instead of drawing it twice, and a call without `add` starts over.
    #[test]
    fn continuity_add_keeps_the_seams_shown() {
        let mut s = Session::default();
        boxed(&mut s, 10.0);
        s.execute("inspect.continuity", &json!({"edges": [[5, 0, 0]]})).unwrap();
        assert_eq!(s.continuity.len(), 1);
        let v = s.execute("inspect.continuity", &json!({"edges": [[0, 5, 0]], "add": true})).unwrap();
        assert_eq!(edges(&v).len(), 1, "the reply covers the edges this call measured: {v}");
        assert_eq!(s.continuity.len(), 2, "the first seam stays");
        s.execute("inspect.continuity", &json!({"edges": [[5, 0, 0]], "add": true, "samples": 5})).unwrap();
        assert_eq!(s.continuity.len(), 2, "measured again, not added twice");
        assert!(s.continuity.iter().any(|seam| seam.points.len() == 5), "with its new reading");
        assert!(s.continuity.iter().all(|seam| seam.body == "Body1"), "{:?}", s.continuity);
        s.execute("inspect.continuity", &json!({"edges": [[10, 5, 0]]})).unwrap();
        assert_eq!(s.continuity.len(), 1, "without `add` the shading starts over");
        s.execute("inspect.continuity", &json!({"add": true})).unwrap();
        assert_eq!(s.continuity.len(), 12, "every edge once");
    }

    /// Hostile parameters are refused with an error, never a panic.
    #[test]
    fn hostile_continuity_params_are_refused() {
        let mut s = Session::default();
        boxed(&mut s, 10.0);
        for params in [
            json!({"samples": 0}),
            json!({"samples": 2}),
            json!({"samples": 258}),
            json!({"samples": 3.5}),
            json!({"samples": "33"}),
            json!({"samples": null}),
            json!({"gap_tolerance_mm": -1}),
            json!({"gap_tolerance_mm": 1e12}),
            json!({"angle_tolerance_deg": f64::NAN}),
            json!({"curvature_tolerance_per_mm": "tight"}),
            json!({"edges": 3}),
            json!({"edges": [[1, 2]]}),
            json!({"edges": [[]]}),
            json!({"edges": [{"index": 99999}]}),
            json!({"edges": [[500, 500, 500]]}),
            json!({"body": "Nope"}),
        ] {
            assert!(s.execute("inspect.continuity", &params).is_err(), "accepted {params}");
        }
        let many: Vec<Value> = (0..513).map(|i| json!([i as f64, 0.0, 0.0])).collect();
        assert!(s.execute("inspect.continuity", &json!({"edges": many})).is_err(), "513 picks");
        assert!(s.continuity.is_empty(), "a refused call shades nothing");
        assert_eq!(s.undo.len(), 1, "only the box is an undo step");
    }
}
