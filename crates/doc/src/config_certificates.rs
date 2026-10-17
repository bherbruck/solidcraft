//! Certificates are producer identities, never a nearest-surface selection.
use crate::{DocError, Document, FeatureKind, ModelBody, Result, SolvedSketch};
use serde::{Deserialize, Serialize};
use solidcraft_geom::Seg2;

fn valid_signature(outer: &[String], holes: &[Vec<String>]) -> bool {
    let valid = |ids: &[String]| {
        !ids.is_empty() && ids.len() <= 2000 && ids.iter().all(|id| !id.is_empty() && id.len() <= 128) && ids.windows(2).all(|pair| pair[0] < pair[1])
    };
    valid(outer)
        && holes.len() <= 128
        && holes.iter().all(|ids| valid(ids))
        && outer.len() + holes.iter().map(Vec::len).sum::<usize>() <= 2000
        && holes.windows(2).all(|pair| pair[0] < pair[1])
        && holes.iter().flatten().all(|id| outer.binary_search(id).is_err())
        && holes.iter().flatten().collect::<std::collections::BTreeSet<_>>().len() == holes.iter().map(Vec::len).sum::<usize>()
}
fn valid_signature_json(outer: Option<&serde_json::Value>, holes: Option<&serde_json::Value>) -> bool {
    let valid = |value: &serde_json::Value| {
        value.as_array().is_some_and(|ids| {
            !ids.is_empty()
                && ids.len() <= 2000
                && ids.iter().all(|id| id.as_str().is_some_and(|id| !id.is_empty() && id.len() <= 128))
                && ids.windows(2).all(|pair| pair[0].as_str() < pair[1].as_str())
        })
    };
    let Some(outer) = outer.filter(|value| valid(value)).and_then(serde_json::Value::as_array) else {
        return false;
    };
    let Some(holes) = holes.and_then(serde_json::Value::as_array).filter(|holes| holes.len() <= 128) else {
        return false;
    };
    if !holes.iter().all(valid) {
        return false;
    }
    let total = outer.len() + holes.iter().filter_map(serde_json::Value::as_array).map(Vec::len).sum::<usize>();
    let ids: Vec<_> = outer.iter().filter_map(serde_json::Value::as_str).collect();
    total <= 2000
        && holes
            .iter()
            .filter_map(serde_json::Value::as_array)
            .flatten()
            .filter_map(serde_json::Value::as_str)
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            == total - outer.len()
        && holes
            .iter()
            .filter_map(serde_json::Value::as_array)
            .flatten()
            .filter_map(serde_json::Value::as_str)
            .all(|id| ids.binary_search(&id).is_err())
        && holes.windows(2).all(|pair| {
            let a: Vec<_> = pair[0].as_array().into_iter().flatten().filter_map(serde_json::Value::as_str).collect();
            let b: Vec<_> = pair[1].as_array().into_iter().flatten().filter_map(serde_json::Value::as_str).collect();
            a < b
        })
}
pub(crate) fn cap_key(outer: &[String], holes: &[Vec<String>]) -> String {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    for ids in std::iter::once(outer).chain(holes.iter().map(Vec::as_slice)) {
        hash.update((ids.len() as u64).to_le_bytes());
        for id in ids {
            hash.update((id.len() as u64).to_le_bytes());
            hash.update(id.as_bytes());
        }
    }
    format!("{:x}", hash.finalize())
}
fn exact_region_signature(curves: &ExactSketchCurves, region: &solidcraft_geom::Region2) -> Option<solidcraft_kernel::CapSignature> {
    if region.outer.segs.len() + region.holes.iter().map(|hole| hole.segs.len()).sum::<usize>() > 4096 || region.holes.len() > 128 {
        return None;
    }
    let signature = |outline: &solidcraft_geom::Loop2| {
        let mut ids = outline.segs.iter().map(|segment| curves.find(segment)).collect::<Option<Vec<_>>>()?;
        ids.sort();
        ids.dedup();
        Some(ids)
    };
    let outer = signature(&region.outer)?;
    let mut holes = region.holes.iter().map(signature).collect::<Option<Vec<_>>>()?;
    holes.sort();
    valid_signature(&outer, &holes).then_some((outer, holes))
}

/// Charge all source-curve comparisons before matching; over-budget tools stay usable but uncertified.
type CapBindings = Vec<Vec<Option<solidcraft_kernel::CapSignature>>>;
pub(crate) fn extrude_cap_bindings(sketch: &SolvedSketch, bodies: &[solidcraft_kernel::Body], budget: usize) -> Option<CapBindings> {
    use solidcraft_kernel::FaceSource;
    if sketch.sketch.curves.len() > 2000 {
        return None;
    }
    let mut faces = 0usize;
    let mut comparisons = 0usize;
    for body in bodies {
        for source in body.face_sources().into_iter().flatten() {
            faces = faces.checked_add(1)?;
            if faces > 50_000 {
                return None;
            }
            let segments = match source {
                Some(FaceSource::ExtrudeSide { .. }) => 1,
                Some(FaceSource::ExtrudeCap { region, .. }) => {
                    region.outer.segs.len().checked_add(region.holes.iter().map(|hole| hole.segs.len()).sum::<usize>())?
                }
                _ => 0,
            };
            comparisons = comparisons.checked_add(segments.checked_mul(sketch.sketch.curves.len())?)?;
            if comparisons > budget {
                return None;
            }
        }
    }
    let curves = ExactSketchCurves::new(sketch);
    let mut counts = std::collections::BTreeMap::new();
    let mut bindings = Vec::with_capacity(bodies.len());
    for body in bodies {
        let mut cached: Option<(&solidcraft_geom::Region2, Option<solidcraft_kernel::CapSignature>)> = None;
        let mut values = Vec::new();
        for source in body.face_sources().into_iter().flatten() {
            let signature = if let Some(FaceSource::ExtrudeCap { region, high, .. }) = source {
                let signature = match &cached {
                    Some((previous, signature)) if *previous == region => signature.clone(),
                    _ => exact_region_signature(&curves, region),
                };
                cached = Some((region, signature.clone()));
                if let Some((outer, holes)) = &signature {
                    *counts.entry((*high, outer.clone(), holes.clone())).or_insert(0usize) += 1;
                }
                signature
            } else {
                None
            };
            values.push(signature);
        }
        bindings.push(values);
    }
    for (body, values) in bodies.iter().zip(&mut bindings) {
        for (source, value) in body.face_sources().into_iter().flatten().zip(values) {
            if let (Some(FaceSource::ExtrudeCap { high, .. }), Some((outer, holes))) = (source, value.as_ref())
                && counts.get(&(*high, outer.clone(), holes.clone())) != Some(&1)
            {
                *value = None;
            }
        }
    }
    Some(bindings)
}

