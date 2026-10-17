//! Evaluation of the surface features: Patch, Stitch, Thicken, Trim, Extend, Surface Extrude,
//! Revolve, Sweep and Loft, Offset, Ruled, Untrim, Unstitch and Reverse Normal (kernel `surfaces`,
//! `surface_offset`, `surface_ruled`, `surface_untrim`).

use super::*;
use crate::{RuledCorners, RuledMode, SheetSection};
use solidcraft_geom::Seg2;

pub(super) fn eval(doc: &Document, vals: &BTreeMap<String, Value>, f: &Feature, st: &mut ModelState, warning: &mut Option<String>) -> Result<()> {
    match &f.kind {
        FeatureKind::Patch { sketch, profiles, body, edges, crown, continuity, points } => {
            let mut made = Vec::new();
            if let Some(sk) = sketch {
                let ss = st.sketch(sk.0).ok_or_else(|| DocError::Unknown(format!("sketch {sk} (it must come earlier in the timeline)")))?;
                let regions = solidcraft_sketch::merge_regions(&select_profiles(ss, profiles)?);
                for r in &regions {
                    made.push(kernel::patch_region(&ss.plane, r)?);
                }
            }
            if !edges.is_empty() {
                let i = body_at(st, body, &crate::plain(edges))?;
                let b = st.bodies.get(i).ok_or_else(|| DocError::Invalid("body".into()))?;
                let spec = kernel::PatchSpec {
                    continuity: continuity.all,
                    edges: continuity.edges.iter().map(|e| (e.edge.0, e.g)).collect(),
                    points: crate::plain(points),
                    crown: *crown,
                };
                made.push(kernel::patch_edges_with(&b.body, &crate::plain(edges), &spec)?);
            }
            if made.is_empty() {
                return Err(DocError::Invalid("a patch needs a sketch region or a loop of edges".into()));
            }
            for (k, b) in made.into_iter().enumerate() {
                let name = new_name(st, f, k);
                st.bodies.push(ModelBody::new(name, b, f.id));
            }
            Ok(())
        }
        FeatureKind::Stitch { bodies, tolerance } => {
            let tol = val(vals, tolerance, Kind::Length)?;
            let idx: Vec<usize> = bodies
                .iter()
                .map(|n| st.bodies.iter().position(|b| &b.name == n).ok_or_else(|| DocError::Unknown(format!("body `{n}`"))))
                .collect::<Result<_>>()?;
            let Some(&first) = idx.first() else { return Err(DocError::Invalid("pick the surfaces to stitch".into())) };
            let refs: Vec<&kernel::Body> = idx.iter().filter_map(|i| st.bodies.get(*i).map(|b| &b.body)).collect();
            let sewn = kernel::stitch(&refs, tol)?;
            let Some(mb) = st.bodies.get(first).cloned() else { return Err(DocError::Invalid("body".into())) };
            if let Some(slot) = st.bodies.get_mut(first) {
                *slot = ModelBody::new(mb.name, sewn, mb.feature);
            }
            // The others are now part of the first.
            let gone: Vec<String> = idx.iter().skip(1).filter_map(|i| st.bodies.get(*i).map(|b| b.name.clone())).collect();
            st.bodies.retain(|b| !gone.contains(&b.name));
            Ok(())
        }
        FeatureKind::Thicken { bodies, thickness, symmetric, operation, targets } => {
            let t = val(vals, thickness, Kind::Length)?;
            let mut tools = Vec::new();
            for n in bodies {
                let b = st.body(n).ok_or_else(|| DocError::Unknown(format!("body `{n}`")))?;
                tools.push(kernel::thicken(&b.body, t, *symmetric)?);
            }
            if tools.is_empty() {
                return Err(DocError::Invalid("pick the surfaces to thicken".into()));
            }
            apply_op(st, f, tools, *operation, &crate::plain(targets))
        }
        FeatureKind::SurfaceTrim { body, plane, tool, keep } => {
            let i = st.bodies.iter().position(|b| &b.name == body).ok_or_else(|| DocError::Unknown(format!("body `{body}`")))?;
            let Some(mb) = st.bodies.get(i).cloned() else { return Err(DocError::Invalid("body".into())) };
            let trimmed = match tool {
                Some(t) => {
                    let tb = st.body(&t.body).ok_or_else(|| DocError::Unknown(format!("body `{}`", t.body)))?;
                    let face = crate::appearance::face_index_at(tb, t.point.0)
                        .ok_or_else(|| DocError::Invalid(format!("no face of `{}` at the tool point", t.body)))?;
                    kernel::trim(&mb.body, &kernel::SplitTool::Face { body: &tb.body, face }, keep.0)?
                }
                None => kernel::trim(&mb.body, &kernel::SplitTool::Plane(doc.resolve_plane_in(vals, plane, 0, Some(st))?), keep.0)?,
            };
            if let Some(slot) = st.bodies.get_mut(i) {
                *slot = ModelBody::new(mb.name, trimmed, mb.feature);
            }
            Ok(())
        }
        FeatureKind::SurfaceExtend { body, edges, distance, mode, up_to, neighbours } => {
            let i = st.bodies.iter().position(|b| &b.name == body).ok_or_else(|| DocError::Unknown(format!("body `{body}`")))?;
            let Some(mb) = st.bodies.get(i).cloned() else { return Err(DocError::Invalid("body".into())) };
            let mode = match mode {
                crate::ExtendMode::Natural => kernel::ExtendMode::Natural,
                crate::ExtendMode::Tangent => kernel::ExtendMode::Tangent,
                crate::ExtendMode::Curvature => kernel::ExtendMode::Curvature,
            };
            let at = |limit| kernel::ExtendOptions { mode, limit, neighbours: *neighbours };
            let out = match up_to {
                None => {
                    kernel::extend_surface(&mb.body, &crate::plain(edges), &at(kernel::ExtendLimit::Distance(val(vals, distance, Kind::Length)?)))?
                }
                Some(crate::ExtendTo::Bounds) => kernel::extend_surface(&mb.body, &crate::plain(edges), &at(kernel::ExtendLimit::Bounds))?,
                Some(crate::ExtendTo::Plane(plane)) => {
                    let tool = kernel::SplitTool::Plane(doc.resolve_plane_in(vals, plane, 0, Some(st))?);
                    kernel::extend_surface(&mb.body, &crate::plain(edges), &at(kernel::ExtendLimit::UpTo(&tool)))?
                }
                Some(crate::ExtendTo::Face(t)) => {
                    let tb = st.body(&t.body).ok_or_else(|| DocError::Unknown(format!("body `{}`", t.body)))?;
                    let face = crate::appearance::face_index_at(tb, t.point.0)
                        .ok_or_else(|| DocError::Invalid(format!("no face of `{}` at the target point", t.body)))?;
                    let tool = kernel::SplitTool::Face { body: &tb.body, face };
                    kernel::extend_surface(&mb.body, &crate::plain(edges), &at(kernel::ExtendLimit::UpTo(&tool)))?
                }
            };
            if let Some(slot) = st.bodies.get_mut(i) {
                *slot = ModelBody::new(mb.name, out, mb.feature);
            }
            Ok(())
        }
        FeatureKind::SurfaceBlend { from, to, continuity, bulge, align, stitch } => {
            let a = st.body(&from.body).ok_or_else(|| DocError::Unknown(format!("body `{}`", from.body)))?.body.clone();
            let b = st.body(&to.body).ok_or_else(|| DocError::Unknown(format!("body `{}`", to.body)))?.body.clone();
            if from.edges.is_empty() || to.edges.is_empty() {
                return Err(DocError::Invalid("a blend needs a chain of edges at each end".into()));
            }
            let (made, gap) = kernel::blend_between(
                &a,
                &crate::plain(&from.edges),
                &b,
                &crate::plain(&to.edges),
                *continuity,
                *bulge,
                align.map(|[p, q]| [p.0, q.0]),
                BLEND_STATIONS,
                *stitch,
            )?;
            if gap > 1e-9 {
                *warning = Some(format!("the blend's boundary lies up to {gap:.4} mm from the edges it was built on"));
            }
            let name = new_name(st, f, 0);
            // A stitched blend takes the place of both bodies it joins; otherwise it is a new
            // surface body of its own.
            if *stitch {
                let gone = [from.body.0.clone(), to.body.0.clone()];
                st.bodies.retain(|x| !gone.contains(&x.name));
            }
            st.bodies.push(ModelBody::new(name, made, f.id));
            Ok(())
        }
        FeatureKind::SurfaceBoundary { sketch, curves, orders, crown, samples } => {
            let ss = sketch_in_feature_frame(
                doc,
                st.sketch(sketch.0).ok_or_else(|| DocError::Unknown(format!("sketch {sketch} (it must come earlier in the timeline)")))?,
                f.component,
            )?;
            let boundaries = boundary_frames(&ss, curves, orders, *samples)?;
            let body = kernel::boundary_patch(&boundaries, *crown, *samples)?;
            let name = new_name(st, f, 0);
            st.bodies.push(ModelBody::new(name, body, f.id));
            Ok(())
        }
        FeatureKind::SurfaceMatch { body, faces, target, edges, order } => {
            let i = st.bodies.iter().position(|b| &b.name == body).ok_or_else(|| DocError::Unknown(format!("body `{body}`")))?;
            let Some(mb) = st.bodies.get(i).cloned() else { return Err(DocError::Invalid("body".into())) };
            let tb = st.body(target).ok_or_else(|| DocError::Unknown(format!("body `{target}`")))?.body.clone();
            if edges.is_empty() {
                return Err(DocError::Invalid("a match needs a chain of the target's edges".into()));
            }
            // Picked by a point on the face; without picks, the body's first face.
            let picked: Vec<solidcraft_geom::Vec3> = if faces.is_empty() {
                let faces = mb.body.faces(mb.body.size() * 1e-2)?;
                vec![faces.first().ok_or_else(|| DocError::Invalid(format!("`{body}` has no face to match")))?.centroid]
            } else {
                crate::plain(faces)
            };
            let made = kernel::match_surface(&mb.body, &picked, &tb, &crate::plain(edges), *order)?;
            // A match rebuilds the face in place: the body keeps its name, and whatever meets it
            // downstream still finds it.
            if let Some(slot) = st.bodies.get_mut(i) {
                *slot = ModelBody::new(mb.name, made, mb.feature);
            }
            Ok(())
        }
        FeatureKind::SurfaceFair { body, faces, strength, iterations, keep_edges, tolerance } => {
            let tol = val(vals, tolerance, Kind::Length)?;
            let opts = kernel::FairOptions::new(*strength, *iterations as usize, *keep_edges, tol)?;
            let i = st.bodies.iter().position(|b| &b.name == body).ok_or_else(|| DocError::Unknown(format!("body `{body}`")))?;
            let Some(mb) = st.bodies.get(i).cloned() else { return Err(DocError::Invalid("body".into())) };
            // Picked by a point on the face; without picks, every face of the body.
            let mut picked: Vec<usize> = Vec::new();
            if faces.is_empty() {
                picked.extend(0..mb.body.face_count());
            } else {
                for p in faces {
                    let fi =
                        crate::appearance::face_index_at(&mb, p.0).ok_or_else(|| DocError::Invalid(format!("Fair: no face of {body} at {p:?}")))?;
                    if !picked.contains(&fi) {
                        picked.push(fi);
                    }
                }
            }
            if picked.is_empty() {
                return Err(DocError::Invalid("the body has no faces to fair".into()));
            }
            // The face keeps every edge it had: only the surface between them is faired. The
            // energy before and after goes into the feature's warning, so the reply can say how
            // much the face moved.
            let (faired, report) = kernel::fair_faces(&mb.body, &picked, &opts)?;
            *warning = Some(format!(
                "faired {} face(s): energy {:.6} → {:.6} in {} step(s), control points moved up to {:.4} mm",
                picked.len(),
                report.energy_before,
                report.energy_after,
                report.steps,
                report.movement
            ));
            if let Some(slot) = st.bodies.get_mut(i) {
                *slot = ModelBody::new(mb.name, faired, mb.feature);
            }
            Ok(())
        }
        FeatureKind::SurfaceExtrude { sketch, profiles, curves, extent } => {
            let ss = sketch_in_feature_frame(
                doc,
                st.sketch(sketch.0).ok_or_else(|| DocError::Unknown(format!("sketch {sketch} (it must come earlier in the timeline)")))?,
                f.component,
            )?;
            if extent.through_all || extent.taper.is_some() || extent.to.is_some() || extent.from.is_some() {
                return Err(DocError::Invalid("not supported yet: a surface extrude through all, to or from an object, or with a taper".into()));
            }
            let d = val(vals, &extent.distance, Kind::Length)?;
            let off = extent.start_offset.as_deref().map(|e| val(vals, e, Kind::Length)).transpose()?.unwrap_or(0.0);
            let (a, b) = extent_sides(vals, extent, d, extent.direction)?;
            let chains = sheet_chains(&ss, profiles.as_ref(), curves)?;
            let body = kernel::extrude_sheet(&ss.plane, &chains, a + off, b + off)?;
            push_sheet(st, f, crate::lineage::ChainRoles { sketch: &ss, chains: &chains }.tool(body));
            Ok(())
        }
        FeatureKind::SurfaceRevolve { sketch, profiles, curves, axis, angle, angle2, symmetric } => {
            let ss = sketch_in_feature_frame(
                doc,
                st.sketch(sketch.0).ok_or_else(|| DocError::Unknown(format!("sketch {sketch} (it must come earlier in the timeline)")))?,
                f.component,
            )?;
            let (o, d) = revolve_axis(&ss, axis)?;
            let a = val(vals, angle, Kind::Angle)?;
            let (start, sweep) = match (angle2, symmetric) {
                (Some(a2), _) => {
                    let a2 = val(vals, a2, Kind::Angle)?;
                    (-a2, a + a2)
                }
                (None, true) => (-a / 2.0, a),
                (None, false) => (0.0, a),
            };
            let chains = sheet_chains(&ss, profiles.as_ref(), curves)?;
            let body = kernel::revolve_sheet(&ss.plane, &chains, o, d, start, sweep)?;
            push_sheet(st, f, crate::lineage::ChainRoles { sketch: &ss, chains: &chains }.tool(body));
            Ok(())
        }
        FeatureKind::Unstitch { body, faces } => {
            let i = st.bodies.iter().position(|b| &b.name == body).ok_or_else(|| DocError::Unknown(format!("body `{body}`")))?;
            let Some(mb) = st.bodies.get(i).cloned() else { return Err(DocError::Invalid("body".into())) };
            let (pieces, rest) = kernel::unstitch(&mb.body, &crate::plain(faces))?;
            let source = mb.traced();
            // The faces left keep the body's name; each face taken out is a new surface body.
            match rest {
                Some(rest) => {
                    if let Some(slot) = st.bodies.get_mut(i) {
                        *slot = ModelBody::traced_new(mb.name.clone(), source.then(rest), mb.feature);
                    }
                }
                None => {
                    st.bodies.remove(i);
                }
            }
            for (k, piece) in pieces.into_iter().enumerate() {
                let name = new_name(st, f, k);
                st.bodies.push(ModelBody::traced_new(name, source.then(piece), f.id));
            }
            Ok(())
        }
        FeatureKind::ReverseNormal { body, faces } => {
            let i = st.bodies.iter().position(|b| &b.name == body).ok_or_else(|| DocError::Unknown(format!("body `{body}`")))?;
            let Some(mb) = st.bodies.get(i).cloned() else { return Err(DocError::Invalid("body".into())) };
            let turned = mb.traced().then(kernel::reverse_faces(&mb.body, &crate::plain(faces))?);
            if let Some(slot) = st.bodies.get_mut(i) {
                *slot = ModelBody::traced_new(mb.name, turned, mb.feature);
            }
            Ok(())
        }
        FeatureKind::SurfaceSweep { sketch, profiles, curves, path_sketch, path, orientation, twist, scale, rails, rail_fit } => {
            let ss = st.sketch(sketch.0).ok_or_else(|| DocError::Unknown(format!("sketch {sketch} (it must come earlier in the timeline)")))?;
            let ps =
                st.sketch(path_sketch.0).ok_or_else(|| DocError::Unknown(format!("sketch {path_sketch} (it must come earlier in the timeline)")))?;
            // As Sweep: a 3D path sketch is followed in the feature's frame, the curves with it.
            let profile;
            let ss = if !ps.sketch.wires.is_empty() {
                profile = profile_in_frame(doc, ss, f.component)?;
                &profile
            } else {
                ss
            };
            let mapped = path_in_frame(doc, ps, f.component)?;
            let ps = mapped.as_ref();
            let chains = sheet_chains(ss, profiles.as_ref(), curves)?;
            // The path runs on from the end nearest the middle of the curves.
            let pts: Vec<Vec2> = chains.iter().flatten().flat_map(|s| s.polyline(1e-2)).collect();
            let start = ss.plane.to_world(pts.iter().fold(Vec2::default(), |a, p| a + *p) * (1.0 / pts.len().max(1) as f64));
            let body = match path_points(ps, path, start)? {
                Some(pts) => {
                    let rails = crate::eval::rail_polylines(doc, st, rails, f.component, pts.first().copied().unwrap_or(start))?;
                    let ctl = crate::eval::sweep_control(vals, orientation, twist, scale, rails, *rail_fit)?;
                    kernel::sweep_sheet_points_with(&ss.plane, &chains, &pts, &ctl)?
                }
                None => {
                    let segs = path_segments(ps, path, start)?;
                    let path_start = match segs.first() {
                        Some(kernel::PathSeg::Line { a, .. } | kernel::PathSeg::Arc { a, .. }) => *a,
                        None => start,
                    };
                    let rails = crate::eval::rail_polylines(doc, st, rails, f.component, path_start)?;
                    let ctl = crate::eval::sweep_control(vals, orientation, twist, scale, rails, *rail_fit)?;
                    kernel::sweep_sheet_with(&ss.plane, &chains, &segs, &ctl)?
                }
            };
            let traced = crate::lineage::ChainRoles { sketch: ss, chains: &chains }.tool(body);
            push_sheet(st, f, traced);
            Ok(())
        }
        FeatureKind::SurfaceLoft { sections, continuity, crown } => {
            // A loft with edge chains for sections, or one that has to leave its ends a particular
            // way, is built through `loft_blend` from the frames of every section; the plain one
            // (sketch chains only, both ends G0) is the loft of old.
            if *continuity != [kernel::Continuity::G0, kernel::Continuity::G0] || sections.iter().any(|s| s.body.is_some()) {
                return loft_blend(st, f, sections, *continuity, *crown, doc);
            }
            let mut secs = Vec::with_capacity(sections.len());
            let mut sketches = Vec::with_capacity(sections.len());
            for sec in sections {
                let ss = sketch_in_feature_frame(
                    doc,
                    st.sketch(sec.sketch.0)
                        .ok_or_else(|| DocError::Unknown(format!("sketch {} (it must come earlier in the timeline)", sec.sketch)))?,
                    f.component,
                )?;
                let profiles = match (&sec.profiles, sec.curves.is_empty()) {
                    (None, true) => Some(&ProfileSel::All),
                    (p, _) => p.as_ref(),
                };
                let mut chains = sheet_chains(&ss, profiles, &sec.curves)?.into_iter();
                let (Some(chain), None) = (chains.next(), chains.next()) else {
                    return Err(DocError::Invalid(
                        "each loft section must be one chain: one profile without holes, or curves meeting end to end".into(),
                    ));
                };
                secs.push((ss.plane, chain));
                sketches.push(ss);
            }
            let body = kernel::loft_sheet(&secs)?;
            // Each wall named by the section curves it runs between, as a loft's sides are.
            let loops: Vec<solidcraft_geom::Loop2> = secs.iter().map(|(_, c)| solidcraft_geom::Loop2 { segs: c.clone() }).collect();
            let traced = crate::lineage::SectionRoles { sections: sketches.iter().zip(&loops).collect() }.tool(body);
            push_sheet(st, f, traced);
            Ok(())
        }
        FeatureKind::SurfaceOffset { faces, body, distance, chain, tolerance } => {
            let d = val(vals, distance, Kind::Length)?;
            let tol = match tolerance {
                Some(t) => val(vals, t, Kind::Length)?,
                None => 0.01,
            };
            let i = body_at(st, body, &crate::plain(faces))?;
            let Some(mb) = st.bodies.get(i).cloned() else { return Err(DocError::Invalid("body".into())) };
            let picks = if *chain { smooth_chain(&mb, &crate::plain(faces)) } else { crate::plain(faces) };
            let mut picked: Vec<usize> = Vec::new();
            for p in &picks {
                let fi =
                    crate::appearance::face_index_at(&mb, *p).ok_or_else(|| DocError::Invalid(format!("Offset: no face of {} at {p:?}", mb.name)))?;
                if !picked.contains(&fi) {
                    picked.push(fi);
                }
            }
            let made = Traced::step(kernel::offset_surface(&mb.body, &picked, d, tol)?, &[&mb.traced()]);
            let name = new_name(st, f, 0);
            st.bodies.push(ModelBody::traced_new(name, made, f.id));
            Ok(())
        }
        FeatureKind::SurfaceRuled { body, edges, distance, mode, angle, direction, flip, corners, face } => {
            let d = val(vals, distance, Kind::Length)?;
            let mode = match mode {
                RuledMode::Normal => kernel::RuledMode::Normal,
                RuledMode::Tangent => kernel::RuledMode::Tangent,
                RuledMode::Angle => {
                    let a = angle.as_ref().ok_or_else(|| DocError::Invalid("a ruled surface at an angle needs `angle`".into()))?;
                    kernel::RuledMode::Angle { angle: val(vals, a, Kind::Angle)?, reference: direction.map(Vec3::from) }
                }
                RuledMode::Direction => kernel::RuledMode::Direction(
                    direction.map(Vec3::from).ok_or_else(|| DocError::Invalid("a ruled surface along a direction needs `direction`".into()))?,
                ),
            };
            let corners = match corners {
                RuledCorners::Mitre => kernel::RuledCorners::Mitre,
                RuledCorners::Round => kernel::RuledCorners::Round,
            };
            let mb = st.body(body).cloned().ok_or_else(|| DocError::Unknown(format!("body `{body}`")))?;
            let r = kernel::Ruled { distance: d, mode, flip: *flip, corners, face: face.map(Vec3::from) };
            let made = Traced::step(kernel::ruled_surface(&mb.body, &crate::plain(edges), &r)?, &[&mb.traced()]);
            let name = new_name(st, f, 0);
            st.bodies.push(ModelBody::traced_new(name, made, f.id));
            Ok(())
        }
        FeatureKind::SurfaceUntrim { face, holes, outer, in_place } => {
            let i = st.bodies.iter().position(|b| b.name == face.body).ok_or_else(|| DocError::Unknown(format!("body `{}`", face.body)))?;
            let Some(mb) = st.bodies.get(i).cloned() else { return Err(DocError::Invalid("body".into())) };
            let fi = crate::appearance::face_index_at(&mb, face.point.0)
                .ok_or_else(|| DocError::Invalid(format!("Untrim: no face of {} at {:?}", mb.name, face.point)))?;
            let which = match holes {
                None => kernel::UntrimHoles::All,
                Some(points) => kernel::UntrimHoles::At(&crate::plain(points)),
            };
            // A surface body's face is untrimmed where it is when asked; a solid's is copied out.
            let replace = *in_place && mb.body.is_surface();
            let made = mb.traced().then(kernel::untrim(&mb.body, fi, which, *outer, replace)?);
            if replace {
                if let Some(slot) = st.bodies.get_mut(i) {
                    *slot = ModelBody::traced_new(mb.name, made, mb.feature);
                }
            } else {
                let name = new_name(st, f, 0);
                st.bodies.push(ModelBody::traced_new(name, made, f.id));
            }
            Ok(())
        }
        _ => Err(DocError::Invalid("not a surface feature".into())),
    }
}

