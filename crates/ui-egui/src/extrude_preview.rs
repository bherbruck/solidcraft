//! Straight extrusion feedback from existing display triangles, without a scratch design.
use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;
use solidcraft_engine::{
    Sel,
    doc::Document,
    geom::{Aabb3, Seg2, Vec3},
};

use crate::{
    SolveApp,
    gpu::GpuScene,
    preview::{Built, Colors},
};

#[derive(Default)]
pub(crate) struct StraightPreview {
    cached: Option<Cached>,
}

struct Cached {
    document: Arc<Document>,
    revision: u64,
    inputs: Vec<Sel>,
    surfaces: Vec<Surface>,
    bounds: Aabb3,
}
struct Surface {
    normal: Vec3,
    triangles: Vec<[Vec3; 3]>,
    edges: Vec<[Vec3; 2]>,
}

impl StraightPreview {
    pub(crate) fn clear(&mut self) {
        self.cached = None;
    }
    pub(crate) fn compute(&mut self, app: &SolveApp, commands: &[(String, Value)], colors: Colors) -> Option<Result<Built, String>> {
        let [(id, params)] = commands else { return None };
        let params = match id.as_str() {
            "solid.extrude" => params,
            "timeline.redefine" if params.get("command").and_then(Value::as_str) == Some("solid.extrude") => params.get("params")?,
            _ => return None,
        };
        // A newly opened STEP is now a component. Its single identity placement
        // has the same authoring coordinates; other placements keep Engine preview.
        if app.session.active_component != 0 || app.session.active_occurrence_path.is_some() {
            let path = app.session.active_occurrence_path.as_ref()?;
            let placed = app.session.occurrence_placement(path).ok()?;
            if path.len() != 1
                || placed.component != app.session.active_component
                || placed.world != solidcraft_engine::doc::IDENTITY
                || app.session.doc.occurrences.iter().filter(|o| o.component == placed.component).count() != 1
            {
                return None;
            }
        }
        if app.session.doc.units != "mm"
            || ["distance2", "start_offset", "from", "to", "to_offset", "thin"].iter().any(|key| params.get(*key).is_some())
            || params.get("through_all").and_then(Value::as_bool) == Some(true)
        {
            return None;
        }
        let direction = params.get("direction").and_then(Value::as_str).unwrap_or("positive");
        if !matches!(direction, "positive" | "negative") {
            return None;
        }
        if let Some(taper) = params.get("taper") {
            let expression = taper.as_str().map(str::to_owned).unwrap_or_else(|| taper.to_string());
            if app.session.doc.eval(&expression, solidcraft_engine::doc::expr::Kind::Angle).ok()?.abs() > 1e-12 {
                return None;
            }
        }
        let distance = params.get("distance")?;
        let expression = distance.as_str().map(str::to_owned).unwrap_or_else(|| distance.to_string());
        let mut distance = app.session.doc.eval(&expression, solidcraft_engine::doc::expr::Kind::Length).ok()?;
        if direction == "negative" {
            distance = -distance;
        }
        if !distance.is_finite() || distance.abs() < 1e-9 || distance.abs() > 1e6 {
            return None;
        }
        let operation = params.get("operation").and_then(Value::as_str).unwrap_or("new");
        if !matches!(operation, "new" | "join" | "cut" | "intersect") {
            return None;
        }
        let inputs = &app.dialog.as_ref()?.inputs.first()?.items;
        if inputs.is_empty() || inputs.len() > 1024 {
            return None;
        }
        if self
            .cached
            .as_ref()
            .is_none_or(|cache| cache.revision != app.session.revision || !Arc::ptr_eq(&cache.document, &app.session.doc) || cache.inputs != *inputs)
        {
            self.cached = prepare(app, inputs);
        }
        let cache = self.cached.as_ref()?;
        let mut scene = GpuScene::default();
        let mut bounds = cache.bounds;
        let color = if operation == "cut" {
            colors.removed
        } else if operation == "intersect" {
            [colors.added[0], colors.added[1], colors.added[2], colors.removed[3]]
        } else {
            colors.added
        };
        let triangle = |scene: &mut GpuScene, points: [Vec3; 3], normal: Vec3| {
            for point in points {
                if matches!(operation, "cut" | "intersect") {
                    scene.xray_tri(point.to_f32(), normal.to_f32(), color);
                } else {
                    scene.tri(point.to_f32(), normal.to_f32(), color);
                }
            }
        };
        for surface in &cache.surfaces {
            let shift = surface.normal * distance;
            let (low, high) = if distance > 0. { (Vec3::ZERO, shift) } else { (shift, Vec3::ZERO) };
            for &[a, b, c] in &surface.triangles {
                triangle(&mut scene, [a + low, c + low, b + low], -surface.normal);
                triangle(&mut scene, [a + high, b + high, c + high], surface.normal);
                for point in [a + low, b + low, c + low, a + high, b + high, c + high] {
                    bounds.add(point);
                }
            }
            let edge_color = if operation == "cut" { [color[0], color[1], color[2], 255] } else { colors.edge };
            for &[a, b] in &surface.edges {
                let normal = (b - a).cross(surface.normal).normalized()?;
                triangle(&mut scene, [a + low, b + low, b + high], normal);
                triangle(&mut scene, [a + low, b + high, a + high], normal);
                for (a, b) in [(a + low, b + low), (a + high, b + high), (a + low, a + high)] {
                    scene.line(a.to_f32(), b.to_f32(), edge_color, 1.3, operation != "join");
                }
            }
        }
        Some(Ok(Built {
            scene,
            sketches: Vec::new(),
            applied: None,
            stopped: None,
            tapered: Vec::new(),
            replaced: Vec::new(),
            error: crate::preview::extrusion_target_error(&app.session, commands, &app.session.model.state()),
            bounds,
        }))
    }
}

