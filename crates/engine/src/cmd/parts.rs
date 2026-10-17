//! Standard parts: ISO metric fasteners, dowel pins and ball bearings, generated in code from
//! the dimensions in the public standards tables (no vendor geometry). Each part is inserted as
//! its own component, built by ordinary features (so it stays parametric and editable), with a
//! cosmetic thread where it has one; at a hole it gets a rigid joint that puts its axis on the
//! hole's and seats it on the face.
//!
//! Part frame: the seating plane is z = 0 (the underside of a head, the bottom of a nut or
//! washer, the top of a set screw or pin); the shank runs down −z.

use serde_json::{Value, json};
use solidcraft_geom::Vec3;

use super::CommandSpec;
use crate::params::{bad, num, str_, vec3};
use crate::{EngineError, Result, Session};

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("parts.library", "Standard Parts", library).icon("library")
        .noundo()
        .params("family? → families (name, standard, sizes, lengths) and, with family, each size's dimensions (mm)"),
    CommandSpec::new("parts.fastener", "Insert Fastener", insert).at("SOLID", "INSERT").icon("fastener").params(
        "the Insert Part dialog's command (same as parts.insert): family, size, length?, at?|point?+direction?, name?",
    ),
    CommandSpec::new("parts.insert", "Insert Part", insert).icon("fastener").params(
        "family (socket_head_cap_screw | hex_bolt | hex_nut | washer | set_screw | dowel_pin | bearing); size (\"M6\", or \"608\" for bearings); \
         length? (mm; screws, bolts, set screws, pins; default a standard length near 2.5 d); \
         at?: [x,y,z] on a hole's rim or wall (the part seats on the hole's face with a rigid joint) | point?: [x,y,z] with direction?: [x,y,z] (no joint); name?",
    ),
    CommandSpec::new("parts.resize", "Change Part Size", resize).icon("scale")
        .params("component: id|name of an inserted part; size?; length? — rebuilds it (joints keep it seated)"),
];

/// A family: (id, name, standard).
const FAMILIES: [(&str, &str, &str); 7] = [
    ("socket_head_cap_screw", "Socket head cap screw", "ISO 4762"),
    ("hex_bolt", "Hex head bolt / screw", "ISO 4014 / ISO 4017"),
    ("hex_nut", "Hex nut", "ISO 4032"),
    ("washer", "Plain washer", "ISO 7089"),
    ("set_screw", "Hex socket set screw, flat point", "ISO 4026"),
    ("dowel_pin", "Parallel pin (dowel)", "ISO 8734"),
    ("bearing", "Deep groove ball bearing (envelope)", "ISO 15 (6000/6200/600 series)"),
];

/// ISO metric coarse: (size, d, pitch).
const METRIC: [(&str, f64, f64); 9] = [
    ("M3", 3.0, 0.5),
    ("M4", 4.0, 0.7),
    ("M5", 5.0, 0.8),
    ("M6", 6.0, 1.0),
    ("M8", 8.0, 1.25),
    ("M10", 10.0, 1.5),
    ("M12", 12.0, 1.75),
    ("M16", 16.0, 2.0),
    ("M20", 20.0, 2.5),
];

/// ISO 4762: (size, head diameter dk, head height k, socket s, socket depth t).
const SHCS: [(&str, f64, f64, f64, f64); 9] = [
    ("M3", 5.5, 3.0, 2.5, 1.3),
    ("M4", 7.0, 4.0, 3.0, 2.0),
    ("M5", 8.5, 5.0, 4.0, 2.5),
    ("M6", 10.0, 6.0, 5.0, 3.0),
    ("M8", 13.0, 8.0, 6.0, 4.0),
    ("M10", 16.0, 10.0, 8.0, 5.0),
    ("M12", 18.0, 12.0, 10.0, 6.0),
    ("M16", 24.0, 16.0, 14.0, 8.0),
    ("M20", 30.0, 20.0, 17.0, 10.0),
];

/// ISO 4014/4017 hex head: (size, across flats s, head height k).
const HEX_BOLT: [(&str, f64, f64); 9] = [
    ("M3", 5.5, 2.0),
    ("M4", 7.0, 2.8),
    ("M5", 8.0, 3.5),
    ("M6", 10.0, 4.0),
    ("M8", 13.0, 5.3),
    ("M10", 16.0, 6.4),
    ("M12", 18.0, 7.5),
    ("M16", 24.0, 10.0),
    ("M20", 30.0, 12.5),
];

