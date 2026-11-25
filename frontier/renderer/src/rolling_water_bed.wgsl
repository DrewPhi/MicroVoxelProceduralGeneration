// Canonical near columns are the common source for the water bed on either
// side of NAADF's resident box. Positions are relative to the camera.
struct NativeView {
    grid: vec4f,
    owner_min: vec4f,
    owner_max: vec4f,
    sun: vec4f,
    sky: vec4f, // overcast, low part of eye Y, native-local eye X/Z
    voxel: vec4f, // edge, reciprocal edge, relative eye Y, tan(half vertical FOV)
    water: vec4f, // world phase of camera X/Z, elapsed seconds, reciprocal image height
    source: vec4u,
    palette: array<vec4f, 10>,
}
@group(ROLLING_WATER_GROUP) @binding(9) var<uniform> native_view: NativeView;
@group(ROLLING_WATER_GROUP) @binding(11) var native_columns: texture_2d<i32>;

fn rolling_light_visible(normal:vec3f,visibility:f32)->vec3f {
    let sky=.5+.5*normal.y;
    let cosine=max(dot(normal,native_view.sun.xyz),0.);
    if native_view.sun.w>.5 {
        let ambient=mix(vec3f(.12,.145,.075),vec3f(.25,.33,.43),sky)*(1.+native_view.sky.x*.3);
        return ambient+outdoor_sun_color(native_view.sun.y)*cosine*(1.-.72*native_view.sky.x)*visibility;
    }
    return mix(vec3f(.12,.105,.085),vec3f(.28,.33,.40),sky)+vec3f(.78,.70,.58)*cosine*visibility;
}

fn rolling_water_normal(relative:vec3f,face:vec3f,ray:vec3f,distance:f32)->vec3f {
    let footprint=2.*distance*native_view.voxel.w*native_view.water.w/max(.12,abs(dot(ray,face)));
    let amount=.023*(1.-smoothstep(.065,.28,footprint))*(1.-smoothstep(22.,78.,distance));
    if amount<=0. { return face; }
    let phase=relative.xz+native_view.water.xy;
    let ripple=water_ripple(vec3f(phase.x,0.,phase.y),face,native_view.water.z,amount);
    return select(face,ripple,dot(ray,ripple)<-.002);
}

fn rolling_bed_transmission(ray:vec3f,normal:vec3f,bed:vec3f,depth:f32)->vec3f {
    let transmitted=refract(ray,normal,1./1.333);
    let path=depth/max(.05,-transmitted.y);
    let color=bed*rolling_light_visible(vec3f(0.,1.,0.),.48);
    return water_medium(color,path);
}

fn rolling_column_optics(relative:vec3f,face:vec3f,ray:vec3f,distance:f32)->WaterOptics {
    let normal=rolling_water_normal(relative,face,ray,distance);
    // Sample on the water side of a voxel riser.
    let cell=vec2i(floor(native_view.sky.zw+relative.xz*native_view.voxel.y-face.xz*.005));
    if any(cell<vec2i(0)) || any(cell>=vec2i(i32(native_view.grid.w))) {
        return WaterOptics(normal,water_deep(),1.);
    }
    let column=textureLoad(native_columns,cell,0);
    let ground=f32(column.x)*native_view.voxel.x;
    let depth=(native_view.voxel.z-ground)+native_view.sky.y+relative.y;
    let bed=native_view.palette[u32(column.y&255)].rgb;
    return WaterOptics(normal,rolling_bed_transmission(ray,normal,bed,max(0.,depth)),1.);
}
