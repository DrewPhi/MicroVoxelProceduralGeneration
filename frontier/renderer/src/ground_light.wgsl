struct GroundLight {
    origin_strength: vec4<f32>,
    nodes: array<vec4<f32>,169>,
};
@group(NAADF_GROUP) @binding(NAADF_BASE+9u) var<uniform> ground_light: GroundLight;

fn ground_available() -> bool {
    return ground_light.origin_strength.w>0.;
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

// Daylight warms as its path through the atmosphere gets longer.
fn outdoor_sun_color(elevation:f32)->vec3<f32> {
    return mix(vec3(2.12,1.44,0.83),vec3(2.,1.78,1.46),smoothstep(.06,.64,elevation));
}

fn leaf_transmitted_light(
    ray:vec3f, normal:vec3f, sun:vec3f, sunlight:vec3f,
    transmission:f32, visibility:f32, ao:f32,
)->vec3f {
    let alignment=max(dot(ray,sun),0.);
    let forward=alignment*alignment*alignment*alignment;
    let back=max(-dot(normal,sun),0.);
    return sunlight*vec3f(.88,1.,.44)*.5*transmission
        *(.25+.75*forward)*back*(.08+.92*visibility)*ao*ao;
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
