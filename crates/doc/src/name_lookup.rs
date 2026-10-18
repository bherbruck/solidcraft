//! Finding a face or edge again by the name a feature stored for it.
//!
//! The face with that name is the face; else the one face answering to it (a face merged into
//! it, or the named piece told apart by the faces around it, which still bound it); else, when
//! several faces are pieces of the named one, the piece nearest the reference's point, and the
//! feature says the face was split. Names saved before format 14 ([`LEGACY_MARK`]) are found
//! among the names geometry gives, as the builds that saved them found them. A face whose name
//! was worked out from geometry, found where the reference's point no longer lies, is reported:
//! only geometry decided.

use solidcraft_geom::{Mesh, Vec3};

use crate::history_names::{Match, Names};
use crate::naming::{self, closest, edge_names, face_names, legacy_edge_names, legacy_face_names, strip_piece};
use crate::piece_names::{strip_edge_number, strip_edge_piece};
use crate::{ModelBody, ModelState};

/// The mark on a name saved before format 14 (`?F2:top#2`). Those names were worked out from
/// geometry, and a few of them name a face that exact history names differently (a tube's outer
/// wall was named after the nearest sketch curve to its centre, its inner circle): such a name is
/// found again among the names that geometry gives today, as the build that saved it did, so an
/// older design evaluates as it was saved. Picking the face again stores its exact name.
pub const LEGACY_MARK: char = '?';

/// A name saved before format 14, without its mark (`None`: an exact name).
pub fn legacy_name(name: &str) -> Option<&str> {
    name.strip_prefix(LEGACY_MARK)
}

/// Where a stored edge name was found.
#[derive(Clone, Copy, Debug)]
pub struct EdgeHit {
    /// The edge (its index in the body's mesh).
    pub edge: usize,
    /// A point on the edge: the reference's own point when it lies on it, else the edge's middle.
    pub point: Vec3,
    /// The named edge is not one edge any more: the piece nearest the point was taken.
    pub split: bool,
    /// Whether the names of the faces it separates follow exact history (and the edge is not
    /// one of several between them, numbered by where they lie).
    pub verified: bool,
    /// Whether the reference's point still lies on the edge.
    pub on_point: bool,
    /// The name was saved before format 14 and found as that build did ([`LEGACY_MARK`]).
    pub legacy: bool,
}

/// The edge of a body a stored edge name finds: the edge with that name; else the one edge
/// between the faces it names (followed through splits and merges of those faces); else the
/// piece of it nearest `near` (`split`). `Err` explains when there is none.
pub fn locate_edge(b: &ModelBody, name: &str, near: Vec3) -> Result<EdgeHit, String> {
    if let Some(old) = legacy_name(name) {
        return legacy_locate_edge(b, old, near).map_err(|_| format!("edge `{name}` no longer exists"));
    }
    let m = b.mesh();
    let names = edge_names(b);
    let faces = naming::names(b);
    let mid = |i: usize| edge_mid(&m, i);
    // Several edges between the same two faces are numbered by where they lie: not exact.
    let verified = |i: usize| {
        m.edge_faces.get(i).is_some_and(|fs| fs.iter().all(|f| faces.verified(*f as usize)))
            && names.get(i).is_some_and(|n| strip_edge_number(n).len() == n.len())
    };
    let exact = |i: usize| -> Result<EdgeHit, String> {
        let on = edge_near(b, &m, i, near);
        mid(i)
            .map(|p| EdgeHit { edge: i, point: if on { near } else { p }, split: false, verified: verified(i), on_point: on, legacy: false })
            .ok_or_else(|| "edge".into())
    };
    if let Some(i) = names.iter().position(|n| n == name) {
        return exact(i);
    }
    // By the faces it separates: the best way any edge matches.
    let wanted = strip_edge_number(name);
    let mut best: Option<(Match, Vec<usize>)> = None;
    for (i, fs) in m.edge_faces.iter().enumerate() {
        let [fa, fb] = fs.as_slice() else { continue };
        let Some(level) = edge_match(&faces, *fa as usize, *fb as usize, wanted) else { continue };
        keep_best(&mut best, level, i);
    }
    let (level, cands) = best.ok_or_else(|| format!("edge `{name}` no longer exists"))?;
    if let ([i], true) = (cands.as_slice(), level >= Match::Piece) {
        return exact(*i);
    }
    cands
        .iter()
        .filter_map(|i| mid(*i).map(|p| (p, *i)))
        .min_by(|a, b| a.0.dist(near).total_cmp(&b.0.dist(near)))
        .map(|(p, i)| EdgeHit { edge: i, point: p, split: true, verified: verified(i), on_point: false, legacy: false })
        .ok_or_else(|| format!("edge `{name}` no longer exists"))
}

