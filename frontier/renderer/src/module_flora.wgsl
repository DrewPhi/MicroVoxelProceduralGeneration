@group(0) @binding(41) var<storage,read> flora_atlas:array<u32>;

struct FloraInstance {
    root:vec4i,
    rectangle:vec4f,
}
struct FloraDraw {
    vertices:u32,
    instances:atomic<u32>,
    first_vertex:u32,
    first_instance:u32,
}
@group(0) @binding(42) var<storage,read_write> flora_write:array<FloraInstance>;
@group(0) @binding(43) var<storage,read> flora_read:array<FloraInstance>;
@group(0) @binding(44) var<storage,read_write> flora_indirect:FloraDraw;
@group(0) @binding(45) var<storage,read> flora_edits:array<vec4i>;

override FLORA_MSAA:bool = false;
override FLORA_TERRAIN_DEPTH:bool = false;

fn flora_rotate(p:vec2f,r:u32) -> vec2f {
    switch r&3u {
        case 1u: {return vec2f(-p.y,p.x);}
        case 2u: {return -p;}
        case 3u: {return vec2f(p.y,-p.x);}
        default: {return p;}
    }
}

fn flora_vertex_bounds(kind:u32,upper:bool) -> vec3i {
    let i=16u*kind+select(8u,12u,upper);
    return vec3i(i32(flora_atlas[i]),i32(flora_atlas[i+1u]),i32(flora_atlas[i+2u]));
}

fn flora_eye_delta(root:vec3i) -> vec3f {
    let eye_y=floor(lattice_view.eye.y*16.);
    return vec3f(
        f32(root.x-lattice_view.origin.x-lattice_view.eye_cell.x)-lattice_view.eye_fraction.x,
        f32(root.y-i32(eye_y))-(lattice_view.eye.y*16.-eye_y),
        f32(root.z-lattice_view.origin.y-lattice_view.eye_cell.y)-lattice_view.eye_fraction.y,
    );
}

// Exact root height and dry/grass admission from the same controls used for
// the terrain ray solver. This function never uses an interpolated display LOD.
fn flora_root_height(root:vec2i) -> vec2i {
    let rel=root-lattice_view.origin.xy;
    let tile=rel>>vec2u(7u);
    let width=i32(lattice_view.config.x);
    if any(tile<vec2i(0)) || any(tile>=vec2i(width)) {return vec2i(0);}
    let pitch=width+1;
    let idx=tile.y*pitch+tile.x;
    let a=lattice_controls[idx];
    let b=lattice_controls[idx+1];
    let c=lattice_controls[idx+pitch];
    let d=lattice_controls[idx+pitch+1];
    let local=rel&vec2i(127);
    let tags=vec4i(a.y,b.y,c.y,d.y);
    let corner=u32(local.x>=64)+2u*u32(local.y>=64);
    if ((tags[corner]>>22u)&15i)!=1i {return vec2i(0);}
    let heights=lattice_control_heights(vec4i(a.x,b.x,c.x,d.x));
    let carrier=lattice_interpolate(heights,local);
    var ground=carrier;
    if a.y<0 {ground+=lattice_detail_top(u32(a.x)>>24u,local,0u);}
    if ((a.y|b.y|c.y|d.y)&(1i<<30u))!=0 {
        let head=lattice_interpolate(heights+((tags<<vec4u(10u))>>vec4u(10u)),local);
        if head>carrier || head>ground {return vec2i(0);}
    }
    return vec2i(ground,1);
}

