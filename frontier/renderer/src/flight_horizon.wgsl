const unit: f32 = GRID_UNIT;
struct Camera { matrix: mat4x4<f32>, eye: vec4<f32>, sun: vec4<f32>, screen: vec4<f32> };
@group(0) @binding(0) var<uniform> camera: Camera;
struct Vertex {
    @builtin(position) clip: vec4<f32>,
    @location(0) relative: vec3<f32>,
    @location(1) @interpolate(flat) color: vec3<f32>,
    @location(2) view_depth: f32,
    @location(3) @interpolate(flat) optics:vec2<f32>,
};
@vertex fn vs(@builtin(vertex_index) i:u32, @location(0) packed:vec2<u32>, @location(1) heights:vec2<f32>) -> Vertex {
    let x = f32(bitcast<i32>(packed.x<<21u)>>21u)*unit;
    let z = f32(bitcast<i32>(packed.x<<10u)>>21u)*unit;
    let axis = (packed.y>>24u)&7u;
    let size = unit*f32(1u<<((packed.y>>27u)&3u));
    let u = f32(i&1u);
    let v = f32(i>>1u);
    var p = vec3(x,heights.y,z);
    if axis==0u {
        let span=u32(heights.x);
        p.x += v*size*f32(1u+(span&31u));
        p.z += u*size*f32(1u+((span>>5u)&31u));
    }
    var normal = vec3(0.,1.,0.);
    if axis==1u || axis==2u {
        p = vec3(x,mix(heights.x,heights.y,v),z+select(u,1.-u,axis==1u)*size);
        normal = vec3(select(-1.,1.,axis==1u),0.,0.);
    } else if axis>=3u {
        p = vec3(x+select(u,1.-u,axis==4u)*size,mix(heights.x,heights.y,v),z);
        normal = vec3(0.,0.,select(-1.,1.,axis==3u));
    }
    let albedo=vec3<f32>(vec3<u32>(packed.y,packed.y>>8u,packed.y>>16u)&vec3(255u))/255.;
    // The roof and each riser keep their own lighting when their shorter
    // dimension resolves in the horizon render target. Only thin silhouettes
    // and grazing faces take light from the surrounding sampled relief.
    let slope=vec2<f32>(f32(bitcast<i32>(packed.x<<5u)>>27u),f32(bitcast<i32>(packed.x)>>27u))/15.;
    let relief=normalize(vec3(slope.x,max(0.,1.-abs(slope.x)-abs(slope.y)),slope.y));
    var center=vec3(x+.5*size,heights.y,z+.5*size);
    var face_width=size;
    if axis!=0u {
        center.y=.5*(heights.x+heights.y);
        if axis<=2u {center.x=x;} else {center.z=z;}
        face_width=min(size,heights.y-heights.x);
    }
    let to_eye=camera.eye.xyz-center;
    let pixel_span=camera.screen.x*face_width*abs(dot(normal,to_eye))/max(1.,dot(to_eye,to_eye));
    let unresolved=1.-smoothstep(.5,2.5,pixel_span);
    let direct=mix(max(0.,dot(normal,camera.sun.xyz)),max(0.,dot(relief,camera.sun.xyz)),unresolved);
    let sky=.5+.5*mix(normal.y,relief.y,unresolved);
    let fill=mix(vec3(.12,.145,.075),vec3(.25,.33,.43),sky)*(1.+.3*camera.sun.w);
    let light=vec3(.93,.84,.65)*direct*(1.-.72*camera.sun.w);
    let color=albedo*(fill+light);
    let clip=camera.matrix*vec4(p,1.);
    var optics=vec2(0.);
    if axis==0u {optics=vec2(f32(packed.y>>29u)/7.,f32(u32(heights.x)>>10u)*.25);}
    return Vertex(clip,p-camera.eye.xyz,color,clip.w,optics);
}
@fragment fn fs(v:Vertex) -> @location(0) vec4<f32> {
    if v.view_depth < camera.eye.w { discard; }
    var color=v.color;
    if v.optics.x>0. {
        let ray=normalize(v.relative);
        let normal=vec3(0.,1.,0.);
        let above=water_sky(reflect(ray,normal),camera.sun.xyz,camera.sun.w);
        let cosine=clamp(-ray.y,0.,1.);
        let path=v.optics.y*inverseSqrt(1.-.5628*(1.-cosine*cosine));
        let surface=mix(water_medium(color,path),above,water_fresnel(cosine))
            +water_glint(ray,normal,camera.sun.xyz,camera.sun.w);
        color=mix(color,surface,v.optics.x);
    }
    return vec4(color,length(v.relative)*.001);
}
