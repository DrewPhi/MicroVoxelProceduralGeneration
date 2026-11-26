// Greenfield analytical exponential-height fog and a single world-anchored cloud
// sheet. Fog is independent of geometry availability. Distances are metres.
struct AtmosphereUniform {
    inverse_view_projection: mat4x4<f32>,
    relative_eye: vec4<f32>, // camera position, projected cloud shadows enabled
    world_eye: vec4<f32>,
    sun: vec4<f32>,
    fog: vec4<f32>, // density, height falloff, exposure, enabled
    weather: vec4<f32>, // coverage, speed, overcast, elapsed seconds
    beauty: vec4<f32>, // contract v2: enabled, shadowed air enabled, debug mode, reserved
    lighting: vec4<f32>, // shafts, streaming haze, region count, horizon radius (0 disables)
    far_shadow: mat4x4<f32>,
    haze_regions: array<vec4<f32>,8>,
    storm_origin:vec4f,storm_extent:vec4f,storm_low:vec4f,storm_high:vec4f,
    celestial:vec4f,moon:vec4f,
    regional_origin:vec4f,regional_extent:vec4f,
    cloud_phase:vec4f,
};
@group(0) @binding(0) var<uniform> atmosphere: AtmosphereUniform;
// SCENE_BINDINGS
@group(0) @binding(3) var meadow_air:texture_2d<f32>;
@group(0) @binding(4) var cloud_lighting:texture_2d<f32>;
@group(0) @binding(5) var cloud_sampler:sampler;
@group(0) @binding(6) var lunar_voxels:texture_3d<f32>;
@group(0) @binding(7) var previous_shadow_cloud:texture_3d<f32>;
@group(0) @binding(8) var current_shadow_cloud:texture_3d<f32>;
@group(0) @binding(9) var previous_shadow_regional:texture_3d<f32>;
@group(0) @binding(10) var current_shadow_regional:texture_3d<f32>;

