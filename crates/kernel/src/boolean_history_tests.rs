//! Booleans' histories, path by path: every face and edge origin checked against the geometry
//! of the operands (in tests only: the history itself is recorded, never sampled), and the exact
//! origins asserted where the outcome is known.

use std::collections::BTreeSet;

use solidcraft_geom::{Mesh, Vec3};

use crate::body::Body;
use crate::history::{Change, EdgeOrigin, FaceOrigin, History, Side};
use crate::{BoolOp, boolean, box_solid};

const fn face(operand: usize, face: usize) -> FaceOrigin {
    FaceOrigin::Face { operand, face }
}

fn closest_on_triangle(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Vec3 {
    // Ericson, Real-Time Collision Detection, 5.1.5.
    let (ab, ac, ap) = (b - a, c - a, p - a);
    let (d1, d2) = (ab.dot(ap), ac.dot(ap));
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = p - b;
    let (d3, d4) = (ab.dot(bp), ac.dot(bp));
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return a + ab * (d1 / (d1 - d3));
    }
    let cp = p - c;
    let (d5, d6) = (ab.dot(cp), ac.dot(cp));
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return a + ac * (d2 / (d2 - d6));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        return b + (c - b) * ((d4 - d3) / ((d4 - d3) + (d5 - d6)));
    }
    let denom = 1.0 / (va + vb + vc);
    a + ab * (vb * denom) + ac * (vc * denom)
}

/// Distance from `p` to face `f` of a mesh.
pub(crate) fn to_face(m: &Mesh, f: usize, p: Vec3) -> f64 {
    m.triangles
        .iter()
        .zip(&m.tri_face)
        .filter(|(_, x)| **x as usize == f)
        .map(|(t, _)| {
            let [a, b, c] = t.map(|i| m.positions[i as usize]);
            closest_on_triangle(p, a, b, c).dist(p)
        })
        .fold(f64::INFINITY, f64::min)
}

/// Distance from `p` to edge `e` of a mesh.
pub(crate) fn to_edge(m: &Mesh, e: usize, p: Vec3) -> f64 {
    m.edges[e].windows(2).map(|w| p.dist_to_segment(w[0], w[1])).fold(f64::INFINITY, f64::min)
}

/// How far, in a check, a face or edge may lie from what it came from.
#[derive(Clone, Copy)]
pub(crate) struct Reach {
    /// A face made in place of an edge, or an offset of a face, lies within this of it.
    pub made: f64,
    /// A face or edge the operation moved lies within this of the one it came from.
    pub moved: f64,
}

/// The result's history, checked against it and its operands, with every origin checked
/// against the geometry: each face's triangles lie on its origin faces, each origin face holds
/// some of them; each edge lies along its origin edges, or on both faces of a meeting. `tol`:
/// the meshes' tolerance.
fn verified<'a>(r: &'a Body, operands: &[&Body], tol: f64) -> &'a History {
    checked(r, operands, tol, Reach { made: 0.0, moved: 0.0 }, &|_| false)
}

