//! Exact profile references through analytic offset topology transitions.
use std::collections::BTreeSet;

use crate::{DocError, ProfileSel, Result, SolvedSketch, WholeProfileBoundary};

const MAX_SELECTIONS: usize = 128;

fn whole_boundary(profile: &solidcraft_sketch::Profile) -> Option<WholeProfileBoundary> {
    let (outer, holes) = profile.whole_curve_boundaries()?;
    Some(WholeProfileBoundary { outer, holes })
}

fn validate_whole(profiles: &[WholeProfileBoundary]) -> Result<()> {
    if profiles.is_empty() || profiles.len() > MAX_SELECTIONS {
        return Err(DocError::Invalid("whole-source profile selection needs bounded unique profiles".into()));
    }
    let mut total = 0usize;
    let mut identities = BTreeSet::new();
    for profile in profiles {
        if profile.holes.len() > 128 || !identities.insert((&profile.outer, &profile.holes)) {
            return Err(DocError::Invalid("invalid whole-source profile holes or duplicate identity".into()));
        }
        let mut all = BTreeSet::new();
        for boundary in std::iter::once(&profile.outer).chain(&profile.holes) {
            total += boundary.len();
            if boundary.is_empty() || boundary.len() > 512 || total > 20_000 {
                return Err(DocError::Invalid("whole-source boundary exceeds its identity budget".into()));
            }
            let mut previous = None;
            for curve in boundary {
                if curve.is_empty()
                    || curve.len() > 512
                    || curve.chars().any(char::is_control)
                    || previous.is_some_and(|previous| previous >= curve)
                    || !all.insert(curve)
                {
                    return Err(DocError::Invalid("whole-source boundary needs canonical disjoint curve identities".into()));
                }
                previous = Some(curve);
            }
        }
        if profile.holes.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(DocError::Invalid("whole-source holes need canonical unique identities".into()));
        }
    }
    Ok(())
}

fn whole_profile_indices(ss: &SolvedSketch, profiles: &[WholeProfileBoundary]) -> Result<Vec<usize>> {
    validate_whole(profiles)?;
    let current = ss.profiles.iter().map(whole_boundary).collect::<Vec<_>>();
    profiles
        .iter()
        .map(|identity| {
            let mut matches = current.iter().enumerate().filter(|(_, candidate)| candidate.as_ref() == Some(identity));
            let (index, _) = matches.next().ok_or_else(|| DocError::Unknown(format!("complete source profile in {}", ss.name)))?;
            if matches.next().is_some() {
                return Err(DocError::Invalid(format!("ambiguous complete source profile in {}", ss.name)));
            }
            Ok(index)
        })
        .collect()
}

fn same_ids(a: &[String], b: &[String]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let (mut a, mut b) = (a.iter().collect::<Vec<_>>(), b.iter().collect::<Vec<_>>());
    a.sort();
    b.sort();
    a == b
}

fn unique_profile(ss: &SolvedSketch, curves: &[String]) -> Result<usize> {
    let mut matches = ss.profiles.iter().enumerate().filter(|(_, profile)| same_ids(curves, &profile.outer_curves));
    let (index, _) = matches.next().ok_or_else(|| DocError::Unknown(format!("profile bounded by {curves:?} in {}", ss.name)))?;
    if matches.next().is_some() {
        return Err(DocError::Invalid(format!("ambiguous profile boundary in {}", ss.name)));
    }
    Ok(index)
}

fn reserved_boundaries(ss: &SolvedSketch) -> Result<std::collections::BTreeMap<Vec<String>, String>> {
    let mut found = std::collections::BTreeMap::new();
    for offset in &ss.sketch.offsets {
        let Some(topology) = &offset.topology else { continue };
        let mut reserved = topology.roles.iter().map(|role| role.output.clone()).collect::<Vec<_>>();
        reserved.sort();
        if found.insert(reserved, topology.id.clone()).is_some() {
            return Err(DocError::Invalid("profile role set belongs to multiple offset associations".into()));
        }
    }
    Ok(found)
}