fn flora_project(root:vec3i,kind:u32,rotation:u32) -> vec4f {
    let lower=vec3f(flora_vertex_bounds(kind,false));
    let upper=vec3f(flora_vertex_bounds(kind,true));
    let a=flora_rotate(lower.xz-.5,rotation)+.5;
    let b=flora_rotate(upper.xz-.5,rotation)+.5;
    let delta=flora_eye_delta(root);
    let lo=(delta+vec3f(min(a.x,b.x),lower.y,min(a.y,b.y)))*.0625;
    let hi=(delta+vec3f(max(a.x,b.x),upper.y,max(a.y,b.y)))*.0625;
    let center=(lo+hi)*.5;
    let half=(hi-lo)*.5;
    let right=view.right.xyz/dot(view.right.xyz,view.right.xyz);
    let up=view.up.xyz/dot(view.up.xyz,view.up.xyz);
    let ahead=dot(center,view.forward.xyz);
    let spread=dot(half,abs(view.forward.xyz));
    let upper_depth=ahead+spread;
    if upper_depth<.025 || length(max(abs(center)-half,vec3f(0.)))>view.eye.w+.001 {
        return vec4f(2.,2.,-2.,-2.);
    }
    let x=dot(center,right);
    let y=dot(center,up);
    let xr=dot(half,abs(right));
    let yr=dot(half,abs(up));
    if abs(x)-xr>upper_depth || abs(y)-yr>upper_depth {return vec4f(2.,2.,-2.,-2.);}
    if ahead-spread<.025 {return vec4f(-1.,-1.,1.,1.);}
    // Interval projection is conservative even when the AABB spans an axis.
    // Its box is evaluated once per site, not per raster corner.
    let z=ahead-spread;
    let xlo=x-xr;
    let ylo=y-yr;
    let xhi=x+xr;
    let yhi=y+yr;
    let lo_ndc=vec2f(xlo/select(z,upper_depth,xlo>0.),ylo/select(z,upper_depth,ylo>0.));
    let hi_ndc=vec2f(xhi/select(z,upper_depth,xhi<0.),yhi/select(z,upper_depth,yhi<0.));
    // Leave an eighth of a pixel for subpixel raster rounding.
    let pad=vec2f(.25)/vec2f(view.size.xy);
    return clamp(vec4f(lo_ndc-pad,hi_ndc+pad),vec4f(-1.),vec4f(1.));
}

@compute @workgroup_size(8,8)
fn flora_place(@builtin(global_invocation_id) id:vec3u) {
    if lattice_view.config.z==0u {return;}
    let radius=i32(ceil(view.eye.w/16.))+1;
    if any(vec2i(id.xy)>vec2i(2*radius)) {return;}
    let center=(lattice_view.origin.xy+lattice_view.eye_cell.xy)>>vec2u(8u);
    let site=center+vec2i(id.xy)-vec2i(radius);
    let seed=u32(lattice_view.origin.z);
    let hash=flora_hash(site,seed^0x7e013e59u);
    let p=site>>vec2u(4u);
    let f=site&vec2i(15);
    let salt=seed^0xf7baa127u;
    let a=i32(flora_hash(p,salt)&255u);
    let b=i32(flora_hash(p+vec2i(1,0),salt)&255u);
    let c=i32(flora_hash(p+vec2i(0,1),salt)&255u);
    let d=i32(flora_hash(p+1,salt)&255u);
    let grove=u32(((a*(16-f.x)+b*f.x)*(16-f.y)+(c*(16-f.x)+d*f.x)*f.y)>>8);
    if (hash>>24u)>=18u+((grove*225u)>>8u) {return;}
    let xz=site*256+vec2i(48)+vec2i(i32(((hash&255u)*160u)>>8u),i32((((hash>>8u)&255u)*160u)>>8u));
    let offset=vec2f(xz-lattice_view.origin.xy-lattice_view.eye_cell.xy)*.0625;
    if dot(offset,offset)>(view.eye.w+9.)*(view.eye.w+9.) {return;}
    let height=flora_root_height(xz);
    if height.y==0 {return;}
    let kind=(hash>>16u)&15u;
    let rotation=(hash>>20u)&3u;
    let root=vec3i(xz.x,height.x,xz.y);
    let rect=flora_project(root,kind,rotation);
    if any(rect.xy>=rect.zw) {return;}
    let index=atomicAdd(&flora_indirect.instances,1u);
    flora_write[index]=FloraInstance(vec4i(root,i32(kind|(rotation<<4u))),rect);
}

struct FloraVertex {
    @builtin(position) position:vec4f,
    @location(0) @interpolate(flat) root:vec4i,
    @location(1) @interpolate(linear,sample) screen:vec2f,
}

