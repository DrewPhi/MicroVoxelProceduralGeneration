struct Params { side:u32, brick:u32, grid:u32, rays:u32 };
struct Brick { lo:vec3<f32>, pointer:u32, hi:vec3<f32>, mask:u32 };
struct Ray { o:vec4<f32>, d:vec4<f32> };
struct Hit { cell:u32, t:f32 };
@group(0) @binding(0) var<uniform> p:Params;
@group(0) @binding(1) var<storage,read> grid:array<u32>;
@group(0) @binding(2) var<storage,read> bricks:array<Brick>;
@group(0) @binding(3) var<storage,read> voxels:array<u32>;
@group(0) @binding(4) var<storage,read> masks:array<u32>;
@group(0) @binding(5) var<storage,read> rays:array<Ray>;
@group(0) @binding(6) var<storage,read_write> output:array<vec2<u32>>;
override use_mask:bool=false;
fn interval(o:vec3<f32>,d:vec3<f32>,lo:vec3<f32>,hi:vec3<f32>)->vec2<f32> {
    var near=0.;var far=1e20;
    for(var a=0u;a<3u;a++) {
        if d[a]==0. {if o[a]<lo[a] || o[a]>=hi[a] {return vec2(1.,-1.);}}
        else {let t0=(lo[a]-o[a])/d[a];let t1=(hi[a]-o[a])/d[a];near=max(near,min(t0,t1));far=min(far,max(t0,t1));}
    }
    return vec2(near,far);
}
fn brick_hit(id:u32,o:vec3<f32>,d:vec3<f32>)->Hit {
    let b=bricks[id];let span=interval(o,d,b.lo,b.hi);
    if span.x>=span.y {return Hit(0u,1e20);}
    var t=span.x;
    var cell=clamp(vec3<i32>(floor(o+d*t)),vec3<i32>(b.lo),vec3<i32>(b.hi)-vec3(1));
    if (b.pointer&0x80000000u)!=0u {
        let c=vec3<u32>(cell);return Hit(1u+c.x+p.side*(c.y+p.side*c.z),t);
    }
    // Exact native plane events; no arbitrary fixed ray-marching increments.
    for(var step=0u;step<3u*p.brick+3u;step++) {
        if any(cell<vec3<i32>(b.lo)) || any(cell>=vec3<i32>(b.hi)) {break;}
        let local=vec3<u32>(cell)%p.brick;
        let index=local.x+p.brick*(local.y+p.brick*local.z);
        var solid=false;
        if use_mask {solid=(masks[b.mask+index/32u]&(1u<<(index%32u)))!=0u;}
        else {let address=b.pointer+index;solid=((voxels[address/2u]>>((address%2u)*16u))&65535u)!=0u;}
        if solid {let c=vec3<u32>(cell);return Hit(1u+c.x+p.side*(c.y+p.side*c.z),t);}
        var next=vec3(1e20);
        for(var a=0u;a<3u;a++){if d[a]!=0.{next[a]=(f32(cell[a]+select(0,1,d[a]>0.))-o[a])/d[a];}}
        let crossing=min(next.x,min(next.y,next.z));
        if crossing>span.y {break;}
        for(var a=0u;a<3u;a++){if next[a]==crossing{cell[a]+=select(-1,1,d[a]>0.);}}
        t=crossing;
    }
    return Hit(0u,1e20);
}
fn software(o:vec3<f32>,d:vec3<f32>)->Hit {
    let span=interval(o,d,vec3(0.),vec3(f32(p.side)));
    if span.x>=span.y{return Hit(0u,1e20);}
    var cell=clamp(vec3<i32>(floor((o+d*span.x)/f32(p.brick))),vec3(0),vec3(i32(p.grid)-1));
    for(var step=0u;step<3u*p.grid+3u;step++) {
        if any(cell<vec3(0)) || any(cell>=vec3(i32(p.grid))) {break;}
        let c=vec3<u32>(cell);let id=grid[c.x+p.grid*(c.y+p.grid*c.z)];
        if id!=0u {let hit=brick_hit(id-1u,o,d);if hit.cell!=0u{return hit;}}
        var next=vec3(1e20);
        for(var a=0u;a<3u;a++){if d[a]!=0.{next[a]=(f32((cell[a]+select(0,1,d[a]>0.))*i32(p.brick))-o[a])/d[a];}}
        let crossing=min(next.x,min(next.y,next.z));
        if crossing>span.y {break;}
        for(var a=0u;a<3u;a++){if next[a]==crossing{cell[a]+=select(-1,1,d[a]>0.);}}
    }
    return Hit(0u,1e20);
}
@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) i:vec3<u32>) {
    if i.x>=p.rays{return;}
    let h=software(rays[i.x].o.xyz,rays[i.x].d.xyz);
    output[i.x]=vec2(h.cell,bitcast<u32>(h.t));
}
