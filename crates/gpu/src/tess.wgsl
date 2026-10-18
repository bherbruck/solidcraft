// Display tessellation: surface points and normals at each vertex's (u, v).
//
// The GPU twin of solidcraft_geom::tess::EvalSurface::{derivs, eval}: the same surface forms,
// the same algorithms (The NURBS Book A2.1 span search, A2.2 basis functions, A2.3 first
// derivatives, the rational quotient rule), in f32.

const PLANE: u32 = 0u;
const NURBS: u32 = 1u;
const REVOLVED: u32 = 2u;
// Basis slots for the highest degree (MAX_SPLINE_DEGREE = 7).
const W: u32 = 8u;

struct Surface {
    kind: u32,
    // Degrees and control point counts (u, v); a revolved surface's profile uses pu, nu.
    pu: u32,
    pv: u32,
    nu: u32,
    nv: u32,
    // Offsets into `data` (f32 units): knots along u, knots along v, homogeneous control
    // points (4 each, u fastest), and the placement (plane: origin, du, dv; revolved: origin,
    // axis, row-major 3×4 transform).
    knots_u: u32,
    knots_v: u32,
    ctrl: u32,
    extra: u32,
    // Revolved: parameters exchanged; mirror transform (normal reversed).
    swap: u32,
    mirror: u32,
    // NURBS: bit 0 the surface joins itself along u, bit 1 along v (the direction is periodic).
    closed: u32,
};

struct Vertex {
    u: f32,
    v: f32,
    surface: u32,
    // bit 0: normal reversed (face against its surface); bit 1: position fixed (p).
    flags: u32,
    p: vec4<f32>,
};

struct Params {
    count: u32,
    _p0: u32,
    _p1: u32,
    _p2: u32,
};

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read> surfaces: array<Surface>;
@group(0) @binding(2) var<storage, read> data: array<f32>;
@group(0) @binding(3) var<storage, read> vertices: array<Vertex>;
// Out: position (xyz, w = 1 when evaluated) and normal (xyz) per vertex.
@group(0) @binding(4) var<storage, read_write> out: array<vec4<f32>>;

struct Derivs {
    p: vec3<f32>,
    du: vec3<f32>,
    dv: vec3<f32>,
    ok: bool,
};

fn vec3_at(o: u32) -> vec3<f32> {
    return vec3<f32>(data[o], data[o + 1u], data[o + 2u]);
}

fn ctrl_at(o: u32) -> vec4<f32> {
    return vec4<f32>(data[o], data[o + 1u], data[o + 2u], data[o + 3u]);
}

