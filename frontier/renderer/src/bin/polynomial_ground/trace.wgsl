// The generator has one world-aligned lattice of bilinear patches. Every
// returned intersection is against a native column of that single source.
struct View {
    eye: vec4f,
    forward: vec4f,
    right: vec4f,
    up: vec4f,
    size: vec4u,
    origin: vec4i,
}
@group(0) @binding(0) var<uniform> view: View;
@group(0) @binding(1) var<storage,read> vertices: array<vec2i>;
@group(0) @binding(2) var<storage,read> roofs: array<i32>;
@group(0) @binding(3) var<storage,read_write> pixels: array<vec2u>;
@group(0) @binding(4) var<storage,read_write> failures: atomic<u32>;
@group(0) @binding(5) var<storage,read_write> prefix: array<f32>;

override RELIEF: bool = true;
override LEVEL_STEP: u32 = 1u;
override SOLVE: bool = true;
override SIBLING: bool = true;
override BEAMS: bool = false;
override EARLY_RELIEF: bool = true;
override GROUP_WIDTH: u32 = 8u;
override GROUP_HEIGHT: u32 = 8u;
override WIDTH: i32 = 128i;
override INTERPOLANT_BITS: u32 = 8u;
const INF = 1e20;
const SKY = -1.;
const BAD = -2.;

struct Ray {
    origin: vec2f,
    velocity: vec2f,
    inverse: vec2f,
    sign: vec2i,
    limit: i32,
}

fn crossings(ray: Ray, edge: vec2i) -> vec2f {
    return select((vec2f(edge) - ray.origin) * ray.inverse, vec2f(INF), ray.velocity == vec2f(0.));
}

fn cell_at(ray: Ray, t: f32) -> vec2i {
    let estimate = vec2i(floor(fma(vec2f(t), ray.velocity, ray.origin)));
    let before = (crossings(ray, estimate) > vec2f(t)) & (ray.velocity > vec2f(0.));
    let after = crossings(ray, estimate + 1) <= vec2f(t);
    return estimate + select(vec2i(0),vec2i(1),after) - select(vec2i(0),vec2i(1),before);
}

fn advance(ray: Ray, edge: vec2i, exits: vec2f, t: f32) -> vec2i {
    return select(min(cell_at(ray,t),edge-1),edge,exits<=vec2f(t));
}

fn increment(cell:vec2i, exits:vec2f, t:f32) -> vec2i {
    return cell+select(vec2i(0),vec2i(1),exits<=vec2f(t));
}

fn face_at(ray: Ray, exits: vec2f) -> u32 {
    if exits.x <= exits.y {
        return select(2u, 1u, ray.sign.x > 0);
    }
    return select(4u, 3u, ray.sign.y > 0);
}

fn real_cell(ray: Ray, cell: vec2i) -> vec2i {
    return select(vec2i(ray.limit-1)-cell,cell,ray.sign>vec2i(0));
}

fn hash(x: i32, z: i32, seed: u32) -> i32 {
    var v = u32(x) * 0x9e3779b9u ^ u32(z) * 0x85ebca6bu ^ seed;
    v = (v ^ (v >> 16u)) * 0x7feb352du;
    v = (v ^ (v >> 15u)) * 0x846ca68bu;
    return i32((v ^ (v >> 16u)) & 255u) - 128;
}

fn relief_band(cell: vec2i, power: u32, seed: u32) -> i32 {
    let base = cell >> vec2u(power);
    let width = 1i << power;
    let uv = cell & vec2i(width-1);
    let a = hash(base.x,base.y,seed) * (width-uv.x) + hash(base.x+1,base.y,seed)*uv.x;
    let b = hash(base.x,base.y+1,seed) * (width-uv.x) + hash(base.x+1,base.y+1,seed)*uv.x;
    return (a*(width-uv.y)+b*uv.y) >> (2u*power);
}

