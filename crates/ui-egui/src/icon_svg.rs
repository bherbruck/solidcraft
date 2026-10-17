//! The UI icon renderer: the SVG masters in `assets/icons/ui/design/svg`, drawn at the pixel size
//! they are shown.
//!
//! The masters are a small, linted SVG subset (`tools/lint_icons.py`): absolute `M L C Q A Z`
//! paths, circles, groups with a clip or opacity, masks and clip paths. They are embedded at build
//! time together with `palette.toml` (every colour role in light and dark, and the accents) and
//! `aliases.toml` (see `build.rs`), parsed once, and rasterised with tiny-skia into a texture per
//! (icon, accent, theme, pixel size). A master never carries a colour of its own: it names a
//! palette role, which resolves to the theme's hex, or to the accent's hex for the `active-*`
//! roles. GAP instead erases preceding artwork to transparency; it never paints a background.
//! Changing a colour is an edit to `palette.toml` only.
//!
//! Nothing here panics on bad artwork: a master that does not parse is logged and drawn as the
//! placeholder, and `every_master_parses` holds that none do.

use std::collections::HashMap;
use std::sync::OnceLock;

use tiny_skia as sk;

include!(concat!(env!("OUT_DIR"), "/icon_data.rs"));

/// The id drawn for every icon that has no master yet.
pub const PLACEHOLDER: &str = "placeholder";

/// How a shape is painted.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Paint {
    None,
    /// Erase earlier artwork; never paint a theme background into the texture.
    Knockout,
    /// `currentColor`: the `line` role.
    Line,
    /// A palette role (index into `ROLE_NAMES`).
    Role(usize),
    /// A fixed colour (mask artwork).
    Fixed([u8; 3]),
}

#[derive(Debug)]
struct Node {
    path: sk::Path,
    fill: Paint,
    rule: sk::FillRule,
    stroke: Paint,
    width: f32,
    dash: Vec<f32>,
    opacity: f32,
    mask: Option<usize>,
}

#[derive(Debug)]
enum Item {
    Node(Node),
    Group { clip: Option<usize>, opacity: f32, items: Vec<Item> },
}

/// A parsed master.
#[derive(Debug)]
pub struct Doc {
    items: Vec<Item>,
    masks: Vec<Vec<Node>>,
    clips: Vec<Vec<Node>>,
}

// ---------------------------------------------------------------------------------------------
// Parsing

struct Tag<'a> {
    name: &'a str,
    attrs: Vec<(&'a str, &'a str)>,
    close: bool,
    selfclose: bool,
}

impl Tag<'_> {
    fn get(&self, k: &str) -> Option<&str> {
        self.attrs.iter().find(|(n, _)| *n == k).map(|(_, v)| *v)
    }
    fn num(&self, k: &str) -> Result<f32, String> {
        self.get(k).ok_or_else(|| format!("missing {k}"))?.trim().parse::<f32>().map_err(|e| format!("{k}: {e}"))
    }
}

fn tags(src: &str) -> Result<Vec<Tag<'_>>, String> {
    let mut out = Vec::new();
    let mut rest = src;
    while let Some(i) = rest.find('<') {
        let after = rest.get(i + 1..).ok_or("truncated")?;
        let j = after.find('>').ok_or("unterminated tag")?;
        let body = after.get(..j).ok_or("truncated")?;
        rest = after.get(j + 1..).ok_or("truncated")?;
        if body.starts_with('?') || body.starts_with('!') {
            continue;
        }
        let (close, body) = match body.strip_prefix('/') {
            Some(b) => (true, b),
            None => (false, body),
        };
        let (selfclose, body) = match body.strip_suffix('/') {
            Some(b) => (true, b),
            None => (false, body),
        };
        let body = body.trim();
        let name_end = body.find(char::is_whitespace).unwrap_or(body.len());
        let name = body.get(..name_end).ok_or("bad tag")?;
        let mut attrs = Vec::new();
        let mut a = body.get(name_end..).unwrap_or("");
        while let Some(eq) = a.find('=') {
            let key = a.get(..eq).ok_or("bad attribute")?.trim();
            let v = a.get(eq + 1..).ok_or("bad attribute")?.trim_start();
            let q = v.chars().next().filter(|c| *c == '"' || *c == '\'').ok_or("unquoted attribute")?;
            let v = v.get(1..).ok_or("bad attribute")?;
            let end = v.find(q).ok_or("unterminated attribute")?;
            attrs.push((key, v.get(..end).ok_or("bad attribute")?));
            a = v.get(end + 1..).unwrap_or("");
        }
        out.push(Tag { name, attrs, close, selfclose });
    }
    Ok(out)
}

