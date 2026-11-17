override native_sand_footprint:bool=true;
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
    let interval=voxel_interval(hit*16.,sun);
    let receiver=hit+sun*max(0.,interval.y*0.0625+0.002)-naadf.eye.xyz;
    let clip=naadf.far_matrix*vec4(receiver,1.);let q=clip.xyz/clip.w;
    let uv=q.xy*vec2(0.5,-0.5)+0.5;
    if any(uv<vec2(0.002))||any(uv>vec2(0.998))||q.z<=0.||q.z>=1.{return 1.;}
    return textureSampleCompareLevel(far_depth,far_sampler,uv,q.z-0.0002);
}
fn light(normal:vec3<f32>,visibility:f32)->vec3<f32>{
    var sun=normalize(vec3(-0.5,0.8,-0.3));if naadf.sun.w>0.5{sun=naadf.sun.xyz;}
    let sky=0.5+0.5*normal.y;
    let ambient=mix(vec3(0.12,0.145,0.075),vec3(0.25,0.33,0.43),sky)*(1.+naadf.sky.x*0.3);
    let direct=vec3(2.,1.7,1.2)*max(dot(normal,sun),0.)*(1.-0.72*naadf.sky.x);
    if naadf.sun.w>0.5{return ambient+direct*visibility;}
    return mix(vec3(0.12,0.105,0.085),vec3(0.28,0.33,0.40),sky)+vec3(0.78,0.70,0.58)*max(dot(normal,sun),0.);
}
fn pixel_ray(point:vec2<f32>)->vec3<f32> {
    let ndc=point/naadf.viewport.xy*vec2(2.,-2.)+vec2(-1.,1.);
    return normalize(naadf.forward.xyz+ndc.x*naadf.right.xyz*naadf.right.w+ndc.y*naadf.up.xyz*naadf.up.w);
}
// Visit each point in a 4x4 stratified grid once per history cycle.
fn temporal_sample_offset(phase:u32)->vec2<f32> {
    let x=phase&3u;
    let y=((phase>>2u)+x*3u)&3u;
    return (vec2<f32>(f32(x),f32(y))+0.5)*0.25-0.5;
}
fn shade_voxel(hit:vec3<f32>,h:VoxelHit)->vec3<f32> {
    var visibility=1.;
    if naadf.sun.w>0.5 && dot(h.normal,naadf.sun.xyz)>0. {
        if naadf.shadow.y<0.5{visibility=naadf_sun_visibility(hit-naadf.eye.xyz,h.normal,naadf.sun.xyz);}
        if visibility>0.5{visibility*=distant_sun_visibility(hit,naadf.sun.xyz);}
    }
    return palette[h.pigment].rgb*light(h.normal,visibility);
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
        let slot=(root+naadf.ring.xyz)%vec3(naadf.extent.w);
        word=textureLoad(chunks,vec3<i32>(slot.yxz),0).x;
    }
    word=root_decode(word,root);
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

fn sand_top(cell:vec3<i32>)->i32 {
    var lo=cell.y-8;
    var hi=cell.y+8;
    let base=footprint_word(vec3<u32>(vec3(cell.x,lo,cell.z)));
    if (base&0x40000000u)==0u || palette[base&0x3fffffffu].material!=8u
        || (footprint_word(vec3<u32>(vec3(cell.x,hi,cell.z)))&0x40000000u)!=0u {
        return -1;
    }
    // The slope estimate assumes one local sand surface. Reject water and
    // other materials encountered inside the bracket.
    for(var i=0u;i<4u;i++) {
        let mid=(lo+hi)/2;
        let word=footprint_word(vec3<u32>(vec3(cell.x,mid,cell.z)));
        if (word&0x40000000u)!=0u {
            if palette[word&0x3fffffffu].material!=8u {return -1;}
            lo=mid;
        } else {
            hi=mid;
        }
    }
    return hi;
}