fn exact_height(corners: vec4i, local: vec2i, world: vec2i, entry_y:f32, minimum_y:f32) -> f32 {
    let p = 2*local+1;
    let denominator=2*WIDTH;
    let a = corners.x*(denominator-p.x)+corners.y*p.x;
    let b = corners.z*(denominator-p.x)+corners.w*p.x;
    let integral = (a>>INTERPOLANT_BITS)*(denominator-p.y)+(b>>INTERPOLANT_BITS)*p.y;
    let mask=denominator-1;
    let residual = ((a&mask)*(denominator-p.y)+(b&mask)*p.y)>>INTERPOLANT_BITS;
    var value = (integral+residual)>>INTERPOLANT_BITS;
    if RELIEF {
        if EARLY_RELIEF {
            let low=f32(value-4)*.0625;
            let high=f32(value+3)*.0625;
            // These return values can only yield a certified entry-side hit
            // or a miss. A potential top intersection uses the exact height.
            if entry_y<=low {return low;}
            if minimum_y>high {return high;}
        }
        let seed = u32(view.origin.z);
        let coarse=3*relief_band(world,4u,seed);
        if EARLY_RELIEF {
            let low=f32(value+((coarse-128)>>7u))*.0625;
            let high=f32(value+((coarse+127)>>7u))*.0625;
            if entry_y<=low {return low;}
            if minimum_y>high {return high;}
            if low==high {return low;}
        }
        value += (coarse+relief_band(world,2u,seed^0x2d1189a1u))>>7u;
    }
    return f32(value)*.0625;
}

fn root_hit(curve:vec3f, y:f32, slope:f32, length:f32) -> f32 {
    let c = y-curve.x;
    if c<=0. {return 0.;}
    let b = slope*length-curve.y;
    let a = -curve.z;
    if a<=0. && c+b+a>0. {return -1.;}
    if a>=0. && b>=0. {return -1.;}
    if abs(a)<1e-7 {
        if b<0. && (c+b)<=0. {return clamp(-c/b,0.,1.);}
        return -1.;
    }
    let determinant = fma(b,b,-4.*a*c);
    if determinant<0. {return -1.;}
    let q = -.5*(b+select(-1.,1.,b>=0.)*sqrt(determinant));
    if q==0. {return -1.;}
    let roots=vec2f(q/a,c/q);
    let first=min(
        select(INF,roots.x,roots.x>=0.&&roots.x<=1.),
        select(INF,roots.y,roots.y>=0.&&roots.y<=1.)
    );
    return select(-1.,first,first<INF);
}

struct Hit {
    t:f32,
    lit:vec3f,
}

fn miss(distance:f32) -> Hit {
    return Hit(distance,vec3f(0.));
}

fn albedo(packed:i32) -> vec3f {
    let palette=array(vec3f(.25,.25,.24),vec3f(.15,.29,.079),
        vec3f(.29,.19,.098),vec3f(.47,.37,.21),vec3f(.82,.88,.93));
    return palette[min(u32(packed)&15u,4u)];
}

fn lit_color(packed:i32) -> vec3f {
    let bits=u32(packed);
    return vec3f(vec3u((bits>>4u)&255u,(bits>>12u)&255u,(bits>>20u)&255u))*(1./255.);
}

fn patch_hit(direction:vec3f, broad:Ray, tile_cell:vec2i, lo:f32, hi:f32, entry:u32) -> Hit {
    let side=i32(view.size.z)+1;
    let p=real_cell(broad,tile_cell);
    let vertex_index=p.x+side*p.y;
    let a=vertices[vertex_index];
    let b=vertices[vertex_index+1];
    let c=vertices[vertex_index+side];
    let d=vertices[vertex_index+side+1];
    let heights=vec4i(a.x,b.x,c.x,d.x);
    let h=vec4f(heights)*.0625;
    let dx=h.y-h.x;
    let dz=h.z-h.x;
    let mixed=h.w-h.z-dx;
    let mirror=clamp(fma(vec2f(lo),broad.velocity,broad.origin)-vec2f(tile_cell),vec2f(0.),vec2f(1.));
    let uv=select(1.-mirror,mirror,broad.sign>vec2i(0));
    let duration=hi-lo;
    var t=lo;
    var face=entry;
    if SOLVE {
        let dh=vec2f(broad.sign)*broad.velocity*duration;
        let gradient=vec2f(dx+mixed*uv.y,dz+mixed*uv.x);
        let curve=vec3f(h.x+dx*uv.x+dz*uv.y+mixed*uv.x*uv.y,
            dot(gradient,dh),mixed*dh.x*dh.y);
        // Every occupied column is at most this much above the continuous
        // generator, including the cell-centre offset and native relief.
        let slope=max(abs(dx),abs(dx+mixed))+max(abs(dz),abs(dz+mixed));
        let margin=slope*(.5/f32(WIDTH))+.012+select(0.,.1875,RELIEF);
        let crossing=root_hit(curve+vec3f(margin,0.,0.),fma(lo,direction.y,view.eye.y),direction.y,duration);
        if crossing<0. {return miss(SKY);}
        t=max(lo,fma(crossing,duration,lo)-.003);
        if t>lo {face=0u;}
    }
    let native=Ray(broad.origin*f32(WIDTH),broad.velocity*f32(WIDTH),
                   broad.inverse/f32(WIDTH),broad.sign,broad.limit*WIDTH);
    let first=tile_cell*WIDTH;
    let last=first+WIDTH-1;
    let base=p*WIDTH;
    var cell=clamp(cell_at(native,t),first,last);
    for (var i=0u;i<258u;i+=1u) {
        let local=real_cell(native,cell)-base;
        let outgoing=crossings(native,cell+1);
        let end=max(t,min(hi,min(outgoing.x,outgoing.y)));
        if end>t {
            let y=fma(t,direction.y,view.eye.y);
            let end_y=fma(end,direction.y,view.eye.y);
            let height=exact_height(heights,local,base+local+view.origin.xy,y,min(y,end_y));
            var distance=SKY;
            var hit_face=face;
            if y<=height {
                distance=t;
            } else if direction.y<0. && end_y<=height {
                distance=clamp((height-view.eye.y)/direction.y,t,end);
                hit_face=0u;
            }
            if distance>=0. {
                let position=(vec2f(local)+.5)/f32(WIDTH);
                let pixel_span=distance*1.1547/f32(view.size.y);
                var color=mix(mix(lit_color(a.y),lit_color(b.y),position.x),
                              mix(lit_color(c.y),lit_color(d.y),position.x),position.y);
                if pixel_span<.14 {
                    let material=select(select(a.y,b.y,local.x>=WIDTH/2),
                                        select(c.y,d.y,local.x>=WIDTH/2),local.y>=WIDTH/2);
                    let faces=array(.779532,.459724,0.,.439736,0.);
                    let voxel=albedo(material)*(.38+.83*faces[hit_face]);
                    color=mix(voxel,color,smoothstep(.025,.14,pixel_span));
                }
                return Hit(distance,color);
            }
        }
        if end>=hi {return miss(SKY);}
        let next=increment(cell,outgoing,end);
        if any(next<first)||any(next>last) {return miss(SKY);}
        face=face_at(native,outgoing);
        t=end;
        cell=next;
    }
    return miss(BAD);
}