/// [`verified`] for any operation: a face lies on its origin faces (within `at.moved` where the
/// operation moves faces), within `at.made` of the edges it is made in place of and of the faces
/// it is an offset of; an edge lies along its origin edges (within `at.moved`). Faces from
/// nothing are allowed only where `generated` says.
pub(crate) fn checked<'a>(r: &'a Body, operands: &[&Body], tol: f64, at: Reach, generated: &dyn Fn(usize) -> bool) -> &'a History {
    let h = r.history().expect("the operation reports its history");
    h.check(r, operands).unwrap();
    let rm = r.tessellate(tol).unwrap();
    let ms: Vec<Mesh> = operands.iter().map(|b| b.tessellate(tol).unwrap()).collect();
    let near = 4.0 * tol + r.size() * 1e-6;
    let on = |o: &FaceOrigin, p: Vec3| match *o {
        FaceOrigin::Face { operand, face } => to_face(&ms[operand], face, p) < near.max(at.moved),
        FaceOrigin::Edge { operand, edge } => to_edge(&ms[operand], edge, p) < at.made + near,
        FaceOrigin::Offset { operand, face } => to_face(&ms[operand], face, p) < at.made + near,
        _ => false,
    };
    let along = |o: &EdgeOrigin, p: &Vec3| match o {
        EdgeOrigin::Edge { operand, edge } => to_edge(&ms[*operand], *edge, *p) < near.max(at.moved),
        EdgeOrigin::Meet { a, b } => a.origins().iter().any(|o| on(o, *p)) && b.origins().iter().any(|o| on(o, *p)),
    };
    for (i, f) in h.faces.iter().enumerate() {
        assert!(!f.origins.is_empty() || generated(i), "face {i} has no origin");
        let centres = centres(&rm, i);
        for c in centres.iter().filter(|_| !f.origins.is_empty()) {
            assert!(f.origins.iter().any(|o| on(o, *c)), "face {i} ({f:?}) at {c:?} lies on none of its origins");
        }
        for o in &f.origins {
            assert!(centres.iter().any(|c| on(o, *c)), "face {i}: origin {o:?} is under none of it");
        }
    }
    for (j, e) in h.edges.iter().enumerate() {
        let pts = &rm.edges[j];
        for o in &e.origins {
            match o {
                // A joined edge runs along all of its origins, each along part of it (one in the
                // middle of a straight edge holds none of its points, but its own lie on it).
                EdgeOrigin::Edge { operand, edge } if e.origins.len() > 1 => assert!(
                    pts.iter().any(|p| along(o, p)) || ms[*operand].edges[*edge].iter().any(|q| to_edge(&rm, j, *q) < near.max(at.moved)),
                    "edge {j}: {o:?} is not along it"
                ),
                _ => assert!(pts.iter().all(|p| along(o, p)), "edge {j} ({e:?}) does not lie along {o:?}"),
            }
        }
        if e.origins.len() > 1 {
            for p in pts {
                assert!(e.origins.iter().any(|o| along(o, p)), "edge {j}: {p:?} lies along none of {e:?}");
            }
        }
    }
    h
}

/// The centres of face `f`'s triangles.
pub(crate) fn centres(m: &Mesh, f: usize) -> Vec<Vec3> {
    m.triangles
        .iter()
        .zip(&m.tri_face)
        .filter(|(_, x)| **x as usize == f)
        .map(|(t, _)| t.map(|k| m.positions[k as usize]).iter().fold(Vec3::ZERO, |s, p| s + *p) * (1.0 / 3.0))
        .collect()
}

/// How many faces of each change.
pub(crate) fn changes(h: &History) -> [usize; 5] {
    let mut n = [0; 5];
    for f in &h.faces {
        n[match f.change {
            Change::Kept => 0,
            Change::Modified => 1,
            Change::Split => 2,
            Change::Merged => 3,
            Change::Generated => 4,
        }] += 1;
    }
    n
}

/// The faces of the result that come from operand face `o`.
fn from(h: &History, o: FaceOrigin) -> Vec<usize> {
    (0..h.faces.len()).filter(|i| h.faces[*i].origins.contains(&o)).collect()
}

/// The meetings among the edges' origins, as face pairs (each side one face here).
fn meets(h: &History) -> BTreeSet<(FaceOrigin, FaceOrigin)> {
    let one = |s: &Side| s.single().expect("a side of one face");
    h.edges.iter().flat_map(|e| &e.origins).filter_map(|o| if let EdgeOrigin::Meet { a, b } = o { Some((one(a), one(b))) } else { None }).collect()
}

fn boxed(x0: f64, y0: f64, z0: f64, x1: f64, y1: f64, z1: f64) -> Body {
    box_solid(Vec3::new(x0, y0, z0), Vec3::new(x1, y1, z1)).unwrap()
}

