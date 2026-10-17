//! Face trimming in the surface chart. Boundary loops are sampled from the exact edge curves,
//! unwrapped across periodic directions, and a point is classified by the parity of boundary
//! crossings on a path to a point known to be inside. Seams (an edge used twice by one face)
//! are crossed twice, so they cancel without special cases.

use super::curve::C3;
use super::surface::S3;
use crate::Vec3;
use crate::brep::Face;

#[derive(Clone, Debug)]
pub(crate) struct Trim {
    /// Unwrapped chart segments of all loops.
    segs: Vec<([f64; 2], [f64; 2])>,
    period: [Option<f64>; 2],
    /// A point inside the face; `None` when the face covers its whole chart.
    inside: Option<[f64; 2]>,
    /// 3D boundary chords, for distance-to-boundary margins.
    pub chords: Vec<(Vec3, Vec3)>,
    /// Largest distance between an exact boundary curve and its chords.
    pub chord_error: f64,
    pub lo: Vec3,
    pub hi: Vec3,
    /// Why classification is unavailable, if it is.
    pub unsupported: Option<&'static str>,
    /// Index for the two-leg parity path q -> (r.u, q.v) -> r.
    index: Option<Index>,
}

/// Segment copies shifted across periods near the inside point, the sorted crossings of the
/// vertical line u = r.u, and v-buckets of the copies for the horizontal leg.
#[derive(Clone, Debug)]
struct Index {
    copies: Vec<([f64; 2], [f64; 2])>,
    vcross: Vec<f64>,
    buckets: Vec<Vec<u32>>,
    v0: f64,
    dv: f64,
}

fn spans(x: f64, a: f64, b: f64) -> bool {
    (a <= x && x < b) || (b <= x && x < a)
}

fn index(segs: &[([f64; 2], [f64; 2])], period: [Option<f64>; 2], r: [f64; 2]) -> Index {
    let shifts = |k: usize| -> Vec<f64> {
        let Some(p) = period[k] else { return vec![0.] };
        let (lo, hi) = segs.iter().fold((f64::MAX, f64::MIN), |(lo, hi), (a, b)| (lo.min(a[k]).min(b[k]), hi.max(a[k]).max(b[k])));
        let k0 = ((r[k] - p - hi) / p).floor() as i64;
        let k1 = ((r[k] + p - lo) / p).ceil() as i64;
        (k0.max(-8)..=k1.min(8)).map(|i| i as f64 * p).collect()
    };
    let mut copies = Vec::new();
    for dx in shifts(0) {
        for dy in shifts(1) {
            copies.extend(segs.iter().map(|(a, b)| ([a[0] + dx, a[1] + dy], [b[0] + dx, b[1] + dy])));
        }
    }
    let mut vcross: Vec<f64> =
        copies.iter().filter(|(a, b)| spans(r[0], a[0], b[0])).map(|(a, b)| a[1] + (b[1] - a[1]) * (r[0] - a[0]) / (b[0] - a[0])).collect();
    vcross.sort_by(f64::total_cmp);
    let (v0, v1) = copies.iter().fold((f64::MAX, f64::MIN), |(lo, hi), (a, b)| (lo.min(a[1]).min(b[1]), hi.max(a[1]).max(b[1])));
    let n = ((copies.len() as f64).sqrt().ceil() as usize).clamp(1, 4096);
    let dv = ((v1 - v0) / n as f64).max(1e-300);
    let mut buckets = vec![Vec::new(); n];
    for (i, (a, b)) in copies.iter().enumerate() {
        let k0 = (((a[1].min(b[1]) - v0) / dv).floor().max(0.) as usize).min(n - 1);
        let k1 = (((a[1].max(b[1]) - v0) / dv).floor().max(0.) as usize).min(n - 1);
        for bucket in buckets.iter_mut().take(k1 + 1).skip(k0) {
            bucket.push(i as u32);
        }
    }
    Index { copies, vcross, buckets, v0, dv }
}

fn lift(x: f64, near: f64, p: Option<f64>) -> f64 {
    match p {
        Some(p) if p > 0. => x + p * ((near - x) / p).round(),
        _ => x,
    }
}

