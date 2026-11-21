struct View { matrix:mat4x4<f32>, sun:vec4<f32>, eye:vec4<f32> };
@group(0) @binding(0) var<uniform> view:View;
struct V { @builtin(position) clip:vec4<f32>, @location(0) normal:vec3<f32>, @location(1) color:vec3<f32>, @location(2) local:vec3<f32> };
@vertex fn vs(@location(0) p:vec3<f32>, @location(1) c:vec3<f32>, @location(2) n:vec3<f32>, @builtin(vertex_index) id:u32)->V {
    var pos=p;
    var normal=n;
    if f32(id)>=view.eye.w {
        let hub=vec2(0.,-.06);
        let a=cos(view.sun.w);let b=sin(view.sun.w);
        let rotation=mat2x2<f32>(vec2(a,b),vec2(-b,a));
        let rotated=rotation*(p.xy-hub)+hub;
        pos=vec3(rotated,p.z);
        normal=vec3(rotation*n.xy,n.z);
    }
    return V(view.matrix*vec4(pos,1.),normal,c,pos);
}
@fragment fn fs(v:V)->@location(0) vec4<f32> {
    let n=normalize(v.normal);
    let eye=normalize(view.eye.xyz-v.local);
    let half_vec=normalize(eye+view.sun.xyz);
    let sky=vec3(.28,.34,.42)*(.78+.22*n.y);
    let direct=vec3(.95,.83,.68)*max(dot(n,view.sun.xyz),0.);
    let glass=v.color.b>2.*v.color.r;
    let gloss=pow(max(dot(n,half_vec),0.),select(32.,72.,glass));
    return vec4(v.color*(sky+direct)+vec3(gloss*select(.15,.75,glass)),1.);
}