/// A short summary of a history for failure messages.
fn show(h: &History) -> String {
    let faces: Vec<String> = h.faces.iter().enumerate().map(|(i, f)| format!("{i}: {:?} {:?}", f.change, f.origins)).collect();
    let edges: Vec<String> = h.edges.iter().enumerate().map(|(i, e)| format!("{i}: {:?} {:?}", e.faces, e.origins)).collect();
    format!("faces\n{}\nedges\n{}", faces.join("\n"), edges.join("\n"))
}

/// Each face's origins and change, in face order.
fn faces_of(h: &History) -> Vec<(Change, Vec<FaceOrigin>)> {
    h.faces.iter().map(|f| (f.change, f.origins.clone())).collect()
}

/// Truck's boolean with a sphere (no prism, no coincident faces): a box with a corner taken
/// off by a sphere. The three faces at the corner get a notch, the sphere's patches inside the
/// box bound it, meeting the box's faces along new edges.
#[test]
fn box_minus_sphere_through_truck() {
    let a = boxed(0.0, 0.0, 0.0, 10.0, 10.0, 10.0);
    let b = crate::sphere(Vec3::new(10.0, 10.0, 10.0), 4.0).unwrap();
    let r = boolean(&a, &b, BoolOp::Cut).unwrap().unwrap();
    let h = verified(&r, &[&a, &b], 0.02);
    assert_eq!(changes(h), [3, 6, 0, 0, 0], "{}", show(h));
    let from_b: BTreeSet<usize> = h
        .faces
        .iter()
        .flat_map(|f| &f.origins)
        .filter_map(|o| match o {
            FaceOrigin::Face { operand: 1, face } => Some(*face),
            _ => None,
        })
        .collect();
    assert_eq!(from_b.len(), 3);
    // Each notched box face meets two of the sphere's patches.
    assert_eq!(meets(h).len(), 6, "{}", show(h));
    assert!(meets(h).iter().all(|(x, y)| matches!((x, y), (FaceOrigin::Face { operand: 0, .. }, FaceOrigin::Face { operand: 1, .. }))));
}

/// The jitter retries move both operands and the result back: the history holds for the
/// operands themselves, the same faces coming from the same operand faces.
#[test]
fn shifted_truck_attempt_keeps_the_history() {
    let a = boxed(0.0, 0.0, 0.0, 10.0, 10.0, 10.0);
    let b = crate::sphere(Vec3::new(10.0, 10.0, 10.0), 4.0).unwrap();
    let counts = crate::record::counts(&[&a, &b]);
    let mut compared = 0;
    for op in [BoolOp::Cut, BoolOp::Union, BoolOp::Intersect] {
        let Ok((_, Some((plain, _)))) = crate::ops::truck_boolean(&a, &b, op, None, 0.035) else { continue };
        // The shifts the retries use (a hundredth of the size along each).
        for j in [[0.1373, 0.2719, 0.3911], [-0.5127, 0.0833, 0.2291], [0.6911, -0.3337, -0.6127]] {
            let shift = truck_modeling::Vector3::new(j[0], j[1], j[2]) * 0.17;
            let Ok((s, h)) = crate::ops::truck_boolean(&a, &b, op, Some(shift), 0.035) else { continue };
            let faces = h.as_ref().map(|(f, _)| f.clone());
            assert_eq!(faces, Some(plain.clone()), "{op:?}");
            let r = crate::boolean_record::truck_result(s, h, 17.4, &[], &counts).unwrap();
            verified(&r, &[&a, &b], 0.02);
            compared += 1;
        }
    }
    assert!(compared > 0);
}