fn trace(direction:vec3f, start: f32) -> Hit {
    let side=i32(view.size.z);
    let root=view.size.w;
    let sign=select(vec2i(-1),vec2i(1),direction.xz>=vec2f(0.));
    let speed=abs(direction.xz)*(16./f32(WIDTH));
    let origin=select(vec2f(f32(side))-view.eye.xz*(16./f32(WIDTH)),view.eye.xz*(16./f32(WIDTH)),sign>vec2i(0));
    let ray=Ray(origin,speed,select(1./speed,vec2f(INF),speed==vec2f(0.)),sign,side);
    var t=max(.025,start);
    if t>=view.eye.w {return miss(SKY);}
    var leaf=clamp(cell_at(ray,t),vec2i(0),vec2i(side-1));
    var mip=root;
    var entry=0u;
    let far=view.eye.w;
    let descending=direction.y<0.;
    for (var i=0u;i<8192u;i+=1u) {
        let edge=((leaf>>vec2u(mip))+1)<<vec2u(mip);
        let outgoing=crossings(ray,edge);
        let end=max(t,min(far,min(outgoing.x,outgoing.y)));
        let n=side>>mip;
        let index=select(vec2i(n-1)-(leaf>>vec2u(mip)),leaf>>vec2u(mip),sign>vec2i(0));
        let offset=(4*n*n-1)/3-n*n;
        let roof=f32(roofs[offset+index.y*n+index.x])*.0625+.003;
        if end>t && fma(select(t,end,descending),direction.y,view.eye.y)<=roof {
            if mip==0u {
                let hit=patch_hit(direction,ray,leaf,t,end,entry);
                if hit.t!=SKY {return hit;}
            } else {
                let low=edge-(1i<<mip);
                mip-=min(mip,LEVEL_STEP);
                if descending {
                    let later=clamp((roof-view.eye.y)/direction.y,t,end);
                    if later>t && later<end {
                        leaf=clamp(cell_at(ray,later),low,edge-1);
                        t=later;
                        entry=0u;
                    }
                }
                continue;
            }
        }
        if end>=far {return miss(SKY);}
        var next=increment(leaf,outgoing,end);
        if mip>0u {next=advance(ray,edge,outgoing,end);}
        if any(next>=vec2i(side)) {return miss(BAD);}
        let changed=u32((next.x^leaf.x)|(next.y^leaf.y));
        if changed==0u {return miss(BAD);}
        // Each newly entered sibling can be queried directly.
        let wanted=firstLeadingBit(changed)+select(1u,0u,SIBLING);
        mip=min(root,((wanted+LEVEL_STEP-1u)/LEVEL_STEP)*LEVEL_STEP);
        t=end;
        leaf=next;
        entry=face_at(ray,outgoing);
    }
    return miss(BAD);
}

