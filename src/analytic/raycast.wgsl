// Specialized once per loaded scene; old models compile out spline traversal.
const USE_SPLINE:bool=false;
struct Frame { eye:vec4f, forward:vec4f, right:vec4f, up:vec4f, grid:vec4u, scene:vec4f, options:vec4u }
struct Quad { a:vec4f, b:vec4f, c:vec4f }
@group(0) @binding(0) var<uniform> frame:Frame;
@group(0) @binding(1) var<storage,read> quads:array<Quad>;
@group(0) @binding(2) var output:texture_storage_2d<rgba8unorm,write>;
@group(0) @binding(3) var<storage,read_write> probes:array<vec4f>;
@group(0) @binding(4) var<storage,read> quads_tail:array<Quad>;
fn quad_at(i:u32)->Quad {
 let split=arrayLength(&quads);
 if i<split {return quads[i];}
 return quads_tail[i-split];
}
struct Temporal { previous:Frame, jitter:vec4f, state:vec4f, tuning:vec4f }
@group(0) @binding(5) var<uniform> temporal:Temporal;
@group(0) @binding(6) var history:texture_2d<f32>;
@group(0) @binding(7) var next_history:texture_storage_2d<rgba32float,write>;
fn hash_cell(p:vec3i)->f32 {
 var h=bitcast<u32>(p.x)*1597334677u ^ bitcast<u32>(p.y)*3812015801u ^ bitcast<u32>(p.z)*2798796415u;
 h=(h^(h>>16u))*2246822519u;h=(h^(h>>13u))*3266489917u;h=h^(h>>16u);
 return f32(h>>8u)/16777216.;
}
fn accumulate(color:vec3f,p:vec3f,depth:f32)->vec3f {
 if temporal.state.x<0.5 {return color;}
 let prev=temporal.previous;
 let v=select(p-prev.eye.xyz,p,depth<0.);let forward=dot(v,prev.forward.xyz);
 if forward<=0. {return color;}
 let uv=vec2f(dot(v,prev.right.xyz)/(forward*prev.eye.w*prev.forward.w),-dot(v,prev.up.xyz)/(forward*prev.eye.w));
 let pixel=(uv*.5+vec2f(.5))*vec2f(frame.options.xy)-vec2f(.5)-temporal.jitter.zw;
 let low=vec2i(floor(pixel));let frac=fract(pixel);var sum=vec3f(0.);var weight=0.;
 let expected=length(v);
 for(var y=0;y<2;y++){for(var x=0;x<2;x++){
  let q=low+vec2i(x,y);if any(q<vec2i(0))||any(q>=vec2i(frame.options.xy)){continue;}
  let old=textureLoad(history,q,0);
  if depth<0. {if old.w>=0. {continue;}}
  else {if old.w<=0. || abs(old.w-expected)>max(frame.up.w*2.,expected*.001){continue;}}
  let w=select(1.-frac.x,frac.x,x==1)*select(1.-frac.y,frac.y,y==1);
  sum+=old.xyz*w;weight+=w;
 }}
 if weight<0.2{return color;}
 // Bound history influence to suppress persistent trails at changing material edges.
 let old=clamp(sum/weight,color-vec3f(.12),color+vec3f(.12));
 return mix(color,old,min(temporal.tuning.x,.95)*weight);
}
@group(0) @binding(8) var<storage,read> air_nodes:array<u32>;
struct AirJump { t:f32, cell:vec3i, normal:vec3f }
@group(0) @binding(9) var<storage,read> envelope_nodes:array<Quad>;
var<private> envelope_work:u32;
fn interval_max(q:Quad,u:vec3f,v:vec3f,duration:f32)->f32 {
 let a=dot(q.b.xyz,v*v)+q.b.w*v.x*v.y+q.c.x*v.x*v.z+q.c.y*v.y*v.z;
 let b=dot(gradient(q,u),v);let c=value(q,u);
 var high=max(c,value(q,u+duration*v));
 if a<0. {let vertex=-b/(2.*a);if vertex>0.&&vertex<duration {high=max(high,value(q,u+vertex*v));}}
 return high;
}
fn air_jump(o:vec3f,d:vec3f,pc:vec3u,cell_size:f32,enter:f32)->AirJump {
 var result:AirJump;result.t=-1.;result.cell=vec3i(0);result.normal=vec3f(0.);
 if temporal.tuning.w<.5 {return result;}
 let use_envelopes=temporal.tuning.w>1.5;
 let base=air_nodes[0]+(pc.y*air_nodes[3]+pc.z)*air_nodes[1]+pc.x;
 let base_air=air_nodes[base]!=0u;
 if !use_envelopes&&air_nodes[base]==0u{return result;}
 for(var l=i32(air_nodes[36])-1;l>=0;l--){
  let level=u32(l);let desc=level*4u;let width=1u<<level;let node=pc/width;
  let index=air_nodes[desc]+(node.y*air_nodes[desc+3u]+node.z)*air_nodes[desc+1u]+node.x;
  let all_air=air_nodes[index]!=0u;
  if !all_air&&(!use_envelopes||level<2u||level>4u){continue;}
  let low=vec3f(node*width)*frame.scene.w;
  let high=min(vec3f((node+vec3u(1))*width)*frame.scene.w,frame.scene.xyz);
  var ends=vec3f(1e30);
  for(var a=0u;a<3u;a++){if abs(d[a])>1e-12 {ends[a]=(select(low[a],high[a],d[a]>0.)-o[a])/d[a];}}
  let box_end=min(ends.x,min(ends.y,ends.z));var t=box_end;
  if !all_air {
   envelope_work++;
   let q=envelope_nodes[index-air_nodes[37]];
   let scale=2./(f32(width)*frame.scene.w);
   let u=(o+enter*d-low)*scale-vec3f(1.);let v=d*scale;
   if value(q,u)+q.c.z>=0. {continue;}
   let duration=max(0.,box_end-enter);
   let skip=first_possible(q,u,v,duration);
   // Check the entire interval, not only its endpoints. Uncertain signs
   // or near-tangent roots fall through to the smaller region / exact path.
   if skip<=max(0.0001,cell_size*2.)||interval_max(q,u,v,skip)+q.c.z*.5>=0. {continue;}
   t=min(box_end,enter+skip);
  }
  if t<=enter {continue;}
  var cell=clamp(vec3i(floor((o+t*d)/cell_size)),vec3i(round(low/cell_size)),vec3i(round(high/cell_size))-vec3i(1));
  var normal=vec3f(0.);
  for(var a=0u;a<3u;a++){if ends[a]<=t+0.000001 {
   cell[a]=i32(round(select(low[a],high[a],d[a]>0.)/cell_size))-select(1,0,d[a]>0.);
   normal=vec3f(0.);normal[a]=-sign(d[a]);
  }}
  result.t=t;result.cell=cell;result.normal=normal;return result;
 }
 return result;
}

