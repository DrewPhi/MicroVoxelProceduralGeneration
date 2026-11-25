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
