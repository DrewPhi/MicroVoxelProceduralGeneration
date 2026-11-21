override traversal_diagnostic:bool = false;
override traversal_beam:bool = false;
@group(2) @binding(0) var beam_prefix:texture_2d<u32>;
@group(1) @binding(0) var far_depth:texture_depth_2d;
@group(1) @binding(1) var far_sampler:sampler_comparison;
@vertex fn vs_main(@builtin(vertex_index) i:u32)->@builtin(position) vec4<f32>{let p=vec2(f32((i<<1u)&2u),f32(i&2u));return vec4(p*2.-1.,0.,1.);}
struct Fragment {@location(0) color:vec4<f32>,@builtin(frag_depth) depth:f32};
fn distant_sun_visibility(hit:vec3<f32>,sun:vec3<f32>)->f32 {
    if naadf.shadow.x<0.5{return 1.;}
    // Compare at the ray's exit, so shadow geometry inside the NAADF box
    // cannot substitute for, or double-shadow, our exact voxel query.
    let interval=voxel_interval(hit*VOXELS_PER_METER,sun);
    let receiver=hit+sun*max(0.,interval.y*NATIVE_VOXEL_METERS+0.002)-naadf.eye.xyz;
    let clip=naadf.far_matrix*vec4(receiver,1.);let q=clip.xyz/clip.w;
    let uv=q.xy*vec2(0.5,-0.5)+0.5;
    if any(uv<vec2(0.002))||any(uv>vec2(0.998))||q.z<=0.||q.z>=1.{return 1.;}
    return textureSampleCompareLevel(far_depth,far_sampler,uv,q.z-0.0002);
}
fn light(normal:vec3<f32>,visibility:f32)->vec3<f32>{
    if naadf.sky.z>1.5 || (naadf.sky.z<0.5 && naadf.sun.w>0.5) {
        return regional_skylight(normal,naadf.sun.xyz,naadf.sky.x,visibility,1.,naadf.sun.w>0.5);
    }
    let sun=normalize(vec3(-0.5,0.8,-0.3));
    let sky=0.5+0.5*normal.y;
    return mix(vec3(0.12,0.105,0.085),vec3(0.28,0.33,0.40),sky)+vec3(0.78,0.70,0.58)*max(dot(normal,sun),0.);
}
fn bounce_at(position:vec3<f32>,normal:vec3<f32>,visibility:f32)->vec3<f32> {
    if naadf.sky.z<1.5 && naadf.sun.w<0.5 {return vec3(0.);}
    return ground_irradiance(position-naadf.eye.xyz,normal,naadf.sun.xyz,
        outdoor_sun_color(naadf.sun.y),naadf.sky.x,visibility);
}
// Terrain and player-built cells use the raster terrain's world-anchored
// paint. Plant-specific pigments already carry their own native-cell color.
fn voxel_albedo(grid_hit:vec3<f32>,h:VoxelHit)->vec3<f32> {
    let base=palette[h.pigment].rgb;
    if naadf.sky.z<0.5 || h.pigment>=10u { return base; }
    let local=vec3<i32>(floor(grid_hit-h.normal*.01));
    let cell=bitcast<vec3<u32>>(local)+naadf.paint_origin.xyz;
    let distance=length((vec3<f32>(local)+.5)*NATIVE_VOXEL_METERS-naadf.eye.xyz);
    return base*display_terrain_tint(cell,1.-smoothstep(12.,40.,distance),bitcast<f32>(naadf.paint_origin.w));
}
fn shade_voxel(hit:vec3<f32>,h:VoxelHit)->vec3<f32> {
    var visibility=1.;
    if (naadf.sky.z>1.5 || naadf.sun.w>0.5) && naadf.shadow.x>0.5 && dot(h.normal,naadf.sun.xyz)>0. {
        if naadf.shadow.y<0.5{visibility=naadf_sun_visibility(hit-naadf.eye.xyz,h.normal,naadf.sun.xyz);}
        if visibility>0.5{visibility*=distant_sun_visibility(hit,naadf.sun.xyz);}
    }
    let color=voxel_albedo(hit*VOXELS_PER_METER,h);
    if ground_light.origin_strength.z<=0. || (naadf.sky.z<1.5 && naadf.sun.w<0.5) {return color*light(h.normal,visibility);}
    let contact=voxel_contact(hit,h.normal);
    // Contact occlusion affects indirect light; the direct sun uses visibility.
    let indirect=light(h.normal,0.)+bounce_at(hit,h.normal,visibility);
    return color*(light(h.normal,visibility)-light(h.normal,0.)+indirect*contact);
}

