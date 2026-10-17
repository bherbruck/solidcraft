//! The shared volume of a pin in a hole of (nearly) its own size whose axis is a hair off the
//! hole's (parts placed from rounded coordinates).
//!
//! The pin's wall and the hole's wall then meet along two lines where they are all but tangent,
//! which the boolean cannot intersect, and the facets of the two walls pass through each other
//! by less than the boolean's sampled shortcuts can see. So the overlap is worked out in two
//! parts instead. The pin's wall is moved off the hole's by a clear step (`offset_body`) and the
//! bodies are intersected as they then are, which the boolean can do: narrowed, that gives the
//! overlap away from the walls (the core). In the band the move swept, what the two walls
//! share is a crescent in every section, whose area follows from the two circles alone. The
//! band is assumed to hold nothing but the two walls; that is checked by intersecting with the
//! pin widened by half the step and by the whole step, whose volumes must come out as the core
//! plus the crescents those wider circles give. Where they don't (a cross hole through the
//! band, a hole that is not round all the way, a pin end inside the hole), the overlap is not
//! answered here and the caller takes the general boolean.

use solidcraft_geom::{Aabb3, Vec3};

use crate::body::Body;
use crate::{BoolOp, FaceSurface};

/// The overlap of two bodies with a pin of one in a hole of the other, worked out as above.
#[derive(Clone, Debug)]
pub struct CoaxialOverlap {
    pub volume: f64,
    /// Approximate: the core's area plus the crescents' walls and ends.
    pub area: f64,
    pub centroid: Vec3,
    pub bbox: Aabb3,
    /// The largest offset between a pin's axis and its hole's, and the length they share.
    pub offset: f64,
    pub length: f64,
}

/// Largest axis offset plus radius difference handled, as a share of the radius.
const NEAR: f64 = 0.01;
/// The step the pin's wall is moved by, as a share of its radius.
const STEP: f64 = 0.1;

/// The faces of one body on one cylinder, the side its material is on and their extent along
/// the axis.
#[derive(Clone, Debug)]
struct Wall {
    faces: Vec<usize>,
    origin: Vec3,
    axis: Vec3,
    radius: f64,
    /// Material inside the cylinder (a pin), not outside it (a hole).
    boss: bool,
    /// Intervals along `axis` from `origin`, one per face.
    spans: Vec<(f64, f64)>,
}

/// A pin of one body (`pin_in_a`) in a hole of the other, and the length they share.
#[derive(Clone, Debug)]
struct Pair {
    pin: Wall,
    pin_in_a: bool,
    hole: Wall,
    /// The hole's axis seen from the pin's, across it, and how far.
    toward: Vec3,
    offset: f64,
    /// Where they share length along the pin's axis.
    shared: Vec<(f64, f64)>,
    step: f64,
}

/// The overlap of `a` and `b` when a pin of one sits in a hole of the other on nearly the same
/// cylinder (axes parallel, offset plus radius difference within 1 % of the radius, but not on
/// the same surface), worked out as the module says. `None` when there is no such pin, or the
/// bodies hold more than the pin and the hole where the work assumes they don't: the caller
/// then takes the general boolean.
pub fn coaxial_overlap(a: &Body, b: &Body) -> Option<CoaxialOverlap> {
    if a.is_mesh() || b.is_mesh() {
        return None;
    }
    crate::guard("near-coaxial overlap", || Ok(overlap(a, b))).ok().flatten()
}

