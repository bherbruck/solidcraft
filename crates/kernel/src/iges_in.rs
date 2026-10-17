//! IGES 5.3 import, written from the published specification. The file's manifold solid B-reps
//! (186) and trimmed or bounded surfaces (144, 143, sewn into shells by their shared edges) are
//! restated as an equivalent STEP exchange and read by the STEP reader, so both formats share
//! one geometry and topology path.
//!
//! Files are untrusted: sizes, entity counts, references and recursion are bounded; anything
//! that cannot be read becomes a warning.

use std::collections::HashMap;
use std::fmt::Write as _;

mod assembly;

/// Largest IGES file we read.
pub const MAX_IGES_BYTES: usize = 512 << 20;
const MAX_ENTITIES: usize = 2_000_000;
const MAX_DEPTH: usize = 32;
const MAX_LIST: usize = 1_000_000;
/// Entities visited and segments produced while expanding model curves. A composite curve
/// (102) lists its children by pointer, and nothing stops a tree whose every node names the
/// same child twice: 30 levels under the depth cap is 2^30 leaf visits and about 160 GB of
/// segments. The depth cap alone does not see fan-out, and a node that produces no segment
/// trips no output bound, so the visits are counted too.
const MAX_CURVE_WORK: usize = 4 * MAX_ENTITIES;

type R<T> = Result<T, String>;
type V3 = [f64; 3];

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn mul(a: V3, k: f64) -> V3 {
    [a[0] * k, a[1] * k, a[2] * k]
}
fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn norm(a: V3) -> f64 {
    dot(a, a).sqrt()
}
fn unit(a: V3) -> Option<V3> {
    let n = norm(a);
    (n > 1e-300 && n.is_finite()).then(|| mul(a, 1.0 / n))
}

/// A rigid (or general affine) map: rows of a 3×4 matrix.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Xf([[f64; 4]; 3]);

impl Xf {
    const ID: Xf = Xf([[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]]);
    fn point(&self, p: V3) -> V3 {
        let m = &self.0;
        [0, 1, 2].map(|i| m[i][0] * p[0] + m[i][1] * p[1] + m[i][2] * p[2] + m[i][3])
    }
    fn dir(&self, d: V3) -> V3 {
        let m = &self.0;
        [0, 1, 2].map(|i| m[i][0] * d[0] + m[i][1] * d[1] + m[i][2] * d[2])
    }
    /// `self` after `inner` (inner applied first).
    fn then(&self, inner: &Xf) -> Xf {
        let (a, b) = (&self.0, &inner.0);
        let mut m = [[0.0; 4]; 3];
        for i in 0..3 {
            for j in 0..4 {
                m[i][j] = (0..3).map(|k| a[i][k] * b[k][j]).sum::<f64>() + if j == 3 { a[i][3] } else { 0.0 };
            }
        }
        Xf(m)
    }
    /// Scale of lengths (cube root of the determinant's size).
    fn scale(&self) -> f64 {
        let m = &self.0;
        let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
        det.abs().cbrt()
    }
}

/// One directory entry.
#[derive(Clone, Debug, Default)]
struct De {
    typ: i64,
    pd: usize,
    xform: i64,
    color: i64,
    lines: usize,
    form: i64,
    label: String,
}

/// The parsed file: directory entries (by DE sequence number) and their parameter tokens.
struct Iges {
    des: HashMap<usize, De>,
    params: HashMap<usize, Vec<String>>,
    /// Millimetres per model unit.
    unit: f64,
    /// Minimum resolution (model units).
    resolution: f64,
}

/// Split parameter text into fields at `delim`, stopping at `rdelim`; Hollerith strings (`nH…`)
/// may contain either.
fn tokens(s: &str, delim: char, rdelim: char) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == delim || c == rdelim {
            out.push(cur.trim().to_string());
            cur.clear();
            if c == rdelim {
                return out;
            }
            i += 1;
            continue;
        }
        if (c == 'H' || c == 'h') && !cur.trim().is_empty() && cur.trim().chars().all(|d| d.is_ascii_digit()) {
            let n: usize = cur.trim().parse().unwrap_or(0).min(chars.len());
            let start = i + 1;
            let end = (start + n).min(chars.len());
            cur = chars.get(start..end).map(|x| x.iter().collect::<String>()).unwrap_or_default();
            out.push(cur.clone());
            cur.clear();
            i = end;
            // Skip to the next delimiter.
            while i < chars.len() && chars[i] != delim && chars[i] != rdelim {
                i += 1;
            }
            if i < chars.len() {
                if chars[i] == rdelim {
                    return out;
                }
                i += 1;
            }
            continue;
        }
        cur.push(c);
        i += 1;
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

fn num(s: &str) -> Option<f64> {
    let t = s.trim().replace(['D', 'd'], "E");
    if t.is_empty() {
        return None;
    }
    t.parse::<f64>().ok().filter(|x| x.is_finite())
}

fn int(s: &str) -> Option<i64> {
    let t = s.trim();
    if t.is_empty() {
        return Some(0);
    }
    t.parse::<i64>().ok().or_else(|| num(t).filter(|x| x.fract() == 0.0 && x.abs() < 1e15).map(|x| x as i64))
}

fn parse(text: &str) -> R<Iges> {
    let mut g = String::new();
    let mut d_lines: Vec<String> = Vec::new();
    let mut p_lines: HashMap<usize, String> = HashMap::new();
    for raw in text.lines() {
        let line = raw.trim_end_matches(['\r', '\n']);
        if line.trim().is_empty() {
            continue;
        }
        let chars: Vec<char> = line.chars().collect();
        if chars.len() < 73 {
            continue;
        }
        let sect = chars[72];
        let body = |a: usize, b: usize| chars.get(a..b.min(chars.len())).map(|x| x.iter().collect::<String>()).unwrap_or_default();
        match sect {
            'S' => {}
            'G' => g.push_str(&body(0, 72)),
            'D' => d_lines.push(body(0, 72)),
            'P' => {
                let seq: usize = body(73, 80).trim().parse().unwrap_or(0);
                p_lines.insert(seq, body(0, 64));
            }
            'T' => break,
            'C' => return Err("compressed IGES is not supported".into()),
            'B' => return Err("binary IGES is not supported".into()),
            _ => {}
        }
        if d_lines.len() > 2 * MAX_ENTITIES {
            return Err("too many entities".into());
        }
    }
    // Global section: parameter and record delimiters, then fields from 3 on.
    let gc: Vec<char> = g.chars().collect();
    let (mut delim, mut rdelim, mut pos) = (',', ';', 0usize);
    if gc.len() >= 3 && gc[0] == '1' && (gc[1] == 'H' || gc[1] == 'h') {
        delim = gc[2];
        pos = 3;
    }
    if gc.get(pos) == Some(&delim) {
        pos += 1;
    }
    if gc.len() >= pos + 3 && gc[pos] == '1' && (gc[pos + 1] == 'H' || gc[pos + 1] == 'h') {
        rdelim = gc[pos + 2];
        pos += 3;
    }
    if gc.get(pos) == Some(&delim) {
        pos += 1;
    }
    let rest: String = gc.get(pos..).map(|x| x.iter().collect()).unwrap_or_default();
    let gf = tokens(&rest, delim, rdelim);
    // Field k (1-based) is gf[k - 3].
    let field = |k: usize| gf.get(k.wrapping_sub(3)).map(String::as_str).unwrap_or("");
    let unit = match int(field(14)).unwrap_or(2) {
        1 => 25.4,
        2 => 1.0,
        3 => match field(15).trim().to_ascii_uppercase().as_str() {
            "IN" | "INCH" => 25.4,
            "FT" => 304.8,
            "M" => 1000.0,
            "CM" => 10.0,
            "UM" => 1e-3,
            _ => 1.0,
        },
        4 => 304.8,
        5 => 1_609_344.0,
        6 => 1000.0,
        7 => 1e6,
        8 => 0.0254,
        9 => 1e-3,
        10 => 10.0,
        11 => 2.54e-5,
        _ => 1.0,
    };
    let resolution = num(field(19)).filter(|x| *x > 0.0).unwrap_or(1e-6);
    // Directory: two lines per entity, nine 8-column fields each.
    let mut des = HashMap::new();
    for (k, pair) in d_lines.chunks(2).enumerate() {
        let [a, b] = pair else { break };
        let f = |s: &str, i: usize| s.chars().skip(i * 8).take(8).collect::<String>();
        let de = De {
            typ: int(&f(a, 0)).unwrap_or(0),
            pd: int(&f(a, 1)).unwrap_or(0).max(0) as usize,
            xform: int(&f(a, 6)).unwrap_or(0),
            color: int(&f(b, 2)).unwrap_or(0),
            lines: int(&f(b, 3)).unwrap_or(0).clamp(0, 1_000_000) as usize,
            form: int(&f(b, 4)).unwrap_or(0),
            label: f(b, 7).trim().to_string(),
        };
        des.insert(2 * k + 1, de);
    }
    let mut params = HashMap::new();
    // Nothing stops thousands of directory entries from naming the same parameter range, and
    // each one materialises that range as its own string and token list. Keep the total text
    // proportional to the parameter section the file actually carries.
    let max_lines = p_lines.len().saturating_mul(4).saturating_add(1024);
    let mut lines_used = 0usize;
    for (seq, de) in &des {
        lines_used = lines_used.saturating_add(de.lines.max(1));
        if lines_used > max_lines {
            return Err("the file's parameter data is too large".into());
        }
        let mut s = String::new();
        for l in de.pd..de.pd.saturating_add(de.lines.max(1)) {
            // Columns 1–64 run on (a long string may continue on the next line).
            match p_lines.get(&l) {
                Some(x) => s.push_str(&format!("{x:<64}")),
                None => break,
            }
        }
        let t = tokens(&s, delim, rdelim);
        params.insert(*seq, t);
    }
    if des.is_empty() {
        return Err("the file has no entities".into());
    }
    Ok(Iges { des, params, unit, resolution })
}

