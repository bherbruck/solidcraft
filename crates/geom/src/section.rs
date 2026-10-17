//! Backend-neutral section geometry in a model-space plane, in millimetres and radians.
//!
//! Curve and wire indices identify items in this result only, not persistent model geometry.
//! Producers own source identity, topology and accuracy evidence. [`SectionGeometry::validate_structure`]
//! checks bounded payloads, references and connected wires; it does not certify simple loops,
//! winding, hole nesting, region separation, source coincidence or an approximation bound.
//! Material producers must supply CCW outer loops and CW holes in the plane's x/y basis,
//! whose normal is x cross y. Conversion preserves those supplied orientations.
//!
//! Stored coordinates are finite and at most 1e12 mm in absolute value. Radii and conic
//! weights lie in 1e-12..=1e12; arc starts in -1e6..=1e6 radians and absolute sweeps in
//! 1e-12..=one turn (with 1e-12 radians of allowance). Positive conic weights are the supported
//! subset. These arithmetic limits do not promise a small absolute error at extreme magnitudes.
//! Closure tolerance is 1e-9..=1 mm, independent of approximation accuracy.
//!
//! Validate after deserialization and before use or serialization. The DTOs deliberately retain
//! [`Plane`]'s existing serde representation and construction behavior. Validation after parsing
//! cannot bound parsing allocations: an encoded-input boundary must also limit bytes and nesting.
//! The per-wire/per-region conversions revalidate the whole result on each call; they are not an
//! optimized batch-conversion API. Existing profile area/containment methods are not general
//! exact or certified measures for conics and freeform curves. Profile values contain geometry
//! only: callers retain the original result's accuracy declarations and local identities.

use std::f64::consts::TAU;

use serde::{Deserialize, Serialize};

use crate::{Loop2, MAX_BEZIER_DEGREE, Plane, Region2, Seg2, Vec2, Vec3};

/// Aggregate input records/references, and the selected conversion's output items, at most.
/// Input counts curves, wires, regions, contacts, curve uses and hole references together.
pub const MAX_ITEMS: usize = 100_000;
const MAX_NUMBER: f64 = 1e12;
const MIN_POSITIVE: f64 = 1e-12;
const MAX_START_ANGLE: f64 = 1e6;
const FRAME_TOLERANCE: f64 = 1e-8;

/// A section's geometric result, without source identity, freshness or persistence semantics.
/// Empty, open-wire and contact-only results remain distinct; no material is inferred from them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SectionGeometry {
    pub plane: Plane,
    pub curves: Vec<SectionCurve>,
    pub wires: Vec<SectionWire>,
    pub regions: Vec<SectionRegion>,
    /// Isolated contacts in plane coordinates, not invented closed regions.
    pub contacts: Vec<Vec2>,
}

/// One curve and the producer's declared accuracy relative to its source.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SectionCurve {
    pub geometry: Seg2,
    pub accuracy: CurveAccuracy,
}

/// How the producer represents its source; structure validation cannot certify this declaration.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CurveAccuracy {
    /// An analytic representation without fitting; not a certificate about the source or topology.
    Analytic,
    /// A producer-established bound, not merely a requested fitting tolerance.
    Bounded { max_deviation_mm: f64 },
    /// A deviation measured at samples only, with no bound between them. Zero remains sampled.
    Sampled { max_sample_deviation_mm: f64 },
}

/// An ordered connected chain. `closed` is explicit, never inferred or repaired.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SectionWire {
    pub curves: Vec<CurveUse>,
    pub closed: bool,
}

/// An oriented use of a curve by its result-local index.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CurveUse {
    pub curve: u32,
    pub forward: bool,
}

/// A producer-declared material region referring to closed wires. Shared wires are permitted.
/// The outer wire cannot also be a hole, and a hole cannot occur twice in this one region.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SectionRegion {
    pub outer: u32,
    pub holes: Vec<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("section geometry: {0}")]
