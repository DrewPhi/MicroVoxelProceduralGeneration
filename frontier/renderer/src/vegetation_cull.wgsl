// One bounded work item per submitted mesh/instance cluster. A cluster is
// rejected only when all eight corners lie strictly outside one clip plane.
struct Camera { view_projection:mat4x4<f32>, eye:vec4<f32>, paint_origin:vec4<u32> };
@group(0) @binding(0) var<uniform> camera:Camera;
struct Cluster { low:vec4<f32>, high:vec4<f32>, args:vec4<u32> };
struct Draw { vertex_count:u32, instance_count:u32, first_vertex:u32, first_instance:u32 };
@group(1) @binding(0) var<storage,read> clusters:array<Cluster>;
@group(1) @binding(1) var<storage,read_write> draws:array<Draw>;
@compute @workgroup_size(64) fn cull(@builtin(global_invocation_id) id:vec3<u32>) {
    let index=id.x;
    if index>=arrayLength(&clusters) {return;}
    let cluster=clusters[index];
    var outside=array<u32,6>(0u,0u,0u,0u,0u,0u);
    for(var corner=0u;corner<8u;corner++) {
        let local=vec3<f32>(select(cluster.low.x,cluster.high.x,(corner&1u)!=0u),select(cluster.low.y,cluster.high.y,(corner&2u)!=0u),select(cluster.low.z,cluster.high.z,(corner&4u)!=0u));
        let p=camera.view_projection*vec4<f32>(local,1.);
        // Absolute + relative roundoff margin: uncertain boundaries stay visible.
        let e=0.001+abs(p.w)*0.00001;
        outside[0]+=u32(p.x < -p.w-e); outside[1]+=u32(p.x > p.w+e);
        outside[2]+=u32(p.y < -p.w-e); outside[3]+=u32(p.y > p.w+e);
        outside[4]+=u32(p.z < -e); outside[5]+=u32(p.z > p.w+e);
    }
    var count=cluster.args.y;
    for(var plane=0u;plane<6u;plane++) {if outside[plane]==8u {count=0u;}}
    draws[index]=Draw(cluster.args.x,count,0u,0u);
}
