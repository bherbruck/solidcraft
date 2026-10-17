// Original software BVH and unidirectional Monte Carlo integrator. All directions
// at a surface point away from it, matching render::surface's GGX/Smith oracle.
struct Triangle {
    p0: vec4<f32>, p1: vec4<f32>, p2: vec4<f32>,
    n0: vec4<f32>, n1: vec4<f32>, n2: vec4<f32>,
    // base.w: dielectric flag, parameters.w: thin-sheet thickness (-1 = solid).
    // optics.xyz: extinction per mm, optics.w: body/medium identity (1..16384).
    base: vec4<f32>, parameters: vec4<f32>, optics: vec4<f32>,
    coat: vec4<f32>, // weight, perceptual roughness, IOR, shadow transmission flag
};
struct Node { lo: vec3<f32>, first: u32, hi: vec3<f32>, count: u32 };
struct AreaLight { center: vec4<f32>, u: vec4<f32>, v: vec4<f32>, radiance: vec4<f32> };
struct Params {
    // width, height, sample index, pixel offset
    image: vec4<u32>,
    // seed, maximum depth, tile length, reserved
    sampling: vec4<u32>,
    eye: vec4<f32>, right: vec4<f32>, up: vec4<f32>, back: vec4<f32>,
    // ray epsilon, test environment radiance, studio scale, reserved
    lighting: vec4<f32>,
    environment: vec4<f32>, // RGB radiance, enabled flag
    light_count: vec4<f32>, // w: rectangle count
    area_lights: array<AreaLight,4>,
    environment_map: vec4<f32>, // cosine rotation, sine rotation, intensity, enabled
    sun_direction: vec4<f32>, // normalized toward sun, cosine angular radius
    sun_radiance: vec4<f32>, // linear RGB, cone density (zero when disabled)

};
@group(0) @binding(0) var<uniform> u: Params;
@group(0) @binding(1) var<storage, read> geometry: array<u32>;
@group(0) @binding(2) var<storage, read> nodes: array<Node>;
@group(0) @binding(3) var<storage, read_write> accumulation: array<vec4<f32>>;
struct EnvironmentTexel { rgb: vec3<f32>, pdf: f32, threshold: f32, alternate: u32, padding: vec2<f32> };
struct EnvironmentData { header: vec4<u32>, texels: array<EnvironmentTexel> };
@group(0) @binding(4) var<storage, read> environment_data: EnvironmentData;
const PI: f32 = 3.141592653589793;
const FAR: f32 = 1e30;

// Header: triangle count, vertex offset, material offset, planar-patch offset (all words).
// Indexed triangles contain three vertex indices and one resolved material ID.
fn vector_at(word: u32) -> vec4<f32> {
    return bitcast<vec4<f32>>(vec4<u32>(geometry[word],geometry[word+1u],geometry[word+2u],geometry[word+3u]));
}
fn triangle(index: u32) -> Triangle {
    let word = 4u+index*4u;
    let a = geometry[1]+geometry[word]*8u;
    let b = geometry[1]+geometry[word+1u]*8u;
    let c = geometry[1]+geometry[word+2u]*8u;
    let material = geometry[2]+geometry[word+3u]*16u;
    return Triangle(vector_at(a),vector_at(b),vector_at(c),vector_at(a+4u),vector_at(b+4u),vector_at(c+4u),vector_at(material),vector_at(material+4u),vector_at(material+8u),vector_at(material+12u));
}

// Exact s/p power reflectance and transmitted cosine; -1 denotes TIR.
fn dielectric(c: f32, ni: f32, nt: f32) -> vec3<f32> {
    if ni == nt { return vec3<f32>(0.0,0.0,c); }
    let eta = ni/nt;
    let s2 = eta*eta*max(0.0,1.0-c*c);
    if s2 >= 1.0 { return vec3<f32>(1.0,1.0,-1.0); }
    let ct = sqrt(1.0-s2);
    let rs = (ni*c-nt*ct)/(ni*c+nt*ct);
    let rp = (nt*c-ni*ct)/(nt*c+ni*ct);
    return vec3<f32>(rs*rs,rp*rp,ct);
}
struct Sheet { reflection: vec3<f32>, transmission: vec3<f32> };
fn sheet_polarization(r: f32, a: vec3<f32>) -> Sheet {
    let denominator = vec3<f32>(1.0)-r*r*a*a;
    // The grazing lossless limit is perfectly reflecting.
    if r >= 1.0 { return Sheet(vec3<f32>(1.0),vec3<f32>(0.0)); }
    return Sheet(vec3<f32>(r)+(1.0-r)*(1.0-r)*r*a*a/denominator,(1.0-r)*(1.0-r)*a/denominator);
}
fn sheet(t: Triangle, f: vec3<f32>) -> Sheet {
    if f.z <= 0.0 { return Sheet(vec3<f32>(1.0),vec3<f32>(0.0)); }
    let attenuation = exp(-t.optics.xyz*(t.parameters.w/f.z));
    let s = sheet_polarization(f.x,attenuation);
    let p = sheet_polarization(f.y,attenuation);
    return Sheet((s.reflection+p.reflection)*0.5,(s.transmission+p.transmission)*0.5);
}
struct Medium { id: f32, ior: f32, sigma: vec3<f32> };

fn random(state: ptr<function, u32>) -> f32 {
    *state = *state * 747796405u + 2891336453u;
    let word = ((*state >> ((*state >> 28u) + 4u)) ^ *state) * 277803737u;
    return f32(((word >> 22u) ^ word) >> 8u) * (1.0 / 16777216.0);
}

fn unit(v: vec3<f32>) -> vec3<f32> {
    let scale = max(max(abs(v.x), abs(v.y)), abs(v.z));
    if scale == 0.0 { return vec3<f32>(0.0, 0.0, 1.0); }
    return normalize(v / scale);
}
fn fresnel(f0: vec3<f32>, c: f32) -> vec3<f32> {
    let t = 1.0 - clamp(c, 0.0, 1.0);
    return f0 + (vec3<f32>(1.0) - f0) * (t*t*t*t*t);
}
fn distribution(nh: f32, a2: f32) -> f32 {
    let d = (1.0 - nh*nh) + nh*nh*a2;
    return a2 / (PI*d*d);
}
fn brdf(t: Triangle, n: vec3<f32>, v: vec3<f32>, l: vec3<f32>) -> vec3<f32> {
    let nv = clamp(dot(n,v), -1.0, 1.0);
    let nl = clamp(dot(n,l), -1.0, 1.0);
    if nv <= 0.0 || nl <= 0.0 { return vec3<f32>(0.0); }
    let h = unit(v+l);
    let a = t.parameters.y*t.parameters.y;
    let a2 = a*a;
    let d = distribution(clamp(dot(n,h), 0.0, 1.0), a2);
    let gv = 2.0 / (nv + sqrt(a2 + (1.0-a2)*nv*nv));
    let gl = 2.0 / (nl + sqrt(a2 + (1.0-a2)*nl*nl));
    let ratio = (t.parameters.z - 1.0) / (t.parameters.z + 1.0);
    let f0 = mix(vec3<f32>(ratio*ratio), t.base.xyz, t.parameters.x);
    let diffuse = t.base.xyz * ((1.0-t.parameters.x)/PI);
    let base = fresnel(f0, dot(v,h)) * (d*0.25*gv*gl)
        + (vec3<f32>(1.0)-fresnel(f0,nv)) * (vec3<f32>(1.0)-fresnel(f0,nl)) * diffuse;
    if t.coat.x <= 0.0 { return base; }
    let coat_a = t.coat.y*t.coat.y;
    let coat_a2 = coat_a*coat_a;
    let coat_d = distribution(clamp(dot(n,h),0.0,1.0),coat_a2);
    let coat_gv = 2.0/(nv+sqrt(coat_a2+(1.0-coat_a2)*nv*nv));
    let coat_gl = 2.0/(nl+sqrt(coat_a2+(1.0-coat_a2)*nl*nl));
    let coat_ratio = (t.coat.z-1.0)/(t.coat.z+1.0);
    let coat_f0 = vec3<f32>(coat_ratio*coat_ratio);
    let attenuation = (vec3<f32>(1.0)-t.coat.x*coat_energy_bound(coat_f0,nv))*(vec3<f32>(1.0)-t.coat.x*coat_energy_bound(coat_f0,nl));
    return base*attenuation+t.coat.x*fresnel(coat_f0,dot(v,h))*(coat_d*0.25*coat_gv*coat_gl);
}

