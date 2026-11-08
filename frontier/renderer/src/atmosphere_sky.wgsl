// Bounded, half-resolution cloud lighting. This texture contains premultiplied
// cloud scattering RGB and clear-sky transmittance A, never terrain colour.
struct AtmosphereUniform {
    inverse_view_projection:mat4x4<f32>,relative_eye:vec4<f32>,world_eye:vec4<f32>,
    sun:vec4<f32>,fog:vec4<f32>,weather:vec4<f32>,beauty:vec4<f32>,
};
@group(0) @binding(0) var<uniform> atmosphere:AtmosphereUniform;
struct VertexOutput {@builtin(position) position:vec4<f32>,@location(0) uv:vec2<f32>};
@vertex fn vs_main(@builtin(vertex_index) i:u32)->VertexOutput {
    var p=array<vec2<f32>,3>(vec2(-1.,-1.),vec2(3.,-1.),vec2(-1.,3.));
    var o:VertexOutput;o.position=vec4(p[i],0.,1.);o.uv=vec2(p[i].x*.5+.5,.5-p[i].y*.5);return o;
}
fn hash3(p:vec3<f32>)->f32 {
    var q=fract(p*vec3(.1031,.1030,.0973));
    q+=dot(q,q.yxz+vec3(33.33));return fract((q.x+q.y)*q.z);
}
fn noise3(p:vec3<f32>)->f32 {
    let i=floor(p);let f=fract(p);let u=f*f*f*(f*(f*6.-15.)+10.);
    let a=mix(mix(hash3(i),hash3(i+vec3(1.,0.,0.)),u.x),mix(hash3(i+vec3(0.,1.,0.)),hash3(i+vec3(1.,1.,0.)),u.x),u.y);
    let b=mix(mix(hash3(i+vec3(0.,0.,1.)),hash3(i+vec3(1.,0.,1.)),u.x),mix(hash3(i+vec3(0.,1.,1.)),hash3(i+vec3(1.,1.,1.)),u.x),u.y);
    return mix(a,b,u.z);
}
fn density(p:vec3<f32>)->f32 {
    let height=(p.y-1720.)/680.;
    let wind=vec3(1.,0.,.32)*atmosphere.weather.w*atmosphere.weather.y;
    let q=p*vec3(.0030,.0038,.0030)+wind;
    // Genuine 3D density removes the six isolated projected noise sheets.
    // A broad vertical dome cuts rounded tops; a shallow base fades smoothly.
    let broad=noise3(q);
    let detail=noise3(q*2.07+vec3(17.,3.,11.));
    let shape=broad*.69+detail*.31;
    let dome=pow(abs((height-.42)/.58),2.);
    let threshold=mix(.79,.30,atmosphere.weather.x);
    let envelope=smoothstep(0.,.10,height)*(1.-smoothstep(.82,1.,height));
    return smoothstep(threshold,threshold+.16,shape-dome*.19)*envelope;
}
@fragment fn fs_main(i:VertexOutput)->@location(0) vec4<f32> {
    let p=atmosphere.inverse_view_projection*vec4(i.uv.x*2.-1.,1.-i.uv.y*2.,1.,1.);
    let ray=normalize(p.xyz/p.w-atmosphere.relative_eye.xyz);
    if ray.y<=.035 || atmosphere.weather.x<=.001 || atmosphere.world_eye.y>=2400. {return vec4(0.,0.,0.,1.);}
    let start=max((1720.-atmosphere.world_eye.y)/ray.y,0.);
    let end=min((2400.-atmosphere.world_eye.y)/ray.y,start+12000.);
    let step=(end-start)/20.;var transmittance=1.;var light=vec3(0.);
    let warm=1.-smoothstep(.04,.45,atmosphere.sun.y);
    let sun_color=mix(vec3(1.12,1.08,1.00),vec3(1.15,.85,.58),warm*.7);
    // World-direction anchored substep offset, continuous across the image and
    // stable while stationary. It breaks coherent slab integration boundaries
    // without temporal random noise or screen-space grain.
    let phase=.5+.16*sin(dot(ray,vec3(371.,173.,257.)));
    for(var k=0u;k<20u;k+=1u) {
        let position=atmosphere.world_eye.xyz+ray*(start+(f32(k)+phase)*step);
        let rho=density(position);
        if rho>.001 {
            let sun_density=density(position+atmosphere.sun.xyz*110.)+density(position+atmosphere.sun.xyz*280.);
            let direct=exp(-sun_density*.90)*(1.-atmosphere.weather.z*.65);
            let h=clamp((position.y-1720.)/680.,0.,1.);
            let ambient=mix(vec3(.60,.66,.75),vec3(.84,.89,.96),h);
            // Approximate the low-order multiple-scattering fill that keeps a
            // sunlit cumulus body pale rather than a dark pillow with a rim.
            let source=ambient*.72+sun_color*direct*.68;
            let alpha=1.-exp(-rho*step*.0045);
            light+=transmittance*alpha*source;transmittance*=1.-alpha;
            if transmittance<.015 {break;}
        }
    }
    let horizon=smoothstep(.035,.13,ray.y);
    return vec4(light*horizon,mix(1.,transmittance,horizon));
}
