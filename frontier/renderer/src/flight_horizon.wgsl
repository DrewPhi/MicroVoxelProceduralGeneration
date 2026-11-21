const unit: f32 = GRID_UNIT;
struct Camera { matrix: mat4x4<f32>, eye: vec4<f32>, sun: vec4<f32>, screen: vec4<f32>, cloud_panorama:array<vec4<u32>,512> };
@group(0) @binding(0) var<uniform> camera: Camera;
struct EditFrame { eye:vec4<f32>, origin:vec4<u32> };
@group(0) @binding(1) var edited_columns:texture_2d<u32>;
@group(0) @binding(2) var<uniform> edit:EditFrame;
struct Vertex {
    @builtin(position) clip: vec4<f32>,
    @location(0) relative: vec3<f32>,
    @location(1) @interpolate(flat) color: vec3<f32>,
    @location(2) view_depth: f32,
    @location(3) @interpolate(flat) optics:vec3<f32>,
    @location(4) @interpolate(flat) normal_xz:vec2<f32>,
};
fn face_color(albedo:vec3<f32>, direct:f32, sky:f32)->vec3<f32> {
    let fill=mix(vec3(.12,.145,.075),vec3(.25,.33,.43),sky)*(1.+.3*camera.sun.w);
    let light=outdoor_sun_color(camera.sun.y)*direct*(1.-.72*camera.sun.w);
    return albedo*(fill+light);
}
@vertex fn vs(@builtin(vertex_index) i:u32, @location(0) packed:vec2<u32>, @location(1) heights:vec2<f32>) -> Vertex {
    let axis = (packed.y>>24u)&7u;
    let size = unit*f32(1u<<((packed.y>>27u)&3u));
    let x = f32(bitcast<i32>(packed.x<<23u)>>23u)*size;
    let z = f32(bitcast<i32>(packed.x<<14u)>>23u)*size;
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
    // Each coarse roof and riser covers many native stairs. Weight their
    // upward and outward faces by projected area, leaving some direct face
    // contrast when the geometry spans multiple pixels.
    let slope=vec2<f32>(f32(bitcast<i32>(packed.x<<7u)>>25u),f32(bitcast<i32>(packed.x)>>25u))/63.;
    let relief=vec3(slope.x,max(0.,1.-abs(slope.x)-abs(slope.y)),slope.y);
    var center=vec3(x+.5*size,heights.y,z+.5*size);
    var face_width=size;
    if axis!=0u {
        center.y=.5*(heights.x+heights.y);
        if axis<=2u {center.x=x;} else {center.z=z;}
        face_width=min(size,heights.y-heights.x);
    }
    let to_eye=camera.eye.xyz-center;
    let pixel_span=camera.screen.x*face_width*abs(dot(normal,to_eye))/max(1.,dot(to_eye,to_eye));
    let projected=max(vec3(0.),to_eye*relief);
    let sum=dot(projected,vec3(1.));
    let areas=projected/max(sum,.0001);
    let native_sun=dot(areas,max(vec3(0.),sign(relief)*camera.sun.xyz))*native_stair_visibility(relief,camera.sun.xyz,to_eye).x;
    let native_span=length(to_eye)/max(.0001,camera.screen.x*NATIVE_EDGE);
    let filtered=select(0.,mix(voxel_face_filter(pixel_span),.98,smoothstep(2.,8.,native_span)),sum>.0001);
    let direct=mix(max(0.,dot(normal,camera.sun.xyz)),native_sun,filtered);
    let sky=.5+.5*mix(normal.y,areas.y,filtered);
    let color=face_color(albedo,direct,sky);
    let clip=camera.matrix*vec4(p,1.);
    var optics=vec3(0.);
    if axis==0u {
        let word=u32(heights.x);
        optics=vec3(f32(packed.y>>29u)/7.,f32((word>>10u)&63u)*.25,f32(word>>16u)/3.);
    }
    return Vertex(clip,p-camera.eye.xyz,color,clip.w,optics,normal.xz);
}
@vertex fn vs_edit(@builtin(vertex_index) i:u32, @location(0) anchor:vec3<f32>,
    @location(1) extent:vec2<f32>, @location(2) axis_sign:u32, @location(3) material:u32)->Vertex {
    let axis=axis_sign&3u;
    let negative=(axis_sign&4u)!=0u;
    var p=anchor;
    let u=f32(i&1u);
    p[(axis+1u)%3u]+=extent.x*select(u,1.-u,negative);
    p[(axis+2u)%3u]+=extent.y*f32(i>>1u);
    var normal=vec3(0.);
    normal[axis]=select(1.,-1.,negative);
    let relative=p-edit.eye.xyz;
    let clip=camera.matrix*vec4(relative+camera.eye.xyz,1.);
    let color=face_color(MATERIAL_COLORS[material],max(0.,dot(normal,camera.sun.xyz)),.5+.5*normal.y);
    return Vertex(clip,relative,color,clip.w,vec3(0.),normal.xz);
}
fn cloud_texel(x:u32,y:u32)->vec4<f32> {
    return unpack4x8unorm(camera.cloud_panorama[16u*y+x/4u][x%4u]);
}
fn surface(v:Vertex) -> vec4<f32> {
    if v.view_depth < camera.eye.w { discard; }
    var color=v.color;
    if v.optics.x>0. {
        let ray=normalize(v.relative);
        let normal=vec3(0.,1.,0.);
        let reflected=reflect(ray,normal);
        let above=water_cloud_light(v.relative,reflected,water_sky(reflected,camera.sun.xyz,camera.sun.w),camera.screen.yzw);
        let cosine=clamp(-ray.y,0.,1.);
        let path=v.optics.y*inverseSqrt(1.-.5628*(1.-cosine*cosine));
        let surface=mix(water_medium(color,path),above,water_fresnel(cosine))
            +water_glint(ray,normal,camera.sun.xyz,camera.sun.w);
        let foam=rapid_foam(v.relative.xz+camera.eye.xz,0.,8.,v.optics.z,vec2(0.));
        let lit=face_color(vec3(1.),max(camera.sun.y,0.),1.);
        color=mix(color,foam_color(surface,lit,foam),v.optics.x);
    }
    return vec4(color,length(v.relative)*.001);
}
@fragment fn fs(v:Vertex) -> @location(0) vec4<f32> {return surface(v);}
@fragment fn fs_masked(v:Vertex) -> @location(0) vec4<f32> {
    let local=vec2<i32>(floor((v.relative.xz+edit.eye.xz)*edit.eye.w-v.normal_xz*.01));
    if edit_owns_column(edited_columns,edit.origin.xz+bitcast<vec2<u32>>(local)) {discard;}
    return surface(v);
}