// Explicit stored vertex anchor plus unadded local offset. Neither field is
// an error-free floating residual; do not combine them before relative tests.
// Shader backends may reassociate floating expressions, so the captured corner
// regressions remain required on each supported backend.
// Integer-defined IEEE-754 binary32 addition, round-to-nearest/ties-to-even.
// All alignment and rounding happen in u32, so shader floating reassociation
// cannot combine anchor subtraction with the subsequent local displacement.
// The bit interface also preserves subnormals for the arithmetic oracle; normal
// shader float consumers may still flush subnormal values under WGSL's rules.
fn fp_shift_jam(value: u32, amount: u32) -> u32 {
    if amount == 0u { return value; }
    if amount >= 32u { return select(0u,1u,value != 0u); }
    let lost = value << (32u-amount);
    return (value >> amount) | select(0u,1u,lost != 0u);
}
fn fp_add_bits(left: u32, right: u32) -> u32 {
    var a=left;
    var b=right;
    if (a&0x7fffffffu) < (b&0x7fffffffu) { a=right; b=left; }
    let sign=a&0x80000000u;
    let ea=(a>>23u)&255u;
    let eb=(b>>23u)&255u;
    let fa=a&0x7fffffu;
    let fb=b&0x7fffffu;
    if ea == 255u {
        if fa != 0u || (eb == 255u && fb != 0u) { return 0x7fc00000u; }
        if eb == 255u && ((a^b)&0x80000000u) != 0u { return 0x7fc00000u; }
        return a;
    }
    if (b&0x7fffffffu) == 0u {
        if (a&0x7fffffffu) == 0u { return (a&b)&0x80000000u; }
        return a;
    }
    var exponent=max(ea,1u);
    let other_exponent=max(eb,1u);
    let sa=(fa | select(0u,0x800000u,ea != 0u))<<3u;
    let sb=fp_shift_jam((fb | select(0u,0x800000u,eb != 0u))<<3u,exponent-other_exponent);
    var significand=0u;
    if ((a^b)&0x80000000u) == 0u {
        significand=sa+sb;
        if significand >= 0x8000000u {
            significand=fp_shift_jam(significand,1u);
            exponent+=1u;
        }
    } else {
        significand=sa-sb;
        if significand == 0u { return 0u; }
        // Nonzero significand is below bit 27. Shift to bit 26, or stop at
        // the subnormal exponent, exactly as the bounded normalization loop.
        let shift=min(countLeadingZeros(significand)-5u,exponent-1u);
        significand<<=shift;
        exponent-=shift;
    }
    let remainder=significand&7u;
    var rounded=significand>>3u;
    if remainder>4u || (remainder == 4u && (rounded&1u) != 0u) { rounded+=1u; }
    if rounded >= 0x1000000u { rounded>>=1u; exponent+=1u; }
    if exponent >= 255u { return sign|0x7f800000u; }
    if rounded < 0x800000u { exponent=0u; }
    return sign|(exponent<<23u)|(rounded&0x7fffffu);
}
fn fp_sub_bits(left: u32, right: u32) -> u32 {
    return fp_add_bits(left,right^0x80000000u);
}
// Exact identities, then a guarded Sterbenz subtraction. WGSL does not specify
// RTE for a general floating subtraction, so approximate differences must retain
// the integer implementation. The fast case has an exactly representable normal
// result, independent of rounding direction and permitted subnormal flushing.
fn fp_sub_barrier_bits(left: u32, right: u32) -> u32 {
    let a=left&0x7fffffffu;
    let b=right&0x7fffffffu;
    if a>=0x7f800000u || b>=0x7f800000u { return fp_sub_bits(left,right); }
    if a==0u {
        if b==0u { return left&(right^0x80000000u)&0x80000000u; }
        return right^0x80000000u;
    }
    if b==0u { return left; }
    if left==right { return 0u; }
    let small=min(a,b);
    let large=max(a,b);
    let small_exponent=small>>23u;
    let large_exponent=large>>23u;
    let same_sign=((left^right)&0x80000000u)==0u;
    let within_two=large_exponent==small_exponent ||
        (large_exponent==small_exponent+1u && (large&0x7fffffu)<=(small&0x7fffffu));
    // Encoded exponent 24 has spacing 2^-126. Any distinct pair admitted here
    // therefore has a normal difference, including the closest adjacent pair.
    if same_sign && small_exponent>=24u && within_two {
        return bitcast<u32>(bitcast<f32>(left)-bitcast<f32>(right));
    }
    return fp_sub_bits(left,right);
}
fn anchored_delta_bits(a: u32, a_low: u32, b: u32, b_low: u32) -> u32 {
    let offset=fp_sub_barrier_bits(a_low,b_low);
    // Identical finite anchors have +0 difference. Adding it preserves a
    // nonzero offset; either sign of a zero offset becomes +0.
    if a==b && (a&0x7fffffffu)<0x7f800000u {
        return select(offset,0u,(offset&0x7fffffffu)==0u);
    }
    let anchor=fp_sub_barrier_bits(a,b);
    if (offset&0x7fffffffu)==0u {
        // IEEE RTE addition of two zeros retains -0 only if both are -0.
        return select(anchor,anchor&offset,(anchor&0x7fffffffu)==0u);
    }
    // This nonlinear integer operation is the reassociation barrier. Keep it
    // even when both component differences used exact hardware subtraction.
    return fp_add_bits(anchor,offset);
}
fn anchored_delta_component(a: f32, a_low: f32, b: f32, b_low: f32) -> f32 {
    return bitcast<f32>(anchored_delta_bits(bitcast<u32>(a),bitcast<u32>(a_low),bitcast<u32>(b),bitcast<u32>(b_low)));
}

struct PrecisePoint { hi: vec3<f32>, lo: vec3<f32> };
fn precise_point_delta(a: vec3<f32>, a_low: vec3<f32>, b: vec3<f32>, b_low: vec3<f32>) -> vec3<f32> {
    if all(a_low==vec3<f32>(0.0)) && all(b_low==vec3<f32>(0.0)) { return a-b; }
    return vec3<f32>(anchored_delta_component(a.x,a_low.x,b.x,b_low.x),anchored_delta_component(a.y,a_low.y,b.y,b_low.y),anchored_delta_component(a.z,a_low.z,b.z,b_low.z));
}
fn ray_segment_precise(o: vec3<f32>, o_low: vec3<f32>, previous: vec3<f32>, previous_low: vec3<f32>, d: vec3<f32>, distance: f32) -> vec3<f32> {
    // Subtract the physical origins before adding a possibly tiny ray segment.
    return precise_point_delta(o,o_low,previous,previous_low)+d*distance;
}

