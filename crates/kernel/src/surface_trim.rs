//! Trim: a surface body cut where a tool meets it (a plane, another body's face, or sketch
//! curves swept onto it), keeping the side of a point or the regions points pick; and Mutual
//! Trim: two surfaces cut by each other, each keeping its picked side, extended to meet first
//! when one stops short of the other.

use solidcraft_geom::Vec3;
use truck_modeling as mt;

use crate::body::Body;
use crate::record::{Rebuild, rebuilt_body};
use crate::splitface::{SplitTool, ToolSide};
use crate::surface_extend::{ExtendLimit, ExtendMode, ExtendOptions};
use crate::surfaces::surface_body;
use crate::{KernelError, Result, guard};

/// Most points one trim picks with.
const MAX_PICKS: usize = 10_000;

/// What a trim keeps.
#[derive(Clone, Copy, Debug)]
pub enum TrimKeep<'a> {
    /// Every piece on the side of the tool this point is on. A tool of several sketch curves
    /// has no one side: the region the point picks is kept.
    Side(Vec3),
    /// The regions the points pick (a region: the pieces that hang together once the tool has
    /// cut the body, each picked by the piece nearest a point).
    Regions(&'a [Vec3]),
}

impl TrimKeep<'_> {
    fn points(&self) -> &[Vec3] {
        match self {
            TrimKeep::Side(p) => std::slice::from_ref(p),
            TrimKeep::Regions(at) => at,
        }
    }

    fn check(&self) -> Result<()> {
        let at = self.points();
        if at.is_empty() {
            return Err(KernelError::Invalid("pick what to keep".into()));
        }
        if at.len() > MAX_PICKS {
            return Err(KernelError::Invalid("too many points to keep".into()));
        }
        if at.iter().any(|p| !p.is_finite()) {
            return Err(KernelError::Invalid("a point to keep is not finite".into()));
        }
        Ok(())
    }
}

/// A surface body trimmed by a tool: its faces are cut where the tool meets them and the
/// pieces on the side of `keep` are kept. The result's history is over `[body]`.
pub fn trim(body: &Body, tool: &SplitTool, keep: Vec3) -> Result<Body> {
    trim_keep(body, tool, TrimKeep::Side(keep))
}

/// A surface body trimmed by a tool, keeping what `keep` says. Each kept face is the face it is
/// a piece of (its history is over `[body]`).
pub fn trim_keep(body: &Body, tool: &SplitTool, keep: TrimKeep) -> Result<Body> {
    body.require_brep("trim")?;
    keep.check()?;
    let total = body.solid.face_iter().count();
    let all: Vec<usize> = (0..total).collect();
    let (split, _) = crate::splitface::split_any(body, &all, tool)?;
    let one_side = match keep {
        TrimKeep::Side(p) if crate::splitface::field_count(tool)? == 1 => Some(crate::splitface::side_of_point(tool, p)?),
        _ => None,
    };
    let picked = match one_side {
        Some(_) => Vec::new(),
        None => nearest_faces(&split, keep.points())?,
    };
    guard("trim", || {
        let mut kept = Vec::new();
        match one_side {
            Some(side) => {
                for f in split.solid.face_iter() {
                    let x = crate::splitface::face_side(f, tool, split.size())?;
                    if (x >= 0.0) == (side >= 0.0) {
                        kept.push(f.clone());
                    }
                }
            }
            None => {
                let (region, count) = crate::splitface::regions(&split, tool)?;
                let mut wanted = vec![false; count];
                for f in &picked {
                    if let Some(w) = region.get(*f).and_then(|r| wanted.get_mut(*r)) {
                        *w = true;
                    }
                }
                for (f, r) in split.solid.face_iter().zip(&region) {
                    if wanted.get(*r).copied().unwrap_or(false) {
                        kept.push(f.clone());
                    }
                }
            }
        }
        if kept.is_empty() {
            return Err(KernelError::Invalid("nothing is left on that side of the tool".into()));
        }
        // The kept pieces are the split's own; its history carries them back to `body`.
        let trimmed = rebuilt_body(surface_body(kept), &[&split.solid], &Rebuild::default());
        Ok(crate::record::after(trimmed, &split, &crate::record::counts(&[body])))
    })
}

