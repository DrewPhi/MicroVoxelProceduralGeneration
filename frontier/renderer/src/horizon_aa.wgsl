// Smooth distant silhouettes and faded grazing-surface detail. Near voxel
// faces and aircraft geometry keep the ordinary scene resolve.
fn smooth_horizon(pixel:vec2i,uv:vec2f,color:vec3f)->vec3f {
    let radius=atmosphere.lighting.w;
    if radius<=0. || (atmosphere.beauty.z>.5 && atmosphere.beauty.z<2.5) {return color;}
    let depth=horizon_depth(pixel);
    if depth<.99997 {return color;}
    let sky_pixel=depth>=1.;
    let own_sample=horizon_color(pixel);
    let own_haze=horizon_extinction(own_sample.a);
    let size=vec2i(textureDimensions(scene));
    let neighbors=array(vec2i(0,-1),vec2i(0,1),vec2i(-1,0),vec2i(1,0));
    let own_distance=length(reconstruct(uv,depth));
    let minimum=clamp(radius*.30,700.,2400.);
    var edge_sum=vec3f(0.);
    var edge_weight=0.;
    var ground_sum=vec3f(0.);
    var ground_weight=0.;
    for(var i=0u;i<4u;i+=1u) {
        let next=clamp(pixel+neighbors[i],vec2i(0),size-1);
        let next_depth=horizon_depth(next);
        let next_sky=next_depth>=1.;
        if next_depth<.99997 || (sky_pixel && next_sky) {continue;}
        let next_uv=(vec2f(next)+.5)/vec2f(size);
        let sample_color=horizon_color(next);
        if next_sky != sky_pixel {
            let distance=select(own_distance,length(reconstruct(next_uv,next_depth)),sky_pixel);
            let amount=.125*smoothstep(minimum,minimum+650.,distance);
            if amount>0. {
                edge_sum+=amount*composite(sample_color,next_depth,next_uv);
                edge_weight+=amount;
            }
        } else {
            // Preserve depth and fade at the center. Average the surface
            // radiance before applying atmosphere and tone mapping.
            let haze_overlap=min(own_haze,horizon_extinction(sample_color.a));
            let amount=.15*smoothstep(.025,.31,haze_overlap);
            ground_sum+=amount*sample_color.rgb;
            ground_weight+=amount;
        }
    }
    var center=color;
    if ground_weight>0. {
        let surface=own_sample.rgb*(1.-ground_weight)+ground_sum;
        center=composite(vec4f(surface,own_sample.a),depth,uv);
    }
    return center*(1.-edge_weight)+edge_sum;
}
