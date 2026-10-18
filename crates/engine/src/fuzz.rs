//! Random modelling sessions for robustness testing (`solidcraft-cli fuzz`, docs/robustness.md).
//!
//! A seeded generator builds realistic feature sequences: a base (box, extruded sketch profile
//! with an arc, revolved section), then joins and cuts of boxes, cylinders and spheres placed on
//! a coarse grid against the body (so coincident faces and tangencies come up often), fillets
//! and chamfers on random edges, shells, holes and patterns. Every step is checked: the command
//! succeeds, every body is a sound solid, and the volume moves the right way.

use serde::Serialize;
use serde_json::{Value, json};
use solidcraft_geom::Vec3;
use solidcraft_kernel::fuzz::Rng;
pub use solidcraft_kernel::fuzz::{BoolCase, boolean_case, boolean_shapes};

use crate::{EngineError, Session};

/// One step: the commands it runs (a sketch and its feature go together).
#[derive(Clone, Debug, Serialize)]
pub struct Step {
    pub kind: &'static str,
    pub commands: Vec<(String, Value)>,
    /// How the total volume must move.
    #[serde(skip)]
    expect: Expect,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Expect {
    Any,
    Grow,
    Shrink,
}

/// What happened at a step.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Outcome {
    /// "ok", "unsupported" (a feature the kernel doesn't do yet), "rejected" (the generator's
    /// parameters were refused), "failed", "panic" (one that escaped every guard), "invalid" (a
    /// body that isn't a sound solid), "volume".
    pub kind: String,
    pub detail: String,
}

impl Outcome {
    fn ok() -> Outcome {
        Outcome { kind: "ok".into(), detail: String::new() }
    }
    fn new(kind: &str, detail: impl Into<String>) -> Outcome {
        Outcome { kind: kind.into(), detail: detail.into() }
    }
    /// A failure the kernel should not have (not a known limitation or a refused input).
    pub fn is_bug(&self) -> bool {
        matches!(self.kind.as_str(), "failed" | "panic" | "invalid" | "volume")
    }
}

/// A design's run: its steps, and what happened at the last one run.
#[derive(Clone, Debug, Serialize)]
pub struct Run {
    pub seed: u64,
    pub steps: Vec<Step>,
    pub outcome: Outcome,
    /// Steps refused as not supported (kind, message): undone, not part of the design.
    pub unsupported: Vec<(String, String)>,
    /// Steps whose generated parameters a command refused.
    pub rejected: usize,
    pub boolean_fallbacks: std::collections::BTreeMap<solidcraft_kernel::BooleanStrategy, usize>,
    pub boolean_reports_truncated: bool,
}

impl Run {
    /// The commands as a script for `solidcraft-cli run`.
    pub fn script(&self) -> Value {
        Value::Array(self.steps.iter().flat_map(|s| s.commands.iter().map(|(c, p)| json!({"command": c, "params": p}))).collect())
    }
}

fn total_volume(s: &Session) -> f64 {
    s.model.state().bodies.iter().filter_map(|b| solidcraft_kernel::measure(&b.body).ok()).map(|m| m.volume).sum()
}

fn classify(e: &EngineError) -> Outcome {
    let msg = e.to_string();
    match e {
        EngineError::Internal(..) => Outcome::new("panic", msg),
        EngineError::BadParams { .. } | EngineError::Disabled(..) | EngineError::UnknownCommand(_) => Outcome::new("rejected", msg),
        // A failed boolean is a failure even when it says "not supported" (the planar fallback on
        // curved bodies): those are the cases to fix. Features the kernel doesn't do yet (a
        // fillet between curved faces) are limitations.
        // Picks the generator got wrong (a centroid off its face).
        EngineError::Other(_) if msg.starts_with("invalid input") => Outcome::new("rejected", msg),
        EngineError::Other(_) if msg.contains("boolean") => Outcome::new("failed", msg),
        EngineError::Other(_) if msg.contains("not supported") || msg.contains("only ") => Outcome::new("unsupported", msg),
        EngineError::Other(_) => Outcome::new("failed", msg),
    }
}

