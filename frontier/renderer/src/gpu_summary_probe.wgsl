// Experimental exact per-patch face counts; CPU canonical heights/materials only.
const PATCH_WIDTH: u32 = 16u;
const PATCH_SIDE: u32 = 128u / PATCH_WIDTH;
const PATCHES: u32 = PATCH_SIDE * PATCH_SIDE;
struct Column { height: i32, deep: u32, soil: u32, top: u32 }
@group(0) @binding(0) var<storage, read> columns: array<Column>;
@group(0) @binding(1) var<storage, read_write> counts: array<u32>;
var<workgroup> partial: array<atomic<u32>, 25>;
fn add(axis:u32, material:u32, n:u32) { if n>0u { atomicAdd(&partial[axis*5u+material],n); } }
@compute @workgroup_size(64)
fn main(@builtin(workgroup_id) group:vec3<u32>, @builtin(local_invocation_index) lane:u32) {
    if lane<25u {atomicStore(&partial[lane],0u);}
    workgroupBarrier();
    let patch_x=(group.x%PATCH_SIDE)*PATCH_WIDTH;
    let patch_z=(group.x/PATCH_SIDE)*PATCH_WIDTH;
    for(var cell=lane;cell<PATCH_WIDTH*PATCH_WIDTH;cell+=64u) {
        let x=patch_x+cell%PATCH_WIDTH+1u; let z=patch_z+cell/PATCH_WIDTH+1u;
        let index=group.y*16900u+z*130u+x;
        let c=columns[index];
        add(0u,c.top,1u);
        let neighbors=array<i32,4>(columns[index+1u].height,columns[index-1u].height,columns[index+130u].height,columns[index-130u].height);
        for(var axis=0u;axis<4u;axis++) {
            let lo=neighbors[axis];
            add(axis+1u,c.deep,u32(max(0,c.height-12-lo)));
            add(axis+1u,c.soil,u32(max(0,(c.height-1)-max(lo,c.height-12))));
            add(axis+1u,c.top,u32(max(0,c.height-max(lo,c.height-1))));
        }
    }
    workgroupBarrier();
    if lane<25u {counts[(group.y*PATCHES+group.x)*25u+lane]=atomicLoad(&partial[lane]);}
}
