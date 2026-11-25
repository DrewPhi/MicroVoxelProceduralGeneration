// Independent experiment. See PROCEDURAL_RAYCAST.md for research provenance.
struct View {
    eye: vec4f,
    forward: vec4f,
    right: vec4f,
    up: vec4f,
    grid: vec4f, // origin X/Z, inverse sample pitch, cell count
    size: vec4u, // output width/height, coarsest mip, rows per vertical group
    cache: vec4i, // world sample origin for toroidal mips
    inner: vec4f,
    inner_cache: vec4i,
    old_eye: vec4f,
    old_forward: vec4f,
    old_right: vec4f,
    old_up: vec4f,
}
@group(0) @binding(0) var<uniform> view: View;
@group(0) @binding(1) var maxima: texture_2d<f32>;
@group(0) @binding(2) var samples: texture_2d<f32>;
@group(0) @binding(3) var<storage, read_write> hits: array<vec2<f32>>;
@group(0) @binding(4) var<storage, read_write> colors: array<u32>;
@group(0) @binding(5) var<storage, read_write> counters: array<vec4u>;
@group(0) @binding(6) var inner_maxima: texture_2d<f32>;
@group(0) @binding(7) var inner_samples: texture_2d<f32>;
@group(0) @binding(8) var<storage, read_write> history: array<vec2f>;

override METHOD: u32 = 1u; // restart, boundary ascent, miss count
override BILINEAR: bool = false;
override MINMAX: bool = false;
override INSTRUMENT: bool = false;
override STREAM: bool = false;
override NESTED: bool = false;
override HISTORY: bool = false;
override LEVEL_STEP: u32 = 1u;
const INF: f32 = 1.e20;
const NEAR: f32 = .025;