// Explicit parallel slab handling avoids 0*infinity/NaN at box boundaries.
fn box_hit(node: Node, o: vec3<f32>, d: vec3<f32>, limit: f32) -> bool {
    return box_hit_padded(node,o,d,limit,u.lighting.x);
}
fn box_hit_padded(node: Node, o: vec3<f32>, d: vec3<f32>, limit: f32, pad: f32) -> bool {
    var near = 0.0;
    var far = limit;
    for (var k = 0u; k < 3u; k += 1u) {
        if abs(d[k]) < 1e-20 {
            if o[k] < node.lo[k]-pad || o[k] > node.hi[k]+pad { return false; }
        } else {
            let a = (node.lo[k]-pad-o[k])/d[k];
            let b = (node.hi[k]+pad-o[k])/d[k];
            near = max(near, min(a,b)); far = min(far, max(a,b));
        }
    }
    return far >= near;
}
// A strictly positive ray cannot reach a triangle when any coordinate starts
// on/outside its exact stored bound and moves away. These comparisons introduce
// no epsilon or rounded division; padded BVH bounds remain only broad phase.
fn triangle_ahead(t: Triangle, o: vec3<f32>, d: vec3<f32>) -> bool {
    let lo=min(t.p0.xyz,min(t.p1.xyz,t.p2.xyz));
    let hi=max(t.p0.xyz,max(t.p1.xyz,t.p2.xyz));
    if any((o<=lo)&(d<vec3<f32>(0.0))) || any((o>=hi)&(d>vec3<f32>(0.0))) { return false; }
    if any((d==vec3<f32>(0.0))&((o<lo)|(o>hi))) { return false; }
    return true;
}
fn triangle_ahead_precise(t: Triangle, o: vec3<f32>, o_low: vec3<f32>, d: vec3<f32>) -> bool {
    if all(o_low==vec3<f32>(0.0)) { return triangle_ahead(t,o,d); }
    let lo=precise_point_delta(min(t.p0.xyz,min(t.p1.xyz,t.p2.xyz)),vec3<f32>(0.0),o,o_low);
    let hi=precise_point_delta(max(t.p0.xyz,max(t.p1.xyz,t.p2.xyz)),vec3<f32>(0.0),o,o_low);
    if any((lo>=vec3<f32>(0.0))&(d<vec3<f32>(0.0))) || any((hi<=vec3<f32>(0.0))&(d>vec3<f32>(0.0))) { return false; }
    if any((d==vec3<f32>(0.0))&((lo>vec3<f32>(0.0))|(hi<vec3<f32>(0.0)))) { return false; }
    return true;
}
fn difference_products(a: f32, b: f32, c: f32, d: f32) -> f32 {
    let cd = c*d;
    let error = fma(-c,d,cd);
    return fma(a,b,-cd)+error;
}
struct Hit { distance: f32, triangle: u32, bary: vec3<f32> };
fn trace_precise(o: vec3<f32>, o_low: vec3<f32>, d: vec3<f32>, any_hit: bool, excluded: u32, minimum: f32) -> Hit {
    // Dispatch once before traversal. Ordinary rays keep the original lean BVH
    // loop, without precision-helper branches inside every candidate/node test.
    if all(o_low==vec3<f32>(0.0)) { return trace_bounded(o,d,any_hit,excluded,minimum); }
    var hit = Hit(FAR, 0xffffffffu, vec3<f32>(0.0));
    // Project every candidate into the same ray-aligned 2D coordinate system.
    // Shared edges then use identical products with opposite signs, avoiding the
    // independently rounded barycentric comparisons of Moller-Trumbore.
    var kz = 0u;
    if abs(d.y)>abs(d.x) { kz=1u; }
    if abs(d.z)>abs(d[kz]) { kz=2u; }
    let kx=(kz+1u)%3u; let ky=(kx+1u)%3u;
    let shear=vec3<f32>(d[kx]/d[kz],d[ky]/d[kz],1.0/d[kz]);
    // Only candidates near the origin need the integer-defined deltas. The
    // rounded origin is within a few ULPs of the true anchor plus offset, so
    // boxes grown by that bound reject conservatively: every point the true
    // ray reaches at depth t lies in the grown box along the rounded ray at
    // the same t. A triangle whose bounds are farther from the rounded origin
    // than the near radius cannot change hit sign or order under that error,
    // and uses ordinary arithmetic.
    let rounded=o+o_low;
    let scale=max(max(abs(rounded.x),max(abs(rounded.y),abs(rounded.z))),1.0);
    let pad=u.lighting.x+scale*(1.0/1048576.0);
    let near_radius=scale*(1.0/256.0);
    // A balanced tree over <= 2^20 triangles needs <= 21 pending nodes.
    var stack: array<u32, 32>;
    stack[0] = 0u;
    var size = 1u;
    loop {
        if size == 0u { break; }
        size -= 1u;
        let node = nodes[stack[size]];
        if node.first == 0xffffffffu || !box_hit_padded(node, rounded, d, hit.distance, pad) { continue; }
        if node.count == 0u {
            if size + 2u > 32u { break; }
            stack[size] = node.first; stack[size+1u] = node.first+1u; size += 2u;
            continue;
        }
        for (var i = node.first; i < node.first + node.count; i += 1u) {
            if i == excluded { continue; }
            // A straight continuation cannot revisit its producing exact plane.
            // IDs join only connected, exactly coplanar uploaded f32 triangles
            // of one body; curved facets and other bodies remain candidates.
            if excluded!=0xffffffffu && geometry[geometry[3]+i]==geometry[geometry[3]+excluded] { continue; }
            let t = triangle(i);
            let lo=min(t.p0.xyz,min(t.p1.xyz,t.p2.xyz));
            let hi=max(t.p0.xyz,max(t.p1.xyz,t.p2.xyz));
            let gap=max(lo-rounded,rounded-hi);
            if any(gap>vec3<f32>(near_radius)) {
                if !triangle_ahead(t,rounded,d) { continue; }
                let qa=t.p0.xyz-rounded; let qb=t.p1.xyz-rounded; let qc=t.p2.xyz-rounded;
                let fa=vec2<f32>(qa[kx]-shear.x*qa[kz],qa[ky]-shear.y*qa[kz]);
                let fb=vec2<f32>(qb[kx]-shear.x*qb[kz],qb[ky]-shear.y*qb[kz]);
                let fc=vec2<f32>(qc[kx]-shear.x*qc[kz],qc[ky]-shear.y*qc[kz]);
                let far_weights=vec3<f32>(difference_products(fb.x,fc.y,fb.y,fc.x),difference_products(fc.x,fa.y,fc.y,fa.x),difference_products(fa.x,fb.y,fa.y,fb.x));
                if any(far_weights<vec3<f32>(0.0)) && any(far_weights>vec3<f32>(0.0)) { continue; }
                let far_det=far_weights.x+far_weights.y+far_weights.z;
                if far_det==0.0 { continue; }
                let far_bary=far_weights/far_det;
                let far_distance=surface_ray_depth(t,far_bary,rounded,kz)*shear.z;
                if far_distance > minimum && far_distance < hit.distance {
                    hit = Hit(far_distance,i,far_bary);
                    if any_hit { return hit; }
                }
                continue;
            }
            if !triangle_ahead_precise(t,o,o_low,d) { continue; }
            let pa=precise_point_delta(t.p0.xyz,vec3<f32>(0.0),o,o_low);
            let pb=precise_point_delta(t.p1.xyz,vec3<f32>(0.0),o,o_low);
            let pc=precise_point_delta(t.p2.xyz,vec3<f32>(0.0),o,o_low);
            let a=vec2<f32>(pa[kx]-shear.x*pa[kz],pa[ky]-shear.y*pa[kz]);
            let b=vec2<f32>(pb[kx]-shear.x*pb[kz],pb[ky]-shear.y*pb[kz]);
            let c=vec2<f32>(pc[kx]-shear.x*pc[kz],pc[ky]-shear.y*pc[kz]);
            let weights=vec3<f32>(difference_products(b.x,c.y,b.y,c.x),difference_products(c.x,a.y,c.y,a.x),difference_products(a.x,b.y,a.y,b.x));
            if any(weights<vec3<f32>(0.0)) && any(weights>vec3<f32>(0.0)) { continue; }
            let det=weights.x+weights.y+weights.z;
            if det==0.0 { continue; }
            // Retain all three directly evaluated weights: 1-u-v can erase a
            // small corner clearance when the other two weights sum to one.
            let bary=weights/det;
            let distance=surface_ray_depth_precise(t,bary,o,o_low,kz)*shear.z;
            if distance > minimum && distance < hit.distance {
                hit = Hit(distance,i,bary);
                if any_hit { return hit; }
            }
        }
    }
    return hit;
}