/// A surface builder's body, added to the model with what its faces came from.
fn push_sheet(st: &mut ModelState, f: &Feature, traced: crate::lineage::Traced) {
    let name = new_name(st, f, 0);
    st.bodies.push(ModelBody::traced_new(name, traced, f.id));
}

/// The boundaries `surface.boundary` builds its patch from: each picked curve's own stations in
/// the sketch's frame, every one of them leaving the boundary square into the loop — the direction
/// from the curve towards the middle of the loop, taken across its tangent — over the plane the
/// curve lies in (so a chain of planar curves makes a flat patch, and a network that is not
/// coplanar is followed as the plane through the curve's own tangent). `orders` sets how much of
/// that plane each side repeats.
fn boundary_frames(
    ss: &SolvedSketch,
    curves: &[String],
    orders: &[solidcraft_kernel::Continuity],
    samples: usize,
) -> Result<Vec<solidcraft_kernel::Boundary>> {
    use solidcraft_kernel::{Boundary, Frame};
    // The stations along each curve, evenly in its own arclength, so every side carries the same
    // number of frames and the patch's net is square.
    let mut sides: Vec<Vec<solidcraft_geom::Vec3>> = Vec::with_capacity(curves.len());
    for id in curves {
        let ci = ss.sketch.curve_index(id).ok_or_else(|| DocError::Unknown(format!("sketch curve `{id}`")))?;
        let pieces = ss.sketch.exact_pieces(ci);
        if pieces.is_empty() {
            return Err(DocError::Invalid(format!("curve `{id}` has no exact geometry to build on")));
        }
        let lengths: Vec<f64> = pieces.iter().map(Seg2::length).collect();
        let total: f64 = lengths.iter().sum();
        if !total.is_finite() || total <= 1e-9 {
            return Err(DocError::Invalid(format!("curve `{id}` has no length")));
        }
        let mut pts = Vec::with_capacity(samples);
        let (mut used, mut at) = (0.0, 0usize);
        for k in 0..samples {
            let want = total * k as f64 / (samples.saturating_sub(1)).max(1) as f64;
            while at + 1 < pieces.len() && used + lengths[at] < want {
                used += lengths[at];
                at += 1;
            }
            let (Some(piece), Some(len)) = (pieces.get(at), lengths.get(at)) else { return Err(DocError::Invalid("boundary piece".into())) };
            let t = ((want - used) / len.max(1e-12)).clamp(0.0, 1.0);
            pts.push(ss.plane.to_world(piece.point_at(t)));
        }
        sides.push(pts);
    }
    // The middle of the loop, so every side leaves towards the inside of the patch.
    let count = sides.iter().map(Vec::len).sum::<usize>().max(1) as f64;
    let centre = sides.iter().flatten().fold(solidcraft_geom::Vec3::ZERO, |a, p| a + *p) * (1.0 / count);
    let mut out = Vec::with_capacity(sides.len());
    for (side, pts) in sides.iter().enumerate() {
        let mut frames = Vec::with_capacity(pts.len());
        for (i, p) in pts.iter().enumerate() {
            // The tangent from the stations on either side, one-sided at the ends (where the side
            // only has one neighbour).
            let before = if i == 0 { *p } else { pts[i - 1] };
            let after = if i + 1 == pts.len() { *p } else { pts[i + 1] };
            let tangent = (after - before).normalized().ok_or_else(|| DocError::Invalid("a boundary curve has a stationary station".into()))?;
            let across = centre - *p;
            let leave = (across - tangent * across.dot(tangent))
                .normalized()
                .ok_or_else(|| DocError::Invalid("a boundary runs through the middle of the loop: no patch can leave it".into()))?;
            frames.push(Frame { p: *p, leave, bend: Vec3::ZERO, bend_change: Vec3::ZERO });
        }
        out.push(Boundary { frames, order: orders.get(side).copied().unwrap_or(solidcraft_kernel::Continuity::G1) });
    }
    Ok(out)
}