/// Cap signatures for a thin extrude (#1519). A thin wall's caps are bounded by offset curves
/// the sketch doesn't have, so [`extrude_cap_bindings`] can never certify them. A wall built
/// from one region is signed by the curves it follows instead: the cap of this extrude whose
/// wall runs along them. That is the signature its solid caps had when the wall follows the
/// whole profile (a rectangle's four lines), so a sketch attached to a solid cap keeps its face
/// when the extrude is made thin, and the other way round. Several regions (walls apart, or one
/// meeting itself) are left uncertified: they keep their history names.
pub(crate) fn thin_extrude_cap_bindings(curves: &[String], regions: usize, bodies: &[solidcraft_kernel::Body]) -> CapBindings {
    use solidcraft_kernel::FaceSource;
    let mut outer = curves.to_vec();
    outer.sort();
    outer.dedup();
    let signed = regions == 1 && bodies.len() == 1 && valid_signature(&outer, &[]);
    bodies
        .iter()
        .map(|body| {
            let sources: Vec<_> = body.face_sources().into_iter().flatten().collect();
            let ends =
                |high: bool| sources.iter().filter(|source| matches!(source, Some(FaceSource::ExtrudeCap { high: h, .. }) if *h == high)).count();
            // One cap at each end, or none of them is certified.
            let unique = ends(true) <= 1 && ends(false) <= 1;
            sources
                .iter()
                .map(|source| (signed && unique && matches!(source, Some(FaceSource::ExtrudeCap { .. }))).then(|| (outer.clone(), Vec::new())))
                .collect()
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum FaceCertificate {
    Box {
        axis: u8,
        high: bool,
    },
    /// A cylinder primitive's cap: `high` is its `top` (the far end), else its `bottom`.
    CylinderCap {
        high: bool,
    },
    ExtrudeCap {
        sketch: u64,
        high: bool,
        outer: Vec<String>,
        holes: Vec<Vec<String>>,
    },
    ExtrudeSide {
        sketch: u64,
        curve: String,
    },
    Imported {
        payload_sha256: String,
        item: u64,
        face: u64,
    },
    ShellPlanar {
        source_producer: u64,
        source: Box<FaceCertificate>,
        inner: bool,
    },
    /// A flat face known by its exact history name (the target's `name`, without piece
    /// numbers): what the kernel's face history says it came from, followed through every
    /// feature since (a cut wall `F5:side:l1`, a chamfer `F7:blend:…`, a pattern copy
    /// `F4:top@F6.2`, a face a join merged from two features). Used where no construction
    /// certificate above names the face uniquely. Its plane is the face's exact current plane in
    /// the frame [`lineage_frame`] gives it. See docs/face-attachments.md.
    Lineage,
}

impl FaceCertificate {
    pub fn remap_features(&mut self, features: &std::collections::BTreeMap<u64, u64>) {
        if let Self::ShellPlanar { source_producer, source, .. } = self {
            if let Some(mapped) = features.get(source_producer) {
                *source_producer = *mapped;
            }
            if !matches!(source.as_ref(), Self::ShellPlanar { .. }) {
                source.remap_features(features);
            }
        }
        if let Self::ExtrudeSide { sketch, .. } | Self::ExtrudeCap { sketch, .. } = self
            && let Some(mapped) = features.get(sketch)
        {
            *sketch = *mapped;
        }
    }
    pub fn validate(&self) -> Result<()> {
        let valid = match self {
            Self::Box { axis, .. } => *axis <= 2,
            Self::CylinderCap { .. } => true,
            Self::ExtrudeCap { sketch, outer, holes, .. } => *sketch != 0 && valid_signature(outer, holes),
            Self::ExtrudeSide { sketch, curve } => *sketch != 0 && !curve.is_empty() && curve.len() <= 128,
            Self::Imported { payload_sha256, item, face } => {
                *item != 0
                    && *face != 0
                    && payload_sha256.len() == 64
                    && payload_sha256.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            }
            Self::ShellPlanar { source_producer, source, .. } => {
                *source_producer != 0 && !matches!(source.as_ref(), Self::ShellPlanar { .. } | Self::Lineage) && source.validate().is_ok()
            }
            Self::Lineage => true,
        };
        if valid { Ok(()) } else { Err(DocError::Invalid("invalid face source certificate".into())) }
    }
}

pub(crate) fn validate_certificate_json(value: &serde_json::Value) -> Result<()> {
    if value.is_null() {
        return Ok(());
    }
    let fields = value.as_object().ok_or_else(|| DocError::Invalid("face certificate must be an object".into()))?;
    let nonzero = |field: &str| fields.get(field).and_then(serde_json::Value::as_u64).is_some_and(|id| id != 0);
    let valid = match fields.get("kind").and_then(serde_json::Value::as_str) {
        Some("box") => {
            fields.len() == 3
                && fields.keys().all(|key| ["kind", "axis", "high"].contains(&key.as_str()))
                && fields.get("axis").and_then(serde_json::Value::as_u64).is_some_and(|axis| axis <= 2)
                && fields.get("high").and_then(serde_json::Value::as_bool).is_some()
        }
        Some("cylinder_cap") => {
            fields.len() == 2
                && fields.keys().all(|key| ["kind", "high"].contains(&key.as_str()))
                && fields.get("high").and_then(serde_json::Value::as_bool).is_some()
        }
        Some("extrude_cap") => {
            fields.len() == 5
                && fields.keys().all(|key| ["kind", "sketch", "high", "outer", "holes"].contains(&key.as_str()))
                && nonzero("sketch")
                && fields.get("high").and_then(serde_json::Value::as_bool).is_some()
                && valid_signature_json(fields.get("outer"), fields.get("holes"))
        }
        Some("extrude_side") => {
            fields.len() == 3
                && fields.keys().all(|key| ["kind", "sketch", "curve"].contains(&key.as_str()))
                && nonzero("sketch")
                && fields.get("curve").and_then(serde_json::Value::as_str).is_some_and(|curve| !curve.is_empty() && curve.len() <= 128)
        }
        Some("imported") => {
            fields.len() == 4
                && fields.keys().all(|key| ["kind", "payload_sha256", "item", "face"].contains(&key.as_str()))
                && nonzero("item")
                && nonzero("face")
                && fields
                    .get("payload_sha256")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|digest| digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)))
        }
        Some("lineage") => fields.len() == 1,
        Some("shell_planar") => {
            fields.len() == 4
                && fields.keys().all(|key| ["kind", "source_producer", "source", "inner"].contains(&key.as_str()))
                && nonzero("source_producer")
                && fields.get("inner").and_then(serde_json::Value::as_bool).is_some()
                && fields.get("source").is_some_and(|source| {
                    source
                        .as_object()
                        .is_some_and(|source| !matches!(source.get("kind").and_then(serde_json::Value::as_str), Some("shell_planar" | "lineage")))
                        && validate_certificate_json(source).is_ok()
                })
        }
        _ => false,
    };
    if valid { Ok(()) } else { Err(DocError::Invalid("malformed or oversized face source certificate".into())) }
}