/// For each point, the face of `body` nearest it (over one tessellation).
fn nearest_faces(body: &Body, at: &[Vec3]) -> Result<Vec<usize>> {
    let m = body.tessellate(body.size() * 2e-3)?;
    at.iter()
        .map(|p| {
            let mut best: Option<(f64, u32)> = None;
            for (t, f) in m.triangles.iter().zip(&m.tri_face) {
                let Some([a, b, c]) = m.tri(t) else { continue };
                let d = ((a + b + c) / 3.0).dist(*p).min(a.dist(*p)).min(b.dist(*p)).min(c.dist(*p));
                if best.is_none_or(|(x, _)| d < x) {
                    best = Some((d, *f));
                }
            }
            best.map(|(_, f)| f as usize).ok_or_else(|| KernelError::Invalid("no face there".into()))
        })
        .collect()
}

/// Two surface bodies trimmed by each other: `a` cut where `b`'s surface (extended) meets it,
/// keeping what `keep_a` says, and `b` by `a`'s, keeping `keep_b`. A surface that stops short of
/// the other is extended to meet it when `extend` is set (naturally, its nearest boundary edge
/// up to the other's surface), else that is an error; one that ends on the other is kept whole.
/// Each body must be one face (its surface is the other's tool). The results' histories are over
/// `[a]` and `[b]`.
pub fn mutual_trim(a: &Body, b: &Body, keep_a: TrimKeep, keep_b: TrimKeep, extend: bool) -> Result<(Body, Body)> {
    for (x, name) in [(a, "first"), (b, "second")] {
        x.require_brep("trim")?;
        if !x.is_surface() {
            return Err(KernelError::Invalid(format!("the {name} body is a solid: a mutual trim takes two surfaces")));
        }
        if x.face_count() != 1 {
            return Err(KernelError::Invalid(format!(
                "not supported yet: a mutual trim of a surface of more than one face (the {name} body has {}; trim it by one face instead)",
                x.face_count()
            )));
        }
    }
    keep_a.check()?;
    keep_b.check()?;
    let ta = cut_by(a, &SplitTool::Face { body: b, face: 0 }, keep_a, extend, "first")?;
    let tb = cut_by(b, &SplitTool::Face { body: a, face: 0 }, keep_b, extend, "second")?;
    Ok((ta, tb))
}

/// `x` trimmed by `tool` where the tool crosses it; whole where it ends on the tool; extended to
/// the tool first (when `extend`) where it stops short of it.
fn cut_by(x: &Body, tool: &SplitTool, keep: TrimKeep, extend: bool, name: &str) -> Result<Body> {
    let side = ToolSide::of(tool)?;
    let size = x.size();
    let touch = size * 1e-6 + 1e-9;
    let m = x.tessellate(size * 2e-3)?;
    let (mut lo, mut hi, mut near) = (f64::INFINITY, f64::NEG_INFINITY, f64::INFINITY);
    for p in &m.positions {
        let v = side.at(*p);
        if !v.is_finite() {
            return Err(KernelError::Failed("the other surface cannot be evaluated over this one".into()));
        }
        lo = lo.min(v);
        hi = hi.max(v);
        near = near.min(v.abs());
    }
    if lo < -touch && hi > touch {
        return trim_keep(x, tool, keep);
    }
    if near <= touch {
        return Ok(crate::boolean_record::operand(x, 0, &crate::record::counts(&[x])));
    }
    if !extend {
        return Err(KernelError::Invalid(format!("the {name} surface does not reach the other one: extend it to meet")));
    }
    let edge = nearest_boundary_edge(x, &side)?;
    let opts = ExtendOptions { mode: ExtendMode::Natural, limit: ExtendLimit::UpTo(tool), neighbours: true };
    crate::extend_surface(x, &[edge], &opts)
}

