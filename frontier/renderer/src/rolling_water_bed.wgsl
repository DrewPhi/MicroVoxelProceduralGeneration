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
    horizon_grid: vec4f,
    world_eye: vec4f,
    palette: array<vec4f, 10>,
}
@group(ROLLING_WATER_GROUP) @binding(9) var<uniform> native_view: NativeView;
@group(ROLLING_WATER_GROUP) @binding(11) var native_columns: texture_2d<i32>;
@group(ROLLING_WATER_GROUP) @binding(23) var terrain_horizon: texture_2d<f32>;
@group(ROLLING_WATER_GROUP) @binding(24) var terrain_horizon_sampler: sampler;

// Direct sun visibility and diffuse sky openness in both terrain owners.
fn terrain_irradiance(world:vec3f)->vec2f {
    let grid=native_view.horizon_grid;
    if grid.w<=0. || native_view.source.z==0u {return vec2f(1.);}
    let uv=(world.xz-grid.xy)*(grid.z/grid.w);
    let h=textureSampleLevel(terrain_horizon,terrain_horizon_sampler,uv,0.);
    return terrain_horizon_light(native_view.sun.xyz,world.y,h);
}

fn rolling_voxel_resolution(relative:vec3f, ray:vec3f, normal:vec3f)->f32 {
    let incidence = max(.015,-dot(ray,normal));
    let footprint = 2.*length(relative)*native_view.voxel.w*native_view.water.w/incidence;
    return smoothstep(.65,2.4,native_view.voxel.x/max(.0001,footprint));
}

// Exact depth still selects the first axis-aligned voxel face. The normal
// below estimates the terrain slope only when that face pattern is subpixel.
fn rolling_surface_filter(relative:vec3f, face:vec3f, ray:vec3f)->vec4f {
    let unchanged = vec4f(face,0.);
    if (native_view.source.w & 2u)==0u {return unchanged;}
    let grid = native_view.sky.zw + relative.xz*native_view.voxel.y;
    let cell = vec2i(floor(grid-face.xz*.012));
    let stride = clamp(i32(round(.72*native_view.voxel.y)),2,20);
    let side = i32(native_view.grid.w);
    if any(cell<vec2i(stride)) || any(cell>=vec2i(side-stride)) {return unchanged;}
    let center = textureLoad(native_columns,cell,0);
    let y = relative.y + native_view.voxel.z + native_view.sky.y;
    let top = f32(center.x)*native_view.voxel.x;
    if center.w!=0 || center.z!=center.x || y>top+native_view.voxel.x*.04 ||
        y<top-max(.52,native_view.voxel.x) {return unchanged;}
    let x0 = textureLoad(native_columns,cell-vec2i(stride,0),0);
    let x1 = textureLoad(native_columns,cell+vec2i(stride,0),0);
    let z0 = textureLoad(native_columns,cell-vec2i(0,stride),0);
    let z1 = textureLoad(native_columns,cell+vec2i(0,stride),0);
    if any(vec4i(x0.w,x1.w,z0.w,z1.w)!=vec4i(0)) {return unchanged;}
    let slope = vec2f(f32(x0.x-x1.x),f32(z0.x-z1.x))/(2.*f32(stride));
    let normal = normalize(vec3f(slope.x,1.,slope.y));
    let resolve = rolling_voxel_resolution(relative,ray,normal);
    let edge = min(cell.x,min(cell.y,min(side-1-cell.x,side-1-cell.y)));
    let blend = (1.-resolve)*smoothstep(f32(stride),f32(stride)*1.5,f32(edge));
    return vec4f(normal,blend);
}

fn rolling_light_visible(normal:vec3f,visibility:f32)->vec3f {
    return rolling_light_irradiance(normal,visibility,1.);
}

fn rolling_light_irradiance(normal:vec3f,visibility:f32,openness:f32)->vec3f {
    let sky=.5+.5*normal.y;
    let cosine=max(dot(normal,native_view.sun.xyz),0.);
    if native_view.sun.w>.5 {
        let ambient=mix(vec3f(.12,.145,.075),vec3f(.25,.33,.43),sky)*(1.+native_view.sky.x*.3);
        return ambient*openness+outdoor_sun_color(native_view.sun.y)*cosine*(1.-.72*native_view.sky.x)*visibility;
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
    let surface=select(face,vec3f(0.,1.,0.),(native_view.source.w&8u)!=0u);
    let normal=rolling_water_normal(relative,surface,ray,distance);
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