struct VertexOutput { @builtin(position) position: vec4<f32>, @location(0) uv: vec2<f32> };
@vertex fn vs_main(@builtin(vertex_index) index: u32) -> VertexOutput {
    var p = array<vec2<f32>, 3>(vec2(-1.0,-1.0),vec2(3.0,-1.0),vec2(-1.0,3.0));
    var out: VertexOutput;
    out.position = vec4(p[index],0.0,1.0);
    out.uv = vec2(p[index].x * 0.5 + 0.5, 0.5 - p[index].y * 0.5);
    return out;
}
fn hash(p: vec2<f32>) -> f32 {
    // No screen-space noise: this deterministic field moves only with world wind.
    let q = fract(p * vec2(0.1031,0.11369));
    let h = q + dot(q,q.yx + vec2(19.19));
    return fract((h.x+h.y)*h.x);
}
fn noise(p: vec2<f32>) -> f32 {
    let i = floor(p); let f = fract(p); let u = f*f*(3.0-2.0*f);
    return mix(mix(hash(i),hash(i+vec2(1.0,0.0)),u.x),
        mix(hash(i+vec2(0.0,1.0)),hash(i+vec2(1.0,1.0)),u.x),u.y);
}
fn cloud_field(p: vec2<f32>) -> f32 {
    return noise(p)*0.58 + noise(p*2.03+vec2(13.4,7.8))*0.28 + noise(p*4.11)*0.14;
}
fn clear_sky(ray: vec3<f32>) -> vec3<f32> {
    let elevation = max(ray.y,0.0);
    let sunset = 1.0-smoothstep(0.04,0.45,atmosphere.sun.y);
    let horizon = mix(vec3(0.63,0.77,0.88),vec3(0.94,0.63,0.40),sunset*0.7);
    let zenith = mix(vec3(0.16,0.37,0.66),vec3(0.18,0.25,0.43),sunset);
    var color = mix(horizon,zenith,pow(elevation,0.45));
    let sun_dot = max(dot(ray,atmosphere.sun.xyz),0.0);
    let sun_color = mix(vec3(1.0,0.91,0.72),vec3(1.0,0.48,0.22),sunset);
    color += sun_color * (pow(sun_dot,32.0)*0.13 + smoothstep(0.99988,0.99996,sun_dot)*2.0);
    return mix(color,vec3(0.57,0.62,0.67),atmosphere.weather.z*0.72);
}
fn sky(ray: vec3<f32>) -> vec3<f32> {
    var color = clear_sky(ray);
    let coverage = atmosphere.weather.x;
    // A plane at 1800 m: no volumetric loops, bounded low-frequency detail.
    if abs(ray.y) > 0.0001 && coverage > 0.001 {
        let distance = (1800.0-atmosphere.world_eye.y)/ray.y;
        if distance < 0. || distance >= 30000. { return color; }
        let wind = vec2(1.0,0.32)*atmosphere.weather.w*atmosphere.weather.y;
        let p = (atmosphere.world_eye.xz+ray.xz*distance)*0.00034+wind;
        let value = cloud_field(p);
        let threshold = mix(0.88,0.22,coverage);
        let density = smoothstep(threshold,threshold+0.17,value);
        let horizon_fade = (1.-smoothstep(12000.,30000.,distance))*smoothstep(.01,.09,abs(ray.y));
        let cloud = mix(vec3(0.94,0.94,0.91),vec3(0.60,0.64,0.68),atmosphere.weather.z);
        let shade = 0.80 + 0.20*smoothstep(threshold,threshold+0.25,value);
        color = mix(color,cloud*shade,density*horizon_fade*0.87);
    }
    return color;
}
// Beauty profile: greenfield artistic single-scattering approximation. World
// positions and actual depth alone drive fog; no clip distance/LOD inputs exist.
fn beauty_sky(ray: vec3<f32>, solar_disc: f32) -> vec3<f32> {
    let elevation = max(ray.y, 0.0);
    let dusk = 1.0 - smoothstep(0.02, 0.40, atmosphere.sun.y);
    let forward = max(dot(ray, atmosphere.sun.xyz), 0.0);
    let sun_color = mix(vec3(1.0, 0.91, 0.74), vec3(1.0, 0.55, 0.29), dusk);
    let natural = atmosphere.sun.w > .5;
    let horizon = mix(select(vec3f(.48,.63,.76),vec3f(.44,.61,.76),natural), vec3(.72,.52,.40), dusk*.65);
    let zenith = mix(select(vec3f(.065,.22,.46),vec3f(.030,.132,.330),natural), vec3(.08,.13,.27), dusk);
    // Reduce the vertical slope of the sky colours close to the horizon.
    let horizon_offset = pow(.014,.42);
    let gradient = (pow(elevation+.014,.42)-horizon_offset)/(pow(1.014,.42)-horizon_offset);
    var color = mix(horizon, zenith, clamp(gradient,0.,1.));
    // Broad low-energy Mie lobe plus a correctly small angular solar disc.
    color += sun_color * (pow(forward, 12.0) * 0.055 + pow(forward, 96.0) * 0.17);
    color += sun_color * smoothstep(0.999975, 0.999989, forward) * solar_disc;
    if atmosphere.storm_origin.w>.5 && atmosphere.regional_extent.w>.5 {
        let warmth=dusk*dusk*smoothstep(-.10,.015,atmosphere.sun.y);
        let sunlight=.19+.81*pow(forward,4.);
        let band=exp(-abs(ray.y)*13.);
        color+=vec3f(.62,.235,.050)*warmth*sunlight*band;
    }
    color=mix(color, vec3(0.48, 0.54, 0.61), atmosphere.weather.z * 0.72);
    if atmosphere.celestial.w<.5 {return color;}
    let twilight=atmosphere.celestial.y*(1.-atmosphere.weather.z*.63);
    let night=mix(vec3f(.026,.037,.066),vec3f(.004,.008,.027),pow(elevation,.36));
    let horizon_color=mix(vec3f(.19,.18,.24),vec3f(.52,.19,.095),pow(forward,3.));
    return mix(night,color,atmosphere.celestial.x)+twilight*horizon_color*exp(-elevation*7.5)*.68;
}

