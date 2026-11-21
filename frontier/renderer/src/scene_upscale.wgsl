@group(0) @binding(0) var image: texture_2d<f32>;
@group(0) @binding(1) var image_sampler: sampler;
struct Output { @builtin(position) position: vec4<f32>, @location(0) uv: vec2<f32> };
@vertex fn vs_main(@builtin(vertex_index) i:u32)->Output {
 var p=array<vec2<f32>,3>(vec2(-1.0,-1.0),vec2(3.0,-1.0),vec2(-1.0,3.0));
 var o:Output; o.position=vec4(p[i],0.0,1.0); o.uv=vec2(p[i].x*0.5+0.5,0.5-p[i].y*0.5); return o;
}
const cubic_enabled:bool=false;

fn read_color(pixel:vec2<f32>,size:vec2<f32>)->vec3<f32> {
    return textureSampleLevel(image,image_sampler,pixel/size,0.).rgb;
}
@fragment fn fs_main(i:Output)->@location(0) vec4<f32> {
    if !cubic_enabled {return textureSampleLevel(image,image_sampler,i.uv,0.);}
    let size=vec2<f32>(textureDimensions(image));
    let pixel=i.uv*size;
    let center=floor(pixel-.5)+.5;
    let f=pixel-center;
    // The two inner Catmull-Rom weights are positive. A hardware bilinear
    // lookup evaluates their product for the central 2x2 footprint.
    let w0=f*(-.5+f*(1.-.5*f));
    let w1=1.+f*f*(-2.5+1.5*f);
    let w2=f*(.5+f*(2.-1.5*f));
    let w3=.5*f*f*(f-1.);
    let inner=w1+w2;
    let middle=center+w2/inner;
    let left=w0.x*inner.y;
    let right=w3.x*inner.y;
    let above=inner.x*w0.y;
    let below=inner.x*w3.y;
    let core=inner.x*inner.y;
    let center_color=read_color(middle,size);
    let top=read_color(vec2(middle.x,center.y-1.),size);
    let bottom=read_color(vec2(middle.x,center.y+2.),size);
    let west=read_color(vec2(center.x-1.,middle.y),size);
    let east=read_color(vec2(center.x+2.,middle.y),size);
    let sum=core*center_color+above*top+below*bottom+left*west+right*east;
    let color=sum/(core+above+below+left+right);
    // Bound negative-lobe overshoot at high-contrast silhouettes.
    let low=min(center_color,min(min(top,bottom),min(west,east)));
    let high=max(center_color,max(max(top,bottom),max(west,east)));
    return vec4(clamp(color,low,high),1.);
}
