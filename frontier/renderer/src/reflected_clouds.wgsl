// Sample the cloud deck from the water surface. The low angular resolution
// filters the cloud light while native water normals retain their fine ripples.
fn water_cloud_light(relative:vec3<f32>,ray:vec3<f32>,clear:vec3<f32>,probe:vec3<f32>)->vec3<f32> {
    if probe.x<=0. || ray.y<=0. {return clear;}
    var direction=ray;
    let range=(probe.y-relative.y)/max(.001,ray.y);
    if range>20. {
        let toward=relative+ray*range-vec3(0.,probe.z,0.);
        direction=toward*inverseSqrt(max(dot(toward,toward),.001));
    }
    let x=atan2(direction.x,direction.z)*(64./6.283185307)-.5;
    let y=clamp(asin(clamp(direction.y,0.,1.))*(32./1.570796327)-.5,0.,31.);
    let i=u32(i32(floor(x))&63);
    let j=(i+1u)&63u;
    let lo=u32(y);let hi=min(lo+1u,31u);
    let clouds=mix(mix(cloud_texel(i,lo),cloud_texel(j,lo),fract(x)),
        mix(cloud_texel(i,hi),cloud_texel(j,hi),fract(x)),fract(y));
    return clear*(1.-clouds.a)+clouds.rgb*2.;
}