// Exact occupancy lookup in the published native snapshot. Skip proofs encode
// traversal distances, so only the solid bit contributes to the slope fit.
fn footprint_word(cell:vec3<u32>)->u32 {
    let root=cell/16u;
    var word=0u;
    if naadf_sparse() {
        let side=(naadf.extent.w+3u)/4u;
        let page=root/4u;
        let shape=textureDimensions(chunks,0);
        word=sparse_root_load(page.y+side*(page.x+side*page.z),shape);
        if (word&0x80000000u)!=0u {
            let local=root%4u;
            word=sparse_root_load(side*side*side+(word&0x3fffffffu)+local.y+4u*(local.x+4u*local.z),shape);
        }
    } else {
        word=textureLoad(chunks,dense_root_slot(root),0).x;
    }
    // A repaired root proves air. Point occupancy never consumes skip distances.
    if root_repaired(word) {return 0u;}
    if (word&0x80000000u)!=0u {
        let block=(cell%16u)/4u;
        word=blocks[(word&0x3fffffffu)+block.x+4u*(block.y+4u*block.z)];
        if (word&0x80000000u)!=0u {
            let voxel=cell%4u;
            word=naadf_leaf_word((word&0x3fffffffu)+voxel.x+4u*(voxel.y+4u*voxel.z));
        }
    }
    return word;
}

fn contact_solid(cell:vec3<i32>)->f32 {
    if any(cell<vec3(0)) || any(cell>=vec3<i32>(naadf.extent.xyz)) {return 0.;}
    let word=footprint_word(vec3<u32>(cell));
    return select(0.,1.,(word&0x40000000u)!=0u && !water_node(word));
}

fn voxel_contact(hit:vec3<f32>,normal:vec3<f32>)->f32 {
    if naadf.sun.w<0.5 || ground_light.origin_strength.z<=0. {return 1.;}
    let near=1.-smoothstep(2.5,9.,length(hit-naadf.eye.xyz));
    if near<=0. {return 1.;}
    let grid=hit*VOXELS_PER_METER;
    let cell=vec3<i32>(floor(grid+normal*.014));
    let a=select(vec3(1.,0.,0.),vec3(0.,1.,0.),abs(normal.x)>0.5);
    let b=abs(cross(normal,a));
    let edge=vec2(dot(fract(grid),a),dot(fract(grid),b));
    let side=select(vec2(-1.),vec2(1.),edge>vec2(0.5));
    let weight=1.-smoothstep(vec2(0.02),vec2(0.44),min(edge,1.-edge));
    let ao_a=weight.x*contact_solid(cell+vec3<i32>(a*side.x));
    let ao_b=weight.y*contact_solid(cell+vec3<i32>(b*side.y));
    return 1.-near*(0.19*(ao_a+ao_b));
}

fn water_phase_position(p:vec3<f32>)->vec3<f32> {
    return p+vec3(naadf.water.x,0.,naadf.water.y);
}

fn connected_stream_step(surface:vec3<f32>,face:vec3<f32>)->bool {
    let inside=vec3<i32>(floor(surface*VOXELS_PER_METER-face*.014));
    let lower=inside+vec3<i32>(face)-vec3(0,1,0);
    let upper=inside+vec3(0,1,0);
    let extent=vec3<i32>(naadf.extent.xyz);
    if any(lower<vec3(0)) || any(lower>=extent) || any(upper<vec3(0)) || any(upper>=extent) {return false;}
    let wet=water_state(lower);
    if wet!=2u && (wet==1u || !water_node(footprint_word(vec3<u32>(lower)))) {return false;}
    let above=water_state(upper);
    let word=footprint_word(vec3<u32>(upper));
    return above!=2u && ((word&0x40000000u)==0u || (above==1u && water_node(word)));
}

