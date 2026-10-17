//! The bracket example must cut two through holes, rather than successfully cutting empty
//! space beside the body. Check the shipped script and rebuild its saved design from scratch.

use std::f64::consts::PI;

use serde_json::Value;
use solidcraft_engine::geom::Vec3;
use solidcraft_engine::{Session, io, kernel};

fn assert_two_through_holes(session: &Session, stage: &str) {
    let errors: Vec<_> = session.model.results.iter().filter_map(|r| r.error.as_ref().map(|e| (&r.name, e))).collect();
    assert!(errors.is_empty(), "{stage}: every example feature evaluates: {errors:?}");

    let cut = session.doc.features.last().expect("the example ends with the hole cut");
    let before = session.model.state_before(cut.id);
    let after = session.model.state();
    assert_eq!(before.bodies.len(), 1, "{stage}: one bracket before cutting the holes");
    assert_eq!(after.bodies.len(), 1, "{stage}: the holes leave one bracket");
    let uncut = &before.body("Bracket").expect("the uncut bracket").body;
    let bracket = &after.body("Bracket").expect("the finished bracket").body;

    // Measure the actual state before the final cut so the existing edge fillet cancels out.
    // Both holes have diameter 8 mm and pass through the 6 mm base. Kernel measurements are
    // tessellated, so the relative tolerance applies to the removed material, not the body.
    let removed = kernel::measure(uncut).unwrap().volume - kernel::measure(bracket).unwrap().volume;
    let expected = 2.0 * PI * 4.0_f64.powi(2) * 6.0;
    assert!((removed - expected).abs() < expected * 1e-3, "{stage}: the two holes remove {removed} mm³, expected {expected} mm³");

    let before_mesh = uncut.tessellate(0.01).unwrap();
    let after_mesh = bracket.tessellate(0.01).unwrap();
    let down = Vec3::new(0.0, 0.0, -1.0);
    for y in [-15.0, -45.0] {
        let center = Vec3::new(25.0, y, 1.0);
        assert_eq!(before_mesh.ray_crossings(center, down), 2, "{stage}: hole at y={y} starts over the solid base");
        // Rays through each opening must miss the entire bracket, including its top and
        // bottom caps. Points away from the centre guard against a narrow slit or shifted cut.
        for (dx, dy) in [(0.0, 0.0), (3.0, 0.0), (-3.0, 0.0), (0.0, 3.0), (0.0, -3.0)] {
            let ray = center + Vec3::new(dx, dy, 0.0);
            assert_eq!(after_mesh.ray_crossings(ray, down), 0, "{stage}: the hole at y={y} is open through both caps at ({dx}, {dy})");
        }
        // Material just outside each radius remains, so neither opening is an edge notch.
        for (dx, dy) in [(5.0, 0.0), (-5.0, 0.0), (0.0, 5.0), (0.0, -5.0)] {
            let ray = center + Vec3::new(dx, dy, 0.0);
            assert_eq!(after_mesh.ray_crossings(ray, down), 2, "{stage}: material remains around the hole at y={y}, offset ({dx}, {dy})");
        }
    }
}

#[test]
fn bracket_example_cuts_two_through_holes_and_reopens() {
    let script: Value = serde_json::from_str(include_str!("../../../examples/bracket.json")).unwrap();
    let mut session = Session::default();
    session.run_script(&script).unwrap();
    assert_two_through_holes(&session, "script");

    // No geometry cache is supplied: reopening the container re-evaluates the saved timeline.
    let saved = io::write_design_container(&session.doc).unwrap();
    let document = io::read_design(&saved).unwrap();
    assert_eq!(document.features, session.doc.features, "saving preserves the example's feature parameters");
    let reopened = Session::new(document);
    assert_two_through_holes(&reopened, "reopened");
}