/// Run one step and check it.
fn apply(s: &mut Session, step: &Step) -> Outcome {
    let before = total_volume(s);
    for (c, p) in &step.commands {
        if let Err(e) = s.execute(c, p) {
            return classify(&e);
        }
    }
    let st = s.model.state();
    if let Some(f) = s.model.results.iter().find(|r| r.error.is_some()) {
        return Outcome::new("failed", format!("{}: {}", f.name, f.error.clone().unwrap_or_default()));
    }
    for b in &st.bodies {
        let bad = b.body.validity();
        if !bad.is_empty() {
            return Outcome::new("invalid", format!("{}: {}", b.name, bad.join("; ")));
        }
    }
    let after = total_volume(s);
    let slack = 1e-4 * before.max(after) + 1e-6;
    let wrong = match step.expect {
        Expect::Any => false,
        Expect::Grow => after < before - slack,
        Expect::Shrink => after > before + slack,
    };
    if wrong || !after.is_finite() {
        return Outcome::new("volume", format!("{} moved the volume the wrong way: {before:.4} → {after:.4}", step.kind));
    }
    Outcome::ok()
}

fn step(kind: &'static str, expect: Expect, commands: Vec<(&str, Value)>) -> Step {
    Step { kind, expect, commands: commands.into_iter().map(|(c, p)| (c.to_string(), p)).collect() }
}

/// The bodies' bounds (min, max).
fn bounds(s: &Session) -> Option<(Vec3, Vec3)> {
    let st = s.model.state();
    let mut it = st.bodies.iter().map(|b| b.mesh().bounds());
    let first = it.next()?;
    Some(it.fold((first.min, first.max), |(lo, hi), b| {
        (Vec3::new(lo.x.min(b.min.x), lo.y.min(b.min.y), lo.z.min(b.min.z)), Vec3::new(hi.x.max(b.max.x), hi.y.max(b.max.y), hi.z.max(b.max.z)))
    }))
}

const G: f64 = 2.5;

fn snap(x: f64) -> f64 {
    (x / G).round() * G
}

/// A coordinate in the bounds: often exactly on a side (coincident faces), else on the grid.
fn coord(r: &mut Rng, lo: f64, hi: f64) -> f64 {
    match r.below(4) {
        0 => lo,
        1 => hi,
        _ => snap(lo + (hi - lo) * r.unit()),
    }
}

fn base(r: &mut Rng) -> Step {
    match r.below(3) {
        0 => {
            let (l, w, h) = (r.grid(10.0, 60.0, G), r.grid(10.0, 60.0, G), r.grid(5.0, 40.0, G));
            step("box", Expect::Any, vec![("solid.box", json!({"length": l, "width": w, "height": h}))])
        }
        1 => {
            // A profile of lines closed by an arc, extruded.
            let (w, h) = (r.grid(15.0, 50.0, G), r.grid(10.0, 40.0, G));
            let bulge = r.grid(2.5, (w / 2.0).min(15.0), G);
            let notch = r.chance(0.5);
            let mut pts = vec![json!([0, 0]), json!([w, 0]), json!([w, h])];
            if notch {
                let nx = snap(w * 0.6);
                pts.push(json!([nx, h]));
                pts.push(json!([nx, snap(h * 0.5)]));
                pts.push(json!([snap(w * 0.3), snap(h * 0.5)]));
                pts.push(json!([snap(w * 0.3), h]));
            }
            pts.push(json!([0, h]));
            let d = r.grid(5.0, 40.0, G);
            step(
                "extruded profile",
                Expect::Any,
                vec![
                    ("sketch.create", json!({"plane": "XY"})),
                    ("sketch.line", json!({"points": pts})),
                    ("sketch.arc.three_point", json!({"start": [0, h], "end": [0, 0], "through": [-bulge, h / 2.0]})),
                    ("sketch.finish", json!({})),
                    ("solid.extrude", json!({"distance": d})),
                ],
            )
        }
        _ => {
            let (r0, r1, h) = (r.grid(0.0, 10.0, G), r.grid(12.5, 30.0, G), r.grid(5.0, 30.0, G));
            let angle = r.pick(&["360 deg", "180 deg", "90 deg", "270 deg"]).unwrap_or("360 deg");
            step(
                "revolved section",
                Expect::Any,
                vec![
                    ("sketch.create", json!({"plane": "XZ"})),
                    ("sketch.rectangle.two_point", json!({"p0": [r0, 0], "p1": [r1, h]})),
                    ("sketch.finish", json!({})),
                    ("solid.revolve", json!({"axis": "Z", "angle": angle})),
                ],
            )
        }
    }
}