/// Two prisms along x, the tool's section inside the target's, overlapping in part: sewn slab by
/// slab, then healed. The target's face at x = 10 gets the tool's outline as a hole, which the
/// tool's sides meet; the tool's sides and far end stand out of it.
#[test]
fn box_union_box_prism() {
    let a = boxed(0.0, 0.0, 0.0, 10.0, 10.0, 10.0);
    let b = boxed(5.0, 2.0, 2.0, 15.0, 8.0, 8.0);
    let r = boolean(&a, &b, BoolOp::Union).unwrap().unwrap();
    let h = verified(&r, &[&a, &b], 0.02);
    assert_eq!(h.faces.len(), 11);
    let mut origins: Vec<FaceOrigin> = h
        .faces
        .iter()
        .map(|f| match f.origins.as_slice() {
            [o] => *o,
            _ => panic!("one origin each: {}", show(h)),
        })
        .collect();
    origins.sort();
    // All of a's faces, and b's but the end inside a.
    let mut want: Vec<FaceOrigin> = (0..6).map(|i| face(0, i)).chain((0..6).map(|i| face(1, i))).collect();
    let inside = face(1, 4);
    want.retain(|o| *o != inside);
    assert_eq!(origins, want, "{}", show(h));
    // b's four sides meet a's face at x = 10.
    let rim: BTreeSet<_> = meets(h).iter().map(|(x, _)| *x).collect();
    assert_eq!(meets(h).len(), 4);
    assert_eq!(rim.len(), 1);
    assert!(h.edges.iter().all(|e| e.origins.len() == 1));
}

/// A through hole: the box's sides are kept, top and bottom get the hole, the cylinder's two
/// half walls line it, meeting top and bottom along its rims, and meet each other along the
/// cylinder's own seams.
#[test]
fn box_minus_cylinder_prism() {
    let a = boxed(0.0, 0.0, 0.0, 10.0, 10.0, 10.0);
    let b = crate::cylinder(Vec3::new(5.0, 5.0, -1.0), Vec3::Z, 2.0, 12.0).unwrap();
    let r = boolean(&a, &b, BoolOp::Cut).unwrap().unwrap();
    let h = verified(&r, &[&a, &b], 0.01);
    assert_eq!(changes(h), [4, 4, 0, 0, 0], "{}", show(h));
    let walls = from(h, face(1, 1)).len() + from(h, face(1, 2)).len();
    assert_eq!(walls, 2, "{}", show(h));
    // Rims: each wall meets top and bottom.
    assert_eq!(meets(h).len(), 4, "{}", show(h));
    let seams = h.edges.iter().filter(|e| matches!(e.origins.as_slice(), [EdgeOrigin::Edge { operand: 1, .. }])).count();
    assert_eq!(seams, 2);
    // Every other edge is one of the box's twelve.
    let boxed_edges: BTreeSet<_> = h.edges.iter().flat_map(|e| &e.origins).filter(|o| matches!(o, EdgeOrigin::Edge { operand: 0, .. })).collect();
    assert_eq!(boxed_edges.len(), 12);
}

/// A slot across the top (a stepped prism along y): the top and the box's ends come apart in
/// pieces (Split); the slot's floor and sides are the tool's.
#[test]
fn slot_splits_a_face() {
    let a = boxed(0.0, 0.0, 0.0, 10.0, 10.0, 10.0);
    let b = boxed(4.0, -1.0, 5.0, 6.0, 11.0, 11.0);
    let r = boolean(&a, &b, BoolOp::Cut).unwrap().unwrap();
    let h = verified(&r, &[&a, &b], 0.02);
    let top = (0..6).find(|i| a.planar_face(*i).is_some_and(|p| p.origin.z > 9.0 && p.normal().z > 0.9)).unwrap();
    let pieces = from(h, face(0, top));
    assert!(pieces.len() >= 2, "{}", show(h));
    assert!(pieces.iter().all(|i| h.faces[*i].change == Change::Split));
    // Three of the tool's faces bound the slot.
    let tool: BTreeSet<_> = h.faces.iter().flat_map(|f| &f.origins).filter(|o| matches!(o, FaceOrigin::Face { operand: 1, .. })).collect();
    assert_eq!(tool.len(), 3, "{}", show(h));
}

