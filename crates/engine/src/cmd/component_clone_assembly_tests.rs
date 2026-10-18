#![allow(clippy::unwrap_used, clippy::panic)]
use super::*;
use solidcraft_doc::joints::{Coupling, Joint, JointKind, MotionLink, NamedOrigin, Size, Snap, XRef};
use solidcraft_doc::motion::{ContactSet, ExplodedView, MotionStudy, StudyKey};
use solidcraft_doc::{IDENTITY, Occurrence};
use solidcraft_geom::Vec3;

fn at(occurrence: u64) -> JointOrigin {
    JointOrigin { occurrence, snap: Snap::Point { point: Vec3::ZERO, z: None } }
}

fn joint(id: u64, a: u64, b: u64) -> Joint {
    Joint {
        id,
        name: format!("Joint{id}"),
        kind: JointKind::Revolute,
        a: at(a),
        b: at(b),
        values: vec![0.25],
        limits: vec![Some((-1.0, 1.0))],
        flip: true,
        offset: 2.0,
        angle: 0.1,
        suppressed: false,
    }
}

fn occurrence(id: u64) -> Occurrence {
    Occurrence { id, component: id, parent: 0, name: format!("Occurrence{id}"), transform: IDENTITY, grounded: false }
}

fn documents() -> (Document, Document) {
    let mut source = Document { occurrences: [1, 2, 8, 9].map(occurrence).to_vec(), ..Document::default() };
    source.assembly.joints = vec![joint(1, 1, 2), joint(2, 1, 2), joint(3, 2, 9), joint(4, 8, 9)];
    source.assembly.origins = vec![
        NamedOrigin {
            name: "Typed origin".into(),
            origin: JointOrigin {
                occurrence: 1,
                snap: Snap::FaceCenter {
                    pick: Vec3::new(1.0, 2.0, 3.0),
                    face: Some("F1:start@F10.2".into()),
                    x: Some(XRef::Face { face: "F10:side".into(), pick: None }),
                },
            },
        },
        NamedOrigin {
            name: "Foreign geometry".into(),
            origin: JointOrigin { occurrence: 1, snap: Snap::CircleCenter { pick: Vec3::ZERO, face: Some("F99:end".into()), end: None, x: None } },
        },
        NamedOrigin { name: "Outside".into(), origin: at(9) },
    ];
    source.assembly.links = vec![
        MotionLink { a: 1, ia: 0, b: 2, ib: 0, ratio: -2.0, offset: 0.3, coupling: None, lift: Vec::new() },
        MotionLink { a: 1, ia: 0, b: 3, ib: 0, ratio: 1.0, offset: 0.0, coupling: None, lift: Vec::new() },
    ];
    source.assembly.contact_sets = vec![
        ContactSet { id: 1, name: "Internal contact".into(), occurrences: vec![1, 2], suppressed: true },
        ContactSet { id: 2, name: "Cross contact".into(), occurrences: vec![1, 9], suppressed: false },
    ];
    let key = |joint| StudyKey { joint, index: 0, points: vec![(0.0, 0.0), (10.0, 0.5)] };
    source.assembly.studies = vec![
        MotionStudy { id: 3, name: "Internal study".into(), steps: 10, keys: vec![key(1), key(2)] },
        MotionStudy { id: 4, name: "Cross study".into(), steps: 10, keys: vec![key(1), key(3)] },
    ];
    source.assembly.exploded = vec![
        ExplodedView { id: 5, name: "Internal explode".into(), moves: vec![(1, Vec3::X * 10.0), (2, Vec3::Y * 20.0)] },
        ExplodedView { id: 6, name: "Cross explode".into(), moves: vec![(1, Vec3::X), (9, Vec3::Y)] },
    ];
    let mut destination = source.clone();
    destination.occurrences.extend([101, 102].map(occurrence));
    (source, destination)
}