fn screen_ray(pixel: vec2u) -> vec3f {
    let uv = (vec2f(pixel)+.5)*2./vec2f(view.size.xy)-1.;
    return view.forward.xyz + uv.x*view.right.xyz - uv.y*view.up.xyz;
}
fn row_slope(row: i32) -> f32 {
    return screen_ray(vec2u(0,u32(row))).y;
}
struct Grid {
    extent: vec4f,
    offset: vec2i,
    root: u32,
    fine: bool,
}
fn outer_grid() -> Grid { return Grid(view.grid,view.cache.xy,view.size.z,false); }
fn inner_grid() -> Grid { return Grid(view.inner,view.inner_cache.xy,u32(view.inner_cache.z),true); }
struct Ray {
    origin: vec2f,  // mirrored texture-cell units
    velocity: vec2f,
    inverse: vec2f,
    signs: vec2i,
    lo: f32,
    hi: f32,
    grid: Grid,
}
fn ray_grid(direction: vec2f, grid: Grid, clip_near: f32, clip_far: f32) -> Ray {
    return ray_grid_at((view.eye.xz-grid.extent.xy)*grid.extent.z,direction,grid,clip_near,clip_far);
}
fn ray_grid_at(origin: vec2f, direction: vec2f, grid: Grid, clip_near: f32, clip_far: f32) -> Ray {
    let signs = select(vec2i(1), vec2i(-1), direction < vec2f(0.));
    let start = select(origin,vec2f(grid.extent.w)-origin,signs<vec2i(0));
    let velocity = abs(direction*grid.extent.z);
    let stationary = velocity < vec2f(1.e-12);
    let inverse = select(1./velocity,vec2f(INF),stationary);
    let lower = select(-start*inverse,vec2f(-INF),stationary);
    let upper = select((grid.extent.w-start)*inverse,vec2f(INF),stationary);
    let outside = any(stationary & ((start<vec2f(0.)) | (start>=vec2f(grid.extent.w))));
    return Ray(start,velocity,inverse,signs,max(clip_near,max(lower.x,lower.y)),
               select(min(clip_far,min(upper.x,upper.y)),-1.,outside),grid);
}
fn lookup_cell(ray: Ray, fine: vec2i, mip: u32) -> vec2i {
    let n = i32(ray.grid.extent.w) >> mip;
    let index = fine >> vec2u(mip);
    return select(index,vec2i(n-1)-index,ray.signs<vec2i(0));
}
fn fine_at(ray: Ray, t: f32) -> vec2i {
    let guess=vec2i(floor(fma(vec2f(t),ray.velocity,ray.origin)));
    // Match the same reciprocal-based crossing test used by the DDA on
    // the other axis. Rounded positions can fall into a texel we did not
    // reach, especially for nearly coincident distant corner crossings.
    let still=ray.velocity<vec2f(1.e-12);
    let too_far=(exit_times(ray,guess)>vec2f(t)) & !still;
    let too_early=(exit_times(ray,guess+1)<=vec2f(t)) & !still;
    return guess+select(vec2i(0),vec2i(1),too_early)-select(vec2i(0),vec2i(1),too_far);
}
fn initial_cell(ray: Ray) -> vec2i {
    return clamp(fine_at(ray,ray.lo),vec2i(0),vec2i(i32(ray.grid.extent.w)-1));
}
fn cell_edge(fine: vec2i, mip: u32) -> vec2i {
    return ((fine >> vec2u(mip))+1) << vec2u(mip);
}
fn exit_times(ray: Ray, edge: vec2i) -> vec2f {
    return select((vec2f(edge)-ray.origin)*ray.inverse,vec2f(INF),ray.velocity < vec2f(1.e-12));
}
fn advance_cell(ray: Ray, edge: vec2i, exit: vec2f, mip: u32, next: f32) -> vec2i {
    let lower = edge - (1i<<mip);
    let inside = clamp(fine_at(ray,next),lower,edge-1);
    return select(inside,edge,exit<=vec2f(next));
}
fn entry_face(ray: Ray, exit: vec2f) -> f32 {
    if exit.x <= exit.y { return select(2.,1.,ray.signs.x>0); }
    return select(4.,3.,ray.signs.y>0);
}
fn parent_level(mip: u32, old: vec2i, next: vec2i) -> u32 {
    let shift = mip+LEVEL_STEP;
    return select(mip, min(shift,view.size.z),
                  any((old>>vec2u(shift))!=(next>>vec2u(shift))));
}
fn bounds_at(grid: Grid, cell: vec2i, mip: u32) -> vec2f {
    var address=cell;
    if STREAM && mip<=6u {
        let side=i32(grid.extent.w)>>mip;
        address=(cell+(grid.offset>>vec2u(mip)))&vec2i(side-1);
    }
    if NESTED && grid.fine { return textureLoad(inner_maxima,address,i32(mip)).rg; }
    return textureLoad(maxima,address,i32(mip)).rg;
}
fn raw_sample(grid: Grid, cell: vec2i) -> f32 {
    var address=cell;
    if STREAM {
        let side=i32(grid.extent.w);
        address=select((cell+grid.offset)&vec2i(side-1),cell,cell==vec2i(side));
    }
    if NESTED && grid.fine { return textureLoad(inner_samples,address,0).r; }
    return textureLoad(samples,address,0).r;
}
fn parent_height(world: vec2f) -> f32 {
    let outer=outer_grid();
    let p=(world-outer.extent.xy)*outer.extent.z;
    let cell=clamp(vec2i(floor(p)),vec2i(0),vec2i(i32(outer.extent.w)-1));
    let uv=clamp(p-vec2f(cell),vec2f(0.),vec2f(1.));
    return mix(mix(raw_sample(outer,cell),raw_sample(outer,cell+vec2i(1,0)),uv.x),
               mix(raw_sample(outer,cell+vec2i(0,1)),raw_sample(outer,cell+1),uv.x),uv.y);
}
fn fade_limits(grid: Grid) -> vec2f {
    // Origin snaps in 64-sample increments. The blend is already entirely
    // parent terrain before reaching either edge, at every sub-strip pose.
    var end=(grid.extent.w*.5-66.)/grid.extent.z;
    if view.inner_cache.w != 0 {
        let offset=(view.eye.xz-grid.extent.xy)*grid.extent.z;
        let border=min(offset,grid.extent.w-offset);
        end=max(0.,min(end,(min(border.x,border.y)-2.)/grid.extent.z));
    }
    return vec2f(.55*end,end);
}
fn sample_at(grid: Grid, cell: vec2i) -> f32 {
    let h=raw_sample(grid,cell);
    if NESTED && grid.fine {
        let world=grid.extent.xy+vec2f(cell)/grid.extent.z;
        let delta=abs(world-view.eye.xz);
        let distance=max(delta.x,delta.y);
        let limits=fade_limits(grid);
        if distance>limits.x {
            return mix(h,parent_height(world),smoothstep(limits.x,limits.y,distance));
        }
    }
    return h;
}
fn terrain_corners(grid: Grid, cell: vec2i) -> vec4f {
    return vec4f(sample_at(grid,cell),sample_at(grid,cell+vec2i(1,0)),
                 sample_at(grid,cell+vec2i(0,1)),sample_at(grid,cell+1));
}
fn terrain_bounds(grid: Grid, cell: vec2i, mip: u32, stats: ptr<function,vec4u>) -> vec2f {
    let native=bounds_at(grid,cell,mip);
    if INSTRUMENT {(*stats).x+=1u;}
    if NESTED && grid.fine {
        let size=f32(1u<<mip)/grid.extent.z;
        let lo=grid.extent.xy+vec2f(cell)*size;
        let hi=lo+size;
        let delta=max(abs(lo-view.eye.xz),abs(hi-view.eye.xz));
        if max(delta.x,delta.y)>fade_limits(grid).x {
            let outer=outer_grid();
            // Fine pitch is 1/16 of parent pitch. The parent rectangles at
            // this mip are at least as wide as the projected fine cell.
            let pmip=max(mip,4u)-4u;
            let scale=outer.extent.z/f32(1u<<pmip);
            let last=vec2i((i32(outer.extent.w)>>pmip)-1);
            let a=clamp(vec2i(floor((lo-outer.extent.xy)*scale)),vec2i(0),last);
            let b=clamp(vec2i(floor((hi-outer.extent.xy)*scale)),vec2i(0),last);
            let top=max(max(bounds_at(outer,a,pmip).x,bounds_at(outer,b,pmip).x),
                        max(bounds_at(outer,vec2i(a.x,b.y),pmip).x,bounds_at(outer,vec2i(b.x,a.y),pmip).x));
            if INSTRUMENT {(*stats).x+=4u;}
            return vec2f(max(top,native.x),0.);
        }
    }
    return native;
}
fn bilinear_curve(ray: Ray, fine: vec2i, lo: f32, hi: f32) -> vec3f {
    let h = terrain_corners(ray.grid,lookup_cell(ray,fine,0u));
    let mirror = clamp(fma(vec2f(lo),ray.velocity,ray.origin)-vec2f(fine),vec2f(0.),vec2f(1.));
    let uv = select(mirror,1.-mirror,ray.signs<vec2i(0));
    let delta = vec2f(ray.signs)*ray.velocity*(hi-lo);
    let dx = h.y-h.x;
    let dz = h.z-h.x;
    let cross = (h.w-h.z)-dx;
    let base=mix(mix(h.x,h.y,uv.x),mix(h.z,h.w,uv.x),uv.y);
    let linear=dot(delta,vec2f(dx+cross*uv.y,dz+cross*uv.x));
    return vec3f(base,linear,cross*delta.x*delta.y);
}
fn bilinear_hit(curve: vec3f, slope: f32, lo: f32, hi: f32) -> f32 {
    // Parameter is the fraction of this horizontal texel interval.
    let c=fma(lo,slope,view.eye.y)-curve.x;
    if c<=0. {return lo;}
    let b=slope*(hi-lo)-curve.y;
    let a=-curve.z;
    if abs(a) < 1.e-9 {
        if b<0. && c+b<=0. {return lo+(hi-lo)*(-c/b);}
        return -1.;
    }
    let discriminant = fma(b,b,-4.*a*c);
    if discriminant < 0. {return -1.;}
    let q = -.5*(b+select(-1.,1.,b>=0.)*sqrt(discriminant));
    if q == 0. {return -1.;}
    let roots = vec2f(q/a,c/q);
    let usable = select(vec2f(INF),roots,(roots>=vec2f(0.)) & (roots<=vec2f(1.)));
    let root = min(usable.x,usable.y);
    return select(-1.,fma(hi-lo,root,lo),root<INF);
}
fn column_hit(height: f32, slope: f32, lo: f32, hi: f32, face: f32) -> vec2f {
    if fma(lo,slope,view.eye.y)<=height { return vec2f(lo,face); }
    if slope<0. && fma(hi,slope,view.eye.y)<=height {
        return vec2f(clamp((height-view.eye.y)/slope,lo,hi),0.);
    }
    return vec2f(-1.,0.);
}
fn trace(direction: vec3f, ray: Ray, stats: ptr<function,vec4u>) -> vec2f {
    let root=ray.grid.root;
    if ray.lo>=ray.hi {return vec2f(-1.,0.);}
    var fine = initial_cell(ray);
    var t = ray.lo;
    var entry = 0.;
    var mip = root;
    var misses = 0u;
    // Even a full leaf walk visits at most 2*N cells. Reserve a conservative
    // allowance for all ascents and descents, with a reported failure sentinel.
    for (var step=0u;step<u32(ray.grid.extent.w)*8u+64u;step+=1u) {
        let edge = cell_edge(fine,mip);
        let exit = exit_times(ray,edge);
        let end = max(t,min(ray.hi,min(exit.x,exit.y)));
        let bounds=terrain_bounds(ray.grid,lookup_cell(ray,fine,mip),mip,stats);
        let y = min(fma(t,direction.y,view.eye.y),fma(end,direction.y,view.eye.y));
        if end>t && y<=bounds.x {
            if MINMAX && !BILINEAR && fma(t,direction.y,view.eye.y)<=bounds.y {
                return vec2f(t,entry);
            }
            if mip>0u {
                let lower=edge-(1i<<mip);
                mip-=min(mip,LEVEL_STEP);
                misses=0u;
                if METHOD==1u && direction.y<0. {
                    let later=clamp((bounds.x+0.001-view.eye.y)/direction.y,t,end);
                    if later>t && later<end {
                        fine=clamp(fine_at(ray,later),lower,edge-1);
                        t=later;
                        entry=0.;
                    }
                }
                continue;
            }
            if INSTRUMENT { (*stats).y+=1u; }
            if BILINEAR {
                let depth=bilinear_hit(bilinear_curve(ray,fine,t,end),direction.y,t,end);
                if depth>=0. {return vec2f(depth,0.);}
            } else {
                return column_hit(bounds.x,direction.y,t,end,entry);
            }
        }
        if end>=ray.hi {return vec2f(-1.,0.);}
        let next=advance_cell(ray,edge,exit,mip,end);
        if any(next>=vec2i(i32(ray.grid.extent.w))) {return vec2f(-1.,0.);}
        if METHOD==0u { mip=root; }
        if METHOD==1u { mip=min(root,parent_level(mip,fine,next)); }
        if METHOD==2u {
            misses+=1u;
            if misses>=select(8u,16u,mip==0u) {
                mip=min(root,mip+LEVEL_STEP);
                misses=0u;
            }
        }
        fine=next;
        t=end;
        entry=entry_face(ray,exit);
    }
    return vec2f(-2.,0.);
}

