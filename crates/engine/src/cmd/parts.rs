//! Standard parts: ISO metric fasteners, dowel pins and ball bearings, generated in code from
//! the dimensions in the public standards tables (no vendor geometry). Each part is inserted as
//! its own component, built by ordinary features (so it stays parametric and editable), with a
//! cosmetic thread where it has one; at a hole it gets a rigid joint that puts its axis on the
//! hole's and seats it on the face.
//!
//! Part frame: the seating plane is z = 0 (the underside of a head, the bottom of a nut or
//! washer, the top of a set screw or pin or of a countersunk head); the shank runs down −z.
//!
//! Beside the built-in families, fastener families can come from part library files
//! (`parts.library.load`, see [`solidcraft_doc::part_family`]): rivets, bolts, screws, nuts,
//! washers and pins described by their sizes, heads and the lengths each size comes in with
//! the grip each length holds. Their parts are built from the same parametric shapes, and each
//! keeps a copy of its family in the design.

use serde_json::{Value, json};
use solidcraft_doc::part_family::{FastenerFamily, FastenerShape, HeadStyle, Library};
use solidcraft_geom::Vec3;

use super::CommandSpec;
use crate::params::{bad, bool_, num, str_, vec3};
use crate::{EngineError, Result, Session};

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("parts.library", "Standard Parts", library).icon("library").noundo().params(
        "family? → families (family, name, standard, source: built-in | a library's id | design, sizes, lengths) and, with family, each size's dimensions (mm; \
         a library family's sizes list their lengths with the grip [min, max] each holds); \
         check?: true with family (a library or design family) or path (a part library file) → {ok, problems: [{path, message}]} \
         (bad values, overlapping grips, sizes without lengths or listed twice, names of built-in families)",
    ),
    CommandSpec::new("parts.library.load", "Load Part Library", open_library).icon("library").noundo().params(
        "path: a part library (a solidcraft-parts/1 JSON file, or a folder with library.json in it) → {library, name, families, problems}; \
         its fastener families sit beside the built-in ones (parts.library, parts.insert, parts.resize) for the rest of the session. \
         A malformed file is refused with the JSON path of the first bad value, a family named like a built-in or another open library's family is refused; \
         opening a library again reloads it",
    ),
    CommandSpec::new("parts.fastener", "Insert Fastener", insert).at("SOLID", "INSERT").icon("fastener").params(
        "the Insert Part dialog's command (same as parts.insert): family, size, length?, at?|point?+direction?, name?",
    ),
    CommandSpec::new("parts.insert", "Insert Part", insert).icon("fastener").params(
        "family (socket_head_cap_screw | hex_bolt | hex_nut | washer | set_screw | dowel_pin | bearing, or a library family: parts.library); size (\"M6\", or \"608\" for bearings); \
         length? (mm; screws, bolts, set screws, pins; default a standard length near 2.5 d, a library family's shortest); \
         grip?: mm (a library family: the shortest length whose grip range holds it); \
         drawn?: \"set\" | \"unset\" (default unset; a library blind rivet set on `grip`: its shank that long, with its formed tail beyond); \
         at?: [x,y,z] on a hole's rim or wall (the part seats on the hole's face with a rigid joint) | point?: [x,y,z] with direction?: [x,y,z] (no joint); name?",
    ),
    CommandSpec::new("parts.resize", "Change Part Size", resize).icon("scale")
        .params("component: id|name of an inserted part; size?; length? | grip? (a library family); drawn?: set | unset (a blind rivet; a set one stays set, on `grip` if given) — rebuilds it (joints keep it seated)"),
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
    /// A head, a shank down −z, an optional hex socket in the head's top, an optional thread.
    Screw { d: f64, pitch: Option<f64>, len: f64, head: Head, socket: Option<(f64, f64)> },
    /// A hex prism with a through hole (nut), z 0…m.
    Nut { d: f64, pitch: Option<f64>, s: f64, m: f64 },
    /// A ring (washer, bearing envelope), z 0…h.
    Ring { d_in: f64, d_out: f64, h: f64 },
    /// A cylinder down −z with a socket in its top (set screw), or plain (pin).
    Rod { d: f64, pitch: Option<f64>, len: f64, socket: Option<(f64, f64)> },
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Head {
    Round {
        dk: f64,
        k: f64,
    },
    Hex {
        s: f64,
        k: f64,
    },
    /// A low rounded head (a quarter ellipse in profile).
    Dome {
        dk: f64,
        k: f64,
    },
    /// A cone below the seating plane, flush with it (the shank's length includes it).
    Countersunk {
        dk: f64,
        k: f64,
    },
}

