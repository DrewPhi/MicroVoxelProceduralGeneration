// Playable composition of the experimental rolling heightfield and exact columns.
@group(0) @binding(10) var native_maxima: texture_2d<f32>;
@group(0) @binding(12) var<storage, read> native_runs: array<vec4i>;
@group(0) @binding(13) var outer_pigment: texture_2d<f32>;
@group(0) @binding(14) var inner_pigment: texture_2d<f32>;
override CANONICAL_PIGMENT: bool = false;

fn exact_top_hit(top: f32, slope: f32, start: f32, end: f32, face: f32) -> vec2f {
    // Compare the crossing time with the half-open horizontal interval.
    // Rounding the remote endpoint's height can incorrectly include a roof
    // that the ray crosses just after leaving this column.
    let offset = (top - native_view.voxel.z) - native_view.sky.y;
    if slope < 0. {
        let crossing = offset / slope;
        if crossing < end {
            return select(vec2f(crossing,0.),vec2f(start,face),crossing < start);
        }
    } else if (slope > 0. && start < offset/slope) || (slope == 0. && offset > 0.) {
        return vec2f(start,face);
    }
    return vec2f(-1.,0.);
}

fn native_hit(direction: vec3f, ray: Ray) -> vec3f {
    if ray.lo >= ray.hi { return vec3f(-1.); }
    var fine = initial_cell(ray);
    var t = ray.lo;
    var face = 0.;
    var mip = ray.grid.root;
    for (var step = 0u; step < u32(ray.grid.extent.w) * 8u + 64u; step += 1u) {
        let edge = cell_edge(fine, mip);
        let exit = exit_times(ray, edge);
        let end = max(t, min(ray.hi, min(exit.x, exit.y)));
        let cell = lookup_cell(ray, fine, mip);
        let y0 = fma(t, direction.y, native_view.voxel.z);
        let y1 = fma(end, direction.y, native_view.voxel.z);
        let top = textureLoad(native_maxima, cell, i32(mip)).r;
        if end > t && min(y0, y1) <= top + max(.00001,abs(top)*.00000012) {
            if mip > 0u {
                mip -= 1u;
                continue;
            }
            let column = textureLoad(native_columns, cell, 0);
            if column.w == 0 {
                let h = exact_top_hit(top, direction.y, t, end, face);
                if h.x >= 0. {
                    // Pick the solid cell on the face's interior, including
                    // the uppermost block on a horizontal top face.
                    let y = fma(h.x,direction.y,native_view.voxel.z) + native_view.sky.y;
                    let layer = select(column.x - i32(floor(y * native_view.voxel.y)), 1, h.y == 0.);
                    let material = select(select(3, (column.y >> 8) & 255, layer <= 12), column.y & 255, layer <= 1);
                    let wet = column.z > column.x && (h.y == 0. || y >= f32(column.x)*native_view.voxel.x);
                    return vec3f(h, f32(select(material,7,wet)));
                }
            } else {
                var nearest = vec3f(end, 0., 0.);
                for (var i = 0; i < column.w; i += 1) {
                    let run = native_runs[column.z + i];
                    let low = f32(run.x) * native_view.voxel.x;
                    let high = f32(run.y) * native_view.voxel.x;
                    var depth = t;
                    var normal = face;
                    if y0 >= high {
                        if direction.y >= 0. || y1 > high { continue; }
                        depth = ((high - native_view.voxel.z) - native_view.sky.y) / direction.y;
                        normal = 0.;
                    } else if y0 < low {
                        if direction.y <= 0. || y1 < low { continue; }
                        depth = ((low - native_view.voxel.z) - native_view.sky.y) / direction.y;
                        normal = 5.;
                    }
                    if depth < nearest.x { nearest = vec3f(max(depth,t), normal, f32(run.z)); }
                }
                if nearest.z > 0. { return nearest; }
            }
        }
        if end >= ray.hi { return vec3f(-1.); }
        let next = advance_cell(ray, edge, exit, mip, end);
        if any(next >= vec2i(i32(ray.grid.extent.w))) { return vec3f(-1.); }
        let parent = min(ray.grid.root, mip + 1u);
        if any((fine >> vec2u(parent)) != (next >> vec2u(parent))) { mip = parent; }
        fine = next;
        face = entry_face(ray, exit);
        t = end;
    }
    return vec3f(-2.);
}