/// A model curve segment ready for an edge: STEP curve id, its end points, points a quarter and
/// three quarters of the way along and one at the middle (for matching shared edges), and
/// whether the curve runs from `b` to `a`.
#[derive(Clone, Copy, Debug)]
struct Seg {
    curve: u64,
    a: V3,
    b: V3,
    q: V3,
    q3: V3,
    mid: V3,
    rev: bool,
}

impl Seg {
    fn along(curve: u64, f: impl Fn(f64) -> V3) -> Seg {
        Seg { curve, a: f(0.0), b: f(1.0), q: f(0.25), q3: f(0.75), mid: f(0.5), rev: false }
    }
    fn reversed(self) -> Seg {
        Seg { a: self.b, b: self.a, q: self.q3, q3: self.q, rev: !self.rev, ..self }
    }
}

/// The STEP exchange being written.
struct Out {
    lines: Vec<String>,
}

impl Out {
    fn add(&mut self, s: impl Into<String>) -> u64 {
        self.lines.push(s.into());
        self.lines.len() as u64
    }
    fn point(&mut self, p: V3) -> u64 {
        self.add(format!("CARTESIAN_POINT('',({},{},{}))", r(p[0]), r(p[1]), r(p[2])))
    }
    fn dir(&mut self, d: V3) -> u64 {
        self.add(format!("DIRECTION('',({},{},{}))", r(d[0]), r(d[1]), r(d[2])))
    }
    fn placement(&mut self, o: V3, z: V3, x: V3) -> u64 {
        let (a, b, c) = (self.point(o), self.dir(z), self.dir(x));
        self.add(format!("AXIS2_PLACEMENT_3D('',#{a},#{b},#{c})"))
    }
}

fn r(x: f64) -> String {
    let s = format!("{x:?}");
    if s.contains('.') || s.contains('e') || s.contains('E') { s.replace('e', "E") } else { format!("{s}.") }
}

/// Any direction square to `z`.
fn perp(z: V3) -> V3 {
    let a = if z[0].abs() < 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] };
    unit(sub(a, mul(z, dot(a, z)))).unwrap_or([1.0, 0.0, 0.0])
}

/// A rational B-spline curve evaluated by de Boor's algorithm.
struct Nurbs {
    deg: usize,
    knots: Vec<f64>,
    pts: Vec<[f64; 4]>,
}

impl Nurbs {
    fn eval(&self, t: f64) -> Option<V3> {
        let p = self.deg;
        let n = self.pts.len();
        if n == 0 || self.knots.len() != n + p + 1 {
            return None;
        }
        let (lo, hi) = (self.knots[p], self.knots[n]);
        let t = t.clamp(lo, hi);
        let mut k = p;
        while k + 1 < n && self.knots[k + 1] <= t {
            k += 1;
        }
        let mut d: Vec<[f64; 4]> = (0..=p).map(|j| self.pts[(k + j).saturating_sub(p).min(n - 1)]).collect();
        for rr in 1..=p {
            for j in (rr..=p).rev() {
                let i = k + j - p;
                let (a0, a1) = (self.knots[i], self.knots[(i + p + 1 - rr).min(self.knots.len() - 1)]);
                let alpha = if a1 > a0 { (t - a0) / (a1 - a0) } else { 0.0 };
                let (x, y) = (d[j - 1], d[j]);
                d[j] = [0, 1, 2, 3].map(|c| (1.0 - alpha) * x[c] + alpha * y[c]);
            }
        }
        let h = d[p];
        (h[3].abs() > 1e-300).then(|| [h[0] / h[3], h[1] / h[3], h[2] / h[3]])
    }
    fn range(&self) -> (f64, f64) {
        (self.knots.get(self.deg).copied().unwrap_or(0.0), self.knots.get(self.pts.len()).copied().unwrap_or(0.0))
    }
}

struct Tr<'a> {
    f: &'a Iges,
    o: Out,
    warnings: Vec<String>,
    /// Sewing tolerance (mm).
    tol: f64,
    surf_cache: HashMap<(usize, [u64; 12]), u64>,
    /// Entities visited and segments produced while expanding curves (see `MAX_CURVE_WORK`).
    curve_work: usize,
    /// Composite curves (102) on the path being expanded.
    composites: Vec<i64>,
}

fn xf_key(x: &Xf) -> [u64; 12] {
    let mut k = [0u64; 12];
    for i in 0..3 {
        for j in 0..4 {
            k[i * 4 + j] = x.0[i][j].to_bits();
        }
    }
    k
}

