// A branch stores an upper bound. The only reported surfaces are exact columns.
struct View {
    eye: vec4f,
    forward: vec4f,
    right: vec4f,
    up: vec4f,
    size: vec4u, // width, height, root log2 and page log2 (native cells)
    camera_cell: vec4i,
}
struct Node {
    ceiling: f32,
    link: u32,   // zero for missing, high bit for a native page
    parent: u32,
}
@group(0) @binding(0) var<uniform> view: View;
@group(0) @binding(1) var<storage,read> nodes: array<Node>;
@group(0) @binding(2) var<storage,read> columns: array<i32>;
@group(0) @binding(3) var<storage,read_write> pixels: array<vec2u>;
@group(0) @binding(4) var<storage,read_write> requests_ahead: array<vec2u>;
@group(0) @binding(5) var<storage,read_write> resume_from: array<vec2u>;

override STACKLESS: bool = false;
override BEAMS: bool = false;
override RESUME: bool = false;
override SPARSE: bool = false;
override LOOKAHEAD: u32 = 0u;
var<workgroup> empty_prefix:f32;
var<workgroup> first_unresolved:atomic<u32>;
var<workgroup> remaining:atomic<u32>;
const SKY = -1.;
const FAILED = -3.;
const INF = 1e20;
const PAGE = 0x80000000u;

fn ray_vector(pixel:vec2f) -> vec3f {
    let uv=pixel*2./vec2f(view.size.xy)-1.;
    return view.forward.xyz+uv.x*view.right.xyz-uv.y*view.up.xyz;
}

fn ceiling_at(cell:vec2i, wanted_level:u32) -> f32 {
    var level=view.size.z;
    var id=0u;
    for(var depth=0u;depth<19u;depth+=1u) {
        let node=nodes[id];
        if level<=wanted_level || node.link==0u || node.link>=PAGE {return node.ceiling;}
        level-=1u;
        let q=vec2u((cell>>vec2u(level))&vec2i(1));
        id=node.link+q.x+2u*q.y;
    }
    return nodes[id].ceiling;
}

fn empty_slab(corners:array<vec3f,4>, start:f32, end:f32) -> bool {
    var lo=vec2f(INF);
    var hi=vec2f(-INF);
    var bottom=INF;
    let origin=vec2f(view.camera_cell.xy)+view.eye.xz;
    for(var i=0u;i<4u;i+=1u) {
        let d=corners[i];
        let p=fma(vec2f(16.*start),d.xz,origin);
        let q=fma(vec2f(16.*end),d.xz,origin);
        lo=min(lo,min(p,q));
        hi=max(hi,max(p,q));
        bottom=min(bottom,min(fma(start,d.y,view.eye.y),fma(end,d.y,view.eye.y)));
    }
    // Beam and individual rays use different parameterizations. Expand the
    // box to cover the floating-point coordinate and direction differences.
    let limit=vec2f(f32((1u<<view.size.z)-1u));
    let a=vec2i(clamp(floor(lo-vec2f(.25)),vec2f(0.),limit));
    let b=vec2i(clamp(floor(hi+vec2f(.25)),vec2f(0.),limit));
    let span=u32(max(b.x-a.x,b.y-a.y));
    let level=select(0u,firstLeadingBit(span)+1u,span>0u);
    let max_height=max(
        max(ceiling_at(a,level),ceiling_at(vec2i(a.x,b.y),level)),
        max(ceiling_at(vec2i(b.x,a.y),level),ceiling_at(b,level))
    );
    return bottom>max_height+.015;
}

fn beam_start(pixel:vec2u) -> f32 {
    let a=vec2f(pixel);
    let b=vec2f(min(pixel+vec2u(8),view.size.xy));
    let corners=array(ray_vector(a),ray_vector(vec2f(a.x,b.y)),
        ray_vector(vec2f(b.x,a.y)),ray_vector(b));
    var start=0.;
    var length=32.;
    for(var i=0u;i<32u;i+=1u) {
        let end=min(start+length,view.eye.w*1.00001);
        if empty_slab(corners,start,end) {
            start=end;
            if end>=view.eye.w {break;}
            length*=2.;
        } else {
            length*=.5;
            if length<4. {break;}
        }
    }
    return start;
}

