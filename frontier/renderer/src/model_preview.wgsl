@group(0) @binding(0) var<uniform> inverse_camera: mat4x4f;
struct Output {
    @builtin(position) position: vec4f,
    @location(0) ndc: vec2f,
}
@vertex fn vertex(@builtin(vertex_index) i:u32)->Output {
    var tri=array(vec2f(-1.,-1.),vec2f(3.,-1.),vec2f(-1.,3.));
    return Output(vec4f(tri[i],1.,1.),tri[i]);
}
@fragment fn fragment(in:Output)->@location(0) vec4f {
    let a=inverse_camera*vec4f(in.ndc,0.,1.);
    let b=inverse_camera*vec4f(in.ndc,1.,1.);
    let origin=a.xyz/a.w;
    let direction=normalize(b.xyz/b.w-origin);
    let distance=(-1.36-origin.y)/min(direction.y,-.0001);
    let backdrop=mix(vec3f(.042,.07,.10),vec3f(.105,.15,.19),clamp(in.ndc.y*.45+.55,0.,1.));
    let p=origin+distance*direction;
    let plane=select(0.,1.-smoothstep(16.,49.,distance),direction.y<0. && distance>0.);
    let tile=abs(fract(p.xz+.5)-.5);
    let span=clamp(fwidth(p.xz),vec2f(.009),vec2f(2.));
    let line=1.-min(smoothstep(0.,span.x,tile.x),smoothstep(0.,span.y,tile.y));
    let wings=exp(-dot((p.xz-vec2f(0.,-.36))/vec2f(3.5,.62),(p.xz-vec2f(0.,-.36))/vec2f(3.5,.62)));
    let hull=exp(-dot((p.xz-vec2f(0.,-.6))/vec2f(.62,2.8),(p.xz-vec2f(0.,-.6))/vec2f(.62,2.8)));
    let floor=(backdrop*.82+line*.021)*(1.-max(wings,hull)*.65);
    return vec4f(mix(backdrop,floor,plane),1.);
}