impl<'a> Tr<'a> {
    fn warn(&mut self, s: impl Into<String>) {
        let s = s.into();
        if !self.warnings.contains(&s) && self.warnings.len() < 200 {
            self.warnings.push(s);
        }
    }
    fn de(&self, seq: i64) -> R<&'a De> {
        let f = self.f;
        usize::try_from(seq).ok().and_then(|s| f.des.get(&s)).ok_or_else(|| format!("missing entity {seq}"))
    }
    fn p(&self, seq: i64) -> R<&'a [String]> {
        let f = self.f;
        usize::try_from(seq).ok().and_then(|s| f.params.get(&s)).map(Vec::as_slice).ok_or_else(|| format!("missing parameters of entity {seq}"))
    }
    /// Field `i` of an entity (field 0 is the entity type).
    fn n(&self, seq: i64, i: usize) -> R<f64> {
        self.p(seq)?.get(i).and_then(|s| num(s)).ok_or_else(|| format!("entity {seq}: bad number in field {i}"))
    }
    fn i(&self, seq: i64, i: usize) -> R<i64> {
        self.p(seq)?.get(i).and_then(|s| int(s)).ok_or_else(|| format!("entity {seq}: bad integer in field {i}"))
    }
    /// The entity's own transformation (its chain), in millimetres.
    fn own_xf(&self, seq: i64, depth: usize) -> R<Xf> {
        let de = self.de(seq)?;
        if de.xform <= 0 || depth > MAX_DEPTH {
            return Ok(Xf::ID);
        }
        let t = de.xform;
        let tde = self.de(t)?;
        if tde.typ != 124 {
            return Ok(Xf::ID);
        }
        let mut m = [[0.0; 4]; 3];
        for (row, r) in m.iter_mut().enumerate() {
            for (col, x) in r.iter_mut().enumerate() {
                *x = self.n(t, 1 + row * 4 + col)?;
            }
            r[3] *= self.f.unit;
        }
        let parent = self.own_xf(t, depth + 1)?;
        Ok(parent.then(&Xf(m)))
    }
    fn pt(&self, seq: i64, i: usize) -> R<V3> {
        let u = self.f.unit;
        Ok([self.n(seq, i)? * u, self.n(seq, i + 1)? * u, self.n(seq, i + 2)? * u])
    }
    /// A point entity (116) or direction (123), placed.
    fn point_entity(&self, seq: i64, xf: &Xf) -> R<V3> {
        let x = xf.then(&self.own_xf(seq, 0)?);
        Ok(x.point(self.pt(seq, 1)?))
    }
    fn dir_entity(&self, seq: i64, xf: &Xf) -> R<V3> {
        let x = xf.then(&self.own_xf(seq, 0)?);
        unit(x.dir([self.n(seq, 1)?, self.n(seq, 2)?, self.n(seq, 3)?])).ok_or_else(|| format!("entity {seq}: zero direction"))
    }

    /// Curve segments of a model-space curve entity (composite curves give several).
    fn curve(&mut self, seq: i64, xf: &Xf, depth: usize) -> R<Vec<Seg>> {
        if depth > MAX_DEPTH {
            return Err("curves nested too deeply".into());
        }
        self.curve_work = self.curve_work.saturating_add(1);
        if self.curve_work > MAX_CURVE_WORK {
            return Err("the file's curves are too complex".into());
        }
        let out = self.curve_parts(seq, xf, depth)?;
        self.curve_work = self.curve_work.saturating_add(out.len());
        if self.curve_work > MAX_CURVE_WORK {
            return Err("a curve has too many segments".into());
        }
        Ok(out)
    }

    fn curve_parts(&mut self, seq: i64, xf: &Xf, depth: usize) -> R<Vec<Seg>> {
        let de = self.de(seq)?;
        let x = xf.then(&self.own_xf(seq, 0)?);
        match de.typ {
            110 => {
                let (a, b) = (x.point(self.pt(seq, 1)?), x.point(self.pt(seq, 4)?));
                if !(norm(sub(b, a)) > 0.0) {
                    return Ok(Vec::new());
                }
                let (pa, d) = (self.o.point(a), unit(sub(b, a)).ok_or("zero-length line")?);
                let dd = self.o.dir(d);
                let v = self.o.add(format!("VECTOR('',#{dd},{})", r(norm(sub(b, a)))));
                let c = self.o.add(format!("LINE('',#{pa},#{v})"));
                Ok(vec![Seg::along(c, |t| add(a, mul(sub(b, a), t)))])
            }
            100 => {
                let u = self.f.unit;
                let zt = self.n(seq, 1)? * u;
                let c = [self.n(seq, 2)? * u, self.n(seq, 3)? * u, zt];
                let s = [self.n(seq, 4)? * u, self.n(seq, 5)? * u, zt];
                let e = [self.n(seq, 6)? * u, self.n(seq, 7)? * u, zt];
                let rad = norm(sub(s, c));
                if !(rad > 0.0) {
                    return Err(format!("entity {seq}: arc of zero radius"));
                }
                let a0 = (s[1] - c[1]).atan2(s[0] - c[0]);
                let mut a1 = (e[1] - c[1]).atan2(e[0] - c[0]);
                while a1 <= a0 + 1e-12 {
                    a1 += std::f64::consts::TAU;
                }
                let at = |t: f64| x.point([c[0] + rad * t.cos(), c[1] + rad * t.sin(), zt]);
                let z = unit(x.dir([0.0, 0.0, 1.0])).ok_or("degenerate arc frame")?;
                let xa = unit(x.dir([1.0, 0.0, 0.0])).ok_or("degenerate arc frame")?;
                let ax = self.o.placement(x.point(c), z, xa);
                let cid = self.o.add(format!("CIRCLE('',#{ax},{})", r(rad * x.scale())));
                Ok(vec![Seg::along(cid, |t| at(a0 + (a1 - a0) * t))])
            }
            102 => {
                // A composite curve that contains itself, directly or through another, would
                // expand until the depth cap; refusing it on the way down is clearer.
                if self.composites.contains(&seq) {
                    return Err(format!("entity {seq}: composite curve contains itself"));
                }
                let n = self.i(seq, 1)?.clamp(0, MAX_LIST as i64) as usize;
                let mut out = Vec::new();
                self.composites.push(seq);
                let r = (|| {
                    for k in 0..n {
                        let s = self.i(seq, 2 + k)?;
                        out.extend(self.curve(s, &x, depth + 1)?);
                    }
                    Ok(out)
                })();
                self.composites.pop();
                r
            }
            126 => {
                let nb = self.nurbs_curve(seq, &x)?;
                let (t0, t1) = nb.range();
                for k in 0..=4 {
                    nb.eval(t0 + (t1 - t0) * k as f64 / 4.0).ok_or_else(|| format!("entity {seq}: B-spline cannot be evaluated"))?;
                }
                let c = self.write_nurbs_curve(&nb);
                Ok(vec![Seg::along(c, |t| nb.eval(t0 + (t1 - t0) * t).unwrap_or([0.0; 3]))])
            }
            104 => {
                // Ellipse (form 1) in canonical position: A x² + C y² + F = 0.
                let u = self.f.unit;
                let (a, b, c, d, e, f) = (self.n(seq, 1)?, self.n(seq, 2)?, self.n(seq, 3)?, self.n(seq, 4)?, self.n(seq, 5)?, self.n(seq, 6)?);
                let zt = self.n(seq, 7)? * u;
                if de.form != 1 || b.abs() > 1e-12 || d.abs() > 1e-12 || e.abs() > 1e-12 || !(a * f < 0.0 && c * f < 0.0) {
                    return Err(format!("entity {seq}: only canonical ellipses are read"));
                }
                let (ra, rb) = ((-f / a).sqrt() * u, (-f / c).sqrt() * u);
                let s = [self.n(seq, 8)? * u, self.n(seq, 9)? * u];
                let en = [self.n(seq, 10)? * u, self.n(seq, 11)? * u];
                let t0 = (s[1] / rb).atan2(s[0] / ra);
                let mut t1 = (en[1] / rb).atan2(en[0] / ra);
                while t1 <= t0 + 1e-12 {
                    t1 += std::f64::consts::TAU;
                }
                let at = |t: f64| x.point([ra * t.cos(), rb * t.sin(), zt]);
                let z = unit(x.dir([0.0, 0.0, 1.0])).ok_or("degenerate ellipse frame")?;
                let xa = unit(x.dir([1.0, 0.0, 0.0])).ok_or("degenerate ellipse frame")?;
                let ax = self.o.placement(x.point([0.0, 0.0, zt]), z, xa);
                let k = x.scale();
                let cid = if (ra - rb).abs() < 1e-12 * ra {
                    self.o.add(format!("CIRCLE('',#{ax},{})", r(ra * k)))
                } else {
                    self.o.add(format!("ELLIPSE('',#{ax},{},{})", r(ra * k), r(rb * k)))
                };
                Ok(vec![Seg::along(cid, |t| at(t0 + (t1 - t0) * t))])
            }
            130 => Err(format!("entity {seq}: offset curves are not read")),
            t => Err(format!("curve type {t} is not read")),
        }
    }

    fn nurbs_curve(&self, seq: i64, x: &Xf) -> R<Nurbs> {
        let k = self.i(seq, 1)?;
        let m = self.i(seq, 2)?;
        if !(0..=MAX_LIST as i64).contains(&k) || !(1..=32).contains(&m) || m > k.max(1) + 1 {
            return Err(format!("entity {seq}: bad B-spline size"));
        }
        let (k, m) = (k as usize, m as usize);
        let nk = k + m + 2;
        let mut knots = Vec::with_capacity(nk);
        for j in 0..nk {
            knots.push(self.n(seq, 7 + j)?);
        }
        let base_w = 7 + nk;
        let base_p = base_w + k + 1;
        let mut pts = Vec::with_capacity(k + 1);
        for j in 0..=k {
            let w = self.n(seq, base_w + j)?;
            if !(w > 0.0) {
                return Err(format!("entity {seq}: non-positive weight"));
            }
            let p = x.point(self.pt(seq, base_p + 3 * j)?);
            pts.push([p[0] * w, p[1] * w, p[2] * w, w]);
        }
        if knots.windows(2).any(|w| w[1] < w[0]) {
            return Err(format!("entity {seq}: knots out of order"));
        }
        let nb = Nurbs { deg: m, knots, pts };
        let (a, b) = nb.range();
        if !(b > a) {
            return Err(format!("entity {seq}: empty parameter range"));
        }
        Ok(nb)
    }

    fn knot_text(knots: &[f64]) -> (String, String) {
        let mut ks: Vec<f64> = Vec::new();
        let mut ms: Vec<usize> = Vec::new();
        for &k in knots {
            match ks.last() {
                Some(&l) if (k - l).abs() <= 1e-14 * (1.0 + k.abs()) => {
                    if let Some(m) = ms.last_mut() {
                        *m += 1;
                    }
                }
                _ => {
                    ks.push(k);
                    ms.push(1);
                }
            }
        }
        (ms.iter().map(|m| m.to_string()).collect::<Vec<_>>().join(","), ks.iter().map(|k| r(*k)).collect::<Vec<_>>().join(","))
    }

    fn write_nurbs_curve(&mut self, nb: &Nurbs) -> u64 {
        let ids: Vec<String> = nb.pts.iter().map(|h| format!("#{}", self.o.point([h[0] / h[3], h[1] / h[3], h[2] / h[3]]))).collect();
        let (ms, ks) = Self::knot_text(&nb.knots);
        let rational = nb.pts.iter().any(|h| (h[3] - 1.0).abs() > 1e-12);
        if rational {
            let ws: Vec<String> = nb.pts.iter().map(|h| r(h[3])).collect();
            self.o.add(format!(
                "(BOUNDED_CURVE()B_SPLINE_CURVE({},({}),.UNSPECIFIED.,.U.,.U.)B_SPLINE_CURVE_WITH_KNOTS(({ms}),({ks}),.UNSPECIFIED.)CURVE()GEOMETRIC_REPRESENTATION_ITEM()RATIONAL_B_SPLINE_CURVE(({}))REPRESENTATION_ITEM(''))",
                nb.deg,
                ids.join(","),
                ws.join(",")
            ))
        } else {
            self.o.add(format!("B_SPLINE_CURVE_WITH_KNOTS('',{},({}),.UNSPECIFIED.,.U.,.U.,({ms}),({ks}),.UNSPECIFIED.)", nb.deg, ids.join(",")))
        }
    }

    /// A surface entity as a STEP surface id.
    fn surface(&mut self, seq: i64, xf: &Xf) -> R<u64> {
        let key = (usize::try_from(seq).unwrap_or(0), xf_key(xf));
        if let Some(id) = self.surf_cache.get(&key) {
            return Ok(*id);
        }
        let id = self.surface_new(seq, xf)?;
        self.surf_cache.insert(key, id);
        Ok(id)
    }

    fn surface_new(&mut self, seq: i64, xf: &Xf) -> R<u64> {
        let de = self.de(seq)?;
        let x = xf.then(&self.own_xf(seq, 0)?);
        let u = self.f.unit;
        let k = x.scale();
        match de.typ {
            128 => {
                let (k1, k2, m1, m2) = (self.i(seq, 1)?, self.i(seq, 2)?, self.i(seq, 3)?, self.i(seq, 4)?);
                if !(0..=4096).contains(&k1) || !(0..=4096).contains(&k2) || !(1..=32).contains(&m1) || !(1..=32).contains(&m2) {
                    return Err(format!("entity {seq}: bad B-spline surface size"));
                }
                let (k1, k2, m1, m2) = (k1 as usize, k2 as usize, m1 as usize, m2 as usize);
                if (k1 + 1) * (k2 + 1) > MAX_LIST {
                    return Err(format!("entity {seq}: B-spline surface too large"));
                }
                let (n1, n2) = (k1 + m1 + 2, k2 + m2 + 2);
                let mut s_knots = Vec::new();
                for j in 0..n1 {
                    s_knots.push(self.n(seq, 10 + j)?);
                }
                let mut t_knots = Vec::new();
                for j in 0..n2 {
                    t_knots.push(self.n(seq, 10 + n1 + j)?);
                }
                let base_w = 10 + n1 + n2;
                let cnt = (k1 + 1) * (k2 + 1);
                let base_p = base_w + cnt;
                // IGES runs the first index fastest: entry (i, j) is at j * (k1 + 1) + i.
                let mut rows = Vec::new();
                let mut wrows = Vec::new();
                let mut rational = false;
                for i in 0..=k1 {
                    let mut row = Vec::new();
                    let mut wrow = Vec::new();
                    for j in 0..=k2 {
                        let idx = j * (k1 + 1) + i;
                        let w = self.n(seq, base_w + idx)?;
                        if !(w > 0.0) {
                            return Err(format!("entity {seq}: non-positive weight"));
                        }
                        rational |= (w - 1.0).abs() > 1e-12;
                        let p = x.point(self.pt(seq, base_p + 3 * idx)?);
                        row.push(format!("#{}", self.o.point(p)));
                        wrow.push(r(w));
                    }
                    rows.push(format!("({})", row.join(",")));
                    wrows.push(format!("({})", wrow.join(",")));
                }
                let (ms, ks) = Self::knot_text(&s_knots);
                let (mt_, kt) = Self::knot_text(&t_knots);
                Ok(if rational {
                    self.o.add(format!(
                        "(BOUNDED_SURFACE()B_SPLINE_SURFACE({m1},{m2},({}),.UNSPECIFIED.,.U.,.U.,.U.)B_SPLINE_SURFACE_WITH_KNOTS(({ms}),({mt_}),({ks}),({kt}),.UNSPECIFIED.)GEOMETRIC_REPRESENTATION_ITEM()RATIONAL_B_SPLINE_SURFACE(({}))REPRESENTATION_ITEM('')SURFACE())",
                        rows.join(","),
                        wrows.join(",")
                    ))
                } else {
                    self.o.add(format!(
                        "B_SPLINE_SURFACE_WITH_KNOTS('',{m1},{m2},({}),.UNSPECIFIED.,.U.,.U.,.U.,({ms}),({mt_}),({ks}),({kt}),.UNSPECIFIED.)",
                        rows.join(",")
                    ))
                })
            }
            108 => {
                let n = [self.n(seq, 1)?, self.n(seq, 2)?, self.n(seq, 3)?];
                let d = self.n(seq, 4)? * u;
                let nn = unit(n).ok_or_else(|| format!("entity {seq}: plane without a normal"))?;
                let o = mul(nn, d / norm(n));
                let (o, z) = (x.point(o), unit(x.dir(nn)).ok_or("degenerate plane")?);
                let ax = self.o.placement(o, z, perp(z));
                Ok(self.o.add(format!("PLANE('',#{ax})")))
            }
            190 => {
                let o = self.point_entity(self.i(seq, 1)?, &x)?;
                let z = self.dir_entity(self.i(seq, 2)?, &x)?;
                let xd = match self.i(seq, 3) {
                    Ok(p) if p > 0 && de.form == 1 => self.dir_entity(p, &x).unwrap_or_else(|_| perp(z)),
                    _ => perp(z),
                };
                let ax = self.o.placement(o, z, xd);
                Ok(self.o.add(format!("PLANE('',#{ax})")))
            }
            192 | 194 => {
                let o = self.point_entity(self.i(seq, 1)?, &x)?;
                let z = self.dir_entity(self.i(seq, 2)?, &x)?;
                let rad = self.n(seq, 3)? * u * k;
                let (angle, ref_field) = if de.typ == 194 { (self.n(seq, 4)?.to_radians(), 5) } else { (0.0, 4) };
                let xd = match self.i(seq, ref_field) {
                    Ok(p) if p > 0 && de.form == 1 => self.dir_entity(p, &x).unwrap_or_else(|_| perp(z)),
                    _ => perp(z),
                };
                let ax = self.o.placement(o, z, xd);
                Ok(if de.typ == 192 {
                    self.o.add(format!("CYLINDRICAL_SURFACE('',#{ax},{})", r(rad)))
                } else {
                    self.o.add(format!("CONICAL_SURFACE('',#{ax},{},{})", r(rad), r(angle)))
                })
            }
            196 => {
                let o = self.point_entity(self.i(seq, 1)?, &x)?;
                let rad = self.n(seq, 2)? * u * k;
                let (z, xd) = match (self.i(seq, 3), self.i(seq, 4)) {
                    (Ok(a), Ok(b)) if a > 0 && b > 0 && de.form == 1 => (self.dir_entity(a, &x)?, self.dir_entity(b, &x)?),
                    _ => (
                        unit(x.dir([0.0, 0.0, 1.0])).ok_or("degenerate sphere frame")?,
                        unit(x.dir([1.0, 0.0, 0.0])).ok_or("degenerate sphere frame")?,
                    ),
                };
                let ax = self.o.placement(o, z, xd);
                Ok(self.o.add(format!("SPHERICAL_SURFACE('',#{ax},{})", r(rad))))
            }
            198 => {
                let o = self.point_entity(self.i(seq, 1)?, &x)?;
                let z = self.dir_entity(self.i(seq, 2)?, &x)?;
                let (big, small) = (self.n(seq, 3)? * u * k, self.n(seq, 4)? * u * k);
                let xd = match self.i(seq, 5) {
                    Ok(p) if p > 0 && de.form == 1 => self.dir_entity(p, &x).unwrap_or_else(|_| perp(z)),
                    _ => perp(z),
                };
                let ax = self.o.placement(o, z, xd);
                Ok(self.o.add(format!("TOROIDAL_SURFACE('',#{ax},{},{})", r(big), r(small))))
            }
            120 => {
                // Axis line, generatrix, start and end angles (the face's trim bounds the turn).
                let axis = self.i(seq, 1)?;
                let generatrix = self.i(seq, 2)?;
                if self.de(axis)?.typ != 110 {
                    return Err(format!("entity {seq}: revolution axis is not a line"));
                }
                let ax_x = x.then(&self.own_xf(axis, 0)?);
                let (a, b) = (ax_x.point(self.pt(axis, 1)?), ax_x.point(self.pt(axis, 4)?));
                let d = unit(sub(b, a)).ok_or("zero-length revolution axis")?;
                let segs = self.curve(generatrix, &x, 1)?;
                let [seg] = segs.as_slice() else { return Err(format!("entity {seq}: composite generatrix is not read")) };
                let (pa, dd) = (self.o.point(a), self.o.dir(d));
                let a1 = self.o.add(format!("AXIS1_PLACEMENT('',#{pa},#{dd})"));
                Ok(self.o.add(format!("SURFACE_OF_REVOLUTION('',#{},#{a1})", seg.curve)))
            }
            122 => {
                let segs = self.curve(self.i(seq, 1)?, &x, 1)?;
                let [seg] = segs.as_slice() else { return Err(format!("entity {seq}: composite directrix is not read")) };
                let end = x.point(self.pt(seq, 2)?);
                let v = sub(end, seg.a);
                let d = unit(v).ok_or("zero-length extrusion")?;
                let dd = self.o.dir(d);
                let vec = self.o.add(format!("VECTOR('',#{dd},{})", r(norm(v))));
                Ok(self.o.add(format!("SURFACE_OF_LINEAR_EXTRUSION('',#{},#{vec})", seg.curve)))
            }
            118 => Err(format!("entity {seq}: ruled surfaces are not read yet")),
            140 => Err(format!("entity {seq}: offset surfaces are not read")),
            t => Err(format!("surface type {t} is not read")),
        }
    }

    /// The natural boundary of a B-spline surface (its four edge curves), for trimmed surfaces
    /// with no outer curve.
    fn natural_boundary(&mut self, seq: i64, xf: &Xf) -> R<Vec<Seg>> {
        let de = self.de(seq)?;
        if de.typ != 128 {
            return Err(format!("entity {seq}: an untrimmed {} surface has no boundary", de.typ));
        }
        let x = xf.then(&self.own_xf(seq, 0)?);
        let (k1, k2, m1, m2) = (self.i(seq, 1)? as usize, self.i(seq, 2)? as usize, self.i(seq, 3)? as usize, self.i(seq, 4)? as usize);
        let (n1, n2) = (k1 + m1 + 2, k2 + m2 + 2);
        let s_knots: Vec<f64> = (0..n1).map(|j| self.n(seq, 10 + j)).collect::<R<_>>()?;
        let t_knots: Vec<f64> = (0..n2).map(|j| self.n(seq, 10 + n1 + j)).collect::<R<_>>()?;
        let base_w = 10 + n1 + n2;
        let base_p = base_w + (k1 + 1) * (k2 + 1);
        let h = |i: usize, j: usize| -> R<[f64; 4]> {
            let idx = j * (k1 + 1) + i;
            let w = self.n(seq, base_w + idx)?;
            let p = x.point(self.pt(seq, base_p + 3 * idx)?);
            Ok([p[0] * w, p[1] * w, p[2] * w, w])
        };
        // Clamped knot vectors: the boundary rows are the boundary curves.
        let row = |j: usize| -> R<Nurbs> { Ok(Nurbs { deg: m1, knots: s_knots.clone(), pts: (0..=k1).map(|i| h(i, j)).collect::<R<_>>()? }) };
        let col = |i: usize| -> R<Nurbs> { Ok(Nurbs { deg: m2, knots: t_knots.clone(), pts: (0..=k2).map(|j| h(i, j)).collect::<R<_>>()? }) };
        // Counter-clockwise in parameter space: v = v0 forward, u = u1 forward, v = v1 back, u = u0 back.
        let curves = [(row(0)?, false), (col(k1)?, false), (row(k2)?, true), (col(0)?, true)];
        let mut out = Vec::new();
        for (nb, rev) in curves {
            let (t0, t1) = nb.range();
            let ev = |t: f64| nb.eval(t0 + (t1 - t0) * t);
            let (Some(a), Some(b), Some(mid)) = (ev(0.0), ev(1.0), ev(0.5)) else { return Err("boundary cannot be evaluated".into()) };
            if norm(sub(a, b)) <= self.tol && norm(sub(a, mid)) <= self.tol {
                continue; // a pole
            }
            let c = self.write_nurbs_curve(&nb);
            let seg = Seg::along(c, |t| ev(t).unwrap_or([0.0; 3]));
            out.push(if rev { seg.reversed() } else { seg });
        }
        Ok(out)
    }
}

