//! Clearance (`inspect.clearance`): in one call, which bodies overlap and by how much, which
//! come closer than a clearance and where, and each body's nearest neighbours. Pairs are pruned
//! by bounding boxes grown by the clearance, then by the bodies' meshes (which lie within their
//! chord tolerance of the faces); distances are measured on the exact faces and edges
//! ([`solidcraft_kernel::body_distance`]) and overlaps by intersecting the bodies. The pairs
//! that are left run in parallel.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use serde_json::{Value, json};
use solidcraft_geom::mesh_distance::box_gap;
use solidcraft_geom::{Aabb3, Mesh};
use solidcraft_kernel::{BodyDistance, DistanceShape};

use super::CommandSpec;
use crate::params::{bad, num, str_, string_list};
use crate::{Result, Session};

pub static COMMANDS: &[CommandSpec] = &[CommandSpec::new("inspect.clearance", "Clearance", clearance).icon("interference").noundo().params(
    "bodies?: [names] (default: every solid body, in world placement, but those in `against`); against?: [names] (only pairs with one body in `bodies` and the other here); \
     min_volume?: mm³ (default 0.01: smaller overlaps count as contact); clearance?: mm (default 0: report pairs closer than this); \
     nearest?: n (each body of `bodies` gets its n nearest others); skip_within?: \"occurrence\" (pairs of bodies of one occurrence) | \
     \"component\" (pairs of bodies inside one placed component, its subcomponents included) | null; tolerance?: mm (default 1e-6: a gap \
     below it is contact, 0) → overlaps: [{a, b, volume_mm3, area_mm2, bbox, center}]; near: [{a, b, distance_mm, from, to, overlap_mm3?: \
     a contact's shared volume below min_volume, exact?: false when measured on the meshes}], nearest by body (the same for pairs that overlap: 0); \
     nearest: {body: [{body, distance_mm}]}; unchecked: [{a, b, reason}] (pairs whose overlap could not be worked out); pairs_checked; \
     pairs_pruned (ruled out by their boxes); ms. Distances are measured on the exact faces and edges, the meshes only rule pairs out",
)];

/// Most bodies one call takes.
const MAX_BODIES: usize = 20_000;
/// Most pairs whose grown boxes meet that one call measures.
const MAX_PAIRS: usize = 200_000;
/// Most neighbours a body's `nearest` list holds.
const MAX_NEAREST: usize = 1000;
/// Largest clearance (mm).
const MAX_CLEARANCE: f64 = 1e6;

#[derive(Clone, Copy, PartialEq, Eq)]
enum SkipWithin {
    Nothing,
    Occurrence,
    Component,
}

/// What one pair came to.
#[derive(Default)]
struct PairResult {
    /// Shared volume, area, box and centre.
    overlap: Option<(f64, f64, Aabb3, solidcraft_geom::Vec3)>,
    distance: Option<BodyDistance>,
    unchecked: Option<String>,
}