fn shape(family: &str, size: &str, length: Option<f64>) -> std::result::Result<Shape, String> {
    let need = |what: &str| format!("no {what} size `{size}` (parts.library lists them)");
    let len = |d: f64| length.unwrap_or_else(|| default_length(d));
    Ok(match family {
        "socket_head_cap_screw" => {
            let (d, pitch) = metric(size).ok_or_else(|| need("metric"))?;
            let (_, dk, k, s, t) = SHCS.iter().find(|r| r.0.eq_ignore_ascii_case(size)).copied().ok_or_else(|| need("ISO 4762"))?;
            Shape::Screw { d, pitch: Some(pitch), len: len(d), head: Head::Round { dk, k }, socket: Some((s, t)) }
        }
        "hex_bolt" => {
            let (d, pitch) = metric(size).ok_or_else(|| need("metric"))?;
            let (_, s, k) = HEX_BOLT.iter().find(|r| r.0.eq_ignore_ascii_case(size)).copied().ok_or_else(|| need("ISO 4014"))?;
            Shape::Screw { d, pitch: Some(pitch), len: len(d), head: Head::Hex { s, k }, socket: None }
        }
        "hex_nut" => {
            let (d, pitch) = metric(size).ok_or_else(|| need("metric"))?;
            let (_, s, m) = HEX_NUT.iter().find(|r| r.0.eq_ignore_ascii_case(size)).copied().ok_or_else(|| need("ISO 4032"))?;
            Shape::Nut { d, pitch: Some(pitch), s, m }
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
/// Build `sh`; a blind rivet `set` on a grip is drawn closed: its shank that long, and beyond it
/// the tail it forms, a disc 1.5 × d across and d / 2 high.
fn build_drawn(s: &mut Session, sh: &Shape, set: Option<f64>) -> Result<()> {
    match (sh, set) {
        (Shape::Screw { d, len, .. }, Some(g)) if g > 0.0 && g < *len => {
            let d = *d;
            let mut short = sh.clone();
            if let Shape::Screw { len, .. } = &mut short {
                *len = g;
            }
            build(s, &short)?;
            cyl(s, d * 1.5, -g - d * 0.5, -g, "join")
        }
        _ => build(s, sh),
    }
}

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
                Head::Dome { dk, k } => {
                    let mut outline = vec![(0.0, -len), (d / 2.0, -len), (d / 2.0, 0.0)];
                    outline.extend((0..6).map(|i| {
                        let a = f64::from(i) / 6.0 * std::f64::consts::FRAC_PI_2;
                        (dk / 2.0 * a.cos(), k * a.sin())
                    }));
                    outline.push((0.0, k));
                    revolve(s, &outline, "new")?;
                    k
                }
                Head::Countersunk { dk, k } => {
                    revolve(s, &[(0.0, -len), (d / 2.0, -len), (d / 2.0, -k), (dk / 2.0, 0.0), (0.0, 0.0)], "new")?;
                    0.0
                }
            };
            if let Some((af, t)) = socket {
                hex(s, af, k, k - t, "cut")?;
            }
            if let Some(pitch) = pitch {
                thread(s, d, pitch, Vec3::new(d / 2.0, 0.0, -len / 2.0));
            }
        }
        Shape::Nut { d, pitch, s: af, m } => {
            hex(s, af, 0.0, m, "new")?;
            cyl(s, d, -1.0, m + 1.0, "cut")?;
            if let Some(pitch) = pitch {
                thread(s, d, pitch, Vec3::new(d / 2.0, 0.0, m / 2.0));
            }
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

/// A part library opened in the session (`parts.library.load`).
#[derive(Clone, Debug, PartialEq)]
pub struct PartLibrary {
    /// The file it was read from.
    pub path: String,
    pub library: Library,
}

fn builtin(family: &str) -> bool {
    FAMILIES.iter().any(|f| f.0 == family)
}

/// A library family by name (or `library-id/name`) and where it comes from: the open libraries
/// first, then the copies parts already in the design keep.
fn file_family(s: &Session, name: &str) -> Option<(FastenerFamily, String)> {
    let (lib, fam) = match name.split_once('/') {
        Some((l, f)) => (Some(l), f),
        None => (None, name),
    };
    for l in &s.part_libraries {
        if lib.is_some_and(|x| x != l.library.id) {
            continue;
        }
        if let Some(f) = l.library.families.iter().find(|f| f.name == fam) {
            return Some((f.clone(), l.library.id.clone()));
        }
    }
    if lib.is_some() {
        return None;
    }
    s.doc.parts.values().filter_map(|p| p.definition.as_ref()).find(|f| f.name == fam).map(|f| (f.clone(), "design".to_string()))
}

/// Every library family: the open libraries' and, after them, those only the design has.
fn file_families(s: &Session) -> Vec<(FastenerFamily, String)> {
    let mut out: Vec<(FastenerFamily, String)> =
        s.part_libraries.iter().flat_map(|l| l.library.families.iter().map(|f| (f.clone(), l.library.id.clone()))).collect();
    for f in s.doc.parts.values().filter_map(|p| p.definition.as_ref()) {
        if !out.iter().any(|(o, _)| o.name == f.name) {
            out.push((f.clone(), "design".into()));
        }
    }
    out
}

/// A library family's shape for `size`, and the listed length it was given or chose (by
/// `grip`, or the shortest when neither `length` nor `grip` is given).
fn file_shape(
    f: &FastenerFamily,
    size: &str,
    length: Option<f64>,
    grip: Option<f64>,
) -> std::result::Result<(Shape, Option<solidcraft_doc::part_family::FastenerLength>), String> {
    let z = f.size(size).ok_or_else(|| {
        let sizes: Vec<&str> = f.sizes.iter().map(|z| z.size.as_str()).collect();
        format!("no size `{size}` in family `{}` (sizes: {})", f.name, sizes.join(", "))
    })?;
    let pick = || -> std::result::Result<(f64, Option<solidcraft_doc::part_family::FastenerLength>), String> {
        if let Some(g) = grip {
            let l = z.length_for_grip(g).ok_or_else(|| format!("no length of {} {} holds a grip of {g} mm", f.name, z.size))?;
            return Ok((l.length, Some(l.clone())));
        }
        if let Some(l) = length {
            return Ok((l, z.lengths.iter().find(|x| (x.length - l).abs() < 1e-9).cloned()));
        }
        let l = z
            .lengths
            .iter()
            .min_by(|a, b| a.length.total_cmp(&b.length))
            .ok_or_else(|| format!("{} {} lists no lengths: give `length`", f.name, z.size))?;
        Ok((l.length, Some(l.clone())))
    };
    let head = z.head.ok_or_else(|| format!("{} {} has no head", f.name, z.size));
    Ok(match f.shape {
        FastenerShape::BlindRivet | FastenerShape::Bolt | FastenerShape::Screw => {
            let h = head?;
            let (len, l) = pick()?;
            let (dk, k) = (h.d, h.h);
            let shape_head = match h.style {
                HeadStyle::Hex => Head::Hex { s: dk, k },
                HeadStyle::Dome => Head::Dome { dk, k },
                HeadStyle::Flat | HeadStyle::Socket => Head::Round { dk, k },
                HeadStyle::Countersunk => Head::Countersunk { dk, k },
            };
            // A socket half the head across, half its height deep (an envelope, not a standard's).
            let socket = (h.style == HeadStyle::Socket).then_some((dk * 0.5, k * 0.5));
            let pitch = if f.shape == FastenerShape::BlindRivet { None } else { z.pitch };
            (Shape::Screw { d: z.d, pitch, len, head: shape_head, socket }, l)
        }
        FastenerShape::Nut => {
            let h = head?;
            (Shape::Nut { d: z.d, pitch: z.pitch, s: h.d, m: h.h }, None)
        }
        FastenerShape::Washer => {
            let h = head?;
            (Shape::Ring { d_in: z.d, d_out: h.d, h: h.h }, None)
        }
        FastenerShape::Pin => {
            let (len, l) = pick()?;
            (Shape::Rod { d: z.d, pitch: None, len, socket: None }, l)
        }
    })
}

fn family_label(f: &FastenerFamily) -> String {
    f.label.clone().unwrap_or_else(|| f.name.clone())
}

/// A library family as `parts.library` lists it.
fn file_json(f: &FastenerFamily, source: &str) -> Value {
    let sizes: Vec<Value> = f
        .sizes
        .iter()
        .map(|z| {
            let mut v = json!({"size": z.size, "d": z.d});
            if let Some(p) = z.pitch {
                v["pitch"] = json!(p);
            }
            if let Some(h) = z.head {
                v["head_diameter"] = json!(h.d);
                v["head_height"] = json!(h.h);
                v["head_style"] = json!(h.style);
            }
            if f.shape.has_lengths() {
                v["lengths"] = json!(z.lengths);
            }
            v
        })
        .collect();
    let mut lengths: Vec<f64> = f.sizes.iter().flat_map(|z| z.lengths.iter().map(|l| l.length)).collect();
    lengths.sort_by(f64::total_cmp);
    lengths.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
    json!({
        "family": f.name,
        "name": family_label(f),
        "standard": f.standard.clone().unwrap_or_default(),
        "source": source,
        "shape": f.shape,
        "sizes": sizes,
        "lengths": f.shape.has_lengths().then_some(lengths),
    })
}

/// Where a library path's manifest is: the file itself, or `library.json` in a folder.
fn manifest(cmd: &str, p: &Value) -> Result<String> {
    let path = str_(p, "path").ok_or_else(|| bad(cmd, "`path` is required: a part library file or folder"))?;
    if path.is_empty() || path.len() > 4096 || path.chars().any(char::is_control) {
        return Err(bad(cmd, "invalid part library path"));
    }
    let path =
        if path.to_ascii_lowercase().ends_with(".json") { path.to_string() } else { format!("{}/library.json", path.trim_end_matches(['/', '\\'])) };
    solidcraft_io::vfs::check(&path).map_err(|e| bad(cmd, e.to_string()))?;
    Ok(path)
}

fn read_library(cmd: &str, path: &str) -> Result<std::result::Result<Library, solidcraft_doc::part_family::Problem>> {
    let bytes = solidcraft_io::vfs::read(path, solidcraft_doc::part_family::MAX_BYTES).map_err(|e| bad(cmd, format!("{path}: {e}")))?;
    Ok(Library::decode(&bytes))
}

/// What keeps a library's families from sitting beside the others: a built-in's name, or the
/// name of a family in another open library.
fn clashes(s: &Session, lib: &Library) -> Vec<solidcraft_doc::part_family::Problem> {
    let mut out = Vec::new();
    for (i, f) in lib.families.iter().enumerate() {
        let path = format!("families[{i}].name");
        if builtin(&f.name) {
            out.push(solidcraft_doc::part_family::Problem { path, message: format!("`{}` is a built-in family", f.name) });
        } else if let Some(other) = s.part_libraries.iter().find(|l| l.library.id != lib.id && l.library.families.iter().any(|o| o.name == f.name)) {
            out.push(solidcraft_doc::part_family::Problem {
                path,
                message: format!("`{}` is already a family of the open library `{}`", f.name, other.library.id),
            });
        }
    }
    out
}

fn problems_json(problems: &[solidcraft_doc::part_family::Problem]) -> Value {
    json!({"ok": problems.is_empty(), "problems": problems})
}

fn open_library(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "parts.library.load";
    let path = manifest(cmd, p)?;
    let lib = read_library(cmd, &path)?.map_err(|e| bad(cmd, format!("{path}: {}: {}", e.path, e.message)))?;
    if let Some(c) = clashes(s, &lib).into_iter().next() {
        return Err(bad(cmd, format!("{path}: {}: {}", c.path, c.message)));
    }
    let problems: Vec<_> = lib.families.iter().enumerate().flat_map(|(i, f)| f.check(&format!("families[{i}]"))).collect();
    let out = json!({
        "library": lib.id,
        "name": lib.name,
        "path": path,
        "families": lib.families.iter().map(|f| f.name.clone()).collect::<Vec<_>>(),
        "problems": problems,
    });
    s.part_libraries.retain(|l| l.library.id != lib.id);
    s.part_libraries.push(PartLibrary { path, library: lib });
    Ok(out)
}

fn library(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "parts.library";
    let check = match p.get("check") {
        None | Some(Value::Null) => false,
        Some(_) => bool_(p, "check").ok_or_else(|| bad(cmd, "`check` must be true or false"))?,
    };
    if check && p.get("path").is_some() {
        let path = manifest(cmd, p)?;
        let lib = match read_library(cmd, &path)? {
            Ok(lib) => lib,
            Err(e) => return Ok(json!({"path": path, "ok": false, "problems": [e]})),
        };
        let mut problems = clashes(s, &lib);
        problems.extend(lib.families.iter().enumerate().flat_map(|(i, f)| f.check(&format!("families[{i}]"))));
        let mut out = problems_json(&problems);
        out["path"] = json!(path);
        out["library"] = json!(lib.id);
        return Ok(out);
    }
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
        json!({"family": id, "name": name, "standard": standard, "source": "built-in", "sizes": sizes, "lengths": lengths})
    };
    match str_(p, "family") {
        Some(f) if builtin(f) => Ok(if check { json!({"family": f, "ok": true, "problems": []}) } else { fam(f) }),
        Some(f) => {
            let (def, source) = file_family(s, f).ok_or_else(|| bad(cmd, format!("unknown part family `{f}`")))?;
            if !check {
                return Ok(file_json(&def, &source));
            }
            // Its path inside its library file, where it has one.
            let at = s
                .part_libraries
                .iter()
                .find(|l| l.library.id == source)
                .and_then(|l| l.library.families.iter().position(|x| x.name == def.name))
                .map_or_else(|| "family".to_string(), |i| format!("families[{i}]"));
            let mut out = problems_json(&def.check(&at));
            out["family"] = json!(def.name);
            out["source"] = json!(source);
            Ok(out)
        }
        None if check => Err(bad(cmd, "`check` needs `family` or `path`")),
        None => {
            let mut all: Vec<Value> = FAMILIES.iter().map(|f| fam(f.0)).collect();
            all.extend(file_families(s).iter().map(|(f, source)| file_json(f, source)));
            Ok(json!({"families": all}))
        }
    }
}

fn part_name(label: &str, size: &str, sh: &Shape) -> String {
    match sh {
        Shape::Screw { len, .. } | Shape::Rod { len, .. } => format!("{label} {size}x{len}"),
        _ => format!("{label} {size}"),
    }
}

/// A part to build: its shape, name prefix, the family copy it keeps (a library family) and the
/// listed length it was given or chose.
struct Resolved {
    shape: Shape,
    label: String,
    definition: Option<FastenerFamily>,
    listed: Option<solidcraft_doc::part_family::FastenerLength>,
}

impl Resolved {
    /// The article number the library lists for this size and length.
    fn article(&self) -> Option<String> {
        self.listed.as_ref().and_then(|l| l.article.clone())
    }
}

/// A part's component name: label, size and length, and its article number where the library
/// gives one (so two families' parts of one size and length differ in a parts list).
fn resolved_name(r: &Resolved, size: &str) -> String {
    let name = part_name(&r.label, size, &r.shape);
    match r.article() {
        Some(a) => format!("{name} {a}"),
        None => name,
    }
}

fn resolve(s: &Session, family: &str, size: &str, length: Option<f64>, grip: Option<f64>) -> std::result::Result<Resolved, String> {
    if builtin(family) {
        if grip.is_some() {
            return Err(format!("`grip` needs a library family: `{family}` lists no grips"));
        }
        let shape = shape(family, size, length)?;
        return Ok(Resolved { shape, label: label(family).to_string(), definition: None, listed: None });
    }
    let (def, _) = file_family(s, family).ok_or_else(|| format!("unknown part family `{family}` (parts.library lists them)"))?;
    let (shape, listed) = file_shape(&def, size, length, grip)?;
    Ok(Resolved { shape, label: family_label(&def), definition: Some(def), listed })
}

fn grip_param(p: &Value, cmd: &str) -> Result<Option<f64>> {
    match p.get("grip") {
        None | Some(Value::Null) => Ok(None),
        Some(_) => {
            Ok(Some(num(p, "grip").filter(|g| g.is_finite() && *g >= 0.0 && *g <= 2000.0).ok_or_else(|| bad(cmd, "`grip` must be 0…2000 mm"))?))
        }
    }
}

/// What the insert and resize results say about a part.
fn describe(out: &mut Value, r: &Resolved) {
    out["length"] = json!(shape_len(&r.shape));
    if let Some(l) = &r.listed {
        if let Some(g) = l.grip {
            out["grip"] = json!(g);
        }
        if let Some(a) = &l.article {
            out["article"] = json!(a);
        }
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
    let (length, grip) = (length_param(p, cmd)?, grip_param(p, cmd)?);
    let set = match str_(p, "drawn") {
        None | Some("unset") => None,
        Some("set") => Some(grip.ok_or_else(|| bad(cmd, "`drawn: set` needs the `grip` the rivet closes on"))?),
        Some(d) => return Err(bad(cmd, format!("`drawn` must be set or unset, not `{d}`"))),
    };
    if set.is_some() && !blind_rivet(s, &family) {
        return Err(bad(cmd, "`drawn: set` is for blind rivets"));
    }
    // A set rivet of a given length closes on its grip; without a length the grip picks it.
    let r = resolve(s, &family, &size, length, if set.is_some() && length.is_some() { None } else { grip }).map_err(|e| bad(cmd, e))?;
    let family = r.definition.as_ref().map_or(family, |d| d.name.clone());
    let sh = r.shape.clone();
    // A pick on a hole's rim also touches its end face: step onto the wall just inside the rim.
    let at = p.get("at").and_then(vec3).map(|a| hole_wall(s, a).unwrap_or(a));
    // The occurrence the hole belongs to, found before the part exists.
    let host = match at {
        Some(a) => Some(super::joints::occurrence_at(s, a).ok_or_else(|| bad(cmd, "no body at `at` (pick a hole's rim or wall)"))?),
        None => None,
    };
    let name = str_(p, "name").map(str::to_string).unwrap_or_else(|| resolved_name(&r, &size));
    let active = s.active_component;
    let c = run(s, "component.create", json!({"name": name}))?;
    let (comp, occ) = (c["component"].as_u64().unwrap_or(0), c["occurrence"].as_u64().unwrap_or(0));
    build_drawn(s, &sh, set)?;
    let part = solidcraft_doc::StandardPart {
        family: family.clone(),
        size: size.clone(),
        length: shape_len(&sh),
        definition: r.definition.clone(),
        article: r.article(),
        set,
    };
    s.doc_mut().parts.insert(comp, part);
    run(s, "component.activate", json!({"component": active}))?;
    let mut out = json!({"component": comp, "occurrence": occ, "name": name, "family": family, "size": size});
    describe(&mut out, &r);
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

/// Insert a part (the `parts.insert` command's run), for commands that place several.
pub(super) fn insert_part(s: &mut Session, p: &Value) -> Result<Value> {
    insert(s, p)
}

/// A component that already holds `family` `size` at `length` (from the same library
/// definition), to place again rather than build again.
pub(super) fn existing_part(s: &Session, family: &str, size: &str, length: Option<f64>, set: Option<f64>) -> Option<u64> {
    let (name, def) = if builtin(family) {
        (family.to_string(), None)
    } else {
        let (d, _) = file_family(s, family)?;
        (d.name.clone(), Some(d))
    };
    let same_length = |a: Option<f64>| match (a, length) {
        (Some(a), Some(b)) => (a - b).abs() < 1e-9,
        (None, None) => true,
        _ => false,
    };
    s.doc
        .parts
        .iter()
        .find(|(c, p)| {
            p.family == name
                && p.size == size
                && same_length(p.length)
                && p.set.zip(set).map_or(p.set.is_none() && set.is_none(), |(a, b)| (a - b).abs() < 1e-6)
                && p.definition == def
                && s.doc.occurrence_of(**c).is_some()
        })
        .map(|(c, _)| *c)
}

/// Whether `family` is a library family of blind rivets (drawn set when placed in a stack).
pub(super) fn blind_rivet(s: &Session, family: &str) -> bool {
    !builtin(family) && file_family(s, family).is_some_and(|(d, _)| d.shape == FastenerShape::BlindRivet)
}

/// A family's sizes and their nominal diameters (a washer's or bearing's: its bore).
pub(super) fn sizes(s: &Session, family: &str) -> std::result::Result<Vec<(String, f64)>, String> {
    let m = |n: &str| metric(n).map_or(0.0, |x| x.0);
    Ok(match family {
        "socket_head_cap_screw" => SHCS.iter().map(|r| (r.0.to_string(), m(r.0))).collect(),
        "hex_bolt" => HEX_BOLT.iter().map(|r| (r.0.to_string(), m(r.0))).collect(),
        "hex_nut" => HEX_NUT.iter().map(|r| (r.0.to_string(), m(r.0))).collect(),
        "washer" => WASHER.iter().map(|r| (r.0.to_string(), r.1)).collect(),
        "set_screw" => SET_SCREW.iter().map(|r| (r.0.to_string(), m(r.0))).collect(),
        "dowel_pin" => DOWEL.iter().map(|d| (format!("{d}"), *d)).collect(),
        "bearing" => BEARINGS.iter().map(|r| (r.0.to_string(), r.1)).collect(),
        _ => {
            let (def, _) = file_family(s, family).ok_or_else(|| format!("unknown part family `{family}` (parts.library lists them)"))?;
            def.sizes.iter().map(|z| (z.size.clone(), z.d)).collect()
        }
    })
}

/// How thick a washer or nut of `family` `size` is.
pub(super) fn thickness(s: &Session, family: &str, size: &str) -> std::result::Result<f64, String> {
    match resolve(s, family, size, None, None)?.shape {
        Shape::Ring { h, .. } => Ok(h),
        Shape::Nut { m, .. } => Ok(m),
        _ => Err(format!("`{family}` is not a washer or a nut")),
    }
}

/// The thread pitch of a threaded size.
pub(super) fn pitch(s: &Session, family: &str, size: &str) -> Option<f64> {
    if builtin(family) {
        return matches!(family, "socket_head_cap_screw" | "hex_bolt" | "hex_nut" | "set_screw").then(|| metric(size).map(|m| m.1)).flatten();
    }
    file_family(s, family).and_then(|(def, _)| def.size(size).and_then(|z| z.pitch))
}

/// A length of `family` `size` chosen by [`length_for`]: the grip range and article number the
/// library lists for it.
pub(super) struct Chosen {
    pub length: f64,
    pub grip: Option<[f64; 2]>,
    pub article: Option<String>,
}

/// The length of `family` `size` for a stack `clamp` thick. By grip (`by_grip`), a library
/// size's shortest length whose grip range holds `clamp`; where the lengths list no grips, or
/// not by grip, the shortest length at least `clamp + extra` (room for a nut, or the thread a
/// screw engages).
pub(super) fn length_for(s: &Session, family: &str, size: &str, clamp: f64, extra: f64, by_grip: bool) -> std::result::Result<Chosen, String> {
    let need = clamp + extra;
    if builtin(family) {
        if !matches!(family, "socket_head_cap_screw" | "hex_bolt" | "set_screw" | "dowel_pin") {
            return Err(format!("`{family}` has no lengths"));
        }
        return LENGTHS
            .iter()
            .find(|l| **l >= need - 1e-9)
            .map(|l| Chosen { length: *l, grip: None, article: None })
            .ok_or_else(|| format!("{family} {size}: no standard length is {need:.3} mm or more"));
    }
    let (def, _) = file_family(s, family).ok_or_else(|| format!("unknown part family `{family}`"))?;
    if !def.shape.has_lengths() {
        return Err(format!("`{family}` has no lengths"));
    }
    let z = def.size(size).ok_or_else(|| format!("no size `{size}` in `{family}`"))?;
    let chosen = |l: &solidcraft_doc::part_family::FastenerLength| Chosen { length: l.length, grip: l.grip, article: l.article.clone() };
    if by_grip && z.lengths.iter().any(|l| l.grip.is_some()) {
        return z.length_for_grip(clamp).map(chosen).ok_or_else(|| format!("no length of {family} {size} holds a grip of {clamp:.3} mm"));
    }
    z.lengths
        .iter()
        .filter(|l| l.length >= need - 1e-9)
        .min_by(|a, b| a.length.total_cmp(&b.length))
        .map(chosen)
        .ok_or_else(|| format!("{family} {size}: no length is {need:.3} mm or more"))
}

/// A point on the cylindrical wall of the hole at `a`, a little in from the picked rim.
pub(super) fn hole_wall(s: &Session, a: Vec3) -> Option<Vec3> {
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
    let grip = grip_param(p, cmd)?;
    // A set rivet stays set, on the grip given if one is.
    let set = match str_(p, "drawn") {
        None => part.set.map(|g| grip.unwrap_or(g)),
        Some("unset") => None,
        Some("set") if blind_rivet(s, &part.family) => {
            Some(grip.or(part.set).ok_or_else(|| bad(cmd, "`drawn: set` needs the `grip` the rivet closes on"))?)
        }
        Some("set") => return Err(bad(cmd, "`drawn: set` is for blind rivets")),
        Some(d) => return Err(bad(cmd, format!("`drawn` must be set or unset, not `{d}`"))),
    };
    let length = if grip.is_some() { None } else { length_param(p, cmd)?.or(if size == part.size { part.length } else { None }) };
    // A library part rebuilds from the copy of its family it keeps.
    let r = match &part.definition {
        Some(def) => {
            let (shape, listed) = file_shape(def, &size, length, grip).map_err(|e| bad(cmd, e))?;
            Resolved { shape, label: family_label(def), definition: Some(def.clone()), listed }
        }
        None => resolve(s, &part.family, &size, length, grip).map_err(|e| bad(cmd, e))?,
    };
    let sh = r.shape.clone();
    // Rebuild: the component's features go, the new shape is built in their place.
    let ids: Vec<u64> = s.doc.features.iter().filter(|f| f.component == comp).map(|f| f.id).collect();
    for id in ids.iter().rev() {
        let _ = s.doc_mut().delete_feature(*id);
    }
    let active = s.active_component;
    run(s, "component.activate", json!({"component": comp}))?;
    build_drawn(s, &sh, set)?;
    let name = resolved_name(&r, &size);
    let standard = solidcraft_doc::StandardPart {
        family: part.family.clone(),
        size: size.clone(),
        length: shape_len(&sh),
        definition: r.definition.clone(),
        article: r.article(),
        set,
    };
    s.doc_mut().parts.insert(comp, standard);
    if let Some(c) = s.doc_mut().components.iter_mut().find(|c| c.id == comp) {
        c.name = name.clone();
    }
    run(s, "component.activate", json!({"component": active}))?;
    s.refresh();
    if let Some(e) = s.model.results.iter().find_map(|r| r.error.clone()) {
        return Err(EngineError::Other(e));
    }
    let mut out = json!({"component": comp, "name": name, "size": size});
    describe(&mut out, &r);
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

#[cfg(test)]
#[path = "parts_library_tests.rs"]
mod library_tests;
