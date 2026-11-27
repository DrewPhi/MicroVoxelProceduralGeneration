// Exact first hits against the fixed world-space voxel-column source.
// The maximum hierarchy only rejects ray intervals. Leaf intersections use
// canonical integer heights and the native column faces.
const LATTICE_WIDTH = 128i;

struct LatticeView {
    eye:vec4f,
    origin:vec4i,
    config:vec4u,
    eye_cell:vec4i,
    eye_fraction:vec4f,
}
@group(0) @binding(32) var<uniform> lattice_view:LatticeView;
@group(0) @binding(33) var<storage,read> lattice_controls:array<vec2i>;
@group(0) @binding(34) var<storage,read> lattice_roofs:array<i32>;
@group(0) @binding(35) var<storage,read> lattice_edits:array<vec4i>;
@group(0) @binding(36) var<storage,read> lattice_read_prefix:array<f32>;
@group(0) @binding(37) var<storage,read_write> lattice_write_prefix:array<f32>;
const LATTICE_BEAM_MAX_EXTENT=4096u;
const LATTICE_COMPUTE_MAX_PIXELS=2097152u;
override LATTICE_COMPUTE:bool = false;
override LATTICE_BEAMS:bool = false;
override LATTICE_BEAM_WIDTH:u32 = 8u;
override LATTICE_LOCAL_LEAP:bool = false;
override LATTICE_RELIEF_BOUNDS:bool = false;
override LATTICE_DETAIL:bool = false;
override LATTICE_FLORA:bool = false;
override LATTICE_DETAIL_STEP:u32 = 1u;
override LATTICE_DETAIL_TILE_X:u32 = 8u;
override LATTICE_GROUP_THREADS:u32 = 64u;
override LATTICE_SPLIT:bool = false;
@group(0) @binding(40) var lattice_detail:texture_2d_array<u32>;
@group(0) @binding(38) var<storage,read> lattice_read_hits:array<vec2u>;
@group(0) @binding(39) var<storage,read_write> lattice_write_hits:array<vec2u>;

struct LatticeRay {
    origin: vec2f,
    base: vec2i,
    velocity: vec2f,
    inverse: vec2f,
    sign: vec2i,
    limit: i32,
}

fn lattice_crossings(ray: LatticeRay, edge: vec2i) -> vec2f {
    return select((vec2f(edge-ray.base)-ray.origin)*ray.inverse,vec2f(INF),ray.velocity==vec2f(0.));
}

fn lattice_cell_at(ray: LatticeRay, t: f32) -> vec2i {
    let estimate = ray.base+vec2i(floor(fma(vec2f(t),ray.velocity,ray.origin)));
    let before = (lattice_crossings(ray, estimate) > vec2f(t)) & (ray.velocity > vec2f(0.));
    let after = lattice_crossings(ray, estimate + 1) <= vec2f(t);
    return estimate + select(vec2i(0),vec2i(1),after) - select(vec2i(0),vec2i(1),before);
}

fn lattice_advance(ray: LatticeRay, edge: vec2i, exits: vec2f, t: f32) -> vec2i {
    return select(min(lattice_cell_at(ray,t),edge-1),edge,exits<=vec2f(t));
}

fn lattice_increment(cell:vec2i, exits:vec2f, t:f32) -> vec2i {
    return cell+select(vec2i(0),vec2i(1),exits<=vec2f(t));
}

fn lattice_face_at(ray: LatticeRay, exits: vec2f) -> u32 {
    if exits.x <= exits.y {
        return select(2u, 1u, ray.sign.x > 0);
    }
    return select(4u, 3u, ray.sign.y > 0);
}

fn lattice_real_cell(ray: LatticeRay, cell: vec2i) -> vec2i {
    return select(vec2i(ray.limit-1)-cell,cell,ray.sign>vec2i(0));
}

fn lattice_hash(x: i32, z: i32, seed: u32) -> i32 {
    var v = u32(x) * 0x9e3779b9u ^ u32(z) * 0x85ebca6bu ^ seed;
    v = (v ^ (v >> 16u)) * 0x7feb352du;
    v = (v ^ (v >> 15u)) * 0x846ca68bu;
    return i32((v ^ (v >> 16u)) & 255u) - 128;
}

fn flora_hash(pos:vec2i,seed:u32) -> u32 {
    var v=u32(pos.x)*0x9e3779b9u ^ u32(pos.y)*0x85ebca6bu ^ seed;
    v=(v^(v>>16u))*0x7feb352du;
    v=(v^(v>>15u))*0x846ca68bu;
    return v^(v>>16u);
}

fn flora_active() -> bool {return LATTICE_FLORA && lattice_view.config.z!=0u;}
const FLORA_BAND = .3125;

fn lattice_relief_band(cell: vec2i, power: u32, seed: u32) -> i32 {
    let base = cell >> vec2u(power);
    let width = 1i << power;
    let uv = cell & vec2i(width-1);
    let a = lattice_hash(base.x,base.y,seed) * (width-uv.x) + lattice_hash(base.x+1,base.y,seed)*uv.x;
    let b = lattice_hash(base.x,base.y+1,seed) * (width-uv.x) + lattice_hash(base.x+1,base.y+1,seed)*uv.x;
    return (a*(width-uv.y)+b*uv.y) >> (2u*power);
}

fn lattice_interpolate(corners: vec4i, local: vec2i) -> i32 {
    // Split the signed numerator so world-height units never need i64.
    let p = 2*local+1;
    let a = corners.x*(256-p.x)+corners.y*p.x;
    let b = corners.z*(256-p.x)+corners.w*p.x;
    let integral = (a>>8u)*(256-p.y)+(b>>8u)*p.y;
    let residual = ((a&255)*(256-p.y)+(b&255)*p.y)>>8u;
    return (integral+residual)>>8u;
}

