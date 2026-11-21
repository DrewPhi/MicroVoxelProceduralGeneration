struct GroundLight {
    origin_strength: vec4<f32>,
    nodes: array<vec4<f32>,169>,
    water_options:vec4<f32>, // cloud map enabled, deck relative to eye, river course present, probe height relative to eye
    currents:array<vec4<u32>,43>,
    wet_depths:array<vec4<u32>,11>,
    reflection_origin:vec4<f32>,
    skyline:array<vec4<f32>,16>,
    terrain_panorama:array<vec4<u32>,256>,
    cloud_panorama:array<vec4<u32>,512>,
};
@group(NAADF_GROUP) @binding(NAADF_BASE+9u) var<uniform> ground_light: GroundLight;

fn ground_available() -> bool {
    return ground_light.origin_strength.w>0.;
}

fn current_node(i:u32)->vec4<f32> {
    return unpack4x8snorm(ground_light.currents[i/4u][i%4u]);
}

fn stream_current(relative:vec2<f32>)->vec4<f32> {
    let radius=max(abs(relative.x),abs(relative.y));
    if ground_light.water_options.z<=0. || radius>=62. {return vec4(0.);}
    let grid=clamp((relative-ground_light.origin_strength.xy)/ground_light.origin_strength.w,
        vec2(0.),vec2(12.));
    let corner=vec2<u32>(min(floor(grid),vec2(11.)));
    let w=grid-vec2<f32>(corner);
    let i=corner.x+13u*corner.y;
    let a=mix(current_node(i),current_node(i+1u),w.x);
    let b=mix(current_node(i+13u),current_node(i+14u),w.x);
    let course=mix(a,b,w.y);
    let magnitude=length(course.xy);
    if magnitude<.015 {return vec4(0.);}
    return vec4(course.xy/magnitude,course.z/max(course.w,.001),
        smoothstep(.015,.56,magnitude)*(1.-smoothstep(42.,62.,radius)));
}

fn ground_sample(position:vec3<f32>) -> vec4<f32> {
    let grid=clamp((position.xz-ground_light.origin_strength.xy)
        /ground_light.origin_strength.w,vec2(0.),vec2(12.));
    let corner=vec2<u32>(min(floor(grid),vec2(11.)));
    let weight=grid-vec2<f32>(corner);
    let i=corner.x+corner.y*13u;
    let low=mix(ground_light.nodes[i],ground_light.nodes[i+1u],weight.x);
    let high=mix(ground_light.nodes[i+13u],ground_light.nodes[i+14u],weight.x);
    return mix(low,high,weight.y);
}

// Beyond the native volume a cut optical ray can still see shallow water.
// Dry samples contribute no bed pigment or depth to the reconstruction.
fn surveyed_water_bed(relative:vec3<f32>)->vec4<f32> {
    let grid=(relative.xz-ground_light.origin_strength.xy)/max(.001,ground_light.origin_strength.w);
    if !ground_available() || any(grid<vec2(0.)) || any(grid>vec2(12.)) {return vec4(0.);}
    let corner=vec2<u32>(min(floor(grid),vec2(11.)));
    let f=grid-vec2<f32>(corner);
    let weights=vec4((1.-f.x)*(1.-f.y),f.x*(1.-f.y),(1.-f.x)*f.y,f.x*f.y);
    let indices=vec4(corner.x+13u*corner.y)+vec4(0u,1u,13u,14u);
    var bed=vec4(0.);
    var total=0.;
    for (var k=0u;k<4u;k++) {
        let i=indices[k];
        let code=(ground_light.wet_depths[i/16u][(i/4u)%4u]>>(8u*(i%4u)))&255u;
        if code>0u {
            let w=max(.005,weights[k]);
            bed+=vec4(ground_light.nodes[i].rgb,max(.03125,f32(code-1u)/16.))*w;
            total+=w;
        }
    }
    if total>0. {return bed/total;}
    return vec4(0.);
}