fn star_field(ray:vec3f)->f32 {
    let night=(1.-smoothstep(-.25,-.12,atmosphere.sun.y))*smoothstep(.02,.22,ray.y);
    if night<.005 {return 0.;}
    let grid=vec2f(atan2(ray.x,ray.z)*98.,asin(clamp(ray.y,-1.,1.))*98.);
    let id=floor(grid);
    let value=hash(id+vec2f(14.,105.));
    let center=vec2f(hash(id),hash(id+57.))*.62+.19;
    let d=length(fract(grid)-center);
    return select(0.,night*(1.-smoothstep(.045,.158,d))*mix(.12,.53,value),value>.979);
}

fn lunar_surface(ray:vec3f)->vec4f {
    let forward=atmosphere.moon.xyz;
    let distance=1./max(atmosphere.moon.w,.0001);
    if forward.y<(-atmosphere.moon.w) || dot(forward,ray)<.99980 {return vec4f(0.);}
    let u=normalize(cross(vec3f(0.,1.,0.),forward));
    let v=cross(forward,u);
    let rotation=transpose(mat3x3f(u,v,forward));
    let dir=rotation*ray;
    let eye=vec3f(0.,0.,-distance);
    let closest=-dot(eye,dir);
    let miss=length(cross(eye,dir));
    if miss>=1. {return vec4f(0.);}
    let entry=closest-sqrt(1.-miss*miss);
    let size=vec3i(textureDimensions(lunar_voxels));
    let p=clamp((eye+entry*dir+1.)*.5*vec3f(size),vec3f(.0001),vec3f(size)-.0001);
    var cell=vec3i(floor(p));
    let step=select(vec3i(-1),vec3i(1),dir>=vec3f(0.));
    let inverse=1./max(abs(dir),vec3f(1.e-8));
    var t=select(fract(p),1.-fract(p),dir>=vec3f(0.))*inverse;
    var face=vec3f(0.,0.,-1.);
    let sunlight=rotation*atmosphere.sun.xyz;
    for (var i=0;i<168;i+=1) {
        if any(cell<vec3i(0)) || any(cell>=size) {break;}
        let albedo=textureLoad(lunar_voxels,cell,0).r;
        if albedo>0. {
            let radial=normalize(vec3f(cell)+.5-vec3f(size)*.5);
            let normal=normalize(face*.62+radial*.38);
            let light=max(dot(normal,sunlight),0.);
            let lambert=max(dot(radial,sunlight),0.);
            let brightness=albedo*(.055+.38*light+.72*lambert);
            let haze=smoothstep(-.015,.17,forward.y);
            return vec4f(vec3f(.91,.95,1.0)*brightness,haze);
        }
        if t.x<=min(t.y,t.z) {
            t.x+=inverse.x;cell.x+=step.x;face=vec3f(-f32(step.x),0.,0.);
        } else if t.y<=t.z {
            t.y+=inverse.y;cell.y+=step.y;face=vec3f(0.,-f32(step.y),0.);
        } else {
            t.z+=inverse.z;cell.z+=step.z;face=vec3f(0.,0.,-f32(step.z));
        }
    }
    return vec4f(0.);
}

