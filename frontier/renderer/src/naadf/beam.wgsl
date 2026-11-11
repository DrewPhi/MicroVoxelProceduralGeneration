// One invocation proves a shared empty prefix for all samples in an 8x8 tile.
// Unnormalized camera directions are affine in pixel position; over a forward
// slab each coordinate's extrema lie at the tile corners and slab endpoints.
@group(0) @binding(1) var beam_heights:texture_2d<u32>;
@group(0) @binding(2) var beam_out:texture_storage_2d<r32uint,write>;
override height_tile_roots:u32=8u;
const BEAM_PIXELS:u32=8u;
fn beam_slope(point:vec2<f32>)->vec3<f32> {
    let ndc=vec2(point.x/naadf.viewport.x*2.-1.,1.-point.y/naadf.viewport.y*2.);
    return naadf.forward.xyz+ndc.x*naadf.right.xyz*naadf.right.w+ndc.y*naadf.up.xyz*naadf.up.w;
}
@compute @workgroup_size(8,8)
fn main(@builtin(global_invocation_id) id:vec3<u32>) {
    if any(id.xy>=(vec2<u32>(naadf.viewport.xy)+7u)/BEAM_PIXELS) {return;}
    trace_metric(2u,0u,1u);
    let start=vec2<f32>(id.xy*BEAM_PIXELS);
    let end=min(start+vec2(f32(BEAM_PIXELS)),naadf.viewport.xy);
    let a=beam_slope(start);
    let b=beam_slope(end);
    let c=beam_slope(vec2(start.x,end.y));
    let d=beam_slope(vec2(end.x,start.y));
    let min_slope=min(min(a,b),min(c,d));
    let max_slope=max(max(a,b),max(c,d));
    let eye=naadf.eye.xyz*16.;
    let extent=vec3<f32>(naadf.extent.xyz);
    let span=f32(height_tile_roots*16u);
    let tile_side=height_tile_roots*16u;
    let last_tile=vec2<i32>((naadf.extent.xz+tile_side-1u)/tile_side)-vec2(1);
    let max_abs_slope=max(abs(min_slope),abs(max_slope));
    let max_slope_axis=max(max(max_abs_slope.x,max_abs_slope.y),max_abs_slope.z);
    if any(abs(eye)>extent*16.) || max_slope_axis>4. {
        trace_metric(2u,2u,1u);
        textureStore(beam_out,vec2<i32>(id.xy),vec4(0u));
        return;
    }
    var passed=0u;
    for(var attempt=0u;attempt<8u;attempt++){
        trace_metric(2u,3u,1u);
        let next=16u<<attempt;
        // Pads normalized-direction conversion, dot products and cell selection.
        let margin=max(0.25,(max(max(abs(eye.x),abs(eye.y)),abs(eye.z))+f32(next)*4.)*0.00002);
        let low=eye+min(min_slope*f32(passed),min_slope*f32(next))-vec3(margin);
        let high=eye+max(max_slope*f32(passed),max_slope*f32(next))+vec3(margin);
        if high.x>=0. && high.z>=0. && high.y>=0. && low.x<extent.x && low.z<extent.z && low.y<extent.y {
            let first=clamp(vec2<i32>(floor(low.xz/span)),vec2(0),last_tile);
            let last=clamp(vec2<i32>(floor(high.xz/span)),vec2(0),last_tile);
            // Bound the cost. An untested slab is never skipped.
            if any(last-first>vec2(3)) {trace_metric(2u,6u,1u);break;}
            var max_height=0u;
            for(var z=first.y;z<=last.y;z++){
                for(var x=first.x;x<=last.x;x++){
                    trace_metric(2u,4u,1u);
                    max_height=max(max_height,textureLoad(beam_heights,vec2(x,z),0).x);
                }
            }
            if max_height>0u && low.y<=f32(max_height) {trace_metric(2u,5u,1u);break;}
        }
        passed=next;
    }
    trace_metric(2u,select(2u,1u,passed>0u),1u);
    trace_distance(2u,15u,f32(passed));
    textureStore(beam_out,vec2<i32>(id.xy),vec4(passed,0u,0u,0u));
}