fn random_point(r: &mut Rng, lo: Vec3, hi: Vec3) -> [f64; 3] {
    [coord(r, lo.x, hi.x), coord(r, lo.y, hi.y), coord(r, lo.z, hi.z)]
}

/// The next step for the design as it stands (None: nothing fits, try another).
fn next_step(r: &mut Rng, s: &Session, last_feature: &Option<String>) -> Option<Step> {
    let (lo, hi) = bounds(s)?;
    let st = s.model.state();
    let body = st.bodies.get(r.below(st.bodies.len()))?;
    let tol = (body.body.size() * 1e-3).max(1e-3);
    Some(match r.below(12) {
        0 | 1 => {
            let a = random_point(r, lo, hi);
            let size = [r.grid(2.5, 25.0, G), r.grid(2.5, 25.0, G), r.grid(2.5, 25.0, G)];
            let join = r.chance(0.5);
            let (kind, expect, op) = if join { ("join box", Expect::Grow, "join") } else { ("cut box", Expect::Shrink, "cut") };
            step(kind, expect, vec![("solid.box", json!({"corner": a, "length": size[0], "width": size[1], "height": size[2], "operation": op}))])
        }
        2 | 3 => {
            let base = random_point(r, lo, hi);
            let axis = r.pick(&[[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [0.0, 0.0, -1.0]])?;
            let join = r.chance(0.4);
            let (kind, expect, op) = if join { ("join cylinder", Expect::Grow, "join") } else { ("cut cylinder", Expect::Shrink, "cut") };
            let p = json!({"base": base, "axis": axis, "radius": r.grid(1.25, 10.0, 1.25), "height": r.grid(2.5, 30.0, G), "operation": op});
            step(kind, expect, vec![("solid.cylinder", p)])
        }
        4 => {
            let c = random_point(r, lo, hi);
            step("cut sphere", Expect::Shrink, vec![("solid.sphere", json!({"center": c, "radius": r.grid(2.5, 10.0, G), "operation": "cut"}))])
        }
        5 | 6 => {
            let edges = body.body.edges(tol).ok()?;
            let n = 1 + r.below(3);
            let picks: Vec<[f64; 3]> = (0..n).filter_map(|_| edges.get(r.below(edges.len()))).map(|e| [e.mid.x, e.mid.y, e.mid.z]).collect();
            let rad = r.pick(&[0.5, 1.0, 2.0, 2.5, 3.0])?;
            step("fillet", Expect::Any, vec![("solid.fillet", json!({"edges": picks, "radius": rad}))])
        }
        7 => {
            let edges = body.body.edges(tol).ok()?;
            let n = 1 + r.below(3);
            let picks: Vec<[f64; 3]> = (0..n).filter_map(|_| edges.get(r.below(edges.len()))).map(|e| [e.mid.x, e.mid.y, e.mid.z]).collect();
            step("chamfer", Expect::Shrink, vec![("solid.chamfer", json!({"edges": picks, "distance": r.pick(&[0.5, 1.0, 2.0])?}))])
        }
        8 => {
            let faces: Vec<_> = body.body.faces(tol).ok()?.into_iter().filter(|f| f.plane_normal.is_some()).collect();
            let f = faces.get(r.below(faces.len()))?;
            let p = [f.centroid.x, f.centroid.y, f.centroid.z];
            step("shell", Expect::Shrink, vec![("solid.shell", json!({"faces": [p], "thickness": r.pick(&[1.0, 1.5, 2.0])?}))])
        }
        9 => {
            let faces: Vec<_> = body.body.faces(tol).ok()?.into_iter().filter(|f| f.plane_normal.is_some()).collect();
            let f = faces.get(r.below(faces.len()))?;
            let p = [f.centroid.x, f.centroid.y, f.centroid.z];
            let mut h = json!({"position": p, "diameter": r.grid(2.0, 8.0, 1.0)});
            if r.chance(0.5) {
                h["depth"] = json!(r.grid(2.5, 20.0, G));
                if r.chance(0.5) {
                    h["type"] = json!("drilled");
                }
            }
            step("hole", Expect::Shrink, vec![("solid.hole", h)])
        }
        10 => {
            let name = last_feature.clone()?;
            let dir = r.pick(&[[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]])?;
            let p = json!({"features": [name], "dir1": dir, "count1": 2 + r.below(2), "spacing1": r.grid(5.0, 20.0, G)});
            step("rectangular pattern", Expect::Any, vec![("solid.pattern.rectangular", p)])
        }
        _ => {
            let name = last_feature.clone()?;
            let p = json!({"features": [name], "axis": "Z", "count": 2 + r.below(5)});
            step("circular pattern", Expect::Any, vec![("solid.pattern.circular", p)])
        }
    })
}

/// Generate and run a design of up to `max_steps` steps, stopping at the first problem. Steps
/// refused as unsupported (or with parameters the command rejects) are undone and counted.
pub fn run(seed: u64, max_steps: usize) -> Run {
    run_with(seed, max_steps, &mut |_| {})
}

/// [`run`], telling `on_step` about each step before it runs (to see where a hang happened).
pub fn run_with(seed: u64, max_steps: usize, on_step: &mut dyn FnMut(&Step)) -> Run {
    run_session(seed, max_steps, on_step).0
}

/// What a seed builds, as recorded in the robustness goldens (`tests/robustness/goldens.json`):
/// the steps, how it ended (with the start of the message when it did not end ok) and, for the
/// design it left, its bodies' total volume and area and their face, edge and shell counts.
pub fn golden(seed: u64, max_steps: usize) -> Value {
    let (run, s) = run_session(seed, max_steps, &mut |_| {});
    let st = s.model.state();
    let (mut volume, mut area, mut faces, mut edges, mut shells) = (0.0, 0.0, 0, 0, 0);
    let mut measured = true;
    for b in &st.bodies {
        match solidcraft_kernel::measure(&b.body) {
            Ok(m) => {
                volume += m.volume;
                area += m.area;
                faces += m.faces;
                edges += m.edges;
                shells += m.shells;
            }
            Err(_) => measured = false,
        }
    }
    let round = |x: f64| (x * 1e6).round() / 1e6;
    json!({
        "steps": run.steps.iter().map(|st| st.kind).collect::<Vec<_>>(),
        "outcome": run.outcome.kind,
        "error": (run.outcome.kind != "ok").then(|| gist(&run.outcome.detail)),
        "unsupported": run.unsupported.len(),
        "rejected": run.rejected,
        "bodies": st.bodies.len(),
        "volume": measured.then(|| round(volume)),
        "area": measured.then(|| round(area)),
        "faces": faces,
        "edges": edges,
        "shells": shells,
    })
}

/// [`run_with`], and the session the run left.
fn run_session(seed: u64, max_steps: usize, on_step: &mut dyn FnMut(&Step)) -> (Run, Session) {
    let mut r = Rng::new(seed);
    let mut s = Session::default();
    let mut run = Run {
        seed,
        steps: Vec::new(),
        outcome: Outcome::ok(),
        unsupported: Vec::new(),
        rejected: 0,
        boolean_fallbacks: Default::default(),
        boolean_reports_truncated: false,
    };
    let mut last_feature: Option<String> = None;
    let n = 2 + r.below(max_steps.max(2) - 1);
    let mut tries = 0;
    while run.steps.len() < n && tries < n * 4 {
        tries += 1;
        let st = if run.steps.is_empty() { Some(base(&mut r)) } else { next_step(&mut r, &s, &last_feature) };
        let Some(st) = st else { continue };
        let before = s.doc.features.len();
        on_step(&st);
        let out = apply(&mut s, &st);
        if out.kind == "rejected" || out.kind == "unsupported" {
            // Not this one: undo what it added and try another.
            while s.doc.features.len() > before {
                s.doc_mut().features.pop();
            }
            s.refresh();
            if out.kind == "rejected" {
                run.rejected += 1;
            } else {
                run.unsupported.push((st.kind.to_string(), out.detail));
            }
            continue;
        }
        let kind = st.kind;
        for result in s.model.results.iter().skip(before).filter(|result| result.error.is_none()) {
            run.boolean_reports_truncated |= result.boolean_diagnostics.truncated;
            for event in &result.boolean_diagnostics.events {
                *run.boolean_fallbacks.entry(event.strategy).or_default() += 1;
            }
        }
        run.steps.push(st);
        if out.kind != "ok" {
            run.outcome = out;
            return (run, s);
        }
        if matches!(kind, "join box" | "cut box" | "join cylinder" | "cut cylinder" | "cut sphere" | "hole") {
            last_feature = s.doc.features.last().map(|f| f.name.clone());
        }
    }
    (run, s)
}

/// Run fixed steps (a recorded or minimised design): the index and outcome of the first step
/// that isn't ok (or the last step's, ok).
pub fn replay(steps: &[Step]) -> (usize, Outcome) {
    let (index, outcome, _) = replay_session(steps);
    (index, outcome)
}

fn replay_session(steps: &[Step]) -> (usize, Outcome, Session) {
    let mut s = Session::default();
    for (i, st) in steps.iter().enumerate() {
        let out = apply(&mut s, st);
        if out.kind != "ok" {
            return (i, out, s);
        }
    }
    (steps.len().saturating_sub(1), Outcome::ok(), s)
}

/// The start of a message, to tell failures apart (numbers vary between versions of a case).
fn gist(d: &str) -> String {
    d.chars().take_while(|c| !c.is_ascii_digit()).take(60).collect()
}

/// A failing design cut down: steps are dropped while the last one still fails the same way.
pub fn minimise(run: &Run) -> Run {
    let mut steps = run.steps.clone();
    let (kind, g) = (run.outcome.kind.clone(), gist(&run.outcome.detail));
    let same = |st: &[Step]| {
        let (i, o) = replay(st);
        i + 1 == st.len() && o.kind == kind && gist(&o.detail) == g
    };
    let mut changed = true;
    while changed && steps.len() > 1 {
        changed = false;
        let mut i = 0;
        while i + 1 < steps.len() {
            let mut t = steps.clone();
            t.remove(i);
            if same(&t) {
                steps = t;
                changed = true;
            } else {
                i += 1;
            }
        }
    }
    let (_, outcome, session) = replay_session(&steps);
    let mut boolean_fallbacks = std::collections::BTreeMap::new();
    let mut boolean_reports_truncated = false;
    for result in &session.model.results {
        boolean_reports_truncated |= result.boolean_diagnostics.truncated;
        for event in &result.boolean_diagnostics.events {
            *boolean_fallbacks.entry(event.strategy).or_default() += 1;
        }
    }
    Run { seed: run.seed, steps, outcome, unsupported: Vec::new(), rejected: 0, boolean_fallbacks, boolean_reports_truncated }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[allow(clippy::expect_used)]
    fn seed_measures(seed: u64) -> Vec<solidcraft_kernel::BodyMeasure> {
        let (run, s) = run_session(seed, 8, &mut |_| {});
        assert_eq!(run.outcome.kind, "ok", "seed {seed}: {:?}", run.outcome);
        s.model.state().bodies.iter().map(|b| solidcraft_kernel::measure(&b.body).expect("measure a valid seeded body")).collect()
    }

    /// Seed 34: a quarter cylinder R15 x 12.5, a spherical bite R2.5 centred
    /// at (10, 12.5, 0), and an independent R2 x 5 blind drill with a 118° tip.
    /// The circular-lens surface/volume integrals give these references
    /// independently of the B-rep and its measurement tessellation (docs/robustness.md).
    #[test]
    fn a_spherical_bite_and_blind_drill_match_the_analytic_solid() {
        let measured = seed_measures(34);
        assert_eq!(measured.len(), 1);
        let m = &measured[0];
        for (name, got, want) in [("volume", m.volume, 2_134.404_133), ("area", m.area, 1_086.316_691)] {
            // measure uses a chordal mesh with a 5e-5 relative chord tolerance.
            assert!((got - want).abs() <= 5e-5 * want, "{name}: {got} versus analytic {want}");
        }
        assert_eq!(m.merged.face_types, BTreeMap::from([("cone".into(), 1), ("cylinder".into(), 2), ("plane".into(), 4), ("sphere".into(), 1)]));
    }

    /// Seed 67 leaves two disconnected bodies: the notched extrusion with its
    /// bores and cylindrical boss, and a chamfered block with a patterned bore.
    /// Redundant raw subdivisions must preserve their analytic surface inventory.
    #[test]
    fn a_patterned_bore_preserves_the_analytic_surfaces() {
        let measured = seed_measures(67);
        assert_eq!(measured.len(), 2);
        assert_eq!(measured.iter().map(|m| m.shells).sum::<usize>(), 2);
        let mut kinds = BTreeMap::new();
        for m in &measured {
            for (kind, count) in &m.merged.face_types {
                *kinds.entry(kind.clone()).or_insert(0) += count;
            }
        }
        assert_eq!(kinds, BTreeMap::from([("cone".into(), 1), ("cylinder".into(), 6), ("plane".into(), 22)]));
    }
}