struct Ray {
    origin: vec2i,    // mirrored horizontal native coordinates
    fraction: vec2f,
    velocity: vec2f,  // native cells per metre along the ray
    inverse: vec2f,
    sign: vec2i,
    side: i32,
}
fn exit_times(ray:Ray,edge:vec2i) -> vec2f {
    return select((vec2f(edge-ray.origin)-ray.fraction)*ray.inverse,vec2f(INF),ray.velocity==vec2f(0.));
}
fn column_at(ray:Ray,t:f32) -> vec2i {
    let guess=vec2i(floor(fma(vec2f(t),ray.velocity,ray.fraction)))+ray.origin;
    let previous = (exit_times(ray,guess)>vec2f(t)) & (ray.velocity>vec2f(0.));
    let following = exit_times(ray,guess+1)<=vec2f(t);
    return guess+select(vec2i(0),vec2i(1),following)-select(vec2i(0),vec2i(1),previous);
}
fn advance_column(ray:Ray,edge:vec2i,t:f32) -> vec2i {
    let crossed=exit_times(ray,edge)<=vec2f(t);
    return select(min(column_at(ray,t),edge-1),edge,crossed);
}
fn entry_face(ray:Ray,exits:vec2f) -> u32 {
    if exits.x<=exits.y {return select(2u,1u,ray.sign.x>0);}
    return select(4u,3u,ray.sign.y>0);
}
fn world_column(ray:Ray,fine:vec2i) -> vec2i {
    return select(vec2i(ray.side-1)-fine,fine,ray.sign>vec2i(0));
}
fn result(distance:f32,payload:u32) -> vec2u {
    return vec2u(bitcast<u32>(distance), payload);
}