// One bounded bed ray and one bounded shore ray, only on visible water.
fn shade_water(surface:vec3<f32>,normal:vec3<f32>,d:vec3<f32>,distance:f32)->vec3<f32> {
    let pixel_height=2.*distance*naadf.up.w/naadf.viewport.y;
    var optical_face=normal;
    let course=stream_current(surface.xz-naadf.eye.xz);
    let step=normal.y==0. && (pixel_height*3.3>NATIVE_VOXEL_METERS || course.z>.50)
        && connected_stream_step(surface,normal);
    if step && pixel_height*3.3>NATIVE_VOXEL_METERS {
        optical_face=stream_step_normal(normal,NATIVE_VOXEL_METERS,pixel_height);
    }
    let phase_position=water_phase_position(surface);
    let pixel=2.*distance*naadf.up.w/(naadf.viewport.y*max(.06,abs(dot(d,normal))));
    let ripple=water_surface_normal(phase_position.xz,optical_face,naadf.water.z,pixel,distance,course);
    let n=select(normal,ripple,dot(d,ripple)<-0.002);
    let transmitted=refract(d,n,1./1.333);
    let origin=surface*VOXELS_PER_METER-normal*.012;
    let limit=8.*VOXELS_PER_METER;
    var bed=trace_water_optics(origin,transmitted,limit);
    var bed_direction=transmitted;
    var path=bed.t*NATIVE_VOXEL_METERS;
    if !bed.found && normal.y>.5 {
        let leave=voxel_interval(origin,transmitted).y;
        if leave>0. && leave<limit && origin.y+leave*transmitted.y>1. {
            // A shallow refracted ray can leave the exact box sideways before it
            // reaches the bed. Measure the water column underneath this roof.
            bed_direction=vec3(0.,-1.,0.);
            bed=trace_water_optics(origin,bed_direction,limit);
            path=bed.t*NATIVE_VOXEL_METERS/max(.05,-transmitted.y);
        }
    }
    var below=water_medium(vec3(0.03,0.10,0.08),8.);
    if bed.found {
        let bed_grid=origin+bed_direction*bed.t;
        let bed_position=bed_grid*NATIVE_VOXEL_METERS;
        let p=water_phase_position(bed_position);
        let resolve=1.-0.70*smoothstep(3.,18.,distance);
        let bed_light=mix(light(vec3(0.,1.,0.),0.48),light(bed.normal,0.48),resolve);
        let color=voxel_albedo(bed_grid,bed)*(bed_light+bounce_at(bed_position,bed.normal,0.48));
        let phase=sin(dot(p.xz,vec2(43.,-35.)*0.049087385)-naadf.water.z)*1.9;
        let caustic=sin(dot(p.xz,vec2(183.,82.)*0.049087385)+naadf.water.z*1.5+phase);
        below=water_medium(color*(1.+0.065*caustic),path);
    } else {
        let leave=voxel_interval(origin,transmitted).y;
        if leave>0. && leave<limit {
            let survey=surveyed_water_bed(surface-naadf.eye.xyz);
            if survey.a>0. {
                let up=vec3(0.,1.,0.);
                let bed_light=light(up,.48)+bounce_at(surface,up,.48);
                let path=max(leave*NATIVE_VOXEL_METERS,survey.a/max(.05,-transmitted.y));
                below=water_medium(survey.rgb*bed_light,path);
            }
        }
    }

    let mirror=reflect(d,n);
    var above=water_sky(mirror,naadf.sun.xyz,naadf.sky.x);
    if optical_face.y>.5 && naadf.sun.w>.5 {
        above=reflected_terrain(surface-naadf.eye.xyz,mirror,above);
    }
    let reflection_weight=1.-smoothstep(28.,56.,distance);
    if reflection_weight>0. && dot(mirror,normal)>0. {
        let bank=trace_water_optics(surface*VOXELS_PER_METER+normal*0.012,mirror,24.*VOXELS_PER_METER);
        if bank.found {
            let color=voxel_albedo(surface*VOXELS_PER_METER+normal*.012+mirror*bank.t,bank)*(light(bank.normal,0.82)+bounce_at(surface+mirror*bank.t*NATIVE_VOXEL_METERS,bank.normal,0.82));
            above=mix(above,color,reflection_weight*(1.-smoothstep(16.,24.,bank.t*NATIVE_VOXEL_METERS)));
        }
    }
    let fresnel=water_fresnel(-dot(d,n));
    let color=mix(below,above,fresnel)+water_glint(d,n,naadf.sun.xyz,naadf.sky.x)*min(1.,3.*fresnel);
    let world_height=surface.y+f32(bitcast<i32>(naadf.paint_origin.y))*NATIVE_VOXEL_METERS;
    if (normal.y>.5 || step) && world_height>0. && naadf.sun.w>.5 && course.z>.50 {
        let amount=rapid_foam(phase_position.xz,naadf.water.z,pixel_height/max(.18,abs(d.y)),
            course.w*smoothstep(.50,.98,course.z),course.xy);
        if amount>.001 {
            let up=vec3(0.,1.,0.);
            var visibility=distant_sun_visibility(surface,naadf.sun.xyz);
            if visibility>.01 && naadf.shadow.x>.5 && naadf.shadow.y<.5 {
                visibility*=naadf_sun_visibility(surface-naadf.eye.xyz,up,naadf.sun.xyz);
            }
            return foam_color(color,light(up,visibility),amount);
        }
    }
    return color;
}