/// Canonical diagnostic role; producer identity remains in the typed certificate.
pub(crate) fn shell_certificate_role(source: &FaceCertificate, inner: bool) -> Result<String> {
    source.validate()?;
    let role = match source {
        FaceCertificate::Box { axis, high } => {
            let axis = ["x", "y", "z"].get(*axis as usize).ok_or_else(|| DocError::Invalid("invalid shell source axis".into()))?;
            format!("box:{}{axis}", if *high { "+" } else { "-" })
        }
        FaceCertificate::CylinderCap { high } => format!("cylinder:{}", cylinder_cap_role(*high)),
        FaceCertificate::ExtrudeSide { curve, .. } => format!("side:{curve}"),
        FaceCertificate::ExtrudeCap { high, outer, holes, .. } => format!("cap:{}:{}", if *high { "high" } else { "low" }, cap_key(outer, holes)),
        FaceCertificate::Imported { payload_sha256, item, face } => format!("import:{payload_sha256}:{item}:{face}"),
        FaceCertificate::ShellPlanar { .. } => return Err(DocError::Invalid("nested shell source certificates are unsupported".into())),
        FaceCertificate::Lineage => return Err(DocError::Invalid("a shell's source needs a construction certificate".into())),
    };
    Ok(format!("shell:{}:{role}", if inner { "inner" } else { "outer" }))
}

/// A cylinder cap's role, as the cylinder's face names call it.
pub(crate) fn cylinder_cap_role(high: bool) -> &'static str {
    if high { "top" } else { "bottom" }
}

pub(crate) fn flat_source_certificate(source: &solidcraft_kernel::FaceSource) -> Option<(u64, FaceCertificate)> {
    use solidcraft_kernel::FaceSource;
    match source {
        FaceSource::Box { producer: Some(producer), axis, high, .. } => Some((*producer, FaceCertificate::Box { axis: *axis, high: *high })),
        FaceSource::CylinderCap { producer, high, .. } => Some((*producer, FaceCertificate::CylinderCap { high: *high })),
        FaceSource::CertifiedExtrudeSide { producer, sketch, curve, .. } => {
            Some((*producer, FaceCertificate::ExtrudeSide { sketch: *sketch, curve: curve.clone() }))
        }
        FaceSource::CertifiedExtrudeCap { producer, sketch, high, outer, holes, .. } => {
            Some((*producer, FaceCertificate::ExtrudeCap { sketch: *sketch, high: *high, outer: outer.clone(), holes: holes.clone() }))
        }
        FaceSource::Imported { producer: Some(producer), payload_sha256: Some(digest), item, face } => {
            Some((*producer, FaceCertificate::Imported { payload_sha256: digest.clone(), item: *item, face: *face }))
        }
        _ => None,
    }
}

pub(crate) fn flat_source_matches(certificate: &FaceCertificate, source: &solidcraft_kernel::FaceSource, producer: u64) -> bool {
    use solidcraft_kernel::FaceSource;
    match (certificate, source) {
        (FaceCertificate::Box { axis, high }, FaceSource::Box { producer: Some(current), axis: current_axis, high: current_high, .. }) => {
            producer == *current && axis == current_axis && high == current_high
        }
        (FaceCertificate::CylinderCap { high }, FaceSource::CylinderCap { producer: current, high: current_high, .. }) => {
            producer == *current && high == current_high
        }
        (
            FaceCertificate::ExtrudeSide { sketch, curve },
            FaceSource::CertifiedExtrudeSide { producer: current, sketch: current_sketch, curve: current_curve, .. },
        ) => producer == *current && sketch == current_sketch && curve == current_curve,
        (
            FaceCertificate::ExtrudeCap { sketch, high, outer, holes },
            FaceSource::CertifiedExtrudeCap {
                producer: current,
                sketch: current_sketch,
                high: current_high,
                outer: current_outer,
                holes: current_holes,
                ..
            },
        ) => producer == *current && sketch == current_sketch && high == current_high && outer == current_outer && holes == current_holes,
        (
            FaceCertificate::Imported { payload_sha256, item, face },
            FaceSource::Imported { producer: Some(current), payload_sha256: Some(digest), item: current_item, face: current_face },
        ) => producer == *current && payload_sha256 == digest && item == current_item && face == current_face,
        _ => false,
    }
}

/// Match a generated segment to one exact analytic sketch curve. Coincident
/// candidates are ambiguous and never resolved by distance or candidate order.
pub(crate) struct ExactSketchCurves<'a> {
    /// Each analytic curve's id, whether it is projected (linked) reference geometry, and its
    /// segments.
    segments: Vec<(&'a str, bool, Vec<Seg2>)>,
}

