//! Section curves: the cut of bodies by a plane as 2D lines, arcs and splines in the plane's
//! frame, returned as JSON or written as SVG or DXF. It changes nothing in the design.

use std::fmt::Write as _;

use serde_json::{Value, json};
use solidcraft_doc::project::{SectionLoop, SectionSeg, section_loops};
use solidcraft_geom::Vec2;
use solidcraft_io::{DxfEntity, DxfLayer, DxfShape};

use crate::params::{bad, str_, string_list};
use crate::{Result, Session};

pub(super) const CMD: &str = "inspect.section_curves";
pub(super) const PARAMS: &str = "plane: XY|XZ|YZ | construction plane name or id | {origin, normal}; offset?, at?, flip?: as inspect.section; \
     bodies?: [name] (default every visible body); tolerance?: 0.0001..1 mm (mesh chord tolerance, default 0.01); path?: .svg | .dxf file (one group or layer per body). \
     Returns {frame: {origin, x, y, normal}, loops: [{body, closed, area (mm², closed loops), segments}]}: points [x, y] in the frame, \
     segments {type: line, start, end} | {type: arc, center, radius, start, end, sweep_deg (counter-clockwise +)} | \
     {type: spline, closed, points (fit points), beziers ([[p0, p1, p2, p3]], the same curve as cubic pieces)}. \
     Lines and arcs are exact; other curves are fit splines through points on the surface. Outer loops run counter-clockwise, holes clockwise";

const MAX_BODIES: usize = 10_000;

fn pt(p: Vec2) -> Value {
    json!([p.x, p.y])
}

pub(super) fn section_curves(s: &mut Session, p: &Value) -> Result<Value> {
    let (plane, _) = super::cut_plane(s, p, CMD)?;
    let tolerance = match p.get("tolerance") {
        None => 0.01,
        Some(v) => v.as_f64().filter(|t| (0.0001..=1.0).contains(t)).ok_or_else(|| bad(CMD, "tolerance must be 0.0001..1 mm"))?,
    };
    let path = match p.get("path") {
        None => None,
        Some(_) => Some(str_(p, "path").filter(|v| !v.trim().is_empty() && v.len() < 4096).ok_or_else(|| bad(CMD, "invalid path"))?),
    };
    let format = match path.map(|v| v.to_ascii_lowercase()) {
        None => None,
        Some(v) if v.ends_with(".svg") => Some(Format::Svg),
        Some(v) if v.ends_with(".dxf") => Some(Format::Dxf),
        Some(_) => return Err(bad(CMD, "path must end in .svg or .dxf")),
    };
    if let Some(path) = path {
        solidcraft_io::vfs::check(path).map_err(|e| crate::EngineError::Other(e.to_string()))?;
    }
    let world = s.world_state();
    let names = string_list(p, "bodies");
    if p.get("bodies").is_some() && (names.is_empty() || names.len() > MAX_BODIES) {
        return Err(bad(CMD, "`bodies` must list body names"));
    }
    if let Some(n) = names.iter().find(|n| world.bodies.iter().all(|b| &b.name != *n)) {
        return Err(bad(CMD, format!("no body `{n}`")));
    }
    let mut cut: Vec<(String, Vec<SectionLoop>)> = Vec::new();
    for b in world.bodies.iter().filter(|b| if names.is_empty() { s.body_visible(&b.name) } else { names.contains(&b.name) }) {
        // Bodies the plane misses need no finer mesh.
        let bounds = b.mesh().bounds();
        let heights = [bounds.min.x, bounds.max.x]
            .iter()
            .flat_map(|x| [bounds.min.y, bounds.max.y].map(|y| (*x, y)))
            .flat_map(|(x, y)| [bounds.min.z, bounds.max.z].map(|z| plane.height(solidcraft_geom::Vec3::new(x, y, z))))
            .collect::<Vec<_>>();
        if bounds.is_empty() || heights.iter().all(|h| *h > 0.0) || heights.iter().all(|h| *h < 0.0) {
            continue;
        }
        let mesh = b.body.reference_display_mesh_angular(tolerance, solidcraft_kernel::DISPLAY_ANGLE)?;
        let loops = section_loops(&plane, &mesh);
        if !loops.is_empty() {
            cut.push((b.name.clone(), loops));
        }
    }
    let loops: Vec<Value> = cut
        .iter()
        .flat_map(|(body, loops)| {
            loops.iter().map(move |l| {
                let mut v = json!({"body": body, "closed": l.closed, "segments": l.segments.iter().map(seg_json).collect::<Vec<_>>()});
                if let Some(a) = l.area {
                    v["area"] = json!(a);
                }
                v
            })
        })
        .collect();
    let mut result = json!({
        "frame": {"origin": plane.origin, "x": plane.x, "y": plane.y, "normal": plane.normal()},
        "units": "mm",
        "loops": loops,
    });
    if let (Some(path), Some(format)) = (path, format) {
        let text = match format {
            Format::Svg => svg(&cut),
            Format::Dxf => dxf(&cut),
        };
        solidcraft_io::vfs::check(path).map_err(|e| crate::EngineError::Other(e.to_string()))?;
        solidcraft_io::write_atomic(std::path::Path::new(path), text.as_bytes(), false).map_err(|e| crate::EngineError::Other(e.to_string()))?;
        result["path"] = json!(path);
    }
    Ok(result)
}

