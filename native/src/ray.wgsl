struct Camera { vp:mat4x4<f32>, selection:vec4<f32>, viewport:vec4<f32>, inverse:mat4x4<f32>, eye:vec4<f32>, effects:vec4<f32> }
@group(0) @binding(0) var<uniform> camera:Camera;
@group(0) @binding(1) var horizon_visibility:texture_2d<f32>;
@group(1) @binding(0) var<storage,read> data:array<u32>;
// SHARED_SKY
@vertex fn vs_ray(@builtin(vertex_index) i:u32)->@builtin(position) vec4<f32>{
 let p=array<vec2<f32>,3>(vec2<f32>(-1,-1),vec2<f32>(3,-1),vec2<f32>(-1,3));return vec4<f32>(p[i],0.,1.);
}
fn bounds(p:u32)->mat2x3<f32>{return mat2x3<f32>(vec3<f32>(f32(data[p]),f32(data[p+1u]),f32(data[p+2u])),vec3<f32>(f32(data[p+4u]),f32(data[p+5u]),f32(data[p+6u])));}
fn span(low:vec3<f32>,high:vec3<f32>,origin:vec3<f32>,inv:vec3<f32>)->vec2<f32>{
 let a=(low-origin)*inv;let b=(high-origin)*inv;let near=min(a,b);let far=max(a,b);
 return vec2<f32>(max(max(near.x,near.y),near.z),min(min(far.x,far.y),far.z));
}
fn shadow_at(world:vec3<f32>)->f32 {
 let size=vec2<i32>(textureDimensions(horizon_visibility));let uv=clamp(world.xz/10000.*vec2<f32>(size)-0.5,vec2<f32>(0.),vec2<f32>(size)-1.);
 let a=vec2<i32>(floor(uv));let b=min(a+vec2<i32>(1),size-vec2<i32>(1));let t=fract(uv);
 return mix(mix(textureLoad(horizon_visibility,a,0).r,textureLoad(horizon_visibility,vec2<i32>(b.x,a.y),0).r,t.x),mix(textureLoad(horizon_visibility,vec2<i32>(a.x,b.y),0).r,textureLoad(horizon_visibility,b,0).r,t.x),t.y);
}
struct Pixel {@location(0) color:vec4<f32>,@builtin(frag_depth) depth:f32}
@fragment fn fs_ray(@builtin(position) pixel:vec4<f32>)->Pixel {
 let ndc=(pixel.xy/camera.viewport.xy*2.-1.)*vec2<f32>(1.,-1.);
 let point=camera.inverse*vec4<f32>(ndc,0.5,1.);let direction=normalize(point.xyz/point.w);
 let inv=select(vec3<f32>(1e10),1./direction,abs(direction)>vec3<f32>(1e-10));
 let origin=camera.eye.xyz/camera.viewport.w;
 var closest=camera.effects.w/camera.viewport.w;var found=false;var best_run=0u;var best_normal=vec3<f32>(0,1,0);
 // A queued node already passed its bounds test. Retain the entry distance so
 // a later, nearer hit can reject it without reloading/testing its bounds.
 var stack:array<vec2<u32>,32>;var count=0u;
 let root=bounds(0u);let root_interval=span(root[0],root[1],origin,inv);
 if root_interval.y>=max(root_interval.x,0.4)&&root_interval.x<=closest {
  stack[0]=vec2<u32>(0u,bitcast<u32>(root_interval.x));count=1u;
 }
 loop {
  if count==0u {break;}count-=1u;let work=stack[count];let node=work.x;
  if bitcast<f32>(work.y)>closest {continue;}
  let left=data[node+3u];let right=data[node+7u];
  if (left&0x80000000u)!=0u {
   let box=bounds(node);
   let first=left&0x7fffffffu;
   for(var i=0u;i<right;i+=1u){
    let r=first+i*8u;let low=vec3<f32>(box[0].x,f32(data[r]),box[0].z);let high=vec3<f32>(box[1].x,f32(data[r+1u]),box[1].z);
    let hit=span(low,high,origin,inv);let t=select(hit.x,hit.y,hit.x<0.4);
    if hit.y>=max(hit.x,0.4)&&t<closest {
     let a=(low-origin)*inv;let b=(high-origin)*inv;let face=select(min(a,b),max(a,b),hit.x<0.4);
     var normal=vec3<f32>(0,0,-sign(direction.z));
     let order=select(face,-face,hit.x<0.4);
     if order.x>=order.y&&order.x>=order.z {normal=vec3<f32>(-sign(direction.x),0,0);}
     else if order.y>=order.z {normal=vec3<f32>(0,-sign(direction.y),0);}
     if hit.x<0.4 {normal=-normal;}
     closest=t;found=true;best_run=r;best_normal=normal;
    }
   }
  }else if (left&0x40000000u)!=0u {
   let child=left&0x3fffffffu;let box=bounds(child);
   let interval=span(box[0],box[1],origin,inv);
   if interval.y>=max(interval.x,0.4)&&interval.x<=closest {
    stack[count]=vec2<u32>(child,bitcast<u32>(interval.x));count+=1u;
   }
  }
  else {
   let a=bounds(left);let b=bounds(right);let ta=span(a[0],a[1],origin,inv);let tb=span(b[0],b[1],origin,inv);
   let hit_a=ta.y>=max(ta.x,0.4)&&ta.x<=closest;
   let hit_b=tb.y>=max(tb.x,0.4)&&tb.x<=closest;
   // Queue only intersected children, far first. Never spend a stack entry or
   // another bounds test on a missed sibling. Near-first ordering is unchanged.
   if hit_a&&hit_b {
    let near=select(vec2<u32>(right,bitcast<u32>(tb.x)),vec2<u32>(left,bitcast<u32>(ta.x)),ta.x<=tb.x);
    let far=select(vec2<u32>(left,bitcast<u32>(ta.x)),vec2<u32>(right,bitcast<u32>(tb.x)),ta.x<=tb.x);
    if count+2u<=32u {stack[count]=far;stack[count+1u]=near;count+=2u;}
   }else if hit_a {stack[count]=vec2<u32>(left,bitcast<u32>(ta.x));count+=1u;}
   else if hit_b {stack[count]=vec2<u32>(right,bitcast<u32>(tb.x));count+=1u;}
  }
 }
 var out:Pixel;if !found {discard;}
 let relative=direction*closest*camera.viewport.w;let world=relative+camera.eye.xyz;
 let r=best_run;let y=i32(floor(world.y/camera.viewport.w-best_normal.y*0.001));
 let index=u32(clamp(y-i32(data[r+4u]),0,i32(data[r+5u])-1));
 let packed=select(data[data[r+3u]+index],data[r+2u],y<i32(data[r+4u]));let rgb=vec3<f32>(f32(packed&255u),f32((packed>>8u)&255u),f32((packed>>16u)&255u))/255.;
 let flags=data[r+6u];let water=(flags&0x1000000u)!=0u;let light=(flags>>16u)&255u;
 var sunlight=max(0.,dot(best_normal,normalize(vec3<f32>(-0.4,0.85,-0.35))));
 var sky_light=0.34+0.10*max(best_normal.y,0.);var ambient=1.;
 if !water {
  let projected=camera.viewport.w*camera.viewport.y*camera.effects.z/max(length(relative),0.01);let blend=1.-smoothstep(0.8,3.,projected);
  sunlight=mix(sunlight,f32(light&63u)/63.,blend);sky_light=mix(sky_light,0.44,blend);
  ambient=mix(select(1.,0.7+0.1*f32(light>>6u),camera.effects.x>0.5),1.,blend);
 }
 let visibility=select(1.,shadow_at(world),camera.effects.y>0.5);
 var lit=pow(rgb,vec3<f32>(2.2))*(sky_light*ambient+0.58*sunlight*visibility);
 let fog=1.-exp(-max(length(relative)-60.,0.)*camera.viewport.z*1.65);
 let cutoff=smoothstep(camera.effects.w*0.75,camera.effects.w,length(relative));
 lit=mix(lit,sky_color(direction),max(fog,cutoff));
 if camera.selection.w>0. {
  let p=(relative-camera.selection.xyz)/camera.viewport.w;let edge=min(abs(p),abs(vec3<f32>(1)-p));
  if all(p>=vec3<f32>(-0.002))&&all(p<=vec3<f32>(1.002))&&((edge.x<0.025&&edge.y<0.025)||(edge.x<0.025&&edge.z<0.025)||(edge.y<0.025&&edge.z<0.025)){lit=vec3<f32>(0.015);}
 }
 let clip=camera.vp*vec4<f32>(relative,1.);out.depth=clip.z/clip.w;out.color=vec4<f32>(lit,1.);return out;
}
