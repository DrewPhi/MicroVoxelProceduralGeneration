@group(3) @binding(2) var crown_color_views:texture_2d_array<f32>;
@group(3) @binding(3) var crown_surface_views:texture_2d_array<f32>;
@group(3) @binding(4) var crown_view_sampler:sampler;
override CROWN_ATLAS_TRIANGLE:bool=false;
override CROWN_ATLAS_FILTER:bool=false;
override CROWN_ATLAS_PROJECTED_FOOTPRINT:bool=false;
override CROWN_ATLAS_ZENITH:bool=false;
struct AtlasBounds {
    @builtin(position) clip:vec4f,
    @location(0) view_face:vec3f,
    @location(1) @interpolate(flat) root:vec3f,
    @location(2) @interpolate(flat) turn:u32,
    @location(3) @interpolate(flat) fade:f32,
    @location(4) @interpolate(flat) seed:u32,
    @location(5) @interpolate(flat) view_mix:vec4f,
    @location(6) view_texels:vec2f,
}

fn crown_center()->vec3f {
    return (crown_min()+.5*vec3f(crown_size()))*crown_pitch();
}
fn crown_local_eye(root:vec3f,turn:u32)->vec3f {
    let pivot=crown_pivot();
    return (pivot+rotate(camera.eye.xyz-root-pivot,4u-(turn&3u)))
        /vec3f(1.,plant_height_scale(turn),1.);
}
fn crown_right(v:vec3f)->vec3f {
    let horizontal=length(v.xz);
    return select(vec3f(0.,0.,-1.),vec3f(v.z,0.,-v.x)/max(horizontal,1.e-6),horizontal>1.e-6);
}
@vertex fn vs_crown_atlas(
    @builtin(vertex_index) vertex:u32,
    @location(3) root:vec3f, @location(4) turn:u32, @location(5) fade:f32, @location(6) seed:u32,
)->AtlasBounds {
    let center=crown_center();
    let direction=normalize(crown_local_eye(root,turn)-center);
    let right=crown_right(direction);
    let up=cross(direction,right);
    let half=.5*vec3f(crown_size())*crown_pitch();
    let x=dot(abs(right),half);
    let y=dot(abs(up),half);
    let corner=triangle_corner(1u,vertex);
    let u=select(-1.,1.,corner==1u||corner==2u);
    let v=select(-1.,1.,corner>=2u);
    let local=center+right*(x*u)+up*(y*v);
    let pivot=crown_pivot();
    var out:AtlasBounds;
    out.view_face=root+pivot+rotate(local*vec3f(1.,plant_height_scale(turn),1.)-pivot,turn);
    out.clip=camera.view_projection*vec4f(out.view_face,1.);
    out.root=root;
    out.turn=turn;
    out.fade=fade;
    out.seed=seed;
    out.view_texels=vec2f(15.*u,-27.*v);
    let yaw=atan2(direction.z,direction.x)/CROWN_YAW_STEP;
    let pitch=asin(clamp(direction.y,-1.,1.));
    let y0=u32(i32(floor(yaw))+8)&7u;
    let e0=select(select(0u,1u,pitch>CROWN_ELEVATION[1]),2u,pitch>CROWN_ELEVATION[2]);
    let b=clamp((pitch-CROWN_ELEVATION[e0])/(CROWN_ELEVATION[e0+1u]-CROWN_ELEVATION[e0]),0.,1.);
    out.view_mix=vec4f(f32(y0),f32(e0),fract(yaw),b);
    return out;
}
struct AtlasHit {
    color:vec4f,
    normal_t:vec4f,
}
fn crown_view(
    p:vec3f, toward_eye:vec3f, half:vec3f, azimuth:u32, elevation:u32, lod:f32,
)->AtlasHit {
    let yaw=CROWN_AZIMUTH_AXES[azimuth&7u];
    let pitch=CROWN_ELEVATION_AXES[elevation];
    let direction=vec3f(yaw.x*pitch.x,pitch.y,yaw.y*pitch.x);
    let right=vec3f(yaw.y,0.,-yaw.x);
    return projected_crown_view(p,toward_eye,half,direction,right,i32((azimuth&7u)+8u*elevation),lod);
}
fn projected_crown_view(
    p:vec3f, toward_eye:vec3f, half:vec3f, direction:vec3f, right:vec3f, index:i32, lod:f32,
)->AtlasHit {
    let up=cross(direction,right);
    let range=vec2f(dot(abs(right),half),dot(abs(up),half));
    // Projection mirrors the one-texel border in the CPU atlas.
    let uv=.5+vec2f(dot(p,right),-dot(p,up))/range*vec2f(.46875,.482142857);
    let color=textureSampleLevel(crown_color_views,crown_view_sampler,uv,index,lod);
    let surface=textureSampleLevel(crown_surface_views,crown_view_sampler,uv,index,lod);
    let depth=(2.*surface.a-color.a)*dot(abs(direction),half);
    let travel=(depth-dot(p,direction)*color.a)/max(.25,dot(toward_eye,direction));
    return AtlasHit(color,vec4f(2.*surface.rgb-color.a,travel));
}
@fragment fn fs_crown_atlas(i:AtlasBounds)->CrownFragment {
    let pivot=crown_pivot();
    let inverse=4u-(i.turn&3u);
    let scale=vec3f(1.,plant_height_scale(i.turn),1.);
    let center=crown_center();
    let local_eye=crown_local_eye(i.root,i.turn);
    let direction=normalize(local_eye-center);
    let toward_eye=normalize(rotate(camera.eye.xyz-i.view_face,inverse)/scale);
    let p=(pivot+rotate(i.view_face-i.root-pivot,inverse))/scale-center;
    let half=.5*vec3f(crown_size())*crown_pitch();
    var lod=0.;
    if CROWN_ATLAS_FILTER {
        // Filter premultiplied coverage, pigment, normal and depth together.
        // Conservative screen differentials limit hairline crown shimmer.
        if CROWN_ATLAS_PROJECTED_FOOTPRINT {
            // Interpolate the camera-facing plane in atlas texels. Its
            // derivative already accounts for perspective and view angle.
            let a=dpdx(i.view_texels);
            let b=dpdy(i.view_texels);
            lod=clamp(.5*log2(max(1.,max(dot(a,a),dot(b,b)))),0.,3.);
        } else {
            let footprint=max(length(dpdx(p)),length(dpdy(p)));
            let texels_per_meter=max(16./max(half.x,half.z),28./half.y);
            lod=clamp(log2(max(1.,footprint*texels_per_meter)),0.,3.);
        }
    }
    let y0=u32(i.view_mix.x);
    let e0=u32(i.view_mix.y);
    let a=i.view_mix.z;
    let b=i.view_mix.w;
    var color:vec4f;
    var surface_premultiplied:vec4f;
    if CROWN_ATLAS_ZENITH && direction.y>CROWN_ELEVATION_AXES[3].y {
        // All azimuths converge to the same physical view overhead. Only
        // the top slanted ring contributes in this elevation band.
        let blend=smoothstep(CROWN_ELEVATION_AXES[3].y,.995,direction.y);
        let overhead=projected_crown_view(p,toward_eye,half,vec3f(0.,1.,0.),vec3f(0.,0.,-1.),32,lod);
        color=overhead.color;
        surface_premultiplied=overhead.normal_t;
        if blend<1. {
            let left=crown_view(p,toward_eye,half,y0,3u,lod);
            let right=crown_view(p,toward_eye,half,y0+1u,3u,lod);
            color=mix(mix(left.color,right.color,a),color,blend);
            surface_premultiplied=mix(mix(left.normal_t,right.normal_t,a),surface_premultiplied,blend);
        }
    } else if CROWN_ATLAS_TRIANGLE {
        let v10=crown_view(p,toward_eye,half,y0+1u,e0,lod);
        let v01=crown_view(p,toward_eye,half,y0,e0+1u,lod);
        // Continuous barycentric weights across the shared diagonal.
        if a+b<=1. {
            let v00=crown_view(p,toward_eye,half,y0,e0,lod);
            color=v00.color*(1.-a-b)+v10.color*a+v01.color*b;
            surface_premultiplied=v00.normal_t*(1.-a-b)+v10.normal_t*a+v01.normal_t*b;
        } else {
            let v11=crown_view(p,toward_eye,half,y0+1u,e0+1u,lod);
            color=v11.color*(a+b-1.)+v10.color*(1.-b)+v01.color*(1.-a);
            surface_premultiplied=v11.normal_t*(a+b-1.)+v10.normal_t*(1.-b)+v01.normal_t*(1.-a);
        }
    } else {
        let v00=crown_view(p,toward_eye,half,y0,e0,lod);
        let v10=crown_view(p,toward_eye,half,y0+1u,e0,lod);
        let v01=crown_view(p,toward_eye,half,y0,e0+1u,lod);
        let v11=crown_view(p,toward_eye,half,y0+1u,e0+1u,lod);
        color=mix(mix(v00.color,v10.color,a),mix(v01.color,v11.color,a),b);
        surface_premultiplied=mix(mix(v00.normal_t,v10.normal_t,a),mix(v01.normal_t,v11.normal_t,a),b);
    }
    // Far outlines lose their least-covered texels first. A stable root
    // variation spreads the range where the final dense texels vanish.
    let variation=f32((i.seed^(i.seed>>21u))&255u)/255.;
    let cutoff=mix(.38,.29,smoothstep(.38,2.7,lod))+(1.-i.fade)*(.79+.28*variation);
    if color.a<cutoff {discard;}
    let surface=surface_premultiplied/color.a;
    let from_plane=rotate(toward_eye*surface.w*scale,i.turn);
    let world=i.view_face+from_plane;
    let relative=world-camera.eye.xyz;
    if near_shadow.naadf_hi.w>0.5 && all(relative>=near_shadow.naadf_lo.xyz)
        && all(relative<near_shadow.naadf_hi.xyz) {discard;}
    var leaf:Out;
    leaf.local=world;
    leaf.normal=rotate(normalize((surface.xyz+direction*.002)/scale),i.turn);
    leaf.color=color.rgb/color.a;
    leaf.ao=.91;
    leaf.transmission=select(0.,.90,leaf.color.g>leaf.color.r*1.15);
    let clip=camera.view_projection*vec4f(world,1.);
    return CrownFragment(shade_vegetation(leaf),clamp(clip.z/clip.w,0.,1.));
}