fn celestial_sky(ray:vec3f)->vec3f {
    let background=beauty_sky(ray,3.);
    if atmosphere.celestial.w<.5 {return background;}
    let moon=lunar_surface(ray);
    let stars=star_field(ray)*vec3f(.83,.91,1.);
    return mix(background+stars,moon.rgb,moon.a);
}
// Only low-frequency cloud scattering/transmittance is reconstructed here.
// The sky gradient and solar disc remain analytic at native pixel resolution.
fn cloud_light(uv:vec2<f32>)->vec4<f32> {
    if atmosphere.storm_origin.w>1.5 {
        return textureSampleLevel(cloud_lighting,cloud_sampler,uv,0.);
    }
    // A separable cubic B-spline uses four bilinear samples. Its continuous
    // slope suppresses the half-resolution texel grid during subpixel motion.
    let size=vec2f(textureDimensions(cloud_lighting));
    let p=uv*size-.5;
    let base=floor(p)+.5;
    let f=fract(p);
    let ff=f*f;
    let fff=ff*f;
    let a=(1.-f)*(1.-f)*(1.-f);
    let b=4.+3.*fff-6.*ff;
    let c=1.+3.*f+3.*ff-3.*fff;
    let d=fff;
    let lo=a+b;
    let hi=c+d;
    let left=(base-1.+b/lo)/size;
    let right=(base+1.+d/hi)/size;
    let c00=textureSampleLevel(cloud_lighting,cloud_sampler,left,0.);
    let c10=textureSampleLevel(cloud_lighting,cloud_sampler,vec2f(right.x,left.y),0.);
    let c01=textureSampleLevel(cloud_lighting,cloud_sampler,vec2f(left.x,right.y),0.);
    let c11=textureSampleLevel(cloud_lighting,cloud_sampler,right,0.);
    return (c00*lo.x*lo.y+c10*hi.x*lo.y+c01*lo.x*hi.y+c11*hi.x*hi.y)/36.;
}
fn alpine_sky(ray:vec3<f32>,uv:vec2<f32>)->vec3<f32> {
    let clouds=cloud_light(uv);
    return clouds.rgb+celestial_sky(ray)*clouds.a;
}
fn bounded_height_integral(start: f32, end: f32, distance: f32, falloff: f32) -> f32 {
    // Density rho*exp(-k*max(y,0)). Cap LOCAL density at sea-level rho,
    // never optical depth or visibility: very long paths can still become hazy.
    // Split a sea-level crossing analytically; no ray-march or cancellation.
    if falloff < 0.0000001 { return distance; }
    let low = min(start, end);
    let high = max(start, end);
    let span = high - low;
    if span < 0.01 { return distance * exp(-falloff * max((start + end) * 0.5, 0.0)); }
    let below = clamp(-low / span, 0.0, 1.0);
    let lo = max(low, 0.0);
    let hi = max(high, 0.0);
    let delta = falloff * (hi - lo);
    var average = exp(-falloff * lo) * (1.0 - delta * 0.5 + delta * delta / 6.0);
    if delta > 0.01 { average = (exp(-falloff * lo) - exp(-falloff * hi)) / delta; }
    return distance * (below + (1.0 - below) * average);
}
fn beauty_finish(color: vec3<f32>) -> vec3<f32> {
    let exposed = max(color * atmosphere.fog.z, vec3(0.0));
    // Luminance-only shoulder preserves hues and native voxel edge contrast.
    // Midtones stay unchanged; the sun/cloud highlights roll off without clips.
    let luminance = dot(exposed, vec3(0.2126, 0.7152, 0.0722));
    if luminance <= 0.65 { return exposed; }
    let mapped = 0.65 + 0.35 * (1.0 - exp(-(luminance - 0.65) / 0.35));
    return exposed * (mapped / luminance);
}
fn reconstruct(uv: vec2<f32>, depth: f32) -> vec3<f32> {
    let p = atmosphere.inverse_view_projection * vec4(uv.x*2.0-1.0,1.0-uv.y*2.0,depth,1.0);
    return (p.xyz - atmosphere.relative_eye.xyz * p.w) / p.w;
}
fn fog_transmittance(distance: f32, direction_y: f32) -> f32 {
    let falloff = atmosphere.fog.y;
    let start_density = atmosphere.fog.x*exp(clamp(-falloff*atmosphere.world_eye.y,-16.0,16.0));
    let delta = falloff*direction_y*distance;
    // Integral of rho0 exp(-height*k) along the ray. A series avoids
    // cancellation for almost horizontal rays, with the correct zero-k limit.
    var integral = 1.0-delta*0.5+delta*delta/6.0;
    if abs(delta) > 0.01 {
        integral = (1.0-exp(clamp(-delta,-60.0,60.0)))/delta;
    }
    return exp(-min(start_density*distance*integral,80.0));
}
// Bilateral reconstruction operates on scattering only; native scene colour
// and depth never pass through a spatial blur. Reject across terrain/sky edges.
fn meadow_scattering(uv:vec2<f32>,distance:f32)->vec3<f32> {
    if atmosphere.beauty.y<.5 || atmosphere.beauty.z>2.5 {return vec3(0.);}
    let size=vec2<i32>(textureDimensions(meadow_air));
    let p=uv*vec2<f32>(size)-.5;let base=vec2<i32>(floor(p));let f=fract(p);
    var light=vec3(0.);var total=0.;
    for(var y=0i;y<2i;y+=1i) {for(var x=0i;x<2i;x+=1i) {
        let value=textureLoad(meadow_air,clamp(base+vec2(x,y),vec2(0),size-vec2(1)),0);
        let spatial=mix(1.-f.x,f.x,f32(x))*mix(1.-f.y,f.y,f32(y));
        let weight=spatial*exp(-abs(value.a-distance)/max(1.,distance*.025));
        light+=value.rgb*weight;total+=weight;
    }}
    return light/max(total,.00001);
}
// Temporary opacity only at published terrain whose requested refinement is
// still in flight. World-space regions soften across tile boundaries.
fn refinement_transmittance(position:vec3<f32>,distance:f32)->f32 {
    if atmosphere.lighting.y<=0. || atmosphere.lighting.z<0.5 || distance<22. {
        return 1.;
    }
    var amount=0.;
    for(var i=0u;i<u32(atmosphere.lighting.z);i+=1u) {
        let region=atmosphere.haze_regions[i];
        let relative=abs(position.xz-region.xy);
        let edge=max(relative.x,relative.y);
        let inside=1.-smoothstep(region.z*.55,region.z+min(35.,region.z*.6),edge);
        amount=max(amount,inside*region.w);
    }
    let range=smoothstep(22.,65.,distance)*(1.-smoothstep(480.,800.,distance));
    return 1.-clamp(atmosphere.lighting.y*amount*range*.74,0.,.74);
}