pub(crate) fn build(face: &Face, edges: &[C3], surface: &S3, per_break: usize) -> Trim {
    let period = surface.periods();
    let mut segs = Vec::new();
    let mut seam = Vec::new();
    let mut chords = Vec::new();
    let mut chord_error = 0f64;
    let (mut lo, mut hi) = (Vec3::new(f64::MAX, f64::MAX, f64::MAX), Vec3::new(f64::MIN, f64::MIN, f64::MIN));
    let mut uses = std::collections::BTreeMap::new();
    for u in face.loops.iter().flatten() {
        *uses.entry(u.edge).or_insert(0usize) += 1;
    }
    let mut area_u = 0.; // -loop integral of v du
    let mut area_v = 0.; // loop integral of u dv
    let mut wraps = [false; 2];
    let mut unsupported = None;
    for lp in &face.loops {
        let mut pts: Vec<([f64; 2], bool)> = Vec::new();
        let mut hint: Option<[f64; 2]> = None;
        for u in lp {
            let Some(c) = edges.get(u.edge as usize) else {
                unsupported = Some("edge reference");
                continue;
            };
            let is_seam = uses.get(&u.edge).copied().unwrap_or(0) > 1;
            let mut ts = Vec::new();
            let b = c.breaks();
            for w in b.windows(2) {
                let (Some(t0), Some(t1)) = (w.first(), w.get(1)) else { continue };
                for i in 0..per_break {
                    ts.push(t0 + (t1 - t0) * i as f64 / per_break as f64);
                }
            }
            if let Some(last) = b.last() {
                ts.push(*last);
            }
            if !u.forward {
                ts.reverse();
            }
            let mut prev3: Option<(f64, Vec3)> = None;
            for t in ts {
                let p = c.at(t);
                lo = lo.min(p);
                hi = hi.max(p);
                if let Some((tp, q)) = prev3 {
                    chords.push((q, p));
                    let m = c.at(0.5 * (t + tp));
                    chord_error = chord_error.max(m.dist_to_segment(q, p));
                }
                prev3 = Some((t, p));
                let Some(mut uv) = surface.inverse(p, hint) else {
                    unsupported = Some("boundary point off its surface");
                    continue;
                };
                if let Some(h) = hint {
                    uv = [lift(uv[0], h[0], period[0]), lift(uv[1], h[1], period[1])];
                }
                hint = Some(uv);
                pts.push((uv, is_seam));
            }
        }
        let (Some(first), Some(last)) = (pts.first().copied(), pts.last().copied()) else { continue };
        // Close the loop at the lifted copy of its start (a wrapping loop ends a period away).
        let close = [lift(first.0[0], last.0[0], period[0]), lift(first.0[1], last.0[1], period[1])];
        for k in 0..2 {
            if (close[k] - first.0[k]).abs() > 1e-9 {
                wraps[k] = true;
            }
        }
        pts.push((close, last.1));
        for w in pts.windows(2) {
            let (Some((a, _)), Some((b, s))) = (w.first(), w.get(1)) else { continue };
            area_u -= 0.5 * (a[1] + b[1]) * (b[0] - a[0]);
            area_v += 0.5 * (a[0] + b[0]) * (b[1] - a[1]);
            segs.push((*a, *b));
            seam.push(*s);
        }
    }
    let area = match wraps {
        [true, true] => {
            unsupported = unsupported.or(Some("loops wrap in both chart directions"));
            0.
        }
        [false, true] => area_v,
        _ => area_u,
    };
    let sigma = area.signum();
    // Inside point: beside the longest non-seam segment, on the face's (left) side.
    let mut inside = None;
    let real: Vec<usize> = (0..segs.len()).filter(|i| !seam.get(*i).copied().unwrap_or(true)).collect();
    if !real.is_empty() && area != 0. {
        let len = |i: usize| segs.get(i).map_or(0., |(a, b)| ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt());
        let best = real.iter().copied().max_by(|a, b| len(*a).total_cmp(&len(*b)));
        if let Some((a, b)) = best.and_then(|i| segs.get(i)) {
            let m = [0.5 * (a[0] + b[0]), 0.5 * (a[1] + b[1])];
            let l = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
            let n = [-(b[1] - a[1]) / l * sigma, (b[0] - a[0]) / l * sigma];
            // Stay well inside: a quarter of the distance to any other boundary piece.
            let mut room = l;
            for (j, (c, e)) in segs.iter().enumerate() {
                if Some(j) == best {
                    continue;
                }
                for (dx, dy) in shifts(period) {
                    let (c2, e2) = ([c[0] + dx, c[1] + dy], [e[0] + dx, e[1] + dy]);
                    room = room.min(point_segment(m, c2, e2));
                }
            }
            let delta = 0.25 * room;
            if delta > 0. && delta.is_finite() {
                inside = Some([m[0] + n[0] * delta, m[1] + n[1] * delta]);
            }
        }
        if inside.is_none() {
            unsupported = unsupported.or(Some("no room for an inside reference point"));
        }
    }
    let index = inside.map(|r| index(&segs, period, r));
    Trim { segs, period, inside, chords, chord_error, lo, hi, unsupported, index }
}

