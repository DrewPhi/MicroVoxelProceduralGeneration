// Descriptors and exact payload share one staging SSBO. Each workgroup owns
// one disjoint newly reserved destination range; published roots cannot yet
// reach any of these destinations.
@group(0) @binding(0) var<storage,read> staged:array<u32>;
@group(0) @binding(1) var<storage,read_write> blocks:array<u32>;
@group(0) @binding(2) var<storage,read_write> voxels:array<u32>;
@group(0) @binding(3) var<storage,read_write> palette:array<u32>;

@compute @workgroup_size(64)
fn main(@builtin(workgroup_id) group:vec3<u32>,@builtin(local_invocation_index) lane:u32) {
    let descriptor=group.x*4u;
    let kind=staged[descriptor];
    let destination=staged[descriptor+1u];
    let source=staged[descriptor+2u];
    let count=staged[descriptor+3u];
    for(var i=lane;i<count;i+=64u) {
        let value=staged[source+i];
        switch kind {
            case 0u: {blocks[destination+i]=value;}
            case 1u: {voxels[destination+i]=value;}
            default: {palette[destination+i]=value;}
        }
    }
}