/// ISO 4032: (size, across flats s, height m).
const HEX_NUT: [(&str, f64, f64); 9] = [
    ("M3", 5.5, 2.4),
    ("M4", 7.0, 3.2),
    ("M5", 8.0, 4.7),
    ("M6", 10.0, 5.2),
    ("M8", 13.0, 6.8),
    ("M10", 16.0, 8.4),
    ("M12", 18.0, 10.8),
    ("M16", 24.0, 14.8),
    ("M20", 30.0, 18.0),
];

/// ISO 7089: (size, inner d1, outer d2, thickness h).
const WASHER: [(&str, f64, f64, f64); 9] = [
    ("M3", 3.2, 7.0, 0.5),
    ("M4", 4.3, 9.0, 0.8),
    ("M5", 5.3, 10.0, 1.0),
    ("M6", 6.4, 12.0, 1.6),
    ("M8", 8.4, 16.0, 1.6),
    ("M10", 10.5, 20.0, 2.0),
    ("M12", 13.0, 24.0, 2.5),
    ("M16", 17.0, 30.0, 3.0),
    ("M20", 21.0, 37.0, 3.0),
];

/// ISO 4026: (size, socket s, socket depth t).
const SET_SCREW: [(&str, f64, f64); 7] =
    [("M3", 1.5, 1.2), ("M4", 2.0, 1.5), ("M5", 2.5, 2.0), ("M6", 3.0, 2.0), ("M8", 4.0, 3.0), ("M10", 5.0, 4.0), ("M12", 6.0, 4.8)];

/// ISO 8734 diameters.
const DOWEL: [f64; 9] = [2.0, 3.0, 4.0, 5.0, 6.0, 8.0, 10.0, 12.0, 16.0];

/// Bearings: (designation, bore, outside diameter, width).
const BEARINGS: [(&str, f64, f64, f64); 13] = [
    ("625", 5.0, 16.0, 5.0),
    ("626", 6.0, 19.0, 6.0),
    ("608", 8.0, 22.0, 7.0),
    ("6000", 10.0, 26.0, 8.0),
    ("6001", 12.0, 28.0, 8.0),
    ("6002", 15.0, 32.0, 9.0),
    ("6003", 17.0, 35.0, 10.0),
    ("6004", 20.0, 42.0, 12.0),
    ("6200", 10.0, 30.0, 9.0),
    ("6201", 12.0, 32.0, 10.0),
    ("6202", 15.0, 35.0, 11.0),
    ("6203", 17.0, 40.0, 12.0),
    ("6204", 20.0, 47.0, 14.0),
];

/// Standard lengths for screws, bolts and pins (mm).
const LENGTHS: [f64; 22] =
    [4.0, 5.0, 6.0, 8.0, 10.0, 12.0, 16.0, 20.0, 25.0, 30.0, 35.0, 40.0, 45.0, 50.0, 55.0, 60.0, 65.0, 70.0, 80.0, 90.0, 100.0, 120.0];

fn metric(size: &str) -> Option<(f64, f64)> {
    METRIC.iter().find(|(s, _, _)| s.eq_ignore_ascii_case(size)).map(|(_, d, p)| (*d, *p))
}

fn default_length(d: f64) -> f64 {
    LENGTHS.iter().copied().find(|l| *l >= 2.5 * d).unwrap_or(100.0)
}