struct PageBlock {
    offset: u32,
    bits: u32,
    height_origin: i32,
    tiled: bool,
}
fn read_block(offset:u32, width:u32, bits:u32, block_coord:vec2u) -> PageBlock {
    let blocks=width>>2u;
    let count=blocks*blocks;
    let header=offset+2u+count;
    if bits!=0u {return PageBlock(header,bits,columns[offset],false);}
    let block=block_coord.y*blocks+block_coord.x;
    let lane=block&31u;
    let group=header+2u*(block>>5u);
    let mask=u32(columns[group+1u]);
    let wider=countOneBits(mask&((1u<<lane)-1u));
    let payload=header+2u*((count+31u)>>5u)+u32(columns[group])+4u*(lane+wider);
    let bit_count=8u<<((mask>>lane)&1u);
    return PageBlock(payload,bit_count,columns[offset+2u+block],true);
}
fn read_column(block:PageBlock, width:u32, native:vec2u) -> i32 {
    let local=native&vec2u(3u);
    let index=select(native.y*width+native.x,local.y*4u+local.x,block.tiled);
    let bits=block.bits;
    if bits==32u {return columns[block.offset+index];}
    let per_word=32u/bits;
    let word=u32(columns[block.offset+index/per_word]);
    let value=(word>>(bits*(index%per_word)))&((1u<<bits)-1u);
    let delta=i32(value>>3u);
    return ((block.height_origin+select(delta,-delta,block.tiled))<<8u)|i32(value&7u);
}
fn trace(direction:vec3f, safe_start:f32, enter_face:u32, pixel:u32, request_base:u32, request_limit:u32) -> vec2u {
    let root=view.size.z;
    let page_log=view.size.w;
    let side=1i<<root;
    let origin=view.camera_cell.xy;
    let sign=select(vec2i(1),vec2i(-1),direction.xz<vec2f(0.));
    let v=abs(direction.xz)*16.;
    let ray=Ray(select(vec2i(side)-origin,origin,sign>vec2i(0)),
        view.eye.xz*vec2f(sign),v,
        select(1./v,vec2f(INF),v==vec2f(0.)),sign,side);
    // The camera contract places the entire far sphere within the root.
    var start=max(.025,safe_start);
    let stop=view.eye.w;
    var fine=clamp(column_at(ray,start),vec2i(0),vec2i(side-1));
    var face=enter_face;
    var first_missing=vec2u(0);
    var written=0u;
    var stack:array<u32,19>;
    var level=root;
    var id=0u;
    if !STACKLESS { stack[root]=0u; }
    for(var step=0u;step<16384u;step+=1u) {
        let edge=((fine>>vec2u(level))+1)<<vec2u(level);
        let exits=exit_times(ray,edge);
        let end=max(start,min(stop,min(exits.x,exits.y)));
        let node=nodes[id];
        let y0=fma(start,direction.y,view.eye.y);
        let y1=fma(end,direction.y,view.eye.y);
        if min(y0,y1)<=node.ceiling && end>start {
            if node.link==0u && !(SPARSE && level==page_log) {
                let missing=result(min(y0,y1),PAGE|id);
                if first_missing.y==0u {
                    first_missing=missing;
                    if RESUME {resume_from[pixel]=result(start,face);}
                    if request_limit==0u {return first_missing;}
                } else {
                    requests_ahead[request_base+written]=missing;
                    written+=1u;
                    if written==request_limit {return first_missing;}
                }
            } else if node.link!=0u && node.link<PAGE {
                level-=1u;
                let which=vec2u((world_column(ray,fine)>>vec2u(level))&vec2i(1));
                id=node.link+which.x+2u*which.y;
                if !STACKLESS { stack[level]=id; }
                continue;
            } else {
            // Page maxima and the 4x4 block maxima are exact reductions of
            // the resident columns. Neither changes the surface we intersect.
            let width=1u<<page_log;
            let mask=vec2i(i32(width)-1);
            let offset=node.link&~PAGE;
            var bits=0u;
            if !SPARSE {bits=u32(columns[offset+1u]);}
            let blocks=width>>2u;
            let first=fine;
            for(var block=0u;block<2u*blocks+2u;block+=1u) {
                let block_edge=((fine>>vec2u(2u))+1)<<vec2u(2u);
                let block_exits=exit_times(ray,block_edge);
                let block_end=min(end,max(start,min(block_exits.x,block_exits.y)));
                let native=vec2u(world_column(ray,fine)&mask);
                let block_coord=native>>vec2u(2u);
                let block_index=block_coord.y*blocks+block_coord.x;
                var roof=node.ceiling;
                var block_offset=offset;
                var block_width=width;
                var block_bits=bits;
                var pending_block=false;
                if SPARSE {
                    var code=0u;
                    if node.link!=0u {code=u32(columns[offset+block_index]);}
                    pending_block=code==0u || code>=PAGE;
                    if code>=PAGE {
                        let height=i32(code<<8u)>>8u;
                        roof=f32(height)*.0625+.003;
                    } else if code!=0u {
                        block_offset=code-1u;
                        block_width=4u;
                        block_bits=u32(columns[block_offset+1u]);
                        roof=f32(columns[block_offset+2u])*.0625+.003;
                    }
                } else {
                    roof=f32(columns[offset+2u+block_index])*.0625+.003;
                }
                let block_y=min(fma(start,direction.y,view.eye.y),fma(block_end,direction.y,view.eye.y));
                if block_y<=roof && block_end>start && pending_block {
                    let node_bits=select(23u,22u,page_log==6u);
                    let missing=result(block_y,PAGE|((block_index+1u)<<node_bits)|id);
                    if first_missing.y==0u {
                        first_missing=missing;
                        if RESUME {resume_from[pixel]=result(start,face);}
                        if request_limit==0u {return first_missing;}
                    } else {
                        requests_ahead[request_base+written]=missing;
                        written+=1u;
                        if written==request_limit {return first_missing;}
                    }
                }
                if block_y<=roof && block_end>start && !pending_block {
                    let encoding=read_block(block_offset,block_width,block_bits,block_coord);
                    for(var leaf=0u;leaf<10u;leaf+=1u) {
                        let next_exits=exit_times(ray,fine+1);
                        let t=min(block_end,max(start,min(next_exits.x,next_exits.y)));
                        let index=vec2u(world_column(ray,fine)&mask);
                        let column=read_column(encoding,block_width,index&vec2u(block_width-1u));
                        let height=f32(column>>8)*.0625;
                        if t>start {
                            if fma(start,direction.y,view.eye.y)<=height {
                                return select(result(start,u32(column&255)|(face<<8u)),first_missing,first_missing.y!=0u);
                            }
                            if direction.y<0. && fma(t,direction.y,view.eye.y)<=height {
                                return select(result(clamp((height-view.eye.y)/direction.y,start,t),u32(column&255)),first_missing,first_missing.y!=0u);
                            }
                        }
                        if t>=block_end {break;}
                        face=entry_face(ray,next_exits);
                        fine=advance_column(ray,fine+1,t);
                        start=t;
                    }
                }
                if block_end>=end {break;}
                face=entry_face(ray,block_exits);
                fine=advance_column(ray,block_edge,block_end);
                start=block_end;
                if any((first>>vec2u(page_log))!=(fine>>vec2u(page_log))) {
                    return select(result(FAILED,0u),first_missing,first_missing.y!=0u);
                }
            }
            }
        }
        if end>=stop {return select(result(SKY,0u),first_missing,first_missing.y!=0u);}
        let next=advance_column(ray,edge,end);
        if any(next>=vec2i(side)) {return select(result(FAILED,0u),first_missing,first_missing.y!=0u);}
        let difference=u32((fine.x^next.x)|(fine.y^next.y));
        if difference==0u {return select(result(FAILED,0u),first_missing,first_missing.y!=0u);}
        let ancestor=min(root,firstLeadingBit(difference)+1u);
        if STACKLESS {
            for (var up=level;up<ancestor;up+=1u) { id=nodes[id].parent; }
        } else {
            id=stack[ancestor];
        }
        level=ancestor;
        start=end;
        fine=next;
        face=entry_face(ray,exits);
    }
    return select(result(FAILED,0u),first_missing,first_missing.y!=0u);
}
@compute @workgroup_size(8,8)
fn visibility(@builtin(global_invocation_id) id:vec3u, @builtin(local_invocation_index) lane:u32) {
    if BEAMS {
        if lane==0u {empty_prefix=beam_start(id.xy);}
        workgroupBarrier();
    }
    let valid=all(id.xy<view.size.xy);
    let pixel=id.y*view.size.x+id.x;
    var requester=0u;
    var limit=u32(view.camera_cell.w);
    if SPARSE {limit=min(limit,8u);}
    if LOOKAHEAD>0u && RESUME && view.camera_cell.z!=0 {
        if lane==0u {
            atomicStore(&first_unresolved,64u);
            if SPARSE {atomicStore(&remaining,0u);}
        }
        workgroupBarrier();
        if valid && pixels[pixel].y>=PAGE {
            atomicMin(&first_unresolved,lane);
            if SPARSE {atomicAdd(&remaining,1u);}
        }
        workgroupBarrier();
        requester=atomicLoad(&first_unresolved);
        if SPARSE {limit=min(u32(view.camera_cell.w),max(8u,64u/max(1u,atomicLoad(&remaining))));}
    }
    if !valid {return;}
    let ray=ray_vector(vec2f(id.xy)+.5);
    var prefix=select(0.,empty_prefix*length(ray),BEAMS);
    var face=0u;
    if RESUME && view.camera_cell.z!=0 {
        if pixels[pixel].y<PAGE {return;}
        let previous=resume_from[pixel];
        let distance=bitcast<f32>(previous.x);
        if distance>=prefix {
            prefix=distance;
            face=previous.y;
        }
    }
    let tile=id.xy/vec2u(8);
    let tiles_x=(view.size.x+7u)/8u;
    let base=(tile.y*tiles_x+tile.x)*LOOKAHEAD;
    pixels[pixel]=trace(normalize(ray),prefix,face,pixel,base,select(0u,limit,lane==requester));
}
