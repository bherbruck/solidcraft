use std::f64::consts::PI;

use super::*;
use crate::{MAX_BEZIER_CTRL, bezier};

const TOL: f64 = 1e-6;

fn empty(plane: Plane) -> SectionGeometry {
    SectionGeometry { plane, curves: vec![], wires: vec![], regions: vec![], contacts: vec![] }
}

fn analytic(geometry: Seg2) -> SectionCurve {
    SectionCurve { geometry, accuracy: CurveAccuracy::Analytic }
}

fn used(curve: u32, forward: bool) -> CurveUse {
    CurveUse { curve, forward }
}

fn plate() -> SectionGeometry {
    let p = Vec2::new;
    SectionGeometry {
        plane: Plane::XY.offset(10.0),
        curves: vec![
            // Deliberately stored backwards: the wire's use restores the CCW rectangle.
            analytic(Seg2::Line { a: p(100.0, 0.0), b: p(0.0, 0.0) }),
            analytic(Seg2::Line { a: p(100.0, 0.0), b: p(100.0, 60.0) }),
            analytic(Seg2::Line { a: p(100.0, 60.0), b: p(0.0, 60.0) }),
            analytic(Seg2::Line { a: p(0.0, 60.0), b: p(0.0, 0.0) }),
            analytic(Seg2::Arc { center: p(50.0, 30.0), radius: 5.0, start: 0.0, sweep: PI }),
            analytic(Seg2::Arc { center: p(50.0, 30.0), radius: 5.0, start: PI, sweep: PI }),
        ],
        wires: vec![
            SectionWire { curves: vec![used(0, false), used(1, true), used(2, true), used(3, true)], closed: true },
            SectionWire { curves: vec![used(5, false), used(4, false)], closed: true },
        ],
        regions: vec![SectionRegion { outer: 0, holes: vec![1] }],
        contacts: vec![],
    }
}

#[test]
fn section_frame_preserves_named_translated_tilted_offset_and_flip_conventions() {
    for plane in [Plane::XY, Plane::XZ, Plane::YZ] {
        validate_plane(&plane).unwrap();
    }
    assert_eq!(Plane::XZ.normal(), Vec3::Y);
    assert_eq!(Plane::XZ.to_world(Vec2::new(3.0, 4.0)), Vec3::new(3.0, 0.0, -4.0));
    let plane = Plane::from_normal(Vec3::new(13.0, -7.0, 19.0), Vec3::new(2.0, -3.0, 5.0)).unwrap();
    validate_plane(&plane).unwrap();
    let local = Vec2::new(7.0, -11.0);
    let height = 4.5;
    let point = plane.to_world(local) + plane.normal() * height;
    assert!(plane.to_local(point).dist(local) < 1e-12);
    assert!((plane.height(point) - height).abs() < 1e-12);
    let moved = plane.offset(8.0);
    validate_plane(&moved).unwrap();
    assert!((plane.height(moved.origin) - 8.0).abs() < 1e-12);
    for about_y in [false, true] {
        let flipped = moved.flipped(about_y);
        validate_plane(&flipped).unwrap();
        assert_eq!(flipped.origin, moved.origin);
        assert_eq!(flipped.normal(), -moved.normal());
        assert_eq!(flipped.flipped(about_y), moved);
        let on_plane = moved.to_world(local);
        let mirrored = if about_y { Vec2::new(-local.x, local.y) } else { Vec2::new(local.x, -local.y) };
        assert!(flipped.to_local(on_plane).dist(mirrored) < 1e-12);
        assert!((flipped.height(point) + moved.height(point)).abs() < 1e-12);
    }
}