fn clearance(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "inspect.clearance";
    let started = web_time::Instant::now();
    let param = |k: &str, default: f64, max: f64| -> Result<f64> {
        match p.get(k) {
            None | Some(Value::Null) => Ok(default),
            Some(_) => match num(p, k) {
                Some(v) if (0.0..=max).contains(&v) => Ok(v),
                _ => Err(bad(cmd, format!("`{k}` must be a number from 0 to {max}"))),
            },
        }
    };
    let min_volume = param("min_volume", 0.01, f64::MAX)?;
    let gap = param("clearance", 0.0, MAX_CLEARANCE)?;
    let tolerance = param("tolerance", 1e-6, 1.0)?;
    let nearest = param("nearest", 0.0, MAX_NEAREST as f64)?.floor() as usize;
    let skip = match p.get("skip_within") {
        None | Some(Value::Null) => SkipWithin::Nothing,
        Some(_) => match str_(p, "skip_within") {
            Some("occurrence") => SkipWithin::Occurrence,
            Some("component") => SkipWithin::Component,
            _ => return Err(bad(cmd, "`skip_within` must be \"occurrence\", \"component\" or null")),
        },
    };

    let st = s.world_state();
    let solid: Vec<&solidcraft_doc::ModelBody> = st.bodies.iter().filter(|b| !b.body.is_mesh()).collect();
    let named = |key: &str| -> Result<Vec<usize>> {
        let names = string_list(p, key);
        let mut out = Vec::new();
        for n in &names {
            let i = solid.iter().position(|b| &b.name == n).ok_or_else(|| bad(cmd, format!("no solid body `{n}`")))?;
            if !out.contains(&i) {
                out.push(i);
            }
        }
        Ok(out)
    };
    let against = if p.get("against").is_none_or(Value::is_null) { None } else { Some(named("against")?) };
    // Without `bodies`: every body (every body not in `against`, when it is given).
    let subjects = if p.get("bodies").is_none_or(Value::is_null) {
        (0..solid.len()).filter(|i| against.as_ref().is_none_or(|a| !a.contains(i))).collect()
    } else {
        named("bodies")?
    };
    if solid.len() > MAX_BODIES {
        return Err(bad(cmd, format!("at most {MAX_BODIES} bodies")));
    }
    let n = solid.len();
    let mut in_subjects = vec![false; n];
    for &i in &subjects {
        if let Some(x) = in_subjects.get_mut(i) {
            *x = true;
        }
    }
    let mut in_against = vec![false; n];
    for &i in against.iter().flatten() {
        if let Some(x) = in_against.get_mut(i) {
            *x = true;
        }
    }
    let used: Vec<bool> = (0..n).map(|i| in_subjects.get(i).copied().unwrap_or(false) || in_against.get(i).copied().unwrap_or(false)).collect();

    // The occurrence path of each body, for `skip_within`.
    let paths: HashMap<String, Vec<u64>> = s.doc.body_frames(&s.model.state()).into_iter().map(|(name, _, path, _)| (name, path)).collect();
    let path_by_body: Vec<&[u64]> = solid.iter().map(|b| paths.get(&b.name).map(Vec::as_slice).unwrap_or(&[])).collect();
    let path_of = |i: usize| path_by_body.get(i).copied().unwrap_or(&[]);
    let flag = |v: &[bool], i: usize| v.get(i).copied().unwrap_or(false);
    let allowed = |i: usize, j: usize| -> bool {
        if i == j {
            return false;
        }
        let paired = match &against {
            None => flag(&in_subjects, i) && flag(&in_subjects, j),
            Some(_) => (flag(&in_subjects, i) && flag(&in_against, j)) || (flag(&in_subjects, j) && flag(&in_against, i)),
        };
        if !paired {
            return false;
        }
        let (a, b) = (path_of(i), path_of(j));
        match skip {
            SkipWithin::Nothing => true,
            SkipWithin::Occurrence => a.is_empty() || a != b,
            SkipWithin::Component => a.is_empty() || a.first() != b.first(),
        }
    };

    // Meshes (made once, in parallel) and the distance shapes over them.
    let idx: Vec<usize> = (0..n).filter(|&i| flag(&used, i)).collect();
    let meshes: Vec<Option<Arc<Mesh>>> = {
        let made = solidcraft_kernel::map_with_local_pools(&idx, usize::MAX, |&i| Ok(solid.get(i).map(|b| b.mesh())));
        let mut out = vec![None; n];
        for (&i, m) in idx.iter().zip(made) {
            if let (Some(slot), Ok(m)) = (out.get_mut(i), m) {
                *slot = m;
            }
        }
        out
    };
    let shapes: Vec<Option<DistanceShape>> = (0..n)
        .map(|i| match (solid.get(i), meshes.get(i).and_then(Option::as_ref)) {
            (Some(b), Some(m)) if flag(&used, i) => Some(DistanceShape::new(&b.body, m, b.display_tol())),
            _ => None,
        })
        .collect();
    let shape = |i: usize| shapes.get(i).and_then(Option::as_ref);
    let boxes: Vec<Aabb3> = (0..n).map(|i| shape(i).map_or(Aabb3::EMPTY, DistanceShape::bounds)).collect();

    // Pairs whose boxes, grown by half the clearance each, meet.
    let half = solidcraft_geom::Vec3::new(1.0, 1.0, 1.0) * (gap / 2.0);
    let grown: Vec<Aabb3> = boxes.iter().map(|b| if b.is_empty() { *b } else { Aabb3 { min: b.min - half, max: b.max + half } }).collect();
    let (close, truncated) = super::joints::overlapping_pairs(&grown, MAX_PAIRS);
    let close: Vec<(usize, usize)> = close.into_iter().filter(|&(i, j)| allowed(i, j) && shape(i).is_some() && shape(j).is_some()).collect();
    let mut total = 0usize;
    for i in 0..n {
        for j in i + 1..n {
            if allowed(i, j) {
                total += 1;
            }
        }
    }

    let results = solidcraft_kernel::map_with_local_pools(&close, usize::MAX, |&(i, j)| {
        let (Some(a), Some(b)) = (shape(i), shape(j)) else { return Ok(PairResult::default()) };
        let (Some(ba), Some(bb)) = (solid.get(i), solid.get(j)) else { return Ok(PairResult::default()) };
        Ok(measure_pair(ba, a, bb, b, gap, min_volume, tolerance))
    });

    let mut known: HashMap<(usize, usize), f64> = HashMap::new();
    let (mut overlaps, mut near, mut unchecked) = (Vec::new(), Vec::new(), Vec::new());
    let name = |i: usize| solid.get(i).map_or("", |b| b.name.as_str());
    for (&(i, j), r) in close.iter().zip(results) {
        let r = match r {
            Ok(r) => r,
            Err(e) => {
                unchecked.push(json!({"a": name(i), "b": name(j), "reason": e.to_string()}));
                continue;
            }
        };
        if let Some(reason) = r.unchecked {
            unchecked.push(json!({"a": name(i), "b": name(j), "reason": reason}));
            continue;
        }
        if let Some((volume, area, bbox, center)) = r.overlap
            && volume >= min_volume
        {
            overlaps.push(json!({"a": name(i), "b": name(j), "volume_mm3": volume, "area_mm2": area,
                "bbox": {"min": bbox.min, "max": bbox.max}, "center": center}));
            known.insert((i, j), 0.0);
            continue;
        }
        let Some(d) = r.distance else { continue };
        known.insert((i, j), d.distance);
        if d.distance < gap {
            let mut e = json!({"a": name(i), "b": name(j), "distance_mm": d.distance, "from": d.from, "to": d.to});
            if let Some((v, ..)) = r.overlap.filter(|o| o.0 > 0.0) {
                e["overlap_mm3"] = json!(v);
            }
            if !d.exact {
                e["exact"] = json!(false);
            }
            near.push(e);
        }
    }
    near.sort_by(|x, y| x["distance_mm"].as_f64().unwrap_or(0.0).total_cmp(&y["distance_mm"].as_f64().unwrap_or(0.0)));

    let mut nearest_out = serde_json::Map::new();
    if nearest > 0 {
        let lists = nearest_lists(&subjects, n, nearest, &boxes, &allowed, &mut known, |pairs| {
            solidcraft_kernel::map_with_local_pools(pairs, usize::MAX, |&(i, j)| {
                let (Some(a), Some(b)) = (shape(i), shape(j)) else { return Ok(None) };
                Ok(solidcraft_kernel::body_distance(a, b, f64::INFINITY)?.map(|d| if d.distance < tolerance { 0.0 } else { d.distance }))
            })
        });
        for (i, list) in lists {
            let rows: Vec<Value> = list.iter().map(|&(j, d)| json!({"body": name(j), "distance_mm": d})).collect();
            nearest_out.insert(name(i).to_string(), Value::Array(rows));
        }
    }

    Ok(json!({
        "overlaps": overlaps,
        "near": near,
        "nearest": nearest_out,
        "unchecked": unchecked,
        "bodies": subjects.len(),
        "pairs_checked": close.len(),
        "pairs_pruned": total.saturating_sub(close.len()),
        "truncated": truncated > 0,
        "ms": started.elapsed().as_secs_f64() * 1000.0,
    }))
}

