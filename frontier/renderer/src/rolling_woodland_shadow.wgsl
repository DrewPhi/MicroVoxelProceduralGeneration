struct WoodlandShadowReceiver {
    projection: mat4x4<f32>,
    parameters: vec4f,
}
@group(ROLLING_WATER_GROUP) @binding(25) var<uniform> woodland_shadow: WoodlandShadowReceiver;
@group(ROLLING_WATER_GROUP) @binding(26) var woodland_depth: texture_depth_2d;
@group(ROLLING_WATER_GROUP) @binding(27) var woodland_compare: sampler_comparison;

fn woodland_shadow_active()->bool {
    return woodland_shadow.parameters.x>.5;
}

fn woodland_sun_visibility(relative:vec3f)->f32 {
    if !woodland_shadow_active() || dot(relative,relative)>48400. { return 1.; }
    let q = (woodland_shadow.projection*vec4f(relative,1.)).xyz;
    let uv = q.xy*vec2f(.5,-.5)+.5;
    if q.z<=0. || q.z>=1. || any(uv<=vec2f(0.)) || any(uv>=vec2f(1.)) { return 1.; }
    let edge=max(abs(q.x),abs(q.y));
    let weight=1.-smoothstep(.72,.99,edge);
    let texel=1./vec2f(textureDimensions(woodland_depth));
    let z=q.z-woodland_shadow.parameters.y;
    let a=textureSampleCompareLevel(woodland_depth,woodland_compare,uv+vec2f(-1.2,-.6)*texel,z);
    let b=textureSampleCompareLevel(woodland_depth,woodland_compare,uv+vec2f(.6,-1.2)*texel,z);
    let c=textureSampleCompareLevel(woodland_depth,woodland_compare,uv+vec2f(-.6,1.2)*texel,z);
    let d=textureSampleCompareLevel(woodland_depth,woodland_compare,uv+vec2f(1.2,.6)*texel,z);
    return mix(1.,.12+.22*(a+b+c+d),weight);
}