impl<'a> ExactSketchCurves<'a> {
    /// Prepare only bounded analytic curves, once per certification pass. Point lookup
    /// and segment allocation must not be repeated for every generated face.
    pub(crate) fn new(sketch: &'a SolvedSketch) -> Self {
        let mut segments = Vec::new();
        if sketch.sketch.curves.len() > 2000 {
            return Self { segments };
        }
        for (index, curve) in sketch.sketch.curves.iter().enumerate() {
            if curve.construction
                || !matches!(
                    curve.kind,
                    solidcraft_sketch::CurveKind::Line { .. }
                        | solidcraft_sketch::CurveKind::Arc { .. }
                        | solidcraft_sketch::CurveKind::Circle { .. }
                )
            {
                continue;
            }
            let originals = if let solidcraft_sketch::CurveKind::Circle { c, r } = curve.kind {
                sketch
                    .sketch
                    .point(c)
                    .map(|center| vec![Seg2::Arc { center, radius: r, start: 0., sweep: std::f64::consts::TAU }])
                    .unwrap_or_default()
            } else {
                sketch.sketch.segs(index)
            };
            segments.push((curve.id.as_str(), curve.link.is_some(), originals));
        }
        Self { segments }
    }

    /// The one sketch curve a generated segment lies on. A curve drawn over projected
    /// reference geometry (a profile traced over a face's live boundary) is that segment's
    /// source: the projection only marks where it was drawn. Two drawn curves, or two
    /// projections, over the same segment stay ambiguous.
    pub(crate) fn find(&self, generated: &Seg2) -> Option<String> {
        let mut drawn = None;
        let mut projected = None;
        for (id, linked, originals) in &self.segments {
            if originals.iter().any(|original| exact_segment_contains(original, generated)) {
                let slot = if *linked { &mut projected } else { &mut drawn };
                if slot.is_some() {
                    *slot = Some(None);
                } else {
                    *slot = Some(Some(*id));
                }
            }
        }
        match (drawn, projected) {
            (Some(id), _) => id.map(str::to_owned),
            (None, Some(id)) => id.map(str::to_owned),
            (None, None) => None,
        }
    }

    /// [`find`](Self::find), else the one sketch line a generated straight segment runs along
    /// and overlaps: a profile a feature grew to overlap a flush face (its neighbours lengthened
    /// past the sketch's corners) still names its sides by the lines they were drawn on. Two
    /// candidates stay ambiguous.
    pub(crate) fn find_along(&self, generated: &Seg2) -> Option<String> {
        if let Some(id) = self.find(generated) {
            return Some(id);
        }
        let Seg2::Line { a: c, b: d } = *generated else { return None };
        let mut found: Option<Option<&str>> = None;
        for (id, linked, originals) in &self.segments {
            if *linked {
                continue;
            }
            let along = originals.iter().any(|original| {
                let Seg2::Line { a, b } = *original else { return false };
                let direction = b - a;
                let scale = direction.len().max((d - c).len()).max(1.);
                if direction.len() <= 1e-12
                    || direction.cross(c - a).abs() > 1e-10 * scale * scale
                    || direction.cross(d - a).abs() > 1e-10 * scale * scale
                {
                    return false;
                }
                let (tc, td) = ((c - a).dot(direction) / direction.len2(), (d - a).dot(direction) / direction.len2());
                tc.min(td).max(0.) < tc.max(td).min(1.) - 1e-9
            });
            if along {
                found = Some(if found.is_some() { None } else { Some(*id) });
            }
        }
        found.flatten().map(str::to_owned)
    }
}

impl ExactSketchCurves<'_> {
    /// The sketch curves a generated segment is drawn on: [`find_along`](Self::find_along)'s one
    /// curve, else every drawn curve that contains it or runs along and overlaps it (projections
    /// only where no drawn curve does), in the sketch's order. A segment two coincident curves
    /// were drawn over (four squares drawn edge to edge), or one a profile merged from collinear
    /// curves (the outline of several adjacent profiles extruded together), is drawn on all of
    /// them: the set is exact and fixed by the sketch, so its face is named by it (the first its
    /// name, the others aliases) rather than not at all. Empty when it lies on none.
    pub(crate) fn find_all_along(&self, generated: &Seg2) -> Vec<String> {
        if let Some(id) = self.find_along(generated) {
            return vec![id];
        }
        let runs_along = |original: &Seg2| {
            if exact_segment_contains(original, generated) {
                return true;
            }
            let (Seg2::Line { a, b }, Seg2::Line { a: c, b: d }) = (*original, *generated) else { return false };
            let direction = b - a;
            let scale = direction.len().max((d - c).len()).max(1.);
            if direction.len() <= 1e-12
                || direction.cross(c - a).abs() > 1e-10 * scale * scale
                || direction.cross(d - a).abs() > 1e-10 * scale * scale
            {
                return false;
            }
            let (tc, td) = ((c - a).dot(direction) / direction.len2(), (d - a).dot(direction) / direction.len2());
            tc.min(td).max(0.) < tc.max(td).min(1.) - 1e-9
        };
        let pick = |linked: bool| -> Vec<String> {
            self.segments
                .iter()
                .filter(|(_, l, originals)| *l == linked && originals.iter().any(runs_along))
                .map(|(id, _, _)| (*id).to_owned())
                .collect()
        };
        let drawn = pick(false);
        if drawn.is_empty() { pick(true) } else { drawn }
    }
}

fn exact_segment_contains(original: &Seg2, generated: &Seg2) -> bool {
    match (original, generated) {
        (Seg2::Line { a, b }, Seg2::Line { a: c, b: d }) => {
            let direction = *b - *a;
            let scale = direction.len().max((*d - *c).len()).max(1.);
            let interval = |point: solidcraft_geom::Vec2| {
                let t = (point - *a).dot(direction) / direction.len2();
                (-1e-10..=1. + 1e-10).contains(&t)
            };
            direction.len() > 1e-12
                && direction.cross(*c - *a).abs() <= 1e-10 * scale * scale
                && direction.cross(*d - *a).abs() <= 1e-10 * scale * scale
                && interval(*c)
                && interval(*d)
        }
        (Seg2::Arc { center: a, radius: ar, start: os, sweep: ow }, Seg2::Arc { center: b, radius: br, start: gs, sweep: gw }) => {
            let tau = std::f64::consts::TAU;
            let tolerance = 1e-10;
            let length = ow.abs();
            let generated_length = gw.abs();
            if a.dist(*b) > 1e-10 * ar.max(*br).max(1.)
                || (ar - br).abs() > 1e-10 * ar.max(*br).max(1.)
                || length > tau + tolerance
                || generated_length > length + tolerance
            {
                return false;
            }
            if length >= tau - tolerance {
                return true;
            }
            let original_low = if *ow >= 0. { *os } else { os + ow };
            let generated_low = if *gw >= 0. { *gs } else { gs + gw };
            let offset = (generated_low - original_low).rem_euclid(tau);
            let offset = if offset > tau - tolerance { 0. } else { offset };
            offset + generated_length <= length + tolerance
        }
        _ => false,
    }
}

