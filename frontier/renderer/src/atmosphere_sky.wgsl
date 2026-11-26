// Bounded, half-resolution cloud lighting. This texture contains premultiplied
// cloud scattering RGB and clear-sky transmittance A, never terrain colour.
struct AtmosphereUniform {
    inverse_view_projection:mat4x4<f32>,relative_eye:vec4<f32>,world_eye:vec4<f32>,
    sun:vec4<f32>,fog:vec4<f32>,weather:vec4<f32>,beauty:vec4<f32>,
    lighting: vec4<f32>, // shafts, streaming haze, region count, horizon radius
    far_shadow: mat4x4<f32>,
    haze_regions: array<vec4<f32>,8>,
    storm_origin:vec4f,storm_extent:vec4f,storm_low:vec4f,storm_high:vec4f,
    celestial:vec4f,moon:vec4f,
    regional_origin:vec4f,regional_extent:vec4f,
    cloud_phase:vec4f,
};
@group(0) @binding(0) var<uniform> atmosphere:AtmosphereUniform;
// STORM_DEPTH_BINDING
@group(0) @binding(2) var previous_cloud:texture_3d<f32>;
@group(0) @binding(3) var current_cloud:texture_3d<f32>;
@group(0) @binding(4) var storm_sampler:sampler;
@group(0) @binding(5) var previous_regional:texture_3d<f32>;
@group(0) @binding(6) var current_regional:texture_3d<f32>;
struct VertexOutput {@builtin(position) position:vec4<f32>,@location(0) uv:vec2<f32>};
@vertex fn vs_main(@builtin(vertex_index) i:u32)->VertexOutput {
    var p=array<vec2<f32>,3>(vec2(-1.,-1.),vec2(3.,-1.),vec2(-1.,3.));
    var o:VertexOutput;o.position=vec4(p[i],0.,1.);o.uv=vec2(p[i].x*.5+.5,.5-p[i].y*.5);return o;
}
fn condensate(p:vec3f)->vec2f {
    let uv=(p-atmosphere.storm_origin.xyz)/atmosphere.storm_extent.xyz;
    let v=mix(textureSampleLevel(previous_cloud,storm_sampler,uv,0.).rg,
              textureSampleLevel(current_cloud,storm_sampler,uv,0.).rg,atmosphere.storm_low.w);
    let edge=min(uv,1.-uv);
    return v*smoothstep(0.,.047,min(min(edge.x,1.-uv.y),edge.z));
}

fn distant_water(p:vec3f)->vec2f {
    let uv=(p-atmosphere.regional_origin.xyz)/atmosphere.regional_extent.xyz;
    if any(uv<vec3f(0.)) || any(uv>vec3f(1.)) { return vec2f(0.); }
    let water=mix(textureSampleLevel(previous_regional,storm_sampler,uv,0.).rg,
                  textureSampleLevel(current_regional,storm_sampler,uv,0.).rg,atmosphere.regional_origin.w);
    return water*vec2f(smoothstep(250.,750.,p.y),1.);
}

fn cloud_body(shape:vec4f)->f32 {
    return smoothstep(.28,.74,.64*shape.r+.36*shape.g);
}

fn carved_condensate(water:f32,shape:vec4f,detail:f32)->f32 {
    // Rounded lobes keep their water. Fine cells carve the gaps and fringes.
    let billow=cloud_body(shape);
    let cavities=1.-smoothstep(.28,.70,shape.b);
    let erosion=mix(.035,.006,billow)+.021*cavities*detail;
    let pocket_fill=mix(.27,.63,smoothstep(.065,.24,water));
    let core=mix(pocket_fill,2.32,pow(billow,1.45));
    return max(0.,water-erosion)*max(.17,core+.32*detail*(shape.b-.5));
}

fn local_cloud_shape(p:vec3f)->vec4f {
    return cloud_texture(p*vec3f(.00033,.00036,.00034)+atmosphere.cloud_phase.xyz);
}

fn local_cloud_water(p:vec3f,water:f32,shape:vec4f,detail:f32)->f32 {
    // The weather grid supplies the large-scale column. Lift the bulbs and
    // lower the pockets inside its padded bounds to break up flat layer tops.
    let relief=(.76*(shape.r-.52)+.24*(shape.g-.47))*790.;
    let shifted=condensate(p-vec3f(0.,relief,0.)).x;
    return carved_condensate(mix(water,shifted,.88),shape,detail);
}