fn prepare(app: &SolveApp, inputs: &[Sel]) -> Option<Cached> {
    let state = app.session.world_state();
    let mut surfaces = Vec::new();
    let mut budget = 65_536usize;
    if inputs.iter().all(|input| matches!(input, Sel::Profile { .. })) {
        let Sel::Profile { sketch: owner, .. } = inputs.first()? else { return None };
        if inputs.iter().any(|input| !matches!(input,Sel::Profile {sketch,..} if sketch==owner)) {
            return None;
        }
        for input in inputs {
            let Sel::Profile { sketch, index } = input else { return None };
            let sketch = state.sketch(*sketch)?;
            let region = &sketch.profiles.get(*index)?.region;
            let loops = std::iter::once(&region.outer).chain(&region.holes);
            if loops.clone().flat_map(|loop_| &loop_.segs).any(|segment| !matches!(segment, Seg2::Line { .. })) {
                return None;
            }
            let triangles = crate::sketch_display::fill_triangles(region, &mut budget)
                .into_iter()
                .map(|triangle| triangle.map(|point| sketch.plane.to_world(point)))
                .collect::<Vec<_>>();
            if triangles.is_empty() {
                return None;
            }
            let edges = std::iter::once((&region.outer, true))
                .chain(region.holes.iter().map(|hole| (hole, false)))
                .flat_map(|(loop_, outer)| {
                    let reverse = (loop_.signed_area() > 0.) != outer;
                    loop_.segs.iter().map(move |segment| {
                        let pair = [sketch.plane.to_world(segment.start()), sketch.plane.to_world(segment.end())];
                        if reverse { [pair[1], pair[0]] } else { pair }
                    })
                })
                .collect();
            surfaces.push(Surface { normal: sketch.plane.normal(), triangles, edges });
        }
    } else if inputs.iter().all(|input| matches!(input, Sel::Face { .. })) {
        // One pass over each selected body's triangles, rather than one full scan per pick.
        for body in &state.bodies {
            let mut selected = HashMap::new();
            for input in inputs {
                if let Sel::Face { body: name, index, point } = input
                    && name == &body.name
                {
                    let face = u32::try_from(*index).ok()?;
                    if selected.contains_key(&face) {
                        return None;
                    }
                    let normal = body.body.planar_face(*index)?.normal();
                    selected.insert(face, (surfaces.len(), *point));
                    surfaces.push(Surface { normal, triangles: Vec::new(), edges: Vec::new() });
                }
            }
            if selected.is_empty() {
                continue;
            }
            let mesh = body.mesh();
            for (triangle, face) in mesh.triangles.iter().zip(&mesh.tri_face) {
                if let Some((index, _)) = selected.get(face) {
                    budget = budget.checked_sub(1)?;
                    let mut points = mesh.tri(triangle)?;
                    let surface = surfaces.get_mut(*index)?;
                    if (points[1] - points[0]).cross(points[2] - points[0]).dot(surface.normal) < 0. {
                        points.swap(1, 2);
                    }
                    surface.triangles.push(points);
                }
            }
            // The command resolves picks by their point. A stale display face index
            // must fall back rather than previewing a different face.
            let tolerance = (mesh.bounds().diagonal() * 1e-8).max(1e-8);
            for (index, point) in selected.values() {
                if !surfaces.get(*index)?.triangles.iter().any(|triangle| crate::dialogs::point_tri_dist(*point, *triangle) < tolerance) {
                    return None;
                }
            }
        }
        if surfaces.len() != inputs.len() {
            return None;
        }
        for surface in &mut surfaces {
            let key = |point: Vec3| [point.x, point.y, point.z].map(|coordinate| if coordinate == 0. { 0 } else { coordinate.to_bits() });
            let mut edges = HashMap::new();
            for &[a, b, c] in &surface.triangles {
                for (a, b) in [(a, b), (b, c), (c, a)] {
                    let (ka, kb) = (key(a), key(b));
                    let entry = edges.entry(if ka < kb { (ka, kb) } else { (kb, ka) });
                    match entry {
                        std::collections::hash_map::Entry::Vacant(entry) => {
                            entry.insert(Some([a, b]));
                        }
                        std::collections::hash_map::Entry::Occupied(mut entry) => {
                            entry.get().as_ref()?;
                            entry.insert(None);
                        }
                    }
                }
            }
            surface.edges = edges.into_values().flatten().collect();
            if surface.triangles.is_empty() || surface.edges.is_empty() {
                return None;
            }
        }
    } else {
        return None;
    }
    let bounds = state.bodies.iter().fold(Aabb3::EMPTY, |bounds, body| bounds.union(&body.mesh().bounds()));
    Some(Cached { document: app.session.doc.clone(), revision: app.session.revision, inputs: inputs.to_vec(), surfaces, bounds })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{dialogs::Kind, scenario::Harness};
    use serde_json::json;

    fn mesh_volume(scene: &GpuScene) -> f64 {
        scene
            .tris
            .as_chunks::<{ crate::gpu::TRI_SIZE * 3 }>()
            .0
            .iter()
            .map(|triangle| {
                let points: Vec<_> = triangle
                    .as_chunks::<{ crate::gpu::TRI_SIZE }>()
                    .0
                    .iter()
                    .map(|vertex| {
                        let numbers: Vec<_> =
                            vertex.get(..12).unwrap().as_chunks::<4>().0.iter().map(|bytes| f32::from_le_bytes(*bytes) as f64).collect();
                        Vec3::new(numbers[0], numbers[1], numbers[2])
                    })
                    .collect();
                points[0].dot(points[1].cross(points[2])) / 6.
            })
            .sum()
    }
    #[test]
    fn face_mesh_feedback_reuses_cap_and_handles_both_distance_signs() {
        let mut harness = Harness::new();
        harness.app.run("solid.box", json!({"length":4,"width":3,"height":5,"body_name":"Stock"})).unwrap();
        let state = harness.app.session.world_state();
        let face = state
            .body("Stock")
            .unwrap()
            .body
            .faces(0.001)
            .unwrap()
            .into_iter()
            .find(|face| face.plane_normal.is_some_and(|normal| normal.z > 0.99))
            .unwrap();
        harness.app.session.selection = vec![Sel::Face { body: "Stock".into(), index: face.index, point: face.centroid }];
        harness.app.start("solid.extrude");
        let original = harness.app.session.doc.clone();
        let undo = harness.app.session.undo.len();
        let mut preview = StraightPreview::default();
        let colors = Colors::from_tokens(&crate::theme::Tokens::get());
        let mut pointer = None;
        for distance in [2.0_f64, -2., 3.] {
            if let Kind::Extrude { distance: value, operation, auto_op, .. } = &mut harness.app.dialog.as_mut().unwrap().kind {
                *value = distance.to_string();
                *operation = 0;
                *auto_op = false;
            }
            let commands = crate::dialogs::apply_commands(&harness.app, harness.app.dialog.as_ref().unwrap()).unwrap();
            let scene = preview.compute(&harness.app, &commands, colors).unwrap().unwrap();
            assert!((mesh_volume(&scene.scene) - 12. * distance.abs()).abs() < 1e-5);
            let current = preview.cached.as_ref().unwrap().surfaces[0].triangles.as_ptr();
            if let Some(pointer) = pointer {
                assert_eq!(pointer, current);
            } else {
                pointer = Some(current);
            }
        }
        assert!(Arc::ptr_eq(&original, &harness.app.session.doc));
        assert_eq!(harness.app.session.undo.len(), undo);
        harness.app.run("solid.box", json!({"length":1,"width":1,"height":1,"corner":[30,0,0]})).unwrap();
        let commands = crate::dialogs::apply_commands(&harness.app, harness.app.dialog.as_ref().unwrap()).unwrap();
        preview.compute(&harness.app, &commands, colors).unwrap().unwrap();
        assert!(Arc::ptr_eq(&preview.cached.as_ref().unwrap().document, &harness.app.session.doc));
        assert!(!Arc::ptr_eq(&original, &preview.cached.as_ref().unwrap().document));
        if let Sel::Face { point, .. } = &mut harness.app.dialog.as_mut().unwrap().inputs[0].items[0] {
            *point = Vec3::new(100., 100., 100.);
        }
        let commands = crate::dialogs::apply_commands(&harness.app, harness.app.dialog.as_ref().unwrap()).unwrap();
        assert!(preview.compute(&harness.app, &commands, colors).is_none());
        let mut commands = crate::dialogs::apply_commands(&harness.app, harness.app.dialog.as_ref().unwrap()).unwrap();
        commands[0].1["through_all"] = json!(true);
        assert!(preview.compute(&harness.app, &commands, colors).is_none());
        commands[0].1 = json!({"distance":2,"taper":"3deg"});
        assert!(preview.compute(&harness.app, &commands, colors).is_none());
    }
    #[test]
    fn identity_component_feedback_skips_moved_and_repeated_placements() {
        let mut h = Harness::new();
        let made = h.app.run("component.create", json!({"name":"Imported part"})).unwrap();
        h.app.run("solid.box", json!({"length":4,"width":3,"height":5,"body_name":"Stock"})).unwrap();
        let world = h.app.session.world_state();
        let face =
            world.body("Stock").unwrap().body.faces(0.001).unwrap().into_iter().find(|face| face.plane_normal.is_some_and(|n| n.z > 0.99)).unwrap();
        h.app.session.selection = vec![Sel::Face { body: "Stock".into(), index: face.index, point: face.centroid }];
        h.app.start("solid.extrude");
        let commands = crate::dialogs::apply_commands(&h.app, h.app.dialog.as_ref().unwrap()).unwrap();
        let colors = Colors::from_tokens(&crate::theme::Tokens::get());
        let mut preview = StraightPreview::default();
        assert!(preview.compute(&h.app, &commands, colors).unwrap().is_ok());
        h.app.run("occurrence.move", json!({"occurrence":made["occurrence"],"translate":[20,0,0]})).unwrap();
        assert!(preview.compute(&h.app, &commands, colors).is_none());
        h.app.run("occurrence.move", json!({"occurrence":made["occurrence"],"translate":[-20,0,0]})).unwrap();
        h.app.run("occurrence.copy", json!({"component":made["component"],"translate":[30,0,0]})).unwrap();
        assert!(preview.compute(&h.app, &commands, colors).is_none());
    }

    #[test]
    fn profile_feedback_keeps_holes_and_skips_unsupported_curves() {
        let mut harness = Harness::new();
        harness.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
        for (a, b) in [([0., 0.], [10., 10.]), ([3., 3.], [7., 7.])] {
            harness.app.run("sketch.rectangle.two_point", json!({"p0":a,"p1":b})).unwrap();
        }
        harness.app.run("sketch.finish", json!({})).unwrap();
        let sketch = harness.app.session.model.state().sketches[0].clone();
        let index = sketch.profiles.iter().position(|profile| !profile.region.holes.is_empty()).unwrap();
        harness.app.session.selection = vec![Sel::Profile { sketch: sketch.feature, index }];
        harness.app.start("solid.extrude");
        if let Kind::Extrude { distance, operation, auto_op, .. } = &mut harness.app.dialog.as_mut().unwrap().kind {
            *distance = "2".into();
            *operation = 0;
            *auto_op = false;
        }
        let commands = crate::dialogs::apply_commands(&harness.app, harness.app.dialog.as_ref().unwrap()).unwrap();
        let scene = StraightPreview::default().compute(&harness.app, &commands, Colors::from_tokens(&crate::theme::Tokens::get())).unwrap().unwrap();
        assert!((mesh_volume(&scene.scene) - 168.).abs() < 1e-5);
        harness.app.dialog = None;
        harness.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
        harness.app.run("sketch.circle.center", json!({"center":[20,0],"diameter":4})).unwrap();
        harness.app.run("sketch.finish", json!({})).unwrap();
        let feature = harness.app.session.model.state().sketches.last().unwrap().feature;
        harness.app.session.selection = vec![Sel::Profile { sketch: feature, index: 0 }];
        harness.app.start("solid.extrude");
        let commands = crate::dialogs::apply_commands(&harness.app, harness.app.dialog.as_ref().unwrap()).unwrap();
        assert!(StraightPreview::default().compute(&harness.app, &commands, Colors::from_tokens(&crate::theme::Tokens::get())).is_none());
    }
}