/// One pair: its overlap when the meshes meet or one holds the other, otherwise its distance
/// when it may be closer than `gap`.
fn measure_pair(
    ba: &solidcraft_doc::ModelBody,
    a: &DistanceShape,
    bb: &solidcraft_doc::ModelBody,
    b: &DistanceShape,
    gap: f64,
    min_volume: f64,
    tolerance: f64,
) -> PairResult {
    let mut out = PairResult::default();
    let slack = 2.0 * (a.chord() + b.chord());
    let mesh = a.mesh_closest(b, gap + slack);
    let holds = |outer: &solidcraft_doc::ModelBody, inner: &solidcraft_doc::ModelBody| {
        let (o, i) = (outer.mesh(), inner.mesh());
        let (ob, ib) = (o.bounds(), i.bounds());
        let nested = ob.min.x <= ib.min.x
            && ob.min.y <= ib.min.y
            && ob.min.z <= ib.min.z
            && ib.max.x <= ob.max.x
            && ib.max.y <= ob.max.y
            && ib.max.z <= ob.max.z;
        nested && i.positions.first().is_some_and(|p| o.contains(*p))
    };
    let touching = mesh.is_some_and(|m| m.distance <= slack);
    if touching || holds(ba, bb) || holds(bb, ba) {
        // A pair whose shared part can't be worked out is reported, not passed as clear.
        let region = shared_region(a, b);
        let (ca, cb) = (clipped(&ba.body, region), clipped(&bb.body, region));
        let (shared, report) = solidcraft_kernel::capture_boolean_diagnostics(|| {
            solidcraft_kernel::boolean(ca.as_ref().unwrap_or(&ba.body), cb.as_ref().unwrap_or(&bb.body), solidcraft_kernel::BoolOp::Intersect)
        });
        match shared {
            Err(e) => {
                out.unchecked = Some(e.to_string());
                return out;
            }
            Ok(shared) => {
                if let Some(reason) = report.unverified() {
                    out.unchecked = Some(reason);
                    return out;
                }
                if let Some(o) = shared {
                    match measure_near_origin(&o, region.center()) {
                        Ok(m) if !plausible(m.volume, m.centroid, m.bbox) => {
                            out.unchecked = Some(format!(
                                "the shared part came out malformed ({:.4} mm³ with its centre outside its own box), so it is not measured",
                                m.volume
                            ));
                            return out;
                        }
                        Ok(m) => out.overlap = Some((m.volume, m.area, m.bbox, m.centroid)),
                        Err(e) => {
                            out.unchecked = Some(e.to_string());
                            return out;
                        }
                    }
                }
            }
        }
        if out.overlap.is_some_and(|o| o.0 >= min_volume) {
            return out;
        }
        if out.overlap.is_some_and(|o| o.0 > 0.0) {
            // Contact with a sliver of shared volume.
            let at = out.overlap.map(|o| o.3).unwrap_or_default();
            out.distance = Some(BodyDistance { distance: 0.0, from: at, to: at, exact: true });
            return out;
        }
    }
    if mesh.is_none() {
        return out;
    }
    match solidcraft_kernel::body_distance(a, b, gap) {
        Ok(Some(mut d)) => {
            if d.distance < tolerance {
                d.distance = 0.0;
            }
            out.distance = Some(d);
        }
        Ok(None) => {}
        Err(e) => out.unchecked = Some(e.to_string()),
    }
    out
}

