//! Compare Bodies: two versions of one part laid over each other. Both are measured, each
//! surface's distance to the other is sampled, and the places where only one of them has
//! material are found, measured and, on request, coloured. It works from the surfaces
//! ([`solidcraft_geom::compare`]), not from a boolean of the two bodies, so bodies that share
//! almost every face compare as well as any.

use serde_json::{Value, json};
use solidcraft_geom::Vec3;
use solidcraft_geom::compare::{self, CompareOptions, Side};

use super::CommandSpec;
use crate::params::bad;
use crate::{Result, Sel, Session};

pub static COMMANDS: &[CommandSpec] = &[CommandSpec::new("inspect.compare", "Compare Bodies", compare_bodies).icon("compare").params(
    "a, b: body names (default: the two selected bodies, or the design's only two), in world placement; tolerance?: mm (0.001 to 10, default 0.01): \
     surfaces closer than this agree; spacing?: mm (default 0.5): the largest piece of a face sampled as one point, coarser for a body that would need \
     more than max_points? (default 8 million, up to 50 million) samples; max_regions? (default 50); color?: true | {only_in_a?, only_in_b?: \"#rrggbb\"} \
     (colours the faces that hold a region, red where only a has material and green where only b has: the faces mostly beyond the tolerance, else the \
     one with the largest share; an undo step) → a, b: volume_mm3, area_mm2, center_of_mass, bbox; difference: b − a; \
     deviation: a_to_b, b_to_a (max_mm, mean_mm, at, area_beyond_mm2); regions, largest first: side (only_in_a | only_in_b), volume_mm3 (integrated along \
     lines through the region: resolution), area_mm2, max_deviation_mm, at, center, bbox, faces_a, faces_b (face indices, as model.faces lists them); \
     identical; verdict; warnings (a body whose surface does not enclose what it measures, lines left out). Works from the two surfaces, without a boolean of the bodies, so near-identical bodies compare too",
)];

/// Default colours: red for material only `a` has, green for material only `b` has.
const ONLY_IN_A: [u8; 3] = [0xe0, 0x31, 0x31];
const ONLY_IN_B: [u8; 3] = [0x2f, 0x9e, 0x44];

fn side_name(s: Side) -> &'static str {
    match s {
        Side::OnlyInA => "only_in_a",
        Side::OnlyInB => "only_in_b",
    }
}

/// A number parameter in `range`, `default` when absent.
fn number(cmd: &str, p: &Value, key: &str, default: f64, range: std::ops::RangeInclusive<f64>) -> Result<f64> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(v) => v
            .as_f64()
            .filter(|x| x.is_finite() && range.contains(x))
            .ok_or_else(|| bad(cmd, format!("`{key}` must be a number from {} to {}", range.start(), range.end()))),
    }
}

/// The two bodies: `a` and `b`, else the two selected bodies, else the design's only two.
fn pair(s: &Session, st: &solidcraft_doc::ModelState, p: &Value, cmd: &str) -> Result<(String, String)> {
    let given = |k: &str| -> Result<Option<String>> {
        match p.get(k) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(n)) => Ok(Some(n.clone())),
            Some(_) => Err(bad(cmd, format!("`{k}` must be a body name"))),
        }
    };
    let (a, b) = match (given("a")?, given("b")?) {
        (Some(a), Some(b)) => (a, b),
        (None, None) => {
            let selected: Vec<String> =
                s.selection.iter().filter_map(|x| if let Sel::Body { name } = x { Some(name.clone()) } else { None }).collect();
            let all: Vec<String> = st.bodies.iter().map(|b| b.name.clone()).collect();
            match (selected.as_slice(), all.as_slice()) {
                ([a, b], _) | (_, [a, b]) => (a.clone(), b.clone()),
                _ => return Err(bad(cmd, "give `a` and `b` (or select two bodies)")),
            }
        }
        _ => return Err(bad(cmd, "give both `a` and `b`")),
    };
    for n in [&a, &b] {
        if st.body(n).is_none() {
            return Err(bad(cmd, format!("no body `{n}`")));
        }
    }
    if a == b {
        return Err(bad(cmd, "`a` and `b` must be two different bodies"));
    }
    Ok((a, b))
}