/// An SVG elliptical arc as cubic Béziers appended to `pb`.
#[allow(clippy::too_many_arguments)]
fn arc(pb: &mut sk::PathBuilder, x1: f32, y1: f32, rx: f32, ry: f32, rot_deg: f32, large: bool, sweep: bool, x2: f32, y2: f32) {
    let (mut rx, mut ry) = (rx.abs(), ry.abs());
    if rx < 1e-6 || ry < 1e-6 || ((x1 - x2).abs() < 1e-6 && (y1 - y2).abs() < 1e-6) {
        pb.line_to(x2, y2);
        return;
    }
    let phi = rot_deg.to_radians();
    let (sp, cp) = phi.sin_cos();
    let (dx, dy) = ((x1 - x2) / 2.0, (y1 - y2) / 2.0);
    let (x1p, y1p) = (cp * dx + sp * dy, -sp * dx + cp * dy);
    let lam = (x1p * x1p) / (rx * rx) + (y1p * y1p) / (ry * ry);
    if lam > 1.0 {
        let s = lam.sqrt();
        rx *= s;
        ry *= s;
    }
    let num = rx * rx * ry * ry - rx * rx * y1p * y1p - ry * ry * x1p * x1p;
    let den = rx * rx * y1p * y1p + ry * ry * x1p * x1p;
    let sign = if large == sweep { -1.0 } else { 1.0 };
    let coef = sign * (num / den).max(0.0).sqrt();
    let (cxp, cyp) = (coef * rx * y1p / ry, -coef * ry * x1p / rx);
    let (cx, cy) = (cp * cxp - sp * cyp + (x1 + x2) / 2.0, sp * cxp + cp * cyp + (y1 + y2) / 2.0);
    let ang = |ux: f32, uy: f32, vx: f32, vy: f32| (ux * vy - uy * vx).atan2(ux * vx + uy * vy);
    let (ux, uy) = ((x1p - cxp) / rx, (y1p - cyp) / ry);
    let (vx, vy) = ((-x1p - cxp) / rx, (-y1p - cyp) / ry);
    let theta = ang(1.0, 0.0, ux, uy);
    let mut dtheta = ang(ux, uy, vx, vy);
    if !sweep && dtheta > 0.0 {
        dtheta -= std::f32::consts::TAU;
    } else if sweep && dtheta < 0.0 {
        dtheta += std::f32::consts::TAU;
    }
    let n = ((dtheta.abs() / std::f32::consts::FRAC_PI_2).ceil() as usize).clamp(1, 8);
    let delta = dtheta / n as f32;
    let t = 4.0 / 3.0 * (delta / 4.0).tan();
    let map = |x: f32, y: f32| (cx + rx * cp * x - ry * sp * y, cy + rx * sp * x + ry * cp * y);
    for i in 0..n {
        let a1 = theta + delta * i as f32;
        let a2 = a1 + delta;
        let (s1, c1) = a1.sin_cos();
        let (s2, c2) = a2.sin_cos();
        let (p1x, p1y) = map(c1 - t * s1, s1 + t * c1);
        let (p2x, p2y) = map(c2 + t * s2, s2 - t * c2);
        let (ex, ey) = map(c2, s2);
        pb.cubic_to(p1x, p1y, p2x, p2y, ex, ey);
    }
}

fn parse_path(d: &str) -> Result<sk::Path, String> {
    let mut toks: Vec<&str> = Vec::new();
    let b = d.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let c = *b.get(i).ok_or("path")?;
        if c.is_ascii_alphabetic() {
            toks.push(d.get(i..=i).ok_or("path")?);
            i += 1;
        } else if c == b'-' || c == b'.' || c.is_ascii_digit() {
            let s = i;
            i += 1;
            while let Some(&n) = b.get(i) {
                if n.is_ascii_digit()
                    || n == b'.'
                    || ((n == b'-' || n == b'+') && matches!(b.get(i - 1), Some(b'e' | b'E')))
                    || n == b'e'
                    || n == b'E'
                {
                    i += 1;
                } else {
                    break;
                }
            }
            toks.push(d.get(s..i).ok_or("path")?);
        } else {
            i += 1;
        }
    }
    let mut pb = sk::PathBuilder::new();
    let (mut k, mut cmd) = (0usize, ' ');
    let (mut x, mut y) = (0.0f32, 0.0f32);
    let mut open = false;
    let num = |k: &mut usize| -> Result<f32, String> {
        let t = toks.get(*k).ok_or("path: missing number")?;
        *k += 1;
        t.parse::<f32>().map_err(|e| format!("path: {e}"))
    };
    while k < toks.len() {
        let t = *toks.get(k).ok_or("path")?;
        if t.len() == 1 && t.as_bytes().first().is_some_and(u8::is_ascii_alphabetic) {
            cmd = t.chars().next().ok_or("path")?;
            k += 1;
        }
        match cmd {
            'M' => {
                (x, y) = (num(&mut k)?, num(&mut k)?);
                pb.move_to(x, y);
                open = true;
                cmd = 'L';
            }
            'L' => {
                (x, y) = (num(&mut k)?, num(&mut k)?);
                pb.line_to(x, y);
            }
            'C' => {
                let v = [num(&mut k)?, num(&mut k)?, num(&mut k)?, num(&mut k)?, num(&mut k)?, num(&mut k)?];
                pb.cubic_to(v[0], v[1], v[2], v[3], v[4], v[5]);
                (x, y) = (v[4], v[5]);
            }
            'Q' => {
                let v = [num(&mut k)?, num(&mut k)?, num(&mut k)?, num(&mut k)?];
                pb.quad_to(v[0], v[1], v[2], v[3]);
                (x, y) = (v[2], v[3]);
            }
            'A' => {
                let (rx, ry, rot) = (num(&mut k)?, num(&mut k)?, num(&mut k)?);
                let (fa, fs) = (num(&mut k)? != 0.0, num(&mut k)? != 0.0);
                let (ex, ey) = (num(&mut k)?, num(&mut k)?);
                arc(&mut pb, x, y, rx, ry, rot, fa, fs, ex, ey);
                (x, y) = (ex, ey);
            }
            'Z' => {
                if open {
                    pb.close();
                }
            }
            other => return Err(format!("path command {other} is not supported")),
        }
    }
    pb.finish().ok_or_else(|| "empty path".to_string())
}