/// An edge after sewing: STEP curve, end vertices, sample points (curve direction).
struct Edge {
    curve: u64,
    /// The curve runs from `v[1]` to `v[0]`.
    rev: bool,
    v: [usize; 2],
    q: V3,
    mid: V3,
}

/// A face being assembled: surface, sense, loops of (edge, forward), colour.
struct FaceB {
    surface: u64,
    sense: bool,
    loops: Vec<Vec<(usize, bool)>>,
    color: Option<[f32; 3]>,
}

struct Sewer {
    tol: f64,
    verts: Vec<V3>,
    grid: HashMap<[i64; 3], Vec<usize>>,
    edges: Vec<Edge>,
    by_ends: HashMap<(usize, usize), Vec<usize>>,
}

impl Sewer {
    fn vertex(&mut self, p: V3) -> usize {
        // Neighbouring buckets are this key plus or minus one, so leave room for that: a
        // coordinate near f64's range saturates the cast and the addition would overflow.
        let c = |x: f64| (x / self.tol).floor().clamp(-9e15, 9e15) as i64;
        let key = [c(p[0]), c(p[1]), c(p[2])];
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    if let Some(list) = self.grid.get(&[key[0] + dx, key[1] + dy, key[2] + dz]) {
                        for &i in list {
                            if self.verts.get(i).is_some_and(|q| norm(sub(*q, p)) <= self.tol) {
                                return i;
                            }
                        }
                    }
                }
            }
        }
        self.verts.push(p);
        self.grid.entry(key).or_default().push(self.verts.len() - 1);
        self.verts.len() - 1
    }
    /// The edge for a segment (an existing one when another face already has it) and whether
    /// the segment runs along it.
    fn edge(&mut self, s: &Seg) -> (usize, bool) {
        let (a, b) = (self.vertex(s.a), self.vertex(s.b));
        let key = (a.min(b), a.max(b));
        let close = |x: V3, y: V3| norm(sub(x, y)) <= self.tol * 10.0 + 1e-9;
        if let Some(list) = self.by_ends.get(&key) {
            for &e in list {
                let Some(ed) = self.edges.get(e) else { continue };
                if !close(ed.mid, s.mid) {
                    continue;
                }
                let fwd = if a != b {
                    ed.v[0] == a
                } else {
                    // A closed edge: compare a point a quarter of the way along.
                    close(ed.q, s.q)
                };
                return (e, fwd);
            }
        }
        self.edges.push(Edge { curve: s.curve, rev: s.rev, v: [a, b], q: s.q, mid: s.mid });
        let e = self.edges.len() - 1;
        self.by_ends.entry(key).or_default().push(e);
        (e, true)
    }
}

