// Bounded, quarter-resolution cloud lighting. This texture contains premultiplied
// cloud scattering RGB and clear-sky transmittance A, never terrain colour.
struct AtmosphereUniform {
    inverse_view_projection:mat4x4<f32>,relative_eye:vec4<f32>,world_eye:vec4<f32>,
    sun:vec4<f32>,fog:vec4<f32>,weather:vec4<f32>,beauty:vec4<f32>,
    lighting: vec4<f32>, // shafts, streaming haze, region count, reserved
    far_shadow: mat4x4<f32>,
    haze_regions: array<vec4<f32>,8>,
    plane_shadow: array<vec4<f32>,4>,
    cloud_map: vec4<f32>,
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
// Wind-aligned weather cells organize kilometre-scale cloud banks. A
// separate 3D field cuts lobes and wisps into the margins of each bank.
fn weather_noise(p:vec2<f32>)->f32 {
    let i=floor(p); let f=fract(p); let u=f*f*(3.-2.*f);
    return mix(mix(hash3(vec3(i,7.)),hash3(vec3(i+vec2(1.,0.),7.)),u.x),
               mix(hash3(vec3(i+vec2(0.,1.),7.)),hash3(vec3(i+vec2(1.,1.),7.)),u.x),u.y);
}
fn bank_coverage(p:vec2<f32>)->f32 {
    let wind_axis=vec2(.94,.342);
    let uv=vec2(dot(p,wind_axis),dot(p,vec2(-wind_axis.y,wind_axis.x)));
    let regional=weather_noise(uv*vec2(.00013,.00023));
    let bank=weather_noise(uv*vec2(.00042,.00070)+vec2(7.4,19.1));
    return regional*.56+bank*.44;
}
fn cloud_body(p:vec3<f32>, bank:f32)->f32 {
    let h=(p.y-1720.)/680.;
    let cover=atmosphere.weather.x;
    let lo=.10+.06*(1.-bank);
    let hi=.32+.68*smoothstep(.24,.79,bank+(.5-cover)*-.35);
    let vertical=smoothstep(lo-.09,lo+.07,h)*(1.-smoothstep(hi-.28,hi+.03,h));
    if vertical<=0. {return 0.;}
    let shape=noise3(p*vec3(.00115,.00200,.00132));
    let erosion=noise3(p*.0046+vec3(6.1,14.4,-9.2));
    let field=bank*.48+shape*.38+erosion*.14;
    return smoothstep(mix(.77,.27,cover),mix(.77,.27,cover)+.14,field)*vertical;
}
const cloud_step_m:f32=90.;
const cloud_max_steps:u32=128u;
struct CloudOutput {
    @location(0) scattering:vec4<f32>,
    @location(1) transmission:vec4<f32>,
};
@fragment fn fs_main(i:VertexOutput)->CloudOutput {
    let p=atmosphere.inverse_view_projection*vec4(i.uv.x*2.-1.,1.-i.uv.y*2.,0.,1.);
    let ray=normalize(p.xyz/p.w-atmosphere.relative_eye.xyz);
    let segment = cloud_interval(atmosphere.world_eye.y, ray.y);
    if segment.y <= segment.x || atmosphere.weather.x <= .001 { return CloudOutput(vec4(0.,0.,0.,1.),vec4(1.)); }
    let start = segment.x;
    let length_m = segment.y - start;
    // Keep interior sample locations continuous when another interval enters
    // at the far end of the ray. Redistributing every midpoint at that
    // boundary produced horizontal rings when flying inside the cloud layer.
    let spacing = clamp(cloud_step_m, length_m / f32(cloud_max_steps), length_m / 16.);
    let boundaries = length_m*vec4(.125,.25,.5,.75);
    var optical_depth=vec4(0.);
    let count = min(u32(ceil(length_m / spacing)), cloud_max_steps);
    var transmittance=1.;var light=vec3(0.);
    let warm=1.-smoothstep(.04,.45,atmosphere.sun.y);
    let sun_color=mix(vec3(1.12,1.08,1.00),vec3(1.15,.85,.58),warm*.7);
    let drift=vec2(1.,.32)*atmosphere.weather.w*atmosphere.weather.y*1000.;
    let weather_offset=vec2(atmosphere.relative_eye.w,atmosphere.sun.w)+drift;
    let forward=pow(max(0.,dot(ray,atmosphere.sun.xyz)),8.);
    for(var k=0u;k<count;k+=1u) {
        let offset = f32(k)*spacing;
        let step = min(spacing, length_m-offset);
        let position=atmosphere.world_eye.xyz+ray*(start+offset+.5*step);
        let q=position+vec3(weather_offset.x,0.,weather_offset.y);
        let bank=bank_coverage(q.xz);
        let rho=cloud_body(q,bank);
        if rho>.001 {
            let near_sun=cloud_body(q+atmosphere.sun.xyz*95.,bank);
            let far_sun=cloud_body(q+atmosphere.sun.xyz*310.,bank);
            let direct=exp(-near_sun*.80-far_sun*1.25)*(1.-atmosphere.weather.z*.65);
            let h=clamp((position.y-1720.)/680.,0.,1.);
            let ambient=mix(vec3(.39,.48,.61),vec3(.84,.89,.96),h);
            // Approximate the low-order multiple-scattering fill that keeps a
            // sunlit cumulus body pale rather than a dark pillow with a rim.
            let source=ambient*.72+sun_color*(.10+direct*(.69+.28*forward));
            // Integrate the fraction of this exact march interval lying in
            // front of each slice, including a final partial step.
            optical_depth+=rho*.0045*clamp(boundaries-vec4(offset),vec4(0.),vec4(step));
            let alpha=1.-exp(-rho*step*.0045);
            light+=transmittance*alpha*source;transmittance*=1.-alpha;
            if transmittance<.015 {break;}
        }
    }
    let horizon=1.-smoothstep(9000.,24000.,start);
    return CloudOutput(vec4(light*horizon,mix(1.,transmittance,horizon)),mix(vec4(1.),exp(-optical_depth),horizon));
}

// Integrate the same seeded density along the sunlight direction. Store
// transmission from four receiver heights through the top of the deck.
@fragment fn fs_cloud_sun(i:VertexOutput)->@location(0) vec4<f32> {
    let base = atmosphere.cloud_map.xy + (i.uv-.5) / atmosphere.cloud_map.w;
    let slope = atmosphere.sun.xz / atmosphere.sun.y;
    let step = 680./16.;
    var tau = vec4(0.);
    for (var k=0u; k<16u; k+=1u) {
        let y = (f32(k)+.5)*step;
        let xz = base + slope*y;
        let density = cloud_body(vec3(xz.x,1720.+y,xz.y),bank_coverage(xz));
        tau[k/4u] += density*(step*.0045/atmosphere.sun.y);
    }
    tau.z+=tau.w; tau.y+=tau.z; tau.x+=tau.y;
    return exp(-tau);
}