pub struct Invalid(pub &'static str);

type Result<T> = std::result::Result<T, Invalid>;

fn require(condition: bool, message: &'static str) -> Result<()> {
    if condition { Ok(()) } else { Err(Invalid(message)) }
}

struct Budget(usize);
impl Budget {
    fn add(&mut self, n: usize) -> Result<()> {
        self.0 = self.0.checked_add(n).ok_or(Invalid("item budget"))?;
        require(self.0 <= MAX_ITEMS, "item budget")
    }
}

fn numbers(values: impl IntoIterator<Item = f64>) -> Result<()> {
    require(values.into_iter().all(|v| v.is_finite() && v.abs() <= MAX_NUMBER), "non-finite or excessive coordinate")
}

fn point(p: Vec2) -> Result<()> {
    numbers([p.x, p.y])
}

fn vector(p: Vec3) -> Result<()> {
    numbers([p.x, p.y, p.z])
}

/// Strictly check an existing plane without re-orthonormalizing it or changing its serde format.
/// Unit-length and orthogonality checks use 1e-8; the normal is derived as x cross y.
pub fn validate_plane(plane: &Plane) -> Result<()> {
    vector(plane.origin)?;
    vector(plane.x)?;
    vector(plane.y)?;
    require(
        (plane.x.len() - 1.0).abs() <= FRAME_TOLERANCE
            && (plane.y.len() - 1.0).abs() <= FRAME_TOLERANCE
            && plane.x.dot(plane.y).abs() <= FRAME_TOLERANCE,
        "nonorthonormal plane",
    )
}

fn controls(points: &[Vec2]) -> Result<()> {
    for p in points {
        point(*p)?;
    }
    let Some((first, rest)) = points.split_first() else { return Err(Invalid("empty controls")) };
    require(rest.iter().any(|p| p != first), "collapsed curve")
}

fn curve(curve: &SectionCurve) -> Result<()> {
    match curve.geometry {
        Seg2::Line { a, b } => controls(&[a, b])?,
        Seg2::Arc { center, radius, start, sweep } => {
            point(center)?;
            require(radius.is_finite() && (MIN_POSITIVE..=MAX_NUMBER).contains(&radius), "arc radius")?;
            require(start.is_finite() && start.abs() <= MAX_START_ANGLE, "arc start angle")?;
            require(sweep.is_finite() && (MIN_POSITIVE..=TAU + 1e-12).contains(&sweep.abs()), "arc sweep")?;
        }
        Seg2::Cubic { p0, p1, p2, p3 } => controls(&[p0, p1, p2, p3])?,
        Seg2::Conic { a, apex, b, w } => {
            controls(&[a, apex, b])?;
            require(w.is_finite() && (MIN_POSITIVE..=MAX_NUMBER).contains(&w), "conic weight")?;
        }
        Seg2::Bezier { n, ref p } => {
            require((4..=MAX_BEZIER_DEGREE).contains(&usize::from(n)), "Bezier degree")?;
            // The unused slots are serialized too: reject hidden non-finite/excessive values.
            for q in p {
                point(*q)?;
            }
            controls(p.get(..=usize::from(n)).ok_or(Invalid("Bezier degree"))?)?;
        }
    }
    require(curve.geometry.start().is_finite() && curve.geometry.end().is_finite(), "non-finite curve endpoint")?;
    match curve.accuracy {
        CurveAccuracy::Analytic => Ok(()),
        CurveAccuracy::Bounded { max_deviation_mm } => deviation(max_deviation_mm),
        CurveAccuracy::Sampled { max_sample_deviation_mm } => deviation(max_sample_deviation_mm),
    }
}

fn deviation(value: f64) -> Result<()> {
    require(value.is_finite() && (0.0..=MAX_NUMBER).contains(&value), "accuracy deviation")
}

impl SectionGeometry {
    /// Check finite supported payloads, aggregate counts, references and wire connectivity.
    /// This does not certify material validity, winding, nesting, source identity or accuracy.
    /// Count preflight precedes geometry evaluation; encoded input needs separate parsing limits.
    pub fn validate_structure(&self, closure_tolerance_mm: f64) -> Result<()> {
        let mut budget = Budget(0);
        budget.add(self.curves.len())?;
        budget.add(self.wires.len())?;
        budget.add(self.regions.len())?;
        budget.add(self.contacts.len())?;
        for wire in &self.wires {
            budget.add(wire.curves.len())?;
        }
        for region in &self.regions {
            budget.add(region.holes.len())?;
        }
        require(closure_tolerance_mm.is_finite() && (1e-9..=1.0).contains(&closure_tolerance_mm), "closure tolerance must be between 1e-9 and 1 mm")?;
        validate_plane(&self.plane)?;
        for c in &self.curves {
            curve(c)?;
        }
        for p in &self.contacts {
            point(*p)?;
        }
        for wire in &self.wires {
            require(!wire.curves.is_empty(), "empty wire")?;
            let mut first = None;
            let mut last: Option<Vec2> = None;
            for use_ in &wire.curves {
                let c = &self.curves.get(use_.curve as usize).ok_or(Invalid("curve reference"))?.geometry;
                let (a, b) = (c.start(), c.end());
                let (start, end) = if use_.forward { (a, b) } else { (b, a) };
                require(last.is_none_or(|p| p.dist(start) <= closure_tolerance_mm), "disconnected wire")?;
                first.get_or_insert(start);
                last = Some(end);
            }
            if wire.closed {
                require(first.zip(last).is_some_and(|(a, b)| a.dist(b) <= closure_tolerance_mm), "open closed wire")?;
            }
        }
        // One stamp array for all regions: neither pairwise duplicate checks nor clearing the
        // entire array per region. Sharing the same wire between different regions is allowed.
        let mut seen = vec![usize::MAX; self.wires.len()];
        for (stamp, region) in self.regions.iter().enumerate() {
            for id in std::iter::once(&region.outer).chain(&region.holes) {
                let wire = self.wires.get(*id as usize).ok_or(Invalid("wire reference"))?;
                require(wire.closed, "region boundary is not closed")?;
                let mark = seen.get_mut(*id as usize).ok_or(Invalid("wire reference"))?;
                require(*mark != stamp, "duplicate region boundary")?;
                *mark = stamp;
            }
        }
        Ok(())
    }

    /// Copy one closed wire to a profile loop, preserving its oriented curve uses.
    /// Revalidates the whole result; an open wire is never silently closed. Floating-point
    /// reversal must also keep the converted loop connected within the same tolerance.
    pub fn to_loop(&self, wire: u32, closure_tolerance_mm: f64) -> Result<Loop2> {
        self.validate_structure(closure_tolerance_mm)?;
        let wire = self.wires.get(wire as usize).ok_or(Invalid("wire reference"))?;
        require(wire.closed, "wire is not closed")?;
        let mut budget = Budget(0);
        budget.add(1)?;
        budget.add(wire.curves.len())?;
        self.copy_loop(wire, closure_tolerance_mm)
    }

    /// Copy one declared region to a profile, preserving winding and hole order.
    /// Revalidates the whole result and preflights expanded output before allocating it.
    /// This does not certify the producer's region. The original result is unchanged; the
    /// returned profile contains no accuracy declarations or result-local identities.
    pub fn to_region(&self, region: u32, closure_tolerance_mm: f64) -> Result<Region2> {
        self.validate_structure(closure_tolerance_mm)?;
        let region = self.regions.get(region as usize).ok_or(Invalid("region reference"))?;
        let mut budget = Budget(0);
        budget.add(1)?;
        budget.add(region.holes.len())?;
        for id in std::iter::once(&region.outer).chain(&region.holes) {
            let wire = self.wires.get(*id as usize).ok_or(Invalid("wire reference"))?;
            budget.add(wire.curves.len())?;
        }
        let outer = self.copy_loop(self.wires.get(region.outer as usize).ok_or(Invalid("wire reference"))?, closure_tolerance_mm)?;
        let mut holes = Vec::with_capacity(region.holes.len());
        for id in &region.holes {
            holes.push(self.copy_loop(self.wires.get(*id as usize).ok_or(Invalid("wire reference"))?, closure_tolerance_mm)?);
        }
        Ok(Region2 { outer, holes })
    }

    // Called only while the publicly validated result remains immutably borrowed and the
    // selected output's expansion has been counted. Index access stays fallible nonetheless.
    fn copy_loop(&self, wire: &SectionWire, closure_tolerance_mm: f64) -> Result<Loop2> {
        let mut segs = Vec::with_capacity(wire.curves.len());
        for use_ in &wire.curves {
            let c = self.curves.get(use_.curve as usize).ok_or(Invalid("curve reference"))?.geometry;
            segs.push(if use_.forward { c } else { c.reversed() });
        }
        let loop_ = Loop2 { segs };
        // Reversing an arc changes its stored start to start + sweep. Subtracting the sweep
        // at its new end need not recover the original angle exactly at large radii.
        require(loop_.is_closed(closure_tolerance_mm), "converted loop is not closed within tolerance")?;
        Ok(loop_)
    }
}

#[cfg(test)]
#[path = "section_tests.rs"]
mod tests;