impl Document {
    pub fn configuration_face_target(&self, body: &ModelBody, index: usize, point: solidcraft_geom::Vec3) -> Result<crate::config::ConfigFaceTarget> {
        let names = crate::naming::face_names(body);
        let mut name = names
            .get(index)
            .map(|name| crate::naming::strip_piece(name).to_string())
            .ok_or_else(|| DocError::Unknown("face source name missing".into()))?;
        let origin = match body.body.face_sources().and_then(|sources| sources.get(index)).and_then(Option::as_ref) {
            Some(solidcraft_kernel::FaceSource::Imported { producer: Some(producer), .. })
            | Some(solidcraft_kernel::FaceSource::CertifiedExtrudeSide { producer, .. })
            | Some(solidcraft_kernel::FaceSource::CertifiedExtrudeCap { producer, .. })
            | Some(solidcraft_kernel::FaceSource::ShellPlanar { producer: Some(producer), .. })
            | Some(solidcraft_kernel::FaceSource::Box { producer: Some(producer), .. })
            | Some(solidcraft_kernel::FaceSource::CylinderCap { producer, .. }) => *producer,
            _ => name
                .strip_prefix('F')
                .and_then(|name| name.split_once(':'))
                .and_then(|(id, _)| id.parse().ok())
                .ok_or_else(|| DocError::Invalid("face source producer missing".into()))?,
        };
        let certificate = self.configuration_face_certificate(body, index)?;
        if let Some(FaceCertificate::ExtrudeSide { curve, .. }) = &certificate {
            name = format!("F{origin}:side:{curve}");
        } else if let Some(FaceCertificate::ShellPlanar { source, inner, .. }) = &certificate {
            name = format!("F{origin}:{}", shell_certificate_role(source, *inner)?);
        } else if !name.starts_with(&format!("F{origin}:")) {
            name = format!("F{origin}:face:{index}");
        }
        let inverse = crate::mat_inverse(&body.source_transform).ok_or_else(|| DocError::Invalid("invalid definition placement".into()))?;
        Ok(crate::config::ConfigFaceTarget {
            body: body.definition_name().into(),
            origin_feature: origin,
            name,
            point: crate::apply_point(&inverse, point),
            certificate,
        })
    }
    pub fn configuration_face_certificate(&self, body: &ModelBody, index: usize) -> Result<Option<FaceCertificate>> {
        let Some(source) = body.body.face_sources().and_then(|sources| sources.get(index)).and_then(Option::as_ref) else {
            return Ok(None);
        };
        if let solidcraft_kernel::FaceSource::ShellPlanar { producer: Some(producer), source, inner, .. } = source {
            if !self.feature(*producer).is_some_and(|feature| matches!(feature.kind, FeatureKind::Shell { .. })) {
                return Err(DocError::Invalid("shell source producer missing".into()));
            }
            let (source_producer, source) =
                flat_source_certificate(source).ok_or_else(|| DocError::Invalid("shell has no flat certified source".into()))?;
            if !self.flat_certificate_producer_matches(source_producer, &source) {
                return Err(DocError::Invalid("shell original source producer missing".into()));
            }
            let certificate = FaceCertificate::ShellPlanar { source_producer, source: Box::new(source), inner: *inner };
            certificate.validate()?;
            return Ok(Some(certificate));
        }
        if let solidcraft_kernel::FaceSource::Imported { item, face, payload_sha256: Some(digest), producer: Some(producer) } = source {
            if !self.feature(*producer).is_some_and(|feature| matches!(feature.kind, FeatureKind::Import { .. })) {
                return Err(DocError::Invalid("import source producer missing".into()));
            }
            let certificate = FaceCertificate::Imported { payload_sha256: digest.clone(), item: *item, face: *face };
            certificate.validate()?;
            return Ok(Some(certificate));
        }
        let certificate = match source {
            solidcraft_kernel::FaceSource::CertifiedExtrudeSide { producer, sketch, curve, .. } => {
                if !self.feature(*producer).is_some_and(|feature| matches!(feature.kind, FeatureKind::Extrude { .. })) {
                    return Err(DocError::Invalid("side source producer missing".into()));
                }
                FaceCertificate::ExtrudeSide { sketch: *sketch, curve: curve.clone() }
            }
            _ => return Ok(None),
        };
        certificate.validate()?;
        Ok(Some(certificate))
    }
    pub fn strong_face_target(&self, body: &ModelBody, index: usize, point: solidcraft_geom::Vec3) -> Result<crate::config::ConfigFaceTarget> {
        let mut target = self.configuration_face_target(body, index, point)?;
        match body.body.face_sources().and_then(|sources| sources.get(index)).and_then(Option::as_ref) {
            Some(solidcraft_kernel::FaceSource::Box { producer: Some(producer), axis, high, .. }) => {
                target.origin_feature = *producer;
                let axis_name = ["x", "y", "z"].get(*axis as usize).ok_or_else(|| DocError::Invalid("invalid box face axis".into()))?;
                target.name = format!("F{producer}:{}{axis_name}", if *high { "+" } else { "-" });
                target.certificate = Some(FaceCertificate::Box { axis: *axis, high: *high });
            }
            Some(solidcraft_kernel::FaceSource::CylinderCap { producer, high, .. }) => {
                target.origin_feature = *producer;
                target.name = format!("F{producer}:{}", cylinder_cap_role(*high));
                target.certificate = Some(FaceCertificate::CylinderCap { high: *high });
            }
            Some(solidcraft_kernel::FaceSource::CertifiedExtrudeCap { producer, sketch, high, outer, holes, .. }) => {
                target.origin_feature = *producer;
                target.name = format!("F{producer}:cap:{}:{}", if *high { "high" } else { "low" }, cap_key(outer, holes));
                target.certificate = Some(FaceCertificate::ExtrudeCap { sketch: *sketch, high: *high, outer: outer.clone(), holes: holes.clone() });
            }
            _ => {}
        }
        self.certify_config_face(&target)?;
        Ok(target)
    }
}

