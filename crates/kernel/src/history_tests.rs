use solidcraft_geom::Vec3;

use super::{Change, EdgeOrigin, EdgeProv, FaceOrigin, FaceProv, History, Side};
use crate::{Body, box_solid, transform};

fn face(operand: usize, face: usize) -> FaceOrigin {
    FaceOrigin::Face { operand, face }
}

fn prov(origins: Vec<FaceOrigin>, change: Change) -> FaceProv {
    FaceProv { origins, change }
}

fn centroids(b: &Body) -> Vec<Vec3> {
    b.faces(1e-3).unwrap().into_iter().map(|f| f.centroid).collect()
}

#[test]
fn a_transform_keeps_every_face_and_edge_in_place() {
    let a = box_solid(Vec3::ZERO, Vec3::new(40.0, 30.0, 10.0)).unwrap();
    let t = Vec3::new(5.0, -3.0, 2.0);
    let moved = transform(&a, t, Vec3::ZERO, Vec3::Z, 0.7).unwrap();
    let h = moved.history().unwrap();
    h.check(&moved, &[&a]).unwrap();
    assert!(h.faces.iter().enumerate().all(|(i, f)| f.origins == vec![face(0, i)] && f.change == Change::Kept));
    // Face i of the result is face i of the input, moved.
    let rot = |p: Vec3| {
        let (s, c) = 0.7f64.sin_cos();
        Vec3::new(p.x * c - p.y * s, p.x * s + p.y * c, p.z) + t
    };
    for (before, after) in centroids(&a).into_iter().zip(centroids(&moved)) {
        assert!(rot(before).dist(after) < 1e-6, "{before:?} → {after:?}");
    }
    // Each edge separates the same faces it did.
    let (ea, em) = (super::edge_faces(&a.solid), super::edge_faces(&moved.solid));
    assert_eq!(ea, em);
    assert!(h.edges.iter().all(|e| e.faces.len() == 2 && e.origins.len() == 1));
    // The history's edges are the tessellation's edges, bounding the same faces.
    let mesh = moved.tessellate(1e-3).unwrap();
    let mesh_faces: Vec<Vec<usize>> = mesh.edge_faces.iter().map(|fs| fs.iter().map(|f| *f as usize).collect()).collect();
    assert_eq!(mesh_faces, h.edges.iter().map(|e| e.faces.clone()).collect::<Vec<_>>());
}

/// Joining an extrusion: the tool came from a sketch (no operand bodies), so substituting its
/// history removes it from the operand list and its faces become generated profile faces.
#[test]
fn substitute_a_body_made_from_a_sketch() {
    let extrude = History {
        operands: 0,
        faces: vec![prov(vec![FaceOrigin::Cap { end: false }], Change::Generated), prov(vec![FaceOrigin::Profile { segment: 3 }], Change::Generated)],
        edges: vec![EdgeProv { faces: vec![0, 1], origins: vec![] }],
    };
    let mut join = History {
        operands: 2,
        faces: vec![prov(vec![face(0, 2)], Change::Modified), prov(vec![face(1, 1)], Change::Kept)],
        edges: vec![EdgeProv { faces: vec![0, 1], origins: vec![EdgeOrigin::meet(face(0, 2), face(1, 1))] }],
    };
    join.substitute(1, &extrude).unwrap();
    assert_eq!(join.operands, 1);
    assert_eq!(join.faces[0], prov(vec![face(0, 2)], Change::Modified));
    assert_eq!(join.faces[1], prov(vec![FaceOrigin::Profile { segment: 3 }], Change::Generated));
    assert_eq!(join.edges[0].origins, vec![EdgeOrigin::meet(face(0, 2), FaceOrigin::Profile { segment: 3 })]);
}

#[test]
fn bodies_without_reported_history_have_none() {
    let a = box_solid(Vec3::ZERO, Vec3::new(4.0, 3.0, 1.0)).unwrap();
    let bare = Body::new(a.deep_copy()).unwrap();
    assert!(bare.history().is_none());
}

