// A scratch tile uses logical coordinates. Wrap only when loading exact roots;
// new expansion only reads neighbors inside the tile. Repair mode may start
// from larger, independently proven CPU/GPU cuboids crossing a tile boundary;
// each face update only grows that proof.
struct Tile {
    root:vec4<u32>, ring:vec4<u32>, base:vec4<u32>,
    schedule:vec4<u32>, eye:vec4<f32>, forward:vec4<f32>,
    right:vec4<f32>, up:vec4<f32>,
};
@group(0) @binding(0) var exact:texture_3d<u32>;
@group(0) @binding(1) var<storage,read> input:array<u32>;
@group(0) @binding(2) var<storage,read_write> output:array<u32>;
@group(0) @binding(3) var cache:texture_storage_3d<r32uint,write>;
@group(0) @binding(4) var<uniform> tile:Tile;
@group(0) @binding(5) var<storage,read_write> demand:array<atomic<u32>,64>;
@group(0) @binding(6) var<storage,read_write> original:array<u32>;
const SIDE:u32=32u;
const OCCUPIED:u32=0xc0000000u;
fn index(p:vec3<u32>)->u32 {return p.x+SIDE*(p.y+SIDE*p.z);}

@compute @workgroup_size(4,4,4)
fn seed(@builtin(global_invocation_id) p:vec3<u32>) {
    let logical=bitcast<vec3<i32>>(tile.base.xyz)+vec3<i32>(p);
    var word=OCCUPIED;
    if all(logical>=vec3(0)) && all(logical<vec3<i32>(i32(tile.root.x))) {
        let slot=(vec3<u32>(logical)+tile.ring.xyz)%vec3(tile.root.x);
        let raw=textureLoad(exact,vec3<i32>(slot.yxz),0).x;
        var exact_word=raw;
        if (tile.ring.w&2u)!=0u {
            original[index(p)]=raw;
            if (raw&OCCUPIED)==OCCUPIED {exact_word=raw&~OCCUPIED;}
        }
        word=select(exact_word&OCCUPIED,exact_word,(tile.ring.w&1u)!=0u);
    }
    output[index(p)]=word;
}

fn expand(p:vec3<u32>,axis:u32) {
    let old=input[index(p)];
    var result=old;
    if (old&OCCUPIED)==0u {
        for (var side=axis*2u;side<axis*2u+2u;side++) {
            let shift=side*5u;
            let extent=(old>>shift)&31u;
            let positive=(side&1u)!=0u;
            let distance=select(-i32(extent+1u),i32(extent+1u),positive);
            let coordinate=i32(p[axis])+distance;
            if coordinate<0 || coordinate>=i32(SIDE) {continue;}
            var adjacent=p;adjacent[axis]=u32(coordinate);
            let neighbor=input[index(adjacent)];
            if (neighbor&OCCUPIED)!=0u {continue;}
            var covers=true;
            for (var t=0u;t<6u;t++) {
                let k=t*5u;
                if t/2u!=axis && ((neighbor>>k)&31u)<((old>>k)&31u) {covers=false;}
            }
            if covers {
                // The adjacent proof covers every corner of the added slab.
                let available=select(p[axis],SIDE-1u-p[axis],positive);
                let grown=min(available,extent+1u+((neighbor>>shift)&31u));
                result=(result&~(31u<<shift))|(grown<<shift);
            }
        }
    }
    output[index(p)]=result;
}

@compute @workgroup_size(4,4,4)
fn expand_x(@builtin(global_invocation_id) p:vec3<u32>) {expand(p,0u);}
@compute @workgroup_size(4,4,4)
fn expand_y(@builtin(global_invocation_id) p:vec3<u32>) {expand(p,1u);}
@compute @workgroup_size(4,4,4)
fn expand_z(@builtin(global_invocation_id) p:vec3<u32>) {expand(p,2u);}

@compute @workgroup_size(4,4,4)
fn publish(@builtin(global_invocation_id) p:vec3<u32>) {
    textureStore(cache,vec3<i32>(tile.root.yzw+p),vec4(input[index(p)]&~OCCUPIED,0u,0u,0u));
}

