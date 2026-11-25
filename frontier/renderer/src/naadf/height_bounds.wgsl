// Reduce exact occupied roots, including mixed roots for plants and edits.
// Conservative even when a mixed root contains just one occupied native cell.
struct HeightBuild { n:u32, sparse:u32, _padding:vec2<u32>, ring:vec4<u32> };
@group(0) @binding(0) var<uniform> settings:HeightBuild;
@group(0) @binding(1) var height_roots:texture_3d<u32>;
@group(0) @binding(2) var height_out:texture_storage_2d<r32uint,write>;
override height_tile_roots:u32=8u;
var<workgroup> highest:atomic<u32>;
fn height_sparse_word(address:u32, shape:vec3<u32>)->u32 {
    let row=address/shape.x;
    return textureLoad(height_roots,vec3<i32>(vec3(address%shape.x,row%shape.y,row/shape.y)),0).x;
}
fn height_word_occupied(word:u32)->bool {
    let kind=word&0xc0000000u;
    // Both flags mark a GPU-grown empty root proof, not occupied geometry.
    return kind==0x80000000u || kind==0x40000000u;
}
fn height_root_occupied(cp:vec3<u32>)->bool {
    if settings.sparse==0u {
        let address=cp+settings.ring.xyz;
        let slot=select(address,address-vec3(settings.n),address>=vec3(settings.n));
        return height_word_occupied(textureLoad(height_roots,vec3<i32>(slot.yxz),0).x);
    }
    let shape=textureDimensions(height_roots,0);
    let side=(settings.n+3u)/4u;
    let page=cp/4u;
    let entry=height_sparse_word(page.y+side*(page.x+side*page.z),shape);
    if (entry&0x80000000u)==0u {return (entry&0x40000000u)!=0u;}
    let local=cp%4u;
    let address=side*side*side+(entry&0x3fffffffu)+local.y+4u*(local.x+4u*local.z);
    return height_word_occupied(height_sparse_word(address,shape));
}
@compute @workgroup_size(height_tile_roots,height_tile_roots)
fn main(@builtin(workgroup_id) group:vec3<u32>,@builtin(local_invocation_id) id:vec3<u32>,@builtin(local_invocation_index) index:u32) {
    if index==0u {atomicStore(&highest,0u);}
    workgroupBarrier();
    let column=group.xy*height_tile_roots+id.xy;
    if all(column<vec2(settings.n)) {
        var y=settings.n;
        loop {
            if y==0u {break;}
            // Other invocations may already have proved a higher maximum.
            if y*16u<=atomicLoad(&highest) {break;}
            if height_root_occupied(vec3(column.x,y-1u,column.y)) {
                atomicMax(&highest,y*16u);
                break;
            }
            y-=1u;
        }
    }
    workgroupBarrier();
    if index==0u {textureStore(height_out,vec2<i32>(group.xy),vec4(atomicLoad(&highest),0u,0u,0u));}
}