fn weather_sun_color(sun_y:f32)->vec3f {
    return mix(vec3f(1.24,.66,.31),vec3f(1.04,1.01,.96),smoothstep(.015,.34,sun_y));
}

fn sculpted_cloud_light(direct:f32,mu:f32,water:f32,sun_color:vec3f)->vec3f {
    let ambient=mix(vec3f(.074,.132,.230),vec3f(.38,.48,.62),direct);
    let forward=.48+.54*pow(mu,5.);
    // Retain soft fill inside the body and let the illuminated surfaces
    // pick up warm directional light along their thinner edges.
    let fill=ambient*(.84+.16*(1.-exp(-water*4.)));
    return fill+sun_color*(direct*forward);
}

fn cloud_march_phase(p:vec3f)->f32 {
    // A continuous world-space offset breaks up coincident march layers at
    // grazing angles without screen-locked grain or a changing frame seed.
    return mix(.21,.79,cloud_texture(p*.0011+atmosphere.cloud_phase.xyz).b);
}

fn regional_clouds(eye:vec3f,ray:vec3f,scene_range:f32,angular:f32)->vec4f {
    if atmosphere.regional_extent.w<1.5 {return vec4f(0.,0.,0.,1.);}
    let slope=select(min(ray.y,-.00001),max(ray.y,.00001),ray.y>=0.);
    let levels=(vec2f(250.,11400.)-eye.y)/slope;
    let begin=max(0.,min(levels.x,levels.y));
    let end=min(scene_range,max(levels.x,levels.y));
    if end<=begin {return vec4f(0.,0.,0.,1.);}
    let sun=atmosphere.sun.xyz;
    let mu=max(0.,dot(ray,sun));
    let sunlight=weather_sun_color(sun.y);
    var rgb=vec3f(0.);
    var trans=1.;
    let compact=atmosphere.storm_origin.w>1.5;
    let steps=select(30u,select(17u,23u,atmosphere.storm_origin.w>2.5),compact);
    let phase=cloud_march_phase(eye+ray*(begin+min(6300.,(end-begin)*.3)));
    for (var i=0u;i<steps;i+=1u) {
        let a=f32(i)/f32(steps);
        let b=f32(i+1u)/f32(steps);
        // Denser sampling near the camera, with continuous world-space nodes.
        let t=mix(begin,end,mix(a*a,b*b,phase));
        let step=(b*b-a*a)*(end-begin);
        let p=eye+t*ray;
        let local=(p-atmosphere.storm_origin.xyz)/atmosphere.storm_extent.xyz;
        let edge=min(local,1.-local);
        let residue=1.-smoothstep(0.,.11,min(min(edge.x,edge.z),edge.y));
        if residue<.003 {continue;}
        let water=distant_water(p);
        if water.x+water.y<.004 {continue;}
        let haze=1.-exp(-t/56000.);
        let footprint=max(angular*t,step*.5);
        let texture_weight=(1.-smoothstep(390.,2300.,footprint))*smoothstep(.014,.095,abs(ray.y));
        let coord=p*vec3f(.000112,.000094,.000117)+atmosphere.cloud_phase.xyz;
        let shape=mix(vec4f(.52),cloud_texture(coord),texture_weight);
        let cloud=carved_condensate(water.x,shape,texture_weight);
        let drops=water.y*.00043;
        let extinction=cloud*.00124+drops;
        let distance_fade=1.-smoothstep(79000.,120000.,t);
        let alpha=1.-exp(-extinction*step*residue*distance_fade);
        let light_at=p+sun*970.;
        let light_shape=mix(vec4f(.52),cloud_texture(coord+sun*vec3f(.109,.091,.113)),texture_weight);
        let above=carved_condensate(distant_water(light_at).x,light_shape,texture_weight);
        let direct=exp(-5.1*(cloud*.31+above));
        let lit=sculpted_cloud_light(direct,mu,cloud,sunlight);
        let color=mix(vec3f(.32,.39,.51),lit,cloud/(cloud+water.y*.42+.0001));
        rgb+=trans*alpha*mix(color,vec3f(.43,.53,.65),haze*.64);
        trans*=1.-alpha;
        if trans<.018 {break;}
    }
    return vec4f(rgb*cloud_daylight(),trans);
}