/// Resolve exact selection identities to the solved sketch's current profile indices.
/// Complete reserved role sets follow their association; partial sets never expand.
pub fn profile_selection_indices(ss: &SolvedSketch, selection: &ProfileSel) -> Result<Vec<usize>> {
    let indices = match selection {
        ProfileSel::All => (0..ss.profiles.len()).collect(),
        ProfileSel::Indices { indices } => indices.clone(),
        ProfileSel::Curves { loops } => {
            let boundaries = reserved_boundaries(ss)?;
            let mut indices = Vec::with_capacity(loops.len());
            for curves in loops {
                let mut key = curves.clone();
                key.sort();
                if let Some(id) = boundaries.get(&key) {
                    if curves.len() > 512 || curves.iter().collect::<BTreeSet<_>>().len() != curves.len() {
                        return Err(DocError::Invalid("offset profile has repeated or excessive curve roles".into()));
                    }
                    indices.push(unique_profile(ss, &ss.sketch.offset_boundary(id)?)?);
                } else {
                    // Repeated saved IDs identify exact graph spans. Their multiplicity
                    // must not silently turn into a whole-source certificate on resize.
                    if curves.iter().collect::<BTreeSet<_>>().len() != curves.len()
                        && !ss.profiles.iter().any(|profile| same_ids(curves, &profile.outer_curves))
                    {
                        return Err(crate::curve_pick_error(solidcraft_sketch::CurvePick::Missing, curves, &ss.name));
                    }
                    indices.push(
                        solidcraft_sketch::profile_by_curves(&ss.profiles, curves)
                            .map_err(|error| crate::curve_pick_error(error, curves, &ss.name))?,
                    );
                }
            }
            indices
        }
        ProfileSel::OffsetBoundaries { associations } => {
            if associations.is_empty()
                || associations.len() > MAX_SELECTIONS
                || associations.iter().collect::<BTreeSet<_>>().len() != associations.len()
            {
                return Err(DocError::Invalid("offset profile selection needs unique bounded association identities".into()));
            }
            associations.iter().map(|id| unique_profile(ss, &ss.sketch.offset_boundary(id)?)).collect::<Result<Vec<_>>>()?
        }
        ProfileSel::WholeBoundaries { profiles } => whole_profile_indices(ss, profiles)?,
        ProfileSel::Points { points } => {
            let mut indices = Vec::with_capacity(points.len());
            for point in points {
                let (index, _) = ss
                    .profiles
                    .iter()
                    .enumerate()
                    .filter(|(_, profile)| profile.region.contains(*point))
                    .min_by(|(_, a), (_, b)| a.area.total_cmp(&b.area))
                    .ok_or_else(|| DocError::Unknown(format!("profile at {:?} in {}", [point.x, point.y], ss.name)))?;
                indices.push(index);
            }
            indices
        }
    };
    if indices.iter().any(|index| *index >= ss.profiles.len()) {
        return Err(DocError::Invalid(format!("out-of-range profile selection in {}", ss.name)));
    }
    Ok(indices)
}