/// What a surface builder sweeps: the boundary loops of the picked profiles, then the picked
/// curves chained end to end.
fn sheet_chains(ss: &SolvedSketch, profiles: Option<&ProfileSel>, curves: &[String]) -> Result<Vec<Vec<Seg2>>> {
    let mut chains: Vec<Vec<Seg2>> = Vec::new();
    if let Some(sel) = profiles {
        for r in solidcraft_sketch::merge_regions(&select_profiles(ss, sel)?) {
            chains.extend(std::iter::once(r.outer).chain(r.holes).map(|lp| lp.segs));
        }
    }
    chains.extend(curve_chains(ss, curves)?);
    if chains.is_empty() {
        return Err(DocError::Invalid("pick the profiles or curves".into()));
    }
    Ok(chains)
}

/// Most sketch curves one feature chains.
const MAX_CHAIN_CURVES: usize = 1000;
/// Stations a blend samples each of its two boundaries at: enough that the boundary it fits is
/// within a small part of a thousandth of the chain's length of the edges it was built on.
const BLEND_STATIONS: usize = 41;

/// The sketch curves `ids` (free-form ones exactly where they can be) grouped into chains of segments that meet end to end, each oriented
/// to run one way. Curves are joined greedily at their free ends: where three meet, one of them
/// starts a chain of its own.
fn curve_chains(ss: &SolvedSketch, ids: &[String]) -> Result<Vec<Vec<Seg2>>> {
    if ids.len() > MAX_CHAIN_CURVES {
        return Err(DocError::Invalid(format!("at most {MAX_CHAIN_CURVES} curves")));
    }
    let mut pieces: std::collections::VecDeque<Vec<Seg2>> = ids
        .iter()
        .map(|id| {
            let ci = ss.sketch.curve_index(id).ok_or_else(|| DocError::Unknown(format!("curve `{id}`")))?;
            Ok(ss.sketch.exact_segs(ci).unwrap_or_else(|| ss.sketch.segs(ci)))
        })
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect();
    let reversed = |p: Vec<Seg2>| -> Vec<Seg2> { p.iter().rev().map(Seg2::reversed).collect() };
    let near = |p: Vec2, q: Vec2| p.dist(q) <= 1e-6;
    let mut chains = Vec::new();
    while let Some(mut chain) = pieces.pop_front() {
        while let (Some(head), Some(tail)) = (chain.first().map(Seg2::start), chain.last().map(Seg2::end)) {
            if near(head, tail) {
                break;
            }
            let ends = |p: &Vec<Seg2>| p.first().map(Seg2::start).zip(p.last().map(Seg2::end));
            let Some(i) = pieces.iter().position(|p| ends(p).is_some_and(|(s, e)| near(s, tail) || near(e, tail) || near(s, head) || near(e, head)))
            else {
                break;
            };
            let Some(p) = pieces.remove(i) else { break };
            let Some((s, e)) = ends(&p) else { continue };
            if near(s, tail) {
                chain.extend(p);
            } else if near(e, tail) {
                chain.extend(reversed(p));
            } else if near(e, head) {
                chain = p.into_iter().chain(chain).collect();
            } else {
                chain = reversed(p).into_iter().chain(chain).collect();
            }
        }
        chains.push(chain);
    }
    Ok(chains)
}