#[test]
fn strict_frame_validation_does_not_repair_raw_or_deserialized_axes() {
    let bad = Plane { x: Vec3::X * 2.0, ..Plane::XY };
    assert!(Plane::new(bad.origin, bad.x, bad.y).is_some());
    assert_eq!(validate_plane(&bad), Err(Invalid("nonorthonormal plane")));
    assert_eq!(bad.x, Vec3::X * 2.0);
    let restored: Plane = serde_json::from_value(serde_json::json!({
        "origin": [0, 0, 0], "x_dir": [1, 0, 0], "y_dir": [1, 1, 0]
    }))
    .unwrap();
    assert_eq!(validate_plane(&restored), Err(Invalid("nonorthonormal plane")));
    for plane in [
        Plane { origin: Vec3::new(f64::NAN, 0.0, 0.0), ..Plane::XY },
        Plane { origin: Vec3::new(1e12 + 1.0, 0.0, 0.0), ..Plane::XY },
        Plane { x: Vec3::new(f64::MAX, 0.0, 0.0), ..Plane::XY },
        Plane { y: Vec3::new(0.0, f64::INFINITY, 0.0), ..Plane::XY },
        Plane { x: Vec3::ZERO, ..Plane::XY },
        Plane { y: Vec3::X, ..Plane::XY },
    ] {
        assert!(validate_plane(&plane).is_err());
    }
    validate_plane(&Plane { origin: Vec3::new(1e12, -1e12, 1e12), ..Plane::XY }).unwrap();
}

#[test]
fn line_arc_region_conversion_preserves_winding_and_analytic_area() {
    let section = plate();
    let before = section.clone();
    section.validate_structure(TOL).unwrap();
    let outer = section.to_loop(0, TOL).unwrap();
    let hole = section.to_loop(1, TOL).unwrap();
    assert!(outer.is_closed(TOL) && hole.is_closed(TOL));
    assert!((outer.signed_area() - 6000.0).abs() < 1e-10);
    assert!((hole.signed_area() + 25.0 * PI).abs() < 1e-10);
    let region = section.to_region(0, TOL).unwrap();
    assert_eq!(region.outer, outer);
    assert_eq!(region.holes, vec![hole]);
    assert!((region.area() - (6000.0 - 25.0 * PI)).abs() < 1e-10);
    assert_eq!(section, before);
}

#[test]
fn empty_contacts_and_open_wires_remain_distinct_without_inferred_material() {
    let empty = empty(Plane::XY);
    empty.validate_structure(TOL).unwrap();
    assert_eq!(empty.to_region(0, TOL), Err(Invalid("region reference")));
    let mut contact = empty.clone();
    contact.contacts.push(Vec2::new(2.0, 3.0));
    contact.validate_structure(TOL).unwrap();
    assert_ne!(contact, empty);
    let mut open = empty.clone();
    open.curves = vec![analytic(Seg2::Line { a: Vec2::ZERO, b: Vec2::X }), analytic(Seg2::Line { a: Vec2::X, b: Vec2::new(1.0, 1.0) })];
    open.wires = vec![SectionWire { curves: vec![used(0, true), used(1, true)], closed: false }];
    open.validate_structure(TOL).unwrap();
    assert_eq!(open.to_loop(0, TOL), Err(Invalid("wire is not closed")));
    open.regions.push(SectionRegion { outer: 0, holes: vec![] });
    assert_eq!(open.validate_structure(TOL), Err(Invalid("region boundary is not closed")));
    // Even a geometrically closed chain remains explicitly open when the producer says so.
    let mut declared_open = plate();
    declared_open.regions.clear();
    declared_open.wires[0].closed = false;
    declared_open.validate_structure(TOL).unwrap();
    assert_eq!(declared_open.to_loop(0, TOL), Err(Invalid("wire is not closed")));
}