fn parse_paint(v: Option<&str>, in_mask: bool) -> Result<Paint, String> {
    let Some(v) = v else { return Ok(Paint::None) };
    match v {
        "none" => Ok(Paint::None),
        "currentColor" => Ok(Paint::Line),
        "white" if in_mask => Ok(Paint::Fixed([255, 255, 255])),
        "black" if in_mask => Ok(Paint::Fixed([0, 0, 0])),
        _ => {
            let lower = v.to_ascii_lowercase();
            ROLE_SOURCE_HEX
                .iter()
                .position(|h| *h == lower)
                .map(|role| if ROLE_NAMES.get(role) == Some(&"gap") { Paint::Knockout } else { Paint::Role(role) })
                .ok_or_else(|| format!("colour {v} is not a palette colour"))
        }
    }
}

fn parse_dash(v: Option<&str>) -> Vec<f32> {
    let mut d: Vec<f32> = v.map(|s| s.split([' ', ',']).filter_map(|n| n.parse().ok()).collect()).unwrap_or_default();
    if d.len() % 2 == 1 {
        d.extend(d.clone());
    }
    d
}

fn url_ref(v: &str) -> Option<&str> {
    v.strip_prefix("url(#")?.strip_suffix(')')
}

enum Frame {
    Group { clip: Option<usize>, opacity: f32, items: Vec<Item> },
    Mask(usize),
    Clip(usize),
    Defs,
}

impl Doc {
    pub fn parse(src: &str) -> Result<Doc, String> {
        let mut doc = Doc { items: Vec::new(), masks: Vec::new(), clips: Vec::new() };
        let (mut mask_ids, mut clip_ids): (HashMap<String, usize>, HashMap<String, usize>) = (HashMap::new(), HashMap::new());
        let mut stack: Vec<Frame> = Vec::new();
        for t in tags(src)? {
            match (t.name, t.close) {
                ("svg" | "title", _) => {}
                ("defs", false) => stack.push(Frame::Defs),
                ("mask", false) => {
                    doc.masks.push(Vec::new());
                    mask_ids.insert(t.get("id").ok_or("mask without id")?.to_string(), doc.masks.len() - 1);
                    stack.push(Frame::Mask(doc.masks.len() - 1));
                }
                ("clipPath", false) => {
                    doc.clips.push(Vec::new());
                    clip_ids.insert(t.get("id").ok_or("clipPath without id")?.to_string(), doc.clips.len() - 1);
                    stack.push(Frame::Clip(doc.clips.len() - 1));
                }
                ("g", false) => {
                    let clip = match t.get("clip-path").and_then(url_ref) {
                        Some(id) => Some(*clip_ids.get(id).ok_or("unknown clip")?),
                        None => None,
                    };
                    let opacity = t.get("opacity").and_then(|o| o.parse().ok()).unwrap_or(1.0);
                    stack.push(Frame::Group { clip, opacity, items: Vec::new() });
                }
                ("defs" | "mask" | "clipPath" | "g", true) => match stack.pop() {
                    Some(Frame::Group { clip, opacity, items }) => {
                        let item = Item::Group { clip, opacity, items };
                        match stack.last_mut() {
                            Some(Frame::Group { items, .. }) => items.push(item),
                            _ => doc.items.push(item),
                        }
                    }
                    Some(_) => {}
                    None => return Err("unbalanced tags".into()),
                },
                ("path" | "circle", _) => {
                    let in_mask = matches!(stack.last(), Some(Frame::Mask(_)));
                    let path = if t.name == "path" {
                        parse_path(t.get("d").ok_or("path without d")?)?
                    } else {
                        sk::PathBuilder::from_circle(t.num("cx")?, t.num("cy")?, t.num("r")?).ok_or("bad circle")?
                    };
                    let mask = match t.get("mask").and_then(url_ref) {
                        Some(id) => Some(*mask_ids.get(id).ok_or("unknown mask")?),
                        None => None,
                    };
                    let dash = parse_dash(t.get("stroke-dasharray"));
                    let mut stroke = parse_paint(t.get("stroke"), in_mask)?;
                    let visible_stroke = match stroke {
                        Paint::Line => true,
                        Paint::Role(role) => !matches!(ROLE_NAMES.get(role).copied(), Some("gap" | "active-1" | "active-2" | "active-3")),
                        _ => false,
                    };
                    // Non-blue dotted/dashed references share the guide colour.
                    // Blue source paints retain their semantic accent; keep
                    // background knockouts and literal mask paints unchanged.
                    if !dash.is_empty() && visible_stroke && !in_mask {
                        stroke = Paint::Role(ROLE_NAMES.iter().position(|name| *name == "guide").ok_or("missing guide colour")?);
                    }
                    let node = Node {
                        path,
                        fill: if t.get("fill").is_none() && in_mask { Paint::None } else { parse_paint(t.get("fill").or(Some("none")), in_mask)? },
                        rule: if t.get("fill-rule") == Some("evenodd") { sk::FillRule::EvenOdd } else { sk::FillRule::Winding },
                        stroke,
                        width: t.get("stroke-width").and_then(|w| w.parse().ok()).unwrap_or(1.0),
                        dash,
                        opacity: t.get("opacity").and_then(|o| o.parse().ok()).unwrap_or(1.0),
                        mask,
                    };
                    match stack.last_mut() {
                        Some(Frame::Mask(i)) => doc.masks.get_mut(*i).ok_or("mask")?.push(node),
                        Some(Frame::Clip(i)) => doc.clips.get_mut(*i).ok_or("clip")?.push(node),
                        Some(Frame::Group { items, .. }) => items.push(Item::Node(node)),
                        _ => doc.items.push(Item::Node(node)),
                    }
                }
                (other, _) => return Err(format!("element <{other}> is not supported")),
            }
            let _ = t.selfclose;
        }
        if !stack.is_empty() {
            return Err("unbalanced tags".into());
        }
        Ok(doc)
    }
}