// Follow the connected water immediately above the eye. The search is limited
// to the clear part of the water column. Solid banks terminate the interface.
fn submerged_exit(o:vec3<f32>,d:vec3<f32>,maximum:f32)->vec4<f32> {
    let extent=vec3<i32>(naadf.extent.xyz);
    var cell=forward_cell(o,d);
    let step=select(vec3(-1),vec3(1),d>vec3(0.));
    let increment=select(vec3(1e30),abs(1./d),d!=vec3(0.));
    var next=select(vec3(1e30),(vec3<f32>(cell+max(step,vec3(0)))-o)/d,d!=vec3(0.));
    var t=0.;
    var normal=vec3(0.);
    for(var i=0u;i<160u;i++) {
        if t>=maximum || any(cell<vec3(0)) || any(cell>=extent) {break;}
        let state=water_state(cell);
        let word=footprint_word(vec3<u32>(cell));
        if state!=2u && !(state!=1u && water_node(word)) {
            if (word&0x40000000u)==0u || water_node(word) {
                return vec4(normal,t);
            }
            break;
        }
        t=min(next.x,min(next.y,next.z));
        let crossed=next==vec3(t);
        cell+=select(vec3(0),step,crossed);
        next+=select(vec3(0.),increment,crossed);
        normal=vec3<f32>(select(vec3(0),step,crossed));
    }
    return vec4(0.,0.,0.,-1.);
}

fn shade_submerged(o:vec3<f32>,d:vec3<f32>,cosine:f32)->Fragment {
    let limit=192.;
    let eye=o*NATIVE_VOXEL_METERS;
    let ripple=water_ripple(water_phase_position(eye),vec3(0.,1.,0.),naadf.water.z,0.009);
    let ray=normalize(d+vec3(ripple.x,0.,ripple.z));
    let bed=trace_water_optics(o,ray,limit);
    var travel=select(limit,bed.t,bed.found);
    var color=vec3(0.020,0.095,0.085);
    if bed.found {
        color=voxel_albedo(o+ray*bed.t,bed)*(light(bed.normal,0.55)+bounce_at((o+ray*bed.t)*NATIVE_VOXEL_METERS,bed.normal,0.55));
    }
    if ray.y>0.02 {
        let boundary=submerged_exit(o,ray,travel);
        if boundary.w>0. {
            travel=boundary.w;
            let point=(o+ray*travel)*NATIVE_VOXEL_METERS;
            let outward=normalize(boundary.xyz);
            let n=water_ripple(water_phase_position(point),outward,naadf.water.z,0.025);
            let outgoing=refract(ray,-n,1.333);
            if dot(outgoing,outgoing)>0.1 {
                color=water_sky(outgoing,naadf.sun.xyz,naadf.sky.x);
                let shore=trace_water_optics(point*VOXELS_PER_METER+outward*0.012,outgoing,256.);
                if shore.found {
                    color=voxel_albedo(point*VOXELS_PER_METER+outward*.012+outgoing*shore.t,shore)*light(shore.normal,0.85);
                }
                color=mix(color,vec3(0.031,0.13,0.12),water_fresnel(dot(outgoing,n)));
            } else {
                let reflected=reflect(ray,-n);
                let floor_hit=trace_water_optics(point*VOXELS_PER_METER-outward*0.012,reflected,128.);
                color=vec3(0.031,0.13,0.12);
                if floor_hit.found {
                    color=water_medium(voxel_albedo(point*VOXELS_PER_METER-outward*.012+reflected*floor_hit.t,floor_hit)*light(floor_hit.normal,0.6),floor_hit.t*NATIVE_VOXEL_METERS);
                }
            }
        }
    }
    let depth=naadf.projection.z+naadf.projection.w/max(naadf.projection.x,travel*NATIVE_VOXEL_METERS*cosine);
    return Fragment(vec4(water_medium(color,travel*NATIVE_VOXEL_METERS),1.),clamp(depth,0.,1.));
}