/// Whether the picked point `near` lies on edge `i` of body `b` (whose mesh is `m`): then a
/// reference keeps it.
fn edge_near(b: &ModelBody, m: &Mesh, i: usize, near: Vec3) -> bool {
    m.edges.get(i).is_some_and(|e| e.windows(2).any(|w| near.dist_to_segment(w[0], w[1]) < (b.body.size() * 2e-3).max(1e-3)))
}

/// The point of face `face` of a mesh nearest `p`, and how far it is.
fn nearest_on_face(m: &Mesh, face: usize, p: Vec3) -> Option<(f64, Vec3)> {
    let mut best: Option<(f64, Vec3)> = None;
    for (t, f) in m.triangles.iter().zip(&m.tri_face) {
        if *f as usize != face {
            continue;
        }
        let Some([a, b, c]) = m.tri(t) else { continue };
        let q = closest(p, a, b, c);
        let d = q.dist(p);
        // A non-finite candidate must never become the best: nothing compares less than NaN.
        if d.is_finite() && best.is_none_or(|x| d < x.0) {
            best = Some((d, q));
        }
    }
    best
}

/// The middle of edge `i` of a mesh (along its polyline).
fn edge_mid(m: &Mesh, i: usize) -> Option<Vec3> {
    let e = m.edges.get(i)?;
    let total: f64 = e.windows(2).map(|w| w[0].dist(w[1])).sum();
    let mut acc = 0.0;
    for w in e.windows(2) {
        let l = w[0].dist(w[1]);
        if acc + l >= total / 2.0 {
            let t = if l > 0.0 { (total / 2.0 - acc) / l } else { 0.0 };
            return Some(w[0] + (w[1] - w[0]) * t);
        }
        acc += l;
    }
    e.first().copied()
}

/// [`locate_edge`] for a name saved before format 14 (without its mark), as those builds found
/// it: the edge with that name among the names geometry gives, else the piece of it nearest
/// `near`.
fn legacy_locate_edge(b: &ModelBody, name: &str, near: Vec3) -> Result<EdgeHit, ()> {
    let m = b.mesh();
    let names = legacy_edge_names(b);
    if let Some(i) = names.iter().position(|n| n == name) {
        let on = edge_near(b, &m, i, near);
        return edge_mid(&m, i)
            .map(|p| EdgeHit { edge: i, point: if on { near } else { p }, split: false, verified: false, on_point: on, legacy: true })
            .ok_or(());
    }
    let stem = strip_edge_piece(name);
    (0..names.len())
        .filter(|i| names.get(*i).is_some_and(|n| strip_edge_piece(n) == stem))
        .filter_map(|i| edge_mid(&m, i).map(|p| (p, i)))
        .min_by(|a, b| a.0.dist(near).total_cmp(&b.0.dist(near)))
        .map(|(p, i)| EdgeHit { edge: i, point: p, split: true, verified: false, on_point: false, legacy: true })
        .ok_or(())
}

/// How well the stored edge name `wanted` (without its own number) names the edge between faces
/// `a` and `b`: the weaker of its two faces' matches, read every way the name splits into two
/// face names (a face name may hold `|` itself: a fillet face is named after its edge).
fn edge_match(faces: &Names, a: usize, b: usize, wanted: &str) -> Option<Match> {
    let mut best = None;
    for (i, _) in wanted.match_indices('|') {
        let (Some(x), Some(y)) = (wanted.get(..i), wanted.get(i + 1..)) else { continue };
        for (p, q) in [(a, b), (b, a)] {
            if let (Some(m1), Some(m2)) = (crate::history_names::matches(faces, p, x), crate::history_names::matches(faces, q, y)) {
                best = best.max(Some(m1.min(m2)));
            }
        }
    }
    best
}

/// Keep `item` among the best matches so far.
fn keep_best<T>(best: &mut Option<(Match, Vec<T>)>, level: Match, item: T) {
    match best {
        Some((b, items)) if *b == level => items.push(item),
        Some((b, _)) if *b > level => {}
        _ => *best = Some((level, vec![item])),
    }
}

