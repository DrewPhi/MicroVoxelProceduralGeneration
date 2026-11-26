const ROLLING_WATER_GROUP = 3u;

fn naadf_water_optics(surface:vec3f,face:vec3f,ray:vec3f,distance:f32)->WaterOptics {
    var optics=rolling_column_optics(surface-naadf.eye.xyz,face,ray,distance);
    let size=vec3f(naadf.extent.xyz)*NATIVE_VOXEL_METERS;
    let side=min(surface,size-surface);
    let margin=min(side.x,min(side.y,side.z));
    let apron=NATIVE_VOXEL_METERS;
    let width=min(2.5,.25*min(size.x,min(size.y,size.z)));
    // Match the column renderer at the ownership edge. Retain native bed
    // refraction and bank reflection progressively farther inside the box.
    optics.detail=smoothstep(apron,max(apron+.01,width),margin);
    return optics;
}


fn naadf_terrain_lighting_normal(hit: vec3f, face: vec3f, ray: vec3f) -> vec3f {
    let filtered = rolling_surface_filter(hit-naadf.eye.xyz,face,ray);
    return normalize(mix(face,filtered.xyz,filtered.w));
}

fn naadf_gray_enabled() -> bool {return (native_view.source.w&4u)!=0u;}

fn naadf_terrain_irradiance(hit:vec3f)->vec2f {
    return terrain_irradiance(native_view.world_eye.xyz+hit-naadf.eye.xyz);
}