fn rolling_light(normal: vec3f) -> vec3f {return rolling_light_visible(normal,1.);}

fn rolling_normal(face: i32) -> vec3f {
    switch face {
        case 1: { return vec3f(-1.,0.,0.); }
        case 2: { return vec3f(1.,0.,0.); }
        case 3: { return vec3f(0.,0.,-1.); }
        case 4: { return vec3f(0.,0.,1.); }
        case 5: { return vec3f(0.,-1.,0.); }
        default: { return vec3f(0.,1.,0.); }
    }
}

fn pigment_hash(cell: vec2i, salt: u32) -> f32 {
    var h = (bitcast<u32>(cell.x) * 0x9e3779b9u) ^ (bitcast<u32>(cell.y) * 0x85ebca6bu);
    h ^= salt ^ native_view.source.x ^ (native_view.source.y * 0xc2b2ae35u);
    h = (h ^ (h >> 16u)) * 0x7feb352du;
    h = (h ^ (h >> 15u)) * 0x846ca68bu;
    return f32((h ^ (h >> 16u)) & 0xffffu) / 65535.;
}

fn pigment_noise(p: vec2f, salt: u32) -> f32 {
    let cell = vec2i(floor(p));
    let f = fract(p);
    let w = f*f*(3.-2.*f);
    return mix(mix(pigment_hash(cell,salt),pigment_hash(cell+vec2i(1,0),salt),w.x),
               mix(pigment_hash(cell+vec2i(0,1),salt),pigment_hash(cell+vec2i(1),salt),w.x),w.y);
}

struct StairFace {normal: vec3f, layer: f32}

// Query the locally reconstructed slope for its visible native face.
// Scene depth still comes from the cached first hit.
fn local_stair(p: vec3f, ray: vec3f, normal: vec3f) -> StairFace {
    let miss = StairFace(vec3f(0.),-1.);
    let incidence = -dot(normal,ray);
    let extent = normal.y + .5 * (abs(normal.x) + abs(normal.z));
    let radius = extent / max(.001,incidence) + .01;
    if radius > 7. || normal.y < .12 { return miss; }
    let plane = fract(p * native_view.voxel.y);
    let start = plane - radius * ray;
    var cell = vec2i(floor(start.xz));
    let step = select(vec2i(-1),vec2i(1),ray.xz>=vec2f(0.));
    let stationary = abs(ray.xz) < vec2f(1e-7);
    let stride = select(1./max(vec2f(1e-7),abs(ray.xz)),vec2f(INF),stationary);
    let edge = select(vec2f(cell),vec2f(cell+1),step>vec2i(0));
    var next = select(abs(edge-start.xz)*stride,vec2f(INF),stationary);
    var enter = 0.;
    var side = vec3f(0.);
    for (var i=0u;i<16u;i+=1u) {
        let roof = floor(plane.y-dot(normal.xz,vec2f(cell)+.5-plane.xz)/normal.y);
        let end = min(min(next.x,next.y),2.*radius);
        if end > enter {
            let y = fma(enter,ray.y,start.y);
            if y < roof && dot(side,side) > .5 {
                return StairFace(side,max(0.,roof-1.-floor(y)));
            }
            if ray.y < -1e-7 {
                let t = (roof-start.y)/ray.y;
                if t >= enter && t < end { return StairFace(vec3f(0.,1.,0.),0.); }
            }
        }
        if end >= 2.*radius { break; }
        let crossing = next <= vec2f(end);
        cell += select(vec2i(0),step,crossing);
        next += select(vec2f(0.),stride,crossing);
        side = select(vec3f(0.,0.,-f32(step.y)),vec3f(-f32(step.x),0.,0.),crossing.x);
        enter = end;
    }
    return miss;
}

