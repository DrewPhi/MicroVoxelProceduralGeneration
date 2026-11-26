// Sampled display geometry from the same branch recipe as native voxels.
// These grids are independent instances. Rotation only remaps integer axes.
@group(3) @binding(1) var<storage,read> crown_words:array<u32>;

struct CrownBounds {
    @builtin(position) clip:vec4f,
    @location(0) view_face:vec3f,
    @location(1) @interpolate(flat) root:vec3f,
    @location(2) @interpolate(flat) turn:u32,
    @location(3) @interpolate(flat) fade:f32,
    @location(4) @interpolate(flat) seed:u32,
}
struct CrownFragment {
    @location(0) color:vec4f,
    @builtin(frag_depth) depth:f32,
}
fn crown_pitch()->f32 {
    return f32(crown_words[3])*NATIVE_VOXEL_METERS;
}
fn crown_min()->vec3f {
    return vec3f(bitcast<vec3i>(vec3u(crown_words[0],crown_words[1],crown_words[2])));
}
fn crown_size()->vec3u {
    return vec3u(crown_words[4],crown_words[5],crown_words[6]);
}
fn crown_pivot()->vec3f {
    return vec3f(.5,0.,.5)*NATIVE_VOXEL_METERS;
}
@vertex fn vs_crown_volume(
    @builtin(vertex_index) vertex:u32,
    @location(3) root:vec3f, @location(4) turn:u32, @location(5) fade:f32, @location(6) seed:u32,
)->CrownBounds {
    let face=vertex/6u;
    let axis=face/2u;
    let corner=triangle_corner(face,vertex);
    var p=vec3f(0.);
    p[axis]=f32(face&1u);
    p[(axis+1u)%3u]=select(0.,1.,corner==1u||corner==2u);
    p[(axis+2u)%3u]=select(0.,1.,corner>=2u);
    var out:CrownBounds;
    let local=(crown_min()+p*vec3f(crown_size()))*crown_pitch();
    let pivot=crown_pivot();
    out.view_face=root+pivot+rotate(local*vec3f(1.,plant_height_scale(turn),1.)-pivot,turn);
    out.clip=camera.view_projection*vec4f(out.view_face,1.);
    out.root=root;
    out.turn=turn;
    out.fade=fade;
    out.seed=seed;
    return out;
}
fn crossing_axis(v:vec3f)->u32 {
    if v.x<=v.y && v.x<=v.z {return 0u;}
    return select(2u,1u,v.y<=v.z);
}
@fragment fn fs_crown_volume(i:CrownBounds,@builtin(front_facing) front:bool)->CrownFragment {
    let world_ray=normalize(i.view_face-camera.eye.xyz);
    if i.fade<1. {
        // Pixel coverage belongs to stable world-aligned crowns. Using the
        // final hit for this decision would jitter as other trees occlude it.
        let p=bitcast<vec3u>(vec3i(floor(i.view_face*VOXELS_PER_METER)))+camera.paint_origin.xyz;
        var h=p.x*1597334677u ^ p.y*3812015801u ^ p.z*2798796415u ^ i.seed;
        h=(h^(h>>16u))*2246822519u;
        if f32(h&65535u)/65536.>=i.fade {discard;}
    }
    let pitch=crown_pitch();
    let pivot=crown_pivot();
    let origin=i.root+pivot;
    let inverse=4u-(i.turn&3u);
    let scale=vec3f(1.,plant_height_scale(i.turn),1.);
    let low=crown_min();
    let eye=((rotate(camera.eye.xyz-origin,inverse)+pivot)/scale)/pitch-low;
    let d=rotate(world_ray,inverse)/scale;
    let size=crown_size();
    let blocks=(size+3u)/4u;
    let nonzero=abs(d)>vec3f(1.e-9);
    let rcp=1./select(vec3f(1.),d,nonzero);
    if any((!nonzero)&((eye<vec3f(0.))|(eye>=vec3f(size)))) {discard;}
    let a=-eye*rcp;
    let b=(vec3f(size)-eye)*rcp;
    let entry=select(vec3f(-1e25),min(a,b),nonzero);
    let leave=select(vec3f(1e25),max(a,b),nonzero);
    let at=max(entry.x,max(entry.y,entry.z));
    let end=min(leave.x,min(leave.y,leave.z));
    let near=.025/(max((camera.view_projection*vec4f(world_ray,0.)).w,1.e-5)*pitch);
    var t=max(at,near);
    if t>=end || front==(at<near) {discard;}
    var axis=crossing_axis(-entry);
    let advance=vec3i(select(vec3f(-1.),vec3f(1.),d>vec3f(0.)));
    var cell=clamp(vec3i(floor(eye+d*(t+0.0002))),vec3i(0),vec3i(size)-1);
    let owner=near_shadow.naadf_hi.w>0.5;
    let budget=size.x+size.y+size.z+3u;
    for(var n=0u;n<budget;n++) {
        if any(cell<vec3i(0)) || any(vec3u(cell)>=size) || t>=end {break;}
        let p=vec3u(cell);
        let q=p/4u;
        let address=crown_words[8u+q.x+blocks.x*(q.y+blocks.y*q.z)];
        var paint=0u;
        if address!=0u {
            let local=p%4u;
            paint=crown_words[address+local.x+4u*(local.y+4u*local.z)];
        }
        if paint!=0u {
            let local=(eye+d*t+low)*pitch;
            let world=origin+rotate(local*scale-pivot,i.turn);
            let relative=world-camera.eye.xyz;
            if !owner || any(relative<near_shadow.naadf_lo.xyz) || any(relative>=near_shadow.naadf_hi.xyz) {
                var normal=vec3f(0.);
                normal[axis]=-f32(advance[axis]);
                var leaf:Out;
                leaf.local=world;
                leaf.normal=rotate(normal,i.turn);
                leaf.color=vec3f(vec3u(paint,paint>>10u,paint>>20u)&vec3u(1023u))/1023.;
                leaf.ao=.93;
                leaf.transmission=f32((paint>>30u)&1u);
                let clip=camera.view_projection*vec4f(world,1.);
                return CrownFragment(shade_vegetation(leaf),clamp(clip.z/clip.w,0.,1.));
            }
        }
        let tile=4*vec3i(q);
        let lo=select(cell,tile,address==0u);
        let hi=select(cell+1,tile+4,address==0u);
        let border=select(lo,hi,d>vec3f(0.));
        let next=select(vec3f(1.e25),(vec3f(border)-eye)*rcp,nonzero);
        axis=crossing_axis(next);
        t=next[axis];
        let bounds=cell;
        cell=clamp(vec3i(floor(eye+d*t)),lo,hi-1);
        // Handle tied crossings as one voxel-plane event. Clamp the other
        // axes locally before crossing to avoid roundoff into a prior cell.
        for(var k=0u;k<3u;k++) {
            if next[k]==t {cell[k]=select(lo[k]-1,hi[k],advance[k]>0);}
        }
        if all(cell==bounds) {break;}
    }
    discard;
}