#[test]
fn bad_references_empty_disconnected_and_unclosed_wires_fail() {
    let mut section = plate();
    section.wires[0].curves[0].curve = u32::MAX;
    assert_eq!(section.validate_structure(TOL), Err(Invalid("curve reference")));
    let mut section = plate();
    section.wires[0].curves.clear();
    assert_eq!(section.validate_structure(TOL), Err(Invalid("empty wire")));
    let mut section = plate();
    section.wires[0].curves[1].forward = false;
    assert_eq!(section.validate_structure(TOL), Err(Invalid("disconnected wire")));
    let mut section = plate();
    section.wires[0].curves.pop();
    assert_eq!(section.validate_structure(TOL), Err(Invalid("open closed wire")));
    let mut section = plate();
    section.regions[0].outer = u32::MAX;
    assert_eq!(section.validate_structure(TOL), Err(Invalid("wire reference")));
    let mut section = plate();
    section.regions[0].holes[0] = u32::MAX;
    assert_eq!(section.validate_structure(TOL), Err(Invalid("wire reference")));
    assert_eq!(plate().to_loop(u32::MAX, TOL), Err(Invalid("wire reference")));
    assert_eq!(plate().to_region(u32::MAX, TOL), Err(Invalid("region reference")));
}

#[test]
fn duplicate_boundaries_are_local_to_a_region_and_large_reference_tables_are_bounded() {
    let mut section = plate();
    section.regions[0].holes.push(1);
    assert_eq!(section.validate_structure(TOL), Err(Invalid("duplicate region boundary")));
    let mut section = plate();
    section.regions[0].holes.push(0);
    assert_eq!(section.validate_structure(TOL), Err(Invalid("duplicate region boundary")));
    let mut section = plate();
    section.regions.push(section.regions[0].clone());
    section.validate_structure(TOL).unwrap();
    assert_eq!(section.to_region(0, TOL).unwrap(), section.to_region(1, TOL).unwrap());
    // Deliberately coincident boundaries: this tests a large legal reference graph, not material.
    let mut many = empty(Plane::XY);
    many.curves.push(analytic(Seg2::Arc { center: Vec2::ZERO, radius: 1.0, start: 0.0, sweep: TAU }));
    many.wires = vec![SectionWire { curves: vec![used(0, true)], closed: true }; 201];
    many.regions = vec![SectionRegion { outer: 0, holes: (1..=200).collect() }; 200];
    many.validate_structure(TOL).unwrap();
    let last = many.regions.last_mut().unwrap();
    *last.holes.last_mut().unwrap() = 1;
    assert_eq!(many.validate_structure(TOL), Err(Invalid("duplicate region boundary")));
}

#[test]
fn aggregate_counts_fail_before_geometry_or_reference_inspection() {
    let mut section = empty(Plane::XY);
    section.contacts = vec![Vec2::new(f64::NAN, 0.0); MAX_ITEMS + 1];
    assert_eq!(section.validate_structure(TOL), Err(Invalid("item budget")));
    let mut section = empty(Plane::XY);
    section.wires.push(SectionWire { curves: vec![used(u32::MAX, false); MAX_ITEMS], closed: false });
    assert_eq!(section.validate_structure(TOL), Err(Invalid("item budget")));
    let mut section = empty(Plane::XY);
    section.regions.push(SectionRegion { outer: u32::MAX, holes: vec![u32::MAX; MAX_ITEMS] });
    assert_eq!(section.validate_structure(TOL), Err(Invalid("item budget")));
    let mut section = empty(Plane::XY);
    section.contacts = vec![Vec2::ZERO; MAX_ITEMS / 2];
    section.wires.push(SectionWire { curves: vec![used(u32::MAX, false); MAX_ITEMS / 2], closed: false });
    assert_eq!(section.validate_structure(TOL), Err(Invalid("item budget")));
    // Exactly the item limit is accepted, independently of these points' material meaning.
    let mut boundary = empty(Plane::XY);
    boundary.contacts = vec![Vec2::ZERO; MAX_ITEMS];
    boundary.validate_structure(TOL).unwrap();
    let mut count = Budget(MAX_ITEMS);
    assert_eq!(count.add(usize::MAX), Err(Invalid("item budget")));
}

