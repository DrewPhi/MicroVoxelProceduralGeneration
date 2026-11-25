struct Candidate {
    first: u32,
    count: u32,
    slot: u32,
    axis_sign: u32,
    planes: vec2<f32>,
    padding: vec2<u32>,
}
struct Indirect {
    index_count: u32,
    instance_count: u32,
    first_index: u32,
    base_vertex: i32,
    first_instance: u32,
}
@group(0) @binding(0) var<storage, read> candidates: array<Candidate>;
// Each 256-byte slot contains the eye vector at byte offset 64.
@group(0) @binding(1) var<storage, read> camera_words: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read_write> commands: array<Indirect>;

@compute @workgroup_size(64)
fn cull(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= arrayLength(&candidates) { return; }
    let bin = candidates[gid.x];
    let axis = bin.axis_sign & 3u;
    var count = bin.count;
    if axis < 3u {
        let eye = camera_words[bin.slot * 16u + 4u].xyz;
        let scale = max(1., max(max(abs(eye.x), abs(eye.y)), max(abs(eye.z),
            max(abs(bin.planes.x), abs(bin.planes.y)))));
        // Match the conservative CPU NativeBin bound, allowing for f32 inputs.
        let margin = NATIVE_VOXEL_METERS / 1024. + 0.000007629395 * scale;
        let back = select(eye[axis] < bin.planes.x - margin,
            eye[axis] > bin.planes.y + margin, (bin.axis_sign & 4u) != 0u);
        if back { count = 0u; }
    }
    commands[gid.x] = Indirect(6u, count, 0u, 0, bin.first);
}