fn value(q:Quad,p:vec3f)->f32 {
 return q.a.x+dot(q.a.yzw,p)+dot(q.b.xyz,p*p)+q.b.w*p.x*p.y+q.c.x*p.x*p.z+q.c.y*p.y*p.z;
}
fn gradient(q:Quad,p:vec3f)->vec3f {
 return q.a.yzw+2.*q.b.xyz*p+vec3f(q.b.w*p.y+q.c.x*p.z,q.b.w*p.x+q.c.y*p.z,q.c.x*p.x+q.c.y*p.y);
}
fn error_bound(q:Quad,p:vec3f)->f32 {
 let v=abs(p);return 0.000003*(1.+abs(q.a.x)+dot(abs(q.a.yzw),v)+dot(abs(q.b.xyz),v*v)+abs(q.b.w)*v.x*v.y+abs(q.c.x)*v.x*v.z+abs(q.c.y)*v.y*v.z);
}
struct Hit { t:f32, normal:vec3f, patch_id:u32, visits:u32, tests:u32 }
fn trace(o:vec3f,d:vec3f)->Hit {
 var h:Hit;h.t=-1.;h.normal=vec3f(0.,1.,0.);h.patch_id=0u;h.visits=0u;h.tests=0u;
 var enter=0.;var exit=1e30;
 for(var a=0u;a<3u;a++) {
  if abs(d[a])<1e-12 {if o[a]<0.||o[a]>=frame.scene[a]{return h;}}
  else {let t0=-o[a]/d[a];let t1=(frame.scene[a]-o[a])/d[a];enter=max(enter,min(t0,t1));exit=min(exit,max(t0,t1));}
 }
 if enter>=exit{return h;}
 let edge=frame.scene.w;let scale=2./edge;
 var cell=clamp(vec3i(floor((o+(enter+0.00001)*d)/edge)),vec3i(0),vec3i(frame.grid.xyz)-vec3i(1));
 var next=vec3f(1e30);var delta=vec3f(1e30);var step=vec3i(0);
 for(var a=0u;a<3u;a++){if abs(d[a])>1e-12 {
  step[a]=select(-1,1,d[a]>0.);delta[a]=edge/abs(d[a]);
  next[a]=((f32(cell[a])+select(0.,1.,d[a]>0.))*edge-o[a])/d[a];
 }}
 var last_bounds_patch=0xffffffffu;
 for(var visit=0u;visit<1024u;visit++) {
  if any(cell<vec3i(0))||any(cell>=vec3i(frame.grid.xyz))||enter>=exit{break;}
  h.visits++;
  let patch_id=(u32(cell.y)*frame.grid.z+u32(cell.z))*frame.grid.x+u32(cell.x);
  if temporal.tuning.w<1.5||patch_id!=last_bounds_patch {
   last_bounds_patch=patch_id;
   let jump=air_jump(o,d,vec3u(cell),edge,enter);
   if jump.t>enter {
    enter=jump.t;cell=jump.cell;
    for(var a=0u;a<3u;a++){if step[a]!=0 {next[a]=((f32(cell[a])+select(0.,1.,d[a]>0.))*edge-o[a])/d[a];}}
    continue;
   }
  }
  let end=min(exit,min(next.x,min(next.y,next.z)));
  let u=(o+enter*d-vec3f(cell)*edge)*scale-vec3f(1.);let v=d*scale;
  var best=1e30;var best_normal=vec3f(0.,1.,0.);
  for(var i=0u;i<frame.grid.w;i++) {
   let q=quad_at(patch_id*frame.grid.w+i);h.tests++;
   let aa=dot(q.b.xyz,v*v)+q.b.w*v.x*v.y+q.c.x*v.x*v.z+q.c.y*v.y*v.z;
   let bb=dot(gradient(q,u),v);let cc=value(q,u);
   var roots=vec2f(1e30);var count=0u;
   // Compare contributions over this finite interval, not unrelated coefficient units.
   let length=max(end-enter,0.000001);
   if abs(aa)*length*length<=1e-7*max(1.,max(abs(bb)*length,abs(cc))) {
    if abs(bb)>1e-12 {roots.x=-cc/bb;count=1u;}
   } else {
    let discriminant=bb*bb-4.*aa*cc;
    if discriminant>=0. {
     let root=sqrt(discriminant);let qq=-0.5*(bb+select(-root,root,bb>=0.));
     if qq==0.{roots.x=-bb/(2.*aa);count=1u;}
     else {roots=vec2f(qq/aa,cc/qq);count=2u;}
    }
   }
   for(var r=0u;r<count;r++) {
    let dt=roots[r];if dt< -0.00001||dt>end-enter+0.00001||dt>=best{continue;}
    let p=u+dt*v;var valid=true;
    for(var j=0u;j<frame.grid.w;j++){let other=quad_at(patch_id*frame.grid.w+j);if value(other,p)< -error_bound(other,p){valid=false;break;}}
    if valid {let g=gradient(q,p);if dot(g,g)>1e-20 {best=max(dt,0.);best_normal=-normalize(g);}}
   }
  }
  if best<1e29 {h.t=enter+best;h.normal=best_normal;h.patch_id=patch_id;return h;}
  enter=end;
  for(var a=0u;a<3u;a++){if next[a]<=end+0.000001 {cell[a]+=step[a];next[a]+=delta[a];}}
 }
 return h;
}
// On [-1,1]^3, this bounds the change from any ray point to its
// voxel center. Local half-cell width is voxel_size / patch_edge.
// Mean value theorem: |Q(center)-Q(point)| <= r * sum(max |dQ/daxis|).
fn center_margin(q:Quad)->f32 {
 // Precomputed once during coefficient packing, including f32 safety padding.
 return q.c.z;
}
// First possible nonnegative point of a quadratic upper envelope.
// Returns zero when already possible or numerically uncertain; no search.
fn first_possible(q:Quad,u:vec3f,v:vec3f,duration:f32)->f32 {
 let c=value(q,u)+center_margin(q);
 if c>=0.{return 0.;}
 let a=dot(q.b.xyz,v*v)+q.b.w*v.x*v.y+q.c.x*v.x*v.z+q.c.y*v.y*v.z;
 let b=dot(gradient(q,u),v);
 var answer=duration;
 if a==0. {if b>0.{answer=min(answer,-c/b);}}
 else {
  let disc=b*b-4.*a*c;
  let tolerance=0.000002*(b*b+abs(4.*a*c)+1e-20);
  if abs(disc)<=tolerance{return 0.;}
  if disc>0. {
   let qroot=-.5*(b+select(-sqrt(disc),sqrt(disc),b>=0.));
   if qroot==0.{return 0.;}
   let roots=vec2f(qroot/a,c/qroot);
   for(var i=0u;i<2u;i++){if roots[i]>0.{answer=min(answer,roots[i]);}}
  }
 }
 // Back off slightly for f32 root/coordinate roundoff. The residual is
 // handled by exact cell tests, never by declaring an approximate hit.
 return max(0.,answer-0.00002*(1.+abs(answer)));
}
// Exact test baseline: classify cell centers and traverse voxel boundaries.
// Intentionally separate from the cheap snapped-hit visualization.
fn trace_voxels(o:vec3f,d:vec3f,accelerate:bool)->Hit {
 var h:Hit;h.t=-1.;h.normal=vec3f(0.,1.,0.);h.patch_id=0u;h.visits=0u;h.tests=0u;
 var enter=0.;var exit=1e30;var normal=vec3f(0.,1.,0.);
 for(var a=0u;a<3u;a++) {
  if abs(d[a])<1e-12 {if o[a]<0.||o[a]>=frame.scene[a]{return h;}}
  else {let t0=-o[a]/d[a];let t1=(frame.scene[a]-o[a])/d[a];let near=min(t0,t1);
   if near>enter {enter=near;normal=vec3f(0.);normal[a]=-sign(d[a]);}exit=min(exit,max(t0,t1));}
 }
 if enter>=exit{return h;}
 let size=frame.up.w;let dims=vec3i(round(frame.scene.xyz/size));
 let p=(o+enter*d)/size;
 var cell=vec3i(floor(p));
 for(var a=0u;a<3u;a++){if d[a]<0.&&abs(p[a]-round(p[a]))<0.00001{cell[a]=i32(round(p[a]))-1;}}
 cell=clamp(cell,vec3i(0),dims-vec3i(1));
 var next=vec3f(1e30);var delta=vec3f(1e30);var step=vec3i(0);
 for(var a=0u;a<3u;a++){if abs(d[a])>1e-12{step[a]=select(-1,1,d[a]>0.);delta[a]=size/abs(d[a]);next[a]=((f32(cell[a])+select(0.,1.,d[a]>0.))*size-o[a])/d[a];}}
 // A finite grid crosses at most nx+ny+nz voxel boundaries.
 let limit=u32(dims.x+dims.y+dims.z)+3u;
 var last_bounds_patch=0xffffffffu;
 for(var iteration=0u;iteration<limit;iteration++) {
  if any(cell<vec3i(0))||any(cell>=dims)||enter>=exit{break;}
  h.visits++;
  let center=(vec3f(cell)+vec3f(.5))*size;
  let pc=min(vec3u(floor(center/frame.scene.w)),frame.grid.xyz-vec3u(1));
  let pid=(pc.y*frame.grid.z+pc.z)*frame.grid.x+pc.x;
  if accelerate && abs(frame.scene.w/size-round(frame.scene.w/size))<0.00001 && (temporal.tuning.w<1.5||pid!=last_bounds_patch) {
   last_bounds_patch=pid;
   let jump=air_jump(o,d,pc,size,enter);
   if jump.t>enter {
    enter=jump.t;cell=jump.cell;normal=jump.normal;
    for(var a=0u;a<3u;a++){if step[a]!=0 {next[a]=((f32(cell[a])+select(0.,1.,d[a]>0.))*size-o[a])/d[a];}}
    continue;
   }
  }
  let local=(center-vec3f(pc)*frame.scene.w)*(2./frame.scene.w)-vec3f(1.);
  var solid=true;var reject=0u;let spline=USE_SPLINE;
  if spline{reject=spline_piece(local);h.tests++;solid=value(quad_at(pid*24u+reject),local)>0.;}
  else{for(var j=0u;j<frame.grid.w;j++){h.tests++;if value(quad_at(pid*frame.grid.w+j),local)<=0.{solid=false;reject=j;break;}}}
  if solid {h.t=enter;h.normal=normal;h.patch_id=pid;return h;}
  if accelerate && abs(frame.scene.w/size-round(frame.scene.w/size))<0.00001 {
   // Never cross a patch boundary using another patch's polynomial.
   var patch_exit=exit;
   for(var a=0u;a<3u;a++){if abs(d[a])>1e-12{let boundary=(f32(pc[a])+select(0.,1.,d[a]>0.))*frame.scene.w;patch_exit=min(patch_exit,(boundary-o[a])/d[a]);}}
   let u=(o+enter*d-vec3f(pc)*frame.scene.w)*(2./frame.scene.w)-vec3f(1.);
   if spline {
    reject=spline_piece(u+d*(2./frame.scene.w)*1e-6);
    patch_exit=min(patch_exit,enter+spline_exit(reject,u,d*(2./frame.scene.w),max(0.,patch_exit-enter)));
   }
   h.tests++;
   let jump=first_possible(quad_at(pid*frame.grid.w+reject),u,d*(2./frame.scene.w),max(0.,patch_exit-enter));
   if jump>frame.up.w*0.5 {
    let point=(o+(enter+jump)*d)/size;
    var destination=vec3i(floor(point));
    for(var a=0u;a<3u;a++){if d[a]<0.&&abs(point[a]-round(point[a]))<0.00001{destination[a]=i32(round(point[a]))-1;}}
    if any(destination!=cell)&&all(destination>=vec3i(0))&&all(destination<dims) {
     cell=destination;
     // Recover the entry face/time of the destination voxel analytically.
     var entry=0.;normal=vec3f(0.,1.,0.);
     for(var a=0u;a<3u;a++){if abs(d[a])>1e-12{let near=(f32(cell[a])+select(1.,0.,d[a]>0.))*size;let t=(near-o[a])/d[a];if t>entry{entry=t;normal=vec3f(0.);normal[a]=-sign(d[a]);}next[a]=((f32(cell[a])+select(0.,1.,d[a]>0.))*size-o[a])/d[a];}}
     enter=entry;continue;
    }
   }
  }
  let end=min(next.x,min(next.y,next.z));enter=end;normal=vec3f(0.);var assigned=false;
  for(var a=0u;a<3u;a++){if next[a]<=end+0.0000001 {if !assigned{normal[a]=-f32(step[a]);assigned=true;}cell[a]+=step[a];next[a]=((f32(cell[a])+select(0.,1.,d[a]>0.))*size-o[a])/d[a];}}
 }
 return h;
}
struct Sample { color:vec3f, hit:Hit, direction:vec3f, envelope_tests:f32 }
fn sample_pixel(pixel:vec2f)->Sample {
 envelope_work=0u;
 let uv=(pixel+vec2f(.5)+temporal.jitter.xy)/vec2f(frame.options.xy)*2.-vec2f(1.);
 let d=normalize(frame.forward.xyz+frame.right.xyz*uv.x*frame.forward.w*frame.eye.w-frame.up.xyz*uv.y*frame.eye.w);
 var h:Hit;
 if frame.options.z==4u||frame.options.z==5u {h=trace_voxels(frame.eye.xyz,d,frame.options.z==4u);}else if USE_SPLINE{h=trace_spline(frame.eye.xyz,d);}else{h=trace(frame.eye.xyz,d);}
 var material_normal=h.normal;
 if (frame.options.z==3u||frame.options.z==4u||frame.options.z==5u)&&h.t>=0. {
  let snapped=(floor((frame.eye.xyz+d*h.t-h.normal*select(0.,frame.up.w*0.001,frame.options.z>=4u))/frame.up.w)+vec3f(.5))*frame.up.w;
  let pc=clamp(vec3i(floor(snapped/frame.scene.w)),vec3i(0),vec3i(frame.grid.xyz)-vec3i(1));
  let pid=(u32(pc.y)*frame.grid.z+u32(pc.z))*frame.grid.x+u32(pc.x);
  let local=(snapped-vec3f(pc)*frame.scene.w)*(2./frame.scene.w)-vec3f(1.);
  var nearest=1e30;
  if USE_SPLINE{material_normal=-normalize(gradient(quad_at(pid*24u+spline_piece(local)),local));}
  else{for(var j=0u;j<frame.grid.w;j++){let q=quad_at(pid*frame.grid.w+j);let v=value(q,local);if v<nearest{nearest=v;material_normal=-normalize(gradient(q,local));}}}
  if frame.options.z==3u {h.normal=vec3f(0.);let n=abs(material_normal);
  if n.x>=n.y&&n.x>=n.z{h.normal.x=sign(material_normal.x);}else if n.y>=n.z{h.normal.y=sign(material_normal.y);}else{h.normal.z=sign(material_normal.z);}}
 }
 var color=mix(vec3f(.38,.56,.73),vec3f(.65,.77,.86),clamp(d.y*.5+.5,0.,1.));
 if h.t>=0. {
  let p=frame.eye.xyz+d*h.t;
  let footprint=2.*h.t*frame.eye.w/(f32(frame.options.y)*max(abs(dot(material_normal,d)),.06));
  let filtered=select(0.,smoothstep(.4,1.8,footprint/frame.up.w)*temporal.tuning.y,temporal.state.z>0.5&&frame.options.z==4u);
  // Filter subpixel face lighting; geometry and intersection remain exact voxels.
  let sun=normalize(vec3f(-.4,.8,-.3));
  let diffuse=mix(max(0.,dot(h.normal,sun)),max(0.,dot(material_normal,sun)),filtered);
  let light=vec3f(.25,.29,.34)+vec3f(.83,.77,.64)*diffuse;
  let cell=vec3i(floor((p-h.normal*frame.up.w*.001)/frame.up.w));
  let patch_tone=hash_cell(vec3i(floor(vec3f(cell)/32.)))-.5;
  let grain=(hash_cell(cell)-.5)*(1.-smoothstep(.35,1.5,footprint/frame.up.w));
  let stone=vec3f(.36,.355,.33);let grass=vec3f(.25,.35,.13);
  let base=mix(stone,grass,smoothstep(.45,.85,material_normal.y));
  color=base*(1.+patch_tone*.08+grain*.16)*light;
  let fog=1.-exp(-h.t/(max(frame.scene.x,max(frame.scene.y,frame.scene.z))*8.));
  color=mix(color,vec3f(.52,.66,.78),fog);
 }
 if frame.options.z==1u {color=vec3f(f32(h.visits)/12.,.05,1.-min(1.,f32(h.visits)/12.));}
 if frame.options.z==2u {color=vec3f(min(1.,f32(h.tests)/60.),f32(h.tests)/120.,.05);}
 return Sample(color,h,d,f32(envelope_work));
}
fn spatial_offset(i:u32,n:u32)->vec2f {
 if n==1u{return vec2f(0.);}
 if n==2u {return select(vec2f(-.25,-.25),vec2f(.25,.25),i==1u);}
 if n==3u {let offsets=array<vec2f,3>(vec2f(-.3,-.25),vec2f(.3,-.25),vec2f(0.,.35));return offsets[i];}
 let offsets=array<vec2f,4>(vec2f(-.125,-.375),vec2f(.375,-.125),vec2f(.125,.375),vec2f(-.375,.125));return offsets[i];
}
@compute @workgroup_size(8,8)
fn main(@builtin(global_invocation_id) id:vec3u) {
 if id.x>=frame.options.x||id.y>=frame.options.y{return;}
 let n=select(1u,u32(clamp(round(temporal.tuning.z),1.,4.)),temporal.state.z>0.5);
 var first:Sample;var color=vec3f(0.);var visits=0.;var tests=0.;var envelopes=0.;
 for(var i=0u;i<n;i++){
  let sample=sample_pixel(vec2f(id.xy)+spatial_offset(i,n));
  if i==0u {first=sample;}
  color+=sample.color;visits+=f32(sample.hit.visits);tests+=f32(sample.hit.tests);envelopes+=sample.envelope_tests;
 }
 color/=f32(n);let h=first.hit;let d=first.direction;
 if frame.options.w!=0u && id.x%32u==0u && id.y%32u==0u {
  let stride=(frame.options.x+31u)/32u;probes[(id.y/32u)*stride+id.x/32u]=vec4f(h.t,visits/f32(n),tests/f32(n),envelopes/f32(n));
 }
 color=accumulate(color,select(frame.eye.xyz+d*max(h.t,0.),d,h.t<0.),h.t);
 textureStore(next_history,vec2i(id.xy),vec4f(color,h.t));
 var display=pow(clamp(color,vec3f(0.),vec3f(1.)),vec3f(1./2.2));
 if frame.right.w>0.5 {display=menu_pixel(id.xy,display);}
 textureStore(output,vec2i(id.xy),vec4f(display,1.));
}