fn overlap(a: &Body, b: &Body) -> Option<CoaxialOverlap> {
    let size = a.size().max(b.size());
    if !(size.is_finite() && size > 0.0) {
        return None;
    }
    // The same surface as the kernel takes it elsewhere: walls that close are one wall.
    let same = (size * 1e-7).max(1e-9) * 10.0;
    // Meshed only when two cylinders are close enough to be asked about.
    let (ca, cb) = (cylinders(a), cylinders(b));
    let close = |x: &Wall, y: &Wall| x.axis.dot(y.axis).abs() > 1.0 - 1e-9 && near(x, y, same).is_some();
    if !ca.iter().any(|x| cb.iter().any(|y| close(x, y))) {
        return None;
    }
    let (wa, wb) = (walls(a, ca, same)?, walls(b, cb, same)?);
    let mut pairs = Vec::new();
    for x in &wa {
        for y in &wb {
            if x.axis.dot(y.axis).abs() <= 1.0 - 1e-9 {
                continue;
            }
            let (pin, hole, pin_in_a) = match (x.boss, y.boss) {
                (true, false) => (x, y, true),
                (false, true) => (y, x, false),
                // Two pins or two holes on nearly one cylinder: not this case.
                _ if near(x, y, same).is_some() => return None,
                _ => continue,
            };
            let Some((toward, offset)) = near(pin, hole, same) else { continue };
            let shared = intersect(&union(&pin.spans), &union(&project(hole, pin)));
            if total(&shared) <= same {
                continue;
            }
            let step = (pin.radius * STEP).min(size * 0.02);
            pairs.push(Pair { pin: pin.clone(), pin_in_a, hole: hole.clone(), toward, offset, shared, step });
        }
    }
    if pairs.is_empty() {
        return None;
    }
    // A pin's faces move together: one step for each of them (the pin's walls do not repeat
    // across pairs unless one pin is near two holes, which the check below then answers).
    let shared_at = |k: f64| -> Option<(f64, Option<Body>)> {
        let moved = |body: &Body, in_a: bool| -> Option<Body> {
            let mine: Vec<&Pair> = pairs.iter().filter(|p| p.pin_in_a == in_a).collect();
            if mine.is_empty() {
                return Some(body.clone());
            }
            let shift = |i: usize, _| mine.iter().find(|p| p.pin.faces.contains(&i)).map(|p| p.step * k).unwrap_or(0.0);
            let m = crate::offset::offset_body(body, shift).ok()?;
            m.validity().is_empty().then_some(m)
        };
        let (ma, mb) = (moved(a, true)?, moved(b, false)?);
        let (r, report) = crate::capture_boolean_diagnostics(|| crate::ops::boolean(&ma, &mb, BoolOp::Intersect));
        if report.unverified().is_some() || report.warning().is_some() {
            return None;
        }
        match r.ok()? {
            None => Some((0.0, None)),
            Some(body) => Some((crate::measure(&body).ok()?.volume, Some(body))),
        }
    };
    // The crescents' volume with every pin grown by `k` steps.
    let crescents =
        |k: f64| -> f64 { pairs.iter().map(|p| total(&p.shared) * crescent(p.pin.radius + p.step * k, p.hole.radius, p.offset).area).sum() };
    let (core, core_body) = shared_at(-1.0)?;
    for k in [0.5, 1.0] {
        let (v, _) = shared_at(k)?;
        let gain = crescents(k) - crescents(-1.0);
        if !((v - core - gain).abs() <= 0.02 * gain + 1e-4 * v.abs() + 1e-12 * size.powi(3)) {
            return None;
        }
    }
    // The band held the two walls only: the overlap is the core plus the crescents.
    let mut volume = core;
    let mut moment = Vec3::ZERO;
    let mut area = 0.0;
    let mut bbox = Aabb3::EMPTY;
    if let Some(body) = &core_body
        && let Ok(m) = crate::measure(body)
    {
        moment = m.centroid * m.volume;
        area = m.area;
        bbox = m.bbox;
        volume = m.volume;
    }
    for p in &pairs {
        let c = crescent(p.pin.radius, p.hole.radius, p.offset);
        let length = total(&p.shared);
        // Less what the narrowed pin already reached (nothing, unless the hole is the smaller).
        let v = length * c.area - length * crescent(p.pin.radius - p.step, p.hole.radius, p.offset).area;
        let mid = p.shared.iter().map(|(s, t)| (s + t) * 0.5 * (t - s)).sum::<f64>() / length;
        let side = p.toward.cross(p.pin.axis);
        moment += (p.pin.origin + p.pin.axis * mid + p.toward * c.centroid) * v;
        volume += v;
        area += p.shared.len() as f64 * 2.0 * c.area + length * c.perimeter;
        for (s, t) in &p.shared {
            for q in crescent_outline(p.pin.radius, p.hole.radius, p.offset) {
                let at = p.pin.origin + p.toward * q.0 + side * q.1;
                bbox.add(at + p.pin.axis * *s);
                bbox.add(at + p.pin.axis * *t);
            }
        }
    }
    let centroid = if volume > 0.0 { moment / volume } else { bbox.center() };
    let offset = pairs.iter().map(|p| p.offset).fold(0.0, f64::max);
    let length = pairs.iter().map(|p| total(&p.shared)).sum();
    Some(CoaxialOverlap { volume: volume.max(0.0), area, centroid, bbox, offset, length })
}

