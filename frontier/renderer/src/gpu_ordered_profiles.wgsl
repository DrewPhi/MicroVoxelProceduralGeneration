// Bounded ordered construction, not a parallel floating-point reduction.
// One lane owns each profile and preserves CPU z/x/axis/material-span order.
// Columns are canonical CPU cache values; this shader never evaluates terrain.
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
fn add(p:ptr<function,Profile>, axis:u32, material:u32, cells:i32, weight:f32) {
    let a=f32(cells)*weight;
    // Separate expression order mirrors CPU; hardware parity is still a gate.
    (*p).faces[axis]+=vec4(COLORS[material]*a,a);
}
@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id:vec3<u32>) {
    if id.x>=arrayLength(&jobs) {return;}
    let j=jobs[id.x];
    var p:Profile;
    let weight=f32(j.stride*j.stride);
    for(var z=j.z;z<j.z+j.depth;z+=j.stride) {
        for(var x=j.x;x<j.x+j.width;x+=j.stride) {
            let i=j.page*16900u+(z+1u)*130u+x+1u;
            let c=unpack_column(columns[i]);
            add(&p,0u,c.top,1,weight);
            let neighbors=array<i32,4>(column_height(columns[i+1u]),column_height(columns[i-1u]),column_height(columns[i+130u]),column_height(columns[i-130u]));
            for(var axis=0u;axis<4u;axis++) {
                var lo=neighbors[axis];
                let ends=array<i32,3>(c.height-12,c.height-1,c.height);
                for(var span=0u;span<3u;span++) {
                    let hi=ends[span];
                    if hi>lo {
                        var m=c.deep;
                        if lo==c.height-1 {m=c.top;} else if lo>=c.height-12 {m=c.soil;}
                        add(&p,axis+1u,m,hi-lo,weight);
                        lo=hi;
                    }
                }
            }
        }
    }
    // Slot zero is the immutable exact-face fallback; draw IDs start at one.
    profiles[id.x+1u]=p;
}