const labels = array<u32,312>(82u,69u,78u,68u,69u,82u,73u,78u,71u,32u,79u,80u,84u,73u,79u,78u,83u,32u,32u,32u,32u,32u,32u,32u,32u,32u,67u,79u,78u,84u,73u,78u,85u,79u,85u,83u,32u,32u,32u,32u,32u,32u,32u,32u,32u,32u,32u,32u,32u,32u,32u,32u,83u,78u,65u,80u,80u,69u,68u,32u,72u,73u,84u,32u,65u,80u,80u,82u,79u,88u,73u,77u,65u,84u,69u,32u,32u,32u,69u,88u,65u,67u,84u,32u,86u,79u,88u,69u,76u,83u,32u,32u,32u,32u,32u,32u,32u,32u,32u,32u,32u,32u,32u,32u,80u,65u,84u,67u,72u,32u,72u,69u,65u,84u,77u,65u,80u,32u,32u,32u,32u,32u,32u,32u,32u,32u,32u,32u,32u,32u,81u,85u,65u,68u,82u,65u,84u,73u,67u,32u,72u,69u,65u,84u,77u,65u,80u,32u,32u,32u,32u,32u,32u,32u,32u,32u,84u,79u,71u,71u,76u,69u,32u,87u,65u,76u,75u,32u,70u,76u,89u,32u,45u,32u,71u,32u,32u,32u,32u,32u,32u,32u,65u,78u,84u,73u,65u,76u,73u,65u,83u,73u,78u,71u,32u,45u,32u,84u,32u,32u,32u,32u,32u,32u,32u,32u,32u,32u,84u,69u,77u,80u,79u,82u,65u,76u,32u,83u,84u,82u,69u,78u,71u,84u,72u,32u,32u,32u,32u,32u,32u,32u,32u,32u,70u,65u,67u,69u,32u,76u,73u,71u,72u,84u,73u,78u,71u,32u,83u,77u,79u,79u,84u,72u,73u,78u,71u,32u,32u,32u,83u,80u,65u,84u,73u,65u,76u,32u,83u,65u,77u,80u,76u,69u,83u,32u,32u,32u,32u,32u,32u,32u,32u,32u,32u,32u,82u,69u,83u,85u,77u,69u,32u,32u,32u,32u,32u,32u,32u,32u,32u,32u,32u,32u,32u,32u,32u,32u,32u,32u,32u,32u);
fn glyph(c:u32,row:u32)->u32{switch c{
case 65u: {let rows=array<u32,7>(14u,17u,17u,31u,17u,17u,17u);return rows[row];}
case 66u: {let rows=array<u32,7>(30u,17u,17u,30u,17u,17u,30u);return rows[row];}
case 67u: {let rows=array<u32,7>(15u,16u,16u,16u,16u,16u,15u);return rows[row];}
case 68u: {let rows=array<u32,7>(30u,17u,17u,17u,17u,17u,30u);return rows[row];}
case 69u: {let rows=array<u32,7>(31u,16u,16u,30u,16u,16u,31u);return rows[row];}
case 70u: {let rows=array<u32,7>(31u,16u,16u,30u,16u,16u,16u);return rows[row];}
case 71u: {let rows=array<u32,7>(15u,16u,16u,23u,17u,17u,15u);return rows[row];}
case 72u: {let rows=array<u32,7>(17u,17u,17u,31u,17u,17u,17u);return rows[row];}
case 73u: {let rows=array<u32,7>(31u,4u,4u,4u,4u,4u,31u);return rows[row];}
case 74u: {let rows=array<u32,7>(7u,2u,2u,2u,18u,18u,12u);return rows[row];}
case 75u: {let rows=array<u32,7>(17u,18u,20u,24u,20u,18u,17u);return rows[row];}
case 76u: {let rows=array<u32,7>(16u,16u,16u,16u,16u,16u,31u);return rows[row];}
case 77u: {let rows=array<u32,7>(17u,27u,21u,21u,17u,17u,17u);return rows[row];}
case 78u: {let rows=array<u32,7>(17u,25u,21u,19u,17u,17u,17u);return rows[row];}
case 79u: {let rows=array<u32,7>(14u,17u,17u,17u,17u,17u,14u);return rows[row];}
case 80u: {let rows=array<u32,7>(30u,17u,17u,30u,16u,16u,16u);return rows[row];}
case 81u: {let rows=array<u32,7>(14u,17u,17u,17u,21u,18u,13u);return rows[row];}
case 82u: {let rows=array<u32,7>(30u,17u,17u,30u,20u,18u,17u);return rows[row];}
case 83u: {let rows=array<u32,7>(15u,16u,16u,14u,1u,1u,30u);return rows[row];}
case 84u: {let rows=array<u32,7>(31u,4u,4u,4u,4u,4u,4u);return rows[row];}
case 85u: {let rows=array<u32,7>(17u,17u,17u,17u,17u,17u,14u);return rows[row];}
case 86u: {let rows=array<u32,7>(17u,17u,17u,17u,17u,10u,4u);return rows[row];}
case 87u: {let rows=array<u32,7>(17u,17u,17u,21u,21u,21u,10u);return rows[row];}
case 88u: {let rows=array<u32,7>(17u,17u,10u,4u,10u,17u,17u);return rows[row];}
case 89u: {let rows=array<u32,7>(17u,17u,10u,4u,4u,4u,4u);return rows[row];}
case 90u: {let rows=array<u32,7>(31u,1u,2u,4u,8u,16u,31u);return rows[row];}
case 48u: {let rows=array<u32,7>(14u,17u,19u,21u,25u,17u,14u);return rows[row];}
case 49u: {let rows=array<u32,7>(4u,12u,4u,4u,4u,4u,14u);return rows[row];}
case 50u: {let rows=array<u32,7>(14u,17u,1u,2u,4u,8u,31u);return rows[row];}
case 51u: {let rows=array<u32,7>(30u,1u,1u,14u,1u,1u,30u);return rows[row];}
case 52u: {let rows=array<u32,7>(2u,6u,10u,18u,31u,2u,2u);return rows[row];}
case 53u: {let rows=array<u32,7>(31u,16u,16u,30u,1u,1u,30u);return rows[row];}
case 54u: {let rows=array<u32,7>(14u,16u,16u,30u,17u,17u,14u);return rows[row];}
case 55u: {let rows=array<u32,7>(31u,1u,2u,4u,8u,8u,8u);return rows[row];}
case 56u: {let rows=array<u32,7>(14u,17u,17u,14u,17u,17u,14u);return rows[row];}
case 57u: {let rows=array<u32,7>(14u,17u,17u,15u,1u,1u,14u);return rows[row];}
default:{return 0u;} }}
const traversal_labels=array<u32,125>(84u,82u,65u,86u,69u,82u,83u,65u,76u,32u,32u,32u,32u,32u,32u,32u,32u,32u,32u,32u,32u,32u,32u,32u,32u,71u,82u,73u,68u,32u,66u,65u,83u,69u,76u,73u,78u,69u,32u,32u,32u,32u,32u,32u,32u,32u,32u,32u,32u,32u,65u,73u,82u,32u,72u,73u,69u,82u,65u,82u,67u,72u,89u,32u,32u,32u,32u,32u,32u,32u,32u,32u,32u,32u,32u,81u,85u,65u,68u,82u,65u,84u,73u,67u,32u,69u,78u,86u,69u,76u,79u,80u,69u,83u,32u,32u,32u,32u,32u,32u,72u,65u,82u,68u,87u,65u,82u,69u,32u,85u,78u,65u,86u,65u,73u,76u,65u,66u,76u,69u,32u,32u,32u,32u,32u);
fn traversal_panel(id:vec2u,base:vec3f)->vec3f {
 if id.x<436u||id.x>=806u||id.y<20u||id.y>=240u {return base;}
 let p=id-vec2u(436u,20u);let row=p.y/44u;let cy=p.y%44u;
 var color=select(vec3f(.07,.09,.13),vec3f(.16,.2,.27),row>0u);
 if row>0u&&row<4u&&u32(round(temporal.tuning.w))==row-1u {color=vec3f(.16,.38,.3);}
 if p.x>=12u&&cy>=8u&&cy<22u {let x=(p.x-12u)/2u;if x/6u<25u&&x%6u<5u {
 let c=traversal_labels[row*25u+x/6u];if (glyph(c,(cy-8u)/2u)&(1u<<(4u-x%6u)))!=0u {return select(vec3f(.92,.95,1.),vec3f(.46),row==4u);}
 }}return color;
}
fn menu_pixel(id:vec2u,base:vec3f)->vec3f {
 if id.x>=436u{return traversal_panel(id,base);}
 if id.x<20u||id.x>=420u||id.y<20u||id.y>=548u{return base;}
 let p=id-vec2u(20u);var color=vec3f(.07,.09,.13);
 let line=p.y/44u;let cy=p.y%44u;
 if line>0u&&line<=11u&&cy<36u {color=vec3f(.16,.2,.27);let modes=array<u32,11>(0u,3u,4u,1u,2u,98u,97u,96u,95u,94u,99u);if modes[line-1u]==frame.options.z {color=vec3f(.16,.38,.3);}}
 if line==7u&&cy<36u&&temporal.state.z>0.5 {color=vec3f(.16,.38,.3);}
 if (line==8u||line==9u||line==10u)&&cy>=28u&&cy<35u&&p.x>=16u&&p.x<=384u {
  let strength=select(select(temporal.tuning.x,temporal.tuning.y,line==9u),(temporal.tuning.z-1.)/3.,line==10u);
  let knob=16.+368.*strength;
  if abs(f32(p.x)-knob)<4. {return vec3f(.96,.88,.58);}
  return select(vec3f(.08,.11,.15),vec3f(.3,.65,.48),f32(p.x)<knob);
 }
 if (line==8u||line==9u||line==10u)&&p.x>=340u&&p.x<376u&&cy>=8u&&cy<22u {
  let amount=u32(round(select(100.*select(temporal.tuning.x,temporal.tuning.y,line==9u),temporal.tuning.z,line==10u)));
  let digit=(p.x-340u)/12u;let col=((p.x-340u)%12u)/2u;
  let place=select(select(1u,10u,digit==1u),100u,digit==0u);
  if col<5u&&(glyph(48u+(amount/place)%10u,(cy-8u)/2u)&(1u<<(4u-col)))!=0u{return vec3f(.96,.88,.58);}
 }
 if line<12u&&p.x>=16u {let x=(p.x-16u)/2u;let y=cy/2u;if x/6u<26u&&x%6u<5u&&y>=4u&&y<11u {let c=labels[line*26u+x/6u];if (glyph(c,y-4u)&(1u<<(4u-x%6u)))!=0u{return vec3f(.92,.95,1.);}}}
 return color;
}