/// The pin's offset to the hole, when the two are nearly on one cylinder but not on the same
/// surface: the unit direction from the pin's axis to the hole's (any, for one axis) and the
/// distance.
fn near(pin: &Wall, hole: &Wall, same: f64) -> Option<(Vec3, f64)> {
    let d = hole.origin - pin.origin;
    let across = d - pin.axis * d.dot(pin.axis);
    let offset = across.len();
    let gap = (pin.radius - hole.radius).abs();
    if offset <= same && gap <= same {
        return None;
    }
    if !(offset + gap <= NEAR * pin.radius.min(hole.radius)) {
        return None;
    }
    let toward = across.normalized().unwrap_or_else(|| pin.axis.any_perp());
    Some((toward, offset))
}

/// The body's cylindrical faces, each as a wall of its own with no side or extent yet.
fn cylinders(b: &Body) -> Vec<Wall> {
    (0..b.face_count())
        .filter_map(|i| match crate::face_surface(b, i)? {
            FaceSurface::Cylinder { origin, axis, radius } if origin.is_finite() && radius.is_finite() && radius > 0.0 => {
                Some(Wall { faces: vec![i], origin, axis: axis.normalized()?, radius, boss: false, spans: Vec::new() })
            }
            _ => None,
        })
        .collect()
}

/// The cylindrical walls of a body from its `cylinders`: its faces on each cylinder, which side
/// its material is on and where they run along the axis. `None` when the body can't be meshed.
fn walls(b: &Body, cylinders: Vec<Wall>, same: f64) -> Option<Vec<Wall>> {
    let mut out: Vec<Wall> = Vec::new();
    let mesh = b.tessellate_with((b.size() * 1e-3).max(1e-4), true).ok()?;
    for Wall { faces, origin, axis, radius, .. } in cylinders {
        let Some(&face) = faces.first() else { continue };
        // Which way the face looks: out of the axis for a pin, toward it for a hole.
        let (mut facing, mut lo, mut hi) = (0.0, f64::INFINITY, f64::NEG_INFINITY);
        for (t, f) in mesh.triangles.iter().zip(&mesh.tri_face) {
            if *f as usize != face {
                continue;
            }
            let Some([p, q, r]) = mesh.tri(t) else { continue };
            let c = (p + q + r) / 3.0;
            let radial = (c - origin) - axis * (c - origin).dot(axis);
            facing += (q - p).cross(r - p).dot(radial);
            for x in [p, q, r] {
                let s = (x - origin).dot(axis);
                lo = lo.min(s);
                hi = hi.max(s);
            }
        }
        if !(lo < hi) || facing == 0.0 {
            continue;
        }
        let boss = facing > 0.0;
        let wall = Wall { faces: vec![face], origin, axis, radius, boss, spans: vec![(lo, hi)] };
        match out.iter_mut().find(|w| w.boss == boss && on_one_cylinder(w, &wall, same)) {
            Some(w) => {
                let s = (origin - w.origin).dot(w.axis) * if axis.dot(w.axis) < 0.0 { -1.0 } else { 1.0 };
                let (a, b) = if axis.dot(w.axis) < 0.0 { (-hi, -lo) } else { (lo, hi) };
                w.faces.push(face);
                w.spans.push((a + s, b + s));
            }
            None => out.push(wall),
        }
    }
    Some(out)
}

fn on_one_cylinder(x: &Wall, y: &Wall, same: f64) -> bool {
    let d = y.origin - x.origin;
    x.axis.dot(y.axis).abs() > 1.0 - 1e-9 && (x.radius - y.radius).abs() < same && (d - x.axis * d.dot(x.axis)).len() < same
}

