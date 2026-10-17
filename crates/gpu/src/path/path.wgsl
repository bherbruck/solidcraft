// Original software BVH and unidirectional Monte Carlo integrator. All directions
// at a surface point away from it, matching render::surface's GGX/Smith oracle.
struct Triangle {
    p0: vec4<f32>, p1: vec4<f32>, p2: vec4<f32>,
    n0: vec4<f32>, n1: vec4<f32>, n2: vec4<f32>,
    base: vec4<f32>, parameters: vec4<f32>,
};
struct Node { lo: vec3<f32>, first: u32, hi: vec3<f32>, count: u32 };
struct Params {
    // width, height, sample index, pixel offset
    image: vec4<u32>,
    // seed, maximum depth, tile length, reserved
    sampling: vec4<u32>,
    eye: vec4<f32>, right: vec4<f32>, up: vec4<f32>, back: vec4<f32>,
    // ray epsilon, test environment radiance, studio scale, reserved
    lighting: vec4<f32>,
};
@group(0) @binding(0) var<uniform> u: Params;
@group(0) @binding(1) var<storage, read> geometry: array<u32>;
@group(0) @binding(2) var<storage, read> nodes: array<Node>;
@group(0) @binding(3) var<storage, read_write> accumulation: array<vec4<f32>>;
const PI: f32 = 3.141592653589793;
const FAR: f32 = 1e30;

// Header: triangle count, vertex offset (words), material offset (words), vertex count.
// Indexed triangles contain three vertex indices and one resolved material ID.
fn vector_at(word: u32) -> vec4<f32> {
    return bitcast<vec4<f32>>(vec4<u32>(geometry[word],geometry[word+1u],geometry[word+2u],geometry[word+3u]));
}
fn triangle(index: u32) -> Triangle {
    let word = 4u+index*4u;
    let a = geometry[1]+geometry[word]*8u;
    let b = geometry[1]+geometry[word+1u]*8u;
    let c = geometry[1]+geometry[word+2u]*8u;
    let material = geometry[2]+geometry[word+3u]*8u;
    return Triangle(vector_at(a),vector_at(b),vector_at(c),vector_at(a+4u),vector_at(b+4u),vector_at(c+4u),vector_at(material),vector_at(material+4u));
}

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
    return fresnel(f0, dot(v,h)) * (d*0.25*gv*gl)
        + (vec3<f32>(1.0)-fresnel(f0,nv)) * (vec3<f32>(1.0)-fresnel(f0,nl)) * diffuse;
}

// Explicit parallel slab handling avoids 0*infinity/NaN at box boundaries.
fn box_hit(node: Node, o: vec3<f32>, d: vec3<f32>, limit: f32) -> bool {
    var near = 0.0;
    var far = limit;
    for (var k = 0u; k < 3u; k += 1u) {
        if abs(d[k]) < 1e-20 {
            if o[k] < node.lo[k] || o[k] > node.hi[k] { return false; }
        } else {
            let a = (node.lo[k]-o[k])/d[k];
            let b = (node.hi[k]-o[k])/d[k];
            near = max(near, min(a,b)); far = min(far, max(a,b));
        }
    }
    return far >= near;
}
struct Hit { distance: f32, triangle: u32, uv: vec2<f32> };
fn trace(o: vec3<f32>, d: vec3<f32>, any_hit: bool) -> Hit {
    var hit = Hit(FAR, 0xffffffffu, vec2<f32>(0.0));
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
            let t = triangle(i);
            let e1 = t.p1.xyz-t.p0.xyz; let e2 = t.p2.xyz-t.p0.xyz;
            let h = cross(d,e2); let det = dot(e1,h);
            if abs(det) <= 1e-20 { continue; }
            let s = o-t.p0.xyz; let b1 = dot(s,h)/det;
            if b1 < 0.0 || b1 > 1.0 { continue; }
            let q = cross(s,e1); let b2 = dot(d,q)/det;
            if b2 < 0.0 || b1+b2 > 1.0 { continue; }
            let distance = dot(e2,q)/det;
            if distance > u.lighting.x && distance < hit.distance {
                hit = Hit(distance,i,vec2<f32>(b1,b2));
                if any_hit { return hit; }
            }
        }
    }
    return hit;
}