/// Bodies clear of each other: a union keeps both whole, a cut keeps the target.
#[test]
fn trivial_paths() {
    let a = boxed(0.0, 0.0, 0.0, 10.0, 10.0, 10.0);
    let b = crate::sphere(Vec3::new(30.0, 0.0, 0.0), 3.0).unwrap();
    let u = boolean(&a, &b, BoolOp::Union).unwrap().unwrap();
    let h = verified(&u, &[&a, &b], 0.02);
    let want: Vec<(Change, Vec<FaceOrigin>)> =
        (0..a.face_count()).map(|i| (Change::Kept, vec![face(0, i)])).chain((0..b.face_count()).map(|i| (Change::Kept, vec![face(1, i)]))).collect();
    assert_eq!(faces_of(h), want);
    let c = boolean(&a, &b, BoolOp::Cut).unwrap().unwrap();
    let h = verified(&c, &[&a, &b], 0.02);
    assert_eq!(faces_of(h), (0..a.face_count()).map(|i| (Change::Kept, vec![face(0, i)])).collect::<Vec<_>>());
    // Wholly inside: the intersection is the tool, the union the target.
    let inner = crate::sphere(Vec3::new(5.0, 5.0, 5.0), 2.0).unwrap();
    let i = boolean(&a, &inner, BoolOp::Intersect).unwrap().unwrap();
    let h = verified(&i, &[&a, &inner], 0.02);
    assert!(h.faces.iter().all(|f| f.change == Change::Kept && matches!(f.origins.as_slice(), [FaceOrigin::Face { operand: 1, .. }])));
    let u = boolean(&a, &inner, BoolOp::Union).unwrap().unwrap();
    let h = verified(&u, &[&a, &inner], 0.02);
    assert!(h.faces.iter().all(|f| matches!(f.origins.as_slice(), [FaceOrigin::Face { operand: 0, .. }])));
    // Cut out of the middle: a void, the tool's faces turned inward.
    let v = boolean(&a, &inner, BoolOp::Cut).unwrap().unwrap();
    let h = verified(&v, &[&a, &inner], 0.02);
    assert_eq!(v.shell_count(), 2);
    assert_eq!(h.faces.len(), a.face_count() + inner.face_count());
    assert!(h.faces.iter().all(|f| f.change == Change::Kept));
}

/// A body in two pieces, cut a piece at a time: each piece's faces are the body's own.
#[test]
fn by_lumps_composes_the_pieces() {
    let left = boxed(0.0, 0.0, 0.0, 10.0, 10.0, 10.0);
    let right = boxed(20.0, 0.0, 0.0, 30.0, 10.0, 10.0);
    let a = boolean(&left, &right, BoolOp::Union).unwrap().unwrap();
    assert_eq!(a.shell_count(), 2);
    let b = crate::sphere(Vec3::new(15.0, 5.0, 10.0), 7.0).unwrap();
    let r = crate::ops::by_lumps(&a, &b, a.lumps().unwrap(), b.lumps().unwrap(), BoolOp::Cut).unwrap().unwrap();
    let h = verified(&r, &[&a, &b], 0.02);
    // Both pieces are bitten: the sphere's patches inside each line its bite.
    let bites: BTreeSet<_> = h.faces.iter().flat_map(|f| &f.origins).filter(|o| matches!(o, FaceOrigin::Face { operand: 1, .. })).collect();
    assert!(bites.len() >= 2, "{}", show(h));
    let targets: BTreeSet<_> = h.faces.iter().flat_map(|f| &f.origins).filter(|o| matches!(o, FaceOrigin::Face { operand: 0, .. })).collect();
    assert_eq!(targets.len(), 12);
}

/// Two bodies touching along one whole face, back to back: glued, the two faces gone, the
/// coplanar sides each side of the seam merged.
#[test]
fn glue_merges_across_the_seam() {
    let a = boxed(0.0, 0.0, 0.0, 10.0, 10.0, 10.0);
    let b = boxed(10.0, 0.0, 0.0, 20.0, 10.0, 10.0);
    let r = crate::coplanar::glue(&a, &b).unwrap().unwrap();
    let h = verified(&r, &[&a, &b], 0.02);
    assert_eq!(changes(h), [2, 0, 0, 4, 0], "{}", show(h));
    // The four seam edges went with the merged faces; the ends' edges are each box's.
    assert_eq!(h.edges.len(), 12);
    assert!(h.edges.iter().all(|e| !e.origins.is_empty()));
}

