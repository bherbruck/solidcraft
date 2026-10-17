// Bounded straight connection, not refracted caustic transport. Host preparation
// certifies disjoint closed media; malformed crossing order is still blocked.
struct ShadowResult { transmittance: vec3<f32>, branch_pdf: f32, crossings: u32, solid: bool };
fn blocked_shadow() -> ShadowResult {
    return ShadowResult(vec3<f32>(0.0),0.0,0u,false);
}
fn shadow_transmittance(origin: vec3<f32>, direction: vec3<f32>, limit: f32) -> ShadowResult {
    var result = ShadowResult(vec3<f32>(1.0),1.0,0u,false);
    var o = origin;
    var o_low = vec3<f32>(0.0);
    var ray_excluded = 0xffffffffu;
    var ray_minimum = u.lighting.x;
    var medium_id = 0.0;
    var medium_ior = 1.0;
    var sigma = vec3<f32>(0.0);
    var entry = origin;
    var entry_low = vec3<f32>(0.0);
    // Up to 64 interfaces. If traversal cannot establish visibility, block it.
    for (var step=0u; step<=64u; step+=1u) {
        let hit = trace_precise(o,o_low,direction,false,ray_excluded,ray_minimum);
        let traversed = dot(precise_point_delta(o,o_low,origin,vec3<f32>(0.0)),direction);
        let remaining = limit-traversed;
        if hit.triangle == 0xffffffffu {
            if medium_id!=0.0 { return blocked_shadow(); }
            return result;
        }
        if hit.distance>=remaining {
            // A finite emitter may be inside a certified medium. Attenuate only
            // to that emitter, never to the body's farther exit surface.
            if medium_id!=0.0 { result.transmittance*=exp(-sigma*length(ray_segment_precise(o,o_low,entry,entry_low,direction,remaining)))/(medium_ior*medium_ior); }
            return result;
        }
        if step==64u { return blocked_shadow(); }
        let t = triangle(hit.triangle);
        if t.base.w<=0.0 || t.coat.w<=0.0 { return blocked_shadow(); }
        var position=surface_point(t,hit.bary);
        var position_low=vec3<f32>(0.0);
        if t.parameters.w<0.0 {
            let point=surface_point_precise(t,hit.bary);
            position=point.hi; position_low=point.lo;
        }
        let outward=unit(cross(t.p1.xyz-t.p0.xyz,t.p2.xyz-t.p0.xyz));
        let entering=dot(outward,direction)<0.0;
        let geometric=select(-outward,outward,entering);
        var n=unit(t.n0.xyz*hit.bary.x+t.n1.xyz*hit.bary.y+t.n2.xyz*hit.bary.z);
        if dot(n,geometric)<0.0 { n=-n; }
        if dot(n,-direction)<=0.0 { n=geometric; }
        if t.parameters.w>=0.0 {
            if medium_id!=0.0 { return blocked_shadow(); }
            // Match the primary path's shading-normal fallback before the sheet sum.
            var f=dielectric(clamp(dot(n,-direction),0.0,1.0),1.0,t.parameters.z);
            let refracted=refract(direction,n,1.0/t.parameters.z);
            if (f.z>=0.0 && dot(refracted,geometric)>=0.0) || dot(reflect(direction,n),geometric)<=0.0 {
                n=geometric;
                f=dielectric(clamp(dot(n,-direction),0.0,1.0),1.0,t.parameters.z);
            }
            let weights=sheet(t,f);
            let r=dot(weights.reflection,vec3<f32>(1.0/3.0));
            let tr=dot(weights.transmission,vec3<f32>(1.0/3.0));
            if r+tr<=0.0 || tr<=0.0 { return blocked_shadow(); }
            result.transmittance*=weights.transmission;
            result.branch_pdf*=tr/(r+tr);
        } else {
            result.solid=true;
            // Reciprocal external-angle Fresnel at each surface. Applying an
            // inside critical angle to this unrefracted connection would invent TIR.
            let f=dielectric(clamp(abs(dot(outward,direction)),0.0,1.0),1.0,t.parameters.z);
            result.transmittance*=1.0-(f.x+f.y)*0.5;
            if entering {
                if medium_id!=0.0 { return blocked_shadow(); }
                medium_id=t.optics.w; medium_ior=t.parameters.z; entry=position; entry_low=position_low; sigma=t.optics.xyz;
            } else {
                if medium_id==0.0 || medium_id!=t.optics.w { return blocked_shadow(); }
                result.transmittance*=exp(-sigma*length(precise_point_delta(position,position_low,entry,entry_low)));
                medium_id=0.0;
            }
        }
        result.crossings+=1u;
        if max(max(result.transmittance.x,result.transmittance.y),result.transmittance.z)<=0.0 { return blocked_shadow(); }
        // Stay on the same line so the finite emitter endpoint remains meaningful.
        // Absorption uses unshifted entry/exit endpoints, not this numerical offset.
        o=position+direction*(u.lighting.x*2.0); o_low=vec3<f32>(0.0);
        ray_excluded=0xffffffffu; ray_minimum=u.lighting.x;
        if t.parameters.w<0.0 {
            // Preserve arbitrarily short real corner segments, using the same
            // producing-plane exclusion as closed primary-path continuation.
            o=position; o_low=position_low; ray_excluded=hit.triangle; ray_minimum=0.0;
        }
    }
    return blocked_shadow();
}

@compute @workgroup_size(1)
fn shadow_reference() {
    let shadow=shadow_transmittance(u.eye.xyz,u.back.xyz,u.back.w);
    accumulation[0]=vec4<f32>(shadow.transmittance,1.0);
    accumulation[1]=vec4<f32>(shadow.branch_pdf,f32(shadow.crossings),select(0.0,1.0,shadow.solid),1.0);
}