#[test]
fn check_refuses_a_history_that_does_not_describe_the_body() {
    let a = box_solid(Vec3::ZERO, Vec3::new(4.0, 3.0, 1.0)).unwrap();
    let mut h = History::identity(&a);
    h.check(&a, &[&a]).unwrap();
    assert!(h.check(&a, &[]).is_err(), "operand count");
    let mut short = h.clone();
    short.faces.pop();
    assert!(short.check(&a, &[&a]).is_err(), "face count");
    let mut reversed = h.clone();
    reversed.edges[0].origins = vec![EdgeOrigin::Meet { a: face(0, 2).into(), b: face(0, 1).into() }];
    assert!(reversed.check(&a, &[&a]).is_err(), "meeting sides out of order");
    reversed.edges[0].origins = vec![EdgeOrigin::meet(face(0, 2), Side::new([face(0, 1), face(0, 6)]).unwrap())];
    assert!(reversed.check(&a, &[&a]).is_err(), "a side's origin beyond the operand's faces");
    reversed.edges[0].origins = vec![EdgeOrigin::meet(face(0, 2), Side::new([face(0, 1), face(0, 5)]).unwrap())];
    reversed.check(&a, &[&a]).unwrap();
    h.faces[0].origins = vec![face(0, 6)];
    assert!(h.check(&a, &[&a]).is_err(), "origin beyond the operand's faces");
    assert!(a.clone().with_history(h, &[&a]).is_err());
}

/// A two-step chain: op1 makes B from (A0, A1); op2 makes C from (X, B, Y). Composing puts
/// A0, A1 in B's place: C's operands become (X, A0, A1, Y).
#[test]
fn substitute_composes_kept_split_merged_and_generated_faces() {
    let op1 = History {
        operands: 2,
        faces: vec![
            prov(vec![face(0, 0)], Change::Kept),                              // B0 = A0.f0
            prov(vec![face(0, 1)], Change::Split),                             // B1, B2 = pieces of A0.f1
            prov(vec![face(0, 1)], Change::Split),                             //
            prov(vec![face(1, 4)], Change::Modified),                          // B3 = A1.f4 trimmed
            prov(vec![FaceOrigin::Profile { segment: 2 }], Change::Generated), // B4 new
        ],
        edges: vec![
            EdgeProv { faces: vec![0, 1], origins: vec![EdgeOrigin::Edge { operand: 0, edge: 7 }] },
            EdgeProv { faces: vec![3, 4], origins: vec![EdgeOrigin::meet(face(1, 4), FaceOrigin::Profile { segment: 2 })] },
        ],
    };
    let mut op2 = History {
        operands: 3,
        faces: vec![
            prov(vec![face(1, 0)], Change::Kept),                                    // kept B0 → kept A0.f0
            prov(vec![face(1, 1), face(1, 2)], Change::Merged),                      // B1 + B2 rejoined → one origin
            prov(vec![face(1, 3)], Change::Kept),                                    // kept B3 → modified A1.f4
            prov(vec![face(1, 4)], Change::Modified),                                // trimmed generated face
            prov(vec![face(0, 5), face(2, 1)], Change::Merged),                      // X and Y faces renumbered
            prov(vec![FaceOrigin::Edge { operand: 1, edge: 0 }], Change::Generated), // blend of B's edge 0
        ],
        edges: vec![
            EdgeProv { faces: vec![0, 1], origins: vec![EdgeOrigin::Edge { operand: 1, edge: 1 }] },
            EdgeProv { faces: vec![2, 4], origins: vec![EdgeOrigin::meet(face(1, 3), face(2, 1))] },
            EdgeProv { faces: vec![4, 5], origins: vec![EdgeOrigin::Edge { operand: 2, edge: 3 }] },
        ],
    };
    op2.substitute(1, &op1).unwrap();
    assert_eq!(op2.operands, 4);
    let f = &op2.faces;
    assert_eq!(f[0], prov(vec![face(1, 0)], Change::Kept));
    // Two pieces of one face, merged back: one origin, so not a merge across faces, and no
    // other face comes from it, so not split.
    assert_eq!(f[1], prov(vec![face(1, 1)], Change::Modified));
    assert_eq!(f[2], prov(vec![face(2, 4)], Change::Modified));
    assert_eq!(f[3], prov(vec![FaceOrigin::Profile { segment: 2 }], Change::Generated));
    assert_eq!(f[4], prov(vec![face(0, 5), face(3, 1)], Change::Merged));
    assert_eq!(f[5], prov(vec![FaceOrigin::Edge { operand: 1, edge: 7 }], Change::Generated));
    let e = &op2.edges;
    assert_eq!(e[0].origins, vec![EdgeOrigin::meet(face(2, 4), FaceOrigin::Profile { segment: 2 })]);
    assert_eq!(e[1].origins, vec![EdgeOrigin::meet(face(2, 4), face(3, 1))]);
    assert_eq!(e[2].origins, vec![EdgeOrigin::Edge { operand: 3, edge: 3 }]);
}