/// `wall`'s spans along `onto`'s axis.
fn project(wall: &Wall, onto: &Wall) -> Vec<(f64, f64)> {
    let flip = wall.axis.dot(onto.axis) < 0.0;
    let s = (wall.origin - onto.origin).dot(onto.axis);
    wall.spans.iter().map(|&(lo, hi)| if flip { (s - hi, s - lo) } else { (s + lo, s + hi) }).collect()
}

/// Sorted, disjoint intervals covering `spans`.
fn union(spans: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let mut v: Vec<(f64, f64)> = spans.iter().copied().filter(|(a, b)| a < b).collect();
    v.sort_by(|x, y| x.0.total_cmp(&y.0));
    let mut out: Vec<(f64, f64)> = Vec::with_capacity(v.len());
    for (a, b) in v {
        match out.last_mut() {
            Some(last) if a <= last.1 => last.1 = last.1.max(b),
            _ => out.push((a, b)),
        }
    }
    out
}

fn intersect(x: &[(f64, f64)], y: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    for &(a, b) in x {
        for &(c, d) in y {
            let (lo, hi) = (a.max(c), b.min(d));
            if lo < hi {
                out.push((lo, hi));
            }
        }
    }
    out
}

fn total(spans: &[(f64, f64)]) -> f64 {
    spans.iter().map(|(a, b)| b - a).sum()
}

/// The part of a disc of radius `pin` outside a disc of radius `hole` whose centre is `offset`
/// away along +x: its area, its centroid's x (y is 0) and its perimeter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Crescent {
    pub(crate) area: f64,
    pub(crate) centroid: f64,
    pub(crate) perimeter: f64,
}

pub(crate) fn crescent(pin: f64, hole: f64, offset: f64) -> Crescent {
    use std::f64::consts::PI;
    let (r, q, e) = (pin.max(0.0), hole.max(0.0), offset.abs());
    let disc = PI * r * r;
    if e >= r + q {
        return Crescent { area: disc, centroid: 0.0, perimeter: 2.0 * PI * r };
    }
    if e <= (r - q).abs() {
        return if q >= r {
            Crescent { area: 0.0, centroid: 0.0, perimeter: 0.0 }
        } else {
            let area = disc - PI * q * q;
            Crescent { area, centroid: -PI * q * q * e / area, perimeter: 2.0 * PI * (r + q) }
        };
    }
    // Half-angles of the lens's two segments, seen from each centre.
    let alpha = ((e * e + r * r - q * q) / (2.0 * e * r)).clamp(-1.0, 1.0).acos();
    let beta = ((e * e + q * q - r * r) / (2.0 * e * q)).clamp(-1.0, 1.0).acos();
    let segment = |radius: f64, half: f64| {
        let area = radius * radius * (half - half.sin() * half.cos());
        let w = 2.0 * half - (2.0 * half).sin();
        let from_centre = if w > 1e-12 { 4.0 * radius * half.sin().powi(3) / (3.0 * w) } else { radius };
        (area, from_centre)
    };
    let (sa, da) = segment(r, alpha);
    let (sb, db) = segment(q, beta);
    let lens = sa + sb;
    let area = (disc - lens).max(0.0);
    let moment = -(sa * da + sb * (e - db));
    let centroid = if area > 0.0 { moment / area } else { 0.0 };
    Crescent { area, centroid, perimeter: r * (2.0 * PI - 2.0 * alpha) + q * 2.0 * beta }
}