/// Coincident faces pushed apart: a cut flush with the target's top, and a union of bodies
/// standing on one floor (the floor that is cut back is both operands' floors).
#[test]
fn faces_pushed_apart() {
    let a = boxed(0.0, 0.0, 0.0, 10.0, 10.0, 10.0);
    let tool = boxed(2.0, 2.0, 5.0, 8.0, 8.0, 10.0);
    let r = crate::coplanar::boolean_apart(&a, &tool, BoolOp::Cut).unwrap().unwrap().unwrap();
    let h = verified(&r, &[&a, &tool], 0.02);
    // The pocket: its floor and four sides are the tool's.
    let tool_faces: BTreeSet<_> = h.faces.iter().flat_map(|f| &f.origins).filter(|o| matches!(o, FaceOrigin::Face { operand: 1, .. })).collect();
    assert_eq!(tool_faces.len(), 5, "{}", show(h));
    let left = boxed(0.0, 0.0, 0.0, 10.0, 10.0, 10.0);
    let right = boxed(5.0, 3.0, 0.0, 15.0, 7.0, 4.0);
    let r = crate::coplanar::boolean_apart(&left, &right, BoolOp::Union).unwrap().unwrap().unwrap();
    let h = verified(&r, &[&left, &right], 0.02);
    assert!(h.faces.iter().any(|f| f.change == Change::Merged && f.origins.len() == 2), "{}", show(h));
}

/// The exact polygon boolean: pieces of planar faces joined back into faces.
#[test]
fn planar_fallback() {
    let a = boxed(0.0, 0.0, 0.0, 10.0, 10.0, 10.0);
    let b = boxed(5.0, 5.0, 5.0, 15.0, 15.0, 15.0);
    for (op, count) in [(BoolOp::Cut, 9), (BoolOp::Union, 12), (BoolOp::Intersect, 6)] {
        let r = crate::polybool::planar_boolean(&a, &b, op).unwrap().unwrap();
        let h = verified(&r, &[&a, &b], 0.02);
        assert_eq!(h.faces.len(), count, "{op:?}\n{}", show(h));
        assert!(h.faces.iter().all(|f| f.origins.len() == 1));
        // Three faces of each meet three of the other's.
        assert_eq!(meets(h).len(), 6, "{op:?}\n{}", show(h));
        assert!(h.edges.iter().all(|e| e.origins.len() == 1), "{op:?}\n{}", show(h));
    }
}

/// The nudged retry: a boolean with the tool moved a hair, composed with the move.
#[test]
fn nudged_tool_composes() {
    let a = boxed(0.0, 0.0, 0.0, 10.0, 10.0, 10.0);
    let b = crate::sphere(Vec3::new(10.0, 10.0, 10.0), 4.0).unwrap();
    let nb = crate::transform(&b, Vec3::new(1e-5, 0.0, 0.0), Vec3::ZERO, Vec3::Z, 0.0).unwrap();
    let r = boolean(&a, &nb, BoolOp::Cut).unwrap().unwrap();
    let r = crate::ops::nudged(r, &nb, &crate::record::counts(&[&a, &b]));
    verified(&r, &[&a, &b], 0.02);
}