/// Faces flipped so that every shared edge is used once each way (the trimmed surfaces of a
/// shell need not agree on their normals).
fn orient(faces: &mut [FaceB]) {
    let mut uses: HashMap<usize, Vec<(usize, bool)>> = HashMap::new();
    for (fi, f) in faces.iter().enumerate() {
        for &(e, d) in f.loops.iter().flatten() {
            uses.entry(e).or_default().push((fi, d));
        }
    }
    let mut flip: Vec<Option<bool>> = vec![None; faces.len()];
    for start in 0..faces.len() {
        if flip[start].is_some() {
            continue;
        }
        flip[start] = Some(false);
        let mut stack = vec![start];
        while let Some(fi) = stack.pop() {
            let fl = flip[fi].unwrap_or(false);
            let Some(f) = faces.get(fi) else { continue };
            for &(e, d) in f.loops.iter().flatten() {
                let d = d != fl;
                for &(g, dg) in uses.get(&e).into_iter().flatten() {
                    if g == fi || flip[g].is_some() {
                        continue;
                    }
                    // Neighbour must run the edge the other way.
                    flip[g] = Some(dg == d);
                    stack.push(g);
                }
            }
        }
    }
    for (f, fl) in faces.iter_mut().zip(flip) {
        if fl == Some(true) {
            f.sense = !f.sense;
            for l in &mut f.loops {
                l.reverse();
                for x in l.iter_mut() {
                    x.1 = !x.1;
                }
            }
        }
    }
}