/// One part's shape, in its own frame.
#[derive(Clone, Debug, PartialEq)]
enum Shape {
    /// A head (round or hex), a shank down −z, an optional hex socket in the head's top.
    Screw { d: f64, pitch: f64, len: f64, head: Head, socket: Option<(f64, f64)> },
    /// A hex prism with a through hole (nut), z 0…m.
    Nut { d: f64, pitch: f64, s: f64, m: f64 },
    /// A ring (washer, bearing envelope), z 0…h.
    Ring { d_in: f64, d_out: f64, h: f64 },
    /// A cylinder down −z with a socket in its top (set screw), or plain (pin).
    Rod { d: f64, pitch: Option<f64>, len: f64, socket: Option<(f64, f64)> },
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Head {
    Round { dk: f64, k: f64 },
    Hex { s: f64, k: f64 },
}

fn shape(family: &str, size: &str, length: Option<f64>) -> std::result::Result<Shape, String> {
    let need = |what: &str| format!("no {what} size `{size}` (parts.library lists them)");
    let len = |d: f64| length.unwrap_or_else(|| default_length(d));
    Ok(match family {
        "socket_head_cap_screw" => {
            let (d, pitch) = metric(size).ok_or_else(|| need("metric"))?;
            let (_, dk, k, s, t) = SHCS.iter().find(|r| r.0.eq_ignore_ascii_case(size)).copied().ok_or_else(|| need("ISO 4762"))?;
            Shape::Screw { d, pitch, len: len(d), head: Head::Round { dk, k }, socket: Some((s, t)) }
        }
        "hex_bolt" => {
            let (d, pitch) = metric(size).ok_or_else(|| need("metric"))?;
            let (_, s, k) = HEX_BOLT.iter().find(|r| r.0.eq_ignore_ascii_case(size)).copied().ok_or_else(|| need("ISO 4014"))?;
            Shape::Screw { d, pitch, len: len(d), head: Head::Hex { s, k }, socket: None }
        }
        "hex_nut" => {
            let (d, pitch) = metric(size).ok_or_else(|| need("metric"))?;
            let (_, s, m) = HEX_NUT.iter().find(|r| r.0.eq_ignore_ascii_case(size)).copied().ok_or_else(|| need("ISO 4032"))?;
            Shape::Nut { d, pitch, s, m }
        }
        "washer" => {
            let (_, d1, d2, h) = WASHER.iter().find(|r| r.0.eq_ignore_ascii_case(size)).copied().ok_or_else(|| need("ISO 7089"))?;
            Shape::Ring { d_in: d1, d_out: d2, h }
        }
        "set_screw" => {
            let (d, pitch) = metric(size).ok_or_else(|| need("metric"))?;
            let (_, s, t) = SET_SCREW.iter().find(|r| r.0.eq_ignore_ascii_case(size)).copied().ok_or_else(|| need("ISO 4026"))?;
            Shape::Rod {
                d,
                pitch: Some(pitch),
                len: length.unwrap_or_else(|| LENGTHS.iter().copied().find(|l| *l >= d).unwrap_or(d)),
                socket: Some((s, t)),
            }
        }
        "dowel_pin" => {
            let d: f64 = size.trim_start_matches(['d', 'D', 'Ø']).trim().parse().map_err(|_| need("dowel pin"))?;
            if !DOWEL.contains(&d) {
                return Err(need("ISO 8734"));
            }
            Shape::Rod { d, pitch: None, len: len(d * 1.6), socket: None }
        }
        "bearing" => {
            let (_, bore, od, w) = BEARINGS.iter().find(|r| r.0 == size.trim()).copied().ok_or_else(|| need("bearing"))?;
            Shape::Ring { d_in: bore, d_out: od, h: w }
        }
        o => return Err(format!("unknown part family `{o}` (parts.library lists them)")),
    })
}

fn run(s: &mut Session, id: &str, p: Value) -> Result<Value> {
    s.execute(id, &p)
}

/// A hex prism (across flats `af`) from z0 to z1, joined or cut.
fn hex(s: &mut Session, af: f64, z0: f64, z1: f64, op: &str) -> Result<()> {
    run(s, "sketch.create", json!({"plane": "XY", "offset": z0}))?;
    run(s, "sketch.polygon.circumscribed", json!({"center": [0, 0], "radius": af / 2.0, "sides": 6}))?;
    run(s, "sketch.finish", json!({}))?;
    let (dist, dir) = if z1 >= z0 { (z1 - z0, "positive") } else { (z0 - z1, "negative") };
    run(s, "solid.extrude", json!({"distance": dist, "direction": dir, "operation": op}))?;
    Ok(())
}

fn cyl(s: &mut Session, d: f64, z0: f64, z1: f64, op: &str) -> Result<()> {
    run(s, "solid.cylinder", json!({"diameter": d, "height": z1 - z0, "base": [0, 0, z0], "axis": [0, 0, 1], "operation": op}))?;
    Ok(())
}

/// A solid of revolution about z from a closed (r, z) outline, as one body or joined.
fn revolve(s: &mut Session, pts: &[(f64, f64)], op: &str) -> Result<()> {
    run(s, "sketch.create", json!({"plane": "XZ"}))?;
    let points: Vec<[f64; 2]> = pts.iter().map(|(r, z)| [*r, -*z]).collect();
    run(s, "sketch.line", json!({"points": points, "closed": true}))?;
    run(s, "sketch.finish", json!({}))?;
    run(s, "solid.revolve", json!({"axis": "y", "operation": op}))?;
    Ok(())
}

fn thread(s: &mut Session, d: f64, pitch: f64, at: Vec3) {
    let _ = run(s, "solid.thread", json!({"face": at, "designation": format!("M{}x{}", d, pitch)}));
    let _ = d;
}

/// Build a part in the active component.
fn build(s: &mut Session, sh: &Shape) -> Result<()> {
    match *sh {
        Shape::Screw { d, pitch, len, head, socket } => {
            let k = match head {
                // One revolved outline: shank and round head.
                Head::Round { dk, k } => {
                    revolve(s, &[(0.0, -len), (d / 2.0, -len), (d / 2.0, 0.0), (dk / 2.0, 0.0), (dk / 2.0, k), (0.0, k)], "new")?;
                    k
                }
                // A hex head joined over a shank that runs up into it.
                Head::Hex { s: af, k } => {
                    hex(s, af, 0.0, k, "new")?;
                    revolve(s, &[(0.0, -len), (d / 2.0, -len), (d / 2.0, k / 2.0), (0.0, k / 2.0)], "join")?;
                    k
                }
            };
            if let Some((af, t)) = socket {
                hex(s, af, k, k - t, "cut")?;
            }
            thread(s, d, pitch, Vec3::new(d / 2.0, 0.0, -len / 2.0));
        }
        Shape::Nut { d, pitch, s: af, m } => {
            hex(s, af, 0.0, m, "new")?;
            cyl(s, d, -1.0, m + 1.0, "cut")?;
            thread(s, d, pitch, Vec3::new(d / 2.0, 0.0, m / 2.0));
        }
        Shape::Ring { d_in, d_out, h } => {
            revolve(s, &[(d_in / 2.0, 0.0), (d_out / 2.0, 0.0), (d_out / 2.0, h), (d_in / 2.0, h)], "new")?;
        }
        Shape::Rod { d, pitch, len, socket } => {
            revolve(s, &[(0.0, -len), (d / 2.0, -len), (d / 2.0, 0.0), (0.0, 0.0)], "new")?;
            if let Some((af, t)) = socket {
                hex(s, af, 0.0, -t, "cut")?;
            }
            if let Some(p) = pitch {
                thread(s, d, p, Vec3::new(d / 2.0, 0.0, -len / 2.0));
            }
        }
    }
    Ok(())
}

fn library(_s: &mut Session, p: &Value) -> Result<Value> {
    let fam = |id: &str| -> Value {
        let sizes: Vec<Value> = match id {
            "socket_head_cap_screw" => SHCS.iter().map(|r| json!({"size": r.0, "d": metric(r.0).map(|m| m.0), "pitch": metric(r.0).map(|m| m.1), "head_diameter": r.1, "head_height": r.2, "socket": r.3, "socket_depth": r.4})).collect(),
            "hex_bolt" => HEX_BOLT.iter().map(|r| json!({"size": r.0, "d": metric(r.0).map(|m| m.0), "pitch": metric(r.0).map(|m| m.1), "across_flats": r.1, "head_height": r.2})).collect(),
            "hex_nut" => HEX_NUT.iter().map(|r| json!({"size": r.0, "d": metric(r.0).map(|m| m.0), "pitch": metric(r.0).map(|m| m.1), "across_flats": r.1, "height": r.2})).collect(),
            "washer" => WASHER.iter().map(|r| json!({"size": r.0, "inner": r.1, "outer": r.2, "thickness": r.3})).collect(),
            "set_screw" => SET_SCREW.iter().map(|r| json!({"size": r.0, "d": metric(r.0).map(|m| m.0), "pitch": metric(r.0).map(|m| m.1), "socket": r.1, "socket_depth": r.2})).collect(),
            "dowel_pin" => DOWEL.iter().map(|d| json!({"size": format!("{d}"), "d": d})).collect(),
            "bearing" => BEARINGS.iter().map(|r| json!({"size": r.0, "bore": r.1, "outer": r.2, "width": r.3})).collect(),
            _ => Vec::new(),
        };
        let (_, name, standard) = FAMILIES.iter().find(|f| f.0 == id).copied().unwrap_or(("", "", ""));
        let lengths = matches!(id, "socket_head_cap_screw" | "hex_bolt" | "set_screw" | "dowel_pin").then_some(LENGTHS.to_vec());
        json!({"family": id, "name": name, "standard": standard, "sizes": sizes, "lengths": lengths})
    };
    match str_(p, "family") {
        Some(f) if FAMILIES.iter().any(|x| x.0 == f) => Ok(fam(f)),
        Some(f) => Err(bad("parts.library", format!("unknown part family `{f}`"))),
        None => Ok(json!({"families": FAMILIES.iter().map(|f| fam(f.0)).collect::<Vec<_>>()})),
    }
}

fn part_name(family: &str, size: &str, sh: &Shape) -> String {
    match sh {
        Shape::Screw { len, .. } | Shape::Rod { len, .. } => format!("{} {size}x{len}", label(family)),
        _ => format!("{} {size}", label(family)),
    }
}

fn label(family: &str) -> &'static str {
    match family {
        "socket_head_cap_screw" => "SHCS",
        "hex_bolt" => "Hex Bolt",
        "hex_nut" => "Hex Nut",
        "washer" => "Washer",
        "set_screw" => "Set Screw",
        "dowel_pin" => "Dowel Pin",
        _ => "Bearing",
    }
}