// First-bounce approximation for an illuminated ground hemisphere. The same
// sky/shadow inputs drive all receivers, and only diffuse illumination changes.
fn ground_irradiance(position:vec3<f32>,normal:vec3<f32>,sun:vec3<f32>,
    sunlight:vec3<f32>,overcast:f32,ground_visibility:f32) -> vec3<f32> {
    let strength=ground_light.origin_strength.z;
    if strength<=0. || normal.y>0.98 || max(abs(position.x),abs(position.z))>66. {
        return vec3(0.);
    }
    let ground=ground_sample(position);
    let elevation=position.y-ground.a;
    let range=1.-smoothstep(2.,15.,abs(elevation));
    let directional=0.5-0.5*normal.y;
    let lit=max(sun.y,0.)*(1.-0.72*overcast)*mix(0.18,1.,ground_visibility);
    let edge=1.-smoothstep(48.,65.,max(abs(position.x),abs(position.z)));
    return ground.rgb*sunlight*(strength*range*directional*edge*(lit+0.09));
}

fn cloud_texel(x:u32,y:u32)->vec4<f32> {
    return unpack4x8unorm(ground_light.cloud_panorama[16u*y+x/4u][x%4u]);
}

// Sample the actual valley around the nearest wet ground. The hit range lets
// the second lookup correct for the offset from the probe to this water face.
fn panorama_sample(direction:vec3<f32>)->vec4<f32> {
    let azimuth=atan2(direction.x,direction.z)*(64./6.283185307);
    let index=u32(i32(floor(azimuth))&63);
    let next=(index+1u)&63u;
    let angle=asin(clamp(direction.y,-1.,1.));
    let row=clamp(angle*(15./1.3),0.,15.);
    let j=u32(row);
    let k=min(j+1u,15u);
    let a=ground_light.terrain_panorama[index*4u+j/4u][j%4u];
    let b=ground_light.terrain_panorama[index*4u+k/4u][k%4u];
    let c=ground_light.terrain_panorama[next*4u+j/4u][j%4u];
    let d=ground_light.terrain_panorama[next*4u+k/4u][k%4u];
    let shift=vec4<u32>(0u,8u,16u,24u);
    let ca=vec4<f32>((vec4(a)>>shift)&vec4(255u));
    let cb=vec4<f32>((vec4(b)>>shift)&vec4(255u));
    let cc=vec4<f32>((vec4(c)>>shift)&vec4(255u));
    let cd=vec4<f32>((vec4(d)>>shift)&vec4(255u));
    return mix(mix(ca,cb,fract(row)),mix(cc,cd,fract(row)),fract(azimuth));
}
fn reflected_terrain(relative:vec3<f32>,ray:vec3<f32>,clear:vec3<f32>)->vec3<f32> {
    let sky=water_cloud_light(relative,ray,clear,ground_light.water_options.xyw);
    if ground_light.reflection_origin.w<=0. || ray.y<-.05 || ray.y>.985 {return sky;}
    let offset=relative-ground_light.reflection_origin.xyz;
    let local=(1.-smoothstep(90.,290.,length(offset)))*ground_light.reflection_origin.w;
    if local<=0. {return sky;}
    let first=panorama_sample(ray);
    let range=16.*pow(1.35,first.a);
    let projected=ray+offset*sqrt(max(.001,1.-ray.y*ray.y))/range;
    let length2=dot(projected,projected);
    if length2<.000001 {return sky;}
    let corrected=projected*inverseSqrt(length2);
    let az=atan2(corrected.x,corrected.z)*(64./6.283185307);
    let i=u32(i32(floor(az))&63);
    let j=(i+1u)&63u;
    let limit=mix(ground_light.skyline[i/4u][i%4u],ground_light.skyline[j/4u][j%4u],fract(az));
    let coverage=smoothstep(-.018,.018,limit-asin(clamp(corrected.y,-1.,1.)))*local;
    if coverage<=0. {return sky;}
    let sample=panorama_sample(corrected);
    // A long return path picks up the same blue air that softens a distant ridge.
    let air=exp(-.00013*16.*pow(1.35,sample.a));
    return mix(sky,sample.rgb*(2./255.),coverage*air);
}