fn from_local(p: vec3<f32>, n: vec3<f32>) -> vec3<f32> {
    let axis = select(vec3<f32>(0.0,0.0,1.0),vec3<f32>(0.0,1.0,0.0),abs(n.z)>0.9);
    let x = unit(cross(axis,n));
    return unit(x*p.x + cross(n,x)*p.y + n*p.z);
}
fn continuation(t: Triangle, n: vec3<f32>, v: vec3<f32>, rng: ptr<function,u32>) -> vec3<f32> {
    let component = random(rng);
    let x = random(rng); let phi = 2.0*PI*random(rng);
    if component < 0.5 {
        let radius = sqrt(x);
        return from_local(vec3<f32>(radius*cos(phi),radius*sin(phi),sqrt(1.0-x)),n);
    }
    let alpha = t.parameters.y*t.parameters.y;
    let cosine = sqrt((1.0-x)/(1.0+(alpha*alpha-1.0)*x));
    let sine = sqrt(max(0.0,1.0-cosine*cosine));
    let h = from_local(vec3<f32>(sine*cos(phi),sine*sin(phi),cosine),n);
    return reflect(-v,h);
}
fn sample_path(pixel: u32) -> vec4<f32> {
    var rng = pixel ^ (u.image.z * 0x9e3779b9u) ^ u.sampling.x;
    let xy = vec2<f32>(f32(pixel % u.image.x)+random(&rng), f32(pixel / u.image.x)+random(&rng));
    let uv = vec2<f32>(xy.x/f32(u.image.x)*2.0-1.0, 1.0-xy.y/f32(u.image.y)*2.0);
    let offset = u.right.xyz*uv.x + u.up.xyz*uv.y;
    var o = u.eye.xyz + offset;
    var d = -u.back.xyz;
    if u.eye.w > 0.5 { o = u.eye.xyz; d = unit(offset-u.back.xyz*u.back.w); }
    var throughput = vec3<f32>(1.0);
    var radiance = vec3<f32>(0.0);
    var coverage = 0.0;
    let lights = array<vec4<f32>,3>(vec4<f32>(0.6,0.0,0.8,3.0),vec4<f32>(-0.8,0.0,0.6,1.0),vec4<f32>(0.0,-0.6,0.8,1.5));
    for (var bounce = 0u; bounce < u.sampling.y; bounce += 1u) {
        let hit = trace(o,d,false);
        if hit.triangle == 0xffffffffu {
            radiance += throughput * u.lighting.y;
            break;
        }
        coverage = 1.0;
        let t = triangle(hit.triangle); let v = -d;
        var geometric = unit(cross(t.p1.xyz-t.p0.xyz,t.p2.xyz-t.p0.xyz));
        if dot(geometric,v)<0.0 { geometric = -geometric; }
        var n = unit(t.n0.xyz*(1.0-hit.uv.x-hit.uv.y)+t.n1.xyz*hit.uv.x+t.n2.xyz*hit.uv.y);
        if dot(n,geometric)<0.0 { n = -n; }
        if dot(n,v)<=0.0 { n = geometric; }
        // Recover the surface point from barycentrics. A distant ray's f32 distance
        // loses precision on skinny triangles; o+d*t can land behind the surface
        // and make the next shadow ray intersect the same face.
        o = t.p0.xyz*(1.0-hit.uv.x-hit.uv.y) + t.p1.xyz*hit.uv.x + t.p2.xyz*hit.uv.y
            + geometric*(u.lighting.x*2.0);
        for (var i=0u; i<3u; i+=1u) {
            let light = lights[i];
            if u.lighting.z > 0.0 && dot(geometric,light.xyz)>0.0 && dot(n,light.xyz)>0.0 {
                if trace(o,light.xyz,true).triangle == 0xffffffffu {
                    radiance += throughput * brdf(t,n,v,light.xyz) * (light.w*dot(n,light.xyz)*u.lighting.z);
                }
            }
        }
        d = continuation(t,n,v,&rng);
        let nl = dot(n,d);
        if nl<=0.0 || dot(geometric,d)<=0.0 { break; }
        let h = unit(v+d); let vh = max(dot(v,h),1e-20);
        let alpha = t.parameters.y*t.parameters.y;
        let nh = max(dot(n,h),0.0);
        let pdf = 0.5*nl/PI + 0.5*distribution(nh,alpha*alpha)*nh/(4.0*vh);
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
    let sample = sample_path(pixel);
    if u.image.z == 0u { accumulation[pixel] = sample; }
    else { accumulation[pixel] += sample; }
}

// Validation entry point: exact same BRDF used by production paths, read back in tests.
@compute @workgroup_size(1)
fn reference() {
    accumulation[0] = vec4<f32>(brdf(triangle(0u),u.up.xyz,u.back.xyz,u.right.xyz),1.0);
}