/// A boolean with a triangle mesh: the mesh's faces are its triangles (and it has no edges).
#[test]
fn faceted_tool() {
    let a = boxed(0.0, 0.0, 0.0, 10.0, 10.0, 10.0);
    let tool = boxed(5.0, 5.0, 5.0, 15.0, 15.0, 15.0).tessellate(0.1).unwrap();
    let r = crate::faceted_boolean(&a, &tool, BoolOp::Cut, 0.01).unwrap().unwrap();
    let h = r.history().expect("the faceted boolean reports its history");
    h.check_counts((r.face_count(), r.edge_count()), &[(a.face_count(), a.edge_count()), (tool.triangles.len(), 0)]).unwrap();
    // The box's six faces (three notched) and the three tool faces lining the notch.
    let from_a: BTreeSet<_> = h.faces.iter().flat_map(|f| &f.origins).filter(|o| matches!(o, FaceOrigin::Face { operand: 0, .. })).collect();
    assert_eq!(from_a.len(), 6);
    assert_eq!(h.faces.iter().filter(|f| matches!(f.origins.first(), Some(FaceOrigin::Face { operand: 1, .. }))).count(), 3);
}

/// A partial-depth cut of stepped prisms: walls over one segment through successive slabs are
/// sewn as one face (coalesced), which comes from the operand walls under it in every slab.
#[test]
fn coalesced_stepped_walls() {
    use solidcraft_geom::{Loop2, Plane, Region2, Vec2};
    for curved in [true, false] {
        let (outer, inner) = if curved {
            (Loop2::circle(Vec2::ZERO, 10.0), Loop2::circle(Vec2::ZERO, 5.0))
        } else {
            let square = |h: f64| Loop2::polygon(&[Vec2::new(-h, -h), Vec2::new(h, -h), Vec2::new(h, h), Vec2::new(-h, h)]);
            (square(10.0), square(5.0))
        };
        let stock = crate::extrude(&Plane::XY, &[Region2 { outer, holes: vec![] }], 0.0, 20.0).unwrap().pop().unwrap();
        let cutter = crate::extrude(&Plane::XY, &[Region2 { outer: inner, holes: vec![] }], 2.0, 22.0).unwrap().pop().unwrap();
        let r = boolean(&stock, &cutter, BoolOp::Cut).unwrap().unwrap();
        let h = verified(&r, &[&stock, &cutter], 0.01);
        // The stock's walls run whole from 0 to 20, one face each, past the cutter's floor.
        let walls: Vec<usize> = (0..r.face_count()).filter(|i| r.face_was_coalesced(*i)).collect();
        assert_eq!(walls.len(), if curved { 2 } else { 4 }, "{}", show(h));
        for i in walls {
            assert!(matches!(h.faces[i].origins.as_slice(), [FaceOrigin::Face { operand: 0, .. }]), "{}", show(h));
            assert_eq!(h.faces[i].change, Change::Modified, "{}", show(h));
        }
    }
}

/// A step cut into a box: the stepped prism path sews the box's face 3 in pieces at the
/// slab stations, and the joining of generated walls on their common supports makes it one face
/// again; the joined body keeps the sewn body's history composed through the join. A certified
/// box (whose planar regularization would join the same pieces) gets the same history.
#[test]
fn regularized_step_cut() {
    let (pa, pb) = (boxed(0.0, 0.0, 0.0, 30.0, 35.0, 7.5), boxed(0.0, 30.0, 2.5, 22.5, 50.0, 25.0));
    let (a, b) = (pa.clone().with_box_certificate(1).unwrap(), pb.clone().with_box_certificate(2).unwrap());
    let r = boolean(&a, &b, BoolOp::Cut).unwrap().unwrap();
    let h = verified(&r, &[&a, &b], 0.01);
    let plain = boolean(&pa, &pb, BoolOp::Cut).unwrap().unwrap();
    let ph = verified(&plain, &[&pa, &pb], 0.01);
    let joined = from(ph, face(0, 3));
    assert_eq!(joined.len(), 1, "{}", show(ph));
    assert_eq!(faces_of(ph)[joined[0]], (Change::Modified, vec![face(0, 3)]));
    assert_eq!(faces_of(h), faces_of(ph), "{}", show(h));
}