fn horizon_extinction(scene_alpha:f32)->f32 {
    return select(0.,clamp(1.-scene_alpha,0.,1.),atmosphere.lighting.w>0.);
}
// Coarse world-space integration of the same condensate rendered in the sky.
fn cloud_shadow_density(p:vec3f) -> f32 {
    var regional = 0.;
    if atmosphere.regional_extent.w > 1.5 {
        let uv = (p-atmosphere.regional_origin.xyz)/atmosphere.regional_extent.xyz;
        if all(uv>=vec3f(0.)) && all(uv<=vec3f(1.)) {
            let previous = textureSampleLevel(previous_shadow_regional,cloud_sampler,uv,0.).r;
            let current = textureSampleLevel(current_shadow_regional,cloud_sampler,uv,0.).r;
            regional = max(0.,mix(previous,current,atmosphere.regional_origin.w)-.045);
        }
    }
    if atmosphere.storm_high.w > .5 {
        let uv = (p-atmosphere.storm_origin.xyz)/atmosphere.storm_extent.xyz;
        if all(uv>=vec3f(0.)) && all(uv<=vec3f(1.)) {
            let previous = textureSampleLevel(previous_shadow_cloud,cloud_sampler,uv,0.).r;
            let current = textureSampleLevel(current_shadow_cloud,cloud_sampler,uv,0.).r;
            let edge = min(min(uv.x,1.-uv.x),min(uv.z,1.-uv.z));
            let vertical = smoothstep(0.,.07,uv.y)*(1.-smoothstep(.91,1.,uv.y));
            return mix(regional,max(0.,mix(previous,current,atmosphere.storm_low.w)-.035),
                       smoothstep(0.,.09,edge)*vertical);
        }
    }
    return regional;
}

fn cloud_ground_light(p:vec3f) -> f32 {
    let sun = atmosphere.sun.xyz;
    if atmosphere.relative_eye.w < .5 || sun.y < .08 {return 1.;}
    var optical_depth = 0.;
    if atmosphere.storm_origin.w>.5 {
        if atmosphere.storm_high.w<.5 && atmosphere.regional_extent.w<1.5 {return 1.;}
        for (var layer=0u;layer<3u;layer+=1u) {
            let altitude = 1050.+f32(layer)*1050.;
            // Fade this coarse layer in smoothly for surfaces crossing it.
            let rise = altitude-p.y;
            if rise>0. {
                let distance = rise/sun.y;
                optical_depth += cloud_shadow_density(p + sun*distance)*.83*smoothstep(0.,220.,rise);
            }
        }
    } else if atmosphere.weather.x>.001 {
        let rise = CLOUD_BASE+.4264706*cloud_thickness()-p.y;
        if rise>0. {
            optical_depth = density(p+sun*(rise/sun.y),vec2f(.82,0.))*1.9*smoothstep(0.,180.,rise);
        }
    }
    let direct = smoothstep(.08,.28,sun.y)*(1.-.65*atmosphere.weather.z);
    return 1.-direct*.55*(1.-exp(-min(4.,optical_depth/max(.30,sun.y))));
}

