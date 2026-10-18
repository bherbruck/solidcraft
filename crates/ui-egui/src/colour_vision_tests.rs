//! Colour vision modes (#778), measured. Colours that appear together must stay apart (CIEDE2000
//! 10 or more) for normal vision and for the loss each mode is for, with two simulations of that
//! loss: Machado, Oliveira and Fernandes 2009 (severity 1) and Brettel, Viénot and Mollon 1997.
//! The instruments are checked first: CIEDE2000 on the 34 pairs Sharma, Wu and Dalal published
//! with their paper, Brettel on white and its own anchors.
//!
//! When a token edit breaks a mode here, adjust its measured variant. Shared icon accents live
//! in `assets/icons/ui/design/palette.toml`; `assets/styles/ui/palettes.rs` links those generated
//! values to UI tokens and owns the remaining UI-only colour-vision adjustments.

use std::collections::BTreeSet;

use egui::Color32;
use serde_json::json;

use crate::theme::{ColorVision, Tokens};

type Rgb8 = [u8; 3];
type V3 = [f64; 3];
type M3 = [[f64; 3]; 3];

/// The smallest difference a pair may have (CIEDE2000).
const FLOOR: f64 = 10.0;
/// The shading the analysis colours get in the shader (`lit`: 0.55 turned away, 1 facing the eye).
const LIT: [f64; 4] = [0.55, 0.7, 0.85, 1.0];
const MODES: [ColorVision; 3] = [ColorVision::Protan, ColorVision::Deutan, ColorVision::Tritan];

#[derive(Clone, Copy, Debug, PartialEq)]
enum Eye {
    Normal,
    Protan,
    Deutan,
    Tritan,
}

#[derive(Clone, Copy, Debug)]
enum Instrument {
    Machado,
    Brettel,
}

/// The loss a mode is for.
fn eye(mode: ColorVision) -> Eye {
    match mode {
        ColorVision::Off => Eye::Normal,
        ColorVision::Protan => Eye::Protan,
        ColorVision::Deutan => Eye::Deutan,
        ColorVision::Tritan => Eye::Tritan,
    }
}

// ---- the instruments ----

fn to_lin(c: f64) -> f64 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn to_srgb(c: f64) -> f64 {
    if c <= 0.0031308 { 12.92 * c } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }
}

fn mul(m: &M3, v: V3) -> V3 {
    [0, 1, 2].map(|r| m[r][0] * v[0] + m[r][1] * v[1] + m[r][2] * v[2])
}

fn mat(a: &M3, b: &M3) -> M3 {
    [0, 1, 2].map(|r| [0, 1, 2].map(|c| (0..3).map(|k| a[r][k] * b[k][c]).sum()))
}

fn inv(m: &M3) -> M3 {
    let [[a, b, c], [d, e, f], [g, h, i]] = *m;
    let det = a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g);
    [
        [(e * i - f * h) / det, (c * h - b * i) / det, (b * f - c * e) / det],
        [(f * g - d * i) / det, (a * i - c * g) / det, (c * d - a * f) / det],
        [(d * h - e * g) / det, (b * g - a * h) / det, (a * e - b * d) / det],
    ]
}

fn cross(u: V3, v: V3) -> V3 {
    [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]]
}

fn dot(u: V3, v: V3) -> f64 {
    u[0] * v[0] + u[1] * v[1] + u[2] * v[2]
}