/// Where a stored face name was found.
#[derive(Clone, Copy, Debug)]
pub struct FaceHit {
    /// The body (its index in the model's bodies).
    pub body: usize,
    /// The face (its index in the body).
    pub face: usize,
    /// The point on the face nearest the reference's point.
    pub point: Vec3,
    /// The named face is not one face any more (split, or found on several bodies): the piece
    /// nearest the point was taken.
    pub split: bool,
    /// The named face was merged into this one.
    pub merged: bool,
    /// Whether the face's name follows exact history.
    pub verified: bool,
    /// Whether the reference's point still lies on the face (so a name worked out from geometry
    /// and the pick agree).
    pub on_point: bool,
    /// The name was saved before format 14 and found as that build did ([`LEGACY_MARK`]).
    pub legacy: bool,
}

impl FaceHit {
    /// What a feature says about how it found a face by its stored name `name` (`None`: exactly).
    pub fn note(&self, name: &str) -> Option<String> {
        if self.split {
            Some(format!("face `{name}` was split; using the piece nearest the picked point"))
        } else if self.merged {
            Some(format!("face `{name}` was merged into another face; using that face"))
        } else if !self.verified && !self.on_point && !self.legacy {
            Some(format!("face `{name}` was found by geometry, not exact history; check it"))
        } else {
            None
        }
    }
}

impl FaceHit {
    /// Where the line through `p` along `dir` meets the face found (the hit nearest `p`, on
    /// either side of it), `None` when the line misses the face or runs along it. A place on
    /// a face follows the face this way, along its own axis, instead of sliding sideways to
    /// [`point`](Self::point), the face's nearest point.
    pub fn along(&self, st: &ModelState, p: Vec3, dir: Vec3) -> Option<Vec3> {
        let b = st.bodies.get(self.body)?;
        let d = dir.normalized()?;
        let m = b.mesh();
        let slack = (b.body.size() * 1e-9).max(1e-9);
        let mut best: Option<f64> = None;
        for (t, f) in m.triangles.iter().zip(&m.tri_face) {
            if *f as usize != self.face {
                continue;
            }
            let Some([a, bb, c]) = m.tri(t) else { continue };
            // A hair's width around each triangle, so a place on the seam of two is on the face.
            let centre = (a + bb + c) * (1.0 / 3.0);
            let grow = |v: Vec3| v + (v - centre).normalized().unwrap_or(Vec3::ZERO) * slack;
            if let Some(hit) = naming::ray_hit(p, d, grow(a), grow(bb), grow(c))
                && best.is_none_or(|old| hit.abs() < old.abs())
            {
                best = Some(hit);
            }
        }
        best.map(|t| p + d * t)
    }
}

impl EdgeHit {
    /// What a feature says about how it found an edge by its stored name `name` (`None`: exactly).
    pub fn note(&self, name: &str) -> Option<String> {
        if self.split {
            Some(format!("edge `{name}` was split; using the piece nearest the picked point"))
        } else if !self.verified && !self.on_point && !self.legacy {
            Some(format!("edge `{name}` was found by geometry, not exact history; check it"))
        } else {
            None
        }
    }
}

/// The face a stored face name finds, on any body: the face with that name, or the one face
/// answering to it (merged into it, or the named piece told apart by the faces around it); else
/// the piece of it nearest `p` (`split`).
pub fn locate_face(st: &ModelState, name: &str, p: Vec3) -> Option<FaceHit> {
    if crate::sheet::split_sheet_face_name(name).is_some() {
        return sheet_locate(st, name, p);
    }
    // Every body's names are read below: work out the ones not known yet on all cores first
    // (each body's names are its own, whichever thread works them out).
    if legacy_name(name).is_none() {
        let bodies: Vec<&ModelBody> = st.bodies.iter().filter(|b| !b.body.is_mesh()).collect();
        solidcraft_kernel::for_each_on_threads(&bodies, 16, |b| {
            let _ = naming::names(b);
        });
    }
    locate_in(st.bodies.iter().enumerate().filter(|(_, b)| !b.body.is_mesh()), name, p)
}