/// A point in the middle of the boundary edge of `x` nearest the tool.
fn nearest_boundary_edge(x: &Body, side: &ToolSide) -> Result<Vec3> {
    guard("trim", || {
        let mut uses: std::collections::HashMap<mt::EdgeID, (usize, mt::Edge)> = std::collections::HashMap::new();
        for f in x.solid.face_iter() {
            for e in f.boundaries().iter().flat_map(|w| w.edge_iter()) {
                uses.entry(e.id()).or_insert((0, e.clone())).0 += 1;
            }
        }
        let mut best: Option<(f64, Vec3)> = None;
        for (count, e) in uses.values() {
            if *count != 1 {
                continue;
            }
            use mt::{BoundedCurve, ParametricCurve};
            let c = e.curve();
            let (t0, t1) = c.range_tuple();
            let mid = crate::body::from_p3(c.subs(0.5 * (t0 + t1)));
            let d = side.at(mid).abs();
            if d.is_finite() && best.is_none_or(|(b, _)| d < b) {
                best = Some((d, mid));
            }
        }
        best.map(|(_, p)| p).ok_or_else(|| KernelError::Invalid("the surface has no boundary edge to extend".into()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::{Change, FaceOrigin};
    use crate::{extrude_sheet, measure, patch_region, stitch};
    use solidcraft_geom::{Loop2, Plane, Region2, Seg2, Vec2};
    use std::f64::consts::PI;

    fn rel(a: f64, b: f64) -> f64 {
        (a - b).abs() / b.abs().max(1e-12)
    }

    fn rect(plane: &Plane, u0: f64, v0: f64, u1: f64, v1: f64) -> Body {
        let outer = Loop2::polygon(&[Vec2::new(u0, v0), Vec2::new(u1, v0), Vec2::new(u1, v1), Vec2::new(u0, v1)]);
        patch_region(plane, &Region2 { outer, holes: vec![] }).unwrap()
    }

    fn area(b: &Body) -> f64 {
        measure(b).unwrap().area
    }

    /// A U-shaped sheet (legs x = 0 and x = 10, 10 high, a base along y = 0, 10 deep in z) cut
    /// by the plane y = 5: one side keeps both legs' tops, one region pick only one of them, two
    /// picks both again.
    #[test]
    fn regions_keep_what_the_points_pick() {
        let chain = vec![
            Seg2::Line { a: Vec2::new(0.0, 10.0), b: Vec2::new(0.0, 0.0) },
            Seg2::Line { a: Vec2::new(0.0, 0.0), b: Vec2::new(10.0, 0.0) },
            Seg2::Line { a: Vec2::new(10.0, 0.0), b: Vec2::new(10.0, 10.0) },
        ];
        let u = extrude_sheet(&Plane::XY, &[chain], 0.0, 10.0).unwrap();
        let cut = SplitTool::Plane(Plane::new(Vec3::new(0.0, 5.0, 0.0), Vec3::X, Vec3::Z).unwrap());
        let side = trim(&u, &cut, Vec3::new(0.0, 8.0, 5.0)).unwrap();
        assert!(rel(area(&side), 100.0) < 1e-9, "{}", area(&side));
        let one = trim_keep(&u, &cut, TrimKeep::Regions(&[Vec3::new(0.0, 8.0, 5.0)])).unwrap();
        assert!(rel(area(&one), 50.0) < 1e-9, "{}", area(&one));
        let both = trim_keep(&u, &cut, TrimKeep::Regions(&[Vec3::new(0.0, 8.0, 5.0), Vec3::new(10.0, 9.0, 2.0)])).unwrap();
        assert!(rel(area(&both), 100.0) < 1e-9, "{}", area(&both));
        // The base side is one region: its three faces hang together.
        let base = trim_keep(&u, &cut, TrimKeep::Regions(&[Vec3::new(5.0, 0.0, 5.0)])).unwrap();
        assert!(rel(area(&base), 200.0) < 1e-9 && base.face_count() == 3, "{}", area(&base));
        // Each kept face is a piece of the face it was cut from.
        let h = one.history().unwrap();
        assert_eq!(h.operands, 1);
        assert!(
            h.faces.iter().all(|f| f.change == Change::Modified && matches!(f.origins.as_slice(), [FaceOrigin::Face { operand: 0, .. }])),
            "{h:?}"
        );
    }

    /// A 10 × 10 square cut by two sketch lines (x = 3 and x = 7): a side is no answer for two
    /// curves, so a point picks its region; two picks keep the two outer strips.
    #[test]
    fn several_curves_cut_regions() {
        let sq = rect(&Plane::XY, 0.0, 0.0, 10.0, 10.0);
        let lines = vec![
            vec![Seg2::Line { a: Vec2::new(3.0, -5.0), b: Vec2::new(3.0, 15.0) }],
            vec![Seg2::Line { a: Vec2::new(7.0, -5.0), b: Vec2::new(7.0, 15.0) }],
        ];
        let tool = SplitTool::Curves { plane: Plane { origin: Vec3::new(0.0, 0.0, 5.0), ..Plane::XY }, curves: lines, along: None };
        let mid = trim(&sq, &tool, Vec3::new(5.0, 5.0, 0.0)).unwrap();
        assert!(rel(area(&mid), 40.0) < 1e-9, "{}", area(&mid));
        let outer = trim_keep(&sq, &tool, TrimKeep::Regions(&[Vec3::new(1.0, 5.0, 0.0), Vec3::new(9.0, 5.0, 0.0)])).unwrap();
        assert!(rel(area(&outer), 60.0) < 1e-9 && outer.face_count() == 2, "{}", area(&outer));
    }

    /// A line sketched 10 above a 20 × 20 sheet at x = 5, swept along (1, 0, −1): it meets the
    /// sheet at x = 15, so the side of x = 0 keeps 15 × 20. A direction along the sketch plane
    /// is refused.
    #[test]
    fn curves_swept_aslant() {
        let sq = rect(&Plane::XY, 0.0, 0.0, 20.0, 20.0);
        let plane = Plane { origin: Vec3::new(0.0, 0.0, 10.0), ..Plane::XY };
        let curves = vec![vec![Seg2::Line { a: Vec2::new(5.0, -5.0), b: Vec2::new(5.0, 25.0) }]];
        let tool = SplitTool::Curves { plane, curves: curves.clone(), along: Some(Vec3::new(1.0, 0.0, -1.0)) };
        let t = trim(&sq, &tool, Vec3::new(1.0, 5.0, 0.0)).unwrap();
        assert!(rel(area(&t), 300.0) < 1e-9, "{}", area(&t));
        let flat = SplitTool::Curves { plane, curves, along: Some(Vec3::X) };
        assert!(trim(&sq, &flat, Vec3::new(1.0, 5.0, 0.0)).is_err());
    }

    /// The slot |x| ≤ 3 (a straight run z = 12…28 and round ends) sketched in front of a half
    /// cylinder (radius 10 about Z, y ≥ 0, 40 high) and swept along the sketch's normal: the
    /// piece inside it is ∫∫ R / √(R² − x²) dx dz over the slot (the straight run 16 · 2R·asin(3/R),
    /// the two round ends one disc), the piece outside the rest of 10π · 40.
    #[test]
    fn a_slot_projected_onto_a_cylinder() {
        let r: f64 = 10.0;
        let half = extrude_sheet(&Plane::XY, &[vec![Seg2::Arc { center: Vec2::ZERO, radius: r, start: 0.0, sweep: PI }]], 0.0, 40.0).unwrap();
        let a: f64 = 3.0;
        let slot = vec![
            Seg2::Line { a: Vec2::new(a, 12.0), b: Vec2::new(a, 28.0) },
            Seg2::Arc { center: Vec2::new(0.0, 28.0), radius: a, start: 0.0, sweep: PI },
            Seg2::Line { a: Vec2::new(-a, 28.0), b: Vec2::new(-a, 12.0) },
            Seg2::Arc { center: Vec2::new(0.0, 12.0), radius: a, start: PI, sweep: PI },
        ];
        let front = Plane { y: Vec3::Z, ..Plane::XZ };
        let tool = SplitTool::Curves { plane: front, curves: vec![slot], along: None };
        // The round ends: x = a·sin φ, the disc's height 2a·cos φ, dx = a·cos φ dφ (Simpson).
        let n = 4000;
        let f = |phi: f64| 2.0 * a * a * phi.cos().powi(2) * r / (r * r - (a * phi.sin()).powi(2)).sqrt();
        let h = PI / n as f64;
        let ends = (0..=n)
            .map(|k| {
                let w = if k == 0 || k == n {
                    1.0
                } else if k % 2 == 1 {
                    4.0
                } else {
                    2.0
                };
                w * f(-PI / 2.0 + h * k as f64)
            })
            .sum::<f64>()
            * h
            / 3.0;
        let inside = 16.0 * 2.0 * r * (a / r).asin() + ends;
        let on = Vec3::new(0.0, r, 20.0);
        let kept = trim(&half, &tool, on).unwrap();
        eprintln!("slot on the cylinder: {} measured, {inside} analytic", area(&kept));
        // (Measured on meshes.)
        assert!(rel(area(&kept), inside) < 1e-3, "{} vs {inside}", area(&kept));
        let rest = trim(&half, &tool, Vec3::new(r, 0.0, 20.0)).unwrap();
        let all = PI * r * 40.0;
        assert!(rel(area(&rest), all - inside) < 1e-3, "{} vs {}", area(&rest), all - inside);
        let picked = trim_keep(&half, &tool, TrimKeep::Regions(&[on])).unwrap();
        assert!(rel(area(&picked), area(&kept)) < 1e-9);
    }

    /// Two 20 × 10 sheets crossing at right angles (z = 0 and x = 0, both y = 0…10) trimmed by
    /// each other, each keeping its x > 0 / z > 0 half, and stitched: an L of two faces sharing
    /// their edge, 200 in area.
    #[test]
    fn crossing_sheets_trim_each_other_into_an_l() {
        let a = rect(&Plane::XY, -10.0, 0.0, 10.0, 10.0);
        let yz = Plane::new(Vec3::ZERO, Vec3::Y, Vec3::Z).unwrap();
        let b = rect(&yz, 0.0, -10.0, 10.0, 10.0);
        let (ta, tb) = mutual_trim(&a, &b, TrimKeep::Side(Vec3::new(5.0, 5.0, 0.0)), TrimKeep::Side(Vec3::new(0.0, 5.0, 5.0)), false).unwrap();
        assert!(rel(area(&ta), 100.0) < 1e-9 && rel(area(&tb), 100.0) < 1e-9);
        let l = stitch(&[&ta, &tb], 0.0).unwrap();
        assert_eq!((l.face_count(), l.edge_count()), (2, 7), "one shared edge");
        assert!(rel(area(&l), 200.0) < 1e-9);
        assert!(l.is_surface());
        let bb = measure(&l).unwrap().bbox;
        assert!(bb.min.x.abs() < 1e-9 && bb.min.z.abs() < 1e-9 && (bb.max.x - 10.0).abs() < 1e-9 && (bb.max.z - 10.0).abs() < 1e-9, "{bb:?}");
        // The pieces are the sheets' faces.
        for t in [&ta, &tb] {
            let h = t.history().unwrap();
            assert!(matches!(h.faces[0].origins.as_slice(), [FaceOrigin::Face { operand: 0, face: 0 }]) && h.faces[0].change == Change::Modified);
        }
    }

    /// A sheet that stops 3 short of the other (x = 0, z = 3…10) is refused until extended to
    /// meet it; extended, the two still make the L.
    #[test]
    fn a_short_sheet_is_extended_to_meet() {
        let a = rect(&Plane::XY, -10.0, 0.0, 10.0, 10.0);
        let yz = Plane::new(Vec3::ZERO, Vec3::Y, Vec3::Z).unwrap();
        let b = rect(&yz, 0.0, 3.0, 10.0, 10.0);
        let (ka, kb) = (TrimKeep::Side(Vec3::new(5.0, 5.0, 0.0)), TrimKeep::Side(Vec3::new(0.0, 5.0, 5.0)));
        let e = mutual_trim(&a, &b, ka, kb, false).unwrap_err().to_string();
        assert!(e.contains("extend"), "{e}");
        let (ta, tb) = mutual_trim(&a, &b, ka, kb, true).unwrap();
        assert!(rel(area(&ta), 100.0) < 1e-9 && rel(area(&tb), 100.0) < 1e-6, "{} {}", area(&ta), area(&tb));
        let l = stitch(&[&ta, &tb], 0.0).unwrap();
        assert_eq!((l.face_count(), l.edge_count()), (2, 7));
    }

    #[test]
    fn hostile_trims_are_refused() {
        let a = rect(&Plane::XY, 0.0, 0.0, 10.0, 10.0);
        let cut = SplitTool::Plane(Plane::new(Vec3::new(3.0, 0.0, 0.0), Vec3::Y, Vec3::Z).unwrap());
        assert!(trim_keep(&a, &cut, TrimKeep::Regions(&[])).is_err());
        assert!(trim_keep(&a, &cut, TrimKeep::Regions(&[Vec3::new(f64::NAN, 0.0, 0.0)])).is_err());
        assert!(trim(&a, &cut, Vec3::new(f64::INFINITY, 0.0, 0.0)).is_err());
        let many = vec![Vec3::ZERO; MAX_PICKS + 1];
        assert!(trim_keep(&a, &cut, TrimKeep::Regions(&many)).is_err());
        let solid = crate::box_solid(Vec3::ZERO, Vec3::new(1.0, 1.0, 1.0)).unwrap();
        assert!(mutual_trim(&a, &solid, TrimKeep::Side(Vec3::ZERO), TrimKeep::Side(Vec3::ZERO), true).is_err());
        let chain =
            vec![Seg2::Line { a: Vec2::new(0.0, 0.0), b: Vec2::new(5.0, 0.0) }, Seg2::Line { a: Vec2::new(5.0, 0.0), b: Vec2::new(5.0, 5.0) }];
        let two = extrude_sheet(&Plane::XY, &[chain], 0.0, 5.0).unwrap();
        let e = mutual_trim(&a, &two, TrimKeep::Side(Vec3::ZERO), TrimKeep::Side(Vec3::ZERO), true).unwrap_err().to_string();
        assert!(e.contains("not supported yet"), "{e}");
    }
}