fn lattice_control_heights(packed:vec4i) -> vec4i {
    return (packed<<vec4u(8u))>>vec4u(8u);
}

fn lattice_detail_node(tile:u32,local:vec2i,level:u32) -> u32 {
    return textureLoad(lattice_detail,local>>vec2u(level),i32(tile),i32(level)).x;
}

fn lattice_detail_top(tile:u32,local:vec2i,level:u32)->i32 {
    return (i32(lattice_detail_node(tile,local,level))<<24u)>>24u;
}

fn flora_upper(node:u32,cover:bool)->i32 {
    return select((i32(node)<<24u)>>24u,(i32(node)<<8u)>>24u,cover);
}


// The chord of a quadratic deviates from it by at most c*(b-a)^2/4
// on the interval. This lower bound avoids a reciprocal per visited node.
fn lattice_interval_min(polynomial:vec3f,lo:f32,hi:f32)->f32 {
    let a=fma(fma(polynomial.z,lo,polynomial.y),lo,polynomial.x);
    let b=fma(fma(polynomial.z,hi,polynomial.y),hi,polynomial.x);
    let width=hi-lo;
    return min(a,b)-.25*max(0.,polynomial.z)*width*width;
}

fn lattice_exact_height(corners:vec4i, local:vec2i, world:vec2i, entry_y:f32, minimum_y:f32, relief:bool) -> f32 {
    let value = lattice_interpolate(corners,local);
    if !relief {return f32(value)*.0625;}
    let low=f32(value-4)*.0625;
    let high=f32(value+3)*.0625;
    // These limits can certify an entry-side hit or a miss without hashing.
    if entry_y<=low {return low;}
    if minimum_y>high {return high;}
    let seed=u32(lattice_view.origin.z);
    let coarse=3*lattice_relief_band(world,4u,seed);
    let partial_low=f32(value+((coarse-128)>>7u))*.0625;
    let partial_high=f32(value+((coarse+127)>>7u))*.0625;
    if entry_y<=partial_low {return partial_low;}
    if minimum_y>partial_high {return partial_high;}
    if partial_low==partial_high {return partial_low;}
    return f32(value+((coarse+lattice_relief_band(world,2u,seed^0x2d1189a1u))>>7u))*.0625;
}