/// The frame a [`FaceCertificate::Lineage`] face's plane is given, from the plane alone (never
/// from how a kernel parametrizes the surface, so it survives rebuilds and kernel changes): its
/// origin where the body's definition origin projects onto the plane, its x axis the first of
/// the definition's X, Y and Z axes that is not steep to the plane (`|cos| < 0.6`; one always
/// is), projected onto it. A face that moves along its normal keeps the frame where it was;
/// one that moves within its plane (a Move feature) keeps it too, as a sketch on a face does
/// in the definition's coordinates.
pub fn lineage_frame(body: &ModelBody, plane: &solidcraft_geom::Plane) -> Option<solidcraft_geom::Plane> {
    use solidcraft_geom::Vec3;
    let n = plane.normal();
    let m = &body.source_transform;
    let origin = crate::apply_point(m, Vec3::ZERO);
    let axis = [Vec3::X, Vec3::Y, Vec3::Z].into_iter().filter_map(|a| crate::apply_vector(m, a).normalized()).find(|a| a.dot(n).abs() < 0.6)?;
    let x = axis - n * axis.dot(n);
    let origin = origin - n * (origin - plane.origin).dot(n);
    solidcraft_geom::Plane::new(origin, x, n.cross(x))
}

/// Whether Split Face split `body`'s faces on `plane` (a split line lies on it).
fn split_on_purpose(body: &ModelBody, plane: &solidcraft_geom::Plane) -> bool {
    let tolerance = 1e-6 * (1. + body.body.size());
    let on_plane =
        |line: &Vec<solidcraft_geom::Vec3>| !line.is_empty() && line.iter().all(|p| (*p - plane.origin).dot(plane.normal()).abs() <= tolerance);
    body.body.split_lines().is_some_and(|lines| lines.iter().any(on_plane))
}

/// Whether two exact planes are one oriented plane (pieces of one face).
fn same_oriented_plane(a: &solidcraft_geom::Plane, b: &solidcraft_geom::Plane, scale: f64) -> bool {
    a.normal().dot(b.normal()) >= 1. - 1e-8 && (b.origin - a.origin).dot(a.normal()).abs() <= 1e-7 * (1. + scale)
}

impl Document {
    /// The exact plane an anchor on face `index` of `body` is measured in: the construction
    /// certificate's plane, or for a [`FaceCertificate::Lineage`] face its current exact plane in
    /// the [`lineage_frame`].
    pub fn anchor_face_plane(&self, body: &ModelBody, index: usize, certificate: Option<&FaceCertificate>) -> Option<solidcraft_geom::Plane> {
        match certificate {
            Some(FaceCertificate::Lineage) => body.body.planar_face(index).and_then(|plane| lineage_frame(body, &plane)),
            _ => body.body.face_plane(index),
        }
    }

    /// A target for flat face `index` of `body` by its exact history name
    /// ([`FaceCertificate::Lineage`]), when that name follows exact history, names a feature of
    /// the design and finds this face (with its other pieces, all on one oriented plane). Where
    /// the face's pieces no longer share a plane (Offset Face moved one of them), the piece by
    /// its own name: the face's name qualified by the faces around it (`F2:top#[F2:side:l4,…]`).
    pub(crate) fn lineage_face_target(
        &self,
        body: &ModelBody,
        index: usize,
        point: solidcraft_geom::Vec3,
    ) -> Option<crate::config::ConfigFaceTarget> {
        let names = crate::naming::names(body);
        if !names.verified(index) {
            return None;
        }
        let full = names.faces.get(index)?;
        let stem = crate::naming::strip_piece(full);
        let origin = stem.strip_prefix('F')?.split_once(':')?.0.parse::<u64>().ok()?;
        let inverse = crate::mat_inverse(&body.source_transform)?;
        let mut candidates = vec![stem];
        if full != stem {
            candidates.push(full);
        }
        candidates.into_iter().find_map(|name| {
            let target = crate::config::ConfigFaceTarget {
                body: body.definition_name().into(),
                origin_feature: origin,
                name: name.to_string(),
                point: crate::apply_point(&inverse, point),
                certificate: Some(FaceCertificate::Lineage),
            };
            self.configuration_face_indices(body, &target).ok()?.contains(&index).then_some(target)
        })
    }

    /// The faces of `body` an exact history name finds: those named `name` or a piece of it, else
    /// (a later join merged it into another face) the face that answers to it. Every one must
    /// follow exact history and lie on one oriented plane.
    pub(crate) fn lineage_face_indices(&self, body: &ModelBody, name: &str) -> Result<Vec<usize>> {
        let names = crate::naming::names(body);
        // `face` is `name` itself or a piece of it (a piece of a piece, …).
        let piece_of = |face: &str| {
            let mut n = face;
            loop {
                if n == name {
                    return true;
                }
                match crate::piece_names::parent_piece(n) {
                    Some(parent) => n = parent,
                    None => return false,
                }
            }
        };
        let mut indices: Vec<usize> = names.faces.iter().enumerate().filter(|(_, face)| piece_of(face)).map(|(index, _)| index).collect();
        if indices.is_empty() {
            indices =
                names.info.iter().enumerate().filter(|(_, info)| info.aliases.iter().any(|alias| alias == name)).map(|(index, _)| index).collect();
        }
        if indices.is_empty() {
            return Err(DocError::Unknown(format!("face `{name}` on `{}` no longer exists", body.name)));
        }
        self.coplanar_pieces(body, &indices, |index| names.verified(index))?;
        // A piece by its own name stands for itself, which a piece of a face split on purpose
        // (Split Face) never does: that needs a fixed plane.
        if crate::naming::strip_piece(name) != name
            && let Some(plane) = indices.first().and_then(|index| body.body.planar_face(*index))
            && split_on_purpose(body, &plane)
        {
            return Err(DocError::Invalid(
                "this face was split on purpose (Split Face): pick an exact face piece's own plane with fixed:true".into(),
            ));
        }
        Ok(indices)
    }

