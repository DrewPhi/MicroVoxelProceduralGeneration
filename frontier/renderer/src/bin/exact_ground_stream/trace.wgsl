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

override STACKLESS: bool = false;
const SKY = -1.;
const PENDING = -2.;
const FAILED = -3.;
const INF = 1e20;
const PAGE = 0x80000000u;

fn ray_direction(pixel:vec2u) -> vec3f {
    let uv=(vec2f(pixel)+.5)*2./vec2f(view.size.xy)-1.;
    return normalize(view.forward.xyz+uv.x*view.right.xyz-uv.y*view.up.xyz);
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

fn read_column(offset:u32, header:u32, index:u32, bits:u32) -> i32 {
    if bits==32u {return columns[offset+header+index];}
    let per_word=32u/bits;
    let word=u32(columns[offset+header+index/per_word]);
    let value=(word>>(bits*(index%per_word)))&((1u<<bits)-1u);
    return ((i32(value>>3u)+columns[offset])<<8u)|i32(value&7u);
}
fn trace(direction:vec3f) -> vec2u {
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
    var start=.025;
    let stop=view.eye.w;
    var fine=clamp(column_at(ray,start),vec2i(0),vec2i(side-1));
    var face=0u;
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
            if node.link==0u {return result(PENDING,id);}
            if node.link<PAGE {
                level-=1u;
                let which=vec2u((world_column(ray,fine)>>vec2u(level))&vec2i(1));
                id=node.link+which.x+2u*which.y;
                if !STACKLESS { stack[level]=id; }
                continue;
            }
            // Page maxima and the 4x4 block maxima are exact reductions of
            // the resident columns. Neither changes the surface we intersect.
            let width=1u<<page_log;
            let mask=vec2i(i32(width)-1);
            let offset=node.link&~PAGE;
            let bits=u32(columns[offset+1u]);
            let blocks=width>>2u;
            let header=2u+blocks*blocks;
            let first=fine;
            for(var block=0u;block<2u*blocks+2u;block+=1u) {
                let block_edge=((fine>>vec2u(2u))+1)<<vec2u(2u);
                let block_exits=exit_times(ray,block_edge);
                let block_end=min(end,max(start,min(block_exits.x,block_exits.y)));
                let native=vec2u(world_column(ray,fine)&mask);
                let block_coord=native>>vec2u(2u);
                let block_max=f32(columns[offset+2u+block_coord.y*blocks+block_coord.x])*.0625;
                let block_y=min(fma(start,direction.y,view.eye.y),fma(block_end,direction.y,view.eye.y));
                if block_y<=block_max+.003 && block_end>start {
                    for(var leaf=0u;leaf<10u;leaf+=1u) {
                        let next_exits=exit_times(ray,fine+1);
                        let t=min(block_end,max(start,min(next_exits.x,next_exits.y)));
                        let index=vec2u(world_column(ray,fine)&mask);
                        let column=read_column(offset,header,index.y*width+index.x,bits);
                        let height=f32(column>>8)*.0625;
                        if t>start {
                            if fma(start,direction.y,view.eye.y)<=height {
                                return result(start,u32(column&255)|(face<<8u));
                            }
                            if direction.y<0. && fma(t,direction.y,view.eye.y)<=height {
                                return result(clamp((height-view.eye.y)/direction.y,start,t),u32(column&255));
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
                    return result(FAILED,0u);
                }
            }
        }
        if end>=stop {return result(SKY,0u);}
        let next=advance_column(ray,edge,end);
        if any(next>=vec2i(side)) {return result(FAILED,0u);}
        let difference=u32((fine.x^next.x)|(fine.y^next.y));
        if difference==0u {return result(FAILED,0u);}
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
    return result(FAILED,0u);
}
@compute @workgroup_size(8,8)
fn visibility(@builtin(global_invocation_id) id:vec3u) {
    if any(id.xy>=view.size.xy) {return;}
    pixels[id.y*view.size.x+id.x]=trace(ray_direction(id.xy));
}