fn trace_bounded(o: vec3<f32>, d: vec3<f32>, any_hit: bool, excluded: u32, minimum: f32) -> Hit {
    var hit = Hit(FAR, 0xffffffffu, vec3<f32>(0.0));
    // Project every candidate into the same ray-aligned 2D coordinate system.
    // Shared edges then use identical products with opposite signs, avoiding the
    // independently rounded barycentric comparisons of Moller-Trumbore.
    var kz = 0u;
    if abs(d.y)>abs(d.x) { kz=1u; }
    if abs(d.z)>abs(d[kz]) { kz=2u; }
    let kx=(kz+1u)%3u; let ky=(kx+1u)%3u;
    let shear=vec3<f32>(d[kx]/d[kz],d[ky]/d[kz],1.0/d[kz]);
    // A balanced tree over <= 2^20 triangles needs <= 21 pending nodes.
    var stack: array<u32, 32>;
    stack[0] = 0u;
    var size = 1u;
    loop {
        if size == 0u { break; }
        size -= 1u;
        let node = nodes[stack[size]];
        if node.first == 0xffffffffu || !box_hit(node, o, d, hit.distance) { continue; }
        if node.count == 0u {
            if size + 2u > 32u { break; }
            stack[size] = node.first; stack[size+1u] = node.first+1u; size += 2u;
            continue;
        }
        for (var i = node.first; i < node.first + node.count; i += 1u) {
            if i == excluded { continue; }
            // A straight continuation cannot revisit its producing exact plane.
            // IDs join only connected, exactly coplanar uploaded f32 triangles
            // of one body; curved facets and other bodies remain candidates.
            if excluded!=0xffffffffu && geometry[geometry[3]+i]==geometry[geometry[3]+excluded] { continue; }
            let t = triangle(i);
            if !triangle_ahead(t,o,d) { continue; }
            let pa=t.p0.xyz-o; let pb=t.p1.xyz-o; let pc=t.p2.xyz-o;
            let a=vec2<f32>(pa[kx]-shear.x*pa[kz],pa[ky]-shear.y*pa[kz]);
            let b=vec2<f32>(pb[kx]-shear.x*pb[kz],pb[ky]-shear.y*pb[kz]);
            let c=vec2<f32>(pc[kx]-shear.x*pc[kz],pc[ky]-shear.y*pc[kz]);
            let weights=vec3<f32>(difference_products(b.x,c.y,b.y,c.x),difference_products(c.x,a.y,c.y,a.x),difference_products(a.x,b.y,a.y,b.x));
            if any(weights<vec3<f32>(0.0)) && any(weights>vec3<f32>(0.0)) { continue; }
            let det=weights.x+weights.y+weights.z;
            if det==0.0 { continue; }
            // Retain all three directly evaluated weights: 1-u-v can erase a
            // small corner clearance when the other two weights sum to one.
            let bary=weights/det;
            let distance=surface_ray_depth(t,bary,o,kz)*shear.z;
            if distance > minimum && distance < hit.distance {
                hit = Hit(distance,i,bary);
                if any_hit { return hit; }
            }
        }
    }
    return hit;
}
fn trace(o: vec3<f32>, d: vec3<f32>, any_hit: bool) -> Hit {
    return trace_bounded(o,d,any_hit,0xffffffffu,u.lighting.x);
}
fn dominant_vertex(bary: vec3<f32>) -> u32 {
    var anchor=0u;
    if bary.y>bary.x { anchor=1u; }
    if bary.z>bary[anchor] { anchor=2u; }
    return anchor;
}
fn surface_point(t: Triangle, bary: vec3<f32>) -> vec3<f32> {
    // Anchor near the hit, retaining the two other directly computed weights.
    // A tiny weight on a distant vertex can encode an entire corner clearance.
    // Do not recover that weight by subtracting the other two from one.
    let p=array<vec3<f32>,3>(t.p0.xyz,t.p1.xyz,t.p2.xyz);
    let a=dominant_vertex(bary); let b=(a+1u)%3u; let c=(a+2u)%3u;
    return p[a]+((p[b]-p[a])*bary[b]+(p[c]-p[a])*bary[c]);
}
fn surface_point_precise(t: Triangle, bary: vec3<f32>) -> PrecisePoint {
    let p=array<vec3<f32>,3>(t.p0.xyz,t.p1.xyz,t.p2.xyz);
    let a=dominant_vertex(bary); let b=(a+1u)%3u; let c=(a+2u)%3u;
    // Keep the world anchor separate from its local displacement.
    return PrecisePoint(p[a],(p[b]-p[a])*bary[b]+(p[c]-p[a])*bary[c]);
}
fn surface_ray_depth_precise(t: Triangle, bary: vec3<f32>, origin: vec3<f32>, origin_low: vec3<f32>, axis: u32) -> f32 {
    if all(origin_low==vec3<f32>(0.0)) { return surface_ray_depth(t,bary,origin,axis); }
    let point=surface_point_precise(t,bary);
    return anchored_delta_component(point.hi[axis],point.lo[axis],origin[axis],origin_low[axis]);
}
fn surface_ray_depth(t: Triangle, bary: vec3<f32>, origin: vec3<f32>, axis: u32) -> f32 {
    let p=array<vec3<f32>,3>(t.p0.xyz,t.p1.xyz,t.p2.xyz);
    let a=dominant_vertex(bary); let b=(a+1u)%3u; let c=(a+2u)%3u;
    // Original vertex differences avoid subtracting two rounded ray-relative
    // coordinates; adding the anchor last preserves distant thin-hit ordering.
    return (p[a][axis]-origin[axis])+((p[b][axis]-p[a][axis])*bary[b]+(p[c][axis]-p[a][axis])*bary[c]);
}

fn from_local(p: vec3<f32>, n: vec3<f32>) -> vec3<f32> {
    let axis = select(vec3<f32>(0.0,0.0,1.0),vec3<f32>(0.0,1.0,0.0),abs(n.z)>0.9);
    let x = unit(cross(axis,n));
    return unit(x*p.x + cross(n,x)*p.y + n*p.z);
}
fn sample_surface(t: Triangle, n: vec3<f32>, v: vec3<f32>, rng: ptr<function,u32>) -> vec3<f32> {
    let component = random(rng); let x = random(rng); let y = random(rng);
    return sample_surface_values(t,n,v,vec3<f32>(component,x,y));
}
// Conditional opaque mixture density; branch selection is accounted for by callers.
fn surface_pdf(t: Triangle, n: vec3<f32>, v: vec3<f32>, l: vec3<f32>) -> f32 {
    let nl = dot(n,l);
    if nl<=0.0 || dot(n,v)<=0.0 { return 0.0; }
    let h = unit(v+l); let vh = max(dot(v,h),1e-20);
    let nh = max(dot(n,h),0.0);
    let alpha = t.parameters.y*t.parameters.y;
    let base = 0.5*nl/PI+0.5*distribution(nh,alpha*alpha)*nh/(4.0*vh);
    if t.coat.x<=0.0 { return base; }
    let coat_alpha = t.coat.y*t.coat.y;
    let coat_pdf = distribution(nh,coat_alpha*coat_alpha)*nh/(4.0*vh);
    return (1.0-t.coat.x*0.5)*base+t.coat.x*0.5*coat_pdf;
}
fn power_weight(a: f32, b: f32) -> f32 {
    // Ratio form avoids overflow for grazing/very distant rectangular samples.
    if a <= 0.0 { return 0.0; }
    let ratio = b/a;
    return 1.0/(1.0+ratio*ratio);
}
struct EmitterHit { distance: f32, index: u32 };
fn emitter_hit_precise(o: vec3<f32>, o_low: vec3<f32>, d: vec3<f32>, minimum: f32) -> EmitterHit {
    var hit = EmitterHit(FAR,0xffffffffu);
    for (var i=0u; i<u32(u.light_count.w); i+=1u) {
        let light = u.area_lights[i];
        let normal = unit(cross(light.u.xyz,light.v.xyz));
        let cosine = dot(normal,d);
        if abs(cosine)<1e-8 { continue; }
        let to_light=precise_point_delta(light.center.xyz,vec3<f32>(0.0),o,o_low);
        let distance = dot(to_light,normal)/cosine;
        if distance <= minimum || distance >= hit.distance { continue; }
        var q = o+d*distance-light.center.xyz;
        if any(o_low!=vec3<f32>(0.0)) { q=fma(d,vec3<f32>(distance),-to_light); }
        if abs(dot(q,light.u.xyz))<=dot(light.u.xyz,light.u.xyz) && abs(dot(q,light.v.xyz))<=dot(light.v.xyz,light.v.xyz) {
            hit = EmitterHit(distance,i);
        }
    }
    return hit;
}
fn emitter_hit_bounded(o: vec3<f32>, d: vec3<f32>, minimum: f32) -> EmitterHit {
    return emitter_hit_precise(o,vec3<f32>(0.0),d,minimum);
}
fn emitter_hit(o: vec3<f32>, d: vec3<f32>) -> EmitterHit {
    return emitter_hit_bounded(o,d,u.lighting.x);
}

