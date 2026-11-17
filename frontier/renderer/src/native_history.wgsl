struct Params {
    inverse:mat4x4<f32>,
    previous:mat4x4<f32>,
    eye_delta:vec4<f32>,
};
@group(0) @binding(0) var<uniform> params:Params;
@group(0) @binding(1) var scene:texture_2d<f32>;
@group(0) @binding(2) var depth:texture_depth_2d;
@group(0) @binding(3) var history:texture_2d<f32>;
@vertex fn vs_main(@builtin(vertex_index) i:u32)->@builtin(position) vec4<f32> {
    let p=vec2(f32((i<<1u)&2u),f32(i&2u));
    return vec4(p*2.-1.,0.,1.);
}
// Only native hits write history depth. Negative alpha marks those pixels
// in the history texture and stores their forward distance.
@fragment fn fs_main(@builtin(position) position:vec4<f32>)->@location(0) vec4<f32> {
    let pixel=vec2<i32>(position.xy);
    let center=textureLoad(scene,pixel,0);
    if center.a<=2. {return vec4(center.rgb,0.);}
    let size=vec2<i32>(textureDimensions(scene));
    let z=textureLoad(depth,pixel,0);
    let ndc=position.xy/vec2<f32>(size)*vec2(2.,-2.)+vec2(-1.,1.);
    let point=params.inverse*vec4(ndc,z,1.);
    let output_depth=1./point.w;
    if params.eye_delta.w<0. {
        let old=textureLoad(history,pixel,0);
        // At rest, both frames integrate the same pixel footprint. Its
        // subpixel rays can legitimately hit different terraces or materials.
        let tolerance=clamp(output_depth*0.05,.2,1.2);
        if old.a<0. && abs(-old.a-output_depth)<tolerance {
            return vec4(mix(center.rgb,old.rgb,-params.eye_delta.w),-output_depth);
        }
        return vec4(center.rgb,-output_depth);
    }
    let offsets=array(vec2(-1,0),vec2(1,0),vec2(0,-1),vec2(0,1));
    var low=center.rgb;
    var high=center.rgb;
    var current=center.rgb;
    let reach=max((1.-z)*0.07,1.e-7);
    for(var i=0u;i<4u;i+=1u) {
        let p=clamp(pixel+offsets[i],vec2(0),size-1);
        let other=textureLoad(scene,p,0);
        if other.a>2. && abs(textureLoad(depth,p,0)-z)<reach {
            low=min(low,other.rgb);
            high=max(high,other.rgb);
            current+=(other.rgb-center.rgb)*0.045;
        }
    }
    let sand_footprint=clamp(center.a-2.25,0.,1.);
    if sand_footprint>0. {
        // Staggered taps break the repeating pattern from single-ray voxel
        // risers. Constrain the filter to the same depth layer and material.
        let taps=array(vec2(-1,-1),vec2(0,-2),vec2(1,-1),vec2(-2,0),
                       vec2(2,0),vec2(-1,1),vec2(0,2),vec2(1,1));
        var sum=current*2.;
        var weight=2.;
        let tolerance=max((1.-z)*0.13,2.e-7);
        for(var i=0u;i<8u;i+=1u) {
            let p=clamp(pixel+taps[i],vec2(0),size-1);
            let other=textureLoad(scene,p,0);
            if other.a>2.25 {
                let delta=abs(textureLoad(depth,p,0)-z);
                let w=1.-smoothstep(tolerance*0.35,tolerance,delta);
                sum+=other.rgb*w;
                weight+=w;
            }
        }
        current=mix(current,sum/weight,sand_footprint);
    }
    if params.eye_delta.w>0. {
        let old_clip=params.previous*vec4(point.xyz/point.w+params.eye_delta.xyz,1.);
        if old_clip.w>0. {
            let uv=old_clip.xy/old_clip.w*vec2(.5,-.5)+.5;
            let coord=uv*vec2<f32>(size)-.5;
            let base=vec2<i32>(floor(coord));
            if all(base>=vec2(0)) && all(base+1<size) {
                let f=fract(coord);
                let tolerance=clamp(old_clip.w*0.025,.12,.8);
                var color=vec3(0.);
                var weight=0.;
                for(var y=0i;y<2i;y+=1i){for(var x=0i;x<2i;x+=1i){
                    let previous=textureLoad(history,base+vec2(x,y),0);
                    if previous.a<0. && abs(-previous.a-old_clip.w)<tolerance {
                        let w=select(1.-f.x,f.x,x==1i)*select(1.-f.y,f.y,y==1i);
                        color+=previous.rgb*w;
                        weight+=w;
                    }
                }}
                if weight>0.35 {
                    let retained=clamp(color/weight,low-0.02,high+0.02);
                    current=mix(current,retained,params.eye_delta.w*weight);
                }
            }
        }
    }
    return vec4(current,-output_depth);
}