// 64 root-resolution primary rays, sampled on a regular 8x8 pixel grid.
// Full and mixed roots stop the probe. This deliberately undercounts demand
// through mixed roots rather than walking the much larger voxel hierarchy.
@compute @workgroup_size(4,4)
fn sample_feedback(@builtin(global_invocation_id) sample:vec3<u32>) {
    let ndc=(vec2<f32>(sample.xy)+0.5)/4.-1.;
    let o=tile.eye.xyz;
    let d=normalize(tile.forward.xyz+ndc.x*tile.right.xyz*tile.right.w
        +ndc.y*tile.up.xyz*tile.up.w);
    let n=tile.root.x;
    var t=0.;
    var end=tile.eye.w;
    for (var a=0u; a<3u; a++) {
        if abs(d[a])<1e-7 {
            if o[a]<0. || o[a]>=f32(n) { return; }
        } else {
            let t0=-o[a]/d[a];
            let t1=(f32(n)-o[a])/d[a];
            t=max(t,min(t0,t1));
            end=min(end,max(t0,t1));
        }
    }
    if t>=end { return; }
    let shifts=vec3(0u,8u,16u);
    let packed=vec3<i32>((vec3(tile.schedule.z)>>shifts)&vec3(255u));
    let center=vec3(i32((n-min(n,128u))/2u));
    let first=center+select(packed,packed-vec3(256),packed>=vec3(128));
    let offset=(vec3(tile.schedule.w)>>shifts)&vec3(255u);
    let side=tile.schedule.w>>24u;
    var cp=clamp(vec3<i32>(floor(o+d*(t+0.0001))),vec3(0),vec3(i32(n)-1));
    var last=64u;
    var visits=0u;
    for(var i=0u; i<3u*n; i++) {
        if any(cp<vec3(0)) || any(cp>=vec3(i32(n))) { break; }
        let root=(vec3<u32>(cp)+tile.ring.xyz)%vec3(n);
        var word=textureLoad(exact,vec3<i32>(root.yxz),0).x;
        if (tile.ring.w&2u)!=0u && (word&OCCUPIED)==OCCUPIED {word&=~OCCUPIED;}
        if (word&OCCUPIED)!=0u { break; }
        let p=cp-first;
        var id=64u;
        if all(p>=vec3(0)) && all(p<vec3(i32(side*32u))) {
            let slot=(vec3<u32>(p)/32u+offset)%vec3(side);
            let index=slot.x+side*(slot.y+side*slot.z);
            if (tile.schedule[index/32u]&(1u<<(index%32u)))==0u {id=index;}
        }
        if id!=last {
            if visits>0u { atomicAdd(&demand[last],visits); }
            last=id;
            visits=0u;
        }
        if id<64u { visits+=1u; }
        var crossing=vec3(1e30);
        var lo=cp;
        var hi=cp+vec3(1);
        for (var a=0u; a<3u; a++) {
            if tile.base.x!=0u {
                // Hybrid mode measures fallback decisions, using the same
                // CPU root proof as an unready tile in the primary shader.
                lo[a]=max(0,lo[a]-i32((word>>(a*10u))&31u));
                hi[a]=min(i32(n),hi[a]+i32((word>>(a*10u+5u))&31u));
            }
            if abs(d[a])>=1e-7 { crossing[a]=(f32(select(lo[a],hi[a],d[a]>0.))-o[a])/d[a]; }
        }
        let next=min(crossing.x,min(crossing.y,crossing.z));
        if next<t || next>=end { break; }
        let hit=o+d*next;
        let f=floor(hit);
        cp=clamp(vec3<i32>(f)-vec3<i32>(select(vec3(0),vec3(1),(d<vec3(0.))&(hit==f))),lo,hi-vec3(1));
        for (var a=0u; a<3u; a++) {
            if crossing[a]==next { cp[a]=select(lo[a]-1,hi[a],d[a]>0.); }
        }
        t=next;
    }
    if visits>0u { atomicAdd(&demand[last],visits); }
}