fn shifts(period: [Option<f64>; 2]) -> Vec<(f64, f64)> {
    let ks = |p: Option<f64>| p.map_or(vec![0.], |p| vec![-p, 0., p]);
    let mut out = Vec::new();
    for dx in ks(period[0]) {
        for dy in ks(period[1]) {
            out.push((dx, dy));
        }
    }
    out
}

fn point_segment(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let (ex, ey) = (b[0] - a[0], b[1] - a[1]);
    let l2 = ex * ex + ey * ey;
    let t = if l2 > 0. { (((p[0] - a[0]) * ex + (p[1] - a[1]) * ey) / l2).clamp(0., 1.) } else { 0. };
    ((a[0] + t * ex - p[0]).powi(2) + (a[1] + t * ey - p[1]).powi(2)).sqrt()
}

impl Trim {
    /// Whether chart point `uv` lies inside the face; `None` when the trim is unsupported.
    /// Parity of boundary crossings on the path q -> (r.u, q.v) -> r to the inside point r.
    pub fn contains(&self, uv: [f64; 2]) -> Option<bool> {
        if self.unsupported.is_some() {
            return None;
        }
        let (Some(r), Some(ix)) = (self.inside, self.index.as_ref()) else { return Some(true) };
        let q = [lift(uv[0], r[0], self.period[0]), lift(uv[1], r[1], self.period[1])];
        let mut crossings = 0usize;
        // Horizontal leg at v = q.v between q.u and r.u.
        let (ulo, uhi) = (q[0].min(r[0]), q[0].max(r[0]));
        let k = (((q[1] - ix.v0) / ix.dv).floor().max(0.) as usize).min(ix.buckets.len().saturating_sub(1));
        for i in ix.buckets.get(k).map(Vec::as_slice).unwrap_or(&[]) {
            let Some((a, b)) = ix.copies.get(*i as usize) else { continue };
            if spans(q[1], a[1], b[1]) {
                let u = a[0] + (b[0] - a[0]) * (q[1] - a[1]) / (b[1] - a[1]);
                if u > ulo && u <= uhi {
                    crossings += 1;
                }
            }
        }
        // Vertical leg at u = r.u between q.v and r.v.
        let (vlo, vhi) = (q[1].min(r[1]), q[1].max(r[1]));
        let first = ix.vcross.partition_point(|v| *v <= vlo);
        let last = ix.vcross.partition_point(|v| *v <= vhi);
        crossings += last.saturating_sub(first);
        Some(crossings.is_multiple_of(2))
    }

    /// The chart box of the unwrapped boundary.
    pub fn uv_bounds(&self) -> Option<([f64; 2], [f64; 2])> {
        let mut it = self.segs.iter().flat_map(|(a, b)| [a, b]);
        let first = *it.next()?;
        Some(it.fold((first, first), |(lo, hi), p| ([lo[0].min(p[0]), lo[1].min(p[1])], [hi[0].max(p[0]), hi[1].max(p[1])])))
    }

    /// Distance from a 3D point to the sampled boundary.
    pub fn margin(&self, p: Vec3) -> f64 {
        self.chords.iter().map(|(a, b)| p.dist_to_segment(*a, *b)).fold(f64::MAX, f64::min)
    }
}