/// Points around the crescent's outline (x toward the hole's centre, y across), for its box.
fn crescent_outline(pin: f64, hole: f64, offset: f64) -> Vec<(f64, f64)> {
    let (r, q, e) = (pin, hole, offset);
    let alpha = if e > 0.0 { ((e * e + r * r - q * q) / (2.0 * e * r)).clamp(-1.0, 1.0).acos() } else { 0.0 };
    let n = 32;
    (0..=n)
        .map(|i| {
            let t = alpha + (2.0 * std::f64::consts::PI - 2.0 * alpha) * f64::from(i) / f64::from(n);
            (r * t.cos(), r * t.sin())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The crescent against the textbook lens of two equal circles, and its limits.
    #[test]
    fn crescent_matches_the_lens_formula() {
        let (r, e) = (3.0f64, 0.01f64);
        let lens = 2.0 * r * r * (e / (2.0 * r)).acos() - e / 2.0 * (4.0 * r * r - e * e).sqrt();
        let c = crescent(r, r, e);
        assert!((c.area - (std::f64::consts::PI * r * r - lens)).abs() < 1e-12, "{c:?}");
        // Thin: twice the radius times the offset, its centroid π/4 of the radius away from
        // the hole's centre, its outline nearly two half circles.
        assert!((c.area - 2.0 * r * e).abs() < 1e-6, "{c:?}");
        assert!((c.centroid + std::f64::consts::FRAC_PI_4 * r).abs() < 1e-2, "{c:?}");
        assert!((c.perimeter - 2.0 * std::f64::consts::PI * r).abs() < 1e-3, "{c:?}");
        assert_eq!(crescent(r, r, 0.0).area, 0.0);
        // A pin with clearance in its hole shares nothing; a pin wider than its hole a ring.
        assert_eq!(crescent(2.99, 3.0, 0.005).area, 0.0);
        let ring = crescent(3.01, 3.0, 0.0);
        assert!((ring.area - std::f64::consts::PI * (3.01f64.powi(2) - 9.0)).abs() < 1e-12, "{ring:?}");
    }

    /// A 20 x 20 x 5 plate with a hole of radius 3 through it, and a pin of radius `pin` through
    /// the hole, `offset` off its axis along X.
    fn plate_and_pin(pin: f64, offset: f64) -> (Body, Body) {
        let block = crate::box_solid(Vec3::new(-10.0, -10.0, 0.0), Vec3::new(10.0, 10.0, 5.0)).unwrap();
        let drill = crate::cylinder(Vec3::new(0.0, 0.0, -2.0), Vec3::Z, 3.0, 9.0).unwrap();
        let plate = crate::boolean(&block, &drill, BoolOp::Cut).unwrap().unwrap();
        (plate, crate::cylinder(Vec3::new(offset, 0.0, -2.0), Vec3::Z, pin, 9.0).unwrap())
    }

    /// The pin shares a crescent with the plate over the plate's thickness, in either order.
    #[test]
    fn a_pin_off_its_hole_shares_a_crescent() {
        let (plate, pin) = plate_and_pin(3.0, 0.01);
        let want = 5.0 * crescent(3.0, 3.0, 0.01).area;
        for (x, y) in [(&plate, &pin), (&pin, &plate)] {
            let o = coaxial_overlap(x, y).unwrap();
            assert!((o.volume - want).abs() < 1e-6 * want, "{o:?} {want}");
            assert!((o.offset - 0.01).abs() < 1e-9 && (o.length - 5.0).abs() < 1e-9, "{o:?}");
            // The crescent is on the pin's side away from the hole's axis (+X).
            assert!(o.centroid.x > 2.0 && o.bbox.min.z > -1e-9 && o.bbox.max.z < 5.0 + 1e-9, "{o:?}");
        }
    }

    /// A pin with clearance all round, though off the axis, shares nothing.
    #[test]
    fn a_pin_with_clearance_shares_nothing() {
        let (plate, pin) = plate_and_pin(2.995, 0.003);
        let o = coaxial_overlap(&plate, &pin).unwrap();
        assert!(o.volume < 1e-9, "{o:?}");
    }

    /// A slot through the hole's wall where the crescent would be: the band holds more than the
    /// two walls, the widened pin says so, and the pair is left to the boolean.
    #[test]
    fn a_slot_through_the_band_is_left_to_the_boolean() {
        let (plate, pin) = plate_and_pin(3.0, 0.01);
        let slot = crate::box_solid(Vec3::new(2.5, -1.0, -1.0), Vec3::new(4.0, 1.0, 6.0)).unwrap();
        let slotted = crate::boolean(&plate, &slot, BoolOp::Cut).unwrap().unwrap();
        assert!(coaxial_overlap(&slotted, &pin).is_none());
        // Pins on the very axis, or far off it, aren't asked about.
        assert!(coaxial_overlap(&plate, &plate_and_pin(3.0, 0.0).1).is_none());
        assert!(coaxial_overlap(&plate, &plate_and_pin(3.0, 0.5).1).is_none());
    }
}