#[test]
fn closure_tolerance_is_bounded_and_not_used_to_collapse_short_curves() {
    for tolerance in [f64::NAN, f64::INFINITY, -1.0, 0.0, 1e-10, 1.01] {
        assert!(plate().validate_structure(tolerance).is_err());
    }
    for tolerance in [1e-9, 1.0] {
        plate().validate_structure(tolerance).unwrap();
    }
    let mut section = empty(Plane::XY);
    section.curves.push(analytic(Seg2::Line { a: Vec2::ZERO, b: Vec2::new(1e-10, 0.0) }));
    section.validate_structure(1.0).unwrap();
    let mut gap = plate();
    if let Seg2::Line { b, .. } = &mut gap.curves[3].geometry {
        b.x = 5e-7;
    }
    gap.validate_structure(1e-6).unwrap();
    assert_eq!(gap.validate_structure(1e-8), Err(Invalid("open closed wire")));
}

#[test]
fn curve_number_and_domain_errors_are_rejected_before_evaluation() {
    let p = Vec2::new;
    let arc = |radius, start, sweep| Seg2::Arc { center: Vec2::ZERO, radius, start, sweep };
    let conic = |w| Seg2::Conic { a: Vec2::X, apex: p(1.0, 1.0), b: Vec2::Y, w };
    let invalid = [
        Seg2::Line { a: Vec2::ZERO, b: Vec2::ZERO },
        Seg2::Line { a: Vec2::ZERO, b: p(f64::NAN, 0.0) },
        Seg2::Line { a: Vec2::ZERO, b: p(f64::MAX, 0.0) },
        arc(-1.0, 0.0, PI),
        arc(0.0, 0.0, PI),
        arc(1e-13, 0.0, PI),
        arc(1e12 + 1.0, 0.0, PI),
        arc(f64::INFINITY, 0.0, PI),
        arc(1.0, f64::NAN, PI),
        arc(1.0, 1e6 + 1.0, PI),
        arc(1.0, 0.0, 0.0),
        arc(1.0, 0.0, 1e-13),
        arc(1.0, 0.0, TAU + 1e-8),
        arc(1.0, 0.0, f64::INFINITY),
        conic(-0.5),
        conic(0.0),
        conic(1e-13),
        conic(1e12 + 1.0),
        conic(f64::NAN),
        Seg2::Cubic { p0: Vec2::X, p1: Vec2::X, p2: Vec2::X, p3: Vec2::X },
        Seg2::Conic { a: Vec2::X, apex: Vec2::X, b: Vec2::X, w: 1.0 },
    ];
    for geometry in invalid {
        let mut section = empty(Plane::XY);
        section.curves.push(analytic(geometry));
        assert!(section.validate_structure(TOL).is_err(), "accepted {geometry:?}");
    }
    let mut section = empty(Plane::XY);
    section.contacts.push(p(f64::INFINITY, 0.0));
    assert!(section.validate_structure(TOL).is_err());
}

#[test]
fn bezier_degree_and_every_serialized_control_slot_are_validated() {
    for degree in [4u8, 9] {
        let mut p = [Vec2::ZERO; MAX_BEZIER_CTRL];
        for (i, point) in p.iter_mut().enumerate().take(usize::from(degree) + 1) {
            *point = Vec2::new(i as f64, (i % 2) as f64);
        }
        let mut section = empty(Plane::XY);
        section.curves.push(analytic(Seg2::Bezier { n: degree, p }));
        section.validate_structure(TOL).unwrap();
        for wrong in [0, 3, 10, u8::MAX] {
            section.curves[0].geometry = Seg2::Bezier { n: wrong, p };
            assert_eq!(section.validate_structure(TOL), Err(Invalid("Bezier degree")));
        }
    }
    let mut p = [Vec2::ZERO; MAX_BEZIER_CTRL];
    p[4] = Vec2::X;
    for value in [f64::NAN, f64::INFINITY, 1e12 + 1.0] {
        p[MAX_BEZIER_CTRL - 1] = Vec2::new(value, 0.0);
        let mut section = empty(Plane::XY);
        section.curves.push(analytic(Seg2::Bezier { n: 4, p }));
        assert_eq!(section.validate_structure(TOL), Err(Invalid("non-finite or excessive coordinate")));
    }
    let mut section = empty(Plane::XY);
    section.curves.push(analytic(Seg2::Bezier { n: 4, p: [Vec2::ZERO; MAX_BEZIER_CTRL] }));
    assert_eq!(section.validate_structure(TOL), Err(Invalid("collapsed curve")));
}

