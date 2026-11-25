// One workgroup/profile. Integer face/material counts are order independent;
// palette-weighted final sums intentionally differ from ordered CPU rounding.
// Lossless 20-bit CPU ABI: 11-bit height and three 3-bit materials.
struct Column { height:i32, deep:u32, soil:u32, top:u32 }
fn unpack_column(bits:u32) -> Column {
    return Column(i32(bits & 2047u), (bits >> 11u) & 7u,
        (bits >> 14u) & 7u, (bits >> 17u) & 7u);
}
fn column_height(bits:u32) -> i32 { return i32(bits & 2047u); }
struct Job { page:u32, x:u32, z:u32, width:u32, depth:u32, stride:u32, pad0:u32, pad1:u32 }
struct Profile { faces:array<vec4<f32>,5> }
@group(0) @binding(0) var<storage,read> columns:array<u32>;
@group(0) @binding(1) var<storage,read> jobs:array<Job>;
@group(0) @binding(2) var<storage,read_write> profiles:array<Profile>;
const COLORS:array<vec3<f32>,5> = array<vec3<f32>,5>(vec3(0.),vec3(.18,.34,.10),vec3(.32,.22,.13),vec3(.35,.37,.39),vec3(.41,.25,.13));
var<workgroup> counts:array<atomic<u32>,25>;
fn add(axis:u32,material:u32,n:u32,weight:u32) {
    if n>0u {atomicAdd(&counts[axis*5u+material],n*weight);}
}
@compute @workgroup_size(64)
fn main(@builtin(workgroup_id) group:vec3<u32>,@builtin(local_invocation_index) lane:u32) {
    if lane<25u {atomicStore(&counts[lane],0u);}
    workgroupBarrier();
    let j=jobs[group.x];
    let sample_width=j.width/j.stride;
    let samples=sample_width*(j.depth/j.stride);
    let weight=j.stride*j.stride;
    for(var sample=lane;sample<samples;sample+=64u) {
        let x=j.x+(sample%sample_width)*j.stride;
        let z=j.z+(sample/sample_width)*j.stride;
        let i=j.page*16900u+(z+1u)*130u+x+1u;
        let c=unpack_column(columns[i]);
        add(0u,c.top,1u,weight);
        let neighbors=array<i32,4>(column_height(columns[i+1u]),column_height(columns[i-1u]),column_height(columns[i+130u]),column_height(columns[i-130u]));
        for(var axis=0u;axis<4u;axis++) {
            let lo=neighbors[axis];
            add(axis+1u,c.deep,u32(max(0,c.height-12-lo)),weight);
            add(axis+1u,c.soil,u32(max(0,c.height-1-max(lo,c.height-12))),weight);
            add(axis+1u,c.top,u32(max(0,c.height-max(lo,c.height-1))),weight);
        }
    }
    workgroupBarrier();
    if lane<5u {
        var rgb=vec3<f32>(0.);
        var area=0u;
        for(var material=0u;material<5u;material++) {
            let count=atomicLoad(&counts[lane*5u+material]);
            rgb+=COLORS[material]*f32(count);
            area+=count;
        }
        profiles[group.x+1u].faces[lane]=vec4(rgb,f32(area));
    }
}