fn area_pdf(light: AreaLight, delta: vec3<f32>) -> f32 {
    let cosine = dot(unit(cross(light.u.xyz,light.v.xyz)),-unit(delta));
    if cosine<=0.0 { return 0.0; }
    return dot(delta,delta)/(light.center.w*cosine);
}
fn visible(o: vec3<f32>, d: vec3<f32>, distance: f32) -> ShadowResult {
    if emitter_hit(o,d).distance<distance-u.lighting.x*2.0 { return blocked_shadow(); }
    return shadow_transmittance(o,d,distance);
}
struct EnvironmentSample { direction: vec3<f32>, radiance: vec3<f32>, pdf: f32 };
fn environment_to_world(d: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(u.environment_map.x*d.x-u.environment_map.y*d.y,u.environment_map.y*d.x+u.environment_map.x*d.y,d.z);
}
fn environment_index(d: vec3<f32>) -> u32 {
    let local=vec3<f32>(u.environment_map.x*d.x+u.environment_map.y*d.y,-u.environment_map.y*d.x+u.environment_map.x*d.y,d.z);
    var phi=atan2(local.y,local.x); if phi<0.0 {phi+=2.0*PI;}
    let dims=environment_data.header.xy;
    let x=min(u32(phi/(2.0*PI)*f32(dims.x)),dims.x-1u);
    let y=min(u32(acos(clamp(local.z,-1.0,1.0))/PI*f32(dims.y)),dims.y-1u);
    return y*dims.x+x;
}
fn env_eval(d: vec3<f32>) -> vec3<f32> {
    if u.environment_map.w>0.0 {return environment_data.texels[environment_index(d)].rgb*u.environment_map.z;}
    return u.environment.xyz;
}
fn env_pdf(d: vec3<f32>) -> f32 {
    if u.environment_map.w>0.0 {return environment_data.texels[environment_index(d)].pdf;}
    return 1.0/(4.0*PI);
}
fn env_sample(rng: ptr<function,u32>) -> EnvironmentSample {
    if u.environment_map.w>0.0 {
        let dims=environment_data.header.xy;
        let choice=random(rng)*f32(environment_data.header.z);
        let initial=min(u32(choice),environment_data.header.z-1u);
        let entry=environment_data.texels[initial];
        let index=select(entry.alternate,initial,random(rng)<entry.threshold);
        let texel=environment_data.texels[index];
        let row=index/dims.x; let column=index%dims.x;
        let top=cos(PI*f32(row)/f32(dims.y)); let bottom=cos(PI*f32(row+1u)/f32(dims.y));
        let z=mix(top,bottom,random(rng));
        let phi=2.0*PI*(f32(column)+random(rng))/f32(dims.x);
        let radius=sqrt(max(0.0,1.0-z*z));
        let direction=environment_to_world(vec3<f32>(radius*cos(phi),radius*sin(phi),z));
        return EnvironmentSample(direction,texel.rgb*u.environment_map.z,texel.pdf);
    }
    let z=1.0-2.0*random(rng);let phi=2.0*PI*random(rng);
    let radius=sqrt(max(0.0,1.0-z*z));
    return EnvironmentSample(vec3<f32>(radius*cos(phi),radius*sin(phi),z),u.environment.xyz,1.0/(4.0*PI));
}
fn sun_pdf(d:vec3<f32>)->f32 {
    return select(0.0,u.sun_radiance.w,dot(d,u.sun_direction.xyz)>=u.sun_direction.w);
}
fn sun_eval(d:vec3<f32>)->vec3<f32> {
    return select(vec3<f32>(0.0),u.sun_radiance.xyz,sun_pdf(d)>0.0);
}
fn sun_sample(rng:ptr<function,u32>)->EnvironmentSample {
    // Derive 1-cos(radius) from the CPU's stable solid angle, not f32 subtraction.
    let z=1.0-random(rng)/(2.0*PI*u.sun_radiance.w);
    let phi=2.0*PI*random(rng);let radius=sqrt(max(0.0,1.0-z*z));
    let direction=from_local(vec3<f32>(radius*cos(phi),radius*sin(phi),z),u.sun_direction.xyz);
    return EnvironmentSample(direction,u.sun_radiance.xyz,u.sun_radiance.w);
}
fn direct_studio(t: Triangle, position: vec3<f32>, o: vec3<f32>, n: vec3<f32>, geometric: vec3<f32>, v: vec3<f32>, remaining: u32, rng: ptr<function,u32>) -> vec3<f32> {
    var result = vec3<f32>(0.0);
    let opaque = 1.0-t.base.w;
    if opaque<=0.0 { return result; }
    for (var i=0u; i<u32(u.light_count.w); i+=1u) {
        let light = u.area_lights[i];
        let point = light.center.xyz + light.u.xyz*(2.0*random(rng)-1.0) + light.v.xyz*(2.0*random(rng)-1.0);
        let delta = point-position;
        let direction = unit(delta);
        let pdf = area_pdf(light,delta);
        let nl = dot(n,direction);
        if pdf<=0.0 || nl<=0.0 || dot(geometric,direction)<=0.0 { continue; }
        let shadow = point-o;
        let visibility=visible(o,unit(shadow),length(shadow));
        let competing=opaque*surface_pdf(t,n,v,direction)*visibility.branch_pdf;
        let weight=select(power_weight(pdf,competing),1.0,visibility.solid || visibility.crossings>=remaining);
        result += visibility.transmittance*brdf(t,n,v,direction)*light.radiance.xyz*(opaque*nl*weight/pdf);
    }
    if u.environment_map.w>0.0 || any(u.environment.xyz>vec3<f32>(0.0)) {
        let sample=env_sample(rng);
        let nl=dot(n,sample.direction);
        if sample.pdf>0.0 && nl>0.0 && dot(geometric,sample.direction)>0.0 {
            let visibility=visible(o,sample.direction,FAR);
            let competing=opaque*surface_pdf(t,n,v,sample.direction)*visibility.branch_pdf;
            let weight=select(power_weight(sample.pdf,competing),1.0,visibility.solid || visibility.crossings>=remaining);
            result+=visibility.transmittance*brdf(t,n,v,sample.direction)*sample.radiance*(opaque*nl*weight/sample.pdf);
        }
    }
    if u.sun_radiance.w>0.0 {
        let sample=sun_sample(rng);
        let nl=dot(n,sample.direction);
        if nl>0.0 && dot(geometric,sample.direction)>0.0 {
            let visibility=visible(o,sample.direction,FAR);
            let competing=opaque*surface_pdf(t,n,v,sample.direction)*visibility.branch_pdf;
            let weight=select(power_weight(sample.pdf,competing),1.0,visibility.solid || visibility.crossings>=remaining);
            result+=visibility.transmittance*brdf(t,n,v,sample.direction)*sample.radiance*(opaque*nl*weight/sample.pdf);
        }
    }
    return result;
}
struct CameraRay { origin: vec3<f32>, direction: vec3<f32> };
// The lens lies in the eye plane. A uniform-area disk (r=sqrt(u), phi=2pi*v)
// aims at the pinhole ray's intersection with the axial focus plane.
fn primary_ray(uv: vec2<f32>, lens_sample: vec2<f32>) -> CameraRay {
    let offset = u.right.xyz*uv.x + u.up.xyz*uv.y;
    var o = u.eye.xyz + offset;
    var d = -u.back.xyz;
    if u.eye.w > 0.5 { o = u.eye.xyz; d = unit(offset-u.back.xyz*u.back.w); }
    if u.eye.w > 0.5 && u.right.w > 0.0 {
        let radius = sqrt(lens_sample.x)*u.right.w;
        let phi = 2.0*PI*lens_sample.y;
        let displacement = radius*(cos(phi)*unit(u.right.xyz)+sin(phi)*unit(u.up.xyz));
        // Construct the focal point directly, avoiding division by a tiny ray cosine.
        let focal = offset*(u.up.w/u.back.w)-u.back.xyz*u.up.w;
        o += displacement;
        d = unit(focal-displacement);
    }
    return CameraRay(o,d);
}
fn sample_path(pixel: u32) -> vec4<f32> {
    var rng = pixel ^ (u.image.z * 0x9e3779b9u) ^ u.sampling.x;
    let xy = vec2<f32>(f32(pixel % u.image.x)+random(&rng), f32(pixel / u.image.x)+random(&rng));
    let uv = vec2<f32>(xy.x/f32(u.image.x)*2.0-1.0, 1.0-xy.y/f32(u.image.y)*2.0);
    var lens_sample = vec2<f32>(0.0);
    // Zero aperture consumes no variates: preserve the pinhole sample stream exactly.
    if u.eye.w > 0.5 && u.right.w > 0.0 { lens_sample = vec2<f32>(random(&rng),random(&rng)); }
    let primary = primary_ray(uv,lens_sample);
    var o = primary.origin;
    var o_low = vec3<f32>(0.0);
    var d = primary.direction;
    var previous_position = o;
    var previous_position_low = vec3<f32>(0.0);
    var ray_excluded = 0xffffffffu;
    var ray_minimum = u.lighting.x;
    var previous_scatter_position = o;
    var previous_scatter_position_low = vec3<f32>(0.0);
    // NEE-only opt-in solid approximation replaces direct transmitted emitter
    // paths from an opaque bounce. Camera and reflected paths stay physical.
    var suppress_direct = false;
    var throughput = vec3<f32>(1.0);
    var radiance = vec3<f32>(0.0);
    var coverage = 0.0;
    var previous_pdf = 0.0;
    var previous_delta = true;
    var media: array<Medium,16>;
    var medium_count = 0u;
    let lights = array<vec4<f32>,3>(vec4<f32>(0.6,0.0,0.8,3.0),vec4<f32>(-0.8,0.0,0.6,1.0),vec4<f32>(0.0,-0.6,0.8,1.5));
    for (var bounce = 0u; bounce < u.sampling.y; bounce += 1u) {
        let hit = trace_precise(o,o_low,d,false,ray_excluded,ray_minimum);
        if u.environment.w > 0.0 {
            let emitter = emitter_hit_precise(o,o_low,d,ray_minimum);
            if emitter.distance < hit.distance {
                let light = u.area_lights[emitter.index];
                let segment = ray_segment_precise(o,o_low,previous_position,previous_position_low,d,emitter.distance);
                if medium_count>0u {
                    throughput *= exp(-media[medium_count-1u].sigma*length(segment));
                }
                let scatter_segment = ray_segment_precise(o,o_low,previous_scatter_position,previous_scatter_position_low,d,emitter.distance);
                let pdf = area_pdf(light,scatter_segment);
                if pdf>0.0 && !suppress_direct {
                    let weight = select(power_weight(previous_pdf,pdf),1.0,previous_delta);
                    radiance += throughput*light.radiance.xyz*weight;
                }
                coverage = 1.0;
                break;
            }
        }
        if hit.triangle == 0xffffffffu {
            // Camera-inside and intersecting/open volumes are deliberately unsupported.
            if medium_count > 0u { return vec4<f32>(f32(bounce),0.0,0.0,-1.0); }
            if u.environment.w > 0.0 {
                // The backplate remains request.background; environment lights covered paths.
                if bounce>0u && !suppress_direct {
                    let weight = select(power_weight(previous_pdf,env_pdf(d)),1.0,previous_delta);
                    radiance += throughput*env_eval(d)*weight;
                    let sun_weight=select(power_weight(previous_pdf,sun_pdf(d)),1.0,previous_delta);
                    radiance += throughput*sun_eval(d)*sun_weight;
                }
            } else if u.sampling.w == 0u || d.z < 0.0 { radiance += throughput * u.lighting.y; }
            break;
        }
        coverage = 1.0;
        ray_excluded = 0xffffffffu; ray_minimum = u.lighting.x;
        var t = triangle(hit.triangle); let v = -d;
        var geometric = unit(cross(t.p1.xyz-t.p0.xyz,t.p2.xyz-t.p0.xyz));
        let entering = dot(geometric,v)>=0.0;
        if !entering { geometric = -geometric; }
        var ni = 1.0;
        if medium_count > 0u {
            let current = media[medium_count-1u];
            ni = current.ior;
        }
        var n = unit(t.n0.xyz*hit.bary.x+t.n1.xyz*hit.bary.y+t.n2.xyz*hit.bary.z);
        if dot(n,geometric)<0.0 { n = -n; }
        if dot(n,v)<=0.0 { n = geometric; }
        // Recover the surface point from barycentrics. A distant ray's f32 distance
        // loses precision on skinny triangles; o+d*t can land behind the surface
        // and make the next shadow ray intersect the same face.
        var position = surface_point(t,hit.bary);
        var position_low = vec3<f32>(0.0);
        if t.base.w>0.0 && t.parameters.w<0.0 {
            let point=surface_point_precise(t,hit.bary);
            position=point.hi; position_low=point.lo;
        }
        let shading_position = position+position_low;
        t = texture_surface(t,shading_position);
        let original_normal = n;
        n = texture_normal(t,shading_position,n,geometric);
        if dot(n,v)<=0.0 { n=original_normal; }
        if medium_count > 0u {
            let segment=precise_point_delta(position,position_low,previous_position,previous_position_low);
            throughput *= exp(-media[medium_count-1u].sigma*length(segment));
        }
        previous_position = position; previous_position_low=position_low;
        o = position + geometric*(u.lighting.x*2.0); o_low=vec3<f32>(0.0);
        if u.environment.w > 0.0 {
            radiance += throughput*direct_studio(t,position,o,n,geometric,v,u.sampling.y-bounce-1u,&rng);
        }
        for (var i=0u; i<3u && u.environment.w==0.0; i+=1u) {
            let light = lights[i];
            if u.lighting.z > 0.0 && dot(geometric,light.xyz)>0.0 && dot(n,light.xyz)>0.0 {
                let visibility=shadow_transmittance(o,light.xyz,FAR);
                radiance += visibility.transmittance * throughput * brdf(t,n,v,light.xyz) * ((1.0-t.base.w)*light.w*dot(n,light.xyz)*u.lighting.z);
            }
        }
        if t.base.w > 0.0 && random(&rng) < t.base.w {
            var nt = t.parameters.z;
            if t.parameters.w < 0.0 && entering && medium_count > 0u {
                return vec4<f32>(f32(bounce),0.0,0.0,-4.0);
            }
            if t.parameters.w < 0.0 && !entering {
                if medium_count == 0u { return vec4<f32>(f32(bounce),0.0,0.0,-2.0); }
                if media[medium_count-1u].id != t.optics.w { return vec4<f32>(f32(bounce),0.0,0.0,-3.0); }
                nt = 1.0;
                if medium_count > 1u { nt = media[medium_count-2u].ior; }
            }
            var f = dielectric(clamp(dot(n,v),0.0,1.0),ni,nt);
            var transmitted = refract(d,n,ni/nt);
            // A bent shading normal must not send a transmitted ray into the wrong
            // geometric hemisphere. Fall back to the geometric interface there.
            if (f.z >= 0.0 && dot(transmitted,geometric) >= 0.0) || dot(reflect(d,n),geometric) <= 0.0 {
                n = geometric;
                f = dielectric(clamp(dot(n,v),0.0,1.0),ni,nt);
                transmitted = refract(d,n,ni/nt);
            }
            if t.parameters.w >= 0.0 {
                let weights = sheet(t,f);
                let mean_r = dot(weights.reflection,vec3<f32>(1.0/3.0));
                let mean_t = dot(weights.transmission,vec3<f32>(1.0/3.0));
                if mean_r+mean_t <= 0.0 { break; }
                let probability = mean_r/(mean_r+mean_t);
                if random(&rng) < probability {
                    previous_delta=true; previous_scatter_position=position; previous_scatter_position_low=position_low; suppress_direct=false;
                    throughput *= weights.reflection/probability;
                    d = reflect(d,n);
                } else {
                    throughput *= weights.transmission/(1.0-probability);
                    if t.coat.w>0.0 { previous_pdf*=1.0-probability; }
                    else { previous_delta=true; previous_scatter_position=position; previous_scatter_position_low=position_low; suppress_direct=false; }
                    // Keep a straight connection through sheets: geometric-normal
                    // offsets would change emitter solid-angle PDFs along this chain.
                    o = position+d*(u.lighting.x*2.0);
                    // Parallel thin sheet: unchanged direction, no optical displacement.
                }
            } else if random(&rng) < (f.x+f.y)*0.5 {
                previous_delta=true; previous_scatter_position=position; previous_scatter_position_low=position_low; suppress_direct=false;
                d = reflect(d,n);
            } else {
                if t.coat.w>0.0 { suppress_direct=suppress_direct || !previous_delta; }
                else { suppress_direct=false; }
                previous_delta=true; previous_scatter_position=position; previous_scatter_position_low=position_low;
                if entering {
                    if medium_count >= 16u { return vec4<f32>(f32(bounce),0.0,0.0,-4.0); }
                    media[medium_count] = Medium(t.optics.w,nt,t.optics.xyz);
                    medium_count += 1u;
                } else { medium_count -= 1u; }
                throughput *= (ni/nt)*(ni/nt);
                d = unit(transmitted);
                o = position-geometric*(u.lighting.x*2.0);
            }
            if dot(d,geometric) > 0.0 { o = position+geometric*(u.lighting.x*2.0); }
            if t.parameters.w<0.0 {
                // A finite normal offset can cross the next face near a closed
                // corner. Launch on the reconstructed boundary, suppress only
                // the producing triangle, and retain every strictly positive exit.
                // Medium-order errors are still rejected; no neighboring faces
                // are skipped by angular or distance heuristics.
                o=position; o_low=position_low; ray_excluded=hit.triangle; ray_minimum=0.0;
            }
            continue;
        }
        d = sample_surface(t,n,v,&rng);
        let nl = dot(n,d);
        if nl<=0.0 || dot(geometric,d)<=0.0 { break; }
        let pdf = surface_pdf(t,n,v,d);
        previous_pdf = (1.0-t.base.w)*pdf;
        previous_scatter_position=position; previous_scatter_position_low=position_low;
        previous_delta = false;
        suppress_direct=false;
        throughput *= brdf(t,n,v,d) * (nl/pdf);
        if bounce>=3u {
            let survive = clamp(max(max(throughput.x,throughput.y),throughput.z),0.05,0.95);
            if random(&rng)>survive { break; }
            throughput /= survive;
        }
    }
    return vec4<f32>(radiance,coverage);
}

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let pixel = u.image.w + gid.x;
    if gid.x >= u.sampling.z || pixel >= u.image.x*u.image.y { return; }
    if u.image.z > 0u && accumulation[pixel].w < 0.0 { return; }
    let sample = sample_path(pixel);
    if sample.w < 0.0 { accumulation[pixel] = vec4<f32>(sample.x,f32(u.image.z),f32(pixel),sample.w); return; }
    if u.image.z == 0u { accumulation[pixel] = sample; }
    else { accumulation[pixel] += sample; }
}