#[test]
fn substitute_refuses_lineage_it_cannot_express_and_leaves_history_unchanged() {
    let inner = History {
        operands: 1,
        faces: vec![prov(vec![FaceOrigin::Profile { segment: 0 }], Change::Generated)],
        edges: vec![EdgeProv { faces: vec![0], origins: vec![] }],
    };
    // An offset of a face the inner operation generated.
    let mut offset = History { operands: 1, faces: vec![prov(vec![FaceOrigin::Offset { operand: 0, face: 0 }], Change::Generated)], edges: vec![] };
    let before = offset.clone();
    assert!(offset.substitute(0, &inner).is_err());
    assert_eq!(offset, before);
    // A blend of an edge the inner operation generated.
    let mut blend = History { operands: 1, faces: vec![prov(vec![FaceOrigin::Edge { operand: 0, edge: 0 }], Change::Generated)], edges: vec![] };
    assert!(blend.substitute(0, &inner).is_err());
    // A meeting with a face the inner operation made from nothing.
    let bare = History { operands: 0, faces: vec![prov(vec![], Change::Generated)], edges: vec![] };
    let mut meet = History {
        operands: 1,
        faces: vec![prov(vec![face(0, 0)], Change::Modified), prov(vec![FaceOrigin::Cap { end: true }], Change::Generated)],
        edges: vec![EdgeProv { faces: vec![0, 1], origins: vec![EdgeOrigin::meet(face(0, 0), FaceOrigin::Cap { end: true })] }],
    };
    let before = meet.clone();
    assert!(meet.substitute(0, &bare).is_err());
    assert_eq!(meet, before);
    // Out-of-range references and operands.
    let mut bad = History { operands: 1, faces: vec![prov(vec![face(0, 9)], Change::Kept)], edges: vec![] };
    assert!(bad.substitute(0, &inner).is_err());
    assert!(bad.substitute(3, &inner).is_err());
}

/// A side is a set: sorted, without repeats, never empty; a meeting puts the lesser side first.
#[test]
fn sides_and_meetings_are_canonical() {
    let side = |o: &[FaceOrigin]| Side::new(o.iter().copied()).unwrap();
    assert_eq!(side(&[face(1, 2), face(0, 5), face(1, 2)]).origins(), [face(0, 5), face(1, 2)]);
    assert!(Side::new([]).is_none());
    assert_eq!(Side::from(face(0, 1)).single(), Some(face(0, 1)));
    assert_eq!(side(&[face(0, 1), face(0, 2)]).single(), None);
    let (x, y) = (side(&[face(1, 0)]), side(&[face(0, 3), face(0, 4)]));
    assert_eq!(EdgeOrigin::meet(x.clone(), y.clone()), EdgeOrigin::meet(y.clone(), x.clone()));
    assert_eq!(EdgeOrigin::meet(x.clone(), y.clone()), EdgeOrigin::Meet { a: y, b: x });
    assert_eq!(EdgeOrigin::meet(face(2, 0), face(1, 0)), EdgeOrigin::meet(face(1, 0), face(2, 0)));
}

/// An edge meeting a face the inner operation merged: that side becomes the merged face's
/// origins, and the other side stays one face.
#[test]
fn substitute_composes_a_meeting_with_a_merged_face() {
    // Heal-like: the inner operation joined faces 1 and 2 of its operand into its face 0.
    let inner = History {
        operands: 1,
        faces: vec![prov(vec![face(0, 1), face(0, 2)], Change::Merged), prov(vec![face(0, 0)], Change::Kept)],
        edges: vec![EdgeProv { faces: vec![0, 1], origins: vec![EdgeOrigin::Edge { operand: 0, edge: 4 }] }],
    };
    // Blend-like: a face in place of the inner edge 0, meeting both inner faces.
    let mut outer = History {
        operands: 1,
        faces: vec![
            prov(vec![face(0, 0)], Change::Modified),
            prov(vec![face(0, 1)], Change::Modified),
            prov(vec![FaceOrigin::Edge { operand: 0, edge: 0 }], Change::Generated),
        ],
        edges: vec![
            EdgeProv { faces: vec![0, 2], origins: vec![EdgeOrigin::meet(face(0, 0), FaceOrigin::Edge { operand: 0, edge: 0 })] },
            EdgeProv { faces: vec![1, 2], origins: vec![EdgeOrigin::meet(face(0, 1), FaceOrigin::Edge { operand: 0, edge: 0 })] },
        ],
    };
    outer.substitute(0, &inner).unwrap();
    let blend = Side::from(FaceOrigin::Edge { operand: 0, edge: 4 });
    let merged = Side::new([face(0, 2), face(0, 1)]).unwrap();
    assert_eq!(outer.faces[0], prov(vec![face(0, 1), face(0, 2)], Change::Merged));
    assert_eq!(outer.edges[0].origins, [EdgeOrigin::meet(blend.clone(), merged)]);
    assert_eq!(outer.edges[1].origins, [EdgeOrigin::meet(blend, face(0, 0))]);
    // Renumbering joins operands within a side, and keeps the meeting canonical.
    let mut two = History {
        operands: 2,
        faces: vec![],
        edges: vec![EdgeProv { faces: vec![], origins: vec![EdgeOrigin::meet(Side::new([face(0, 3), face(1, 3)]).unwrap(), face(1, 0))] }],
    };
    two.renumber(&[Some(1), Some(1)], 2).unwrap();
    assert_eq!(two.edges[0].origins, [EdgeOrigin::meet(face(1, 0), face(1, 3))]);
}