enum Format {
    Svg,
    Dxf,
}

fn seg_json(s: &SectionSeg) -> Value {
    match s {
        SectionSeg::Line { a, b } => json!({"type": "line", "start": pt(*a), "end": pt(*b)}),
        SectionSeg::Arc { c, r, sweep, .. } => {
            json!({"type": "arc", "center": pt(*c), "radius": r, "start": pt(s.start()), "end": pt(s.end()), "sweep_deg": sweep.to_degrees()})
        }
        SectionSeg::Spline { pts, periodic } => json!({
            "type": "spline",
            "closed": periodic,
            "points": pts.iter().map(|p| pt(*p)).collect::<Vec<_>>(),
            "beziers": s.beziers().iter().map(|b| b.iter().map(|p| pt(*p)).collect::<Vec<_>>()).collect::<Vec<_>>(),
        }),
    }
}

/// An SVG in millimetres, y up as in the frame (the file's y runs down, so it is negated):
/// one group per body, one even-odd filled path per loop set.
fn svg(cut: &[(String, Vec<SectionLoop>)]) -> String {
    let mut pts = Vec::new();
    for (_, loops) in cut {
        for l in loops {
            for s in &l.segments {
                s.sample(&mut pts);
            }
        }
    }
    let (lo, hi) = pts.iter().fold((Vec2::new(f64::INFINITY, f64::INFINITY), Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY)), |(lo, hi), p| {
        (Vec2::new(lo.x.min(p.x), lo.y.min(p.y)), Vec2::new(hi.x.max(p.x), hi.y.max(p.y)))
    });
    let (lo, hi) = if lo.is_finite() && hi.is_finite() { (lo, hi) } else { (Vec2::ZERO, Vec2::ZERO) };
    let margin = ((hi.x - lo.x).max(hi.y - lo.y) * 0.05).max(1.0);
    let (w, h) = (hi.x - lo.x + 2.0 * margin, hi.y - lo.y + 2.0 * margin);
    // Six decimals, no trailing zeros, and no "-0".
    let f = |v: f64| format!("{:.6}", (v * 1e6).round() / 1e6 + 0.0).trim_end_matches('0').trim_end_matches('.').to_string();
    let xy = |p: Vec2| format!("{} {}", f(p.x), f(-p.y));
    let mut o = String::new();
    let _ = writeln!(o, r#"<?xml version="1.0" encoding="UTF-8"?>"#);
    let _ = writeln!(
        o,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{}mm" height="{}mm" viewBox="{} {} {} {}">"#,
        f(w),
        f(h),
        f(lo.x - margin),
        f(-hi.y - margin),
        f(w),
        f(h)
    );
    for (body, loops) in cut {
        let _ = writeln!(o, r#"  <g id="{}" fill="none" stroke="black" stroke-width="0.25">"#, xml(body));
        let mut d = String::new();
        for l in loops {
            let Some(first) = l.segments.first() else { continue };
            let _ = write!(d, "M {}", xy(first.start()));
            for s in &l.segments {
                match s {
                    SectionSeg::Line { b, .. } => {
                        let _ = write!(d, " L {}", xy(*b));
                    }
                    SectionSeg::Arc { c, r, start, sweep } => {
                        // A whole circle is two half arcs (an SVG arc's ends must differ).
                        let halves = if sweep.abs() > std::f64::consts::PI { 2 } else { 1 };
                        for k in 1..=halves {
                            let a = start + sweep * k as f64 / halves as f64;
                            let large = u8::from(sweep.abs() / halves as f64 > std::f64::consts::PI);
                            let _ = write!(d, " A {} {} 0 {} {} {}", f(*r), f(*r), large, u8::from(*sweep > 0.0), xy(*c + Vec2::from_angle(a) * *r));
                        }
                    }
                    SectionSeg::Spline { .. } => {
                        for [_, p1, p2, p3] in s.beziers() {
                            let _ = write!(d, " C {} {} {}", xy(p1), xy(p2), xy(p3));
                        }
                    }
                }
            }
            if l.closed {
                d.push_str(" Z ");
            }
        }
        let _ = writeln!(o, r#"    <path fill-rule="evenodd" d="{}"/>"#, d.trim_end());
        let _ = writeln!(o, "  </g>");
    }
    o.push_str("</svg>\n");
    o
}

fn xml(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// A DXF in millimetres: one layer per body (named after it), LINE, ARC, CIRCLE and SPLINE
/// entities (a spline's cubic pieces as one clamped cubic B-spline).
fn dxf(cut: &[(String, Vec<SectionLoop>)]) -> String {
    let mut layers: Vec<DxfLayer> = Vec::new();
    let mut entities = Vec::new();
    for (body, loops) in cut {
        // DXF layer names exclude <>/\":;?*|=` and should be short.
        let mut name: String = body.chars().map(|c| if "<>/\\\":;?*|=`".contains(c) || c.is_control() { '_' } else { c }).take(200).collect();
        if name.trim().is_empty() {
            name = "BODY".into();
        }
        while layers.iter().any(|l| l.name == name) {
            name.push('_');
        }
        layers.push(DxfLayer::new(name.clone(), 7));
        for s in loops.iter().flat_map(|l| &l.segments) {
            let shape = match s {
                SectionSeg::Line { a, b } => DxfShape::Line(*a, *b),
                SectionSeg::Arc { c, r, sweep, .. } if (sweep.abs() - std::f64::consts::TAU).abs() < 1e-9 => DxfShape::Circle { c: *c, r: *r },
                SectionSeg::Arc { c, r, start, sweep } => {
                    let (a0, a1) = if *sweep > 0.0 { (*start, start + sweep) } else { (start + sweep, *start) };
                    DxfShape::Arc { c: *c, r: *r, a0: a0.to_degrees().rem_euclid(360.0), a1: a1.to_degrees().rem_euclid(360.0) }
                }
                SectionSeg::Spline { .. } => {
                    let pieces = s.beziers();
                    let Some(first) = pieces.first() else { continue };
                    let mut ctrl = vec![first[0]];
                    let mut knots = vec![0.0; 4];
                    for (i, [_, p1, p2, p3]) in pieces.iter().enumerate() {
                        ctrl.extend([*p1, *p2, *p3]);
                        knots.extend([(i + 1) as f64; 3]);
                    }
                    knots.push(pieces.len() as f64);
                    DxfShape::Spline { degree: 3, knots, ctrl, weights: None }
                }
            };
            entities.push(DxfEntity { layer: name.clone(), shape });
        }
    }
    solidcraft_io::write_dxf_drawing(&layers, &entities)
}

#[cfg(test)]
#[path = "section_curves_tests.rs"]
mod tests;