// Validation entry point: exact same BRDF used by production paths, read back in tests.
@compute @workgroup_size(1)
fn reference() {
    accumulation[0] = vec4<f32>(brdf(triangle(0u),u.up.xyz,u.back.xyz,u.right.xyz),1.0);
}

@compute @workgroup_size(1)
fn dielectric_reference() {
    let f = dielectric(u.up.x,u.up.y,u.up.z);
    accumulation[0] = vec4<f32>(f.xy,max(f.z,0.0),1.0);
}

@compute @workgroup_size(1)
fn camera_reference() {
    let ray = primary_ray(u.lighting.xy,u.lighting.zw);
    // Bias signed test coordinates for the radiance readback's nonnegative contract.
    accumulation[0] = vec4<f32>(ray.origin+vec3<f32>(16.0),1.0);
    accumulation[1] = vec4<f32>(ray.direction+vec3<f32>(2.0),1.0);
}

// Deterministic image-space ball-lens check. Uses the production BVH/intersection
// and interpolated normal data, with Snell transport through both interfaces.
@compute @workgroup_size(1)
fn lens_reference() {
    let direction = u.back.xyz;
    let entry = trace(u.eye.xyz,direction,false);
    if entry.triangle == 0xffffffffu { accumulation[0] = vec4<f32>(0.0); return; }
    let a = triangle(entry.triangle);
    let pa = surface_point(a,entry.bary);
    let na = unit(a.n0.xyz*entry.bary.x+a.n1.xyz*entry.bary.y+a.n2.xyz*entry.bary.z);
    let inside = refract(direction,na,1.0/a.parameters.z);
    let exit = trace(pa-na*u.lighting.x*2.0,inside,false);
    if exit.triangle == 0xffffffffu { accumulation[0] = vec4<f32>(0.0); return; }
    let b = triangle(exit.triangle);
    let pb = surface_point(b,exit.bary);
    let nb = unit(b.n0.xyz*exit.bary.x+b.n1.xyz*exit.bary.y+b.n2.xyz*exit.bary.z);
    let outgoing = refract(inside,-nb,b.parameters.z);
    let spot = pb+outgoing*((-u.back.w-pb.z)/outgoing.z);
    accumulation[0] = vec4<f32>(spot+vec3<f32>(1.0,1.0,3.0),1.0);
}

