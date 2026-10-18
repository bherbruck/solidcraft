//! Readable names of a sheet metal body's faces, from the sheet itself: `<panel>:top`,
//! `<panel>:bottom` and `<panel>:rim.<edge>` (the names sketches on sheet faces keep, see
//! [`SheetBody::panel_face_at`]); a bend's curved faces, `<flange>:bend.inner` and
//! `<flange>:bend.outer`; and where a flange's panel runs on past its bend's ends (toward a
//! neighbouring flange), the rim along the bend there, `<flange>:rim.ext0` and `rim.ext1`. A panel is `base` or a flange's tag (`F<feature>.<k>`), so the names
//! say what each face is and stay with it when sizes change or other flanges come and go.

use solidcraft_geom::{Vec2, Vec3};

use super::SheetBody;
use crate::{ModelBody, ModelState, apply_point, mat_inverse};

/// The share of a face's sample points that must agree on a name for the face to take it.
const AGREE: f64 = 0.8;

impl SheetBody {
    /// The readable name of the face of this sheet a point lies on (sheet coordinates, within
    /// `tol`, kept below a quarter of the thickness): a flat face's (see
    /// [`panel_face_at`](Self::panel_face_at)), else a bend's inner or outer face.
    pub fn face_name_at(&self, p: Vec3, tol: f64) -> Option<String> {
        self.panel_face_at(p, tol).map(|(name, _)| name).or_else(|| self.bend_face_at(p, tol)).or_else(|| self.ext_rim_at(p, tol))
    }

    /// The rim of a flange's panel along its bend line, where the panel runs on past the
    /// bend's start (`<flange>:rim.ext0`) or end (`rim.ext1`).
    fn ext_rim_at(&self, p: Vec3, tol: f64) -> Option<String> {
        if self.flat {
            return None;
        }
        let tol = tol.min(self.t * 0.25);
        for (pi, region) in self.panels() {
            let Some(i) = pi else { continue };
            let Some(f) = self.flanges.get(i) else { continue };
            let Some(inverse) = mat_inverse(&self.world(pi)) else { continue };
            let q = apply_point(&inverse, p);
            if q.z < -tol || q.z > self.t + tol {
                continue;
            }
            let q2 = Vec2::new(q.x, q.y);
            let pts = region.outer.ccw().polyline(1e-3);
            let m = pts.len();
            for k in 0..m {
                let (Some(&a), Some(&b)) = (pts.get(k), pts.get((k + 1) % m)) else { continue };
                let d = b - a;
                if d.len2() < 1e-18 || self.rim_edge(pi, a, b, k).as_deref() != Some("bend") {
                    continue;
                }
                let along = (q2 - a).dot(d) / d.len2();
                if !(-1e-9..=1.0 + 1e-9).contains(&along) || ((a + d * along) - q2).len() > tol {
                    continue;
                }
                let end = if (q2 - f.p).dot(f.d) < f.len / 2.0 { "ext0" } else { "ext1" };
                return Some(format!("{}:rim.{end}", self.panel_name(pi)));
            }
        }
        None
    }

    /// The curved face of a bend a point lies on: `<flange>:bend.inner` (the bend's radius) or
    /// `<flange>:bend.outer` (the radius plus the thickness).
    fn bend_face_at(&self, p: Vec3, tol: f64) -> Option<String> {
        if self.flat {
            return None;
        }
        let tol = tol.min(self.t * 0.25);
        for (i, f) in self.flanges.iter().enumerate() {
            let Some((world, o, d, start, toward)) = self.bend_frame(f) else { continue };
            let Some(inverse) = mat_inverse(&world) else { continue };
            let q = apply_point(&inverse, p) - o;
            let u = q.dot(d);
            let (u0, u1) = self.zone_span(f);
            if u < u0 - tol || u > u1 + tol {
                continue;
            }
            let radial = q - d * u;
            let rho = radial.len();
            let side = if (rho - f.radius).abs() <= tol {
                "inner"
            } else if (rho - f.radius - self.t).abs() <= tol {
                "outer"
            } else {
                continue;
            };
            let phi = radial.dot(toward).atan2(radial.dot(start));
            if phi < -1e-6 || phi > f.angle.abs() + 1e-6 {
                continue;
            }
            return Some(format!("{}:bend.{side}", self.panel_name(Some(i))));
        }
        None
    }
}

/// The readable sheet names of a body's faces (by B-rep face index), as
/// [`sheet_face_name`](super::sheet_face_name)s: `sheet:<body>:<panel>:<side>`. `None` for a face
/// the sheet does not name (a hole's wall, a pocket's floor) and for every face of a body that is
/// no folded sheet of `st`.
pub fn body_sheet_face_names(st: &ModelState, b: &ModelBody) -> Vec<Option<String>> {
    let m = b.mesh();
    let nf = m.tri_face.iter().map(|f| *f as usize + 1).max().unwrap_or(0);
    let Some(sheet) = st.sheets.iter().find(|s| s.body == b.name && !s.flat) else { return vec![None; nf] };
    // Triangle centres lie inside a face, off a curved one by its facets' sag: a fifth of the
    // thickness takes them and still tells a flat face from the rim beside it.
    let tol = sheet.t * 0.2;
    let mut votes: Vec<Vec<(String, usize)>> = vec![Vec::new(); nf];
    let mut total = vec![0usize; nf];
    for (t, f) in m.triangles.iter().zip(&m.tri_face) {
        let (Some(v), Some(n), Some([a, b2, c])) = (votes.get_mut(*f as usize), total.get_mut(*f as usize), m.tri(t)) else { continue };
        // At most a few dozen points a face: enough to tell its name, cheap on a fine mesh.
        if *n >= 48 {
            continue;
        }
        *n += 1;
        if let Some(name) = sheet.face_name_at((a + b2 + c) * (1.0 / 3.0), tol) {
            match v.iter_mut().find(|(x, _)| *x == name) {
                Some(x) => x.1 += 1,
                None => v.push((name, 1)),
            }
        }
    }
    votes
        .into_iter()
        .zip(total)
        .map(|(v, n)| {
            v.into_iter()
                .max_by_key(|x| x.1)
                .filter(|(_, k)| *k as f64 >= AGREE * n.max(1) as f64)
                .map(|(name, _)| super::sheet_face_name(&b.name, &name))
        })
        .collect()
}