fn colours(cmd: &str, p: &Value) -> Result<Option<[[u8; 3]; 2]>> {
    match p.get("color") {
        None | Some(Value::Null) | Some(Value::Bool(false)) => Ok(None),
        Some(Value::Bool(true)) => Ok(Some([ONLY_IN_A, ONLY_IN_B])),
        Some(Value::Object(o)) => {
            let pick = |k: &str, d: [u8; 3]| match o.get(k) {
                None | Some(Value::Null) => Ok(d),
                Some(v) => {
                    super::inspect::color_param(v).ok_or_else(|| bad(cmd, format!("`color.{k}` must be \"#rrggbb\" or [r, g, b] with values 0–255")))
                }
            };
            Ok(Some([pick("only_in_a", ONLY_IN_A)?, pick("only_in_b", ONLY_IN_B)?]))
        }
        Some(_) => Err(bad(cmd, "`color` must be true or {only_in_a, only_in_b}")),
    }
}

fn v3(v: &Value) -> Option<Vec3> {
    crate::params::vec3(v)
}

/// b − a of the two bodies' measures.
fn difference(ma: &Value, mb: &Value) -> Value {
    let f = |m: &Value, k: &str| m.get(k).and_then(Value::as_f64);
    let sub = |x: Option<f64>, y: Option<f64>| x.zip(y).map(|(x, y)| y - x);
    let (va, vb) = (f(ma, "volume_mm3"), f(mb, "volume_mm3"));
    let com = v3(&ma["center_of_mass"]).zip(v3(&mb["center_of_mass"])).map(|(a, b)| b - a);
    let corner = |k: &str| v3(&ma["bbox"][k]).zip(v3(&mb["bbox"][k])).map(|(a, b)| b - a);
    json!({
        "volume_mm3": sub(va, vb),
        "volume_percent": va.zip(vb).filter(|(a, _)| a.abs() > 0.0).map(|(a, b)| 100.0 * (b - a) / a),
        "area_mm2": sub(f(ma, "area_mm2"), f(mb, "area_mm2")),
        "center_of_mass": com,
        "center_of_mass_distance_mm": com.map(Vec3::len),
        "bbox_min": corner("min"),
        "bbox_max": corner("max"),
    })
}

/// The volume a closed mesh encloses (divergence theorem, about its box's centre).
fn enclosed_volume(m: &solidcraft_geom::Mesh) -> f64 {
    let o = m.bounds().center();
    m.triangles.iter().filter_map(|t| m.tri(t)).map(|[a, b, c]| (a - o).dot((b - o).cross(c - o)) / 6.0).sum()
}

/// What makes the comparison less certain: a body whose surface does not enclose what it
/// measures (open, or faces that failed), lines left out of region volumes.
fn warnings(bodies: [(&str, &Value, &solidcraft_geom::Mesh); 2], chord: f64, c: &compare::Comparison) -> Vec<String> {
    let mut out = Vec::new();
    for (name, measure, mesh) in bodies {
        let (Some(v), area) = (measure["volume_mm3"].as_f64(), measure["area_mm2"].as_f64().unwrap_or(0.0)) else { continue };
        let enclosed = enclosed_volume(mesh);
        // A mesh within `chord` of the surface encloses the volume to within chord × area.
        if !((enclosed - v).abs() <= chord * area.abs() + 1e-6 * v.abs()) {
            out.push(format!(
                "`{name}`: its surface encloses {enclosed:.3} mm³ but the body measures {v:.3} mm³: it is open or has faces that failed, so its measures and the regions near the gap are unreliable"
            ));
        }
    }
    let skipped: usize = c.regions.iter().map(|r| r.lines.skipped).sum();
    if skipped > 0 {
        out.push(format!("{skipped} line(s) through the regions crossed a gap in a surface and were left out of their volumes"));
    }
    out
}

fn deviation_json(d: &compare::Deviation) -> Value {
    json!({"max_mm": d.max, "mean_mm": d.mean, "at": d.at, "area_beyond_mm2": d.beyond_area, "area_mm2": d.area, "points": d.points})
}