fn length_param(p: &Value, cmd: &str) -> Result<Option<f64>> {
    match p.get("length") {
        None | Some(Value::Null) => Ok(None),
        Some(_) => {
            let l = num(p, "length").filter(|l| l.is_finite() && *l > 0.0 && *l <= 2000.0).ok_or_else(|| bad(cmd, "`length` must be 0…2000 mm"))?;
            Ok(Some(l))
        }
    }
}

fn insert(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "parts.insert (Insert Fastener)";
    let family = str_(p, "family").ok_or_else(|| bad(cmd, "`family` is required (parts.library)"))?.to_string();
    let size = str_(p, "size").ok_or_else(|| bad(cmd, "`size` is required"))?.to_string();
    let sh = shape(&family, &size, length_param(p, cmd)?).map_err(|e| bad(cmd, e))?;
    // A pick on a hole's rim also touches its end face: step onto the wall just inside the rim.
    let at = p.get("at").and_then(vec3).map(|a| hole_wall(s, a).unwrap_or(a));
    // The occurrence the hole belongs to, found before the part exists.
    let host = match at {
        Some(a) => Some(super::joints::occurrence_at(s, a).ok_or_else(|| bad(cmd, "no body at `at` (pick a hole's rim or wall)"))?),
        None => None,
    };
    let name = str_(p, "name").map(str::to_string).unwrap_or_else(|| part_name(&family, &size, &sh));
    let active = s.active_component;
    let c = run(s, "component.create", json!({"name": name}))?;
    let (comp, occ) = (c["component"].as_u64().unwrap_or(0), c["occurrence"].as_u64().unwrap_or(0));
    build(s, &sh)?;
    s.doc_mut().parts.insert(comp, solidcraft_doc::StandardPart { family: family.clone(), size: size.clone(), length: shape_len(&sh) });
    run(s, "component.activate", json!({"component": active}))?;
    let mut out = json!({"component": comp, "occurrence": occ, "name": name});
    if let (Some(a), Some(h)) = (at, host) {
        // Seat it: its axis on the hole's, its seating plane on the hole's face, the shank going
        // into the hole (the joint mates the part's −z with the face's outward z).
        let j = run(
            s,
            "joint.create",
            json!({"type": "rigid", "a": {"occurrence": h, "circle": a}, "b": {"occurrence": occ, "point": [0, 0, 0], "z": [0, 0, -1]}, "name": format!("{name} seat")}),
        )?;
        out["joint"] = j["joint"].clone();
    } else if let Some(pt) = p.get("point").and_then(vec3) {
        // Placed at a point, its +z along `direction`.
        let dir = p.get("direction").and_then(vec3).and_then(|d| d.normalized()).unwrap_or(Vec3::Z);
        let m = solidcraft_doc::rigid(
            Vec3::ZERO,
            Vec3::ZERO,
            Vec3::Z.cross(dir).normalized().unwrap_or(Vec3::X),
            Vec3::Z.dot(dir).clamp(-1.0, 1.0).acos(),
        );
        let mut m = m;
        m[3] = [pt.x, pt.y, pt.z, 1.0];
        if let Some(o) = s.doc_mut().occurrences.iter_mut().find(|o| o.id == occ) {
            o.transform = m;
        }
    }
    Ok(out)
}