// The last non-empty span starting at or before t, clamped to p ≤ k < n (A2.1, by bisection
// over the knots, which do not decrease).
fn span_of(k0: u32, p: u32, n: u32, t: f32) -> u32 {
    if (t >= data[k0 + n]) {
        // At or past the end: the last non-empty span.
        var k = n - 1u;
        while (k > p && data[k0 + k] >= data[k0 + k + 1u]) {
            k = k - 1u;
        }
        return k;
    }
    if (t < data[k0 + p]) {
        return p;
    }
    var lo = p;
    var hi = n;
    // Invariant: knot[lo] ≤ t < knot[hi].
    while (hi - lo > 1u) {
        let mid = (lo + hi) / 2u;
        if (data[k0 + mid] <= t) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    return lo;
}

// The arrays below are one-dimensional locals, written by loop counters and returned whole:
// D3D12's older shader compiler (FXC) must unroll a loop that writes an array inside a struct
// (or a two-dimensional one) by a counter, and it cannot unroll loops whose trip count is only
// known at run time, so such a loop fails to compile there ("unable to unroll loop").

// The non-zero basis functions of degree p on span `span` (The NURBS Book A2.2).
fn basis_funs(k0: u32, span: u32, p: u32, t: f32) -> array<f32, W> {
    var n: array<f32, W>;
    n[0] = 1.0;
    for (var j = 1u; j <= p; j = j + 1u) {
        var saved = 0.0;
        for (var r = 0u; r < j; r = r + 1u) {
            // right[r + 1] and left[j - r] of A2.2 (span ≥ p ≥ j, so no underflow).
            let right = data[k0 + span + r + 1u] - t;
            let left = t - data[k0 + span + 1u + r - j];
            let den = right + left;
            var temp = 0.0;
            if (den != 0.0) {
                temp = n[r] / den;
            }
            n[r] = saved + right * temp;
            saved = left * temp;
        }
        n[j] = saved;
    }
    return n;
}

// Their first derivatives, from the degree p - 1 functions on the same span (A2.3, n = 1).
fn basis_ders(k0: u32, span: u32, p: u32, t: f32) -> array<f32, W> {
    var d: array<f32, W>;
    if (p == 0u) {
        return d;
    }
    let lower = basis_funs(k0, span, p - 1u, t);
    for (var r = 0u; r <= p; r = r + 1u) {
        var x = 0.0;
        if (r >= 1u) {
            let den = data[k0 + span + r] - data[k0 + span + r - p];
            if (den != 0.0) {
                x = x + lower[r - 1u] / den;
            }
        }
        if (r < p) {
            let den = data[k0 + span + r + 1u] - data[k0 + span + r + 1u - p];
            if (den != 0.0) {
                x = x - lower[r] / den;
            }
        }
        d[r] = x * f32(p);
    }
    return d;
}

// Point and derivative of a rational function from its homogeneous value and derivative.
fn rational_point(a: vec4<f32>) -> vec3<f32> {
    return a.xyz / a.w;
}

fn rational_der(a: vec4<f32>, da: vec4<f32>) -> vec3<f32> {
    return (da.xyz - rational_point(a) * da.w) / a.w;
}

// A parameter outside the knot range of a direction the surface joins itself along, moved by
// whole ranges back into it (the CPU evaluator's `EvalSurface::derivs` does the same).
fn wrap_closed(knots: u32, p: u32, n: u32, t: f32, closed: bool) -> f32 {
    if (!closed) { return t; }
    let a = data[knots + p];
    let b = data[knots + n];
    if (!(b > a) || (t >= a && t <= b)) { return t; }
    return a + (t - a) - floor((t - a) / (b - a)) * (b - a);
}

fn nurbs(s: Surface, u_in: f32, v_in: f32) -> Derivs {
    var r: Derivs;
    let u = wrap_closed(s.knots_u, s.pu, s.nu, u_in, (s.closed & 1u) != 0u);
    let v = wrap_closed(s.knots_v, s.pv, s.nv, v_in, (s.closed & 2u) != 0u);
    let su = span_of(s.knots_u, s.pu, s.nu, u);
    let sv = span_of(s.knots_v, s.pv, s.nv, v);
    let bu0 = basis_funs(s.knots_u, su, s.pu, u);
    let bu1 = basis_ders(s.knots_u, su, s.pu, u);
    let bv0 = basis_funs(s.knots_v, sv, s.pv, v);
    let bv1 = basis_ders(s.knots_v, sv, s.pv, v);
    var a = vec4<f32>(0.0);
    var au = vec4<f32>(0.0);
    var av = vec4<f32>(0.0);
    for (var j = 0u; j <= s.pv; j = j + 1u) {
        for (var i = 0u; i <= s.pu; i = i + 1u) {
            let c = ctrl_at(s.ctrl + 4u * ((sv + j - s.pv) * s.nu + su + i - s.pu));
            a = a + bu0[i] * bv0[j] * c;
            au = au + bu1[i] * bv0[j] * c;
            av = av + bu0[i] * bv1[j] * c;
        }
    }
    r.ok = abs(a.w) > 0.0;
    if (r.ok) {
        r.p = rational_point(a);
        r.du = rational_der(a, au);
        r.dv = rational_der(a, av);
    }
    return r;
}

// Rodrigues rotation of x by angle (sin, cos) about the unit axis k.
fn rotate(x: vec3<f32>, k: vec3<f32>, sn: f32, cs: f32) -> vec3<f32> {
    return x * cs + cross(k, x) * sn + k * (dot(k, x) * (1.0 - cs));
}

fn revolved(s: Surface, u: f32, v: f32) -> Derivs {
    var r: Derivs;
    var t = u;
    var angle = v;
    if (s.swap != 0u) {
        t = v;
        angle = u;
    }
    // The profile curve: a one-row B-spline.
    let span = span_of(s.knots_u, s.pu, s.nu, t);
    let b0 = basis_funs(s.knots_u, span, s.pu, t);
    let b1 = basis_ders(s.knots_u, span, s.pu, t);
    var c = vec4<f32>(0.0);
    var dc = vec4<f32>(0.0);
    for (var i = 0u; i <= s.pu; i = i + 1u) {
        let h = ctrl_at(s.ctrl + 4u * (span + i - s.pu));
        c = c + b0[i] * h;
        dc = dc + b1[i] * h;
    }
    r.ok = abs(c.w) > 0.0;
    if (!r.ok) {
        return r;
    }
    let o = vec3_at(s.extra);
    let k = vec3_at(s.extra + 3u);
    let m = s.extra + 6u;
    let row0 = vec4<f32>(data[m], data[m + 1u], data[m + 2u], data[m + 3u]);
    let row1 = vec4<f32>(data[m + 4u], data[m + 5u], data[m + 6u], data[m + 7u]);
    let row2 = vec4<f32>(data[m + 8u], data[m + 9u], data[m + 10u], data[m + 11u]);
    let sn = sin(angle);
    let cs = cos(angle);
    let radius = rotate(rational_point(c) - o, k, sn, cs);
    let p = o + radius;
    let dt = rotate(rational_der(c, dc), k, sn, cs);
    let da = cross(k, radius);
    let pp = vec3<f32>(dot(row0.xyz, p) + row0.w, dot(row1.xyz, p) + row1.w, dot(row2.xyz, p) + row2.w);
    let ldt = vec3<f32>(dot(row0.xyz, dt), dot(row1.xyz, dt), dot(row2.xyz, dt));
    let lda = vec3<f32>(dot(row0.xyz, da), dot(row1.xyz, da), dot(row2.xyz, da));
    r.p = pp;
    if (s.swap != 0u) {
        r.du = lda;
        r.dv = ldt;
    } else {
        r.du = ldt;
        r.dv = lda;
    }
    return r;
}

fn derivs(s: Surface, u: f32, v: f32) -> Derivs {
    if (s.kind == PLANE) {
        var r: Derivs;
        r.du = vec3_at(s.extra + 3u);
        r.dv = vec3_at(s.extra + 6u);
        r.p = vec3_at(s.extra) + r.du * u + r.dv * v;
        r.ok = true;
        return r;
    }
    if (s.kind == NURBS) {
        return nurbs(s, u, v);
    }
    return revolved(s, u, v);
}

fn oriented(s: Surface, n: vec3<f32>) -> vec3<f32> {
    if (s.mirror != 0u) {
        return -n;
    }
    return n;
}

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    if (i >= params.count) {
        return;
    }
    let vert = vertices[i];
    let s = surfaces[vert.surface];
    let d = derivs(s, vert.u, vert.v);
    var n = oriented(s, cross(d.du, d.dv));
    let scale = length(d.du) * length(d.dv);
    if (!(length(n) > 1e-6 * scale) || !d.ok) {
        // A degenerate point (a pole): the normal from a step inside.
        let h = 1e-4 * (1.0 + max(abs(vert.u), abs(vert.v)));
        // Steps (h, h), (-h, h), (h, -h), (-h, -h), without an array indexed by k (FXC).
        for (var k = 0u; k < 4u; k = k + 1u) {
            let du = select(h, -h, (k & 1u) != 0u);
            let dv = select(h, -h, (k & 2u) != 0u);
            let e = derivs(s, vert.u + du, vert.v + dv);
            let m = oriented(s, cross(e.du, e.dv));
            if (e.ok && length(m) > 0.0) {
                n = m;
                break;
            }
        }
    }
    var nn = vec3<f32>(0.0, 0.0, 1.0);
    if (length(n) > 0.0) {
        nn = normalize(n);
    }
    if ((vert.flags & 1u) != 0u) {
        nn = -nn;
    }
    var p = d.p;
    if ((vert.flags & 2u) != 0u) {
        p = vert.p.xyz;
    }
    out[2u * i] = vec4<f32>(p, 1.0);
    out[2u * i + 1u] = vec4<f32>(nn, 0.0);
}