@fragment fn fs_main(@builtin(position) pixel:vec4<f32>,@builtin(sample_index) sample:u32)->Fragment {
    let offsets=array<vec2<f32>,4>(vec2(0.375,0.125),vec2(0.875,0.375),vec2(0.125,0.625),vec2(0.625,0.875));
    var point=pixel.xy;if naadf.viewport.z>1.5{point=floor(point)+offsets[min(sample,3u)];}
    let ndc=vec2(point.x/naadf.viewport.x*2.-1.,1.-point.y/naadf.viewport.y*2.);
    // Work in camera direction space: subtracting the eye after inverse-VP
    // unprojection loses angular precision as the snapshot origin moves away.
    let o=naadf.eye.xyz*VOXELS_PER_METER;
    let d=normalize(naadf.forward.xyz+ndc.x*naadf.right.xyz*naadf.right.w+ndc.y*naadf.up.xyz*naadf.up.w);
    let cosine=dot(d,naadf.forward.xyz);
    let minimum=naadf.projection.x/cosine*VOXELS_PER_METER;
    if !traversal_dry && !traversal_static_water && naadf.eye.w>0.5 && naadf.viewport.w<0.5 && naadf.sky.w<0.5 && !traversal_diagnostic {
        return shade_submerged(o,d,cosine);
    }
    var prefix=minimum;
    if traversal_beam {
        let steps=textureLoad(beam_prefix,vec2<i32>(vec2<u32>(point)/8u),0).x;
        prefix=max(minimum,f32(steps)/cosine);
        if traversal_metrics {
            let interval=voxel_interval(o,d);
            let ordinary=max(minimum,max(0.,interval.x));
            trace_metric(2u,10u,1u);
            if prefix>ordinary && ordinary<interval.y {
                trace_metric(2u,9u,1u);
                if prefix>=interval.y {trace_metric(2u,11u,1u);}
                trace_distance(2u,17u,max(0.,min(prefix,interval.y)-ordinary));
            }
        }
    }
    let h=trace_prefixed(o,d,minimum,prefix,false,traversal_diagnostic);
    if traversal_diagnostic {
        return Fragment(vec4(vec3<f32>(h.visits),1.),1.);
    }
    if !h.found{discard;}
    let hit=(o+d*h.t)*NATIVE_VOXEL_METERS;
    // Storage has a native-cell apron; raster owns the outermost layer.
    let center=(floor(hit*VOXELS_PER_METER-h.normal*0.01)+0.5)*NATIVE_VOXEL_METERS;
    if naadf.viewport.w<0.5 && (any(center<vec3(NATIVE_VOXEL_METERS))||any(center>=vec3<f32>(naadf.extent.xyz)*NATIVE_VOXEL_METERS-vec3(NATIVE_VOXEL_METERS))){discard;}
    let depth=naadf.projection.z+naadf.projection.w/(h.t*NATIVE_VOXEL_METERS*cosine);
    if depth<0.||depth>1.{discard;}
    let pigment=palette[h.pigment];
    if naadf.viewport.w>1.5 {
        let blocked=trace_voxels((hit+h.normal*0.001)*VOXELS_PER_METER,normalize(naadf.sun.xyz),0.,true).found;
        return Fragment(vec4(select(0.,1.,blocked),0.,0.,1.),depth);
    }
    if naadf.viewport.w>0.5{return Fragment(vec4(f32(pigment.material)/255.,0.,0.,1.),depth);}
    if naadf.sky.w>0.5{return Fragment(vec4(vec3(0.8)*(0.3+0.7*max(dot(h.normal,normalize(vec3(-0.5,0.8,-0.3))),0.)),1.),depth);}
    if !traversal_dry && pigment.material==7u {
        return Fragment(vec4(shade_water(hit,h.normal,d,h.t*NATIVE_VOXEL_METERS),1.),depth);
    }
    return Fragment(vec4(shade_voxel(hit,h),1.),depth);
}