fn broad_sun_visibility(p: vec3f) -> f32 {
    let sun = native_view.sun.xyz;
    if native_view.source.z == 0u || sun.y <= 0. { return 1.; }
    // A low-frequency horizon test. Wide penumbras keep the coarse cache
    // from printing the shadow samples as hard bands over distant ridges.
    let low = parent_height(p.xz + 48.*sun.xz) - (p.y + 48.*sun.y);
    let high = parent_height(p.xz + 152.*sun.xz) - (p.y + 152.*sun.y);
    return 1.-.81*max(smoothstep(-2.,9.,low),smoothstep(-6.,21.,high));
}

fn pigment_sample(cell: vec2i, fine: bool) -> vec4f {
    if fine { return textureLoad(inner_pigment,cell,0); }
    return textureLoad(outer_pigment,cell,0);
}

struct SurfacePigment { bed_and_depth: vec4f, coverage: f32 }

fn filtered_pigment(world: vec2f, grid: Grid) -> SurfacePigment {
    let p=(world-grid.extent.xy)*grid.extent.z;
    let c=clamp(vec2i(floor(p)),vec2i(0),vec2i(i32(grid.extent.w)-1));
    let uv=clamp(p-vec2f(c),vec2f(0.),vec2f(1.));
    let a=pigment_sample(c,grid.fine);
    let b=pigment_sample(c+vec2i(1,0),grid.fine);
    let d=pigment_sample(c+vec2i(0,1),grid.fine);
    let e=pigment_sample(c+1,grid.fine);
    let depth=mix(mix(a,b,uv.x),mix(d,e,uv.x),uv.y);
    let wet=select(vec4f(0.),vec4f(1.),vec4f(a.a,b.a,d.a,e.a)>vec4f(0.));
    return SurfacePigment(depth,mix(mix(wet.x,wet.y,uv.x),mix(wet.z,wet.w,uv.x),uv.y));
}

fn cached_pigment(world: vec2f, fine: bool) -> SurfacePigment {
    if !fine { return filtered_pigment(world,outer_grid()); }
    let grid=inner_grid();
    let amount=filtered_pigment(world,grid);
    let delta=abs(world-view.eye.xz);
    let distance=max(delta.x,delta.y);
    let limits=fade_limits(grid);
    if distance<=limits.x { return amount; }
    let coarse=filtered_pigment(world,outer_grid());
    let w=smoothstep(limits.x,limits.y,distance);
    return SurfacePigment(mix(amount.bed_and_depth,coarse.bed_and_depth,w),
                          mix(amount.coverage,coarse.coverage,w));
}

fn rolling_water(ray:vec3f,normal:vec3f,below:vec3f)->vec3f {
    let sky=water_sky(reflect(ray,normal),native_view.sun.xyz,native_view.sky.x);
    return water_interface(ray,normal,below,sky,native_view.sun.xyz,native_view.sky.x);
}

