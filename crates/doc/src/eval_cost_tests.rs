//! What an evaluation costs after an edit at the end of a large design: the features before
//! the edit are reused, so the edit's cost must not grow with the bodies and imported files
//! the design already holds. A sketch on a fixed plane once moved every body of every placed
//! component into its frame, and every evaluation wrote every imported STEP file out again to
//! fingerprint it, so each sketch command took longer the more parts were inserted.

use std::cell::Cell;
use std::sync::Arc;

use solidcraft_geom::{Plane, Vec2, Vec3};
use solidcraft_kernel as kernel;
use solidcraft_sketch::Sketch;

use crate::*;

/// Work counted on this thread: bodies moved into a component's frame, STEP texts digested.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct Counts {
    pub moved: usize,
    pub digested: usize,
}

thread_local! {
    static COUNTS: Cell<Counts> = const { Cell::new(Counts { moved: 0, digested: 0 }) };
}

pub(super) fn count(f: impl FnOnce(&mut Counts)) {
    COUNTS.with(|c| {
        let mut n = c.get();
        f(&mut n);
        c.set(n);
    });
}

/// The work `f` does on this thread.
fn counted(f: impl FnOnce()) -> Counts {
    COUNTS.with(|c| c.set(Counts::default()));
    f();
    COUNTS.with(Cell::get)
}

/// `parts` imported files of two boxes each, every one in its own component placed away from
/// the origin (as inserted parts are).
fn placed_imports(parts: usize) -> Document {
    let mut doc = Document::new("placed imports");
    for i in 0..parts {
        let x = 30.0 * i as f64;
        let a = kernel::box_solid(Vec3::new(x, 0.0, 0.0), Vec3::new(x + 10.0, 10.0, 10.0)).unwrap();
        let b = kernel::box_solid(Vec3::new(x, 20.0, 0.0), Vec3::new(x + 5.0, 25.0, 5.0)).unwrap();
        let step: Arc<str> = kernel::step_export(&[&a, &b], "test").unwrap().into();
        let comp = doc.add_component(Some(&format!("Part {i}")), 0).unwrap();
        let o = doc.occurrences.iter_mut().find(|o| o.component == comp).unwrap();
        o.transform = rigid(Vec3::new(0.0, 0.0, 5.0 * i as f64), Vec3::ZERO, Vec3::Z, 0.05 * i as f64);
        let kind = FeatureKind::Import { file: format!("part{i}.step"), step, components: Vec::new(), skip: Vec::new() };
        let id = doc.add_feature(kind, Some(&format!("Part {i}"))).unwrap();
        doc.feature_mut(id).unwrap().component = comp;
    }
    doc
}

fn line_sketch(plane: PlaneRef, to: Vec2) -> FeatureKind {
    let mut sk = Sketch::new();
    let a = sk.add_point(Vec2::new(0.0, 0.0), None).unwrap();
    let b = sk.add_point(to, None).unwrap();
    sk.add_line_pts(a, b, None).unwrap();
    FeatureKind::Sketch { plane, sketch: sk }
}