#[test]
fn supported_curve_limits_keep_evaluation_finite_without_claiming_accuracy() {
    let p = Vec2::new;
    let mut section = empty(Plane::XY);
    section.curves = vec![
        analytic(Seg2::Line { a: p(-1e12, 1e12), b: p(1e12, -1e12) }),
        analytic(Seg2::Arc { center: p(1e12, 1e12), radius: 1e12, start: 1e6, sweep: -TAU }),
        analytic(Seg2::Arc { center: Vec2::ZERO, radius: 1e-12, start: 0.0, sweep: 1e-12 }),
        analytic(Seg2::Conic { a: p(-1e12, 0.0), apex: p(0.0, 1e12), b: p(1e12, 0.0), w: 1e12 }),
        analytic(Seg2::Conic { a: p(-1e12, 0.0), apex: p(0.0, 1e12), b: p(1e12, 0.0), w: 1e-12 }),
        analytic(bezier(&[p(0.0, 0.0), p(1e12, 0.0), p(0.0, 1e12), p(-1e12, 0.0)]).unwrap()),
    ];
    let controls: Vec<Vec2> = (0..=9).map(|i| p(i as f64 * 1e11, if i % 2 == 0 { 1e12 } else { -1e12 })).collect();
    section.curves.push(analytic(bezier(&controls).unwrap()));
    section.validate_structure(TOL).unwrap();
    for curve in &section.curves {
        for t in [0.0, 0.5, 1.0] {
            assert!(curve.geometry.point_at(t).is_finite());
            assert!(curve.geometry.derivative(t).is_finite());
            assert!(curve.geometry.derivatives(t, 4).is_some_and(|d| d.iter().all(|v| v.is_finite())));
        }
    }
}

#[test]
fn transient_serialization_preserves_accuracy_tags_and_plane_names() {
    let mut section = plate();
    section.curves[1].accuracy = CurveAccuracy::Bounded { max_deviation_mm: 0.01 };
    section.curves[2].accuracy = CurveAccuracy::Sampled { max_sample_deviation_mm: 0.0 };
    section.validate_structure(TOL).unwrap();
    let value = serde_json::to_value(&section).unwrap();
    assert!(value["plane"].get("x_dir").is_some());
    assert!(value["plane"].get("y_dir").is_some());
    assert_eq!(value["curves"][0]["accuracy"], serde_json::json!({"kind": "analytic"}));
    assert_eq!(value["curves"][1]["accuracy"], serde_json::json!({"kind": "bounded", "max_deviation_mm": 0.01}));
    assert_eq!(value["curves"][2]["accuracy"], serde_json::json!({"kind": "sampled", "max_sample_deviation_mm": 0.0}));
    let restored: SectionGeometry = serde_json::from_value(value.clone()).unwrap();
    restored.validate_structure(TOL).unwrap();
    assert_eq!(section, restored);
    let mut unknown = value;
    unknown["certificate"] = serde_json::json!(true);
    assert!(serde_json::from_value::<SectionGeometry>(unknown).is_err());
    assert!(serde_json::from_value::<CurveAccuracy>(serde_json::json!({"kind": "bounded", "max_deviation_mm": 0.1, "exact": true})).is_err());
    assert!(serde_json::from_value::<CurveAccuracy>(serde_json::json!({"kind": "exact"})).is_err());
}

