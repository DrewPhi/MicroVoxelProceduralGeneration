// Portable bounded experiment: one invocation per procedural stroke, then one
// per native grid cell. It consumes growth strokes, never CPU native voxels.
struct Params { models:u32, strokes:u32, cells:u32, pad:u32 }
@group(0) @binding(0) var<uniform> params:Params;
@group(0) @binding(1) var<storage,read> strokes:array<vec4<u32>>;
@group(0) @binding(2) var<storage,read> seeds:array<vec2<u32>>;
@group(0) @binding(3) var<storage,read_write> owners:array<atomic<u32>>;
@group(0) @binding(4) var<storage,read_write> pigments:array<u32>;
@group(0) @binding(5) var<storage,read_write> colors:array<vec4<f32>>;
const VOLUME:u32=17424u; // 33 x 16 x 33, native cells [-16,16] x [0,15]
fn address(p:vec3<i32>)->u32 { return u32(p.x+16)*528u+u32(p.y)*33u+u32(p.z+16); }
@compute @workgroup_size(64)
fn raster(@builtin(global_invocation_id) id:vec3<u32>) {
 if id.x>=params.strokes {return;}
 let a=strokes[id.x*2u]; let b=strokes[id.x*2u+1u];
 var p=bitcast<vec3<i32>>(a.xyz); let end=bitcast<vec3<i32>>(b.xyz);
 loop {
  atomicMax(&owners[a.w*VOLUME+address(p)],id.x+1u);
  if all(p==end) {break;}
  let delta=end-p; let distance=abs(delta); var axis=0u;
  if distance.y>=distance.x {axis=1u;}
  if distance.z>=distance[axis] {axis=2u;}
  p[axis]+=select(-1,1,delta[axis]>0);
 }
}
fn high_product(a:u32,b:u32)->u32 {
 let a0=a&65535u;let a1=a>>16u;let b0=b&65535u;let b1=b>>16u;
 let u=a0*b0;let t=a1*b0+(u>>16u);let v=a0*b1+(t&65535u);
 return a1*b1+(t>>16u)+(v>>16u);
}
fn multiply(a:vec2<u32>,b:vec2<u32>)->vec2<u32> {
 return vec2<u32>(a.x*b.x,high_product(a.x,b.x)+a.x*b.y+a.y*b.x);
}
fn shift_right(a:vec2<u32>,n:u32)->vec2<u32> {return vec2<u32>((a.x>>n)|(a.y<<(32u-n)),a.y>>n);}
fn hash_mix64(value:vec2<u32>)->vec2<u32> {
 var x=multiply(value^shift_right(value,30u),vec2<u32>(0x1ce4e5b9u,0xbf58476du));
 x=multiply(x^shift_right(x,27u),vec2<u32>(0x133111ebu,0x94d049bbu));
 return x^shift_right(x,31u);
}
fn signed64(x:i32)->vec2<u32> {return vec2<u32>(bitcast<u32>(x),select(0u,0xffffffffu,x<0));}
fn plant_hash(seed:vec2<u32>,x:i32,z:i32)->vec2<u32> {
 return hash_mix64(seed^multiply(signed64(x),vec2<u32>(0x7f4a7c15u,0x9e3779b9u))^multiply(signed64(z),vec2<u32>(0x6659fd93u,0xd6e8feb8u)));
}
fn unit_hash(seed:vec2<u32>,x:i32,z:i32)->f32 {
 let h=plant_hash(seed,x,z);return f32(h.y)*(1.0/4294967296.0)+f32(h.x>>11u)*(1.0/9007199254740992.0);
}
fn pigment_patch(seed:vec2<u32>,x:i32,z:i32)->f32 {
 let gx=i32(floor(f32(x)/6.0));let gz=i32(floor(f32(z)/6.0));
 let t=vec2<f32>(f32(x-gx*6),f32(z-gz*6))/6.0;let uv=t*t*(vec2<f32>(3.0)-2.0*t);
 let a=unit_hash(seed,gx,gz)*(1.0-uv.x)+unit_hash(seed,gx+1,gz)*uv.x;
 let b=unit_hash(seed,gx,gz+1)*(1.0-uv.x)+unit_hash(seed,gx+1,gz+1)*uv.x;
 return a*(1.0-uv.y)+b*uv.y;
}
@compute @workgroup_size(64)
fn resolve(@builtin(global_invocation_id) id:vec3<u32>) {
 if id.x>=params.cells {return;}
 let winner=atomicLoad(&owners[id.x]);
 if winner==0u {pigments[id.x]=0u;colors[id.x]=vec4<f32>(0.0);return;}
 let packed=strokes[(winner-1u)*2u+1u].w;pigments[id.x]=packed;
 let local=id.x%VOLUME;
 let p=vec3<i32>(i32(local/528u)-16,i32((local/33u)%16u),i32(local%33u)-16);
 let seed=seeds[id.x/VOLUME];
 let grain_hash=plant_hash(seed,p.x^(p.y<<13),p.z);
 let grain=f32(((grain_hash.y%101u)*68u+grain_hash.x%101u)%101u)/100.0;
 let variation=0.94+0.09*pigment_patch(seed,p.x+p.y/2,p.z)+0.025*grain;
 let rgb=vec3<f32>(f32(packed&255u),f32((packed>>8u)&255u),f32((packed>>16u)&255u));
 let c=min(rgb/255.0*variation,vec3<f32>(1.0));
 let linear=select(pow((c+0.055)/1.055,vec3<f32>(2.4)),c/12.92,c<=vec3<f32>(0.04045));
 colors[id.x]=vec4<f32>(linear,1.0);
}