#[test]
fn a_sketch_on_a_fixed_plane_costs_the_same_in_a_large_placed_design() {
    let parts = 12;
    let mut doc = placed_imports(parts);
    let mut model = Model::new();
    model.evaluate(&doc);
    assert!(model.results.iter().all(|r| r.error.is_none()), "{:?}", model.results.iter().find(|r| r.error.is_some()));
    assert_eq!(model.state().bodies.len(), parts * 2);

    // A construction plane, then sketches on the origin planes, on an offset of one and on the
    // construction plane, as commands make them one after another: no body is moved and no
    // file digested again.
    let plane = doc.add_feature(FeatureKind::ConstructionPlane { plane: PlaneRef::Origin { name: "XZ".into() } }, Some("Datum")).unwrap();
    assert_eq!(counted(|| model.evaluate(&doc)), Counts::default(), "construction plane");
    assert!(model.result(plane).unwrap().error.is_none());
    let datum = doc.feature(plane).unwrap().name.clone();
    let planes = [
        PlaneRef::Origin { name: "XY".into() },
        PlaneRef::Offset { base: Box::new(PlaneRef::Origin { name: "YZ".into() }), distance: "5 mm".into() },
        PlaneRef::Construction { name: datum.into() },
    ];
    for (k, plane) in planes.into_iter().enumerate() {
        let id = doc.add_feature(line_sketch(plane, Vec2::new(10.0, 1.0 + k as f64)), None).unwrap();
        let work = counted(|| model.evaluate(&doc));
        assert_eq!(work, Counts::default(), "sketch {k}");
        assert_eq!(model.last_recomputed, 1);
        let r = model.result(id).unwrap();
        assert!(r.error.is_none(), "{:?}", r.error);
        assert_eq!(model.state().bodies.len(), parts * 2);
        // Editing the sketch again recomputes it alone, for the same cost.
        if let Ok(sk) = doc.sketch_mut(id)
            && let Some(p) = sk.points.get_mut(1)
        {
            p.pos = Vec2::new(12.0, 2.0);
        }
        let work = counted(|| model.evaluate(&doc));
        assert_eq!(work, Counts::default(), "sketch {k} edited");
        assert_eq!(model.last_recomputed, 1);
    }
}

#[test]
fn a_sketch_on_a_face_still_sees_the_placed_bodies() {
    let parts = 3;
    let mut doc = placed_imports(parts);
    let mut model = Model::new();
    model.evaluate(&doc);
    // A face pick reads the model as the sketch's component sees it: the other components'
    // bodies are moved into its frame, as before.
    let top = Plane::new(Vec3::new(0.0, 0.0, 10.0), Vec3::X, Vec3::Y).unwrap();
    let face = PlaneRef::Face { plane: top, at: Vec3::new(5.0, 5.0, 10.0), name: None, anchor: None };
    doc.add_feature(line_sketch(face, Vec2::new(3.0, 3.0)), None).unwrap();
    let work = counted(|| model.evaluate(&doc));
    assert!(work.moved >= parts * 2 - 2, "{work:?}");
    assert_eq!(work.digested, 0);
}

#[test]
fn an_imported_file_is_fingerprinted_by_its_text() {
    let doc = placed_imports(2);
    let mut model = Model::new();
    let work = counted(|| model.evaluate(&doc));
    assert_eq!(work.digested, 2);
    let fingerprints = |m: &Model| m.results.iter().map(|r| r.fingerprint).collect::<Vec<_>>();
    let before = fingerprints(&model);

    // The same text held by another copy (an undo state, a design opened again): the same
    // fingerprints, and nothing evaluated again.
    let mut copy = doc.clone();
    for f in &mut copy.features {
        if let FeatureKind::Import { step, .. } = &mut f.kind {
            *step = Arc::from(step.to_string());
        }
    }
    let work = counted(|| model.evaluate(&copy));
    assert_eq!(work.digested, 2);
    assert_eq!(fingerprints(&model), before);
    assert_eq!(model.last_recomputed, 0);

    // Other text: that feature and the ones after it are evaluated again.
    let other = kernel::box_solid(Vec3::ZERO, Vec3::new(4.0, 4.0, 4.0)).unwrap();
    let text: Arc<str> = kernel::step_export(&[&other], "test").unwrap().into();
    if let Some(FeatureKind::Import { step, .. }) = copy.features.last_mut().map(|f| &mut f.kind) {
        *step = text;
    }
    let work = counted(|| model.evaluate(&copy));
    assert_eq!(work.digested, 1);
    let after = fingerprints(&model);
    assert_eq!(after.first(), before.first());
    assert_ne!(after.last(), before.last());
    assert_eq!(model.last_recomputed, 1);
    assert_eq!(model.state().bodies.len(), 3);
}
