struct Camera { view_projection:mat4x4f, eye:vec4f, paint_origin:vec4u };
struct CanopyGround { coverage:vec4f, sun:vec4f };
@group(0) @binding(0) var<uniform> camera:Camera;
@group(1) @binding(1) var<storage,read> crown:array<u32>;
@group(1) @binding(2) var crown_pigment:texture_2d_array<f32>;
@group(1) @binding(4) var crown_sampler:sampler;
@group(2) @binding(0) var<uniform> ground:CanopyGround;

fn turn_point(p:vec3f,turn:u32)->vec3f {
    switch turn&3u {
        case 1u: {return vec3f(-p.z,p.y,p.x);}
        case 2u: {return vec3f(-p.x,p.y,-p.z);}
        case 3u: {return vec3f(p.z,p.y,-p.x);}
        default: {return p;}
    }
}

struct In {
    @location(3) origin:vec3f,
    @location(4) orientation:u32,
    @location(5) fade:f32,
}
struct Out {
    @builtin(position) position:vec4f,
    @location(0) local:vec3f,
    @location(1) @interpolate(flat) view_mix:vec4f,
    @location(2) @interpolate(flat) fade:f32,
}

fn crown_half()->vec3f {
    return vec3f(f32(crown[4]),f32(crown[5]),f32(crown[6])) * (.5*f32(crown[3])*NATIVE_VOXEL_METERS);
}

@vertex fn vs_main(input:In,@builtin(vertex_index) vertex:u32)->Out {
    let half=crown_half();
    let step=f32(crown[3])*NATIVE_VOXEL_METERS;
    let low=vec3f(f32(bitcast<i32>(crown[0])),f32(bitcast<i32>(crown[1])),f32(bitcast<i32>(crown[2]))) * step;
    let center=low+half;
    let turn=input.orientation;
    let inv=4u-(turn&3u);
    let packed=turn>>8u;
    let scale=vec3f(1.,select(1.,f32(packed)/16384.,packed!=0u),1.);
    let pivot=vec3f(.5,0.,.5)*NATIVE_VOXEL_METERS;
    let root=input.origin-camera.eye.xyz;
    let center_world=root+pivot+turn_point(center*scale-pivot,turn);
    let sunlight=ground.sun.xyz;
    let shear=sunlight.xz/max(sunlight.y,.18);
    let center_floor=center_world.xz-(center_world.y-root.y)*shear;
    let ext=abs(turn_point(half*scale,turn));
    let radius=ext.xz+abs(shear)*ext.y+vec2f(1.2);
    let x=select(-1.,1.,vertex==1u||vertex==2u||vertex==4u);
    let y=select(-1.,1.,vertex==2u||vertex==4u||vertex==5u);
    let ground_point=center_floor+vec2f(x,y)*radius;
    let ray_start=vec3f(ground_point.x,root.y,ground_point.y);
    let local=(pivot+turn_point(ray_start-root-pivot,inv))/scale-center;

    let local_sun=normalize(turn_point(sunlight,inv)/scale);
    let yaw=atan2(local_sun.z,local_sun.x)/CROWN_YAW_STEP;
    let pitch=asin(clamp(local_sun.y,-1.,1.));
    let y0=u32(i32(floor(yaw))+8)&7u;
    let e0=select(select(0u,1u,pitch>CROWN_ELEVATION[1]),2u,pitch>CROWN_ELEVATION[2]);
    let elevation=clamp((pitch-CROWN_ELEVATION[e0])/(CROWN_ELEVATION[e0+1u]-CROWN_ELEVATION[e0]),0.,1.);
    let projected=(ground_point-ground.coverage.xy)*(ground.coverage.z*vec2f(1.,-1.));
    let plane=local-local_sun*dot(local,local_sun);
    return Out(vec4f(projected,0.,1.),plane,vec4f(f32(y0),f32(e0),fract(yaw),elevation),input.fade);
}

fn sample_crown(point:vec3f,half:vec3f,azimuth:u32,elevation:u32)->f32 {
    let yaw=CROWN_AZIMUTH_AXES[azimuth&7u];
    let pitch=CROWN_ELEVATION_AXES[elevation];
    let direction=vec3f(yaw.x*pitch.x,pitch.y,yaw.y*pitch.x);
    let right=vec3f(yaw.y,0.,-yaw.x);
    let up=cross(direction,right);
    let bounds=vec2f(dot(abs(right),half),dot(abs(up),half));
    let uv=.5+vec2f(dot(point,right),-dot(point,up))/bounds*vec2f(.46875,.482142857);
    if any(uv<=vec2f(0.))||any(uv>=vec2f(1.)) {return 0.;}
    let index=i32((azimuth&7u)+8u*elevation);
    return textureSampleLevel(crown_pigment,crown_sampler,uv,index,1.2).a;
}

@fragment fn fs_main(input:Out)->@location(0) f32 {
    let half=crown_half();
    let y=u32(input.view_mix.x);
    let e=u32(input.view_mix.y);
    let low=mix(sample_crown(input.local,half,y,e),sample_crown(input.local,half,y+1u,e),input.view_mix.z);
    let high=mix(sample_crown(input.local,half,y,e+1u),sample_crown(input.local,half,y+1u,e+1u),input.view_mix.z);
    return smoothstep(.13,.72,mix(low,high,input.view_mix.w))*input.fade;
}