/// A ring joined under a floor, its bore on the floor's hole (coincident cylinders): the ring's
/// bore is narrowed, the bodies joined, and the bore cut out again. The ring's bore is a circle
/// built in two arcs, and the recut wall is split where the cutting cylinder's own construction
/// splits it, so which arc each piece of it restores is not known: no history.
#[test]
fn bore_union_of_a_bore_in_two_arcs_has_no_history() {
    let hole = crate::cylinder(Vec3::new(50.0, 40.0, -5.0), Vec3::Z, 10.0, 30.0).unwrap();
    let floor = boolean(&boxed(0.0, 0.0, 0.0, 100.0, 80.0, 10.0), &hole, BoolOp::Cut).unwrap().unwrap();
    let centre = solidcraft_geom::Vec2::new(50.0, 40.0);
    let annulus = solidcraft_geom::Region2 {
        outer: solidcraft_geom::Loop2::circle(centre, 20.0),
        holes: vec![solidcraft_geom::Loop2::circle(centre, 10.0).reversed()],
    };
    let ring = crate::extrude(&solidcraft_geom::Plane::XY, &[annulus], -5.0, 2.0).unwrap().remove(0);
    let r = crate::coplanar::bore_union(&floor, &ring, &|_| true).expect("the bores are recut");
    assert!(r.history().is_none());
}

/// Bodies clearly apart, joined as one body of several pieces (`union_apart`): every face and
/// edge is its piece's own, kept.
#[test]
fn bodies_apart_joined_keep_their_faces() {
    let pieces = [boxed(0.0, 0.0, 0.0, 2.0, 2.0, 2.0), boxed(5.0, 0.0, 0.0, 7.0, 2.0, 2.0), boxed(10.0, 0.0, 0.0, 12.0, 2.0, 2.0)];
    let r = crate::union_apart(&pieces).unwrap().unwrap();
    let operands: Vec<&Body> = pieces.iter().collect();
    let h = verified(&r, &operands, 0.01);
    assert_eq!(h.operands, 3);
    for (k, piece) in pieces.iter().enumerate() {
        for f in 0..piece.face_count() {
            assert_eq!(from(h, face(k, f)).len(), 1, "{}", show(h));
        }
    }
    assert!(faces_of(h).iter().all(|(change, origins)| *change == Change::Kept && origins.len() == 1), "{}", show(h));
}

/// A plate with a round end and a boss of that radius standing on it (#1209): the boss's wall
/// and the plate's round end are two pieces of one cylinder, one over the other, meeting along a
/// circle at the plate's top. That circle crosses the walls at its level only, so a second boss
/// joined to the plate and a cut through the first, both prism booleans sewn slab by slab, keep
/// every face of the plate and its boss, with their history.
#[test]
fn stacked_pieces_of_one_cylinder_keep_their_walls() {
    let plate =
        boolean(&boxed(0.0, -6.0, 0.0, 40.0, 6.0, 5.0), &crate::cylinder(Vec3::ZERO, Vec3::Z, 10.0, 5.0).unwrap(), BoolOp::Union).unwrap().unwrap();
    let arm = boolean(&plate, &crate::cylinder(Vec3::ZERO, Vec3::Z, 10.0, 8.0).unwrap(), BoolOp::Union).unwrap().unwrap();
    verified(&arm, &[&plate, &crate::cylinder(Vec3::ZERO, Vec3::Z, 10.0, 8.0).unwrap()], 0.01);
    // Standing over the plate's far end, as the boss at the arm's other end does.
    let second = crate::cylinder(Vec3::new(40.0, 0.0, 0.0), Vec3::Z, 4.0, 7.0).unwrap();
    let bore = boxed(-3.0, -3.0, -1.0, 3.0, 3.0, 10.0);
    for (tool, op) in [(&second, BoolOp::Union), (&bore, BoolOp::Cut)] {
        let r = boolean(&arm, tool, op).unwrap().unwrap();
        let h = verified(&r, &[&arm, tool], 0.01);
        for f in 0..arm.face_count() {
            assert!(!from(h, face(0, f)).is_empty(), "{op:?}: face {f} of the plate and boss is lost\n{}", show(h));
        }
    }
}