// Same type-6 partition as tools/quadratic_spline.py.
const TET_BARY=array<mat4x4f,24>(
mat4x4f(vec4f(1.0,0.0,0.0,0.0),vec4f(1.0,-1.0,-0.0,-0.0),vec4f(-0.0,-0.0,-0.5,0.5),vec4f(-0.0,1.0,-0.5,-0.5)),
mat4x4f(vec4f(1.0,0.0,0.0,0.0),vec4f(1.0,-1.0,-0.0,-0.0),vec4f(0.0,-1.0,0.5,0.5),vec4f(0.0,0.0,-0.5,0.5)),
mat4x4f(vec4f(1.0,0.0,0.0,0.0),vec4f(1.0,-1.0,-0.0,-0.0),vec4f(0.0,0.0,0.5,-0.5),vec4f(0.0,-1.0,0.5,0.5)),
mat4x4f(vec4f(1.0,0.0,0.0,0.0),vec4f(1.0,-1.0,-0.0,-0.0),vec4f(-0.0,1.0,-0.5,-0.5),vec4f(-0.0,-0.0,0.5,-0.5)),
mat4x4f(vec4f(1.0,0.0,0.0,0.0),vec4f(-1.0,1.0,0.0,0.0),vec4f(-0.0,-0.0,-0.5,0.5),vec4f(-0.0,1.0,-0.5,-0.5)),
mat4x4f(vec4f(1.0,0.0,0.0,0.0),vec4f(-1.0,1.0,0.0,0.0),vec4f(0.0,-1.0,0.5,0.5),vec4f(0.0,0.0,-0.5,0.5)),
mat4x4f(vec4f(1.0,0.0,0.0,0.0),vec4f(-1.0,1.0,0.0,0.0),vec4f(0.0,0.0,0.5,-0.5),vec4f(0.0,-1.0,0.5,0.5)),
mat4x4f(vec4f(1.0,0.0,0.0,0.0),vec4f(-1.0,1.0,0.0,0.0),vec4f(-0.0,1.0,-0.5,-0.5),vec4f(-0.0,-0.0,0.5,-0.5)),
mat4x4f(vec4f(1.0,0.0,0.0,0.0),vec4f(-0.0,-0.0,-0.5,0.5),vec4f(1.0,-1.0,-0.0,-0.0),vec4f(0.0,1.0,-0.5,-0.5)),
mat4x4f(vec4f(1.0,0.0,0.0,0.0),vec4f(0.0,-1.0,0.5,0.5),vec4f(1.0,-1.0,-0.0,-0.0),vec4f(0.0,0.0,-0.5,0.5)),
mat4x4f(vec4f(1.0,0.0,0.0,0.0),vec4f(0.0,0.0,0.5,-0.5),vec4f(1.0,-1.0,-0.0,-0.0),vec4f(-0.0,-1.0,0.5,0.5)),
mat4x4f(vec4f(1.0,0.0,0.0,0.0),vec4f(-0.0,1.0,-0.5,-0.5),vec4f(1.0,-1.0,-0.0,-0.0),vec4f(-0.0,-0.0,0.5,-0.5)),
mat4x4f(vec4f(1.0,0.0,0.0,0.0),vec4f(-0.0,-0.0,-0.5,0.5),vec4f(-1.0,1.0,0.0,0.0),vec4f(0.0,1.0,-0.5,-0.5)),
mat4x4f(vec4f(1.0,0.0,0.0,0.0),vec4f(0.0,-1.0,0.5,0.5),vec4f(-1.0,1.0,0.0,0.0),vec4f(0.0,0.0,-0.5,0.5)),
mat4x4f(vec4f(1.0,0.0,0.0,0.0),vec4f(0.0,0.0,0.5,-0.5),vec4f(-1.0,1.0,0.0,0.0),vec4f(-0.0,-1.0,0.5,0.5)),
mat4x4f(vec4f(1.0,0.0,0.0,0.0),vec4f(-0.0,1.0,-0.5,-0.5),vec4f(-1.0,1.0,0.0,0.0),vec4f(-0.0,-0.0,0.5,-0.5)),
mat4x4f(vec4f(1.0,0.0,0.0,0.0),vec4f(-0.0,-0.0,-0.5,0.5),vec4f(-0.0,1.0,-0.5,-0.5),vec4f(1.0,-1.0,-0.0,-0.0)),
mat4x4f(vec4f(1.0,0.0,0.0,0.0),vec4f(0.0,-1.0,0.5,0.5),vec4f(0.0,0.0,-0.5,0.5),vec4f(1.0,-1.0,-0.0,-0.0)),
mat4x4f(vec4f(1.0,0.0,0.0,0.0),vec4f(0.0,0.0,0.5,-0.5),vec4f(0.0,-1.0,0.5,0.5),vec4f(1.0,-1.0,-0.0,-0.0)),
mat4x4f(vec4f(1.0,0.0,0.0,0.0),vec4f(-0.0,1.0,-0.5,-0.5),vec4f(-0.0,-0.0,0.5,-0.5),vec4f(1.0,-1.0,-0.0,-0.0)),
mat4x4f(vec4f(1.0,0.0,0.0,0.0),vec4f(-0.0,-0.0,-0.5,0.5),vec4f(-0.0,1.0,-0.5,-0.5),vec4f(-1.0,1.0,0.0,0.0)),
mat4x4f(vec4f(1.0,0.0,0.0,0.0),vec4f(0.0,-1.0,0.5,0.5),vec4f(0.0,0.0,-0.5,0.5),vec4f(-1.0,1.0,0.0,0.0)),
mat4x4f(vec4f(1.0,0.0,0.0,0.0),vec4f(0.0,0.0,0.5,-0.5),vec4f(0.0,-1.0,0.5,0.5),vec4f(-1.0,1.0,0.0,0.0)),
mat4x4f(vec4f(1.0,0.0,0.0,0.0),vec4f(-0.0,1.0,-0.5,-0.5),vec4f(-0.0,-0.0,0.5,-0.5),vec4f(-1.0,1.0,0.0,0.0))
);
fn spline_piece(p:vec3f)->u32 {
 let v=abs(p);var axis=0u;if v.y>v.x{axis=1u;}if v.z>v[axis]{axis=2u;}
 var a=0u;var b=1u;if axis==0u{a=1u;b=2u;}else if axis==1u{a=0u;b=2u;}
 var edge=0u;
 if v[a]>=v[b]{edge=select(3u,1u,p[a]>=0.);}else{edge=select(0u,2u,p[b]>=0.);}
 return axis*8u+select(0u,4u,p[axis]>=0.)+edge;
}
fn spline_exit(piece:u32,u:vec3f,v:vec3f,duration:f32)->f32 {
 let l=TET_BARY[piece]*vec4f(1.,u);let d=TET_BARY[piece]*vec4f(0.,v);var end=duration;
 for(var a=0u;a<4u;a++){if d[a]< -1e-12 {end=min(end,-l[a]/d[a]);}}
 return max(0.,end);
}
fn ray_roots(q:Quad,u:vec3f,v:vec3f,duration:f32)->vec2f {
 let a=dot(q.b.xyz,v*v)+q.b.w*v.x*v.y+q.c.x*v.x*v.z+q.c.y*v.y*v.z;
 let b=dot(gradient(q,u),v);let c=value(q,u);let length=max(duration,1e-6);
 if abs(a)*length*length<=1e-7*max(1.,max(abs(b)*length,abs(c))){if abs(b)>1e-12{return vec2f(-c/b,1e30);}return vec2f(1e30);}
 let disc=b*b-4.*a*c;if disc<0.{return vec2f(1e30);}
 let qr=-.5*(b+select(-sqrt(disc),sqrt(disc),b>=0.));
 if qr==0.{return vec2f(-b/(2.*a),1e30);}return vec2f(qr/a,c/qr);
}
fn trace_spline(o:vec3f,d:vec3f)->Hit {
 var h:Hit;h.t=-1.;h.normal=vec3f(0.,1.,0.);h.patch_id=0u;h.visits=0u;h.tests=0u;
 var enter=0.;var exit=1e30;
 for(var a=0u;a<3u;a++){if abs(d[a])<1e-12{if o[a]<0.||o[a]>=frame.scene[a]{return h;}}else{let t0=-o[a]/d[a];let t1=(frame.scene[a]-o[a])/d[a];enter=max(enter,min(t0,t1));exit=min(exit,max(t0,t1));}}
 if enter>=exit{return h;}
 let edge=frame.scene.w;let scale=2./edge;let v=d*scale;
 var cell=clamp(vec3i(floor((o+(enter+1e-6)*d)/edge)),vec3i(0),vec3i(frame.grid.xyz)-vec3i(1));
 var next=vec3f(1e30);var step=vec3i(0);
 for(var a=0u;a<3u;a++){if abs(d[a])>1e-12{step[a]=select(-1,1,d[a]>0.);next[a]=((f32(cell[a])+select(0.,1.,d[a]>0.))*edge-o[a])/d[a];}}
 for(var visit=0u;visit<1024u;visit++){
  if any(cell<vec3i(0))||any(cell>=vec3i(frame.grid.xyz))||enter>=exit{break;}
  let pid=(u32(cell.y)*frame.grid.z+u32(cell.z))*frame.grid.x+u32(cell.x);
  let cube_end=min(exit,min(next.x,min(next.y,next.z)));var t=enter;
  for(var segment=0u;segment<64u;segment++){
   if t>=cube_end{break;}
   h.visits++;let u=(o+t*d-vec3f(cell)*edge)*scale-vec3f(1.);
   let piece=spline_piece(u+v*min(1e-6,(cube_end-t)*.25));
   let duration=spline_exit(piece,u,v,cube_end-t);let q=quad_at(pid*24u+piece);h.tests++;
   let roots=ray_roots(q,u,v,duration);var best=1e30;
   for(var r=0u;r<2u;r++){if roots[r]>=-1e-6&&roots[r]<=duration+1e-6{best=min(best,max(0.,roots[r]));}}
   if best<1e29{let g=gradient(q,u+best*v);if dot(g,g)>1e-20{h.t=t+best;h.normal=-normalize(g);h.patch_id=pid;return h;}}
   t+=max(duration,1e-6);
  }
  enter=cube_end;
  for(var a=0u;a<3u;a++){if next[a]<=cube_end+1e-6{cell[a]+=step[a];next[a]=((f32(cell[a])+select(0.,1.,d[a]>0.))*edge-o[a])/d[a];}}
 }
 return h;
}