    /// Check that `indices` are pieces of one flat face: each `exact` and planar, all on one
    /// oriented plane, and not split on purpose (Split Face: its pieces are separate faces, so
    /// a pick on one never stands for the whole; docs/face-attachments.md).
    pub(crate) fn coplanar_pieces(&self, body: &ModelBody, indices: &[usize], exact: impl Fn(usize) -> bool) -> Result<()> {
        let scale = body.body.size();
        let mut first = None;
        for index in indices {
            if !exact(*index) {
                return Err(DocError::Invalid("this face's history is not exact".into()));
            }
            let plane = body.body.planar_face(*index).ok_or_else(|| DocError::Invalid("attached face is not flat".into()))?;
            match &first {
                None => first = Some(plane),
                Some(first) if same_oriented_plane(first, &plane, scale) => {}
                Some(_) => return Err(DocError::Invalid("attached face pieces no longer share one oriented plane".into())),
            }
        }
        if let (Some(plane), true) = (first, indices.len() > 1)
            && split_on_purpose(body, &plane)
        {
            return Err(DocError::Invalid(
                "this face was split on purpose (Split Face): pick an exact face piece's own plane with fixed:true".into(),
            ));
        }
        Ok(())
    }

    /// Of several faces carrying one construction certificate of feature `origin`, the pieces of
    /// that feature's own face: copies a pattern or mirror made carry their seed's certificate
    /// but are named as copies (`…@F<pattern>.<instance>`, docs/face-attachments.md), and pieces
    /// a later feature split the face into share its exact name and plane.
    pub(crate) fn certified_pieces(&self, body: &ModelBody, indices: Vec<usize>, origin: u64) -> Result<Vec<usize>> {
        if indices.len() < 2 {
            return Ok(indices);
        }
        let names = crate::naming::names(body);
        let own = format!("F{origin}:");
        let stem = |index: usize| names.faces.get(index).map(|name| crate::naming::strip_piece(name));
        let seed: Vec<usize> =
            indices.into_iter().filter(|index| stem(*index).is_some_and(|name| name.starts_with(&own) && !name.contains('@'))).collect();
        let ambiguous = || DocError::Unknown("attached planar face certificate is missing or ambiguous".into());
        let first = seed.first().and_then(|index| stem(*index)).ok_or_else(ambiguous)?;
        if seed.iter().any(|index| stem(*index) != Some(first)) {
            return Err(ambiguous());
        }
        // The certificate is exact; the names only tell the seed from its copies here.
        self.coplanar_pieces(body, &seed, |_| true).map_err(|_| ambiguous())?;
        Ok(seed)
    }

    /// Of several faces carrying one certificate of feature `origin`, those that are not a
    /// pattern's or mirror's copies (named `…@F<pattern>.<instance>`), when any is not.
    pub(crate) fn without_copies(&self, body: &ModelBody, indices: Vec<usize>) -> Vec<usize> {
        if indices.len() < 2 {
            return indices;
        }
        let names = crate::naming::names(body);
        let copy = |index: &usize| names.faces.get(*index).is_some_and(|name| crate::naming::strip_piece(name).contains('@'));
        if indices.iter().all(copy) {
            return indices;
        }
        indices.into_iter().filter(|index| !copy(index)).collect()
    }
}

pub(crate) fn payload_digest(payload: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(payload.as_bytes()))
}