fn screen_ray(pixel:vec2f) -> vec3f {
    let uv=pixel*2./vec2f(view.size.xy)-1.;
    return view.forward.xyz+uv.x*view.right.xyz-uv.y*view.up.xyz;
}

fn ceiling(cell:vec2i, level:u32) -> f32 {
    let n=i32(view.size.z)>>level;
    let tile=cell>>vec2u(level);
    return f32(roofs[(n*n-1)/3+tile.y*n+tile.x])*.0625;
}

fn interval_clear(directions:array<vec3f,4>, start:f32, end:f32) -> bool {
    let origin=view.eye.xz*(16./f32(WIDTH));
    var lo=vec2f(INF);
    var hi=vec2f(-INF);
    var bottom=INF;
    for(var i=0u;i<4u;i+=1u) {
        let d=directions[i];
        let p=fma(vec2f(start*(16./f32(WIDTH))),d.xz,origin);
        let q=fma(vec2f(end*(16./f32(WIDTH))),d.xz,origin);
        lo=min(lo,min(p,q));
        hi=max(hi,max(p,q));
        bottom=min(bottom,min(fma(start,d.y,view.eye.y),fma(end,d.y,view.eye.y)));
    }
    let lower_cell=vec2i(floor(lo-.002));
    let upper_cell=vec2i(floor(hi+.002));
    if any(lower_cell<vec2i(0)) || any(upper_cell>=vec2i(i32(view.size.z))) {return false;}
    let span=u32(max(upper_cell.x-lower_cell.x,upper_cell.y-lower_cell.y));
    let level=select(0u,firstLeadingBit(span)+1u,span>0u);
    let roof=max(
        max(ceiling(lower_cell,level),ceiling(vec2i(lower_cell.x,upper_cell.y),level)),
        max(ceiling(vec2i(upper_cell.x,lower_cell.y),level),ceiling(upper_cell,level))
    );
    return bottom>roof+.025;
}

@compute @workgroup_size(8,8)
fn find_prefix(@builtin(global_invocation_id) id:vec3u) {
    let pixels=(view.size.xy+7u)/8u;
    if any(id.xy>=pixels) {return;}
    let lo=vec2f(id.xy*8u);
    let hi=vec2f(min((id.xy+1u)*8u,view.size.xy));
    let directions=array(screen_ray(lo),screen_ray(vec2f(lo.x,hi.y)),
        screen_ray(vec2f(hi.x,lo.y)),screen_ray(hi));
    var t=0.;
    var step=256.;
    for(var i=0u;i<24u;i+=1u) {
        let end=min(t+step,view.eye.w);
        if interval_clear(directions,t,end) {
            t=end;
            if end>=view.eye.w {break;}
            step*=2.;
        } else {
            step*=.5;
            if step<32. {break;}
        }
    }
    prefix[id.y*pixels.x+id.x]=t;
}

fn terrain_color(hit:Hit, direction:vec3f) -> vec3f {
    let sky=mix(vec3f(.74,.83,.91),vec3f(.27,.49,.73),smoothstep(0.,.7,max(0.,direction.y)));
    if hit.t==SKY {return sky;}
    if hit.t==BAD {return vec3f(1.,0.,.4);}
    let air=1.-exp(-hit.t*.00013);
    return mix(hit.lit,vec3f(.62,.72,.81),.54*air);
}

@compute @workgroup_size(GROUP_WIDTH,GROUP_HEIGHT)
fn visibility(@builtin(global_invocation_id) id:vec3u) {
    if any(id.xy>=view.size.xy) {return;}
    let ray=screen_ray(vec2f(id.xy)+.5);
    let direction=normalize(ray);
    var start=0.;
    if BEAMS {
        let b=id.xy/8u;
        start=max(0.,prefix[b.y*((view.size.x+7u)/8u)+b.x]*length(ray)-.015);
    }
    let hit=trace(direction,start);
    if hit.t==BAD {atomicAdd(&failures,1u);}
    let color=terrain_color(hit,direction);
    let linear=clamp(color,vec3f(0.),vec3f(1.));
    let encoded=vec3u(round(255.*sqrt(linear)*(1.08-.08*linear)));
    pixels[id.y*view.size.x+id.x]=vec2u(bitcast<u32>(hit.t),encoded.x|(encoded.y<<8u)|(encoded.z<<16u));
}