fn previous_ray(pixel: vec2u) -> vec3f {
    let uv=(vec2f(pixel)+.5)*2./vec2f(view.size.xy)-1.;
    return view.old_forward.xyz+uv.x*view.old_right.xyz-uv.y*view.old_up.xyz;
}
fn history_limit(pixel: vec2u, direction: vec3f, counts: ptr<function,vec4u>) -> f32 {
    let far=view.eye.w;
    if view.old_eye.w==0. {return far;}
    let guess=history[pixel.x+pixel.y*view.size.x].x;
    if guess<NEAR {return far;}
    let current=view.eye.xyz+guess*direction;
    let relative=current-view.old_eye.xyz;
    let z=dot(relative,view.old_forward.xyz);
    if z<=NEAR {return far;}
    let uv=vec2f(dot(relative,view.old_right.xyz)/dot(view.old_right.xyz,view.old_right.xyz),
                 -dot(relative,view.old_up.xyz)/dot(view.old_up.xyz,view.old_up.xyz))/z;
    if any(abs(uv)>=vec2f(1.)) {return far;}
    let p=vec2u((uv*.5+.5)*vec2f(view.size.xy));
    let old_t=history[p.x+p.y*view.size.x].x;
    if old_t<NEAR {return far;}
    let old_world=view.old_eye.xyz+old_t*previous_ray(p);
    let projected=dot(old_world-view.eye.xyz,direction)/dot(direction,direction);
    let t=projected+max(.03,projected*.00015);
    if t<=NEAR || t>=far-.05 {return far;}
    let world=fma(vec3f(t),direction,view.eye.xyz);
    var grid=outer_grid();
    if NESTED {
        let g=(world.xz-view.inner.xy)*view.inner.z;
        if all(g>=vec2f(0.)) && all(g<vec2f(view.inner.w)) {grid=inner_grid();}
    }
    let g=(world.xz-grid.extent.xy)*grid.extent.z;
    if any(g<vec2f(0.)) || any(g>=vec2f(grid.extent.w)) {return far;}
    let cell=vec2i(floor(g));
    let frac=g-vec2f(cell);
    let h=terrain_corners(grid,cell);
    let roof=mix(mix(h.x,h.y,frac.x),mix(h.z,h.w,frac.x),frac.y);
    if INSTRUMENT {(*counts).w+=1u;}
    // Use the validated solid point solely as an upper bound. The current
    // traversal still starts at the near plane and finds the first crossing.
    if world.y+0.002<roof {
        if INSTRUMENT {(*counts).z+=1u;}
        return t+.05;
    }
    return far;
}
@compute @workgroup_size(8,8)
fn save_history(@builtin(global_invocation_id) id:vec3u) {
    if any(id.xy>=view.size.xy) {return;}
    let index=id.x+id.y*view.size.x;
    history[index]=hits[index];
}

