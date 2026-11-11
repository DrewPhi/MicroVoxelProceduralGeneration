@group(0) @binding(7) var scene:acceleration_structure;
@compute @workgroup_size(64)
fn hardware_main(@builtin(global_invocation_id) i:vec3<u32>) {
    if i.x>=p.rays{return;}
    let r=rays[i.x];
    var query:ray_query;
    rayQueryInitialize(&query,scene,RayDesc(0u,255u,0.,1e20,r.o.xyz,r.d.xyz));
    var best=Hit(0u,1e20);
    while rayQueryProceed(&query) {
        let candidate=rayQueryGetCandidateIntersection(&query);
        if candidate.kind==RAY_QUERY_INTERSECTION_AABB {
            let hit=brick_hit(candidate.primitive_index,r.o.xyz,r.d.xyz);
            let hit_time=hit.t;
            if hit.cell!=0u && hit_time<best.t {
                best=hit;
                rayQueryGenerateIntersection(&query,hit_time);
            }
        }
    }
    output[i.x]=vec2(best.cell,bitcast<u32>(best.t));
}