// GPU/CPU orientation and matched-density check uses the production lookup helpers.
@compute @workgroup_size(1)
fn environment_reference() {
    accumulation[0]=vec4<f32>(env_eval(u.right.xyz),env_pdf(u.right.xyz));
}

fn coat_energy_bound(f0: vec3<f32>, cosine: f32) -> vec3<f32> {
    let minimum_vh = cosine/sqrt(2.0*(1.0+sqrt(max(0.0,1.0-cosine*cosine))));
    return fresnel(f0,minimum_vh);
}

fn sample_surface_values(t: Triangle, n: vec3<f32>, v: vec3<f32>, variates: vec3<f32>) -> vec3<f32> {
    let component = variates.x;
    let x = variates.y; let phi = 2.0*PI*variates.z;
    let coat_probability = t.coat.x*0.5;
    if component < (1.0-coat_probability)*0.5 {
        let radius = sqrt(x);
        return from_local(vec3<f32>(radius*cos(phi),radius*sin(phi),sqrt(1.0-x)),n);
    }
    let roughness = select(t.parameters.y,t.coat.y,component >= 1.0-coat_probability);
    let alpha = roughness*roughness;
    let cosine = sqrt((1.0-x)/(1.0+(alpha*alpha-1.0)*x));
    let sine = sqrt(max(0.0,1.0-cosine*cosine));
    let h = from_local(vec3<f32>(sine*cos(phi),sine*sin(phi),cosine),n);
    return reflect(-v,h);
}

@compute @workgroup_size(1)
fn surface_sample_reference() {
    let d = sample_surface_values(triangle(0u),u.up.xyz,u.back.xyz,u.lighting.xyz);
    accumulation[0] = vec4<f32>(d+vec3<f32>(2.0),1.0);
}

@compute @workgroup_size(1)
fn pdf_reference() {
    accumulation[0] = vec4<f32>(surface_pdf(triangle(0u),u.up.xyz,u.back.xyz,u.right.xyz),0.0,0.0,1.0);
}

// Diagnostic entry point for independently specified closed-corner fixtures.
@compute @workgroup_size(1)
fn boundary_reference() {
    let entry=trace(u.eye.xyz,u.back.xyz,false);
    if entry.triangle==0xffffffffu {accumulation[0]=vec4<f32>(9.0,9.0,9.0,1.0);return;}
    let t=triangle(entry.triangle);
    let position=surface_point(t,entry.bary);
    let outward=unit(cross(t.p1.xyz-t.p0.xyz,t.p2.xyz-t.p0.xyz));
    let shifted=position-outward*(2.0*u.lighting.x);
    let legacy=trace(shifted,u.right.xyz,false);
    let low_min=trace_bounded(shifted,u.right.xyz,false,entry.triangle,0.0);
    let exact=trace_bounded(position,u.right.xyz,false,entry.triangle,0.0);
    accumulation[0]=vec4<f32>(select(0.0,1.0,legacy.triangle!=0xffffffffu),select(0.0,1.0,low_min.triangle!=0xffffffffu),select(0.0,1.0,exact.triangle!=0xffffffffu),1.0);
    accumulation[1]=vec4<f32>(exact.distance,0.0,0.0,1.0);
    accumulation[2]=vec4<f32>(abs(position),1.0);
    accumulation[3]=vec4<f32>(f32(entry.triangle),entry.bary.yz,1.0);
}

@compute @workgroup_size(1)
fn positive_triangle_reference() {
    let hit=trace_bounded(u.eye.xyz,u.right.xyz,false,0xffffffffu,0.0);
    accumulation[0]=vec4<f32>(select(0.0,1.0,hit.triangle!=0xffffffffu),0.0,0.0,1.0);
}