fn simulated_clouds(ray:vec3f,uv:vec2f,angular:f32)->vec4f {
    let eye=atmosphere.world_eye.xyz;
    let pixel=clamp(vec2i(uv*vec2f(textureDimensions(scene_depth))),vec2i(0),vec2i(textureDimensions(scene_depth))-1);
    let depth=STORM_DEPTH_LOAD;
    let scene=atmosphere.inverse_view_projection*vec4f(uv.x*2.-1.,1.-uv.y*2.,depth,1.);
    let limit=select(20000.,120000.,atmosphere.regional_extent.w>1.5);
    let scene_range=select(min(limit,length(scene.xyz/max(abs(scene.w),1.e-6)-atmosphere.relative_eye.xyz)),limit,depth>=1.);
    if atmosphere.storm_high.w<.5 {return regional_clouds(eye,ray,scene_range,angular);}
    let lo=atmosphere.storm_origin.xyz+atmosphere.storm_extent.xyz*atmosphere.storm_low.xyz;
    let hi=atmosphere.storm_origin.xyz+atmosphere.storm_extent.xyz*atmosphere.storm_high.xyz;
    let inverse=1./select(min(ray,vec3f(-1.e-5)),max(ray,vec3f(1.e-5)),ray>=vec3f(0.));
    let near=min((lo-eye)*inverse,(hi-eye)*inverse);
    let far=max((lo-eye)*inverse,(hi-eye)*inverse);
    let start=max(0.,max(near.x,max(near.y,near.z)));
    let end=min(min(scene_range,20000.),min(far.x,min(far.y,far.z)));
    if end<=start {return regional_clouds(eye,ray,scene_range,angular);}
    let compact=atmosphere.storm_origin.w>1.5;
    let steps=select(36u,select(24u,31u,atmosphere.storm_origin.w>2.5),compact);
    let regional=atmosphere.regional_extent.w>.5;
    var light=vec3f(0.);
    var transmission=1.;
    let sun=atmosphere.sun.xyz;
    let sun_color=mix(vec3f(1.,.73,.42),vec3f(1.04,1.01,.96),smoothstep(.02,.30,sun.y));
    let side_light=weather_sun_color(sun.y);
    let forward=max(dot(ray,sun),0.);
    var phase=.5;
    if regional {
        phase=mix(.5,cloud_march_phase(eye+ray*(start+min(3200.,(end-start)*.3))),
                  1.-smoothstep(.045,.29,abs(ray.y)));
    }
    for (var i=0u;i<steps;i+=1u) {
        let a=f32(i)/f32(steps);
        let b=f32(i+1u)/f32(steps);
        // Bias sampling toward the first visible surface. Farther inside,
        // a longer step carries the corresponding optical path length.
        let front=select(0.,.58,regional);
        let lo=mix(a,a*a,front);
        let hi=mix(b,b*b,front);
        let step=(hi-lo)*(end-start);
        let t=mix(start,end,mix(lo,hi,phase));
        let p=eye+ray*t;
        let water=condensate(p);
        if max(water.x,water.y)>.002 {
            let detail_weight=1.-smoothstep(90.,510.,max(t*angular,step));
            // Erode thin edges using a filtered 3D noise field. The simulated
            // condensate remains the support of the visible cloud.
            var cloud=water.x;
            var detail_sample=vec4f(.5);
            if !regional && atmosphere.sun.w>.5 && cloud>.004 {
                detail_sample=cloud_texture(p*vec3f(.00027,.00031,.00025)+.61*atmosphere.cloud_phase.xyz);
            }
            if !regional && detail_weight>.01 && cloud>.004 {
                var detail=detail_sample.b;
                if atmosphere.sun.w<=.5 { detail=noise3(p*vec3f(.006,.007,.006)); }
                cloud=max(0.,cloud-(.78-detail)*.22*detail_weight*(1.-smoothstep(.18,.64,cloud)));
                cloud*=1.+(detail-.5)*.85*detail_weight;
            }
            if regional && cloud>.002 {
                let footprint=max(t*angular,step);
                let grazing=mix(1.,smoothstep(.016,.12,abs(ray.y)),smoothstep(3500.,13000.,t));
                let broad_weight=(1.-smoothstep(420.,1380.,footprint))*grazing;
                let shape=mix(vec4f(.52),local_cloud_shape(p),broad_weight);
                cloud=local_cloud_water(p,cloud,shape,detail_weight);
            }
            let drops=water.y*.0020;
            let liquid=cloud*select(.0082,.0104,regional);
            let extinction=liquid+drops;
            if extinction<1.e-6 {continue;}
            let alpha=1.-exp(-extinction*step);
            if regional {
                let near=p+sun*185.;
                let blocker=local_cloud_shape(near);
                let local_shadow=carved_condensate(condensate(near).x,blocker,detail_weight*.70);
                let body_shadow=condensate(p+sun*740.).x;
                let direct=exp(-1.65*cloud-8.85*local_shadow-2.3*body_shadow);
                let cloud_color=sculpted_cloud_light(direct,forward,cloud,side_light);
                let rain_color=mix(vec3f(.19,.24,.34),vec3f(.51,.55,.63),direct);
                let color=mix(rain_color,cloud_color,liquid/extinction);
                light+=transmission*alpha*color;
                transmission*=1.-alpha;
                if transmission<.009 {break;}
                continue;
            }
            var into_sun=water.x*.6;
            if !compact {
                into_sun=condensate(p+sun*180.).x+condensate(p+sun*620.).x*.82;
            } else if cloud>.012 {
                into_sun+=condensate(p+sun*340.).x;
            }
            let direct=exp(-into_sun*4.3);
            let ambient=mix(vec3f(.30,.36,.45),vec3f(.59,.67,.78),direct);
            let cloud_color=ambient*.81+sun_color*direct*.65;
            let rain_color=mix(vec3f(.26,.30,.38),vec3f(.49,.55,.62),direct);
            let color=mix(rain_color,cloud_color,liquid/max(extinction,1.e-6));
            light+=transmission*alpha*color;
            transmission*=1.-alpha;
            if transmission<.009 {break;}
        }
    }
    if atmosphere.storm_extent.w>.4 && scene_range>2. {
        // Close drops follow the local rain field and fall in world space.
        // The bulk rainwater also forms a depth-tested shaft above.
        let p=eye+ray*min(scene_range,8.2);
        let streak=p*vec3f(2.6,.17,2.6)+vec3f(0.,atmosphere.weather.w,0.);
        var cell=floor(streak);
        cell.y-=floor(cell.y/1024.)*1024.;
        let offset=vec2f(hash3(cell),hash3(cell+vec3f(3.1,0.,8.2)));
        let g=length(fract(streak.xz)-offset);
        let lines=(1.-smoothstep(.012,.052,g))*smoothstep(.2,.37,fract(streak.y));
        let a=lines*clamp(atmosphere.storm_extent.w/46.,0.,.42);
        light=mix(light,vec3f(.67,.76,.88),a);
        transmission*=1.-a;
    }
    let near_color=light*cloud_daylight();
    if transmission>.012 {
        let distant=regional_clouds(eye,ray,scene_range,angular);
        return vec4f(near_color+transmission*distant.rgb,transmission*distant.a);
    }
    return vec4f(near_color,transmission);
}