@compute @workgroup_size(8,8)
fn independent(@builtin(global_invocation_id) id:vec3u) {
    if any(id.xy>=view.size.xy) {return;}
    let index=id.x+id.y*view.size.x;
    var counts=vec4u(0);
    let direction=screen_ray(id.xy);
    var near=NEAR;
    var far=view.eye.w;
    if HISTORY {far=history_limit(id.xy,direction,&counts);}
    var hit=vec2f(-1.,0.);
    if NESTED {
        let ray=ray_grid(direction.xz,inner_grid(),NEAR,far);
        hit=trace(direction,ray,&counts);
        if hit.x>=0. { hit.y=5.; }
        if ray.lo<ray.hi { near=ray.hi; }
    }
    if hit.x==-1. {hit=trace(direction,ray_grid(direction.xz,outer_grid(),near,far),&counts);}
    hits[index]=hit;
    if INSTRUMENT { counters[index]=counts; }
}

// Upright level camera. Row slopes are ordered. The bottom still-unseen row
// is the easiest to hit, so its failure safely rejects a node for the group.
@compute @workgroup_size(64)
fn vertical(@builtin(global_invocation_id) id:vec3u) {
    let groups=(view.size.y+view.size.w-1u)/view.size.w;
    if id.x>=view.size.x*groups {return;}
    let x=id.x%view.size.x;
    let top=i32(id.x/view.size.x*view.size.w);
    var row=min(top+i32(view.size.w),i32(view.size.y))-1;
    let direction=screen_ray(vec2u(x,0));
    let ray=ray_grid(direction.xz,outer_grid(),NEAR,view.eye.w);
    var counts=vec4u(0);
    var status=-1.;
    if ray.lo<ray.hi {
        var fine=initial_cell(ray);
        var t=ray.lo;
        var entry=0.;
        var mip=view.size.z;
        let budget=u32(view.grid.w)*8u+64u;
        for (var step=0u; step<budget;step+=1u) {
            let edge=cell_edge(fine,mip);
            let exit=exit_times(ray,edge);
            let end=max(t,min(ray.hi,min(exit.x,exit.y)));
            let height=bounds_at(ray.grid,lookup_cell(ray,fine,mip),mip).x;
            if INSTRUMENT {counts.x+=1u;}
            let slope=row_slope(row);
            if end>t && min(fma(t,slope,view.eye.y),fma(end,slope,view.eye.y))<=height {
                if mip>0u {
                    mip-=1u;
                    continue;
                }
                if INSTRUMENT { counts.y+=1u; }
                var curve=vec3f(0.);
                if BILINEAR {curve=bilinear_curve(ray,fine,t,end);}
                loop {
                    let slope=row_slope(row);
                    var hit=vec2f(-1.,0.);
                    if BILINEAR { hit.x=bilinear_hit(curve,slope,t,end); }
                    else { hit=column_hit(height,slope,t,end,entry); }
                    if hit.x<0. {break;}
                    hits[u32(row)*view.size.x+x]=hit;
                    row-=1;
                    if row<top {break;}
                }
                if row<top {break;}
            }
            if end>=ray.hi {break;}
            let next=advance_cell(ray,edge,exit,mip,end);
            if any(next>=vec2i(i32(view.grid.w))) {break;}
            mip=parent_level(mip,fine,next);
            fine=next;
            t=end;
            entry=entry_face(ray,exit);
            if step+1u==budget { status=-2.; }
        }
    }
    for (;row>=top;row-=1) {
        hits[u32(row)*view.size.x+x]=vec2f(status,0.);
    }
    if INSTRUMENT {counters[id.x]=counts;}
}