@vertex
fn flora_vertex(@builtin(vertex_index) index:u32,@builtin(instance_index) instance:u32) -> FloraVertex {
    let tree=flora_read[instance];
    let corner=array<vec2<bool>,6>(
        vec2<bool>(false,false),vec2<bool>(true,false),vec2<bool>(false,true),
        vec2<bool>(false,true),vec2<bool>(true,false),vec2<bool>(true,true)
    )[index];
    let p=select(tree.rectangle.xy,tree.rectangle.zw,corner);
    return FloraVertex(vec4f(p,0.,1.),tree.root,p);
}

fn flora_address(p:vec3u,dims:vec3u) -> u32 {
    return p.x+dims.x*(p.z+dims.z*p.y);
}

fn flora_edit_at(p:vec3i) -> bool {
    var lo=1i;
    var hi=1i+flora_edits[0].x;
    while lo<hi {
        let mid=(hi+lo)>>1u;
        let q=flora_edits[mid].xyz;
        if all(p==q) {return true;}
        if q.x<p.x || (q.x==p.x && (q.y<p.y || (q.y==p.y && q.z<p.z))) {
            lo=mid+1;
        } else {
            hi=mid;
        }
    }
    return false;
}

fn flora_enter_cell(origin:vec3f,dir:vec3f,inv:vec3f,t:f32) -> vec3i {
    let guess=floor(fma(dir,vec3f(t),origin));
    let a=(guess-origin)*inv;
    let b=(guess+1.-origin)*inv;
    let inc=select(b>vec3f(t),b<=vec3f(t),dir>vec3f(0.)) & (dir!=vec3f(0.));
    let dec=select(a<=vec3f(t),a>vec3f(t),dir>vec3f(0.)) & (dir!=vec3f(0.));
    return vec3i(guess)+vec3i(inc)-vec3i(dec);
}

struct FloraHit {
    t:f32,
    material:u32,
    normal:vec3f,
}

fn flora_trace(root:vec4i,ray:vec3f,near:f32,far:f32) -> FloraHit {
    let kind=u32(root.w)&15u;
    let rot=(u32(root.w)>>4u)&3u;
    let local_rotation=(4u-rot)&3u;
    let delta=flora_eye_delta(root.xyz);
    let offset=flora_rotate(-delta.xz-.5,local_rotation)+.5;
    let origin=vec3f(offset.x,-delta.y,offset.y);
    let horizontal=flora_rotate(ray.xz,local_rotation);
    let dir=vec3f(horizontal.x,ray.y,horizontal.y)*16.;
    let stationary=dir==vec3f(0.);
    let inv=1./select(dir,vec3f(1.),stationary);
    let lower=flora_vertex_bounds(kind,false);
    let upper=flora_vertex_bounds(kind,true);
    let lo=vec3f(lower);
    let hi=vec3f(upper);
    let miss=FloraHit(-1.,0u,vec3f(0.));
    if any(stationary & ((origin<lo)|(origin>=hi))) {return miss;}
    let a=(lo-origin)*inv;
    let b=(hi-origin)*inv;
    let enter=select(min(a,b),vec3f(-INF),stationary);
    let exit=select(max(a,b),vec3f(INF),stationary);
    var t=max(near,max(enter.x,max(enter.y,enter.z)));
    let stop=min(far,min(exit.x,min(exit.y,exit.z)));
    if t>=stop {return miss;}
    let p=16u*kind;
    let base=vec3i(i32(flora_atlas[p]),i32(flora_atlas[p+1u]),i32(flora_atlas[p+2u]));
    let dims=vec3u(flora_atlas[p+4u],flora_atlas[p+5u],flora_atlas[p+6u]);
    let positive=dir>=vec3f(0.);
    let step=select(vec3i(-1),vec3i(1),positive);
    var cell=clamp(flora_enter_cell(origin,dir,inv,t),lower,upper-1);
    var axis=select(select(2u,1u,enter.y>=enter.z),0u,enter.x>=max(enter.y,enter.z));
    for (var i=0u;i<1024u;i++) {
        let local=vec3u(cell-base);
        let tile=flora_atlas[flora_atlas[p+3u]+flora_address(local>>vec3u(4u),dims)];
        var bits=4u;
        var mask=0u;
        if tile!=0u {
            let brick=flora_atlas[tile+flora_address((local>>vec3u(2u))&vec3u(3u),vec3u(4u))];
            bits=2u;
            if brick!=0u {
                bits=0u;
                let index=flora_address(local&vec3u(3u),vec3u(4u));
                mask=(flora_atlas[brick+index/16u]>>(2u*(index&15u)))&3u;
            }
        }
        let extent=1i<<bits;
        let block=base+vec3i((local>>vec3u(bits))<<vec3u(bits));
        let edges=block+select(vec3i(0),vec3i(extent),positive);
        let cross=select((vec3f(edges)-origin)*inv,vec3f(INF),stationary);
        let end=min(cross.x,min(cross.y,cross.z));
        if mask!=0u && end>t {
            // Rotate voxel addresses around the root-cell center. The CPU
            // uses the same integer quarter-turn for collision and picking.
            let hx=vec2i(flora_rotate(vec2f(cell.xz),rot));
            let world=root.xyz+vec3i(hx.x,cell.y,hx.y);
            if !flora_edit_at(world) {
                var surface=vec3f(0.);
                surface[axis]=-f32(step[axis]);
                let normal=flora_rotate(surface.xz,rot);
                return FloraHit(t,mask,vec3f(normal.x,surface.y,normal.y));
            }
        }
        if end>=stop {return miss;}
        let moved=cross<=vec3f(end);
        let estimate=flora_enter_cell(origin,dir,inv,end);
        cell=select(clamp(estimate,block,block+extent-1),
            edges-select(vec3i(1),vec3i(0),positive),moved);
        if any(cell<lower)||any(cell>=upper) {return miss;}
        axis=select(select(2u,1u,moved.y),0u,moved.x);
        t=max(t,end);
    }
    return miss;
}