fn compare_bodies(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "inspect.compare";
    let tolerance = number(cmd, p, "tolerance", 0.01, 0.001..=10.0)?;
    let spacing = number(cmd, p, "spacing", 0.5, 0.01..=1000.0)?;
    let max_points = number(cmd, p, "max_points", 8e6, 1e4..=5e7)? as usize;
    let max_regions = number(cmd, p, "max_regions", 50.0, 1.0..=1000.0)? as usize;
    let colors = colours(cmd, p)?;
    let st = s.world_state();
    let (na, nb) = pair(s, &st, p, cmd)?;
    let (Some(ba), Some(bb)) = (st.body(&na), st.body(&nb)) else { return Err(bad(cmd, "no such body")) };
    // Meshed within a quarter of the tolerance (the kernel goes no finer than a millionth of a
    // body's size), so two meshes of one surface agree well within it.
    let chord = tolerance / 4.0;
    let mut done = solidcraft_kernel::map_on_threads(&[ba, bb], 2, |b| (super::inspect::measure_json(b), b.body.tessellate_with(chord, true)))
        .into_iter()
        .zip([&na, &nb]);
    let mut next = || -> Result<(Value, solidcraft_geom::Mesh)> {
        let Some((r, name)) = done.next() else { return Err(bad(cmd, "no such body")) };
        let (m, mesh) = r.ok_or_else(|| bad(cmd, format!("`{name}`: measuring it failed")))?;
        let mesh = mesh.map_err(|e| bad(cmd, format!("`{name}`: {e}")))?;
        if mesh.triangles.is_empty() {
            return Err(bad(cmd, format!("`{name}` has no surface")));
        }
        Ok((m, mesh))
    };
    let ((measure_a, mesh_a), (measure_b, mesh_b)) = (next()?, next()?);
    let options = CompareOptions { tolerance, spacing, max_points, max_regions, ..CompareOptions::default() };
    let each = |n: usize, job: &(dyn Fn(usize) + Sync)| {
        let jobs: Vec<usize> = (0..n).collect();
        solidcraft_kernel::for_each_on_threads(&jobs, 16, |i| job(*i));
    };
    let c = compare::compare(&mesh_a, &mesh_b, &options, &each);
    let axis = |k: usize| ["X", "Y", "Z"].get(k).copied().unwrap_or("?");
    let regions: Vec<Value> = c
        .regions
        .iter()
        .enumerate()
        .map(|(i, r)| {
            json!({
                "index": i + 1,
                "side": side_name(r.side),
                "material_of": if r.side == Side::OnlyInA { &na } else { &nb },
                "volume_mm3": r.volume,
                "area_mm2": r.area_a + r.area_b,
                "area_a_mm2": r.area_a,
                "area_b_mm2": r.area_b,
                "max_deviation_mm": r.max_deviation,
                "at": r.at,
                "center": r.center,
                "bbox": {"min": r.bbox.min, "max": r.bbox.max},
                "faces_a": r.faces_a.iter().map(|x| x.0).collect::<Vec<u32>>(),
                "faces_b": r.faces_b.iter().map(|x| x.0).collect::<Vec<u32>>(),
                "resolution": {"along": axis(r.lines.axis), "line_spacing_mm": r.lines.spacing, "lines": r.lines.count, "skipped_lines": r.lines.skipped},
            })
        })
        .collect();
    let total = |side: Side| {
        let rs: Vec<&compare::Region> = c.regions.iter().filter(|r| r.side == side).collect();
        // `+ 0.0`: an empty sum is -0.0.
        (rs.len(), rs.iter().map(|r| r.volume).filter(|v| v.is_finite()).sum::<f64>() + 0.0)
    };
    let ((ca, va), (cb, vb)) = (total(Side::OnlyInA), total(Side::OnlyInB));
    let worst = c.a_to_b.max.max(c.b_to_a.max);
    let identical = c.regions.is_empty() && c.unlisted == 0;
    let warnings = warnings([(na.as_str(), &measure_a, &mesh_a), (nb.as_str(), &measure_b, &mesh_b)], chord, &c);
    let mut verdict = if identical {
        format!("identical within {tolerance} mm (largest deviation {worst:.4} mm)")
    } else {
        let more = if c.unlisted > 0 { format!(" (and {} more, not measured)", c.unlisted) } else { String::new() };
        format!(
            "{} {} by more than {tolerance} mm: {ca} with material only in `{na}` ({va:.3} mm³), {cb} only in `{nb}` ({vb:.3} mm³){more}",
            c.regions.len(),
            if c.regions.len() == 1 { "region differs" } else { "regions differ" }
        )
    };
    if !warnings.is_empty() {
        verdict += &format!(" ({} warning{})", warnings.len(), if warnings.len() == 1 { "" } else { "s" });
    }
    let mut out = json!({
        "a": measure_a,
        "b": measure_b,
        "difference": difference(&measure_a, &measure_b),
        "tolerance_mm": tolerance,
        "deviation": {"a_to_b": deviation_json(&c.a_to_b), "b_to_a": deviation_json(&c.b_to_a), "max_mm": worst},
        "identical": identical,
        "verdict": verdict,
        "regions": regions,
        "only_in_a": {"regions": ca, "volume_mm3": va},
        "only_in_b": {"regions": cb, "volume_mm3": vb},
        "unlisted_regions": c.unlisted,
        "warnings": warnings,
        "sampling": {"spacing_mm": c.spacing, "chord_mm": chord, "triangles_a": mesh_a.triangles.len(), "triangles_b": mesh_b.triangles.len()},
    });
    if let Some(colors) = colors {
        out["colored_faces"] = json!(color_regions(s, &c, [&na, &nb], colors)?);
    }
    Ok(out)
}