#[cfg(test)]
#[path = "config_shell_tests.rs"]
mod shell_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn solved(mut sketch: solidcraft_sketch::Sketch) -> SolvedSketch {
        let report = solidcraft_sketch::solve(&mut sketch);
        SolvedSketch {
            feature: 1,
            name: "sources".into(),
            plane: solidcraft_geom::Plane::XY,
            sketch,
            report,
            profiles: Vec::new(),
            ends: Default::default(),
        }
    }

    #[test]
    fn cylinder_cap_certificate_json_is_exact() {
        for high in [true, false] {
            let valid = json!({"kind":"cylinder_cap","high":high});
            assert!(validate_certificate_json(&valid).is_ok());
            let certificate: FaceCertificate = serde_json::from_value(valid.clone()).unwrap();
            assert_eq!(certificate, FaceCertificate::CylinderCap { high });
            assert_eq!(serde_json::to_value(&certificate).unwrap(), valid);
        }
        let shelled = json!({"kind":"shell_planar","source_producer":3,"source":{"kind":"cylinder_cap","high":false},"inner":true});
        assert!(validate_certificate_json(&shelled).is_ok());
        let certificate: FaceCertificate = serde_json::from_value(shelled).unwrap();
        assert_eq!(shell_certificate_role(&FaceCertificate::CylinderCap { high: false }, true).unwrap(), "shell:inner:cylinder:bottom");
        assert!(certificate.validate().is_ok());
        for bad in [
            json!({"kind":"cylinder_cap"}),
            json!({"kind":"cylinder_cap","high":1}),
            json!({"kind":"cylinder_cap","high":null}),
            json!({"kind":"cylinder_cap","high":true,"axis":2}),
            json!({"kind":"cylinder_cap","high":true,"extra":[[[]]]}),
        ] {
            assert!(validate_certificate_json(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn prepared_sources_keep_exact_identity_and_reject_ambiguity() {
        use solidcraft_geom::Vec2;
        let mut sketch = solidcraft_sketch::Sketch::new();
        sketch.add_line(Vec2::ZERO, Vec2::new(10., 0.), None, None, Some("line")).unwrap();
        sketch.add_circle(Vec2::new(20., 0.), 2., None, Some("circle")).unwrap();
        let construction = sketch.add_line(Vec2::ZERO, Vec2::new(10., 0.), None, None, Some("construction")).unwrap();
        sketch.curves[construction].construction = true;
        let sketch = solved(sketch);
        let curves = ExactSketchCurves::new(&sketch);
        let partial = Seg2::Line { a: Vec2::new(8., 0.), b: Vec2::new(2., 0.) };
        assert_eq!(curves.find(&partial).as_deref(), Some("line"));
        assert_eq!(curves.find(&Seg2::Arc { center: Vec2::new(20., 0.), radius: 2., start: 1., sweep: -2. }).as_deref(), Some("circle"));
        assert_eq!(curves.find(&Seg2::Line { a: Vec2::ZERO, b: Vec2::new(11., 0.) }), None);
        let mut ambiguous = sketch.clone();
        ambiguous.sketch.add_line(Vec2::new(1., 0.), Vec2::new(9., 0.), None, None, Some("overlap")).unwrap();
        assert_eq!(ExactSketchCurves::new(&ambiguous).find(&partial), None);
        // A prepared pass borrows one immutable evaluated sketch, so an upstream edit
        // necessarily prepares fresh sources rather than retaining stale coordinates.
        let mut moved = sketch.clone();
        moved.sketch = solidcraft_sketch::Sketch::new();
        moved.sketch.add_line(Vec2::new(0., 1.), Vec2::new(10., 1.), None, None, Some("line")).unwrap();
        assert_eq!(ExactSketchCurves::new(&moved).find(&partial), None);
    }

    #[test]
    fn a_curve_drawn_over_a_projected_boundary_is_the_source() {
        use solidcraft_geom::Vec2;
        // A face sketch's live boundary, with a profile edge traced over it (issue #489).
        let mut sketch = solidcraft_sketch::Sketch::new();
        let projected = sketch.add_line(Vec2::ZERO, Vec2::new(10., 0.), None, None, Some("edge")).unwrap();
        sketch.curves[projected].link = Some("boundary".into());
        let sketch = solved(sketch);
        let edge = Seg2::Line { a: Vec2::new(10., 0.), b: Vec2::ZERO };
        assert_eq!(ExactSketchCurves::new(&sketch).find(&edge).as_deref(), Some("edge"), "a lone projection is its own source");
        let mut traced = sketch.clone();
        traced.sketch.add_line(Vec2::ZERO, Vec2::new(10., 0.), None, None, Some("drawn")).unwrap();
        assert_eq!(ExactSketchCurves::new(&traced).find(&edge).as_deref(), Some("drawn"));
        // Two drawn curves over it, or two projections, stay ambiguous.
        let mut twice = traced.clone();
        twice.sketch.add_line(Vec2::ZERO, Vec2::new(10., 0.), None, None, Some("again")).unwrap();
        assert_eq!(ExactSketchCurves::new(&twice).find(&edge), None);
        let mut projections = sketch.clone();
        let other = projections.sketch.add_line(Vec2::ZERO, Vec2::new(10., 0.), None, None, Some("other")).unwrap();
        projections.sketch.curves[other].link = Some("edge link".into());
        assert_eq!(ExactSketchCurves::new(&projections).find(&edge), None);
    }

    #[test]
    #[ignore = "manual optimized certification microbenchmark; no timing assertion"]
    fn benchmark_prepared_extrude_sources() {
        use solidcraft_geom::Vec2;
        let mut sketch = solidcraft_sketch::Sketch::new();
        for i in 0..1000 {
            let y = i as f64;
            sketch.add_line(Vec2::new(0., y), Vec2::new(10., y), None, None, None).unwrap();
        }
        let sketch = solved(sketch);
        let queries: Vec<_> = (0..1000).map(|i| Seg2::Line { a: Vec2::new(2., i as f64), b: Vec2::new(8., i as f64) }).collect();
        let start = std::time::Instant::now();
        let old: Vec<_> = queries
            .iter()
            .map(|generated| {
                let mut found = None;
                for (index, curve) in sketch.sketch.curves.iter().enumerate() {
                    if sketch.sketch.segs(index).iter().any(|original| exact_segment_contains(original, generated)) {
                        if found.is_some() {
                            return None;
                        }
                        found = Some(curve.id.clone());
                    }
                }
                found
            })
            .collect();
        let old_ms = start.elapsed().as_secs_f64() * 1000.;
        let start = std::time::Instant::now();
        let curves = ExactSketchCurves::new(&sketch);
        let new: Vec<_> = queries.iter().map(|query| curves.find(query)).collect();
        let new_ms = start.elapsed().as_secs_f64() * 1000.;
        assert_eq!(old, new);
        assert!(new.iter().all(Option::is_some));
        eprintln!("1000 analytic sources / 1000 face segments: repeated={old_ms:.3}ms prepared={new_ms:.3}ms (includes preparation)");
    }
    #[test]
    fn certificate_budgets_apply_before_saved_target_and_paint_decode() {
        for certificate in [
            json!({"kind":"extrude_side","sketch":1,"curve":"x".repeat(1_000_000)}),
            json!({"kind":"imported","payload_sha256":"0".repeat(64),"item":0,"face":1}),
            json!({"kind":"imported","payload_sha256":"g".repeat(64),"item":1,"face":1}),
            json!({"kind":"extrude_side","sketch":1,"curve":"c1","extra":[[[[[]]]]]}),
        ] {
            assert!(validate_certificate_json(&certificate).is_err());
            let target = json!({"configs":{"columns":[{"face_appearance":{"body":"B","origin_feature":1,"name":"F1:side:c1","point":[0,0,0],"certificate":certificate}}]}});
            assert!(crate::config::validate_styles_json(&target).is_err());
            let paint = json!({"appearances":{"faces":[{"certificate":certificate}]}});
            assert!(crate::config::validate_styles_json(&paint).is_err());
        }
        assert!(validate_certificate_json(&json!({"kind":"imported","payload_sha256":"a".repeat(64),"item":1,"face":2})).is_ok());
        assert!(validate_certificate_json(&json!({"kind":"extrude_side","sketch":1,"curve":"c1"})).is_ok());
    }
    #[test]
    fn analytic_arc_interval_refuses_unsampled_gaps_and_accepts_wrap_reversal() {
        let radians = |degrees: f64| degrees.to_radians();
        let arc = |start, sweep| Seg2::Arc { center: solidcraft_geom::Vec2::default(), radius: 10., start: radians(start), sweep: radians(sweep) };
        assert!(!exact_segment_contains(&arc(10., 350.), &arc(20., 360.)), "five point probes missed the missing ten-degree gap");
        assert!(!exact_segment_contains(&arc(10., 350.), &arc(20., -360.)));
        assert!(exact_segment_contains(&arc(350., 40.), &arc(355., 20.)));
        assert!(exact_segment_contains(&arc(350., 40.), &arc(15., -20.)));
        assert!(exact_segment_contains(&arc(30., -40.), &arc(15., -20.)));
        assert!(!exact_segment_contains(&arc(350., 40.), &arc(345., 20.)));
        assert!(exact_segment_contains(&arc(0., 360.), &arc(20., -360.)));
    }
}