#[test]
fn accuracy_values_are_validated_without_promoting_zero_sampled_error() {
    for value in [f64::NAN, f64::INFINITY, -0.01, 1e12 + 1.0] {
        for accuracy in [CurveAccuracy::Bounded { max_deviation_mm: value }, CurveAccuracy::Sampled { max_sample_deviation_mm: value }] {
            let mut section = plate();
            section.curves[0].accuracy = accuracy;
            assert_eq!(section.validate_structure(TOL), Err(Invalid("accuracy deviation")));
        }
    }
    for value in [0.0, 1e12] {
        let mut section = plate();
        section.curves[0].accuracy = CurveAccuracy::Sampled { max_sample_deviation_mm: value };
        section.validate_structure(TOL).unwrap();
        section.to_region(0, TOL).unwrap();
        assert_eq!(section.curves[0].accuracy, CurveAccuracy::Sampled { max_sample_deviation_mm: value });
    }
}

#[test]
fn conversion_revalidates_mutated_data_and_does_not_trust_an_earlier_success() {
    let mut section = plate();
    section.validate_structure(TOL).unwrap();
    section.curves[0].geometry = Seg2::Line { a: Vec2::ZERO, b: Vec2::new(f64::NAN, 0.0) };
    assert_eq!(section.to_loop(0, TOL), Err(Invalid("non-finite or excessive coordinate")));
    assert_eq!(section.to_region(0, TOL), Err(Invalid("non-finite or excessive coordinate")));
    let mut section = plate();
    section.to_region(0, TOL).unwrap();
    section.contacts = vec![Vec2::ZERO; MAX_ITEMS];
    assert_eq!(section.to_region(0, TOL), Err(Invalid("item budget")));
}

#[test]
fn reversed_large_arc_conversion_rechecks_actual_connectivity_without_snapping() {
    let arc = Seg2::Arc { center: Vec2::ZERO, radius: 1e12, start: 0.1, sweep: 1.0 };
    let mut section = empty(Plane::XY);
    section.curves = vec![analytic(arc), analytic(Seg2::Line { a: arc.start(), b: arc.end() })];
    section.wires.push(SectionWire { curves: vec![used(0, false), used(1, true)], closed: true });
    section.regions.push(SectionRegion { outer: 0, holes: vec![] });
    let before = section.clone();
    // Source endpoints coincide exactly, but (start + sweep) - sweep changes the reversed
    // arc's endpoint by more than this tolerance at this radius.
    section.validate_structure(1e-9).unwrap();
    assert!(arc.reversed().end().dist(arc.start()) > 1e-9);
    assert_eq!(section.to_loop(0, 1e-9), Err(Invalid("converted loop is not closed within tolerance")));
    assert_eq!(section.to_region(0, 1e-9), Err(Invalid("converted loop is not closed within tolerance")));
    assert_eq!(section, before);
    // A caller's explicit larger tolerance can accept the same result without moving endpoints.
    let converted = section.to_loop(0, 0.01).unwrap();
    assert_eq!(converted.segs[0], arc.reversed());
    assert!(converted.is_closed(0.01));
}

#[test]
fn connected_bow_tie_and_wrong_winding_are_not_certified_or_silently_repaired() {
    let p = Vec2::new;
    let loop_ = Loop2::polygon(&[p(0.0, 0.0), p(2.0, 2.0), p(0.0, 2.0), p(2.0, 0.0)]);
    let mut bow = empty(Plane::XY);
    bow.curves = loop_.segs.iter().copied().map(analytic).collect();
    bow.wires.push(SectionWire { curves: (0..4).map(|i| used(i, true)).collect(), closed: true });
    bow.regions.push(SectionRegion { outer: 0, holes: vec![] });
    bow.validate_structure(TOL).unwrap();
    assert_eq!(bow.to_loop(0, TOL).unwrap(), loop_);
    // A successful structural check does not establish a material region, including its winding.
    let mut reversed = plate();
    reversed.wires[0].curves.reverse();
    for use_ in &mut reversed.wires[0].curves {
        use_.forward = !use_.forward;
    }
    reversed.validate_structure(TOL).unwrap();
    let converted = reversed.to_region(0, TOL).unwrap();
    assert!((converted.outer.signed_area() + 6000.0).abs() < 1e-10);
}
