// Daylight warms as its path through the atmosphere gets longer.
fn outdoor_sun_color(elevation:f32)->vec3<f32> {
    return mix(vec3(2.12,1.44,0.83),vec3(2.,1.78,1.46),smoothstep(.06,.64,elevation));
}