#[test]
fn face_sources_follow_kept_split_and_agreeing_merged_faces() {
    let a = box_solid(Vec3::ZERO, Vec3::new(4.0, 3.0, 1.0)).unwrap().with_box_certificate(7).unwrap();
    let src = |i: usize| a.face_sources().unwrap()[i].clone();
    assert!(src(0).is_some() && src(1).is_some() && src(0) != src(1));
    let h = History {
        operands: 1,
        faces: vec![
            prov(vec![face(0, 0)], Change::Kept),
            prov(vec![face(0, 1)], Change::Split),
            prov(vec![face(0, 0), face(0, 1)], Change::Merged),
            prov(vec![face(0, 2), face(0, 2)], Change::Merged),
            prov(vec![FaceOrigin::Cap { end: true }], Change::Generated),
        ],
        edges: vec![],
    };
    let s = h.face_sources(&[&a]);
    assert_eq!(s, vec![src(0), src(1), None, src(2), None]);
}

/// A side drawn in two collinear pieces (here the bottom edge, split at x = 10) sweeps into one
/// side face over both pieces, not two coplanar faces with an edge between them (#1788).
#[test]
fn collinear_profile_lines_extrude_into_one_side_face() {
    use solidcraft_geom::{Loop2, Plane, Region2, Vec2};
    let pts = [(0.0, 0.0), (10.0, 0.0), (20.0, 0.0), (20.0, 10.0), (0.0, 10.0)].map(|(x, y)| Vec2::new(x, y));
    let region = Region2 { outer: Loop2::polygon(&pts), holes: vec![] };
    // Plain `extrude` keeps a face per segment (sheet metal's flat pattern relies on it).
    assert_eq!(crate::extrude(&Plane::XY, std::slice::from_ref(&region), 0.0, 5.0).unwrap()[0].face_count(), 7);
    let body = crate::extrude_whole_sides(&Plane::XY, &[region], 0.0, 5.0).unwrap().pop().unwrap();
    assert_eq!(body.face_count(), 6, "a box has six faces");
    assert!((crate::measure(&body).unwrap().volume - 1000.0).abs() < 1e-9);
    let h = body.history().unwrap();
    h.check(&body, &[]).unwrap();
    let mut sides: Vec<Vec<FaceOrigin>> =
        h.faces.iter().map(|f| f.origins.clone()).filter(|o| matches!(o.first(), Some(FaceOrigin::Profile { .. }))).collect();
    sides.sort();
    assert_eq!(
        sides,
        vec![
            vec![FaceOrigin::Profile { segment: 0 }, FaceOrigin::Profile { segment: 1 }],
            vec![FaceOrigin::Profile { segment: 2 }],
            vec![FaceOrigin::Profile { segment: 3 }],
            vec![FaceOrigin::Profile { segment: 4 }],
        ],
        "the bottom side is generated by both its pieces"
    );
    // A corner is not a straight run: an L-shaped step keeps its faces.
    let step = [(0.0, 0.0), (10.0, 0.0), (10.0, 5.0), (20.0, 5.0), (20.0, 10.0), (0.0, 10.0)].map(|(x, y)| Vec2::new(x, y));
    let body = crate::extrude_whole_sides(&Plane::XY, &[Region2 { outer: Loop2::polygon(&step), holes: vec![] }], 0.0, 5.0).unwrap().pop().unwrap();
    assert_eq!(body.face_count(), 8);
}