fn color_of(t: &Tr, seq: i64) -> Option<[f32; 3]> {
    let de = t.de(seq).ok()?;
    match de.color {
        c if c < 0 => {
            let p = -c;
            if t.de(p).ok()?.typ != 314 {
                return None;
            }
            let ch = |i: usize| t.n(p, i).ok().map(|x| (x / 100.0).clamp(0.0, 1.0) as f32);
            Some([ch(1)?, ch(2)?, ch(3)?])
        }
        1 => Some([0.0, 0.0, 0.0]),
        2 => Some([1.0, 0.0, 0.0]),
        3 => Some([0.0, 1.0, 0.0]),
        4 => Some([0.0, 0.0, 1.0]),
        5 => Some([1.0, 1.0, 0.0]),
        6 => Some([1.0, 0.0, 1.0]),
        7 => Some([0.0, 1.0, 1.0]),
        8 => Some([1.0, 1.0, 1.0]),
        _ => None,
    }
}

/// Write faces sewn by `sw` as a shell; the shell's id and whether it closes.
fn write_shell(o: &mut Out, sw: &Sewer, faces: &[FaceB], styles: &mut Vec<(u64, [f32; 3])>) -> (u64, bool) {
    let vids: Vec<u64> = sw
        .verts
        .iter()
        .map(|p| {
            let c = o.point(*p);
            o.add(format!("VERTEX_POINT('',#{c})"))
        })
        .collect();
    let mut eids: HashMap<usize, u64> = HashMap::new();
    let mut count: HashMap<usize, (usize, usize)> = HashMap::new();
    for &(e, d) in faces.iter().flat_map(|f| f.loops.iter().flatten()) {
        let c = count.entry(e).or_insert((0, 0));
        if d {
            c.0 += 1;
        } else {
            c.1 += 1;
        }
    }
    let mut fids = Vec::new();
    for f in faces {
        let mut bounds = Vec::new();
        for (k, l) in f.loops.iter().enumerate() {
            let mut oes = Vec::new();
            for &(e, d) in l {
                let Some(ed) = sw.edges.get(e) else { continue };
                let id = match eids.get(&e) {
                    Some(id) => *id,
                    None => {
                        let (Some(a), Some(b)) = (vids.get(ed.v[0]), vids.get(ed.v[1])) else { continue };
                        let id = o.add(format!("EDGE_CURVE('',#{a},#{b},#{},{})", ed.curve, if ed.rev { ".F." } else { ".T." }));
                        eids.insert(e, id);
                        id
                    }
                };
                oes.push(format!("#{}", o.add(format!("ORIENTED_EDGE('',*,*,#{id},{})", if d { ".T." } else { ".F." }))));
            }
            if oes.is_empty() {
                continue;
            }
            let lp = o.add(format!("EDGE_LOOP('',({}))", oes.join(",")));
            let kind = if k == 0 { "FACE_OUTER_BOUND" } else { "FACE_BOUND" };
            bounds.push(format!("#{}", o.add(format!("{kind}('',#{lp},.T.)"))));
        }
        if bounds.is_empty() {
            continue;
        }
        let id = o.add(format!("ADVANCED_FACE('',({}),#{},{})", bounds.join(","), f.surface, if f.sense { ".T." } else { ".F." }));
        if let Some(c) = f.color {
            styles.push((id, c));
        }
        fids.push(format!("#{id}"));
    }
    let closed = !count.is_empty() && count.values().all(|c| *c == (1, 1));
    let kind = if closed { "CLOSED_SHELL" } else { "OPEN_SHELL" };
    (o.add(format!("{kind}('',({}))", fids.join(","))), closed)
}

/// IGES text restated as a STEP exchange (millimetres), with what could not be read.
/// This was the one importer stage outside every panic guard, so a panic here reached the CLI
/// and the wasm build, where nothing catches it.
pub fn iges_to_step(text: &str) -> std::result::Result<(String, Vec<String>), String> {
    crate::guard("IGES import", || translate(text).map_err(crate::KernelError::Invalid)).map_err(|e| match e {
        crate::KernelError::Invalid(m) => m,
        other => other.to_string(),
    })
}