fn cloud_daylight()->vec3f {
    let night=vec3f(.025,.043,.082)*(1.+atmosphere.celestial.z*2.2);
    let twilight=atmosphere.celestial.y*vec3f(.18,.095,.11);
    return mix(night+twilight,vec3f(1.),atmosphere.celestial.x);
}

@fragment fn fs_main(i:VertexOutput)->@location(0) vec4<f32> {
    let p=atmosphere.inverse_view_projection*vec4(i.uv.x*2.-1.,1.-i.uv.y*2.,1.,1.);
    let ray=normalize(p.xyz-atmosphere.relative_eye.xyz*p.w);
    let dx=dpdx(ray);
    let dy=dpdy(ray);
    let angular_footprint=sqrt(.5*(dot(dx,dx)+dot(dy,dy)));
    if atmosphere.storm_origin.w>.5 {return simulated_clouds(ray,i.uv,angular_footprint);}
    let eye_y=atmosphere.world_eye.y;
    let cloud_top=CLOUD_BASE+cloud_thickness();
    const MAX_DISTANCE:f32=19500.;
    const STEPS:u32=28u;
    if atmosphere.weather.x<=.001 { return vec4(0.,0.,0.,1.); }
    if abs(ray.y)<1.e-6 && (eye_y<=CLOUD_BASE || eye_y>=cloud_top) {
        return vec4(0.,0.,0.,1.);
    }
    let slope=select(min(ray.y,-1.e-6),max(ray.y,1.e-6),ray.y>=0.);
    let bottom=(CLOUD_BASE-eye_y)/slope;
    let top=(cloud_top-eye_y)/slope;
    let start=max(min(bottom,top),0.);
    let end=min(max(bottom,top),MAX_DISTANCE);
    if end<=start { return vec4(0.,0.,0.,1.); }
    // Integrate a continuous slab with fixed nodes. There are no angle-dependent
    // step counts, slice-index changes, or early-opacity discontinuities.
    let step=(end-start)/f32(STEPS);
    var transmittance=1.;
    var light=vec3(0.);
    let warm=1.-smoothstep(.04,.45,atmosphere.sun.y);
    let sun_color=mix(vec3(1.12,1.08,1.00),vec3(1.15,.85,.58),warm*.7);
    for(var k=0u;k<STEPS;k+=1u) {
        let distance=start+(f32(k)+.5)*step;
        let position=atmosphere.world_eye.xyz+ray*distance;
        // Filter for both march spacing and the stretched footprint of an
        // angular pixel at a shallow intersection.
        let footprint=max(step*1.20,1.30*distance*angular_footprint/max(.09,abs(ray.y)));
        let frequency=select(vec2f(.0011,.00339),vec2f(.00057,.00181),atmosphere.sun.w>.5);
        let octave_weights=1.-smoothstep(vec2f(.32,.22),vec2f(1.03,.67),footprint*frequency);
        // Let the remaining distant cloud contrast decay into the clear sky.
        let fade=1.-smoothstep(8500.,19000.,distance);
        let rho=density(position,octave_weights)*fade;
        if rho>0. {
            var sun_density:f32;
            let h=clamp((position.y-CLOUD_BASE)/cloud_thickness(),0.,1.);
            var ambient=mix(vec3(.40,.48,.60),vec3(.88,.93,1.00),smoothstep(.12,.73,h));
            var scattering=.68;
            var cloud_fill=1.;
            if atmosphere.sun.w>.5 {
                // Integrate light toward the top of the same cloud slab.
                // Layer depth controls the contrast through the cloud body.
                let to_top=clamp((cloud_top-position.y)/max(.20,atmosphere.sun.y),120.,1650.);
                let middle=density(position+atmosphere.sun.xyz*(.36*to_top),octave_weights);
                let far=density(position+atmosphere.sun.xyz*(.72*to_top),octave_weights*vec2f(1.,.48));
                let near_light=density(position+atmosphere.sun.xyz*min(92.,.12*to_top),octave_weights);
                sun_density=.0039*to_top*(.32*near_light+.43*middle+.25*far);
                ambient=mix(vec3f(.41,.49,.61),vec3f(.78,.862,.985),smoothstep(.06,.91,h));
                let cosine=max(0.,dot(ray,atmosphere.sun.xyz));
                scattering=.61+.59*cosine*cosine*cosine;
                cloud_fill=1.+.23*exp(-.32*sun_density);
            } else {
                sun_density=3.2*density(position+atmosphere.sun.xyz*155.,octave_weights);
            }
            let direct=exp(-sun_density*.90)*(1.-atmosphere.weather.z*.65);
            // Approximate the low-order multiple-scattering fill that keeps a
            // sunlit cumulus body pale rather than a dark pillow with a rim.
            let source=ambient*.72*cloud_fill+sun_color*direct*scattering;
            let alpha=1.-exp(-rho*step*.0045);
            light+=transmittance*alpha*source;transmittance*=1.-alpha;
            if atmosphere.sun.w>.5 && transmittance<.001 {break;}
        }
    }
    return vec4(light*cloud_daylight(),transmittance);
}
