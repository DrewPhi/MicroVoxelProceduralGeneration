// Daylight warms as its path through the atmosphere gets longer.
fn outdoor_sun_color(elevation:f32)->vec3<f32> {
    return mix(vec3(2.12,1.44,0.83),vec3(2.,1.78,1.46),smoothstep(.06,.64,elevation));
}

// Both terrain renderers use this transfer at the native-volume boundary.
fn regional_face_light(up:f32,sun_cosine:f32,sun:vec3<f32>,overcast:f32,visibility:f32,exposure:f32,beauty:bool)->vec3<f32> {
    let sky=0.5+0.5*up;
    var ambient=mix(vec3(0.15,0.14,0.115),vec3(0.29,0.36,0.44),sky);
    var sun_color=vec3(0.83,0.75,0.60);
    if beauty {
        ambient=mix(vec3(0.12,0.145,0.075),vec3(0.25,0.33,0.43),sky);
        sun_color=outdoor_sun_color(sun.y);
    }
    let direct=sun_color*sun_cosine;
    return ambient*exposure*(1.+overcast*0.3)+direct*(1.-0.72*overcast)*visibility;
}

fn regional_skylight(normal:vec3<f32>,sun:vec3<f32>,overcast:f32,visibility:f32,exposure:f32,beauty:bool)->vec3<f32> {
    let n=normalize(normal);
    return regional_face_light(n.y,max(dot(n,sun),0.),sun,overcast,visibility,exposure,beauty);
}

fn voxel_face_filter(projected_width:f32)->f32 {
    return .96-.30*smoothstep(1.,5.,projected_width);
}

// A rising stair admits the signed projected flux through its plane. Facets
// already hidden from the eye by a step in the sun's direction cannot shade
// the visible area again. The second component is its visible area fraction.
fn native_stair_visibility(n:vec3<f32>,sun:vec3<f32>,eye:vec3<f32>)->vec2<f32> {
    let light_projection=n*sun;
    let view_projection=n*eye;
    let light_total=max(.00001,dot(max(light_projection,vec3(0.)),vec3(1.)));
    let view_total=max(.00001,dot(max(view_projection,vec3(0.)),vec3(1.)));
    let occluded_light=max(-light_projection,vec3(0.))/light_total;
    let occluded_view=max(-view_projection,vec3(0.))/view_total;
    let common_loss=dot(min(occluded_light,occluded_view),vec3(1.));
    let exposed=dot(light_projection,vec3(1.))/light_total;
    return clamp(vec2(exposed/max(.00001,1.-common_loss),dot(view_projection,vec3(1.))/view_total),vec2(0.),vec2(1.));
}

// The open aperture of a native staircase has area length(n), while its
// axis-aligned roofs and risers have area sum(abs(n)). The difference is
// diffuse exchange between those faces. The crossed-string view factor is
// exact for a two-dimensional stair. In three dimensions distribute the
// same conserved area deficit across all three face orientations.
fn stair_ambient(n:vec3<f32>)->vec3<f32> {
    let area=abs(n);
    let measure=max(.00001,length(area));
    let other=sqrt(max(vec3(0.),vec3(measure*measure)-area*area));
    let loss=.5*(1.-area/(measure+other));
    let hidden=max(0.,dot(area,vec3(1.))-measure);
    let factor=hidden/max(.00001,dot(area,loss));
    // The native daylight model retains 45% ambient on adjacent blocked rays.
    return 1.-.55*min(vec3(1.),factor*loss);
}