fn sand_footprint(cell:vec3<i32>,d:vec3<f32>,h:VoxelHit,cosine:f32)->vec4<f32> {
    // Cheap guard for pixels whose projected reach is likely under a cell.
    let pixel_angle=2.*naadf.up.w/naadf.viewport.y;
    if h.t*pixel_angle<max(abs(d.y)*0.45,0.09) {return vec4(0.);}
    let reach=vec3(8);
    if any(cell<reach) || any(cell+reach>=vec3<i32>(naadf.extent.xyz)) {return vec4(0.);}
    let left=sand_top(cell-vec3(8,0,0));
    let right=sand_top(cell+vec3(8,0,0));
    let back=sand_top(cell-vec3(0,0,8));
    let front=sand_top(cell+vec3(0,0,8));
    if min(min(left,right),min(back,front))<0 {return vec4(0.);}
    let slope=vec2<f32>(f32(right-left),f32(front-back))*0.0625;
    let surface=vec3(-slope.x,1.,-slope.y);
    let incidence=dot(surface,d);
    // In the nearly tangent limit, cap the differential to a finite reach.
    let inverse_incidence=1./min(incidence,-0.025);
    let horizontal=naadf.right.xyz*(2.*naadf.right.w/naadf.viewport.x);
    let vertical=naadf.up.xyz*pixel_angle;
    let scale=h.t*cosine;
    let dx=(horizontal-d*dot(horizontal,surface)*inverse_incidence)*scale;
    let dy=(vertical-d*dot(vertical,surface)*inverse_incidence)*scale;
    // A footprint can also span the little riser segments running along
    // one terrace, even if it does not cross several whole terraces.
    let crossings=abs(dot(dx.xz,slope))+abs(dot(dy.xz,slope));
    let column_span=max(length(dx.xz),length(dy.xz));
    let blend=smoothstep(0.5,1.75,max(crossings,column_span));
    if blend<=0. {return vec4(0.);}

    let nx=vec3(select(1.,-1.,slope.x>0.),0.,0.);
    let nz=vec3(0.,0.,select(1.,-1.,slope.y>0.));
    let top=vec3(0.,1.,0.);
    let weights=vec3(max(-d.y,0.),abs(slope.x)*max(-dot(nx,d),0.),abs(slope.y)*max(-dot(nz,d),0.));
    let coverage=dot(weights,vec3(1.));
    if coverage<=0.0001 {return vec4(0.);}

    // Use one representative shadow receiver on the actual sand surface.
    // Preserve exact shadows at close range through the blend below.
    var visibility=1.;
    if naadf.sun.w>0.5 && naadf.shadow.x>0.5 {
        let height=sand_top(cell);
        if height<0 {return vec4(0.);}
        let receiver=vec3(f32(cell.x)+0.5,f32(height),f32(cell.z)+0.5)*0.0625;
        if naadf.shadow.y<0.5 {
            visibility=naadf_sun_visibility(receiver-naadf.eye.xyz,top,naadf.sun.xyz);
        }
        if visibility>0.5 {visibility*=distant_sun_visibility(receiver,naadf.sun.xyz);}
    }
    let lighting=(weights.x*light(top,visibility)+weights.y*light(nx,visibility)+weights.z*light(nz,visibility))/coverage;
    return vec4(palette[h.pigment].rgb*lighting,blend);
}

@fragment fn fs_main(@builtin(position) pixel:vec4<f32>,@builtin(sample_index) sample:u32)->Fragment {
    let offsets=array<vec2<f32>,4>(vec2(0.375,0.125),vec2(0.875,0.375),vec2(0.125,0.625),vec2(0.625,0.875));
    let temporal=naadf.sky.y>0. && naadf.sky.z>=1. && naadf.viewport.w<0.5 && naadf.sky.w<0.5 && !traversal_diagnostic;
    var point=pixel.xy;
    if naadf.viewport.z>1.5 {
        point=floor(point)+offsets[min(sample,3u)];
    } else if temporal {
        point+=temporal_sample_offset(u32(naadf.sky.z)-1u)*naadf.sky.y;
    }
    // Work in camera direction space: subtracting the eye after inverse-VP
    // unprojection loses angular precision as the snapshot origin moves away.
    let o=naadf.eye.xyz*16.;
    let d=pixel_ray(point);
    let cosine=dot(d,naadf.forward.xyz);
    let minimum=naadf.projection.x/cosine*16.;
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
        return Fragment(vec4(vec3<f32>(h.visits),1.),0.);
    }
    if !h.found{discard;}
    let hit=(o+d*h.t)*0.0625;
    // Storage has a native-cell apron; raster owns the outermost layer.
    let center=(floor(hit*16.-h.normal*0.01)+0.5)*0.0625;
    if naadf.viewport.w<0.5 && (any(center<vec3(0.0625))||any(center>=vec3<f32>(naadf.extent.xyz)*0.0625-vec3(0.0625))){discard;}
    let depth=naadf.projection.z+naadf.projection.w/(h.t*0.0625*cosine);
    if depth<0.||depth>1.{discard;}
    let pigment=palette[h.pigment];
    if naadf.viewport.w>1.5 {
        let blocked=trace_voxels((hit+h.normal*0.001)*16.,normalize(naadf.sun.xyz),0.,true).found;
        return Fragment(vec4(select(0.,1.,blocked),0.,0.,1.),depth);
    }
    if naadf.viewport.w>0.5{return Fragment(vec4(f32(pigment.material)/255.,0.,0.,1.),depth);}
    if naadf.sky.w>0.5{return Fragment(vec4(vec3(0.8)*(0.3+0.7*max(dot(h.normal,normalize(vec3(-0.5,0.8,-0.3))),0.)),1.),depth);}
    var history_alpha=select(1.,2.25,temporal);
    if temporal && pigment.material==8u {
        // At grazing angles, a pixel can span several treads or risers.
        // Encode the footprint so the resolve can filter only nearby sand.
        let footprint=h.t*(2.*naadf.up.w/naadf.viewport.y)/max(abs(d.y),0.06);
        history_alpha+=smoothstep(0.5,1.75,footprint);
    }
    if native_sand_footprint && temporal && pigment.material==8u {
        let filtered=sand_footprint(vec3<i32>(floor(center*16.)),d,h,cosine);
        if filtered.a>=1. {return Fragment(vec4(filtered.rgb,2.25),depth);}
        if filtered.a>0. {
            return Fragment(vec4(mix(shade_voxel(hit,h),filtered.rgb,filtered.a),mix(history_alpha,2.25,filtered.a)),depth);
        }
    }
    return Fragment(vec4(shade_voxel(hit,h),history_alpha),depth);
}
