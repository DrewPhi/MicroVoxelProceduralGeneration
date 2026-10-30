// Instrumented compute microbenchmark. Counters describe logical work, not DRAM.
struct Ray {origin:vec4<f32>,direction:vec4<f32>}
struct Result {cell:vec3<i32>,hit:u32,color:u32,visits:u32,tests:u32,loads:u32}
struct Params {mode:u32,root:u32,height:u32,attributes:u32,count:u32,max_y:u32,pad0:u32,pad1:u32}
@group(0) @binding(0) var<storage,read> source:array<u32>;
@group(0) @binding(1) var<storage,read> tree:array<u32>;
@group(0) @binding(2) var<storage,read> rays:array<Ray>;
@group(0) @binding(3) var<storage,read_write> results:array<Result>;
@group(0) @binding(4) var<uniform> params:Params;
fn span(low:vec3<f32>,high:vec3<f32>,origin:vec3<f32>,inv:vec3<f32>)->vec2<f32>{
 let a=(low-origin)*inv;let b=(high-origin)*inv;let near=min(a,b);let far=max(a,b);
 return vec2<f32>(max(max(near.x,near.y),near.z),min(min(far.x,far.y),far.z));
}
fn normal_at(low:vec3<f32>,high:vec3<f32>,origin:vec3<f32>,inv:vec3<f32>,inside:bool)->vec3<f32>{
 let a=(low-origin)*inv;let b=(high-origin)*inv;let order=select(min(a,b),-max(a,b),inside);
 var n=vec3<f32>(0,0,-sign(inv.z));
 if order.x>=order.y&&order.x>=order.z {n=vec3<f32>(-sign(inv.x),0,0);}else if order.y>=order.z {n=vec3<f32>(0,-sign(inv.y),0);}
 return select(n,-n,inside);
}
fn column_bounds(p:u32)->mat2x3<f32>{
 return mat2x3<f32>(vec3<f32>(f32(source[p]),f32(source[p+1u]),f32(source[p+2u])),vec3<f32>(f32(source[p+4u]),f32(source[p+5u]),f32(source[p+6u])));
}
fn location(p:vec3<u32>,height:u32)->u32 {return p.x|(p.z<<5u)|(p.y<<10u)|(height<<25u);}
fn unpack(p:u32)->vec3<u32>{return vec3<u32>(p&31u,(p>>10u)&32767u,(p>>5u)&31u);}
@compute @workgroup_size(64) fn main(@builtin(global_invocation_id) gid:vec3<u32>){
 if gid.x>=params.count {return;}
 let ray=rays[gid.x];let origin=ray.origin.xyz;let direction=ray.direction.xyz;
 let inv=select(vec3<f32>(1e12),1./direction,abs(direction)>vec3<f32>(1e-12));
 var out:Result;out.cell=vec3<i32>(-1);out.hit=0u;out.color=0u;out.visits=0u;out.tests=0u;out.loads=0u;
 var closest=1e20;var best_run=0u;var best_normal=vec3<f32>(0);var hit_point=vec3<f32>(0);
 let overall=span(vec3<f32>(0),vec3<f32>(32,f32(params.max_y),32),origin,inv);out.tests+=1u;
 if overall.y<max(overall.x,0.) {results[gid.x]=out;return;}
 if params.mode==0u {
  var stack:array<u32,32>;var count=1u;stack[0]=0u;
  loop {
   if count==0u {break;}count-=1u;let n=stack[count];out.visits+=1u;out.loads+=8u;
   let box=column_bounds(n);let interval=span(box[0],box[1],origin,inv);out.tests+=1u;
   if interval.y<max(interval.x,0.)||interval.x>closest {continue;}
   let a=source[n+3u];let b=source[n+7u];
   if (a&0x80000000u)!=0u {
    for(var i=0u;i<b;i+=1u){let p=(a&0x7fffffffu)+i*8u;out.loads+=2u;
     let low=vec3<f32>(box[0].x,f32(source[p]),box[0].z);let high=vec3<f32>(box[1].x,f32(source[p+1u]),box[1].z);
     let hit=span(low,high,origin,inv);out.tests+=1u;let t=select(hit.x,hit.y,hit.x<0.);
     if hit.y>=max(hit.x,0.)&&t<closest {closest=t;out.hit=1u;best_run=p;best_normal=normal_at(low,high,origin,inv,hit.x<0.);hit_point=origin+direction*t;}
    }
   }else{
    let left=column_bounds(a);let right=column_bounds(b);out.loads+=12u;out.tests+=2u;
    let ta=span(left[0],left[1],origin,inv);let tb=span(right[0],right[1],origin,inv);
    stack[count]=select(a,b,ta.x<=tb.x);stack[count+1u]=select(b,a,ta.x<=tb.x);count+=2u;
   }
  }
 }else{
  // A work entry needs only node ID and packed local coordinates/height.
  var stack:array<vec2<u32>,64>;var count=1u;stack[0]=vec2<u32>(params.root,location(vec3<u32>(0),params.height));
  loop {
   if count==0u {break;}count-=1u;let work=stack[count];out.visits+=1u;
   if work.x==0u {continue;}
   let pos=unpack(work.y);let height=work.y>>25u;let size=1u<<height;
   let low=vec3<f32>(pos);let high=min(low+f32(size),vec3<f32>(32,f32(params.max_y),32));
   let interval=span(low,high,origin,inv);out.tests+=1u;
   if interval.y<max(interval.x,0.)||interval.x>closest {continue;}
   if work.x==1u {
    let t=select(interval.x,interval.y,interval.x<0.);
    if t<closest {closest=t;out.hit=1u;best_normal=normal_at(low,high,origin,inv,interval.x<0.);hit_point=origin+direction*t;}
    continue;
   }
   let n=(work.x-2u)*2u;let mask=tree[n];let first=tree[n+1u];out.loads+=2u;
   let half=size/2u;var candidates:array<vec3<u32>,8>;var length=0u;
   for(var child=0u;child<8u;child+=1u){
    if (mask&(1u<<child))==0u {continue;}
    let id=tree[first+countOneBits(mask&((1u<<child)-1u))];out.loads+=1u;
    let p=pos+vec3<u32>(child&1u,(child>>1u)&1u,(child>>2u)&1u)*half;
    if p.x>=32u||p.z>=32u {continue;}
    let a=vec3<f32>(p);let b=min(a+f32(half),vec3<f32>(32,f32(params.max_y),32));
    let t=span(a,b,origin,inv);out.tests+=1u;if t.y<max(t.x,0.)||t.x>closest {continue;}
    let item=vec3<u32>(id,location(p,height-1u),bitcast<u32>(max(t.x,0.)));
    var j=length;
    loop {if j==0u {break;}if bitcast<f32>(candidates[j-1u].z)<=max(t.x,0.) {break;}candidates[j]=candidates[j-1u];j-=1u;}
    candidates[j]=item;length+=1u;
   }
   for(var i=length;i>0u;i-=1u){let item=candidates[i-1u];stack[count]=item.xy;count+=1u;}
  }
 }
 if out.hit!=0u {
  out.cell=vec3<i32>(floor(hit_point-best_normal*0.01));
  if params.attributes!=0u {
   var r=best_run;
   if params.mode!=0u {
    let col=u32(clamp(out.cell.z,0,31))*32u+u32(clamp(out.cell.x,0,31));let first=source[col*2u];let length=source[col*2u+1u];out.loads+=2u;
    r=first;
    for(var i=0u;i<length;i+=1u){let p=first+i*8u;out.loads+=2u;if i32(source[p])<=out.cell.y&&out.cell.y<i32(source[p+1u]) {r=p;break;}}
   }
   out.loads+=5u;
   let y=u32(clamp(out.cell.y-i32(source[r+4u]),0,i32(source[r+5u])-1));out.color=select(source[source[r+3u]+y],source[r+2u],out.cell.y<i32(source[r+4u]));
  }
 }
 results[gid.x]=out;
}