fn translate(text: &str) -> std::result::Result<(String, Vec<String>), String> {
    if text.len() > MAX_IGES_BYTES {
        return Err("IGES file too large".into());
    }
    let f = parse(text)?;
    let tol = (f.resolution * f.unit).clamp(1e-6, 0.05).max(1e-5);
    let mut t =
        Tr { f: &f, o: Out { lines: Vec::new() }, warnings: Vec::new(), tol, surf_cache: HashMap::new(), curve_work: 0, composites: Vec::new() };
    // Header: units and context.
    let mm = t.o.add("(LENGTH_UNIT()NAMED_UNIT(*)SI_UNIT(.MILLI.,.METRE.))");
    let rad = t.o.add("(NAMED_UNIT(*)PLANE_ANGLE_UNIT()SI_UNIT($,.RADIAN.))");
    let sr = t.o.add("(NAMED_UNIT(*)SI_UNIT($,.STERADIAN.)SOLID_ANGLE_UNIT())");
    let unc = t.o.add(format!("UNCERTAINTY_MEASURE_WITH_UNIT(LENGTH_MEASURE({}),#{mm},'distance_accuracy_value','')", r(tol)));
    let ctx = t.o.add(format!(
        "(GEOMETRIC_REPRESENTATION_CONTEXT(3)GLOBAL_UNCERTAINTY_ASSIGNED_CONTEXT((#{unc}))GLOBAL_UNIT_ASSIGNED_CONTEXT((#{mm},#{rad},#{sr}))REPRESENTATION_CONTEXT('',''))"
    ));
    let mut seqs: Vec<usize> = f.des.keys().copied().collect();
    seqs.sort();
    // Entities used by others (a B-rep's faces, a trimmed surface's curves) are not roots;
    // physically dependent entities have status digits 01 in columns 3–4, but files are lax:
    // collect references instead.
    let assembly = assembly::Assembly::read(&t, &seqs)?;
    let mut solids = HashMap::new();
    let mut styles: Vec<(u64, [f32; 3])> = Vec::new();
    let mut n = 0usize;
    let mut fallback = String::new();
    let watch = crate::watch::current();
    for &s in &seqs {
        if watch.as_ref().is_some_and(|w| w.cancelled()) {
            return Err("IGES import cancelled".into());
        }
        let Some(de) = f.des.get(&s) else { continue };
        if de.typ != 186 {
            continue;
        }
        n += 1;
        let name = if let Some(name) = assembly::entity_name(&f, s) {
            name
        } else {
            fallback.clear();
            let _ = write!(fallback, "Body{n}");
            fallback.as_str()
        };
        match brep(&mut t, s as i64, &mut styles) {
            Ok((id, color)) => {
                if let Some(l) = t.o.lines.get_mut(id as usize - 1) {
                    *l = l.replacen("('',", &format!("('{}',", name.replace('\'', "''")), 1);
                }
                if let Some(c) = color {
                    styles.push((id, c));
                }
                solids.insert(s, id);
            }
            Err(e) => t.warn(format!("solid {name}: not read: {e}")),
        }
    }
    assembly.write(&mut t, &seqs, &solids, &mut styles, ctx)?;
    for (item, c) in styles {
        let rgb = t.o.add(format!("COLOUR_RGB('',{},{},{})", r(c[0] as f64), r(c[1] as f64), r(c[2] as f64)));
        let fasc = t.o.add(format!("FILL_AREA_STYLE_COLOUR('',#{rgb})"));
        let fas = t.o.add(format!("FILL_AREA_STYLE('',(#{fasc}))"));
        let ssfa = t.o.add(format!("SURFACE_STYLE_FILL_AREA(#{fas})"));
        let sss = t.o.add(format!("SURFACE_SIDE_STYLE('',(#{ssfa}))"));
        let ssu = t.o.add(format!("SURFACE_STYLE_USAGE(.BOTH.,#{sss})"));
        let psa = t.o.add(format!("PRESENTATION_STYLE_ASSIGNMENT((#{ssu}))"));
        t.o.add(format!("STYLED_ITEM('color',(#{psa}),#{item})"));
    }
    let mut s = String::from(
        "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('IGES import'),'2;1');\nFILE_NAME('','',(''),(''),'SolidCraft IGES','','');\nFILE_SCHEMA(('AUTOMOTIVE_DESIGN'));\nENDSEC;\nDATA;\n",
    );
    for (i, l) in t.o.lines.iter().enumerate() {
        let _ = writeln!(s, "#{}={l};", i + 1);
    }
    s.push_str("ENDSEC;\nEND-ISO-10303-21;\n");
    Ok((s, t.warnings))
}

/// Sew surfaces within their owning group, keeping different parts separate.
fn sew_surfaces(
    t: &mut Tr,
    seqs: impl Iterator<Item = usize>,
    name: &str,
    styles: &mut Vec<(u64, [f32; 3])>,
    faces: &mut Vec<FaceB>,
    sw: &mut Sewer,
) -> R<Option<u64>> {
    faces.clear();
    sw.verts.clear();
    sw.grid.clear();
    sw.edges.clear();
    sw.by_ends.clear();
    let watch = crate::watch::current();
    for seq in seqs {
        if watch.as_ref().is_some_and(|w| w.cancelled()) {
            return Err("IGES import cancelled".into());
        }
        match trimmed(t, sw, seq as i64) {
            Ok(face) => faces.push(face),
            Err(e) => t.warn(format!("surface {seq}: not read: {e}")),
        }
    }
    if faces.is_empty() {
        return Ok(None);
    }
    orient(faces);
    let (shell, closed) = write_shell(&mut t.o, sw, faces, styles);
    let name = name.replace('\'', "''");
    Ok(Some(if closed {
        t.o.add(format!("MANIFOLD_SOLID_BREP('{name}',#{shell})"))
    } else {
        t.warn("the surfaces do not close into a solid: imported as an open body");
        t.o.add(format!("SHELL_BASED_SURFACE_MODEL('{name}',(#{shell}))"))
    }))
}

/// A trimmed (144) or bounded (143) surface, or a bare B-spline surface, as a face.
fn trimmed(t: &mut Tr, sw: &mut Sewer, seq: i64) -> R<FaceB> {
    let de = t.de(seq)?;
    let xf = t.own_xf(seq, 0)?;
    let color = color_of(t, seq);
    let loop_of = |_t: &mut Tr, sw: &mut Sewer, segs: Vec<Seg>| -> Vec<(usize, bool)> {
        let tol = sw.tol;
        segs.iter().filter(|s| norm(sub(s.a, s.b)) > tol || norm(sub(s.a, s.mid)) > tol).map(|s| sw.edge(s)).collect()
    };
    match de.typ {
        144 => {
            let surf_seq = t.i(seq, 1)?;
            let surface = t.surface(surf_seq, &xf)?;
            let n1 = t.i(seq, 2)?;
            let n2 = t.i(seq, 3)?.clamp(0, MAX_LIST as i64) as usize;
            let mut loops = Vec::new();
            let outer = if n1 == 0 { t.natural_boundary(surf_seq, &xf)? } else { cos_curve(t, t.i(seq, 4)?, &xf)? };
            loops.push(loop_of(t, sw, outer));
            for k in 0..n2 {
                let segs = cos_curve(t, t.i(seq, 5 + k)?, &xf)?;
                loops.push(loop_of(t, sw, segs));
            }
            loops.retain(|l| !l.is_empty());
            if loops.is_empty() {
                return Err("no boundary".into());
            }
            Ok(FaceB { surface, sense: true, loops, color })
        }
        143 => {
            let surf_seq = t.i(seq, 2)?;
            let surface = t.surface(surf_seq, &xf)?;
            let n = t.i(seq, 3)?.clamp(0, MAX_LIST as i64) as usize;
            let mut loops = Vec::new();
            for k in 0..n {
                let b = t.i(seq, 4 + k)?;
                let segs = boundary_141(t, b, &xf)?;
                loops.push(loop_of(t, sw, segs));
            }
            loops.retain(|l| !l.is_empty());
            if loops.is_empty() {
                return Err("no boundary".into());
            }
            Ok(FaceB { surface, sense: true, loops, color })
        }
        _ => {
            let surface = t.surface(seq, &Xf::ID)?;
            let outer = t.natural_boundary(seq, &Xf::ID)?;
            let l = loop_of(t, sw, outer);
            Ok(FaceB { surface, sense: true, loops: vec![l], color })
        }
    }
}

/// The model-space curve of a curve on a surface (142).
fn cos_curve(t: &mut Tr, seq: i64, xf: &Xf) -> R<Vec<Seg>> {
    let de = t.de(seq)?;
    if de.typ != 142 {
        return t.curve(seq, xf, 0);
    }
    let x = xf.then(&t.own_xf(seq, 0)?);
    let c = t.i(seq, 4)?;
    if c <= 0 {
        return Err("a curve on a surface without its model-space curve".into());
    }
    t.curve(c, &x, 0)
}