#[test]
fn internal_relationships_receive_independent_ids_and_remapped_typed_references() {
    let (source, mut destination) = documents();
    let report =
        clone_assembly(&mut destination, &source, &BTreeMap::from([(1, 101), (2, 102)]), &BTreeMap::from([(1, 11), (10, 110)]), &BTreeMap::new())
            .unwrap();
    let a = &destination.assembly;
    assert_eq!(a.joints.len(), 6);
    assert_eq!((a.joints[4].id, a.joints[5].id), (5, 6));
    assert_eq!((a.joints[4].a.occurrence, a.joints[4].b.occurrence), (101, 102));
    assert_eq!(a.joints[4].values, source.assembly.joints[0].values);
    assert_eq!(a.joints[4].limits, source.assembly.joints[0].limits);
    assert_eq!(a.joints[4].name, "Joint1 (copy)");
    assert_eq!(a.origins.len(), 4);
    // Its x reference's face is the copy's too.
    assert!(matches!(&a.origins[3].origin.snap, Snap::FaceCenter { face: Some(face), x: Some(XRef::Face { face: x, .. }), .. }
        if face == "F11:start@F110.2" && x == "F110:side"));
    assert_eq!((a.links[2].a, a.links[2].b, a.links[2].ratio), (5, 6, -2.0));
    assert_eq!(a.contact_sets[2].id, 7);
    assert_eq!(a.contact_sets[2].occurrences, vec![101, 102]);
    assert!(a.contact_sets[2].suppressed);
    assert_eq!(a.studies[2].id, 8);
    assert_eq!(a.studies[2].keys.iter().map(|k| k.joint).collect::<Vec<_>>(), vec![5, 6]);
    assert_eq!(a.exploded[2].id, 9);
    assert_eq!(a.exploded[2].moves, vec![(101, Vec3::X * 10.0), (102, Vec3::Y * 20.0)]);
    assert_eq!(report.len(), 6);
    for kind in ["joint origin", "joint `Joint3`", "motion link", "contact set", "motion study", "exploded view"] {
        assert!(report.iter().any(|m| m.contains(kind)), "{kind}: {report:?}");
    }
    assert!(!report.iter().any(|m| m.contains("Outside") || m.contains("Joint4")));
    assert_eq!(destination.assembly.joints[..4], source.assembly.joints);
    destination.assembly.joints[4].values[0] = 0.75;
    assert_eq!(source.assembly.joints[0].values[0], 0.25);
}

/// A gear's picked round face is on the copy's occurrence and face; one picked on geometry the
/// copy doesn't have is reported, not pointed back at the original.
#[test]
fn copied_gears_take_their_picked_faces_along() {
    let (mut source, _) = documents();
    let circle = |face: &str| {
        Size::Circle(JointOrigin {
            occurrence: 1,
            snap: Snap::CircleCenter { pick: Vec3::X, face: Some(face.into()), end: Some("F1:top".into()), x: None },
        })
    };
    let gear = |ra| Coupling::Gear { ra, rb: Size::Length(5.0), internal: false, reverse: false };
    source.assembly.links = vec![
        MotionLink { a: 1, ia: 0, b: 2, ib: 0, ratio: -2.0, offset: 0.0, coupling: Some(gear(circle("F1:side"))), lift: Vec::new() },
        MotionLink { a: 2, ia: 0, b: 1, ib: 0, ratio: -0.5, offset: 0.0, coupling: Some(gear(circle("F99:side"))), lift: Vec::new() },
    ];
    let mut destination = source.clone();
    destination.occurrences.extend([101, 102].map(occurrence));
    let report =
        clone_assembly(&mut destination, &source, &BTreeMap::from([(1, 101), (2, 102)]), &BTreeMap::from([(1, 11), (10, 110)]), &BTreeMap::new())
            .unwrap();
    let copied = destination.assembly.links.get(2).and_then(|l| l.coupling.clone());
    let Some(Coupling::Gear { ra: Size::Circle(o), .. }) = copied else { panic!("{:?}", destination.assembly.links) };
    assert!(
        o.occurrence == 101 && matches!(&o.snap, Snap::CircleCenter { face: Some(f), end: Some(e), .. } if f == "F11:side" && e == "F11:top"),
        "{o:?}"
    );
    assert_eq!(destination.assembly.links.len(), 3, "the gear on foreign geometry stays behind");
    assert!(report.iter().any(|m| m.contains("motion link")), "{report:?}");
}