/// A multi-section blend: every section as frames, the body ones read off the face their edge
/// chain bounds, and the first and last leaving with `continuity` where the caller asks for it.
fn loft_blend(
    st: &mut ModelState,
    f: &Feature,
    sections: &[SheetSection],
    continuity: [kernel::Continuity; 2],
    crown: [f64; 2],
    doc: &Document,
) -> Result<()> {
    /// One section: a chain of sketch curves in its plane, or a body's edge chain.
    enum Piece {
        Sketch(solidcraft_geom::Plane, Vec<solidcraft_geom::Seg2>),
        Body(String, Vec<solidcraft_geom::Vec3>),
    }
    let mut pieces = Vec::with_capacity(sections.len());
    let mut centres = Vec::with_capacity(sections.len());
    for sec in sections {
        if let Some(name) = sec.body.as_deref() {
            if sec.edges.is_empty() {
                return Err(DocError::Invalid("a loft section picked from a body needs a chain of its edges".into()));
            }
            let n = sec.edges.len() as f64;
            centres.push(sec.edges.iter().fold(solidcraft_geom::Vec3::ZERO, |a, p| a + **p) * (1.0 / n));
            pieces.push(Piece::Body(name.to_string(), crate::plain(&sec.edges)));
            continue;
        }
        let ss = sketch_in_feature_frame(
            doc,
            st.sketch(sec.sketch.0).ok_or_else(|| DocError::Unknown(format!("sketch {} (it must come earlier in the timeline)", sec.sketch)))?,
            f.component,
        )?;
        let profiles = match (&sec.profiles, sec.curves.is_empty()) {
            (None, true) => Some(&ProfileSel::All),
            (p, _) => p.as_ref(),
        };
        let mut chains = sheet_chains(&ss, profiles, &sec.curves)?.into_iter();
        let (Some(chain), None) = (chains.next(), chains.next()) else {
            return Err(DocError::Invalid("each loft section must be one chain: one profile without holes, or curves meeting end to end".into()));
        };
        let pts: Vec<Vec2> = chain.iter().flat_map(|s| s.polyline(1e-2)).collect();
        let centre = pts.iter().fold(solidcraft_geom::Vec3::ZERO, |a, p| a + ss.plane.to_world(*p)) * (1.0 / pts.len().max(1) as f64);
        centres.push(centre);
        pieces.push(Piece::Sketch(ss.plane, chain));
    }
    // Each section leaves towards the one after it; the last one leaves back into the blend.
    let stations = BLEND_STATIONS;
    let last = pieces.len().saturating_sub(1);
    let mut frames = Vec::with_capacity(pieces.len());
    for (i, piece) in pieces.iter().enumerate() {
        let (from, to) = if i < last { (i, i + 1) } else { (last, last.saturating_sub(1)) };
        let leave = match (centres.get(from), centres.get(to)) {
            (Some(a), Some(b)) => *b - *a,
            _ => solidcraft_geom::Vec3::ZERO,
        };
        match piece {
            Piece::Body(name, edges) => {
                let b = st.body(name).ok_or_else(|| DocError::Unknown(format!("body `{name}`")))?.body.clone();
                frames.push(kernel::loft_blend::section_frames(&b, edges, leave, stations)?);
            }
            Piece::Sketch(plane, chain) => frames.push(chain_frames(plane, chain, leave, stations)),
        }
    }
    let body = kernel::loft_blend(&frames, continuity, crown, stations)?;
    let name = new_name(st, f, 0);
    st.bodies.push(ModelBody::new(name, body, f.id));
    Ok(())
}

