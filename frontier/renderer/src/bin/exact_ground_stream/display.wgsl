struct View {
    eye: vec4f,
    forward: vec4f,
    right: vec4f,
    up: vec4f,
    size: vec4u,
    camera_cell: vec4i,
}
@group(0) @binding(0) var<uniform> view: View;
@group(0) @binding(1) var<storage,read> pixels:array<vec2u>;
override SRGB_TARGET:bool=true;

struct Varying {
    @builtin(position) position:vec4f,
    @location(0) uv:vec2f,
}
@vertex fn vert(@builtin(vertex_index) id:u32)->Varying {
    let u=vec2f(f32((id<<1u)&2u),f32(id&2u));
    return Varying(vec4f(u*vec2f(2.,-2.)+vec2f(-1.,1.),0.,1.),u);
}
fn ray(pixel:vec2i)->vec3f {
    let uv=(vec2f(pixel)+.5)*2./vec2f(view.size.xy)-1.;
    return normalize(view.forward.xyz+uv.x*view.right.xyz-uv.y*view.up.xyz);
}
fn at(pixel:vec2i)->vec2u {
    let q=vec2u(clamp(pixel,vec2i(0),vec2i(view.size.xy)-1));
    return pixels[q.y*view.size.x+q.x];
}
fn shade_ground(pixel:vec2i, hit:vec2u)->vec3f {
    let faces=array(vec3f(0,1,0),vec3f(-1,0,0),vec3f(1,0,0),vec3f(0,0,-1),vec3f(0,0,1));
    let colors=array(vec3f(.35,.37,.39),vec3f(.18,.34,.10),vec3f(.32,.22,.13),vec3f(.62,.52,.32),vec3f(.88,.92,.96));
    let depth=bitcast<f32>(hit.x);
    let surface=faces[hit.y>>8u];
    var n=surface;
    let width=depth*2.*length(view.up.xyz)/f32(view.size.y);
    // Recover a support normal from exact nearby hits to filter the lighting.
    // Stop at depth discontinuities and keep the original visible face.
    if width>.03 {
        let x=vec2i(1,0);
        let y=vec2i(0,1);
        let tx=vec2f(bitcast<f32>(at(pixel-x).x),bitcast<f32>(at(pixel+x).x));
        let ty=vec2f(bitcast<f32>(at(pixel-y).x),bitcast<f32>(at(pixel+y).x));
        let limit=max(.5,depth*.08);
        if all(tx>vec2f(0.)) && all(ty>vec2f(0.))
            && all(abs(tx-depth)<vec2f(limit)) && all(abs(ty-depth)<vec2f(limit)) {
            let dx=tx.y*ray(pixel+x)-tx.x*ray(pixel-x);
            let dy=ty.y*ray(pixel+y)-ty.x*ray(pixel-y);
            let support=cross(dx,dy);
            if dot(support,support)>1e-12 {
                let up=normalize(select(-support,support,support.y>=0.));
                n=normalize(mix(surface,up,smoothstep(.03,.24,width)));
            }
        }
    }
    let sun=normalize(vec3f(-.54,.66,-.52));
    let light=.39+.16*max(n.y,0.)+.52*max(dot(n,sun),0.);
    return colors[hit.y&255u]*light;
}
@fragment fn shade(input:Varying)->@location(0) vec4f {
    let p=vec2i(input.uv*vec2f(view.size.xy));
    let h=at(p);
    let depth=bitcast<f32>(h.x);
    var color=mix(vec3f(.59,.70,.83),vec3f(.23,.44,.73),smoothstep(0.,.65,max(ray(p).y,0.)));
    if depth>=0. {color=shade_ground(p,h);}
    if !SRGB_TARGET {
        color=select(12.92*color,1.055*pow(color,vec3f(1./2.4))-.055,color>vec3f(.0031308));
    }
    return vec4f(color,1.);
}