/// Machado, Oliveira, Fernandes 2009, severity 1.0, on linear sRGB (the authors' Table 1).
fn machado(eye: Eye) -> M3 {
    match eye {
        Eye::Protan => [[0.152286, 1.052583, -0.204868], [0.114503, 0.786281, 0.099216], [-0.003882, -0.048116, 1.051998]],
        Eye::Deutan => [[0.367322, 0.860646, -0.227968], [0.280085, 0.672501, 0.047413], [-0.011820, 0.042940, 0.968881]],
        Eye::Tritan => [[1.255528, -0.076749, -0.178779], [-0.078411, 0.930809, 0.147602], [0.004733, 0.691367, 0.303900]],
        Eye::Normal => [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
    }
}

/// Linear sRGB to CIE XYZ (D65), IEC 61966-2-1.
const XYZ_FROM_LIN: M3 = [[0.4124564, 0.3575761, 0.1804375], [0.2126729, 0.7151522, 0.0721750], [0.0193339, 0.1191920, 0.9503041]];
/// XYZ to cone responses (LMS), Smith and Pokorny 1975.
const LMS_FROM_XYZ: M3 = [[0.15514, 0.54312, -0.03286], [-0.15514, 0.45684, 0.03286], [0.0, 0.0, 0.00801]];
/// CIE 1931 2-degree colour matching functions (CIE 018:2019) at Brettel's anchors.
const CIE_475: V3 = [0.1421, 0.1126, 1.0419];
const CIE_485: V3 = [0.05795001, 0.1693, 0.6162];
const CIE_575: V3 = [0.8425, 0.9154, 0.0018];
const CIE_660: V3 = [0.1649, 0.061, 0.0];

/// The missing cone (0 L, 1 M, 2 S) and the two anchor wavelengths of its half-planes.
fn anchors(eye: Eye) -> (usize, V3, V3) {
    match eye {
        Eye::Protan => (0, CIE_475, CIE_575),
        Eye::Deutan => (1, CIE_475, CIE_575),
        Eye::Normal | Eye::Tritan => (2, CIE_485, CIE_660),
    }
}

/// Brettel, Viénot, Mollon 1997: the missing cone's response is put back on one of two half-planes
/// through white and an anchor colour, chosen by the side of the white axis the colour is on.
fn brettel(eye: Eye, lin: V3) -> V3 {
    let lms = mat(&LMS_FROM_XYZ, &XYZ_FROM_LIN);
    let (c, w1, w2) = anchors(eye);
    let white = mul(&lms, [1.0; 3]);
    let (a1, a2) = (mul(&LMS_FROM_XYZ, w1), mul(&LMS_FROM_XYZ, w2));
    let mut axis = [0.0; 3];
    axis[c] = 1.0;
    let sep = cross(white, axis);
    let mut q = mul(&lms, lin);
    let n = if dot(q, sep) * dot(a1, sep) >= 0.0 { cross(white, a1) } else { cross(white, a2) };
    q[c] = -(0..3).filter(|k| *k != c).map(|k| n[k] * q[k]).sum::<f64>() / n[c];
    mul(&inv(&lms), q)
}

/// What an eye sees of an sRGB colour, as sRGB 0..1.
fn simulate(c: Rgb8, eye: Eye, how: Instrument) -> V3 {
    let srgb = c.map(|v| f64::from(v) / 255.0);
    if eye == Eye::Normal {
        return srgb;
    }
    let lin = srgb.map(to_lin);
    let out = match how {
        Instrument::Machado => mul(&machado(eye), lin),
        Instrument::Brettel => brettel(eye, lin),
    };
    out.map(|v| to_srgb(v.clamp(0.0, 1.0)))
}

/// CIELAB (D65) of an sRGB colour.
fn lab(srgb: V3) -> V3 {
    let [x, y, z] = mul(&XYZ_FROM_LIN, srgb.map(to_lin));
    let f = |t: f64| if t > (6.0f64 / 29.0).powi(3) { t.cbrt() } else { t / (3.0 * (6.0f64 / 29.0).powi(2)) + 4.0 / 29.0 };
    let (fx, fy, fz) = (f(x / 0.95047), f(y), f(z / 1.08883));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

/// CIEDE2000 (Sharma, Wu, Dalal 2005).
fn de2000(l1: V3, l2: V3) -> f64 {
    let ([lightness1, a1, b1], [lightness2, a2, b2]) = (l1, l2);
    let (c1, c2) = (a1.hypot(b1), a2.hypot(b2));
    let cb = (c1 + c2) / 2.0;
    let g = 0.5 * (1.0 - (cb.powi(7) / (cb.powi(7) + 25f64.powi(7))).sqrt());
    let (a1p, a2p) = ((1.0 + g) * a1, (1.0 + g) * a2);
    let (c1p, c2p) = (a1p.hypot(b1), a2p.hypot(b2));
    let hue = |b: f64, a: f64, c: f64| if c == 0.0 { 0.0 } else { b.atan2(a).to_degrees().rem_euclid(360.0) };
    let (h1p, h2p) = (hue(b1, a1p, c1p), hue(b2, a2p, c2p));
    let dlp = lightness2 - lightness1;
    let dcp = c2p - c1p;
    let dhp = if c1p * c2p == 0.0 {
        0.0
    } else if (h2p - h1p).abs() <= 180.0 {
        h2p - h1p
    } else if h2p - h1p > 180.0 {
        h2p - h1p - 360.0
    } else {
        h2p - h1p + 360.0
    };
    let dhp_big = 2.0 * (c1p * c2p).sqrt() * (dhp / 2.0).to_radians().sin();
    let lbp = (lightness1 + lightness2) / 2.0;
    let cbp = (c1p + c2p) / 2.0;
    let hbp = if c1p * c2p == 0.0 {
        h1p + h2p
    } else if (h1p - h2p).abs() <= 180.0 {
        (h1p + h2p) / 2.0
    } else if h1p + h2p < 360.0 {
        (h1p + h2p + 360.0) / 2.0
    } else {
        (h1p + h2p - 360.0) / 2.0
    };
    let cos = |deg: f64| deg.to_radians().cos();
    let t = 1.0 - 0.17 * cos(hbp - 30.0) + 0.24 * cos(2.0 * hbp) + 0.32 * cos(3.0 * hbp + 6.0) - 0.20 * cos(4.0 * hbp - 63.0);
    let dtheta = 30.0 * (-((hbp - 275.0) / 25.0).powi(2)).exp();
    let rc = 2.0 * (cbp.powi(7) / (cbp.powi(7) + 25f64.powi(7))).sqrt();
    let sl = 1.0 + 0.015 * (lbp - 50.0).powi(2) / (20.0 + (lbp - 50.0).powi(2)).sqrt();
    let sc = 1.0 + 0.045 * cbp;
    let sh = 1.0 + 0.015 * cbp * t;
    let rt = -(2.0 * dtheta).to_radians().sin() * rc;
    ((dlp / sl).powi(2) + (dcp / sc).powi(2) + (dhp_big / sh).powi(2) + rt * (dcp / sc) * (dhp_big / sh)).sqrt()
}

fn diff(a: Rgb8, b: Rgb8, eye: Eye, how: Instrument) -> f64 {
    de2000(lab(simulate(a, eye, how)), lab(simulate(b, eye, how)))
}

fn normal(a: Rgb8, b: Rgb8) -> f64 {
    diff(a, b, Eye::Normal, Instrument::Machado)
}

/// The smallest difference for normal vision and for the mode's loss (both instruments).
fn worst(mode: ColorVision, a: Rgb8, b: Rgb8) -> f64 {
    let e = eye(mode);
    normal(a, b).min(diff(a, b, e, Instrument::Machado)).min(diff(a, b, e, Instrument::Brettel))
}

// ---- the colours that appear together ----

fn rgb(c: Color32) -> Rgb8 {
    [c.r(), c.g(), c.b()]
}

/// A translucent colour over an opaque ground, in sRGB (close to egui's blend).
fn over(c: Color32, ground: Color32) -> Rgb8 {
    let [r, g, b, a] = c.to_srgba_unmultiplied();
    let k = f64::from(a) / 255.0;
    let mix = |f: u8, g: u8| (f64::from(f) * k + f64::from(g) * (1.0 - k)).round() as u8;
    [mix(r, ground.r()), mix(g, ground.g()), mix(b, ground.b())]
}

/// An analysis colour as the shader shades it.
fn lit(c: [f32; 3], k: f64) -> Rgb8 {
    c.map(|x| ((f64::from(x) * 255.0).round() * k).min(255.0).round() as u8)
}

struct Group {
    name: &'static str,
    /// Colours that must differ from each other.
    members: Vec<(&'static str, Rgb8)>,
    /// What they are drawn on.
    grounds: Vec<(&'static str, Rgb8)>,
}

fn group(name: &'static str, members: &[(&'static str, Color32)], grounds: &[(&'static str, Color32)]) -> Group {
    for (n, c) in members.iter().chain(grounds) {
        assert_eq!(c.a(), 255, "{name}: {n} is translucent; put it over its ground first");
    }
    Group { name, members: members.iter().map(|(n, c)| (*n, rgb(*c))).collect(), grounds: grounds.iter().map(|(n, c)| (*n, rgb(*c))).collect() }
}

/// Every group of meaning-carrying colours that appear together (and the icons' colours, drawn on
/// the toolbar with their caller's ink, fill and accent). Each colour is named by its `Tokens`
/// field, so `every_token_is_measured_or_carries_no_meaning` knows which tokens are measured.
fn groups(t: &Tokens) -> Vec<Group> {
    let lift = |c: Color32| {
        Color32::from_rgb(c.r().saturating_add(t.hover_face_lift), c.g().saturating_add(t.hover_face_lift), c.b().saturating_add(t.hover_face_lift))
    };
    let ic = &t.icons;
    let a = &t.analysis;
    let mut out = vec![
        group(
            "sketch lines",
            &[
                ("sketch", t.sketch),
                ("sketch_fixed", t.sketch_fixed),
                ("sketch_locked", t.sketch_locked),
                ("sketch_projected", t.sketch_projected),
                ("sketch_lost", t.sketch_lost),
                ("sketch_construction", t.sketch_construction),
                ("sketch_centerline", t.sketch_centerline),
                ("preview_sketch", t.preview_sketch),
            ],
            &[("viewport_top", t.viewport_top), ("viewport_bottom", t.viewport_bottom)],
        ),
        group(
            "sketch points",
            &[("sketch_point", t.sketch_point), ("sketch_point_free", t.sketch_point_free), ("sketch_locked", t.sketch_locked)],
            &[("viewport_top", t.viewport_top)],
        ),
        group(
            "timeline",
            &[("timeline_item", t.timeline_item), ("timeline_item_error", t.timeline_item_error), ("hover", t.hover), ("accent_soft", t.accent_soft)],
            &[],
        ),
        group("UI text", &[("error", t.error), ("warning", t.warning), ("text", t.text)], &[("panel", t.panel)]),
        group("origin axes", &[("axis_x", t.axis_x), ("axis_y", t.axis_y), ("axis_z", t.axis_z)], &[("viewport_top", t.viewport_top)]),
        group("view triad", &[("triad[0]", t.triad[0]), ("triad[1]", t.triad[1]), ("triad[2]", t.triad[2])], &[("viewport_top", t.viewport_top)]),
        group("gizmo", &[("gizmo[0]", t.gizmo[0]), ("gizmo[1]", t.gizmo[1]), ("gizmo[2]", t.gizmo[2])], &[("viewport_top", t.viewport_top)]),
        group("bend lines", &[("bend_up", t.bend_up), ("bend_down", t.bend_down)], &[("viewport_top", t.viewport_top)]),
        group("box select", &[("box_window", t.box_window), ("box_crossing", t.box_crossing)], &[("viewport_top", t.viewport_top)]),
        group(
            "selection",
            &[
                ("body", t.body),
                ("sel_face", t.sel_face),
                ("body_selected", t.body_selected),
                ("hover_face_lift", lift(t.body)),
                ("preview_add", t.preview_add),
                ("preview_new", t.preview_new),
            ],
            &[],
        ),
        group("section", &[("section_cap", t.section_cap)], &[("body", t.body)]),
        group("interference", &[("interference", t.interference)], &[("body", t.body)]),
        group(
            "continuity seams",
            &[
                ("continuity[0]", t.continuity[0]),
                ("continuity[1]", t.continuity[1]),
                ("continuity[2]", t.continuity[2]),
                ("continuity[3]", t.continuity[3]),
            ],
            &[("body", t.body), ("viewport_top", t.viewport_top)],
        ),
        // The scale's three stops, unlit. Shaded (a stop at one angle to the light, the next at
        // another), middle and tight merge for a red-green loss (3.3 protan, 5.2 deutan) and flat
        // and middle for tritan (4.6). That is the scale's own fault, in Off too: #975.
        group(
            "curvature map",
            &[
                ("analysis.curvature[0]", crate::theme::srgb(a.curvature[0])),
                ("analysis.curvature[1]", crate::theme::srgb(a.curvature[1])),
                ("analysis.curvature[2]", crate::theme::srgb(a.curvature[2])),
            ],
            &[],
        ),
        group(
            "ribbon accents",
            &[("glyph_text", t.glyph_text), ("sketch_accent", t.sketch_accent), ("construction_geom", t.construction_geom)],
            &[("toolbar", t.toolbar)],
        ),
        group(
            "origin icon",
            &[("icons.axes[0]", ic.axes[0]), ("icons.axes[1]", ic.axes[1]), ("icons.axes[2]", ic.axes[2])],
            &[("toolbar", t.toolbar)],
        ),
        group(
            "accessibility icon",
            &[("icons.access[0]", ic.access[0]), ("icons.access[1]", ic.access[1]), ("icon_fill", t.icon_fill), ("icon", t.icon)],
            &[],
        ),
        group("draft icon", &[("icons.draft[0]", ic.draft[0]), ("icons.draft[1]", ic.draft[1]), ("icon", t.icon)], &[]),
        group(
            "curvature icon",
            &[("icons.curvature[0]", ic.curvature[0]), ("icons.curvature[1]", ic.curvature[1]), ("icons.curvature[2]", ic.curvature[2])],
            &[],
        ),
        group(
            "appearance icon",
            &[("icons.paint[0]", ic.paint[0]), ("icons.paint[1]", ic.paint[1]), ("icons.paint[2]", ic.paint[2]), ("icon_fill", t.icon_fill)],
            &[],
        ),
        group("environment icon", &[("icons.sky", ic.sky), ("icons.floor", ic.floor), ("icons.highlight", ic.highlight)], &[]),
        group("warning icon", &[("icons.warning", ic.warning), ("icon", t.icon)], &[("toolbar", t.toolbar)]),
        group("interference icon", &[("icons.interference", ic.interference), ("icon", t.icon)], &[("toolbar", t.toolbar)]),
        group("folder icons", &[("icons.folder", ic.folder), ("icons.plane", ic.plane), ("icons.fold", ic.fold), ("icon", t.icon)], &[]),
        group("sheet icons", &[("icons.sheet", ic.sheet), ("icons.sheet_2", ic.sheet_2), ("icon", t.icon)], &[]),
        group(
            "app bar buttons",
            &[("app_bar_icon[0]", t.app_bar_icon[0]), ("app_bar_icon[1]", t.app_bar_icon[1]), ("app_bar_icon[2]", t.app_bar_icon[2])],
            &[("app_bar", t.app_bar)],
        ),
        group("logo", &[("logo[0]", t.logo[0]), ("logo[1]", t.logo[1]), ("logo[2]", t.logo[2])], &[("app_bar", t.app_bar)]),
    ];
    // The removed part of a cut preview is translucent over the body.
    if let Some(sel) = out.iter_mut().find(|g| g.name == "selection") {
        sel.members.push(("preview_cut", over(t.preview_cut, t.body)));
    }
    out
}

/// Measured by the shaded check in `every_mode_keeps_colours_apart_for_its_eye`.
const SHADED: [&str; 2] = ["analysis.draft", "analysis.access"];

/// Tokens no colour vision mode needs to keep apart, and why. A new token fails
/// `every_token_is_measured_or_carries_no_meaning` until it is in a group above or here.
const NO_MEANING: &[(&str, &str)] = &[
    // The window's chrome: grounds, text and borders (measured as grounds where meaning sits on them).
    ("panel_header", "chrome"),
    ("border", "chrome"),
    ("text_dim", "secondary text"),
    ("accent", "the one interface accent"),
    ("timeline", "the timeline's ground"),
    ("dialog_bg", "chrome"),
    ("dialog_border", "chrome"),
    ("chip", "a label's ground"),
    ("heading_bg", "chrome"),
    ("subheading_bg", "chrome"),
    ("overlay", "a chip's ground over the view"),
    ("app_bar_text", "text"),
    ("app_bar_hover", "a hovered tab, translucent white"),
    ("timeline_marker", "the rollback marker, alone"),
    ("field", "a field's ground"),
    // Highlights: shades of the accent blue over what they mark, by design close to each other.
    ("sel_edge", "selection highlight"),
    ("sel_edge_rim", "selection highlight"),
    ("hover_edge", "hover highlight"),
    ("hover_edge_halo", "hover highlight"),
    ("sel_profile", "selection highlight"),
    ("sel_profile_edge", "selection highlight"),
    ("hover_profile", "hover highlight"),
    ("hover_profile_edge", "hover highlight"),
    ("sel_vertex", "selection highlight"),
    ("rubber_band", "the shape being drawn"),
    ("manipulator", "a drag handle, alone"),
    // Planes, points, the grid and the model's edges: one neutral each, told apart by shape.
    ("origin_plane", "a plane, told apart by its place"),
    ("origin_plane_edge", "a plane's edge"),
    ("origin_plane_hover", "hover highlight"),
    ("origin_point", "the origin, alone"),
    ("construction_plane", "a plane"),
    ("body_edge", "the model's edges"),
    ("grid", "the grid"),
    ("grid_major", "the grid"),
    ("profile_fill", "a closed profile's tint"),
    ("dimension", "dimension text"),
    ("dimension_driven", "dimension text, dimmed"),
    ("glyph_bg", "a constraint glyph's ground"),
    ("glyph_edge", "a constraint glyph's edge"),
    // The navigation cube: labelled faces.
    ("cube_face", "labelled"),
    ("cube_border", "labelled"),
    ("cube_text", "labelled"),
    ("cube_hover", "hover highlight"),
    // Not a colour, or the end of a mark that is measured elsewhere.
    ("analysis.hatch", "a flag: stripes on the failing faces"),
    ("logo[3]", "the mark's white edge"),
    // Icon colours that are a shade of a measured one, or one mark alone.
    ("icons.sheet_light", "a lighter shade of a sheet"),
    ("icons.sheet_line", "lines on a sheet"),
    ("icons.glint", "a highlight"),
    ("icons.stripe", "zebra stripes, grey"),
    ("icons.agent", "the agent's pointer, alone"),
    ("icons.marker_halo", "a dark halo under a marker"),
];

/// Every token field, by path (`sketch`, `analysis.draft`, `logo[3]`…): the arrays of distinct
/// meanings by element where only some elements are measured.
fn token_fields(t: &Tokens) -> BTreeSet<String> {
    let text = format!("{t:#?}");
    let mut out = BTreeSet::new();
    let mut parent: Option<String> = None;
    for line in text.lines() {
        let indent = line.len() - line.trim_start().len();
        let Some((name, rest)) = line.trim_start().split_once(": ") else { continue };
        match indent {
            4 if rest.ends_with('{') => parent = Some(name.to_string()),
            4 => {
                parent = None;
                out.insert(name.to_string());
            }
            8 => {
                if let Some(p) = &parent {
                    out.insert(format!("{p}.{name}"));
                }
            }
            _ => {}
        }
    }
    out
}

/// A group member's token field (`triad[0]` is part of `triad`).
fn field_of(name: &str) -> &str {
    name.split('[').next().unwrap_or(name)
}

#[test]
fn every_token_is_measured_or_carries_no_meaning() {
    for t in [Tokens::light(), Tokens::dark()] {
        let measured: BTreeSet<&str> =
            groups(&t).iter().flat_map(|g| g.members.iter().chain(&g.grounds).map(|(n, _)| field_of(n))).chain(SHADED).collect();
        // An element of an array (`logo[3]`) is listed for the record; its array is measured.
        let listed: BTreeSet<&str> = NO_MEANING.iter().map(|(n, _)| field_of(n)).collect();
        let fields = token_fields(&t);
        let unplaced: Vec<&String> = fields.iter().filter(|f| !measured.contains(f.as_str()) && !listed.contains(f.as_str())).collect();
        assert!(unplaced.is_empty(), "tokens neither measured (`groups`) nor listed in `NO_MEANING`: {unplaced:?}");
        let stale: Vec<&&str> = measured.iter().chain(&listed).filter(|f| !fields.contains(**f)).collect();
        assert!(stale.is_empty(), "names that are not token fields: {stale:?}");
        let both: Vec<&str> = NO_MEANING.iter().map(|(n, _)| *n).filter(|n| !n.contains('[') && measured.contains(n)).collect();
        assert!(both.is_empty(), "measured and also listed as carrying no meaning: {both:?}");
    }
}

/// Every pair in the groups: (group, name a, a, name b, b).
fn pairs(t: &Tokens) -> Vec<(String, Rgb8, Rgb8)> {
    let mut out = Vec::new();
    for g in groups(t) {
        for (i, (na, a)) in g.members.iter().enumerate() {
            for (nb, b) in g.members.iter().skip(i + 1).chain(&g.grounds) {
                out.push((format!("{}: {na} vs {nb}", g.name), *a, *b));
            }
        }
    }
    out
}

/// Pairs under the floor that the tables leave as they are (decided for #778). Each is close to
/// the floor for normal vision too, and construction and centerline lines also differ by their
/// dash patterns.
fn left_as_they_are(mode: ColorVision, dark: bool) -> BTreeSet<String> {
    let mut v = vec!["sketch lines: sketch_construction vs sketch_centerline"];
    if mode == ColorVision::Tritan {
        v.push("sketch lines: sketch_lost vs sketch_centerline");
        v.push(if dark { "timeline: hover vs accent_soft" } else { "timeline: timeline_item vs accent_soft" });
        v.push("selection: sel_face vs preview_add");
    }
    v.into_iter().map(String::from).collect()
}

// ---- the tests ----

/// Sharma, Wu, Dalal, "The CIEDE2000 Color-Difference Formula: Implementation Notes,
/// Supplementary Test Data, and Mathematical Observations", Color Research and Application 30
/// (2005), Table 1: L*, a*, b* of each pair, then the difference.
const SHARMA: [[f64; 7]; 34] = [
    [50.0000, 2.6772, -79.7751, 50.0000, 0.0000, -82.7485, 2.0425],
    [50.0000, 3.1571, -77.2803, 50.0000, 0.0000, -82.7485, 2.8615],
    [50.0000, 2.8361, -74.0200, 50.0000, 0.0000, -82.7485, 3.4412],
    [50.0000, -1.3802, -84.2814, 50.0000, 0.0000, -82.7485, 1.0000],
    [50.0000, -1.1848, -84.8006, 50.0000, 0.0000, -82.7485, 1.0000],
    [50.0000, -0.9009, -85.5211, 50.0000, 0.0000, -82.7485, 1.0000],
    [50.0000, 0.0000, 0.0000, 50.0000, -1.0000, 2.0000, 2.3669],
    [50.0000, -1.0000, 2.0000, 50.0000, 0.0000, 0.0000, 2.3669],
    [50.0000, 2.4900, -0.0010, 50.0000, -2.4900, 0.0009, 7.1792],
    [50.0000, 2.4900, -0.0010, 50.0000, -2.4900, 0.0010, 7.1792],
    [50.0000, 2.4900, -0.0010, 50.0000, -2.4900, 0.0011, 7.2195],
    [50.0000, 2.4900, -0.0010, 50.0000, -2.4900, 0.0012, 7.2195],
    [50.0000, -0.0010, 2.4900, 50.0000, 0.0009, -2.4900, 4.8045],
    [50.0000, -0.0010, 2.4900, 50.0000, 0.0010, -2.4900, 4.8045],
    [50.0000, -0.0010, 2.4900, 50.0000, 0.0011, -2.4900, 4.7461],
    [50.0000, 2.5000, 0.0000, 50.0000, 0.0000, -2.5000, 4.3065],
    [50.0000, 2.5000, 0.0000, 73.0000, 25.0000, -18.0000, 27.1492],
    [50.0000, 2.5000, 0.0000, 61.0000, -5.0000, 29.0000, 22.8977],
    [50.0000, 2.5000, 0.0000, 56.0000, -27.0000, -3.0000, 31.9030],
    [50.0000, 2.5000, 0.0000, 58.0000, 24.0000, 15.0000, 19.4535],
    [50.0000, 2.5000, 0.0000, 50.0000, 3.1736, 0.5854, 1.0000],
    [50.0000, 2.5000, 0.0000, 50.0000, 3.2972, 0.0000, 1.0000],
    [50.0000, 2.5000, 0.0000, 50.0000, 1.8634, 0.5757, 1.0000],
    [50.0000, 2.5000, 0.0000, 50.0000, 3.2592, 0.3350, 1.0000],
    [60.2574, -34.0099, 36.2677, 60.4626, -34.1751, 39.4387, 1.2644],
    [63.0109, -31.0961, -5.8663, 62.8187, -29.7946, -4.0864, 1.2630],
    [61.2901, 3.7196, -5.3901, 61.4292, 2.2480, -4.9620, 1.8731],
    [35.0831, -44.1164, 3.7933, 35.0232, -40.0716, 1.5901, 1.8645],
    [22.7233, 20.0904, -46.6940, 23.0331, 14.9730, -42.5619, 2.0373],
    [36.4612, 47.8580, 18.3852, 36.2715, 50.5065, 21.2231, 1.4146],
    [90.8027, -2.0831, 1.4410, 91.1528, -1.6435, 0.0447, 1.4441],
    [90.9257, -0.5406, -0.9208, 88.6381, -0.8985, -0.7239, 1.5381],
    [6.7747, -0.2908, -2.4247, 5.8714, -0.0985, -2.2286, 0.6377],
    [2.0776, 0.0795, -1.1350, 0.9033, -0.0636, -0.5514, 0.9082],
];

#[test]
fn ciede2000_reproduces_sharmas_test_pairs() {
    for (i, p) in SHARMA.iter().enumerate() {
        let got = de2000([p[0], p[1], p[2]], [p[3], p[4], p[5]]);
        assert!((got - p[6]).abs() < 1e-4, "pair {}: {got}, published {}", i + 1, p[6]);
        // The formula is symmetric.
        assert!((de2000([p[3], p[4], p[5]], [p[0], p[1], p[2]]) - got).abs() < 1e-9, "pair {}", i + 1);
    }
}

#[test]
fn the_simulations_keep_white_and_brettels_anchors() {
    let lms = mat(&LMS_FROM_XYZ, &XYZ_FROM_LIN);
    for e in [Eye::Protan, Eye::Deutan, Eye::Tritan] {
        let w = brettel(e, [1.0; 3]);
        assert!(w.iter().all(|v| (v - 1.0).abs() < 1e-9), "{e:?}: white -> {w:?}");
        let (_, a1, a2) = anchors(e);
        for a in [a1, a2] {
            let lin = mul(&inv(&lms), mul(&LMS_FROM_XYZ, a));
            let back = brettel(e, lin);
            assert!(lin.iter().zip(back).all(|(x, y)| (x - y).abs() < 1e-9), "{e:?}: anchor {lin:?} -> {back:?}");
        }
        let m = machado(e);
        assert!(m.iter().all(|row| (row.iter().sum::<f64>() - 1.0).abs() < 1e-5), "{e:?}: Machado keeps white");
    }
    // Known losses: pure red and green merge for a red-green loss, blue and green for a blue one.
    let (red, green, blue) = ([220, 60, 60], [60, 170, 60], [60, 100, 230]);
    assert!(normal(red, green) > 60.0);
    for how in [Instrument::Machado, Instrument::Brettel] {
        assert!(diff(red, green, Eye::Deutan, how) < 10.0, "{how:?}");
        assert!(diff(blue, green, Eye::Tritan, how) < diff(blue, green, Eye::Normal, how), "{how:?}");
    }
}

#[test]
fn off_gives_the_theme_unchanged() {
    assert_eq!(Tokens::themed(false, ColorVision::Off), Tokens::light());
    assert_eq!(Tokens::themed(true, ColorVision::Off), Tokens::dark());
    for dark in [false, true] {
        assert!(!Tokens::themed(dark, ColorVision::Off).analysis.hatch);
        // Protan and Deutan share the red-green table.
        assert_eq!(Tokens::themed(dark, ColorVision::Protan), Tokens::themed(dark, ColorVision::Deutan));
    }
}

/// The token fields whose `Debug` text differs (`triad`, `analysis.draft`…), so a field cannot
/// change in a mode by accident.
fn changed_fields(a: &Tokens, b: &Tokens) -> BTreeSet<String> {
    let (la, lb) = (format!("{a:#?}"), format!("{b:#?}"));
    let (la, lb): (Vec<&str>, Vec<&str>) = (la.lines().collect(), lb.lines().collect());
    assert_eq!(la.len(), lb.len());
    let mut out = BTreeSet::new();
    for i in (0..la.len()).filter(|i| la[*i] != lb[*i]) {
        // The named fields above this line, innermost first, up to a field of `Tokens`.
        let mut path = Vec::new();
        let mut indent = usize::MAX;
        for line in la[..=i].iter().rev() {
            let here = line.len() - line.trim_start().len();
            if here < indent
                && let Some((name, _)) = line.trim_start().split_once(':')
                && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
            {
                path.push(name.to_string());
                indent = here;
            }
            if indent <= 4 {
                break;
            }
        }
        path.reverse();
        out.insert(path.join("."));
    }
    out
}

#[test]
fn each_mode_changes_only_its_table() {
    let set = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<BTreeSet<_>>();
    let red_green_both = [
        "triad",
        "gizmo",
        "analysis.draft",
        "analysis.access",
        "analysis.hatch",
        "icons.axes",
        "icons.access",
        "icons.draft",
        "bend_up",
        "bend_down",
        "sketch_accent",
        "preview_sketch",
    ];
    let mut red_green_light = red_green_both.to_vec();
    red_green_light.extend(["sketch_projected", "sketch_locked", "timeline_item_error", "construction_geom"]);
    for (dark, mode, want) in [
        (false, ColorVision::Protan, set(&red_green_light)),
        (true, ColorVision::Protan, set(&red_green_both)),
        (false, ColorVision::Deutan, set(&red_green_light)),
        (true, ColorVision::Deutan, set(&red_green_both)),
        (false, ColorVision::Tritan, set(&["sketch_locked", "box_crossing", "construction_geom", "preview_sketch", "analysis.hatch"])),
        (true, ColorVision::Tritan, set(&["sketch_locked", "box_crossing", "construction_geom", "glyph_text", "preview_sketch", "analysis.hatch"])),
    ] {
        let base = Tokens::themed(dark, ColorVision::Off);
        assert_eq!(changed_fields(&base, &Tokens::themed(dark, mode)), want, "{mode:?}, dark {dark}");
    }
}

#[test]
fn every_mode_keeps_colours_apart_for_its_eye() {
    let mut report = Vec::new();
    for dark in [false, true] {
        let off = Tokens::themed(dark, ColorVision::Off);
        for mode in MODES {
            let on = Tokens::themed(dark, mode);
            let mut left = BTreeSet::new();
            for ((name, a0, b0), (_, a1, b1)) in pairs(&off).into_iter().zip(pairs(&on)) {
                // Close for everybody by design (hover tints, a shade of one hue), and not changed
                // by the mode: not a colour vision fault.
                if normal(a0, b0) < FLOOR && (a0, b0) == (a1, b1) {
                    continue;
                }
                let w = worst(mode, a1, b1);
                if w < FLOOR {
                    report.push(format!("{mode:?} dark={dark} {name}: {w:.1} (before {:.1})", worst(mode, a0, b0)));
                    left.insert(name);
                }
            }
            assert_eq!(left, left_as_they_are(mode, dark), "{mode:?}, dark {dark}: {report:#?}");
            // Draft and Accessibility, for faces at any angle to the light against each other.
            let a = &on.analysis;
            let shaded = [(a.draft[0], a.draft[1]), (a.draft[0], a.draft[2]), (a.draft[1], a.draft[2]), (a.access[0], a.access[1])];
            for (p, q) in shaded {
                for x in LIT {
                    for y in LIT {
                        let w = worst(mode, lit(p, x), lit(q, y));
                        assert!(w >= FLOOR, "{mode:?}, dark {dark}: {p:?} lit {x} vs {q:?} lit {y}: {w:.1}");
                    }
                }
            }
        }
    }
}

#[test]
fn the_shader_gets_the_modes_colours_and_its_stripes() {
    for dark in [false, true] {
        let off = crate::gpu::analysis_colors(&Tokens::themed(dark, ColorVision::Off).analysis);
        assert_eq!(off[32], 0.0, "no stripes in Off");
        for mode in MODES {
            let t = Tokens::themed(dark, mode);
            let v = crate::gpu::analysis_colors(&t.analysis);
            assert_eq!(v[32], 1.0, "{mode:?}: stripes on the bad faces");
            assert_eq!(v[..3], t.analysis.draft[0], "{mode:?}");
            assert_eq!(v[16..19], t.analysis.access[1], "{mode:?}");
        }
    }
}

// ---- the preference ----

/// Every message the Colour vision section shows (the heading, the mode names and their
/// tooltips, the sample's names and the note) is in every catalogue, translated: in Japanese,
/// Korean, Russian and Chinese none is the English text. (German keeps "Protan", "Deutan" and
/// "Tritan".)
#[test]
fn the_sections_messages_are_in_every_catalog() {
    let load = |text: &str| -> serde_json::Map<String, serde_json::Value> { serde_json::from_str(text).unwrap() };
    let english = load(include_str!("../../../assets/locales/en.json"));
    let ids: Vec<&str> = ColorVision::ALL
        .into_iter()
        .flat_map(|m| [m.label_id(), m.help_id()])
        .chain(crate::prefs::COLOUR_VISION_SAMPLE)
        .chain(["ui.colour_vision", "ui.colour_vision_mode", "ui.colour_vision_sample", "ui.colour_vision_note"])
        .collect();
    for (lang, text) in [
        ("cs", include_str!("../../../assets/locales/cs.json")),
        ("de", include_str!("../../../assets/locales/de.json")),
        ("ja", include_str!("../../../assets/locales/ja.json")),
        ("ko", include_str!("../../../assets/locales/ko.json")),
        ("pl", include_str!("../../../assets/locales/pl.json")),
        ("ru", include_str!("../../../assets/locales/ru.json")),
        ("zh", include_str!("../../../assets/locales/zh.json")),
    ] {
        let catalog = load(text);
        for id in &ids {
            let (Some(en), Some(here)) = (english.get(*id), catalog.get(*id)) else {
                panic!("{lang}: missing message {id} (English: {})", english.contains_key(*id));
            };
            assert!(lang == "de" || here != en, "{lang}: {id} is the English text {en}");
        }
    }
}

fn app() -> crate::SolveApp {
    crate::SolveApp::new(solidcraft_engine::Session::default(), crate::Services::default())
}

#[test]
fn saved_preferences_load_the_mode_and_a_bad_one_loads_as_off() {
    use crate::prefs::Prefs;
    // Saved before the mode existed.
    let old: Prefs = serde_json::from_value(json!({"theme": "light"})).unwrap();
    assert_eq!((old.color_vision, old.theme.as_str()), (ColorVision::Off, "light"));
    for mode in ColorVision::ALL {
        let p = Prefs { color_vision: mode, ..Prefs::default() };
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["color_vision"], mode.id());
        assert_eq!(serde_json::from_value::<Prefs>(v).unwrap(), p);
    }
    // A value this version doesn't know (a later SolidCraft's mode, a hand edit) is Off, and the
    // rest of the preferences still load.
    for bad in [json!("achromat"), json!("PROTAN"), json!(""), json!(3), json!(null), json!(true), json!({"protan": 1}), json!(["tritan"])] {
        let p: Prefs = serde_json::from_value(json!({"theme": "light", "color_vision": bad})).unwrap();
        assert_eq!((p.color_vision, p.theme.as_str()), (ColorVision::Off, "light"), "{bad}");
    }
}

#[test]
fn ui_prefs_sets_each_mode_and_refuses_anything_else() {
    let mut app = app();
    for mode in ColorVision::ALL {
        let out = crate::prefs::set(&mut app, &json!({"color_vision": mode.id()})).unwrap();
        assert_eq!(out["color_vision"], mode.id());
        assert_eq!(app.preferences.color_vision, mode);
    }
    let long = "p".repeat(1 << 20);
    for bad in [
        json!("purple"),
        json!("Protan"),
        json!(" protan"),
        json!(""),
        json!(long),
        json!(2),
        json!(-1.5),
        json!(null),
        json!(false),
        json!([]),
        json!({}),
    ] {
        let before = app.preferences.clone();
        let err = crate::prefs::set(&mut app, &json!({"color_vision": bad, "theme": "light"})).unwrap_err();
        assert!(err.contains("off, protan, deutan or tritan"), "{err}");
        assert_eq!(app.preferences, before, "a refused value changes nothing");
    }
}

/// Random values (fixed seed): strings near the four ids and far from them, numbers, nesting.
/// `ui.prefs` takes exactly the four ids and refuses the rest without a change; saved
/// preferences load every value, unknown ones as Off. Nothing panics.
#[test]
fn random_values_never_panic_and_only_the_four_ids_are_taken() {
    fn value(next: &mut impl FnMut() -> u64, depth: u32) -> serde_json::Value {
        let ids = ["off", "protan", "deutan", "tritan"];
        match next() % if depth > 3 { 6 } else { 8 } {
            0 => json!(ids[(next() % 4) as usize]),
            1 => {
                // A near miss: one id with a character changed, added or taken away.
                let mut s: Vec<char> = ids[(next() % 4) as usize].chars().collect();
                let i = (next() % s.len() as u64) as usize;
                match next() % 3 {
                    0 => s[i] = char::from_u32((next() % 0x2FFF) as u32).unwrap_or('?'),
                    1 => s.insert(i, ['\0', ' ', 'É', '\u{301}', 'ـ'][(next() % 5) as usize]),
                    _ => {
                        s.remove(i);
                    }
                }
                json!(s.into_iter().collect::<String>())
            }
            2 => json!((0..next() % 40).map(|_| char::from_u32((next() % 0x10FFFF) as u32).unwrap_or('x')).collect::<String>()),
            3 => json!(next() as i64),
            4 => json!(Some(f64::from_bits(next())).filter(|x| x.is_finite()).unwrap_or(0.5)),
            5 => [json!(null), json!(true), json!(false)][(next() % 3) as usize].clone(),
            6 => json!((0..next() % 4).map(|_| value(next, depth + 1)).collect::<Vec<_>>()),
            _ => json!({"color_vision": value(next, depth + 1)}),
        }
    }
    let mut seed = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let mut app = app();
    let mut taken = 0;
    for _ in 0..4000 {
        let v = value(&mut next, 0);
        let id = v.as_str().and_then(ColorVision::from_id);
        let before = app.preferences.clone();
        match crate::prefs::set(&mut app, &json!({"color_vision": v})) {
            Ok(_) => {
                taken += 1;
                assert_eq!(Some(app.preferences.color_vision), id, "{v}");
            }
            Err(e) => {
                assert!(id.is_none() && e.contains("off, protan, deutan or tritan"), "{v}: {e}");
                assert_eq!(app.preferences, before, "{v}");
            }
        }
        let saved: crate::prefs::Prefs = serde_json::from_value(json!({"theme": "light", "color_vision": v})).unwrap();
        assert_eq!((saved.color_vision, saved.theme.as_str()), (id.unwrap_or_default(), "light"), "{v}");
    }
    assert!(taken > 200, "the four ids came up {taken} times");
}

#[test]
fn cancel_puts_the_mode_back_ok_keeps_it_and_restore_defaults_resets_graphics_only() {
    let mut app = app();
    let control = |app: &mut crate::SolveApp, p: serde_json::Value| crate::prefs::control(app, &p).unwrap();
    control(&mut app, json!({"open": "Graphics"}));
    control(&mut app, json!({"color_vision": "protan"}));
    assert_eq!(app.preferences.color_vision, ColorVision::Protan);
    control(&mut app, json!({"button": "cancel"}));
    assert_eq!(app.preferences.color_vision, ColorVision::Off);
    control(&mut app, json!({"open": "Graphics"}));
    control(&mut app, json!({"color_vision": "tritan", "button": "ok"}));
    assert_eq!(app.preferences.color_vision, ColorVision::Tritan);
    // Restore Defaults on another page leaves it.
    control(&mut app, json!({"open": "General"}));
    control(&mut app, json!({"button": "defaults"}));
    assert_eq!(app.preferences.color_vision, ColorVision::Tritan);
    control(&mut app, json!({"open": "Graphics"}));
    control(&mut app, json!({"button": "defaults"}));
    assert_eq!(app.preferences.color_vision, ColorVision::Off);
    control(&mut app, json!({"button": "ok"}));
}