fn composite(sample_color: vec4f, depth: f32, uv: vec2<f32>) -> vec3<f32> {
    let color=sample_color.rgb;
    if atmosphere.fog.w < 0.5 {
        if atmosphere.beauty.x>.5 {return beauty_finish(color);}
        return color*atmosphere.fog.z;
    }
    let offset = reconstruct(uv,depth);
    let distance = length(offset);
    let ray = offset/max(distance,0.0001);
    if atmosphere.beauty.x > 0.5 {
        // Air/visibility diagnostics contain this pass only: no scene colour,
        // sky, exposure, tone mapping, or analytical global height-fog terms.
        if atmosphere.beauty.z>.5 && atmosphere.beauty.z<2.5 {
            return meadow_scattering(uv,select(distance,4000.,depth>=1.));
        }
        if depth >= 1.0 { return beauty_finish(alpine_sky(ray,uv)+meadow_scattering(uv,4000.)); }
        let end_height = atmosphere.world_eye.y + offset.y;
        let integrated = bounded_height_integral(atmosphere.world_eye.y, end_height, distance, atmosphere.fog.y);
        // Clean air includes a weak altitude-independent Rayleigh contribution.
        // The user density scales BOTH terms, so zero fog remains exactly clear.
        let optical_depth = vec3f(atmosphere.fog.x * (integrated * 0.78 + distance * 0.22));
        var path_extinction=optical_depth;
        if atmosphere.sun.w>.5 {
            // Nearly neutral aerosol follows the height layer. The weaker
            // molecular term tints long paths without an extra fog cutoff.
            let aerosol=integrated*.77;
            let molecular=distance*vec3f(.126,.224,.382);
            path_extinction=atmosphere.fog.x*(vec3f(aerosol)+molecular);
        }
        let transmittance = exp(-path_extinction)*refinement_transmittance(offset,distance);
        let haze = beauty_sky(ray,0.);
        let darkness=vec3f(.045,.072,.125)*(1.+atmosphere.celestial.z*1.22)+atmosphere.celestial.y*.13;
        let irradiance=mix(darkness,vec3f(1.),atmosphere.celestial.x);
        let cloud_light_at_surface = cloud_ground_light(atmosphere.world_eye.xyz + offset);
        let terrain = mix(haze, color*irradiance*cloud_light_at_surface, transmittance)
            + meadow_scattering(uv,distance);
        let horizon = horizon_extinction(sample_color.a);
        if atmosphere.storm_origin.w>.5 {
            let base=mix(terrain,celestial_sky(ray)+meadow_scattering(uv,4000.),horizon);
            let cloud=cloud_light(uv);
            return beauty_finish(cloud.rgb+base*cloud.a);
        }
        if horizon > 0. {
            return beauty_finish(mix(terrain,alpine_sky(ray,uv)+meadow_scattering(uv,4000.),horizon));
        }
        return beauty_finish(terrain);
    }
    if depth >= 1.0 { return sky(ray)*atmosphere.fog.z; }
    let transmittance = fog_transmittance(distance,ray.y);
    // Aerial perspective uses the distant sky's scattered-light hue.
    let terrain=mix(clear_sky(ray),color,transmittance);
    let horizon=horizon_extinction(sample_color.a);
    if horizon>0. { return mix(terrain,sky(ray),horizon)*atmosphere.fog.z; }
    return terrain*atmosphere.fog.z;
}
@fragment fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let pixel = vec2<i32>(in.position.xy);
    // SAMPLE_COMPOSITE
}
