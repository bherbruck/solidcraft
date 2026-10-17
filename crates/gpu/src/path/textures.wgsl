@group(0) @binding(5) var<storage, read> texture_words: array<u32>;
fn texture_vector(word:u32)->vec3<f32> {
    return bitcast<vec3<f32>>(vec3<u32>(texture_words[word],texture_words[word+1u],texture_words[word+2u]));
}
fn texture_record(t:Triangle)->u32 {
    let body=u32(t.optics.w);
    if body>=texture_words[0] {return 0u;}
    return texture_words[4u+body];
}
fn texture_pixel(offset:u32,width:u32,x:u32,y:u32)->vec3<f32> {
    return texture_vector(offset+4u*(y*width+x));
}
fn texture_sample(offset:u32,size:vec2<u32>,uv:vec2<f32>)->vec3<f32> {
    let position=fract(uv)*vec2<f32>(size)-vec2<f32>(0.5);
    let lower=vec2<i32>(floor(position));
    let first=vec2<u32>((lower+vec2<i32>(size))%vec2<i32>(size));
    let second=(first+vec2<u32>(1u))%size;
    let fraction=fract(position);
    let a=texture_pixel(offset,size.x,first.x,first.y);
    let b=texture_pixel(offset,size.x,second.x,first.y);
    let c=texture_pixel(offset,size.x,first.x,second.y);
    let d=texture_pixel(offset,size.x,second.x,second.y);
    return mix(mix(a,b,fraction.x),mix(c,d,fraction.x),fraction.y);
}
fn texture_uv(record:u32,position:vec3<f32>)->vec2<f32> {
    let delta=position-texture_vector(record);
    return vec2<f32>(dot(delta,texture_vector(record+4u)),dot(delta,texture_vector(record+8u)));
}
fn texture_surface(t:Triangle,position:vec3<f32>)->Triangle {
    let record=texture_record(t);var result=t;
    if record==0u || texture_words[record+3u]==0u {return result;}
    let rgb=texture_sample(texture_words[record+3u],vec2<u32>(texture_words[record+7u],texture_words[record+11u]),texture_uv(record,position));
    // Texture modulates linear reflectance, never emission or scalar alpha.
    result.base=vec4<f32>(t.base.xyz*rgb,t.base.w);
    return result;
}
fn texture_normal(t:Triangle,position:vec3<f32>,n:vec3<f32>,geometric:vec3<f32>)->vec3<f32> {
    let record=texture_record(t);
    if record==0u || texture_words[record+12u]==0u {return n;}
    let rgb=texture_sample(texture_words[record+12u],vec2<u32>(texture_words[record+13u],texture_words[record+14u]),texture_uv(record,position));
    let u=unit(texture_vector(record+4u));
    let tangent=u-n*dot(n,u);
    if dot(tangent,tangent)<1e-12 {return n;}
    let tangent_unit=unit(tangent);
    let handedness=select(1.0,-1.0,dot(n,cross(texture_vector(record+4u),texture_vector(record+8u)))<0.0);
    let strength=bitcast<f32>(texture_words[record+15u]);
    let local=vec3<f32>((2.0*rgb.xy-vec2<f32>(1.0))*strength,2.0*rgb.z-1.0);
    if dot(local,local)<1e-12 || local.z<=0.0 {return n;}
    let mapped=unit(tangent_unit*local.x+cross(n,tangent_unit)*(handedness*local.y)+n*local.z);
    return select(mapped,n,dot(mapped,geometric)<=0.0);
}
@compute @workgroup_size(1)
fn texture_reference() {
    let t=triangle(0u);
    let mapped=texture_surface(t,u.eye.xyz);
    accumulation[0]=vec4<f32>(mapped.base.xyz,1.0);
    accumulation[1]=vec4<f32>(texture_normal(t,u.eye.xyz,u.up.xyz,u.back.xyz)+vec3<f32>(2.0),1.0);
}