// ---------------------------------------------------------------------------------------------
// Rendering

/// The colours of one drawing: the theme and an accent (index into `ACCENT_NAMES`, or `None` for
/// the base palette).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Look {
    pub dark: bool,
    pub accent: Option<usize>,
    pub vision: crate::theme::ColorVision,
}

impl Look {
    fn rgb(self, role: usize) -> [u8; 3] {
        let theme = usize::from(self.dark);
        if ROLE_NAMES.get(role) == Some(&"guide") {
            return PALETTE.get(theme).and_then(|p| p.get(role)).copied().unwrap_or([255, 0, 255]);
        }
        let vision = match self.vision {
            crate::theme::ColorVision::Off => "",
            crate::theme::ColorVision::Protan | crate::theme::ColorVision::Deutan => "red-green",
            crate::theme::ColorVision::Tritan => "blue-yellow",
        };
        if let Some((_, _, _, l, d)) = ACCENT_VARIANTS.iter().find(|(v, a, r, _, _)| *v == vision && Some(*a) == self.accent && *r == role) {
            return if self.dark { *d } else { *l };
        }
        if let Some((_, l, d)) = self.accent.and_then(|a| ACCENTS.get(a)).and_then(|items| items.iter().find(|(r, _, _)| *r == role)) {
            return if self.dark { *d } else { *l };
        }
        PALETTE.get(theme).and_then(|p| p.get(role)).copied().unwrap_or([255, 0, 255])
    }

    fn resolve(self, p: Paint) -> Option<[u8; 3]> {
        match p {
            Paint::None | Paint::Knockout => None,
            Paint::Fixed(c) => Some(c),
            Paint::Role(r) => Some(self.rgb(r)),
            Paint::Line => ROLE_NAMES.iter().position(|n| *n == "line").map(|r| self.rgb(r)),
        }
    }
}

fn paint_of(c: [u8; 3], alpha: f32) -> sk::Paint<'static> {
    let mut p = sk::Paint::default();
    p.set_color_rgba8(c[0], c[1], c[2], (alpha.clamp(0.0, 1.0) * 255.0).round() as u8);
    p.anti_alias = true;
    p
}

fn shape_paint(paint: Paint, look: Look, alpha: f32) -> Option<sk::Paint<'static>> {
    if paint == Paint::Knockout {
        let mut paint = paint_of([0, 0, 0], alpha);
        paint.blend_mode = sk::BlendMode::DestinationOut;
        Some(paint)
    } else {
        look.resolve(paint).map(|colour| paint_of(colour, alpha))
    }
}

fn draw_node(pm: &mut sk::Pixmap, n: &Node, look: Look, tf: sk::Transform, doc: &Doc, clip: Option<&sk::Mask>, opacity: f32) {
    let own;
    let mask = if let Some(m) = n.mask {
        own = doc.masks.get(m).and_then(|nodes| {
            let mut sub = sk::Pixmap::new(pm.width(), pm.height())?;
            for node in nodes {
                draw_node(&mut sub, node, look, tf, doc, None, 1.0);
            }
            let mut own = sk::Mask::from_pixmap(sub.as_ref(), sk::MaskType::Luminance);
            if let Some(parent) = clip {
                for (value, parent) in own.data_mut().iter_mut().zip(parent.data()) {
                    *value = ((u16::from(*value) * u16::from(*parent) + 127) / 255) as u8;
                }
            }
            Some(own)
        });
        own.as_ref()
    } else {
        clip
    };
    let alpha = n.opacity * opacity;
    if let Some(paint) = shape_paint(n.fill, look, alpha) {
        pm.fill_path(&n.path, &paint, n.rule, tf, mask);
    }
    if let Some(paint) = shape_paint(n.stroke, look, alpha) {
        let stroke = sk::Stroke {
            width: n.width,
            line_cap: sk::LineCap::Round,
            line_join: sk::LineJoin::Round,
            dash: if n.dash.is_empty() { None } else { sk::StrokeDash::new(n.dash.clone(), 0.0) },
            ..sk::Stroke::default()
        };
        pm.stroke_path(&n.path, &paint, &stroke, tf, mask);
    }
}