/// [`locate_face`] for a feature that works on the faces of the body called `body` (a shell, a
/// draft…): the one face of that body with the very name, found without working out the names
/// of every other body (a body named from geometry is meshed for it: seconds in a large model).
/// Else as [`locate_face`], over every body: the name moved to another body (a combine took the
/// face), or on this body it names a piece of a split face or a merged one. The same face on
/// another body (a copy of this one) is not a piece of this body's face.
pub(crate) fn locate_face_on_body(st: &ModelState, body: Option<&str>, name: &str, p: Vec3) -> Option<FaceHit> {
    if crate::sheet::split_sheet_face_name(name).is_some() {
        return sheet_locate(st, name, p);
    }
    if legacy_name(name).is_none()
        && let Some(i) = body.and_then(|n| st.bodies.iter().position(|b| b.name == n))
        && let Some(b) = st.bodies.get(i).filter(|b| !b.body.is_mesh())
        && let Some(hit) = locate_in(std::iter::once((i, b)), name, p)
        && !hit.split
        && naming::names(b).faces.get(hit.face).is_some_and(|n| n == name)
    {
        return Some(hit);
    }
    locate_face(st, name, p)
}

/// [`locate_face`] for a sheet face's readable name (`sheet:<body>:<panel>:<side>`, see
/// [`crate::sheet::body_sheet_face_names`]): the face of that body the sheet names so, else the
/// piece of it nearest `p` (`split`). The sheet itself says which face it is, so the name holds
/// however the faces of the body are numbered.
fn sheet_locate(st: &ModelState, name: &str, p: Vec3) -> Option<FaceHit> {
    let (body, _) = crate::sheet::split_sheet_face_name(name)?;
    let (bi, b) = st.bodies.iter().enumerate().find(|(_, b)| b.name == body && !b.body.is_mesh())?;
    let names = crate::sheet::body_sheet_face_names(st, b);
    let faces: Vec<usize> = names.iter().enumerate().filter(|(_, n)| n.as_deref() == Some(name)).map(|(i, _)| i).collect();
    let m = b.mesh();
    let (distance, face, point) =
        faces.iter().filter_map(|f| nearest_on_face(&m, *f, p).map(|(d, q)| (d, *f, q))).min_by(|a, c| a.0.total_cmp(&c.0))?;
    Some(FaceHit {
        body: bi,
        face,
        point,
        split: faces.len() > 1,
        merged: false,
        verified: true,
        on_point: distance <= (b.body.size() * 1e-3).max(1e-3),
        legacy: false,
    })
}

/// [`locate_face`] among `bodies` (with their indices).
fn locate_in<'a>(bodies: impl Iterator<Item = (usize, &'a ModelBody)>, name: &str, p: Vec3) -> Option<FaceHit> {
    if let Some(old) = legacy_name(name) {
        return legacy_locate(bodies, old, p);
    }
    let mut best: Option<(Match, Vec<(usize, &ModelBody, usize)>)> = None;
    for (bi, b) in bodies {
        let names = naming::names(b);
        for face in 0..names.faces.len() {
            if let Some(level) = crate::history_names::matches(&names, face, name) {
                keep_best(&mut best, level, (bi, b, face));
            }
        }
    }
    let (level, faces) = best?;
    let unique = faces.len() == 1 && level >= Match::Piece;
    let mut nearest: Option<(f64, usize, &ModelBody, usize, Vec3)> = None;
    for (bi, b, face) in faces {
        if let Some((d, q)) = nearest_on_face(&b.mesh(), face, p)
            && nearest.as_ref().is_none_or(|n| d < n.0)
        {
            nearest = Some((d, bi, b, face, q));
        }
    }
    let (distance, body, b, face, point) = nearest?;
    let on_point = distance <= (b.body.size() * 1e-3).max(1e-3);
    Some(FaceHit {
        body,
        face,
        point,
        split: !unique,
        merged: level == Match::Alias,
        verified: naming::names(b).verified(face),
        on_point,
        legacy: false,
    })
}

