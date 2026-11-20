// Water surface optics in linear light. Distances are in metres.
fn water_ripple(p:vec3<f32>,normal:vec3<f32>,time:f32,amplitude:f32)->vec3<f32> {
    // Integer harmonics of a 128 m repeat let the CPU rebase the phase
    // without jumping when the native snapshot changes its world origin.
    let wave=0.049087385;
    let warp=sin(dot(p.xz,vec2(6.,-11.)*wave)+time*0.34)*1.8;
    let a=dot(p.xz,vec2(79.,35.)*wave)+time*1.8+warp;
    let b=dot(p.xz,vec2(-43.,100.)*wave)-time*1.45-warp*0.8;
    let c=dot(p.xz,vec2(147.,-88.)*wave)+time*2.1+warp*1.2;
    let slope=vec2(sin(a)+0.50*sin(b)+0.19*sin(c),
        0.48*sin(a)-sin(b)-0.21*sin(c));
    return normalize(normal+vec3(slope.x,0.,slope.y)*amplitude*abs(normal.y));
}

fn water_fresnel(cosine:f32)->f32 {
    let edge=1.-clamp(cosine,0.,1.);
    let square=edge*edge;
    return 0.0204+0.9796*square*square*edge;
}

fn water_transmittance(distance:f32)->vec3<f32> {
    return exp(-vec3(0.88,0.43,0.39)*distance);
}

fn water_medium(color:vec3<f32>,distance:f32)->vec3<f32> {
    return mix(vec3(0.013,0.063,0.056),color,water_transmittance(distance));
}

fn water_sky(ray:vec3<f32>,sun:vec3<f32>,overcast:f32)->vec3<f32> {
    let dusk=1.-smoothstep(0.05,0.5,sun.y);
    let horizon=mix(vec3(0.37,0.52,0.63),vec3(0.65,0.45,0.33),dusk*0.6);
    let zenith=vec3(0.09,0.27,0.50);
    let sky=mix(horizon,zenith,sqrt(clamp(ray.y,0.,1.)));
    return mix(sky,vec3(0.39,0.45,0.49),overcast*0.65);
}

fn water_glint(direction:vec3<f32>,normal:vec3<f32>,sun:vec3<f32>,overcast:f32)->vec3<f32> {
    let alignment=max(dot(reflect(direction,normal),sun),0.);
    let glint=pow(alignment,95.)*0.17+pow(alignment,420.)*0.37;
    return vec3(1.,0.82,0.57)*glint*(1.-0.9*overcast);
}