/// A face is coloured for a region when at least this share of it deviates there.
const MOSTLY: f64 = 0.5;

/// The faces to colour for a region, as (body: 0 = a, 1 = b; face): those mostly beyond the
/// tolerance (a small difference on a large face would colour all of it), or, when it has none,
/// the one with the largest share.
fn faces_to_color(c: &compare::Comparison, r: &compare::Region) -> Vec<(usize, u32)> {
    let share = |m: usize, &(f, beyond): &(u32, f64)| {
        let all = c.face_area.get(m).and_then(|x| x.get(&f)).copied().unwrap_or(0.0);
        (m, f, if all > 0.0 { beyond / all } else { 0.0 })
    };
    let all: Vec<(usize, u32, f64)> = r.faces_a.iter().map(|x| share(0, x)).chain(r.faces_b.iter().map(|x| share(1, x))).collect();
    let mostly: Vec<(usize, u32)> = all.iter().filter(|x| x.2 >= MOSTLY).map(|x| (x.0, x.1)).collect();
    if !mostly.is_empty() {
        return mostly;
    }
    all.iter().max_by(|x, y| x.2.total_cmp(&y.2)).map(|x| vec![(x.0, x.1)]).unwrap_or_default()
}

/// Give the faces holding each region the colour of its side (a face holding several takes the
/// largest one's); returns how many face appearances were set.
fn color_regions(s: &mut Session, c: &compare::Comparison, names: [&String; 2], colors: [[u8; 3]; 2]) -> Result<usize> {
    use solidcraft_doc::appearance::{FaceLook, Look};
    let st = s.model.state();
    // Where each face of the two bodies is, as the design file names and finds it. A body placed
    // by an occurrence is coloured where it is defined, if at all.
    let found: Vec<_> =
        names.iter().map(|n| st.body(n).map(|b| (solidcraft_doc::naming::face_spots(b), solidcraft_doc::naming::face_names(b)))).collect();
    let mut picks: Vec<(String, Vec3, Option<String>, Look)> = Vec::new();
    // Smallest region first, so the largest one's colour is the one that stays.
    for r in c.regions.iter().rev() {
        let colour = if r.side == Side::OnlyInA { colors[0] } else { colors[1] };
        let mut look = Look::custom(colour, 1.0);
        look.name = if r.side == Side::OnlyInA { "Only in a".into() } else { "Only in b".into() };
        for (m, f) in faces_to_color(c, r) {
            let (Some(Some((spots, face_names))), Some(name)) = (found.get(m), names.get(m)) else { continue };
            let Some(spot) = spots.get(f as usize).copied().flatten() else { continue };
            picks.retain(|(b, pt, _, _)| !(b == *name && pt.dist(spot.point) < 1e-9));
            picks.push(((*name).clone(), spot.point, face_names.get(f as usize).cloned(), look.clone()));
        }
    }
    if picks.is_empty() {
        return Ok(0);
    }
    let a = &mut s.doc_mut().appearances;
    let mut n = 0;
    for (body, point, name, look) in picks {
        a.faces.retain(|f| !(f.body == body && f.point.dist(point) < 1e-9));
        if a.faces.len() >= 100_000 {
            return Err(bad("inspect.compare", "too many face appearances"));
        }
        a.faces.push(FaceLook { body, point, look, name, strict_name: false, certificate: None });
        n += 1;
    }
    Ok(n)
}

#[cfg(test)]
#[path = "compare_tests.rs"]
mod tests;
