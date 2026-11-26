// The projection takes metre positions relative to the render camera.
struct AircraftShadowReceiver {
    projection: mat4x4<f32>,
    parameters: vec4f,
}
@group(ROLLING_WATER_GROUP) @binding(15) var<uniform> aircraft_shadow: AircraftShadowReceiver;
@group(ROLLING_WATER_GROUP) @binding(16) var aircraft_depth: texture_depth_2d;
@group(ROLLING_WATER_GROUP) @binding(17) var aircraft_depth_compare: sampler_comparison;

fn aircraft_sun_visibility(relative:vec3f)->f32 {
    if aircraft_shadow.parameters.x < .5 { return 1.; }
    let q=(aircraft_shadow.projection*vec4f(relative,1.)).xyz;
    let uv=q.xy*vec2f(.5,-.5)+.5;
    if q.z<=0. || q.z>=1. || any(uv<=vec2f(0.)) || any(uv>=vec2f(1.)) { return 1.; }
    let light=textureSampleCompareLevel(aircraft_depth,aircraft_depth_compare,uv,q.z-aircraft_shadow.parameters.y);
    // Avoid a hard disappearance at the end of the local shadow volume.
    return mix(light,1.,smoothstep(.82,1.,q.z));
}

@group(ROLLING_WATER_GROUP) @binding(20) var<uniform> formation_shadow: AircraftShadowReceiver;
@group(ROLLING_WATER_GROUP) @binding(21) var formation_depth: texture_depth_2d;
@group(ROLLING_WATER_GROUP) @binding(22) var formation_depth_compare: sampler_comparison;

fn formation_sun_visibility(relative:vec3f)->f32 {
    if formation_shadow.parameters.x < .5 {return 1.;}
    let q = (formation_shadow.projection*vec4f(relative,1.)).xyz;
    let uv = q.xy*vec2f(.5,-.5)+.5;
    if q.z<=0. || q.z>=1. || any(uv<=vec2f(0.)) || any(uv>=vec2f(1.)) {return 1.;}
    return textureSampleCompareLevel(formation_depth,formation_depth_compare,uv,q.z-formation_shadow.parameters.y);
}