/// Persist selected complete offset boundaries by association identity. Mixed selections
/// retain exact ordinary loops and the complete reserved role set of each offset boundary.
pub fn capture_profile_selection(ss: &SolvedSketch, indices: &[usize]) -> Result<ProfileSel> {
    let checked = profile_selection_indices(ss, &ProfileSel::Indices { indices: indices.to_vec() })?;
    if checked.is_empty() || checked.iter().collect::<BTreeSet<_>>().len() != checked.len() {
        return Ok(ProfileSel::Indices { indices: indices.to_vec() });
    }
    // Capture adaptive free-form boundaries only with complete source provenance,
    // including every hole. Ordinary analytic captures keep their historical form.
    let adaptive = checked.iter().any(|index| {
        ss.profiles.get(*index).is_some_and(|profile| {
            std::iter::once(&profile.outer_curves)
                .chain(&profile.hole_curves)
                .any(|curves| curves.iter().collect::<BTreeSet<_>>().len() != curves.len())
        })
    });
    if adaptive {
        let identities = checked.iter().map(|index| ss.profiles.get(*index).and_then(whole_boundary)).collect::<Option<Vec<_>>>();
        if let Some(profiles) = identities {
            // Dynamic analytic offset groups need their own association identities.
            // Do not silently flatten a mixed selection into current active curve IDs.
            let mut dynamic = false;
            for profile in &profiles {
                for boundary in std::iter::once(&profile.outer).chain(&profile.holes) {
                    dynamic |= ss.sketch.offset_boundary_for_curves(boundary)?.is_some();
                }
            }
            if !dynamic && whole_profile_indices(ss, &profiles)? == checked {
                return Ok(ProfileSel::WholeBoundaries { profiles });
            }
        }
    }
    let mut associations = Vec::new();
    let mut loops = Vec::new();
    for index in checked {
        let curves = &ss.profiles.get(index).ok_or_else(|| DocError::Invalid("profile index changed during capture".into()))?.outer_curves;
        if curves.is_empty() {
            return Ok(ProfileSel::Indices { indices: indices.to_vec() });
        }
        // Ordinary free-form loops retain one curve ID per exact span. That
        // historical multiset is valid profile identity, but is not a candidate
        // for the unique reserved-role set of a topology-aware association.
        let association =
            if curves.iter().collect::<BTreeSet<_>>().len() == curves.len() { ss.sketch.offset_boundary_for_curves(curves)? } else { None };
        if let Some(id) = association {
            unique_profile(ss, curves)?;
            let offset = ss
                .sketch
                .offsets
                .iter()
                .filter_map(|offset| offset.topology.as_ref())
                .find(|topology| topology.id == id)
                .ok_or_else(|| DocError::Unknown(format!("offset association {id}")))?;
            loops.push(offset.roles.iter().map(|role| role.output.clone()).collect());
            associations.push(id);
        } else {
            if ss.profiles.iter().filter(|profile| same_ids(curves, &profile.outer_curves)).count() != 1 {
                return Ok(ProfileSel::Indices { indices: indices.to_vec() });
            }
            loops.push(curves.clone());
        }
    }
    if associations.len() > MAX_SELECTIONS {
        return Err(DocError::Invalid("too many offset association profiles".into()));
    }
    if associations.len() == loops.len() { Ok(ProfileSel::OffsetBoundaries { associations }) } else { Ok(ProfileSel::Curves { loops }) }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]
    use super::*;
    use solidcraft_geom::{Plane, Vec2};
    use solidcraft_sketch::{Sketch, find_profiles, solve};

    fn rounded_source(sketch: &mut Sketch, radius: f64) -> Vec<(String, bool)> {
        let endpoints = [
            Vec2::new(-37., -32. - radius),
            Vec2::new(37., -32. - radius),
            Vec2::new(37. + radius, -32.),
            Vec2::new(37. + radius, 32.),
            Vec2::new(37., 32. + radius),
            Vec2::new(-37., 32. + radius),
            Vec2::new(-37. - radius, 32.),
            Vec2::new(-37. - radius, -32.),
        ];
        let points: Vec<_> =
            endpoints.iter().enumerate().map(|(index, point)| sketch.add_point(*point, Some(&format!("source-p{index}"))).unwrap()).collect();
        let centers = [Vec2::new(37., -32.), Vec2::new(37., 32.), Vec2::new(-37., 32.), Vec2::new(-37., -32.)];
        for index in 0..8 {
            let id = format!("source-edge{index}");
            if index % 2 == 0 {
                sketch.add_line_pts(points[index], points[(index + 1) % 8], Some(&id)).unwrap();
            } else {
                let center = centers[index / 2];
                sketch
                    .add_arc(
                        center,
                        endpoints[index],
                        endpoints[(index + 1) % 8],
                        [None, Some(points[index]), Some(points[(index + 1) % 8])],
                        Some(&id),
                    )
                    .unwrap();
            }
        }
        for point in &mut sketch.points {
            point.fixed = true;
        }
        (0..8).map(|index| (format!("source-edge{index}"), true)).collect()
    }

    fn set_source_radius(sketch: &mut Sketch, radius: f64) {
        let points = [
            Vec2::new(-37., -32. - radius),
            Vec2::new(37., -32. - radius),
            Vec2::new(37. + radius, -32.),
            Vec2::new(37. + radius, 32.),
            Vec2::new(37., 32. + radius),
            Vec2::new(-37., 32. + radius),
            Vec2::new(-37. - radius, 32.),
            Vec2::new(-37. - radius, -32.),
        ];
        for (index, value) in points.into_iter().enumerate() {
            sketch.points.iter_mut().find(|point| point.id == format!("source-p{index}")).unwrap().pos = value;
        }
    }

    #[test]
    fn captured_sharp_offset_profile_survives_arc_resurrection_and_reopen() {
        let mut sketch = Sketch::new();
        let sources = rounded_source(&mut sketch, 1.);
        let created = sketch.add_convex_offset(sources, 1.4).unwrap();
        assert_eq!(created.curves.len(), 4);
        let mut document = crate::Document::new("selected logical inner boundary");
        let feature =
            document.add_feature(crate::FeatureKind::Sketch { plane: crate::PlaneRef::Origin { name: "XY".into() }, sketch }, None).unwrap();
        let mut model = crate::Model::new();
        model.evaluate(&document);
        let state = model.state();
        let solved = state.sketch(feature).unwrap();
        let index = unique_profile(solved, &created.curves).unwrap();
        let selection = capture_profile_selection(solved, &[index]).unwrap();
        assert_eq!(selection, ProfileSel::OffsetBoundaries { associations: vec![created.association.clone()] });
        let original_roles = solved.sketch.offsets[0].topology.as_ref().unwrap().roles.iter().map(|role| role.output.clone()).collect::<Vec<_>>();
        let sharp_area = crate::select_profiles(solved, &selection).unwrap()[0].area();
        assert!((sharp_area - 73.2 * 63.2).abs() < 1e-7, "{sharp_area}");
        document
            .add_feature(
                crate::FeatureKind::Extrude {
                    thin: None,
                    sketch: crate::SketchId(feature),
                    profiles: selection.clone(),
                    extent: crate::Extent {
                        convention: crate::ExtrudeConvention::LegacyPerSide,
                        distance: "2".into(),
                        direction: crate::Direction::Positive,
                        distance2: None,
                        start_offset: None,
                        through_all: false,
                        taper: None,
                        to: None,
                        to_offset: None,
                        from: None,
                    },
                    operation: crate::Operation::NewBody,
                    targets: vec![],
                },
                None,
            )
            .unwrap();
        model.evaluate(&document);
        let volume = solidcraft_kernel::measure(&model.state().bodies[0].body).unwrap().volume;
        assert!((volume - 2. * 73.2 * 63.2).abs() < 1e-6, "{volume}");
        let reopened = crate::Document::from_json(&document.to_json()).unwrap();
        assert_eq!(reopened, document);
        let mut cold = crate::Model::new();
        cold.evaluate(&reopened);
        assert!((solidcraft_kernel::measure(&cold.state().bodies[0].body).unwrap().volume - volume).abs() < 1e-6);
        let crate::FeatureKind::Sketch { sketch, .. } = &mut document.features[0].kind else { panic!("sketch") };
        set_source_radius(sketch, 2.);
        model.evaluate(&document);
        assert!(model.results.iter().all(|result| result.error.is_none()), "{:?}", model.results);
        let state = model.state();
        let solved = state.sketch(feature).unwrap();
        let active = solved.sketch.offset_boundary(&created.association).unwrap();
        assert_eq!(active, original_roles);
        let rounded_area = 75.2 * 65.2 - (4. - std::f64::consts::PI) * 0.6 * 0.6;
        assert!((crate::select_profiles(solved, &selection).unwrap()[0].area() - rounded_area).abs() < 1e-7);
        let volume = solidcraft_kernel::measure(&model.state().bodies[0].body).unwrap().volume;
        // Kernel volume integrates the curved display mesh; the analytic region above
        // checks the exact rounded geometry independently of that tessellation.
        assert!((volume - 2. * rounded_area).abs() < 0.002, "{volume}");
        assert!(
            crate::select_profiles(solved, &ProfileSel::Curves { loops: vec![created.curves.clone()] }).is_err(),
            "partial old active set must not expand after resurrection"
        );
        assert_eq!(
            profile_selection_indices(solved, &ProfileSel::Curves { loops: vec![original_roles] }).unwrap(),
            profile_selection_indices(solved, &selection).unwrap()
        );
        let reopened = crate::Document::from_json(&document.to_json()).unwrap();
        cold.evaluate(&reopened);
        assert!((solidcraft_kernel::measure(&cold.state().bodies[0].body).unwrap().volume - volume).abs() < 1e-5);
    }

    #[test]
    fn ordinary_profile_selection_keeps_legacy_large_and_duplicate_semantics() {
        let mut sketch = Sketch::new();
        sketch.add_circle(Vec2::new(0., 0.), 3., None, Some("circle")).unwrap();
        let report = solve(&mut sketch);
        let profiles = find_profiles(&sketch);
        let ss = SolvedSketch { feature: 1, name: "ordinary circle".into(), plane: Plane::XY, sketch, report, profiles, ends: Default::default() };
        assert_eq!(ss.profiles.len(), 1);
        let indices = vec![0; 600];
        let selection = ProfileSel::Indices { indices: indices.clone() };
        assert_eq!(profile_selection_indices(&ss, &selection).unwrap(), indices);
        assert_eq!(capture_profile_selection(&ss, &indices).unwrap(), selection);
        let points = ProfileSel::Points { points: vec![Vec2::new(0., 0.); 600] };
        assert_eq!(profile_selection_indices(&ss, &points).unwrap(), indices);
        let curves = ProfileSel::Curves { loops: vec![vec!["circle".into()]; 600] };
        let regions = crate::select_profiles(&ss, &curves).unwrap();
        assert_eq!(regions.len(), 600);
        assert!(regions.iter().all(|region| (region.area() - std::f64::consts::PI * 9.).abs() < 1e-9));
        assert!(profile_selection_indices(&ss, &ProfileSel::Indices { indices: vec![1] }).is_err());
    }

    #[test]
    fn periodic_offset_ring_whole_source_survives_adaptive_spans_and_reopen() {
        let mut sketch = Sketch::new();
        let major = sketch.add_point(Vec2::new(10., 0.), None).unwrap();
        let ellipse = sketch.add_curve(solidcraft_sketch::CurveKind::Ellipse { c: 0, m: major, r: 5. }, None).unwrap();
        let first = sketch.add_freeform_offset(ellipse, true, -1.).unwrap();
        sketch.add_freeform_offset(first, true, -0.5).unwrap();
        let mut document = crate::Document::new("periodic offset ring multiset");
        let feature =
            document.add_feature(crate::FeatureKind::Sketch { plane: crate::PlaneRef::Origin { name: "XY".into() }, sketch }, None).unwrap();
        let mut model = crate::Model::new();
        model.evaluate(&document);
        let state = model.state();
        let solved = state.sketch(feature).unwrap();
        assert_eq!(solved.profiles.len(), 3);
        let (index, profile) = solved.profiles.iter().enumerate().min_by(|(_, a), (_, b)| a.area.total_cmp(&b.area)).unwrap();
        assert!(profile.outer_curves.len() > 1);
        assert_eq!(profile.outer_curves.iter().collect::<BTreeSet<_>>().len(), 1);
        assert!(solved.sketch.offset_boundary_for_curves(&profile.outer_curves).is_err(), "strict association identity still refuses repeated roles");
        let selection = capture_profile_selection(solved, &[index]).unwrap();
        assert!(matches!(&selection, ProfileSel::WholeBoundaries { profiles } if profiles.len() == 1 && profiles[0].holes.len() == 1));
        let legacy_spans = ProfileSel::Curves { loops: vec![profile.outer_curves.clone()] };
        assert_eq!(profile_selection_indices(solved, &legacy_spans).unwrap(), vec![index]);
        let legacy_roundtrip: ProfileSel = serde_json::from_str(&serde_json::to_string(&legacy_spans).unwrap()).unwrap();
        assert_eq!(legacy_roundtrip, legacy_spans, "ordinary legacy span multiplicities remain serialized exactly");
        assert_eq!(profile_selection_indices(solved, &selection).unwrap(), vec![index]);
        let selected = crate::select_profiles(solved, &selection).unwrap();
        assert_eq!(selected.len(), 1);
        assert!((selected[0].area() - profile.area).abs() < 1e-10);
        assert!(selected[0].contains(Vec2::new(11.25, 0.)));
        assert!(!selected[0].contains(Vec2::new(10.5, 0.)));
        assert!(!selected[0].contains(Vec2::new(11.75, 0.)));
        let saved = serde_json::to_string(&selection).unwrap();
        let reopened: ProfileSel = serde_json::from_str(&saved).unwrap();
        let reopened_doc = crate::Document::from_json(&document.to_json()).unwrap();
        let mut cold = crate::Model::new();
        cold.evaluate(&reopened_doc);
        let cold_state = cold.state();
        let regions = crate::select_profiles(cold_state.sketch(feature).unwrap(), &reopened).unwrap();
        assert_eq!(regions, selected);

        // An identical complete identity in two profiles is ambiguous, even when
        // their boundary geometry and graph coverage certificates are identical.
        let mut ambiguous = cold_state.sketch(feature).unwrap().clone();
        ambiguous.profiles.push(ambiguous.profiles[index].clone());
        assert!(profile_selection_indices(&ambiguous, &reopened).is_err());

        document
            .add_feature(
                crate::FeatureKind::Extrude {
                    thin: None,
                    sketch: crate::SketchId(feature),
                    profiles: selection.clone(),
                    extent: crate::Extent {
                        convention: crate::ExtrudeConvention::LegacyPerSide,
                        distance: "2".into(),
                        direction: crate::Direction::Positive,
                        distance2: None,
                        start_offset: None,
                        through_all: false,
                        taper: None,
                        to: None,
                        to_offset: None,
                        from: None,
                    },
                    operation: crate::Operation::NewBody,
                    targets: vec![],
                },
                None,
            )
            .unwrap();
        model.evaluate(&document);
        let initial_volume = solidcraft_kernel::measure(&model.state().bodies[0].body).unwrap().volume;
        // Kernel measurement integrates the curved mesh. Scale the established
        // periodic-ring tolerance (0.05 mm³ at height3) to this height2 extrusion.
        // Exact profile identity and cold/warm equality below remain strict.
        let volume_tolerance = 0.05 * 2. / 3.;
        assert!(
            (initial_volume - 2. * profile.area).abs() < volume_tolerance,
            "initial kernel volume {initial_volume}, analytic area {}",
            profile.area
        );
        let crate::FeatureKind::Sketch { sketch, .. } = &mut document.features[0].kind else { panic!("sketch") };
        sketch.points[major].pos.x = 12.;
        model.evaluate(&document);
        assert!(model.results.iter().all(|result| result.error.is_none()), "{:?}", model.results);
        let updated_state = model.state();
        let updated = updated_state.sketch(feature).unwrap();
        assert!(profile_selection_indices(updated, &legacy_spans).is_err(), "legacy tessellation-span multiplicity intentionally remains exact");
        let updated_index = profile_selection_indices(updated, &selection).unwrap()[0];
        let updated_region = &updated.profiles[updated_index].region;
        // Probe away from exact spline-span seams, using the independent analytic
        // ellipse point and unit outward gradient rather than a stored profile sample.
        let parameter = 0.2_f64;
        let source_point = Vec2::new(12. * parameter.cos(), 5. * parameter.sin());
        let normal = Vec2::new(parameter.cos() / 12., parameter.sin() / 5.).normalized().unwrap();
        let ring_point = source_point + normal * 1.25;
        assert!(updated_region.contains(ring_point));
        assert!(!updated_region.contains(source_point + normal * 0.5));
        assert!(!updated_region.contains(source_point + normal * 1.75));
        let updated_volume = solidcraft_kernel::measure(&updated_state.bodies[0].body).unwrap().volume;
        assert!(updated_volume > initial_volume);
        assert!(
            (updated_volume - 2. * updated_region.area()).abs() < volume_tolerance,
            "updated kernel volume {updated_volume}, analytic area {}",
            updated_region.area()
        );
        let reopened_document = crate::Document::from_json(&document.to_json()).unwrap();
        cold.evaluate(&reopened_document);
        assert!(cold.results.iter().all(|result| result.error.is_none()));
        assert!((solidcraft_kernel::measure(&cold.state().bodies[0].body).unwrap().volume - updated_volume).abs() < 1e-6);

        // A new hole is a semantic boundary change, not the old ring. Whole-source
        // identities include holes and must not silently absorb the new material cutout.
        let crate::FeatureKind::Sketch { sketch, .. } = &mut document.features[0].kind else { panic!("sketch") };
        sketch.add_circle(ring_point, 0.1, None, Some("new-hole")).unwrap();
        model.evaluate(&document);
        let changed = model.state();
        assert!(profile_selection_indices(changed.sketch(feature).unwrap(), &selection).is_err());
        assert!(model.results.last().unwrap().error.is_some());
    }
}