/// [`locate_face`] for a name saved before format 14 (without its mark), as those builds found
/// it: among the names geometry gives, the face with that name (or a piece of the same face, if
/// nearer `p`), the nearest point on it.
fn legacy_locate<'a>(bodies: impl Iterator<Item = (usize, &'a ModelBody)>, name: &str, p: Vec3) -> Option<FaceHit> {
    let stem = strip_piece(name);
    let mut best: Option<(f64, usize, usize, Vec3, bool, f64)> = None;
    for (bi, b) in bodies {
        let names = legacy_face_names(b);
        let tol = (b.body.size() * 1e-3).max(1e-3);
        let m = b.mesh();
        for (t, f) in m.triangles.iter().zip(&m.tri_face) {
            let Some(n) = names.get(*f as usize) else { continue };
            let exact = n == name;
            if !exact && strip_piece(n) != stem {
                continue;
            }
            let Some([a, bb, c]) = m.tri(t) else { continue };
            let q = closest(p, a, bb, c);
            let d = q.dist(p) - if exact { 1e-9 } else { 0.0 };
            if d.is_finite() && best.is_none_or(|x| d < x.0) {
                best = Some((d, bi, *f as usize, q, !exact, tol));
            }
        }
    }
    best.map(|(d, body, face, point, split, tol)| FaceHit {
        body,
        face,
        point,
        split,
        merged: false,
        verified: false,
        on_point: d <= tol,
        legacy: true,
    })
}

/// Whether face `index` of a body is the face a stored name names: that name (or one merged into
/// it); for a name saved before format 14, that name among the names those builds gave.
pub fn face_has_name(b: &ModelBody, index: usize, name: &str) -> bool {
    match legacy_name(name) {
        Some(old) => legacy_face_names(b).get(index).is_some_and(|n| n == old),
        None => crate::history_names::matches(&naming::names(b), index, name).is_some_and(|m| m >= Match::Alias),
    }
}

/// The faces of a body that are the stored name's face or pieces of it (by stem), as the name's
/// own era names them.
pub fn faces_with_stem(b: &ModelBody, name: &str) -> Vec<usize> {
    let (names, wanted) = match legacy_name(name) {
        Some(old) => (legacy_face_names(b), old),
        None => (face_names(b), name),
    };
    let stem = strip_piece(wanted);
    let aliases =
        |i: usize| legacy_name(name).is_none() && naming::names(b).info.get(i).is_some_and(|x| x.aliases.iter().any(|a| strip_piece(a) == stem));
    (0..names.len()).filter(|i| names.get(*i).is_some_and(|n| strip_piece(n) == stem) || aliases(*i)).collect()
}

/// [`locate_face`] on one body.
pub(crate) fn locate_on(body: &ModelBody, name: &str, p: Vec3) -> Option<FaceHit> {
    locate_in(std::iter::once((0, body)), name, p)
}

/// The one face a name `model.faces` lists finds, on the body called `body` or, without one, on
/// any body. `Err` says why there is no such face, or why the name does not name just one (the
/// pieces of a split face, or the same face on several bodies): a command that takes a face by
/// its name has no point to tell the pieces apart by.
pub fn locate_one(st: &ModelState, body: Option<&str>, name: &str) -> Result<FaceHit, String> {
    if let Some(n) = body
        && !st.bodies.iter().any(|b| b.name == n)
    {
        return Err(format!("no body `{n}`"));
    }
    if let Some((sheet_body, _)) = crate::sheet::split_sheet_face_name(name) {
        if body.is_some_and(|n| n != sheet_body) {
            return Err(format!("`{name}` is a face of `{sheet_body}`"));
        }
        let hit = sheet_locate(st, name, Vec3::ZERO).ok_or_else(|| format!("no face is named `{name}` (`model.faces` lists the names)"))?;
        if hit.split {
            return Err(format!("`{name}` names more than one face (pieces of a split face): give the name of one piece"));
        }
        return Ok(hit);
    }
    let bodies: Vec<(usize, &ModelBody)> =
        st.bodies.iter().enumerate().filter(|(_, b)| !b.body.is_mesh() && body.is_none_or(|n| b.name == n)).collect();
    // As [`locate_face`]: the names of several bodies are worked out on all cores first.
    if bodies.len() > 1 && legacy_name(name).is_none() {
        solidcraft_kernel::for_each_on_threads(&bodies, 16, |(_, b)| {
            let _ = naming::names(b);
        });
    }
    let hit = locate_in(bodies.into_iter(), name, Vec3::ZERO).ok_or_else(|| format!("no face is named `{name}` (`model.faces` lists the names)"))?;
    if hit.split {
        return Err(format!(
            "`{name}` names more than one face (pieces of a split face, or faces of several bodies): give `body`, or the name of one piece"
        ));
    }
    Ok(hit)
}
