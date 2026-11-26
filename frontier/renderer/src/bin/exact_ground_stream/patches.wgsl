@group(0) @binding(0) var<storage,read> entries: array<vec2u>;
@group(0) @binding(1) var<storage,read_write> pages: array<u32>;
@group(0) @binding(2) var<uniform> count: u32;
@compute @workgroup_size(64)
fn update_tables(@builtin(global_invocation_id) id:vec3u) {
    if id.x>=count {return;}
    let entry=entries[id.x];
    pages[entry.x]=entry.y;
}
