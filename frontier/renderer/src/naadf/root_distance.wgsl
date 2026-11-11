// A scratch tile uses logical coordinates. Wrap only when loading exact roots;
// expansion never crosses the tile or logical window edge.
struct Tile { root:vec4<u32>, ring:vec4<u32>, base:vec4<u32> };
@group(0) @binding(0) var exact:texture_3d<u32>;
@group(0) @binding(1) var<storage,read> input:array<u32>;
@group(0) @binding(2) var<storage,read_write> output:array<u32>;
@group(0) @binding(3) var cache:texture_storage_3d<r32uint,write>;
@group(0) @binding(4) var<uniform> tile:Tile;
const SIDE:u32=32u;
const OCCUPIED:u32=0xc0000000u;
fn index(p:vec3<u32>)->u32 {return p.x+SIDE*(p.y+SIDE*p.z);}

@compute @workgroup_size(4,4,4)
fn seed(@builtin(global_invocation_id) p:vec3<u32>) {
    let logical=tile.base.xyz+vec3(tile.root.y)+p;
    var word=OCCUPIED;
    if all(logical<vec3(tile.root.x)) {
        let slot=(logical+tile.ring.xyz)%vec3(tile.root.x);
        word=textureLoad(exact,vec3<i32>(slot.yxz),0).x&OCCUPIED;
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
    textureStore(cache,vec3<i32>(tile.base.xyz+p),vec4(input[index(p)]&~OCCUPIED,0u,0u,0u));
}