/// The frames of one sketch chain, sampled by arclength at `stations` stations: a sketch has no
/// face behind it, so the surface leaves along the run with no curvature to repeat there.
fn chain_frames(
    plane: &solidcraft_geom::Plane,
    chain: &[solidcraft_geom::Seg2],
    leave: solidcraft_geom::Vec3,
    stations: usize,
) -> Vec<kernel::Frame> {
    let mut pts: Vec<Vec2> = Vec::new();
    for seg in chain {
        for p in seg.polyline(1e-2) {
            if pts.last().is_none_or(|l: &Vec2| (*l - p).len() > 1e-12) {
                pts.push(p);
            }
        }
    }
    if pts.len() < 2 || stations < 2 {
        return Vec::new();
    }
    let total: f64 = pts.windows(2).map(|w| (w[1] - w[0]).len()).sum();
    if !(total > 1e-12) {
        return Vec::new();
    }
    let step = total / (stations - 1) as f64;
    let (mut walked, mut k) = (0.0f64, 0usize);
    let mut out = Vec::with_capacity(stations);
    for i in 0..stations {
        let want = step * i as f64;
        while k + 1 < pts.len() && walked + (pts[k + 1] - pts[k]).len() < want {
            walked += (pts[k + 1] - pts[k]).len();
            k += 1;
        }
        let (Some(a), Some(b)) = (pts.get(k).copied(), pts.get(k + 1).copied()) else {
            if let Some(p) = pts.get(k).copied() {
                out.push(kernel::Frame { p: plane.to_world(p), leave, bend: solidcraft_geom::Vec3::ZERO, bend_change: solidcraft_geom::Vec3::ZERO });
            }
            continue;
        };
        let span = (b - a).len();
        let t = if span > 1e-12 { ((want - walked) / span).clamp(0.0, 1.0) } else { 0.0 };
        let p = a * (1.0 - t) + b * t;
        out.push(kernel::Frame { p: plane.to_world(p), leave, bend: solidcraft_geom::Vec3::ZERO, bend_change: solidcraft_geom::Vec3::ZERO });
    }
    out
}