#[test]
fn naming_remaps_exact_tokens_unicode_and_legacy_body_identity() {
    let features = BTreeMap::from([(1, 21), (10, 30)]);
    let bodies = BTreeMap::from([("Wheel: inner".into(), "Wheel copy: inner".into())]);
    assert_eq!(face_name("F1:blend:F10:start@F1.3", &features, &bodies).as_deref(), Some("F21:blend:F30:start@F21.3"));
    assert_eq!(face_name("F1:side:曲線F10", &features, &bodies).as_deref(), Some("F21:side:曲線F10"));
    assert_eq!(face_name("F1:side:F100.7", &features, &bodies).as_deref(), Some("F21:side:F100.7"));
    assert_eq!(face_name("Wheel: inner:face:7", &features, &bodies).as_deref(), Some("Wheel copy: inner:face:7"));
    assert!(face_name("F100:start", &features, &bodies).is_none());
    assert!(face_name("F1:start@F99.1", &features, &bodies).is_none());
    assert!(face_name("Foreign:face:0", &features, &bodies).is_none());
    assert!(face_name("Foreign:face:0@F1.1", &features, &bodies).is_none());
    let bodies = BTreeMap::from([("F1:Housing".into(), "F1:Housing (copy)".into())]);
    assert_eq!(face_name("F1:Housing:face:0@F10.1", &features, &bodies).as_deref(), Some("F1:Housing (copy):face:0@F30.1"));
}

#[test]
fn malformed_maps_identifier_exhaustion_and_aggregate_budgets_are_atomic() {
    let (source, destination) = documents();
    let mut cases = Vec::new();
    cases.push((source.clone(), destination.clone(), BTreeMap::from([(1, 101), (2, 101)])));
    cases.push((source.clone(), destination.clone(), BTreeMap::from([(1, 999)])));
    let mut overflow = destination.clone();
    overflow.assembly.joints[0].id = u64::MAX;
    cases.push((source.clone(), overflow, BTreeMap::from([(1, 101), (2, 102)])));
    let mut oversized = source.clone();
    oversized.assembly.contact_sets[0].occurrences = vec![1; MAX_REFERENCES + 1];
    cases.push((oversized, destination.clone(), BTreeMap::from([(1, 101), (2, 102)])));
    let mut invalid = source.clone();
    invalid.assembly.links[0].ratio = f64::NAN;
    cases.push((invalid, destination, BTreeMap::from([(1, 101), (2, 102)])));
    for (source, mut destination, occurrences) in cases {
        let before = destination.assembly.clone();
        assert!(clone_assembly(&mut destination, &source, &occurrences, &BTreeMap::from([(1, 11), (10, 110)]), &BTreeMap::new()).is_err());
        assert_eq!(destination.assembly, before);
    }
}

#[test]
fn second_copy_has_unique_names_and_keeps_global_contact_switches_unchanged() {
    let (source, mut destination) = documents();
    destination.assembly.contact_enabled = true;
    destination.assembly.contact_all = true;
    let features = BTreeMap::from([(1, 11), (10, 110)]);
    clone_assembly(&mut destination, &source, &BTreeMap::from([(1, 101), (2, 102)]), &features, &BTreeMap::new()).unwrap();
    destination.occurrences.extend([201, 202].map(occurrence));
    clone_assembly(&mut destination, &source, &BTreeMap::from([(1, 201), (2, 202)]), &features, &BTreeMap::new()).unwrap();
    assert_eq!(destination.assembly.joints[6].name, "Joint1 (copy 2)");
    assert!(destination.assembly.contact_enabled && destination.assembly.contact_all);
    let names: BTreeSet<&str> = destination.assembly.origins.iter().map(|x| x.name.as_str()).collect();
    assert_eq!(names.len(), destination.assembly.origins.len());
}