/// The box where two bodies may share volume: where their boxes meet, grown by a margin so that
/// a cut there stays clear of both bodies' faces.
fn shared_region(a: &DistanceShape, b: &DistanceShape) -> Aabb3 {
    let (x, y) = (a.bounds(), b.bounds());
    let min = x.min.max(y.min);
    let max = x.max.min(y.max);
    if x.is_empty() || y.is_empty() || !(min.x <= max.x && min.y <= max.y && min.z <= max.z) {
        return Aabb3::EMPTY;
    }
    let grow = 1.0 + 0.05 * min.dist(max);
    let g = solidcraft_geom::Vec3::new(grow, grow, grow);
    Aabb3 { min: min - g, max: max + g }
}

/// `body` cut down to `region` when the body is much larger than it. The boolean works to a
/// tolerance that grows with the bodies' size: a 0.01 mm overlap against a plate 1.7 m long
/// comes out a quarter too small, and thinner ones not at all. Cut to the region first, the
/// body is small and its faces there are the same faces. `None`: use the whole body.
fn clipped(body: &solidcraft_kernel::Body, region: Aabb3) -> Option<solidcraft_kernel::Body> {
    if region.is_empty() || body.size() <= 4.0 * region.diagonal() {
        return None;
    }
    let cut = solidcraft_kernel::box_solid(region.min, region.max).ok()?;
    let (r, report) = solidcraft_kernel::capture_boolean_diagnostics(|| solidcraft_kernel::boolean(body, &cut, solidcraft_kernel::BoolOp::Intersect));
    match r {
        Ok(Some(c)) if report.unverified().is_none() => Some(c),
        _ => None,
    }
}

/// [`solidcraft_kernel::measure`] of `body` moved by `-at` and back: far from the origin, the
/// centre of a thin part loses its precision (its moments are large numbers that nearly cancel).
fn measure_near_origin(body: &solidcraft_kernel::Body, at: solidcraft_geom::Vec3) -> solidcraft_kernel::Result<solidcraft_kernel::BodyMeasure> {
    let Ok(moved) = solidcraft_kernel::transform(body, -at, solidcraft_geom::Vec3::ZERO, solidcraft_geom::Vec3::Z, 0.0) else {
        return solidcraft_kernel::measure(body);
    };
    let mut m = solidcraft_kernel::measure(&moved)?;
    m.centroid += at;
    m.bbox = Aabb3 { min: m.bbox.min + at, max: m.bbox.max + at };
    Ok(m)
}