fn terrain_normal(hit: vec2f, p: vec3f) -> vec3f {
    let faces=array<vec3f,5>(vec3f(0,1,0),vec3f(-1,0,0),vec3f(1,0,0),vec3f(0,0,-1),vec3f(0,0,1));
    if !BILINEAR {return faces[u32(hit.y)];}
    var grid=outer_grid();
    if NESTED && hit.y==5. { grid=inner_grid(); }
    let g=(p.xz-grid.extent.xy)*grid.extent.z;
    let cell=clamp(vec2i(floor(g)),vec2i(0),vec2i(i32(grid.extent.w)-1));
    let uv=clamp(g-vec2f(cell),vec2f(0.),vec2f(1.));
    let h=terrain_corners(grid,cell);
    let dx=mix(h.y-h.x,h.w-h.z,uv.y)*grid.extent.z;
    let dz=mix(h.z-h.x,h.w-h.y,uv.x)*grid.extent.z;
    return normalize(vec3f(-dx,1.,-dz));
}
@compute @workgroup_size(8,8)
fn shade_image(@builtin(global_invocation_id) id:vec3u) {
    if any(id.xy>=view.size.xy) {return;}
    let index=id.x+id.y*view.size.x;
    let hit=hits[index];
    let d=screen_ray(id.xy);
    var rgb=mix(vec3f(.59,.70,.83),vec3f(.23,.44,.73),smoothstep(0.,.65,max(d.y,0.)));
    if hit.x>=0. {
        let p=fma(vec3f(hit.x),d,view.eye.xyz);
        let n=terrain_normal(hit,p);
        let slope=length(n.xz)/max(.025,n.y);
        let rock=smoothstep(.52,.77,slope);
        let snow=smoothstep(1160.,1320.,p.y)*(1.-smoothstep(.6,1.4,slope));
        var material=mix(vec3f(.18,.34,.10),vec3f(.35,.37,.39),rock);
        material=mix(material,vec3f(.74,.78,.83),snow);
        if p.z>-550. && p.z<1500. {
            // Filter only the authored trail's appearance across the pixel
            // footprint. Visibility remains a single primary sample.
            let trail=16.+60.*sin(p.z/310.)+.055*p.z;
            let width=clamp(hit.x*1.1547/f32(view.size.y),.025,4.);
            let core=1.-smoothstep(max(0.,.55-.5*width),.55+.5*width,abs(p.x-trail));
            material=mix(material,vec3f(.32,.22,.13),core);
        }
        let sun=normalize(vec3f(-.54,.66,-.52));
        let light=.39+.16*max(n.y,0.)+.52*max(dot(n,sun),0.);
        rgb=material*light;
    }
    if hit.x==-2. {rgb=vec3f(1,0,1);}
    let srgb=select(12.92*rgb,1.055*pow(rgb,vec3f(1./2.4))-.055,rgb>vec3f(.0031308));
    colors[index]=pack4x8unorm(vec4f(srgb,1.));
}
