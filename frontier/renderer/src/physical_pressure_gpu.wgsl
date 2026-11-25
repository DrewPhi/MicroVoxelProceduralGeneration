// Matrix-agnostic pressure diagnostic. No source/ghost/interface rules here.
struct Row { a:vec4<u32>, b:vec2<u32>, diagonal:f32, rhs:f32 };
struct Config { count:u32, groups:u32, max_iterations:u32, pad:u32, relative:f32, absolute:f32, pad1:f32, pad2:f32 };
struct State { rho:f32, alpha:f32, beta:f32, residual2:f32, rhs2:f32, target2:f32, iterations:u32, status:u32 };
@group(0) @binding(0) var<storage,read> rows:array<Row>;
// Per row: solution, true residual, direction, A*direction.
@group(0) @binding(1) var<storage,read_write> vectors:array<vec4<f32>>;
@group(0) @binding(2) var<storage,read_write> partial:array<vec4<f32>>;
@group(0) @binding(3) var<storage,read_write> state:State;
@group(0) @binding(4) var<uniform> config:Config;
var<workgroup> scratch:array<vec4<f32>,256>;
fn finite(x:f32)->bool { return abs(x)<=3.402823466e38; }
fn neighbor(row:Row,k:u32)->u32 { if k<4u {return row.a[k];} return row.b[k-4u]; }
fn apply(i:u32,component:u32)->f32 {
    let row=rows[i];var result=row.diagonal*vectors[i][component];
    for(var k=0u;k<6u;k=k+1u) {let j=neighbor(row,k);if j<config.count {result-=vectors[j][component];}}
    return result;
}
fn group_sum(lane:u32,value:vec4<f32>)->vec4<f32> {
    scratch[lane]=value;workgroupBarrier();
    for(var stride=128u;stride>0u;stride=stride/2u) {
        if lane<stride {scratch[lane]+=scratch[lane+stride];}
        workgroupBarrier();
    }
    return scratch[0];
}
fn final_sum(lane:u32)->vec4<f32> {
    var value=vec4(0.);
    for(var i=lane;i<config.groups;i=i+256u) {value+=partial[i];}
    return group_sum(lane,value);
}
@compute @workgroup_size(256)
fn initialize(@builtin(global_invocation_id) id:vec3<u32>,@builtin(local_invocation_index) lane:u32,@builtin(workgroup_id) block:vec3<u32>) {
    let i=id.x;var value=vec4(0.);
    if i<config.count {
        let r=rows[i].rhs;let z=r/rows[i].diagonal;vectors[i]=vec4(0.,r,z,0.);
        value=vec4(r*r,r*z,r*r,0.);
    }
    let total=group_sum(lane,value);if lane==0u {partial[block.x]=total;}
}
@compute @workgroup_size(256)
fn initialize_sum(@builtin(local_invocation_index) lane:u32) {
    let total=final_sum(lane);
    if lane==0u {
        state.rho=total.y;state.alpha=0.;state.beta=0.;state.residual2=total.z;state.rhs2=total.x;
        state.target2=max(config.absolute*config.absolute,config.relative*config.relative*total.x);
        state.iterations=0u;state.status=0u;
        if !finite(total.x)||!finite(total.y)||!finite(state.target2) {state.status=2u;}
        else if total.z<=state.target2 {state.status=1u;}
    }
}
@compute @workgroup_size(256)
fn multiply_direction(@builtin(global_invocation_id) id:vec3<u32>,@builtin(local_invocation_index) lane:u32,@builtin(workgroup_id) block:vec3<u32>) {
    let i=id.x;var value=vec4(0.);
    if state.status==0u&&i<config.count {
        let ap=apply(i,2u);vectors[i].w=ap;value.x=vectors[i].z*ap;
    }
    let total=group_sum(lane,value);if lane==0u {partial[block.x]=total;}
}
@compute @workgroup_size(256)
fn alpha_sum(@builtin(local_invocation_index) lane:u32) {
    let total=final_sum(lane);
    if lane==0u&&state.status==0u {
        if !finite(total.x)||total.x<=0.||!finite(state.rho)||state.rho<=0. {state.status=2u;}
        else {state.alpha=state.rho/total.x;if !finite(state.alpha) {state.status=2u;}}
    }
}
@compute @workgroup_size(256)
fn update_solution(@builtin(global_invocation_id) id:vec3<u32>) {
    if state.status==0u&&id.x<config.count {vectors[id.x].x+=state.alpha*vectors[id.x].z;}
}
// Separate dispatch after every solution write: no cross-workgroup dependency
// is mistaken for a workgroup barrier. Convergence uses this true residual.
@compute @workgroup_size(256)
fn true_residual(@builtin(global_invocation_id) id:vec3<u32>,@builtin(local_invocation_index) lane:u32,@builtin(workgroup_id) block:vec3<u32>) {
    let i=id.x;var value=vec4(0.);
    if state.status==0u&&i<config.count {
        let r=rows[i].rhs-apply(i,0u);vectors[i].y=r;
        value=vec4(r*r,r*(r/rows[i].diagonal),0.,0.);
    }
    let total=group_sum(lane,value);if lane==0u {partial[block.x]=total;}
}
@compute @workgroup_size(256)
fn beta_sum(@builtin(local_invocation_index) lane:u32) {
    let total=final_sum(lane);
    if lane==0u&&state.status==0u {
        state.iterations+=1u;state.residual2=total.x;
        if !finite(total.x)||!finite(total.y)||total.y<0. {state.status=2u;}
        else if total.x<=state.target2 {state.status=1u;}
        else {state.beta=total.y/state.rho;state.rho=total.y;if !finite(state.beta) {state.status=2u;}}
    }
}
@compute @workgroup_size(256)
fn update_direction(@builtin(global_invocation_id) id:vec3<u32>) {
    if state.status==0u&&id.x<config.count {let i=id.x;vectors[i].z=vectors[i].y/rows[i].diagonal+state.beta*vectors[i].z;}
}
@compute @workgroup_size(1)
fn finalize() {if state.status==0u {state.status=3u;}}