/// The model-space curves of a boundary (141), each in its sense.
fn boundary_141(t: &mut Tr, seq: i64, xf: &Xf) -> R<Vec<Seg>> {
    let de = t.de(seq)?;
    if de.typ != 141 {
        return t.curve(seq, xf, 0);
    }
    let x = xf.then(&t.own_xf(seq, 0)?);
    let n = t.i(seq, 4)?.clamp(0, MAX_LIST as i64) as usize;
    let mut out = Vec::new();
    let mut idx = 5;
    for _ in 0..n {
        let c = t.i(seq, idx)?;
        let sense = t.i(seq, idx + 1)?;
        let k = t.i(seq, idx + 2)?.clamp(0, 1024) as usize;
        idx += 3 + k;
        let mut segs = t.curve(c, &x, 0)?;
        if sense == 2 {
            segs = segs.into_iter().rev().map(Seg::reversed).collect();
        }
        out.extend(segs);
    }
    Ok(out)
}

/// A manifold solid B-rep (186): its solid id and colour.
fn brep(t: &mut Tr, seq: i64, face_styles: &mut Vec<(u64, [f32; 3])>) -> R<(u64, Option<[f32; 3]>)> {
    let xf = t.own_xf(seq, 0)?;
    let shell = t.i(seq, 1)?;
    let sof = t.i(seq, 2)? != 0;
    let nv = t.i(seq, 3)?.clamp(0, 10_000) as usize;
    let mut shells = Vec::new();
    let (s0, closed) = brep_shell(t, shell, sof, &xf, face_styles)?;
    if !closed {
        t.warn("a solid's shell does not close");
    }
    shells.push(s0);
    for k in 0..nv {
        let v = t.i(seq, 4 + 2 * k)?;
        let vof = t.i(seq, 5 + 2 * k)? != 0;
        match brep_shell(t, v, vof, &xf, face_styles) {
            Ok((s, _)) => shells.push(s),
            Err(e) => t.warn(format!("void shell not read: {e}")),
        }
    }
    let id = if shells.len() == 1 {
        t.o.add(format!("MANIFOLD_SOLID_BREP('',#{s0})"))
    } else {
        let voids: Vec<String> = shells.iter().skip(1).map(|s| format!("#{s}")).collect();
        t.o.add(format!("BREP_WITH_VOIDS('',#{s0},({}))", voids.join(",")))
    };
    Ok((id, color_of(t, seq)))
}

/// A B-rep shell (514) of vertex lists (502), edge lists (504), loops (508) and faces (510).
fn brep_shell(t: &mut Tr, seq: i64, outward: bool, xf: &Xf, styles: &mut Vec<(u64, [f32; 3])>) -> R<(u64, bool)> {
    if t.de(seq)?.typ != 514 {
        return Err(format!("entity {seq} is not a shell"));
    }
    let nf = t.i(seq, 1)?.clamp(0, MAX_LIST as i64) as usize;
    let mut sw = Sewer { tol: t.tol, verts: Vec::new(), grid: HashMap::new(), edges: Vec::new(), by_ends: HashMap::new() };
    // Edge-list edges already turned into sewn edges.
    let mut done: HashMap<(i64, i64), (usize, bool)> = HashMap::new();
    let mut faces = Vec::new();
    for k in 0..nf {
        let fseq = t.i(seq, 2 + 2 * k)?;
        let of = t.i(seq, 3 + 2 * k)? != 0;
        match brep_face(t, &mut sw, &mut done, fseq, xf) {
            Ok(mut fb) => {
                // The face's normal: its surface's, flipped when the face or shell says so. Loops
                // run counter-clockwise about the surface normal, so a flipped face reverses them.
                fb.sense = of == outward;
                if !fb.sense {
                    for l in &mut fb.loops {
                        l.reverse();
                        for x in l.iter_mut() {
                            x.1 = !x.1;
                        }
                    }
                }
                faces.push(fb);
            }
            Err(e) => t.warn(format!("face {fseq}: not read: {e}")),
        }
    }
    if faces.is_empty() {
        return Err("no face could be read".into());
    }
    orient(&mut faces);
    Ok(write_shell(&mut t.o, &sw, &faces, styles))
}

fn brep_face(t: &mut Tr, sw: &mut Sewer, done: &mut HashMap<(i64, i64), (usize, bool)>, seq: i64, xf: &Xf) -> R<FaceB> {
    if t.de(seq)?.typ != 510 {
        return Err(format!("entity {seq} is not a face"));
    }
    let surface = t.surface(t.i(seq, 1)?, xf)?;
    let nl = t.i(seq, 2)?.clamp(0, MAX_LIST as i64) as usize;
    let mut loops = Vec::new();
    for k in 0..nl {
        let lseq = t.i(seq, 4 + k)?;
        if t.de(lseq)?.typ != 508 {
            continue;
        }
        let ne = t.i(lseq, 1)?.clamp(0, MAX_LIST as i64) as usize;
        let mut idx = 2;
        let mut l = Vec::new();
        for _ in 0..ne {
            let typ = t.i(lseq, idx)?;
            let list = t.i(lseq, idx + 1)?;
            let ndx = t.i(lseq, idx + 2)?;
            let of = t.i(lseq, idx + 3)? != 0;
            let kp = t.i(lseq, idx + 4)?.clamp(0, 1024) as usize;
            idx += 5 + 2 * kp;
            if typ != 0 {
                continue; // a vertex (a pole): no edge
            }
            let (e, fwd) = match done.get(&(list, ndx)) {
                Some(x) => *x,
                None => {
                    let x = brep_edge(t, sw, list, ndx, xf)?;
                    done.insert((list, ndx), x);
                    x
                }
            };
            l.push((e, fwd == of));
        }
        if !l.is_empty() {
            loops.push(l);
        }
    }
    if loops.is_empty() {
        return Err("no loops".into());
    }
    Ok(FaceB { surface, sense: true, loops, color: color_of(t, seq) })
}

/// Edge `ndx` (1-based) of an edge list (504) as a sewn edge.
fn brep_edge(t: &mut Tr, sw: &mut Sewer, list: i64, ndx: i64, xf: &Xf) -> R<(usize, bool)> {
    if t.de(list)?.typ != 504 {
        return Err(format!("entity {list} is not an edge list"));
    }
    let n = t.i(list, 1)?;
    if ndx < 1 || ndx > n {
        return Err(format!("edge {ndx} of a list of {n}"));
    }
    let base = 2 + 5 * (ndx as usize - 1);
    let curve = t.i(list, base)?;
    let vertex = |t: &Tr, vl: i64, vi: i64| -> R<V3> {
        if t.de(vl)?.typ != 502 {
            return Err(format!("entity {vl} is not a vertex list"));
        }
        let nv = t.i(vl, 1)?;
        if vi < 1 || vi > nv {
            return Err(format!("vertex {vi} of a list of {nv}"));
        }
        let x = xf.then(&t.own_xf(vl, 0)?);
        Ok(x.point(t.pt(vl, 2 + 3 * (vi as usize - 1))?))
    };
    let a = vertex(t, t.i(list, base + 1)?, t.i(list, base + 2)?)?;
    let b = vertex(t, t.i(list, base + 3)?, t.i(list, base + 4)?)?;
    let segs = t.curve(curve, xf, 0)?;
    let [s] = segs.as_slice() else { return Err("composite edge curves are not read".into()) };
    // The edge runs from its start vertex to its end vertex; the curve may run the other way.
    let along = norm(sub(s.a, a)) + norm(sub(s.b, b)) <= norm(sub(s.a, b)) + norm(sub(s.b, a));
    let mut s = if along { *s } else { s.reversed() };
    s.a = a;
    s.b = b;
    Ok(sw.edge(&s))
}

/// Read IGES text: its solids, or its surfaces sewn into shells, as the STEP reader gives them.
pub fn iges_import(text: &str) -> crate::Result<crate::StepImport> {
    let (step, warnings) = iges_to_step(text).map_err(crate::KernelError::Invalid)?;
    let mut imp = crate::step_import(&step)?;
    imp.warnings.splice(0..0, warnings);
    imp.schema = "IGES".into();
    Ok(imp)
}

#[cfg(test)]
mod tests;