fn draw_items(pm: &mut sk::Pixmap, items: &[Item], look: Look, tf: sk::Transform, doc: &Doc, clip: Option<&sk::Mask>, opacity: f32) {
    for item in items {
        match item {
            Item::Node(n) => draw_node(pm, n, look, tf, doc, clip, opacity),
            Item::Group { clip: c, opacity: o, items } => {
                let mask = c.and_then(|c| doc.clips.get(c)).and_then(|nodes| {
                    let mut m = sk::Mask::new(pm.width(), pm.height())?;
                    for n in nodes {
                        m.fill_path(&n.path, n.rule, true, tf);
                    }
                    Some(m)
                });
                draw_items(pm, items, look, tf, doc, mask.as_ref().or(clip), opacity * o);
            }
        }
    }
}

/// Rasterise `doc` into a square `px` × `px` premultiplied RGBA image.
pub fn render(doc: &Doc, look: Look, px: u32) -> Option<sk::Pixmap> {
    let mut pm = sk::Pixmap::new(px, px)?;
    let s = px as f32 / 64.0;
    draw_items(&mut pm, &doc.items, look, sk::Transform::from_scale(s, s), doc, None, 1.0);
    Some(pm)
}

// ---------------------------------------------------------------------------------------------
// The icon table

struct Entry {
    doc: Doc,
}

fn table() -> &'static (HashMap<&'static str, Entry>, HashMap<&'static str, Entry>) {
    static T: OnceLock<(HashMap<&'static str, Entry>, HashMap<&'static str, Entry>)> = OnceLock::new();
    T.get_or_init(|| {
        let (mut big, mut small) = (HashMap::new(), HashMap::new());
        for (id, fam, src) in SVGS {
            match Doc::parse(src) {
                Ok(doc) => {
                    let map = if *fam == "small" { &mut small } else { &mut big };
                    // Model masters precede flat glyphs: keep the reviewed command artwork.
                    map.entry(*id).or_insert(Entry { doc });
                }
                Err(e) => log::warn!("icon {fam}/{id}: {e}"),
            }
        }
        (big, small)
    })
}

/// A resolved icon: the master to draw and the accent it is drawn in.
#[derive(Clone, Copy, Debug)]
pub struct Resolved {
    pub master: &'static str,
    pub accent: Option<usize>,
    pub placeholder: bool,
}

fn accent_index(name: &str) -> Option<usize> {
    if name.is_empty() { None } else { ACCENT_NAMES.iter().position(|a| *a == name) }
}

/// A semantic default belongs to the requested id, never to the caller's current workspace.
fn default_accent(name: &str) -> Option<usize> {
    ICON_ACCENTS.iter().find(|(id, _)| *id == name).and_then(|(_, accent)| accent_index(accent))
}

/// Primary semantic stroke, also used by toolbar affordances. The palette owns its value.
pub(crate) fn primary(name: &str, dark: bool, vision: crate::theme::ColorVision) -> egui::Color32 {
    let look = Look { dark, accent: resolve(name).accent, vision };
    role_colour("active-3", look)
}

pub(crate) fn role_colour(name: &str, look: Look) -> egui::Color32 {
    if name == "gap" {
        return egui::Color32::TRANSPARENT;
    }
    let role = ROLE_NAMES.iter().position(|n| *n == name);
    let [r, g, b] = role.map(|role| look.rgb(role)).unwrap_or([255, 0, 255]);
    crate::theme::icon_color([r, g, b])
}

/// The master and accent for an app icon id: an alias (with its accent), a master of that name,
/// or the placeholder.
pub fn resolve(name: &str) -> Resolved {
    let (big, _) = table();
    let placeholder = Resolved { master: PLACEHOLDER, accent: None, placeholder: true };
    if let Some((_, master, accent)) = ALIASES.iter().find(|(a, _, _)| *a == name) {
        return match big.get_key_value(master) {
            Some((k, _)) => Resolved { master: k, accent: accent_index(accent).or_else(|| default_accent(name)), placeholder: false },
            None => placeholder,
        };
    }
    match big.get_key_value(name) {
        Some((k, _)) if name != PLACEHOLDER => Resolved { master: k, accent: default_accent(name), placeholder: false },
        _ => placeholder,
    }
}

/// The master to draw at `px` pixels: the small-slot variant where one exists.
pub fn doc_for(master: &str, px: u32) -> Option<&'static Doc> {
    let (big, small) = table();
    if let Some(e) = small.get(master).filter(|_| px <= 24) {
        return Some(&e.doc);
    }
    big.get(master).or_else(|| big.get(PLACEHOLDER)).map(|e| &e.doc)
}