fn distant_shade(depth: f32, direction: vec3f, fine: bool) -> vec3f {
    let p = fma(vec3f(depth), direction, view.eye.xyz);
    let normal = terrain_normal(vec2f(depth,select(0.,5.,fine)),p);
    let slope = length(normal.xz) / max(.025,normal.y);
    let ray = normalize(direction);
    let incidence = max(.025,-dot(ray,normal));
    let pixel = max(.0001, depth * 2. * native_view.voxel.w / f32(view.size.y));
    let footprint = pixel / incidence;
    let broad = pigment_noise(p.xz/120.,0xb5297a4du);
    let medium = pigment_noise(p.xz/28.,0x68e31da4u);
    let cover = mix(broad,medium,.28*(1.-smoothstep(5.,25.,footprint))) - .5;
    let limit = .68 + .35 * cover;
    var rock = max(smoothstep(limit-.09,limit+.09,slope),smoothstep(895.+190.*cover,980.+190.*cover,p.y));
    let meadow = native_view.palette[1].rgb * mix(vec3f(.84,.92,.99),vec3f(1.10,1.04,.91),broad);
    let stratum = pigment_noise(
        vec2f(p.x*.040+p.z*.021, p.y*.067+p.x*.014+1.3*broad+.52*medium),
        0x1b56c4e9u);

    let band = (stratum-.5)*(1.-smoothstep(2.5,13.,footprint));
    let stone = native_view.palette[3].rgb * (mix(vec3f(.77,.86,1.02),vec3f(1.11,1.03,.92),broad)+.15*band);
    let snow = smoothstep(1170.+90.*cover,1330.+90.*cover,p.y)*(1.-smoothstep(.65,1.35,slope));
    var roof = mix(mix(meadow,stone,rock),native_view.palette[9].rgb,snow*.78);
    var water = 0.;
    var water_color = vec3f(0.);
    if CANONICAL_PIGMENT {
        let sampled=cached_pigment(p.xz,fine);
        let bed=sampled.bed_and_depth.rgb;
        roof=bed*mix(.94,1.06,broad);
        // Keep sand and snow recognizable even on the shaded voxel risers.
        rock=1.-smoothstep(.06,.15,length(bed-native_view.palette[3].rgb));
        water=smoothstep(.4,.7,sampled.coverage);
        if water>0. {
            let relative=direction*depth;
            let n=rolling_water_normal(relative,vec3f(0.,1.,0.),ray,length(relative));
            let thickness=sampled.bed_and_depth.a*(255./16.)/sampled.coverage;
            water_color=rolling_water(ray,n,rolling_bed_transmission(ray,n,bed,thickness));
            if water>=.99 { return water_color; }
        }
    } else if p.z > -550. && p.z < 1500. {
        let trail = 16. + 60. * sin(p.z/310.) + .055*p.z;
        let width = clamp(max(pixel,.45*native_view.voxel.x),.025,10.);
        let track = .55 + (medium-.5)*.23;
        let distance = abs(p.x-trail);
        let core = 1.-smoothstep(max(0.,track-.5*width),track+.5*width,distance);
        let shoulder = 1.-smoothstep(track,track+.37+.5*width,distance);
        let strength = min(1.,(2.*track+.37)/width);
        roof = mix(roof,native_view.palette[2].rgb,mix(shoulder,core,.76)*strength);
    }
    let sign_x = select(1.,-1.,normal.x<0.);
    let sign_z = select(1.,-1.,normal.z<0.);
    let area = max(vec3f(0.), -ray * normal);
    let total = max(.0001,dot(area,vec3f(1.)));
    let rise = max(vec2f(.001),abs(normal.xz)/max(.025,normal.y));
    let skin = min(vec2f(1.),1./rise);
    let soil = select(mix(native_view.palette[2].rgb,stone,rock),mix(roof*.85,stone,rock),CANONICAL_PIGMENT);
    let soil_share = min(vec2f(1.),12./rise) - skin;
    let side_x = skin.x*roof + soil_share.x*soil + (1.-skin.x-soil_share.x)*stone;
    let side_z = skin.y*roof + soil_share.y*soil + (1.-skin.y-soil_share.y)*stone;
    // The projected area of the mean slope limits the direct light admitted
    // between opposing voxel risers.
    let sun = native_view.sun.xyz;
    let sun_visibility = broad_sun_visibility(p);
    let light_share = sun_visibility * clamp(dot(normal,sun)/max(.0001,dot(max(normal*sun,vec3f(0.)),vec3f(1.))),0.,1.);
    let filtered = (area.x*side_x*rolling_light_visible(vec3f(sign_x,0.,0.),light_share)
        + area.y*roof*rolling_light_visible(vec3f(0.,1.,0.),light_share)
        + area.z*side_z*rolling_light_visible(vec3f(0.,0.,sign_z),light_share)) / total;
    let voxel_pixels = native_view.voxel.x/footprint;
    let resolved = smoothstep(1.05,2.6,voxel_pixels)*smoothstep(.19,.40,incidence);
    var color = mix(roof*rolling_light_visible(normal,sun_visibility),filtered,smoothstep(0.,.09,total)*.92);
    if resolved > .01 {
        let face = local_stair(p,ray,normal);
        if face.layer >= 0. {
            let pigment = select(select(stone,soil,face.layer<12.),roof,face.layer<1.);
            color = mix(color,pigment*rolling_light_visible(face.normal,light_share),resolved*.86);
        }
    }
    return mix(color,water_color,water);
}