// Producing hit barycentrics are prescribed independently to exercise f32 point
// reconstruction on a translated oblique split plane, then a grazing reflection.
@compute @workgroup_size(1)
fn coplanar_boundary_reference() {
    let position=surface_point(triangle(0u),vec3<f32>(1.0-u.eye.x-u.eye.y,u.eye.xy));
    let hit=trace_bounded(position,u.right.xyz,false,0u,0.0);
    accumulation[0]=vec4<f32>(select(0.0,1.0,hit.triangle!=0xffffffffu),hit.distance,0.0,1.0);
    accumulation[1]=vec4<f32>(position,1.0);
}

fn ordering_candidate(t: Triangle, o: vec3<f32>, d: vec3<f32>) -> vec2<f32> {
    var kz=0u;
    if abs(d.y)>abs(d.x) {kz=1u;}
    if abs(d.z)>abs(d[kz]) {kz=2u;}
    let kx=(kz+1u)%3u; let ky=(kx+1u)%3u;
    let shear=vec3<f32>(d[kx]/d[kz],d[ky]/d[kz],1.0/d[kz]);
    let pa=t.p0.xyz-o; let pb=t.p1.xyz-o; let pc=t.p2.xyz-o;
    let a=vec2<f32>(pa[kx]-shear.x*pa[kz],pa[ky]-shear.y*pa[kz]);
    let b=vec2<f32>(pb[kx]-shear.x*pb[kz],pb[ky]-shear.y*pb[kz]);
    let c=vec2<f32>(pc[kx]-shear.x*pc[kz],pc[ky]-shear.y*pc[kz]);
    let weights=vec3<f32>(difference_products(b.x,c.y,b.y,c.x),difference_products(c.x,a.y,c.y,a.x),difference_products(a.x,b.y,a.y,b.x));
    let det=weights.x+weights.y+weights.z;
    let bary=weights/det;
    let distance=surface_ray_depth(t,bary,o,kz)*shear.z;
    let valid=!(any(weights<vec3<f32>(0.0))&&any(weights>vec3<f32>(0.0))) && det!=0.0;
    return vec2<f32>(distance,select(0.0,1.0,valid));
}
@compute @workgroup_size(1)
fn ordering_reference() {
    let hit=trace(u.eye.xyz,u.right.xyz,false);
    let a=ordering_candidate(triangle(0u),u.eye.xyz,u.right.xyz);
    let b=ordering_candidate(triangle(1u),u.eye.xyz,u.right.xyz);
    accumulation[0]=vec4<f32>(f32(hit.triangle),hit.distance,0.0,1.0);
    accumulation[1]=vec4<f32>(a,0.0,1.0);
    accumulation[2]=vec4<f32>(b,0.0,1.0);
}

@compute @workgroup_size(1)
fn refracted_corner_reference() {
    let entry=trace(u.eye.xyz,u.right.xyz,false);
    if entry.triangle==0xffffffffu {accumulation[0]=vec4<f32>(0.0,0.0,0.0,1.0);return;}
    let t=triangle(entry.triangle);
    let position=surface_point(t,entry.bary);
    let outward=unit(cross(t.p1.xyz-t.p0.xyz,t.p2.xyz-t.p0.xyz));
    let inside=refract(u.right.xyz,outward,1.0/t.parameters.z);
    let exit=trace_bounded(position,inside,false,entry.triangle,0.0);
    var valid_exit=false;
    if exit.triangle!=0xffffffffu {
        let other=triangle(exit.triangle);
        let normal=unit(cross(other.p1.xyz-other.p0.xyz,other.p2.xyz-other.p0.xyz));
        valid_exit=other.optics.w==t.optics.w && dot(normal,inside)>0.0;
    }
    accumulation[0]=vec4<f32>(select(0.0,1.0,dot(outward,u.right.xyz)<0.0),select(0.0,1.0,exit.triangle!=0xffffffffu),select(0.0,1.0,valid_exit),1.0);
    accumulation[1]=vec4<f32>(exit.distance,0.0,0.0,1.0);
    accumulation[2]=vec4<f32>(abs(position),1.0);
    accumulation[3]=vec4<f32>(inside+vec3<f32>(2.0),1.0);
}

@compute @workgroup_size(1)
fn rib_residual_reference() {
    var source=0xffffffffu;
    for(var i=0u;i<geometry[0];i+=1u) {
        let t=triangle(i);
        if all(t.p0.xyz==u.eye.xyz)&&all(t.p1.xyz==u.right.xyz)&&all(t.p2.xyz==u.up.xyz) {source=i;break;}
    }
    if source==0xffffffffu {accumulation[0]=vec4<f32>(9.0,9.0,9.0,1.0);return;}
    let t=triangle(source);
    let point=surface_point_precise(t,u.environment.xyz);
    let outward=unit(cross(t.p1.xyz-t.p0.xyz,t.p2.xyz-t.p0.xyz));
    let direction=reflect(u.back.xyz,outward);
    let f=dielectric(abs(dot(outward,u.back.xyz)),t.parameters.z,1.0);
    let dropped=trace_bounded(point.hi+point.lo,direction,false,source,0.0);
    let retained=trace_precise(point.hi,point.lo,direction,false,source,0.0);
    var valid_exit=false;
    if retained.triangle!=0xffffffffu {
        let other=triangle(retained.triangle);
        let n=unit(cross(other.p1.xyz-other.p0.xyz,other.p2.xyz-other.p0.xyz));
        valid_exit=other.optics.w==t.optics.w&&dot(n,direction)>0.0;
    }
    accumulation[0]=vec4<f32>(select(0.0,1.0,dropped.triangle!=0xffffffffu),select(0.0,1.0,retained.triangle!=0xffffffffu),select(0.0,1.0,valid_exit),1.0);
    accumulation[1]=vec4<f32>(retained.distance,select(0.0,1.0,f.z<0.0),0.0,1.0);
    accumulation[2]=vec4<f32>(abs(point.hi),1.0);
    accumulation[3]=vec4<f32>(abs(point.lo),1.0);
}

// Results are encoded as exact positive 16-bit integers in floats so the normal
// radiance readback can transport every binary32 bit pattern, including NaNs.
@compute @workgroup_size(64)
fn integer_float_reference(@builtin(global_invocation_id) invocation: vec3<u32>) {
    let i=invocation.x;
    if i>=geometry[0] { return; }
    let a=geometry[1u+i*2u];
    let b=geometry[2u+i*2u];
    let sum=fp_add_bits(a,b);
    let difference=fp_sub_barrier_bits(a,b);
    accumulation[i]=vec4<f32>(f32(sum&65535u),f32(sum>>16u),f32(difference&65535u),f32(difference>>16u));
}

@compute @workgroup_size(1)
fn emitter_segment_reference() {
    // A local offset large compared with this real ray segment. Accumulating
    // the endpoint into the offset first needlessly rounds away part of d*t.
    let anchor=vec3<f32>(-163.7,-55.0,-55.0);
    let offset=vec3<f32>(0.000007887696,0.000009279658,-10.753371);
    let direction=vec3<f32>(0.6,0.0,0.8);
    let distance=0.00001;
    let segment=ray_segment_precise(anchor,offset,anchor,offset,direction,distance);
    let light=AreaLight(vec4<f32>(0.0,0.0,0.0,16.0),vec4<f32>(0.0,2.0,0.0,0.0),vec4<f32>(2.0,0.0,0.0,0.0),vec4<f32>(1.0));
    accumulation[0]=vec4<f32>(length(segment),exp(-5000.0*length(segment)),area_pdf(light,segment),1.0);
}

@compute @workgroup_size(64)
fn integer_relative_reference(@builtin(global_invocation_id) invocation: vec3<u32>) {
    let i=invocation.x;
    if i>=geometry[0] { return; }
    let a=geometry[1u+i*4u];
    let al=geometry[2u+i*4u];
    let b=geometry[3u+i*4u];
    let bl=geometry[4u+i*4u];
    let reference=fp_add_bits(fp_sub_bits(a,b),fp_sub_bits(al,bl));
    let candidate=anchored_delta_bits(a,al,b,bl);
    accumulation[i]=vec4<f32>(f32(reference&65535u),f32(reference>>16u),f32(candidate&65535u),f32(candidate>>16u));
}