/// Every master id (not the small variants), for tests and tooling.
pub fn master_ids() -> Vec<&'static str> {
    let mut v: Vec<_> = table().0.keys().copied().collect();
    v.sort_unstable();
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_master_parses() {
        for (id, fam, src) in SVGS {
            if let Err(e) = Doc::parse(src) {
                panic!("{fam}/{id}: {e}");
            }
        }
        assert!(table().0.contains_key(PLACEHOLDER), "the placeholder master exists");
    }

    #[test]
    fn aliases_point_at_masters_and_known_accents() {
        for (alias, master, accent) in ALIASES {
            assert!(table().0.contains_key(master), "{alias} -> {master}");
            assert!(accent.is_empty() || ACCENT_NAMES.contains(accent), "{alias}: accent {accent}");
            assert!(!resolve(alias).placeholder);
        }
    }

    #[test]
    fn non_blue_dashed_strokes_use_guides_but_preserve_accents_and_knockouts() {
        let svg = "<svg><path d='M8 20L56 20' stroke='#8e9faf' stroke-width='4.25' stroke-dasharray='3 5'/><path d='M8 40L56 40' stroke='#ffffff' stroke-width='6' stroke-dasharray='3 5'/><path d='M8 52L56 52' stroke='#236ead' stroke-width='4.25' stroke-dasharray='3 5'/></svg>";
        let doc = Doc::parse(svg).unwrap();
        let guide = ROLE_NAMES.iter().position(|name| *name == "guide").unwrap();
        let active = ROLE_NAMES.iter().position(|name| *name == "active-3").unwrap();
        let Item::Node(line) = &doc.items[0] else { panic!("expected line") };
        let Item::Node(knockout) = &doc.items[1] else { panic!("expected knockout") };
        let Item::Node(blue) = &doc.items[2] else { panic!("expected blue line") };
        assert_eq!(line.stroke, Paint::Role(guide));
        assert_eq!(knockout.stroke, Paint::Knockout);
        assert_eq!(blue.stroke, Paint::Role(active));
        for dark in [false, true] {
            for accent in std::iter::once(None).chain((0..ACCENT_NAMES.len()).map(Some)) {
                let look = Look { dark, accent, vision: crate::theme::ColorVision::Off };
                assert_eq!(look.rgb(guide), PALETTE[usize::from(dark)][guide]);
                let pm = render(&doc, look, 64).unwrap();
                assert_eq!([pm.pixel(9, 20).unwrap().red(), pm.pixel(9, 20).unwrap().green(), pm.pixel(9, 20).unwrap().blue()], look.rgb(guide));
                assert_eq!([pm.pixel(9, 52).unwrap().red(), pm.pixel(9, 52).unwrap().green(), pm.pixel(9, 52).unwrap().blue()], look.rgb(active));
            }
        }
    }

    #[test]
    fn knockout_fills_and_strokes_erase_earlier_layers_and_allow_later_foreground() {
        let doc = Doc::parse("<svg><path d='M0 0L64 0L64 64L0 64Z' fill='#8e9faf'/><circle cx='20' cy='20' r='10' fill='#ffffff'/><circle cx='20' cy='20' r='3' fill='#236ead'/><path d='M8 44L56 44' stroke='#ffffff' stroke-width='6'/><circle cx='44' cy='20' r='6' fill='#ffffff' opacity='0.5'/></svg>").unwrap();
        for dark in [false, true] {
            for accent in std::iter::once(None).chain((0..ACCENT_NAMES.len()).map(Some)) {
                let look = Look { dark, accent, vision: crate::theme::ColorVision::Off };
                let pm = render(&doc, look, 64).unwrap();
                assert_eq!(pm.pixel(26, 20).unwrap().alpha(), 0, "fill must reveal the real background");
                assert_eq!(pm.pixel(20, 44).unwrap().alpha(), 0, "stroke must reveal the real background");
                assert_eq!(pm.pixel(20, 20).unwrap().alpha(), 255, "foreground is drawn after erasure");
                assert_eq!(pm.pixel(2, 2).unwrap().alpha(), 255, "unaffected base stays opaque");
                assert!((126..=128).contains(&pm.pixel(44, 20).unwrap().alpha()), "partial opacity erases partially");
            }
        }
    }

    #[test]
    fn knockout_respects_its_clip_and_combines_explicit_masks_with_clips() {
        let doc = Doc::parse("<svg><defs><clipPath id='left'><path d='M0 0L32 0L32 64L0 64Z'/></clipPath><mask id='all'><path d='M0 0L64 0L64 64L0 64Z' fill='white'/></mask></defs><path d='M0 0L64 0L64 64L0 64Z' fill='#8e9faf'/><g clip-path='url(#left)'><circle cx='32' cy='32' r='16' fill='#ffffff' mask='url(#all)'/></g></svg>").unwrap();
        let pm = render(&doc, Look { dark: false, accent: None, vision: crate::theme::ColorVision::Off }, 64).unwrap();
        assert_eq!(pm.pixel(24, 32).unwrap().alpha(), 0);
        assert_eq!(pm.pixel(40, 32).unwrap().alpha(), 255, "cutout must not escape its parent clip");
    }

    #[test]
    fn accepted_badges_and_environment_sun_have_transparent_holes() {
        for dark in [false, true] {
            let look = Look { dark, accent: None, vision: crate::theme::ColorVision::Off };
            assert_eq!(role_colour("gap", look), egui::Color32::TRANSPARENT);
            for (id, x, y) in [("contact_new", 48, 48), ("environment_map", 45, 19), ("line_mid", 35, 28)] {
                let pm = render(doc_for(id, 64).unwrap(), look, 64).unwrap();
                assert_eq!(pm.pixel(x, y).unwrap().alpha(), 0, "{id}: GAP must not be a painted theme colour");
            }
        }
    }

    #[test]
    fn native_clip_paths_and_plus_mask_keep_the_artwork_inside_its_shape() {
        for dark in [false, true] {
            for px in [16, 24, 64] {
                let look = Look { dark, accent: None, vision: crate::theme::ColorVision::Off };
                for id in ["environment_map", "curvature_map", "iso_analysis", "zebra"] {
                    let doc = doc_for(id, px).unwrap();
                    let pm = render(doc, look, px).unwrap();
                    assert_eq!(pm.pixel(px - 2, px - 2).unwrap().alpha(), 0, "{id} escapes its native clip");
                    assert!(pm.pixel(px / 2, px / 2).unwrap().alpha() > 0, "{id} lost its clipped contents");
                }
            }
            let pm = render(doc_for("new", 64).unwrap(), Look { dark, accent: None, vision: crate::theme::ColorVision::Off }, 64).unwrap();
            assert_eq!(pm.pixel(45, 45).unwrap().alpha(), 0, "the plus is genuinely punched out in the native renderer");
        }
    }

    #[test]
    fn command_meanings_choose_the_same_accent_on_every_surface() {
        let accent = |name| resolve(name).accent.map(|i| ACCENT_NAMES[i]);
        for c in solidcraft_engine::command_specs() {
            let want = if c.id.starts_with("surface.") {
                Some("surface")
            } else if c.panel == "CONSTRAINTS" {
                Some("constraint")
            } else if matches!(c.id, "sketch.construction" | "sketch.centerline") {
                Some("construction")
            } else if c.id.starts_with("sketch.") && matches!(c.panel, "CREATE" | "MODIFY" | "PATTERN" | "FINISH SKETCH") {
                Some("sketch")
            } else if c.id == "file.save_mesh" {
                Some("mesh")
            } else {
                continue;
            };
            assert_eq!(accent(c.icon), want, "{} ({})", c.id, c.icon);
        }
        for id in ["move", "scale", "copy", "delete", "export", "ground", "axis_two_planes"] {
            assert_eq!(accent(id), None, "shared utility / modelling icon {id}");
        }
        for (alias, master) in [("sketch_move", "move"), ("sketch_scale", "scale"), ("surface_fair", "spline_cv"), ("mesh_export", "export")] {
            assert_eq!(resolve(alias).master, resolve(master).master, "reuse accepted shapes");
        }
    }

    #[test]
    fn semantic_ui_tokens_and_icon_strokes_share_the_palette_including_accessibility() {
        for dark in [false, true] {
            for vision in crate::theme::ColorVision::ALL {
                let tokens = crate::theme::Tokens::themed(dark, vision);
                assert_eq!(primary("line", dark, vision), tokens.sketch_accent);
                assert_eq!(primary("c_parallel", dark, vision), tokens.glyph_text);
                assert_eq!(primary("centerline", dark, vision), tokens.construction_geom);
                assert_eq!(primary("construction", dark, vision), tokens.construction_geom);
            }
        }
    }

    #[test]
    fn semantic_ramps_preserve_silhouettes_and_passive_faces_in_all_themes() {
        use crate::theme::ColorVision;
        for id in ["line", "c_parallel", "construction", "centerline", "sketch_rect_pattern", "surface_extrude", "mesh_export"] {
            let resolved = resolve(id);
            for dark in [false, true] {
                for vision in ColorVision::ALL {
                    let look = Look { dark, accent: resolved.accent, vision };
                    let base = Look { accent: None, ..look };
                    for role in ["ink", "line", "passive-1", "passive-2", "passive-3", "gap"] {
                        let role = ROLE_NAMES.iter().position(|name| *name == role).unwrap();
                        assert_eq!(look.rgb(role), base.rgb(role), "{id} changes passive paint");
                    }
                    for px in [16, 24, 64] {
                        let doc = doc_for(resolved.master, px).unwrap();
                        let changed = render(doc, look, px).unwrap();
                        let original = render(doc, base, px).unwrap();
                        assert!(changed.pixels().iter().zip(original.pixels()).all(|(a, b)| a.alpha() == b.alpha()), "{id}: silhouette at {px}px");
                    }
                }
            }
        }
    }

    #[test]
    fn command_artwork_resolves_and_construct_methods_are_distinct() {
        let specs = solidcraft_engine::command_specs();
        for command in &specs {
            assert!(!resolve(command.icon).placeholder, "{}: {}", command.id, command.icon);
        }
        let construction: Vec<_> = specs.iter().filter(|c| c.panel == "CONSTRUCT").collect();
        let distinct: std::collections::HashSet<_> = construction.iter().map(|c| c.icon).collect();
        assert_eq!(construction.len(), 19);
        assert_eq!(distinct.len(), construction.len(), "construction methods need distinct visual identities");
    }

    #[test]
    fn helper_controls_resolve_without_command_registry_entries() {
        for name in ["agent_chat", "normal", "infinite", "pattern_flip", "pattern_center"] {
            assert!(!resolve(name).placeholder, "UI helper: {name}");
        }
    }

    #[test]
    fn sketch_construction_modes_have_distinct_icons() {
        let families: &[&[&str]] = &[
            &["sketch.polygon.inscribed", "sketch.polygon.circumscribed", "sketch.polygon.edge"],
            &["sketch.slot.center_to_center", "sketch.slot.overall", "sketch.slot.center_point"],
            &["sketch.slot.arc_three_point", "sketch.slot.arc_center"],
            &["sketch.chamfer.equal_distance", "sketch.chamfer.distance_angle", "sketch.chamfer.two_distance"],
        ];
        for commands in families {
            let icons: std::collections::HashSet<_> = commands.iter().map(|id| solidcraft_engine::find_command(id).unwrap().icon).collect();
            assert_eq!(icons.len(), commands.len(), "Sketch modes share an icon: {commands:?}");
            for icon in icons {
                assert!(!resolve(icon).placeholder, "{icon}");
            }
        }
    }

    #[test]
    fn contact_actions_have_distinct_icons_from_interference_analysis() {
        let mut icons = std::collections::HashSet::new();
        let interference = solidcraft_engine::find_command("inspect.interference").unwrap().icon;
        for id in ["contact.enable_sets", "contact.enable_all", "contact.disable_all", "contact.create"] {
            let icon = solidcraft_engine::find_command(id).unwrap().icon;
            assert_ne!(icon, interference, "{id} is a contact control, not an interference result");
            assert!(icons.insert(icon), "contact controls share an icon: {id}");
            assert!(!resolve(icon).placeholder, "{id}: {icon}");
        }
    }

    #[test]
    fn reviewed_model_folder_is_also_the_folder_drawn_in_the_app() {
        let source = SVGS.iter().find(|(id, fam, _)| *id == "folder" && *fam == "model").unwrap().2;
        let expected = Doc::parse(source).unwrap();
        for dark in [false, true] {
            let look = Look { dark, accent: None, vision: crate::theme::ColorVision::Off };
            let actual = render(doc_for("folder", 64).unwrap(), look, 64).unwrap();
            let reviewed = render(&expected, look, 64).unwrap();
            assert_eq!(actual.data(), reviewed.data());
        }
    }

    #[test]
    fn unknown_names_draw_the_placeholder() {
        assert!(resolve("no_such_icon").placeholder);
        assert!(resolve("").placeholder);
        assert!(!resolve("extrude").placeholder);
    }

    fn coverage(pm: &sk::Pixmap) -> usize {
        pm.pixels().iter().filter(|p| p.alpha() > 0).count()
    }

    #[test]
    fn every_master_draws_something_at_every_size_in_both_themes() {
        for id in master_ids() {
            for dark in [false, true] {
                for px in [16, 20, 26, 64] {
                    let Some(doc) = doc_for(id, px) else { panic!("{id}") };
                    let pm = render(doc, Look { dark, accent: None, vision: crate::theme::ColorVision::Off }, px)
                        .unwrap_or_else(|| panic!("{id} at {px}"));
                    assert!(coverage(&pm) > 8, "{id} at {px}px (dark {dark}) is blank");
                }
            }
        }
    }

    fn rgba(pm: &sk::Pixmap) -> Vec<u8> {
        pm.data().to_vec()
    }

    #[test]
    fn dark_theme_and_accents_change_colour_not_shape() {
        let doc = doc_for("extrude", 64).unwrap_or_else(|| panic!("extrude"));
        let light = render(doc, Look { dark: false, accent: None, vision: crate::theme::ColorVision::Off }, 64).unwrap_or_else(|| panic!("render"));
        let dark = render(doc, Look { dark: true, accent: None, vision: crate::theme::ColorVision::Off }, 64).unwrap_or_else(|| panic!("render"));
        assert_ne!(rgba(&light), rgba(&dark));
        let r = resolve("surface_extrude");
        assert_eq!(r.master, "extrude");
        let surface =
            render(doc, Look { dark: false, accent: r.accent, vision: crate::theme::ColorVision::Off }, 64).unwrap_or_else(|| panic!("render"));
        assert!(r.accent.is_some());
        assert_ne!(rgba(&light), rgba(&surface));
        // Same silhouette: the alpha channel is identical.
        let alpha = |pm: &sk::Pixmap| pm.pixels().iter().map(|p| p.alpha()).collect::<Vec<_>>();
        assert_eq!(alpha(&light), alpha(&surface));
    }

    #[test]
    fn hostile_artwork_is_an_error_not_a_panic() {
        for bad in [
            "",
            "<",
            "<svg",
            "<svg><path d=\"M 1\"/></svg>",
            "<svg><path d=\"X\"/></svg>",
            "<svg><g></svg>",
            "<svg><rect/></svg>",
            "<svg><path d=\"M 0 0 L 1 1\" fill=\"#123456\"/></svg>",
            "<svg><path d='M0 0 L 1 1' mask='url(#x)'/></svg>",
        ] {
            let _ = Doc::parse(bad);
        }
        assert!(Doc::parse("<svg><path d=\"M 1\"/></svg>").is_err());
        assert!(Doc::parse("<svg><g></svg>").is_err());
    }

    #[test]
    fn palette_roles_are_distinct_and_complete() {
        assert_eq!(ROLE_NAMES.len(), ROLE_SOURCE_HEX.len());
        let mut seen = std::collections::HashSet::new();
        assert!(
            ROLE_NAMES.iter().zip(ROLE_SOURCE_HEX).filter(|(n, _)| **n != "line").all(|(_, h)| seen.insert(*h)),
            "two roles share a source identifier"
        );
        assert!(ROLE_NAMES.contains(&"line"));
        assert_eq!(PALETTE.len(), 2);
        assert!(PALETTE.iter().all(|p| p.len() == ROLE_NAMES.len()));
    }
}