@vertex
fn rolling_triangle(@builtin(vertex_index) index: u32) -> @builtin(position) vec4f {
    let xy = vec2f(f32((index << 1u) & 2u), f32(index & 2u));
    return vec4f(xy * 2. - 1., 0., 1.);
}

struct RollingFragment {
    @location(0) color: vec4f,
    @builtin(frag_depth) depth: f32,
}

@fragment
fn rolling_fragment(@builtin(position) pixel: vec4f) -> RollingFragment {
    let direction = screen_ray(vec2u(pixel.xy));
    let far = view.eye.w;
    let grid = Grid(native_view.grid,vec2i(0),u32(firstTrailingBit(u32(native_view.grid.w))),false);
    let local_ray = ray_grid_at(native_view.sky.zw,direction.xz,grid,NEAR,far);
    let local = native_hit(direction,local_ray);
    var t = local.x;
    var color = vec3f(0.);
    var owned = false;
    var stage = vec3f(0.,1.,0.); // exact columns
    if t >= 0. {
        let p = fma(vec3f(t),direction,view.eye.xyz);
        owned = native_view.owner_min.w > .5 && all(p > native_view.owner_min.xyz) && all(p < native_view.owner_max.xyz);
        let normal = rolling_normal(i32(local.y));
        color = native_view.palette[u32(local.z)].rgb * rolling_light(normal);
        if local.z == 7. {
            let relative=direction*t;
            let ray=normalize(direction);
            let optics=rolling_column_optics(relative,normal,ray,length(relative));
            color=rolling_water(ray,optics.normal,optics.below);
        }
    } else {
        var near = NEAR;
        if local_ray.lo < local_ray.hi { near = local_ray.hi; }
        var counts = vec4u(0);
        let grid = inner_grid();
        let inner_far = select(far,near,CANONICAL_PIGMENT && fade_limits(grid).y<=0.);
        let ray = ray_grid(direction.xz,grid,near,inner_far);
        var found = trace(direction,ray,&counts);
        let fine = found.x >= 0.;
        if !fine && found.x != -2. {
            if ray.lo < ray.hi { near = ray.hi; }
            found = trace(direction,ray_grid(direction.xz,outer_grid(),near,far),&counts);
        }
        if found.x < 0. {
            if found.x == -2. || local.x == -2. { return RollingFragment(vec4f(1.,0.,1.,1.),.999999); }
            discard;
        }
        t = found.x;
        color = distant_shade(t,direction,fine);
        stage = select(vec3f(1.,0.,0.),vec3f(1.,1.,0.),fine);
    }
    let depth = clamp((1.-NEAR/max(t,NEAR))*far/(far-NEAR),0.,1.);
    // Give the previously drawn NAADF face priority for coincident hits.
    // Exact columns still fill any rays that the volume pass fails to cover.
    let composed_depth = select(depth, min(1., bitcast<f32>(bitcast<u32>(depth) + 4u)), owned);
    return RollingFragment(vec4f(select(color,stage,native_view.source.w!=0u),1.), composed_depth);
}