/// Is a measured shared part well formed? A part whose shell came out inside out or open can
/// carry a volume with its centre outside its own box, or more volume than its box holds.
fn plausible(volume: f64, centre: solidcraft_geom::Vec3, bbox: Aabb3) -> bool {
    let slack = 1e-6 * bbox.diagonal().max(1.0);
    let (lo, hi) = (bbox.min, bbox.max);
    let inside = [(centre.x, lo.x, hi.x), (centre.y, lo.y, hi.y), (centre.z, lo.z, hi.z)].iter().all(|&(c, l, h)| c >= l - slack && c <= h + slack);
    let s = bbox.size();
    volume.is_finite() && volume >= 0.0 && inside && volume <= s.x * s.y * s.z + slack
}

/// Each subject's `count` nearest bodies, nearest first: pairs are measured in the order of their
/// boxes' gaps (a lower bound of their distance), in rounds, until the boxes of the next ones
/// are farther apart than the last neighbour kept. `known` holds distances already measured.
fn nearest_lists(
    subjects: &[usize],
    n: usize,
    count: usize,
    boxes: &[Aabb3],
    allowed: &dyn Fn(usize, usize) -> bool,
    known: &mut HashMap<(usize, usize), f64>,
    measure: impl Fn(&[(usize, usize)]) -> Vec<solidcraft_kernel::Result<Option<f64>>>,
) -> BTreeMap<usize, Vec<(usize, f64)>> {
    let key = |i: usize, j: usize| (i.min(j), i.max(j));
    let gap_of = |i: usize, j: usize| match (boxes.get(i), boxes.get(j)) {
        (Some(a), Some(b)) => box_gap(a, b),
        _ => f64::INFINITY,
    };
    // Each subject's candidates, by box gap.
    let mut order: BTreeMap<usize, (Vec<(f64, usize)>, usize)> = BTreeMap::new();
    for &i in subjects {
        let mut c: Vec<(f64, usize)> = (0..n).filter(|&j| allowed(i, j)).map(|j| (gap_of(i, j), j)).filter(|(g, _)| g.is_finite()).collect();
        c.sort_by(|x, y| x.0.total_cmp(&y.0).then(x.1.cmp(&y.1)));
        order.insert(i, (c, 0));
    }
    let mut failed: std::collections::HashSet<(usize, usize)> = std::collections::HashSet::new();
    let measured = |known: &HashMap<(usize, usize), f64>, i: usize, c: &[(f64, usize)]| -> Vec<(usize, f64)> {
        let mut l: Vec<(usize, f64)> = c.iter().filter_map(|&(_, j)| known.get(&key(i, j)).map(|d| (j, *d))).collect();
        l.sort_by(|x, y| x.1.total_cmp(&y.1).then(x.0.cmp(&y.0)));
        l
    };
    // Each round measures, for every subject not settled yet, the next `count` candidates.
    for _ in 0..n.max(1) {
        let mut todo: Vec<(usize, usize)> = Vec::new();
        for (&i, (c, next)) in order.iter_mut() {
            let have = measured(known, i, c);
            let mut added = 0;
            while let Some(&(g, j)) = c.get(*next) {
                let bound = if have.len() >= count { have.get(count - 1).map_or(f64::INFINITY, |x| x.1) } else { f64::INFINITY };
                if g > bound || added >= count {
                    break;
                }
                *next += 1;
                let k = key(i, j);
                if !known.contains_key(&k) && !failed.contains(&k) && !todo.contains(&k) {
                    todo.push(k);
                    added += 1;
                }
            }
        }
        if todo.is_empty() {
            break;
        }
        for (k, r) in todo.iter().zip(measure(&todo)) {
            match r {
                Ok(Some(d)) => {
                    known.insert(*k, d);
                }
                _ => {
                    failed.insert(*k);
                }
            }
        }
    }
    order.iter().map(|(&i, (c, _))| (i, measured(known, i, c).into_iter().take(count).collect())).collect()
}

#[cfg(test)]
#[path = "clearance_tests.rs"]
mod tests;
