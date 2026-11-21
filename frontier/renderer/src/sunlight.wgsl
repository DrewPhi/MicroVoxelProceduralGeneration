// Daylight warms as its path through the atmosphere gets longer.
fn outdoor_sun_color(elevation:f32)->vec3<f32> {
    return mix(vec3(2.12,1.44,0.83),vec3(2.,1.78,1.46),smoothstep(.06,.64,elevation));
}

// Both terrain renderers use this transfer at the native-volume boundary.
fn regional_skylight(normal:vec3<f32>,sun:vec3<f32>,overcast:f32,visibility:f32,exposure:f32,beauty:bool)->vec3<f32> {
    let n=normalize(normal);
    let sky=0.5+0.5*n.y;
    var ambient=mix(vec3(0.15,0.14,0.115),vec3(0.29,0.36,0.44),sky);
    var sun_color=vec3(0.83,0.75,0.60);
    if beauty {
        ambient=mix(vec3(0.12,0.145,0.075),vec3(0.25,0.33,0.43),sky);
        sun_color=outdoor_sun_color(sun.y);
    }
    let direct=sun_color*max(dot(n,sun),0.);
    return ambient*exposure*(1.+overcast*0.3)+direct*(1.-0.72*overcast)*visibility;
}