@fragment
fn flora_fragment(input:FloraVertex) -> RollingFragment {
    let xy=vec2u(input.position.xy);
    let ndc=(2.*input.position.xy-vec2f(view.size.xy))/vec2f(view.size.xy)*vec2f(1.,-1.);
    let screen=select(ndc,input.screen,FLORA_MSAA);
    let direction=view.forward.xyz+view.right.xyz*screen.x+view.up.xyz*screen.y;
    let scale=length(direction);
    let ray=direction/scale;
    var far=view.eye.w;
    if FLORA_TERRAIN_DEPTH && lattice_compute_fits() {
        let ground=lattice_read_hits[xy.y*view.size.x+xy.x];
        let distance=bitcast<f32>(ground.x);
        let material=(ground.y>>4u)&15u;
        if distance>0. && material!=7u && (view.forward.w<.5 || distance<.73*far) {
            far=min(far,distance+.004);
        }
    }
    let hit=flora_trace(input.root,ray,NEAR*scale,far);
    if hit.t<0. {discard;}
    let depth=projected_depth(hit.t/scale,view.eye.w);
    if CAPTURE_DISTANCE {return RollingFragment(vec4f(vec3f(hit.t),1.),depth);}
    let kind=u32(input.root.w)&15u;
    let seed=u32(lattice_view.origin.z);
    let variance=.79+.35*f32(flora_hash(input.root.xz,seed)&255u)/255.;
    let leaves=select(vec3f(.063,.171,.071),vec3f(.145,.255,.073),kind>=6u);
    let bark=select(vec3f(.19,.112,.065),vec3f(.45,.42,.33),kind>=6u && kind<=8u);
    let palette=array<vec3f,3>(bark,leaves,vec3f(.52,.105,.038));
    let light=rolling_light_visible(hit.normal,1.);
    let fade=select(1.,1.-smoothstep(.73,.996,hit.t/view.eye.w),view.forward.w>.5);
    return RollingFragment(vec4f(variance*palette[hit.material-1u]*light,fade),depth);
}