fn lattice_root_hit(curve:vec3f, y:f32, slope:f32, length:f32) -> f32 {
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


struct LatticeHit {
    t:f32,
    face:u32,
    material:u32,
    bed:u32,
    water_depth:f32,
}

fn lattice_miss(t:f32) -> LatticeHit {
    return LatticeHit(t,0u,0u,0u,0.);
}

fn lattice_curve(h:vec4f,uv:vec2f,dh:vec2f) -> vec3f {
    let dx=h.y-h.x;
    let dz=h.z-h.x;
    let mixed=h.w-h.z-dx;
    return vec3f(h.x+dx*uv.x+dz*uv.y+mixed*uv.x*uv.y,
        dot(vec2f(dx+mixed*uv.y,dz+mixed*uv.x),dh),mixed*dh.x*dh.y);
}

fn lattice_margin(h:vec4f) -> f32 {
    let slope=max(abs(h.y-h.x),abs(h.w-h.z))+max(abs(h.z-h.x),abs(h.w-h.y));
    let magnitude=max(max(abs(h.x),abs(h.y)),max(abs(h.z),abs(h.w)));
    return slope*(.5/f32(LATTICE_WIDTH))+max(.012,magnitude*1e-6);
}

fn lattice_relief_cap(world:vec2i) -> f32 {
    let knot=world>>vec2u(4u);
    let seed=u32(lattice_view.origin.z);
    let high=max(max(lattice_hash(knot.x,knot.y,seed),lattice_hash(knot.x+1,knot.y,seed)),
                 max(lattice_hash(knot.x,knot.y+1,seed),lattice_hash(knot.x+1,knot.y+1,seed)));
    return f32((3*high+127)>>7u)*.0625;
}

fn lattice_find_edit(key:i32, first:i32, count:i32) -> vec4i {
    var l=first;
    var r=first+count;
    while l<r {
        let m=(l+r)>>1u;
        let row=lattice_edits[m];
        if row.x==key {return row;}
        if row.x<key {l=m+1;} else {r=m;}
    }
    return vec4i(0);
}

fn lattice_edit_hit(edit:vec4i,y:f32,end_y:f32,t:f32,end:f32,slope:f32,face:u32) -> LatticeHit {
    var hit=lattice_miss(-1.);
    for (var i=0;i<edit.z;i+=1) {
        let run=lattice_edits[edit.y+i];
        let lo=f32(run.x)*.0625;
        let hi=f32(run.y)*.0625;
        var distance=-1.;
        var side=face;
        if y>=lo && y<hi {
            distance=t;
        } else if slope<0. && y>=hi && end_y<=hi {
            distance=clamp((hi-lattice_view.eye.y)/slope,t,end);
            side=0u;
        } else if slope>0. && y<lo && end_y>=lo {
            distance=clamp((lo-lattice_view.eye.y)/slope,t,end);
            side=5u;
        }
        if distance>=0. && (hit.t<0. || distance<hit.t) {
            let mat=u32(run.z);
            let below=select(run,lattice_edits[max(edit.y,edit.y+i-1)],i>0);
            let bed=select(mat,u32(below.z),i>0 && below.y==run.x);
            hit=LatticeHit(distance,side|8u,mat,bed,hi-lo);
        }
    }
    return hit;
}

fn lattice_patch_hit(direction:vec3f,broad:LatticeRay,tile_cell:vec2i,lo:f32,hi:f32,entry:u32,source_edited:bool,dry_only:bool) -> LatticeHit {
    let side=i32(lattice_view.config.x)+1;
    let p=lattice_real_cell(broad,tile_cell);
    let vertex_index=p.x+side*p.y;
    let a=lattice_controls[vertex_index];
    let b=lattice_controls[vertex_index+1];
    let c=lattice_controls[vertex_index+side];
    let d=lattice_controls[vertex_index+side+1];
    let heights=lattice_control_heights(vec4i(a.x,b.x,c.x,d.x));
    let tile=u32(a.x)>>24u;
    let tags=vec4i(a.y,b.y,c.y,d.y);
    let materials=vec4u((tags>>vec4u(22u))&vec4i(15));
    let subsurface=vec4u((tags>>vec4u(26u))&vec4i(15));
    let source_wet=((a.y|b.y|c.y|d.y)&(1i<<30u))!=0;
    if dry_only && (source_wet || source_edited) {
        // Earlier intervals have already been proved empty. Preserve enough
        // lead-in for the general traversal to recover the entry face.
        return LatticeHit(-3.,0u,0u,0u,lo);
    }
    let wet=!dry_only && source_wet;
    let edited=!dry_only && source_edited;
    let relief=a.y<0;
    let heads=heights+((tags<<vec4u(10u))>>vec4u(10u));
    let h=vec4f(heights)*.0625;
    let relative_origin=broad.origin+vec2f(broad.base-tile_cell);
    let mirror=clamp(fma(vec2f(lo),broad.velocity,relative_origin),vec2f(0.),vec2f(1.));
    let uv=select(1.-mirror,mirror,broad.sign>vec2i(0));
    let duration=hi-lo;
    let y=fma(lo,direction.y,lattice_view.eye.y);
    let line=vec3f(y,direction.y*duration,0.);
    let dh=vec2f(broad.sign)*broad.velocity*duration;
    let carrier=lattice_curve(h,uv,dh);
    let carrier_margin=lattice_margin(h);
    let head_heights=vec4f(heads)*.0625;
    var head=vec3f(0.);
    var head_margin=0.;
    if wet {
        head=lattice_curve(head_heights,uv,dh);
        head_margin=lattice_margin(head_heights);
    }
    let detailed=LATTICE_DETAIL && relief;
    let cover=flora_active() && any(materials==vec4u(1u));

    var t=lo;
    var face=entry;
    var edits=vec4i(0);
    if edited {
        edits=lattice_find_edit(p.x+p.y*(side-1),1,lattice_edits[0].x);
    } else {
        var margin=carrier_margin+select(0.,.1875,relief)+select(0.,FLORA_BAND,cover);
        if detailed {
            margin=carrier_margin+f32(flora_upper(lattice_detail_node(tile,vec2i(0),7u),cover))*.0625;
        }
        var crossing=lattice_root_hit(carrier+vec3f(margin,0.,0.),y,direction.y,duration);
        if wet {
            let water=lattice_root_hit(head+vec3f(head_margin,0.,0.),y,direction.y,duration);
            if water>=0. && (crossing<0. || water<crossing) {crossing=water;}
        }
        if crossing<0. {return lattice_miss(-1.);}
        t=max(lo,fma(crossing,duration,lo)-.003);
        if t>lo {face=0u;}
    }
    let native=LatticeRay(broad.origin*f32(LATTICE_WIDTH),broad.base*LATTICE_WIDTH,broad.velocity*f32(LATTICE_WIDTH),
                   broad.inverse/f32(LATTICE_WIDTH),broad.sign,broad.limit*LATTICE_WIDTH);
    let first=tile_cell*LATTICE_WIDTH;
    let last=first+LATTICE_WIDTH-1;
    let base=p*LATTICE_WIDTH;
    var cell=clamp(lattice_cell_at(native,t),first,last);
    var last_block=vec2i(-1);
    // Try the flat exact DDA for very short candidate intervals. Its
    // answers are identical and it avoids loading any child bounds.
    let horizontal_work=(hi-t)*(native.velocity.x+native.velocity.y);
    let hierarchy=detailed && !edited && LATTICE_DETAIL_STEP>0u && horizontal_work>5.;
    let difference=line-carrier;
    let inverse_duration=1./duration;
    let water_difference=line-head;
    let step=max(1u,LATTICE_DETAIL_STEP);
    let highest=6u;
    var level=select(0u,highest,hierarchy);
    for (var i=0u;i<2048u;i+=1u) {
        // The one-metre relief band is bilinear between these knots. Bound
        // it once over the block, then add the maximum of the finer band.
        // The quadratic carrier bound can skip the entire block or narrow
        // its entry before the same exact native DDA.
        if LATTICE_RELIEF_BOUNDS && !LATTICE_DETAIL && relief && !wet && !edited {
            let block=cell>>vec2u(4u);
            if any(block!=last_block) {
                last_block=block;
                let edge=(block+1)<<vec2u(4u);
                let block_out=lattice_crossings(native,edge);
                let block_end=min(hi,min(block_out.x,block_out.y));
                let span=block_end-t;
                if span>.24 {
                    let s=clamp(fma(vec2f(t),broad.velocity,relative_origin),vec2f(0.),vec2f(1.));
                    let q=select(1.-s,s,broad.sign>vec2i(0));
                    let dir=vec2f(broad.sign)*broad.velocity;
                    let roof=lattice_relief_cap(lattice_real_cell(native,cell)+lattice_view.origin.xy);
                    let curve=lattice_curve(h,q,dir*span);
                    let height=fma(t,direction.y,lattice_view.eye.y);
                    let crossing=lattice_root_hit(curve+vec3f(lattice_margin(h)+roof,0.,0.),height,direction.y,span);
                    if crossing<0. {
                        if block_end>=hi {return lattice_miss(-1.);}
                        cell=lattice_advance(native,edge,block_out,block_end);
                        if any(cell<first)||any(cell>last) {return lattice_miss(-1.);}
                        face=lattice_face_at(native,block_out);
                        t=block_end;
                        continue;
                    }
                    let later=max(t,fma(crossing,span,t)-.003);
                    if later>t {
                        cell=clamp(lattice_cell_at(native,later),cell,edge-1);
                        face=0u;
                        t=later;
                    }
                }
            }
        }
        let local=lattice_real_cell(native,cell)-base;
        let corner=u32(local.x>=LATTICE_WIDTH/2)+2u*u32(local.y>=LATTICE_WIDTH/2);
        let grass_here=cover && materials[corner]==1u;
        let edge=((cell>>vec2u(level))+1)<<vec2u(level);
        let outgoing=lattice_crossings(native,edge);
        let end=max(t,min(hi,min(outgoing.x,outgoing.y)));
        if end>t && level>0u {
            let a=clamp((t-lo)*inverse_duration,0.,1.);
            let b=clamp((end-lo)*inverse_duration,a,1.);
            let node=lattice_detail_node(tile,local,level);
            let upper=carrier_margin+f32(flora_upper(node,grass_here))*.0625;
            let ground_possible=lattice_interval_min(difference,a,b)<=upper;
            var water_possible=false;
            if wet {
                water_possible=lattice_interval_min(water_difference,a,b)<=head_margin;
            }
            if ground_possible || water_possible {
                if LATTICE_LOCAL_LEAP && !wet && direction.y < .25 {
                    let at_entry=fma(fma(difference.z,a,difference.y),a,difference.x);
                    // Below the entire block, the entry cell already owns
                    // the first hit. Decode only that cell.
                    if grass_here && at_entry <= f32(i32(node)>>24u)*.0625-carrier_margin-.002 {
                        level=0u;
                        continue;
                    }
                    // The quadratic's fastest descent on this subinterval
                    // bounds how far the ray can move before reaching its roof.
                    let gap=at_entry-upper;
                    let drift=-min(fma(2.*difference.z,a,difference.y),fma(2.*difference.z,b,difference.y));
                    if gap>.002 && drift>.00001 {
                        let step_m=(gap-.002)/drift*duration;
                        let later=min(end-.003,t+step_m);
                        if later-t>.125 {
                            cell=clamp(lattice_cell_at(native,later),cell,edge-1);
                            face=0u;
                            t=later;
                        }
                    }
                }
                level-=min(step,level);
                continue;
            }
        }
        if end>t && level==0u {
            let y=fma(t,direction.y,lattice_view.eye.y);
            let end_y=fma(end,direction.y,lattice_view.eye.y);
            var edit=vec4i(0);
            if edited {edit=lattice_find_edit(local.x+local.y*LATTICE_WIDTH,edits.y,edits.z);}
            if edit.z>0 {
                let hit=lattice_edit_hit(edit,y,end_y,t,end,direction.y,face);
                if hit.t>=0. {return hit;}
            } else {
                let world=base+local+lattice_view.origin.xy;
                var ground=0.;
                var height=0.;
                var water=0.;
                var grass_code=0u;
                if LATTICE_DETAIL {
                    let base=lattice_interpolate(heights,local);
                    var bed=base;
                    if relief || grass_here {
                        let sample=lattice_detail_node(tile,local,0u);
                        if relief {bed+=(i32(sample)<<24u)>>24u;}
                        if grass_here {grass_code=sample>>24u;}
                    }
                    if wet {
                        let head=lattice_interpolate(heads,local);
                        if head>base {bed=min(bed,head-1);}
                        water=f32(max(0,head-bed))*.0625;
                    }
                    ground=f32(bed)*.0625;
                    height=ground+water;
                } else {
                    ground=lattice_exact_height(heights,local,world,y,min(y,end_y),relief);
                    height=ground;
                    if wet {
                        ground=lattice_exact_height(heights,local,world,INF,-INF,relief);
                        height=max(ground,f32(lattice_interpolate(heads,local))*.0625);
                        water=height-ground;
                    }
                }
                let bed=materials[corner];
                if water>0. {grass_code=0u;}
                height+=f32(grass_code&7u)*.0625;
                var distance=-1.;
                var hit_face=face;
                if direction.y<0. {
                    let crossing=(height-lattice_view.eye.y)/direction.y;
                    if crossing<end {
                        distance=max(t,crossing);
                        hit_face=select(0u,face,crossing<t);
                    }
                } else if y<height {
                    distance=t;
                }
                if distance>=0. {
                    if grass_code!=0u && (hit_face==0u || y>=ground) {
                        let bloom=(grass_code&8u)!=0u && (hit_face==0u || y>=height-.0625);
                        return LatticeHit(distance,hit_face,select(5u,6u,bloom),bed,0.);
                    }
                    var material=bed;
                    if water>0. && (hit_face==0u || y>=height-water) {
                        material=7u;
                    } else if hit_face!=0u {
                        var actual=ground;
                        if !LATTICE_DETAIL && !wet {actual=lattice_exact_height(heights,local,world,INF,-INF,relief);}
                        if y<actual-.75 {material=3u;}
                        else if y<actual-.0625 {material=subsurface[corner];}
                    }
                    let depth=select(water,max(0.,y-(height-water)),hit_face!=0u);
                    return LatticeHit(distance,hit_face,material,bed,depth);
                }
            }
        }
        if end>=hi {return lattice_miss(-1.);}
        var next=lattice_increment(cell,outgoing,end);
        if level>0u {next=lattice_advance(native,edge,outgoing,end);}
        if any(next<first)||any(next>last) {return lattice_miss(-1.);}
        if hierarchy {
            let changed=u32((next.x^cell.x)|(next.y^cell.y));
            if changed==0u {return lattice_miss(-2.);}
            let sibling=firstLeadingBit(changed);
            level=min(highest,((sibling+step-1u)/step)*step);
        }
        face=lattice_face_at(native,outgoing);
        t=end;
        cell=next;
    }
    return lattice_miss(-2.);
}

fn lattice_trace(direction:vec3f, start: f32, far: f32, dry_only:bool) -> LatticeHit {
    let side=i32(lattice_view.config.x);
    let root=lattice_view.config.y;
    let sign=select(vec2i(-1),vec2i(1),direction.xz>=vec2f(0.));
    let speed=abs(direction.xz)*(16./f32(LATTICE_WIDTH));
    // Subtract integer grid addresses before converting to f32. A window
    // can span 262,144 native cells, which would otherwise lose enough phase
    // to skip a genuine first hit near a grazing edge or a corner.
    let tile=lattice_view.eye_cell.xy/LATTICE_WIDTH;
    let fraction=(vec2f(lattice_view.eye_cell.xy%LATTICE_WIDTH)+lattice_view.eye_fraction.xy)/f32(LATTICE_WIDTH);
    let base=select(vec2i(side)-tile,tile,sign>vec2i(0));
    let origin=vec2f(sign)*fraction;
    let ray=LatticeRay(origin,base,speed,select(1./speed,vec2f(INF),speed==vec2f(0.)),sign,side);
    var t=max(.025,start);
    if t>=far {return lattice_miss(-1.);}
    var leaf=clamp(lattice_cell_at(ray,t),vec2i(0),vec2i(side-1));
    var mip=root;
    var entry=0u;
    let descending=direction.y<0.;
    for (var i=0u;i<8192u;i+=1u) {
        let edge=((leaf>>vec2u(mip))+1)<<vec2u(mip);
        let outgoing=lattice_crossings(ray,edge);
        let end=max(t,min(far,min(outgoing.x,outgoing.y)));
        let n=side>>mip;
        let index=select(vec2i(n-1)-(leaf>>vec2u(mip)),leaf>>vec2u(mip),sign>vec2i(0));
        let offset=(4*n*n-1)/3-n*n;
        let packed=lattice_roofs[offset+index.y*n+index.x];
        let roof=f32(select(packed,(packed<<8u)>>8u,mip==0u))*.0625+.003;
        if end>t && fma(select(t,end,descending),direction.y,lattice_view.eye.y)<=roof {
            if mip==0u {
                let hit=lattice_patch_hit(direction,ray,leaf,t,end,entry,(packed&(1i<<30u))!=0i,dry_only);
                if hit.t!=-1. {return hit;}
            } else {
                let low=edge-(1i<<mip);
                mip-=min(mip,2u);
                if descending {
                    let later=clamp((roof-lattice_view.eye.y)/direction.y,t,end);
                    if later>t && later<end {
                        leaf=clamp(lattice_cell_at(ray,later),low,edge-1);
                        t=later;
                        entry=0u;
                    }
                }
                continue;
            }
        }
        if end>=far {return lattice_miss(-1.);}
        var next=lattice_increment(leaf,outgoing,end);
        if mip>0u {next=lattice_advance(ray,edge,outgoing,end);}
        if any(next>=vec2i(side)) {return lattice_miss(-2.);}
        let changed=u32((next.x^leaf.x)|(next.y^leaf.y));
        if changed==0u {return lattice_miss(-2.);}
        // Each newly entered sibling can be queried directly.
        mip=min(root,(firstLeadingBit(changed)+1u)&0xfffffffeu);
        t=end;
        leaf=next;
        entry=lattice_face_at(ray,outgoing);
    }
    return lattice_miss(-2.);
}


fn lattice_prefix(pixel:vec2u) -> f32 {
    let tiles=(view.size.xy+LATTICE_BEAM_WIDTH-1u)/LATTICE_BEAM_WIDTH;
    let tile=pixel/LATTICE_BEAM_WIDTH;
    return lattice_read_prefix[tile.y*tiles.x+tile.x];
}

fn lattice_compute_fits() -> bool {
    return view.size.x*view.size.y<=LATTICE_COMPUTE_MAX_PIXELS;
}

@compute @workgroup_size(LATTICE_DETAIL_TILE_X,LATTICE_GROUP_THREADS/LATTICE_DETAIL_TILE_X)
fn lattice_visibility(@builtin(global_invocation_id) id:vec3u) {
    if any(id.xy>=view.size.xy) {return;}
    let ray=screen_ray(id.xy);
    let scale=length(ray);
    var start=NEAR;
    if LATTICE_BEAMS && all(view.size.xy<=vec2u(LATTICE_BEAM_MAX_EXTENT)) {
        start=max(start,lattice_prefix(id.xy));
    }
    let hit=lattice_trace(ray/scale,start*scale,view.eye.w,LATTICE_SPLIT);
    lattice_write_hits[id.y*view.size.x+id.x]=lattice_encode_hit(hit);
}

fn lattice_encode_hit(hit:LatticeHit)->vec2u {
    if hit.t == -3. {return vec2u(bitcast<u32>(hit.t),bitcast<u32>(hit.water_depth));}
    let wet_depth=u32(clamp(round(hit.water_depth*256.),0.,1048575.));
    let attributes=hit.face | (hit.material<<4u) | (hit.bed<<8u) | (wet_depth<<12u);
    return vec2u(bitcast<u32>(hit.t),attributes);
}

@compute @workgroup_size(LATTICE_DETAIL_TILE_X,LATTICE_GROUP_THREADS/LATTICE_DETAIL_TILE_X)
fn lattice_resolve(@builtin(global_invocation_id) id:vec3u) {
    if any(id.xy>=view.size.xy) {return;}
    let pixel=id.y*view.size.x+id.x;
    let prior=lattice_write_hits[pixel];
    if bitcast<f32>(prior.x)!=-3. {return;}
    let start=bitcast<f32>(prior.y)-.004;
    let direction=screen_ray(id.xy);
    let hit=lattice_trace(direction/length(direction),start,view.eye.w,false);
    lattice_write_hits[pixel]=lattice_encode_hit(hit);
}

// Project one pixel onto the local terrain plane. At grazing incidence a
// pixel covers a long strip of terrain even when the hit is close by.
fn lattice_pixel_span(direction:vec3f, depth:f32, normal:vec3f) -> f32 {
    let dx=2.*view.right.xyz/f32(view.size.x);
    let dy=2.*view.up.xyz/f32(view.size.y);
    let denominator=dot(normal,direction);
    if abs(denominator)<1e-6 {return depth*2.;}
    let tx=depth*(dx-direction*dot(normal,dx)/denominator);
    let ty=depth*(dy-direction*dot(normal,dy)/denominator);
    return max(length(tx.xz),length(ty.xz));
}

fn lattice_detail_average(sample:vec2i, level:u32, owner:vec2i, control:vec2i) -> f32 {
    let side=LATTICE_WIDTH>>level;
    let tile=sample/side;
    var sample_control=control;
    if any(tile!=owner) {
        let width=i32(lattice_view.config.x)+1;
        sample_control=lattice_controls[tile.y*width+tile.x];
    }
    if sample_control.y>=0 {return 0.;}
    let node=lattice_detail_node(u32(sample_control.x)>>24u,(sample%side)<<vec2u(level),level);
    return f32((i32(node)<<16u)>>24u);
}

fn lattice_detail_normal_gradient(position:vec2f,level:u32,owner:vec2i,control:vec2i) -> vec2f {
    let stride=f32(1u<<level);
    let q=position*(16./stride)-.5;
    let i=vec2i(floor(q));
    let uv=fract(q);
    let a=lattice_detail_average(i,level,owner,control);
    let b=lattice_detail_average(i+vec2i(1,0),level,owner,control);
    let c=lattice_detail_average(i+vec2i(0,1),level,owner,control);
    let d=lattice_detail_average(i+1,level,owner,control);
    return vec2f(mix(b-a,d-c,uv.y),mix(c-a,d-b,uv.x))/stride;
}

fn lattice_screen_gradient(position:vec2f,footprint:f32,owner:vec2i,control:vec2i) -> vec2f {
    let level=clamp(log2(max(1.,footprint*16.)),0.,7.);
    let a=u32(floor(level));
    let b=min(7u,a+1u);
    let slope=lattice_detail_normal_gradient(position,a,owner,control);
    if a==b {return slope*min(1.,8./max(footprint,.0625));}
    return mix(slope,lattice_detail_normal_gradient(position,b,owner,control),fract(level));
}

fn lattice_fragment(direction:vec3f,near:f32,far:f32,pixel:vec2u,center_sample:bool) -> vec4f {
    let scale=length(direction);
    let ray=direction/scale;
    var hit=lattice_miss(-1.);
    if center_sample && LATTICE_COMPUTE && lattice_compute_fits() {
        let stored=lattice_read_hits[pixel.y*view.size.x+pixel.x];
        let t=bitcast<f32>(stored.x);
        if t>far*scale {return vec4f(0.,0.,0.,-1.);}
        hit=LatticeHit(t,stored.y&15u,(stored.y>>4u)&15u,(stored.y>>8u)&15u,f32(stored.y>>12u)/256.);
    } else {
        var start=near;
        if LATTICE_BEAMS && all(view.size.xy<=vec2u(LATTICE_BEAM_MAX_EXTENT)) {
            start=max(start,lattice_prefix(pixel));
        }
        hit=lattice_trace(ray,start*scale,far*scale,false);
    }
    if hit.t<0. {return vec4f(0.,0.,0.,hit.t);}
    let distance=hit.t/scale;
    if CAPTURE_DISTANCE || (native_view.source.w&1u)!=0u {
        return vec4f(0.,0.,0.,distance);
    }
    let relative=ray*hit.t;
    let p=view.eye.xyz+relative;
    let face=rolling_normal(i32(hit.face&7u));
    if hit.material==7u && (native_view.source.w&4u)==0u {
        // Liquid keeps the exact stepped silhouette. Shading uses a rippled
        // fluid surface, avoiding dark bands at every quantized river level.
        let normal=rolling_water_normal(relative,vec3f(0.,1.,0.),ray,hit.t);
        let bed=native_view.palette[hit.bed].rgb;
        return vec4f(rolling_water(ray,normal,rolling_bed_transmission(ray,normal,bed,hit.water_depth)),distance);
    }
    if flora_active() && (native_view.source.w&4u)==0u &&
            (hit.material==1u || hit.material==5u || hit.material==6u) {
        // At small projected coverage, filter leaf and ground normals toward
        // the same supporting slope. Visibility and depth remain native.
        let tile_position=(lattice_view.eye.xz+relative.xz)*.125;
        let tile_cell=vec2i(floor(tile_position));
        let uv=fract(tile_position);
        let pitch=i32(lattice_view.config.x)+1;
        let index=tile_cell.x+tile_cell.y*pitch;
        let h=vec4f(lattice_control_heights(vec4i(
            lattice_controls[index].x,lattice_controls[index+1].x,
            lattice_controls[index+pitch].x,lattice_controls[index+pitch+1].x
        )))*.0625;
        let bend=h.w-h.z-h.y+h.x;
        let slope=normalize(vec3f(-(h.y-h.x+bend*uv.y)*.125,1.,-(h.z-h.x+bend*uv.x)*.125));
        let spread=hit.t*view.up.w/max(.045,-dot(ray,slope));
        let support=min(1.,.0625/max(spread,.0001));
        let normal=normalize(mix(slope,normalize(mix(face,vec3f(0.,1.,0.),.35)),support));
        let base=vec3f(.118,.231,.058);
        let near_color=select(vec3f(.135,.243,.075),vec3f(.097,.213,.045),hit.material==5u);
        let blade=select(near_color,vec3f(.75,.56,.37),hit.material==6u);
        let varies=.83+.32*pigment_noise(p.xz/47.,0x18a49375u);
        let irradiance=broad_irradiance(p);
        let sunlight=irradiance.x*aircraft_sun_visibility(relative)*formation_sun_visibility(relative);
        return vec4f(mix(base,blade,support)*varies*rolling_light_irradiance(normal,sunlight,irradiance.y),distance);
    }
    var slope=face;
    var detail_footprint=.0625;
    var pigment=native_view.palette[hit.material].rgb;
    let edited=(hit.face&8u)!=0u;
    if !edited {
        let position_m=lattice_view.eye.xz+relative.xz;
        let tile=position_m*.125;
        let column=vec2i(floor(tile));
        let position=fract(tile);
        let side=i32(lattice_view.config.x)+1;
        let index=column.x+column.y*side;
        let a=lattice_controls[index];
        let b=lattice_controls[index+1];
        let c=lattice_controls[index+side];
        let d=lattice_controls[index+side+1];
        let h=vec4f(lattice_control_heights(vec4i(a.x,b.x,c.x,d.x)))*.0625;
        let mixed=h.w-h.z-h.y+h.x;
        let gradient=vec2f(h.y-h.x+mixed*position.y,h.z-h.x+mixed*position.x)*.125;
        slope=normalize(vec3f(-gradient.x,1.,-gradient.y));
        if LATTICE_DETAIL {
            detail_footprint=lattice_pixel_span(direction,distance,slope)*select(.5,1.,center_sample);
            let relief=lattice_screen_gradient(position_m,detail_footprint,column,a);
            let combined=gradient+relief;
            slope=normalize(vec3f(-combined.x,1.,-combined.y));
        }
        let materials=vec4u((vec4i(a.y,b.y,c.y,d.y)>>vec4u(22u))&vec4i(15));
        pigment=mix(mix(native_view.palette[materials.x].rgb,native_view.palette[materials.y].rgb,position.x),
                    mix(native_view.palette[materials.z].rgb,native_view.palette[materials.w].rgb,position.x),position.y);
    }
    var visibility_fraction=rolling_voxel_resolution(relative,ray,slope);
    if LATTICE_DETAIL {visibility_fraction=min(1.,.0625/max(detail_footprint,.0001));}
    let resolved=select(visibility_fraction,1.,edited);
    let normal=normalize(mix(slope,face,resolved));
    if (native_view.source.w&4u)!=0u {
        return vec4f(vec3f(.38)*rolling_light_visible(normal,1.),distance);
    }
    let irradiance=broad_irradiance(p);
    let visibility=irradiance.x*aircraft_sun_visibility(relative)*formation_sun_visibility(relative)
        *woodland_sun_visibility(relative+face*.04)*canopy_ground_light(relative);
    let footprint=2.*native_view.voxel.w*distance/f32(view.size.y)
        /max(.04,-dot(ray,slope));
    let cover=pigment_noise(p.xz/120.,0xb5297a4du);
    var grain_weight=1.-smoothstep(6.,35.,footprint);
    if LATTICE_DETAIL {grain_weight=min(1.,24./max(detail_footprint,.0625));}
    let grain=(pigment_noise(p.xz/24.,0x68e31da4u)-.5)*grain_weight;
    var albedo=pigment*(.92+.16*cover+.19*grain);
    if native_view.owner_max.w>.5 && hit.t>570. {
        let canopy=filtered_pigment(p.xz,outer_grid()).grove;
        let reach=smoothstep(570.,1050.,hit.t);
        albedo*=mix(vec3f(1.),vec3f(.53,.71,.66),canopy*reach);
    }
    albedo=mix(albedo,native_view.palette[hit.material].rgb,resolved);
    return vec4f(albedo*rolling_light_irradiance(normal,visibility,irradiance.y),distance);
}

fn lattice_screen_corner(pixel:vec2f) -> vec3f {
    let uv=pixel*2./vec2f(view.size.xy)-1.;
    return view.forward.xyz+uv.x*view.right.xyz-uv.y*view.up.xyz;
}

fn lattice_ceiling(cell:vec2i, level:u32) -> f32 {
    let n=i32(lattice_view.config.x)>>level;
    let tile=cell>>vec2u(level);
    let packed=lattice_roofs[(n*n-1)/3+tile.y*n+tile.x];
    return f32(select(packed,(packed<<8u)>>8u,level==0u))*.0625;
}

fn lattice_interval_clear(directions:array<vec3f,4>, start:f32, end:f32) -> bool {
    let origin=lattice_view.eye.xz*(16./f32(LATTICE_WIDTH));
    var lo=vec2f(INF);
    var hi=vec2f(-INF);
    var bottom=INF;
    for(var i=0u;i<4u;i+=1u) {
        let d=directions[i];
        let p=fma(vec2f(start*(16./f32(LATTICE_WIDTH))),d.xz,origin);
        let q=fma(vec2f(end*(16./f32(LATTICE_WIDTH))),d.xz,origin);
        lo=min(lo,min(p,q));
        hi=max(hi,max(p,q));
        bottom=min(bottom,min(fma(start,d.y,lattice_view.eye.y),fma(end,d.y,lattice_view.eye.y)));
    }
    let lower_cell=vec2i(floor(lo-.002));
    let upper_cell=vec2i(floor(hi+.002));
    if any(lower_cell<vec2i(0)) || any(upper_cell>=vec2i(i32(lattice_view.config.x))) {return false;}
    let span=u32(max(upper_cell.x-lower_cell.x,upper_cell.y-lower_cell.y));
    let level=select(0u,firstLeadingBit(span)+1u,span>0u);
    let roof=max(
        max(lattice_ceiling(lower_cell,level),lattice_ceiling(vec2i(lower_cell.x,upper_cell.y),level)),
        max(lattice_ceiling(vec2i(upper_cell.x,lower_cell.y),level),lattice_ceiling(upper_cell,level))
    );
    return bottom>roof+.025;
}

@compute @workgroup_size(8,8)
fn lattice_find_prefix(@builtin(global_invocation_id) id:vec3u) {
    let pixels=(view.size.xy+LATTICE_BEAM_WIDTH-1u)/LATTICE_BEAM_WIDTH;
    if any(id.xy>=pixels) {return;}
    let lo=vec2f(id.xy*LATTICE_BEAM_WIDTH);
    let hi=vec2f(min((id.xy+1u)*LATTICE_BEAM_WIDTH,view.size.xy));
    let directions=array(lattice_screen_corner(lo),lattice_screen_corner(vec2f(lo.x,hi.y)),
        lattice_screen_corner(vec2f(hi.x,lo.y)),lattice_screen_corner(hi));
    var t=0.;
    var step=256.;
    for(var i=0u;i<24u;i+=1u) {
        let end=min(t+step,view.eye.w);
        if lattice_interval_clear(directions,t,end) {
            t=end;
            if end>=view.eye.w {break;}
            step*=2.;
        } else {
            step*=.5;
            if step<4.*f32(LATTICE_BEAM_WIDTH) {break;}
        }
    }
    lattice_write_prefix[id.y*pixels.x+id.x]=t;
}

struct LatticeSampleVertex {
    @builtin(position) position:vec4f,
    @location(0) @interpolate(linear, sample) ndc:vec2f,
}

@vertex
fn lattice_sample_triangle(@builtin(vertex_index) index:u32) -> LatticeSampleVertex {
    let clip=rolling_clip(index);
    return LatticeSampleVertex(clip,clip.xy);
}

@fragment
fn lattice_terrain_fragment(@builtin(position) pixel:vec4f) -> RollingFragment {
    let coordinate=vec2u(pixel.xy);
    return lattice_render(coordinate,screen_ray(coordinate),true);
}

@fragment
fn lattice_terrain_msaa(pixel:LatticeSampleVertex) -> RollingFragment {
    let direction=view.forward.xyz+pixel.ndc.x*view.right.xyz+pixel.ndc.y*view.up.xyz;
    return lattice_render(vec2u(pixel.position.xy),direction,false);
}

fn lattice_render(coordinate:vec2u,direction:vec3f,center_sample:bool) -> RollingFragment {
    let far=view.eye.w;
    let scale=length(direction);
    let foreground=foreground_at(coordinate);
    var ray_limit=far/scale;
    if foreground<1. {
        let padded=min(1.,bitcast<f32>(bitcast<u32>(foreground)+8u));
        let occluder=NEAR*far/max(NEAR,far-padded*(far-NEAR));
        ray_limit=min(ray_limit,occluder*1.0001+.01);
    }
    let surface=lattice_fragment(direction,NEAR,ray_limit,coordinate,center_sample);
    let t=surface.w;
    if t<0. {
        if t == -2. {return RollingFragment(vec4f(1.,0.,1.,1.),.999999);}
        discard;
    }
    let p=fma(vec3f(t),direction,view.eye.xyz);
    let owned=native_view.owner_min.w>.5 && all(p>native_view.owner_min.xyz) && all(p<native_view.owner_max.xyz);
    let depth=projected_depth(t,far);
    // Give coincident NAADF samples priority, while preserving the exact
    // first hit if the volume has no sample for this ray.
    let candidate=min(bitcast<f32>(0x3f7fffffu),bitcast<f32>(bitcast<u32>(depth)+select(0u,4u,owned)));
    if foreground<1. && candidate>=foreground {discard;}
    if CAPTURE_DISTANCE {
        return RollingFragment(vec4f(vec3f(t*scale),1.),candidate);
    }
    let fade=select(1.,1.-smoothstep(.73,.996,t*scale/far),view.forward.w>.5);
    let color=select(surface.rgb,vec3f(0.,1.,0.),(native_view.source.w&1u)!=0u);
    return RollingFragment(vec4f(color,fade),candidate);
}