#[test]
fn paste_new_internal_motion_study_drives_only_the_copied_assembly_and_reopens() {
    use serde_json::{Value, json};
    fn run(s: &mut crate::Session, command: &str, params: Value) -> Value {
        s.execute(command, &params).unwrap_or_else(|e| panic!("{command}: {e}"))
    }
    let mut s = crate::Session::default();
    let parent = run(&mut s, "component.create", json!({"name":"Mechanism"}))["component"].as_u64().unwrap();
    let fixed = run(&mut s, "component.create", json!({"name":"Support"}))["occurrence"].as_u64().unwrap();
    run(&mut s, "solid.box", json!({"length":2,"width":2,"height":2,"body_name":"SupportBox"}));
    run(&mut s, "occurrence.ground", json!({"occurrence":fixed,"grounded":true}));
    run(&mut s, "component.activate", json!({"component":parent}));
    let moving = run(&mut s, "component.create", json!({"name":"Carriage"}))["occurrence"].as_u64().unwrap();
    run(&mut s, "solid.box", json!({"length":2,"width":2,"height":2,"corner":[0,0,5],"body_name":"CarriageBox"}));
    run(&mut s, "joint.origin", json!({"name":"Support origin","occurrence":fixed,"point":[0,0,0]}));
    let source_joint = run(&mut s, "joint.as_built", json!({"type":"slider","a":fixed,"b":moving,"name":"Rail"}))["joint"].as_u64().unwrap();
    run(&mut s, "contact.create", json!({"name":"Internal contact","occurrences":[fixed,moving]}));
    run(&mut s, "motion.study", json!({"name":"Travel","steps":10,"keys":[{"joint":source_joint,"points":[[0,0],[10,6]]}]}));
    run(
        &mut s,
        "explode.create",
        json!({"name":"Exploded mechanism","moves":[{"occurrence":fixed,"translate":[10,0,0]},{"occurrence":moving,"translate":[0,10,0]}]}),
    );
    let original = s.doc.occurrences.iter().find(|o| o.id == moving).unwrap().transform;
    run(&mut s, "component.activate", json!({"component":"root"}));
    let result = run(&mut s, "component.paste_new", json!({"component":parent,"translate":[20,0,0]}));
    assert_eq!(result["notices"], json!([]));
    assert_eq!(s.doc.assembly.joints.len(), 2);
    assert_eq!(s.doc.assembly.origins.len(), 2);
    assert_eq!(s.doc.assembly.contact_sets.len(), 2);
    assert_eq!(s.doc.assembly.exploded.len(), 2);
    let copied_joint = s.doc.assembly.joints.iter().find(|j| j.name == "Rail (copy)").unwrap().id;
    let copied_moving = s.doc.assembly.joints.iter().find(|j| j.id == copied_joint).unwrap().b.occurrence;
    assert_ne!(copied_moving, moving);
    run(&mut s, "motion.play", json!({"study":"Travel (copy)","step":10}));
    assert_eq!(s.doc.assembly.joints.iter().find(|j| j.id == source_joint).unwrap().values, vec![0.0]);
    assert_eq!(s.doc.assembly.joints.iter().find(|j| j.id == copied_joint).unwrap().values, vec![6.0]);
    assert_eq!(s.doc.occurrences.iter().find(|o| o.id == moving).unwrap().transform, original);
    let reopened = Document::from_json(&s.doc.to_json()).unwrap();
    assert_eq!(reopened.assembly, s.doc.assembly);
    assert!(reopened.assembly.studies.iter().any(|m| m.name == "Travel (copy)" && m.keys[0].joint == copied_joint));
}