/// A point on the cylindrical wall of the hole at `a`, a little in from the picked rim.
fn hole_wall(s: &Session, a: Vec3) -> Option<Vec3> {
    let st = s.model.state();
    let dirs = [Vec3::Z * -1.0, Vec3::Z, Vec3::X, Vec3::X * -1.0, Vec3::Y, Vec3::Y * -1.0];
    for step in [0.2, 0.5, 1.0, 2.0] {
        for b in &st.bodies {
            for d in dirs {
                let q = a + d * step;
                if let Some(c) = solidcraft_doc::kernel::cylinder_face_at(&b.body, q) {
                    let v = q - c.axis_point;
                    let along = v.dot(c.axis);
                    let on_wall = ((v - c.axis * along).len() - c.radius).abs() < 1e-3 && along > c.start && along < c.end;
                    if on_wall {
                        return Some(q);
                    }
                }
            }
        }
    }
    None
}

fn shape_len(sh: &Shape) -> Option<f64> {
    match sh {
        Shape::Screw { len, .. } | Shape::Rod { len, .. } => Some(*len),
        _ => None,
    }
}

fn resize(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "parts.resize";
    let key = match p.get("component") {
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::String(x)) => x.clone(),
        _ => return Err(bad(cmd, "`component` must be an id or a name")),
    };
    let comp = s.doc.find_component(&key).ok_or_else(|| bad(cmd, format!("no component `{key}`")))?;
    let part = s.doc.parts.get(&comp).cloned().ok_or_else(|| bad(cmd, "that component is not a standard part"))?;
    let size = str_(p, "size").map(str::to_string).unwrap_or(part.size.clone());
    let length = length_param(p, cmd)?.or(if size == part.size { part.length } else { None });
    let sh = shape(&part.family, &size, length).map_err(|e| bad(cmd, e))?;
    // Rebuild: the component's features go, the new shape is built in their place.
    let ids: Vec<u64> = s.doc.features.iter().filter(|f| f.component == comp).map(|f| f.id).collect();
    for id in ids.iter().rev() {
        let _ = s.doc_mut().delete_feature(*id);
    }
    let active = s.active_component;
    run(s, "component.activate", json!({"component": comp}))?;
    build(s, &sh)?;
    let name = part_name(&part.family, &size, &sh);
    s.doc_mut().parts.insert(comp, solidcraft_doc::StandardPart { family: part.family.clone(), size: size.clone(), length: shape_len(&sh) });
    if let Some(c) = s.doc_mut().components.iter_mut().find(|c| c.id == comp) {
        c.name = name.clone();
    }
    run(s, "component.activate", json!({"component": active}))?;
    s.refresh();
    if let Some(e) = s.model.results.iter().find_map(|r| r.error.clone()) {
        return Err(EngineError::Other(e));
    }
    Ok(json!({"component": comp, "name": name, "size": size, "length": shape_len(&sh)}))
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

    fn bbox(s: &Session, prefix: &str) -> (solidcraft_geom::Vec3, solidcraft_geom::Vec3) {
        let st = s.world_state();
        let comp = s.doc.components.iter().find(|c| c.name.starts_with(prefix)).map(|c| c.id).unwrap_or(0);
        let (mut lo, mut hi) = (solidcraft_geom::Vec3::new(f64::MAX, f64::MAX, f64::MAX), solidcraft_geom::Vec3::new(f64::MIN, f64::MIN, f64::MIN));
        for b in st.bodies.iter().filter(|b| s.doc.model_body_component(b) == comp) {
            let m = b.mesh().bounds();
            lo = solidcraft_geom::Vec3::new(lo.x.min(m.min.x), lo.y.min(m.min.y), lo.z.min(m.min.z));
            hi = solidcraft_geom::Vec3::new(hi.x.max(m.max.x), hi.y.max(m.max.y), hi.z.max(m.max.z));
        }
        (lo, hi)
    }

    #[test]
    fn dimensions_follow_the_standard_tables() {
        let l = run(&mut Session::default(), "parts.library", json!({"family": "socket_head_cap_screw"}));
        let m6 = l["sizes"].as_array().and_then(|a| a.iter().find(|x| x["size"] == "M6")).cloned().unwrap_or_default();
        assert_eq!((m6["head_diameter"].as_f64(), m6["head_height"].as_f64(), m6["socket"].as_f64()), (Some(10.0), Some(6.0), Some(5.0)));
        // An M6 x 20 cap screw: head Ø10 x 6, shank 20 long.
        let mut s = Session::default();
        run(&mut s, "parts.insert", json!({"family": "socket_head_cap_screw", "size": "M6", "length": 20}));
        let (lo, hi) = bbox(&s, "SHCS M6x20");
        assert!((hi.x - lo.x - 10.0).abs() < 1e-6 && (lo.z + 20.0).abs() < 1e-6 && (hi.z - 6.0).abs() < 1e-6, "{lo:?} {hi:?}");
        assert!(s.model.state().threads.iter().any(|t| t.designation.starts_with("M6")), "{:?}", s.model.state().threads);
        // An M8 nut: 13 across flats (14.43 across corners), 6.8 high.
        let mut n = Session::default();
        run(&mut n, "parts.insert", json!({"family": "hex_nut", "size": "M8"}));
        let (lo, hi) = bbox(&n, "Hex Nut M8");
        let (w, h) = ((hi.x - lo.x).max(hi.y - lo.y), hi.z - lo.z);
        assert!((w - 13.0 / (std::f64::consts::PI / 6.0).cos()).abs() < 1e-6 && (h - 6.8).abs() < 1e-6, "{w} {h}");
        // A 608 bearing: 8 x 22 x 7.
        let mut b = Session::default();
        run(&mut b, "parts.insert", json!({"family": "bearing", "size": "608"}));
        let (lo, hi) = bbox(&b, "Bearing 608");
        assert!((hi.x - lo.x - 22.0).abs() < 1e-6 && (hi.z - lo.z - 7.0).abs() < 1e-6);
        assert!(b.execute("parts.insert", &json!({"family": "hex_nut", "size": "M7"})).is_err());
    }

    #[test]
    fn a_screw_seats_in_a_hole_and_resizes() {
        let mut s = Session::default();
        run(&mut s, "solid.box", json!({"length": 40, "width": 30, "height": 10, "body_name": "Plate"}));
        run(&mut s, "solid.hole", json!({"position": [20, 15, 10], "diameter": 6.4}));
        // Pick the hole's rim at the top face.
        let r = run(&mut s, "parts.insert", json!({"family": "socket_head_cap_screw", "size": "M6", "length": 20, "at": [23.2, 15, 10]}));
        assert!(r["joint"].is_number(), "{r}");
        let (lo, hi) = bbox(&s, "SHCS M6x20");
        // The head sits flush on the top face, the shank down the hole's axis.
        assert!((hi.z - 16.0).abs() < 1e-6 && (lo.z + 10.0).abs() < 1e-6, "{lo:?} {hi:?}");
        assert!(((lo.x + hi.x) / 2.0 - 20.0).abs() < 1e-6 && ((lo.y + hi.y) / 2.0 - 15.0).abs() < 1e-6);
        // M8 x 25: rebuilt in place, still seated.
        let comp = r["component"].as_u64().unwrap_or(0);
        run(&mut s, "parts.resize", json!({"component": comp, "size": "M8", "length": 25}));
        let (lo, hi) = bbox(&s, "SHCS M8x25");
        assert!((hi.z - 18.0).abs() < 1e-6 && (lo.z + 15.0).abs() < 1e-6, "{lo:?} {hi:?}");
    }
}
