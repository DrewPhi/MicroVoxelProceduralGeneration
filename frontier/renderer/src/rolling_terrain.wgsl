// Playable composition of the experimental rolling heightfield and exact columns.
@group(0) @binding(10) var native_maxima: texture_2d<f32>;
@group(0) @binding(12) var<storage, read> native_runs: array<vec4i>;
@group(0) @binding(13) var outer_pigment: texture_2d<f32>;
@group(0) @binding(14) var inner_pigment: texture_2d<f32>;
override CANONICAL_PIGMENT: bool = false;
override LANDSCAPES: bool = false;
override NATURAL_TERRAIN_LIGHT: bool = false;
override FOREST_AERIAL_LIGHT: bool = false;
override PROJECTED_DETAIL: bool = false;
override CAPTURE_DISTANCE: bool = false;
@group(0) @binding(18) var detail_maxima: texture_2d<f32>;
@group(0) @binding(19) var detail_samples: texture_2d<f32>;

fn detail_grid() -> Grid {
    return Grid(view.old_eye,vec2i(0),firstTrailingBit(u32(view.old_eye.w)),false);
}

fn detail_resident_end(grid: Grid) -> f32 {
    let eye = (view.eye.xz-grid.extent.xy)*grid.extent.z;
    let border = min(eye,grid.extent.w-eye);
    let resident = max(0.,min(border.x,border.y)-2.)/grid.extent.z;
    let parent=inner_grid();
    // Only blend into the fully native portion of the parent cache. Include
    // space for the neighbouring sample used at the last detail patch.
    let safe_parent=max(0.,fade_limits(parent).x-2./parent.extent.z);
    return min(resident,safe_parent);
}
fn detail_blend_range(grid: Grid) -> vec2f {
    let metres_per_pixel_per_depth = 2.*native_view.voxel.w* native_view.water.w;
    let end = min(detail_resident_end(grid),1./(grid.extent.z*metres_per_pixel_per_depth));
    return vec2f(.64*end,end);
}
fn detail_envelope(grid: Grid) -> vec2f {
    let automatic=detail_blend_range(grid);
    if !terrain_curve_active() {return automatic;}
    let start=select(automatic.x,.45*automatic.x,view.old_forward.w>0.);
    let end=select(automatic.y,min(detail_resident_end(grid),1.30*automatic.y),view.old_forward.z>0.);
    return vec2f(start,end);
}
fn detail_limits(grid: Grid, distance: f32) -> vec2f {
    let automatic=detail_blend_range(grid);
    let bias=terrain_curve_bias(distance);
    if bias==0. {return automatic;}
    if bias>0. {
        let end=min(detail_resident_end(grid),1.30*automatic.y);
        return mix(automatic,vec2f(.84*end,end),bias);
    }
    return mix(automatic,.45*automatic,-bias);
}
fn profile_detail_strength(distance: f32) -> f32 {
    let bias=terrain_curve_bias(distance);
    return select(1.+min(0.,bias),max(0.,bias)*view.old_right.x,!PROJECTED_DETAIL);
}

fn sampled_grid_position(grid: Grid, world: vec2f) -> vec4f {
    let g = (world-grid.extent.xy)*grid.extent.z;
    let cell = clamp(floor(g),vec2f(0.),vec2f(grid.extent.w-1.));
    return vec4f(cell,clamp(g-cell,vec2f(0.),vec2f(1.)));
}

fn detail_sample(grid: Grid, cell: vec2i, limits: vec2f) -> f32 {
    let height = textureLoad(detail_samples,cell,0).r;
    let world = grid.extent.xy+vec2f(cell)/grid.extent.z;
    let offset = abs(world-view.eye.xz);
    let range = max(offset.x,offset.y);
    if range<=limits.x && !terrain_curve_active() {return height;}
    let blend=detail_limits(grid,range);
    let strength=profile_detail_strength(range);
    if range<=blend.x && strength>=1. {return height;}
    let parent = inner_grid();
    let sample_pos = sampled_grid_position(parent,world);
    let c=vec2i(sample_pos.xy);
    let h=terrain_corners(parent,c);
    let coarse = mix(mix(h.x,h.y,sample_pos.z),mix(h.z,h.w,sample_pos.z),sample_pos.w);
    if strength<=0. || range>=blend.y {return coarse;}
    let nominal = smoothstep(blend.x,blend.y,range);
    let world_pixel = max(.01,range*2.*native_view.voxel.w*native_view.water.w);
    let residual = abs(height-coarse)/world_pixel;
    let retain = smoothstep(.6,1.6,residual);
    let edge = smoothstep(.82*blend.y,blend.y,range);
    let weight=mix(nominal,edge,retain);
    if !terrain_curve_active() {return mix(height,coarse,weight);}
    return mix(coarse,height,strength*(1.-weight));
}

fn detail_corners(grid: Grid, cell: vec2i, limits: vec2f) -> vec4f {
    return vec4f(detail_sample(grid,cell,limits),detail_sample(grid,cell+vec2i(1,0),limits),
                 detail_sample(grid,cell+vec2i(0,1),limits),detail_sample(grid,cell+1,limits));
}

fn detail_parent_max(parent: Grid, cell: vec2i, mip: u32) -> f32 {
    if !terrain_curve_active() {return bounds_at(parent,cell,mip).x;}
    var counts=vec4u(0);
    return terrain_bounds(parent,cell,mip,&counts).x;
}
fn detail_bound(grid: Grid, cell: vec2i, mip: u32, fade_start: f32) -> f32 {
    let cached = textureLoad(detail_maxima,cell,i32(mip)).r;
    let width = f32(1u<<mip)/grid.extent.z;
    let lo = grid.extent.xy+vec2f(cell)*width;
    let hi = lo+width;
    let delta = max(abs(lo-view.eye.xz),abs(hi-view.eye.xz));
    if max(delta.x,delta.y)<=fade_start && !terrain_curve_active() {return cached;}
    let parent = inner_grid();
    let parent_mip = u32(clamp(ceil(log2(width*parent.extent.z)),0.,f32(parent.root)));
    let scale = parent.extent.z/f32(1u<<parent_mip);
    let last = vec2i((i32(parent.extent.w)>>parent_mip)-1);
    let a = clamp(vec2i(floor((lo-parent.extent.xy)*scale)),vec2i(0),last);
    let b = clamp(vec2i(floor((hi-parent.extent.xy)*scale)),vec2i(0),last);
    var maximum=max(cached,detail_parent_max(parent,a,parent_mip));
    if a.x!=b.x {maximum=max(maximum,detail_parent_max(parent,vec2i(b.x,a.y),parent_mip));}
    if a.y!=b.y {maximum=max(maximum,detail_parent_max(parent,vec2i(a.x,b.y),parent_mip));}
    if all(a!=b) {maximum=max(maximum,detail_parent_max(parent,b,parent_mip));}
    return maximum;
}

fn trace_detail(direction: vec3f, ray: Ray, limits: vec2f, stop:f32) -> f32 {
    if ray.hi<=ray.lo {return -1.;}
    let root = ray.grid.root;
    var cell = initial_cell(ray);
    var t = ray.lo;
    var mip = root;
    for (var step=0u;step<u32(ray.grid.extent.w)*8u+64u;step+=1u) {
        if t>stop {return -1.;}
        let edge = cell_edge(cell,mip);
        let exit = exit_times(ray,edge);
        let end = min(ray.hi,min(exit.x,exit.y));
        let top = detail_bound(ray.grid,lookup_cell(ray,cell,mip),mip,limits.x);
        if end>t && min(fma(t,direction.y,view.eye.y),fma(end,direction.y,view.eye.y))<=top {
            if mip>0u {
                let lower=edge-(1i<<mip);
                mip-=1u;
                if direction.y<0. {
                    let later=clamp((top+.001-view.eye.y)/direction.y,t,end);
                    if later>t && later<end {
                        cell=clamp(fine_at(ray,later),lower,edge-1);
                        t=later;
                    }
                }
                continue;
            }
            let h = detail_corners(ray.grid,lookup_cell(ray,cell,0u),limits);
            let mirror = clamp(fma(vec2f(t),ray.velocity,ray.origin)-vec2f(cell),vec2f(0.),vec2f(1.));
            let uv = select(mirror,1.-mirror,ray.signs<vec2i(0));
            let delta = vec2f(ray.signs)*ray.velocity*(end-t);
            let dx = h.y-h.x;
            let dz = h.z-h.x;
            let cross = h.w-h.z-dx;
            let base = mix(mix(h.x,h.y,uv.x),mix(h.z,h.w,uv.x),uv.y);
            let curve = vec3f(base,dot(delta,vec2f(dx+cross*uv.y,dz+cross*uv.x)),cross*delta.x*delta.y);
            let hit = bilinear_hit(curve,direction.y,t,end);
            if hit>=0. {return hit;}
        }
        if end>=ray.hi {return -1.;}
        let next = advance_cell(ray,edge,exit,mip,end);
        if any(next>=vec2i(i32(ray.grid.extent.w))) {return -1.;}
        let parent = min(root,mip+1u);
        if any((cell>>vec2u(parent))!=(next>>vec2u(parent))) {mip=parent;}
        cell=next;
        t=end;
    }
    return -2.;
}

fn detail_normal(p: vec3f, grid: Grid, parent: vec3f) -> vec3f {
    let offset = abs(p.xz-view.eye.xz);
    let distance = max(offset.x,offset.y);
    let fade=detail_limits(grid,distance);
    let strength=profile_detail_strength(distance);
    if distance>=fade.y || strength<=0. {return parent;}
    let sample_pos = sampled_grid_position(grid,p.xz);
    let uv = sample_pos.zw;
    let cell = vec2i(sample_pos.xy);
    let h = vec4f(textureLoad(detail_samples,cell,0).r,
                 textureLoad(detail_samples,cell+vec2i(1,0),0).r,
                 textureLoad(detail_samples,cell+vec2i(0,1),0).r,
                 textureLoad(detail_samples,cell+1,0).r);
    let dx = mix(h.y-h.x,h.w-h.z,uv.y)*grid.extent.z;
    let dz = mix(h.z-h.x,h.w-h.y,uv.x)*grid.extent.z;
    let fine = normalize(vec3f(-dx,1.,-dz));
    let weight=smoothstep(fade.x,fade.y,distance);
    if !terrain_curve_active() {return normalize(mix(fine,parent,weight));}
    return normalize(mix(parent,fine,strength*(1.-weight)));
}

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

fn native_hit(direction: vec3f, ray: Ray, stop:f32) -> vec3f {
    if ray.lo >= ray.hi { return vec3f(-1.); }
    var fine = initial_cell(ray);
    var t = ray.lo;
    var face = 0.;
    var mip = ray.grid.root;
    for (var step = 0u; step < u32(ray.grid.extent.w) * 8u + 64u; step += 1u) {
        if t>stop {return vec3f(-1.);}
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

fn pigment_bits(cell: vec2i, salt: u32) -> u32 {
    var h = (bitcast<u32>(cell.x) * 0x9e3779b9u) ^ (bitcast<u32>(cell.y) * 0x85ebca6bu);
    h ^= salt ^ native_view.source.x ^ (native_view.source.y * 0xc2b2ae35u);
    h = (h ^ (h >> 16u)) * 0x7feb352du;
    h = (h ^ (h >> 15u)) * 0x846ca68bu;
    return h ^ (h >> 16u);
}

fn pigment_hash(cell: vec2i, salt: u32) -> f32 {
    return f32(pigment_bits(cell,salt)&0xffffu)/65535.;
}

fn pigment_noise(p: vec2f, salt: u32) -> f32 {
    let cell = vec2i(floor(p));
    let f = fract(p);
    let w = f*f*(3.-2.*f);
    return mix(mix(pigment_hash(cell,salt),pigment_hash(cell+vec2i(1,0),salt),w.x),
               mix(pigment_hash(cell+vec2i(0,1),salt),pigment_hash(cell+vec2i(1),salt),w.x),w.y);
}

// Canopy coverage uses the same habitat field as placed vegetation.
struct CanopyGround { coverage:vec4f, sun:vec4f };
@group(0) @binding(28) var<uniform> canopy_ground:CanopyGround;
@group(0) @binding(29) var canopy_opacity:texture_2d<f32>;
@group(0) @binding(30) var canopy_sampler:sampler;

fn canopy_ground_light(relative:vec3f)->f32 {
    let g=canopy_ground.coverage;
    if g.w<=0. {return 1.;}
    let p=(relative.xz-g.xy)*g.z;
    let edge=max(abs(p.x),abs(p.y));
    if edge>=1. {return 1.;}
    let uv=.5+.5*p;
    let shade=textureSampleLevel(canopy_opacity,canopy_sampler,uv,0.).r;
    return 1.-g.w*shade*(1.-smoothstep(.88,1.,edge));
}

fn woodland_crown(cell:vec2i,coord:vec2f,pixel:f32,habitat:f32)->vec2f {
    let bits=pigment_bits(cell,0xd2f38a1bu);
    let random=vec4f(vec4u(bits,bits>>8u,bits>>16u,bits>>24u)&vec4u(255u))/255.;
    let center=vec2f(cell)+.105+.79*random.xy;
    let offset=coord-center;
    let radius=.205+.125*random.w;
    let present=smoothstep(-.025,.025,habitat-random.z);
    let area=present*(1.-smoothstep(radius-pixel,radius+pixel,length(offset)));
    let shade=.67+.60*clamp(-dot(offset,native_view.sun.xz)/radius,-.37,.47);
    return vec2f(area,shade*area);
}

// Crowns cross the nearest cell edges as real tree crowns do. Broad jitter
// avoids a visible grid, and every footprint converges to mean habitat area.
fn filtered_woodland(world:vec2f, footprint:f32, habitat:f32)->vec2f {
    let spacing=128.*native_view.voxel.x;
    let projected=1.-smoothstep(.19,.73,footprint/spacing);
    let mean_area=.224*habitat;
    if projected<=0. { return vec2f(mean_area,.79); }
    let coord=world/spacing;
    let cell=vec2i(floor(coord));
    let local=fract(coord);
    let side=select(vec2i(-1),vec2i(1),local>vec2f(.5));
    let edge=min(local,1.-local);
    let pixel=clamp(.47*footprint/spacing,.023,.105);
    let reach=.33+pixel-.105;
    var sampled=woodland_crown(cell,coord,pixel,habitat);
    var open=1.-sampled.x;
    if edge.x<reach {
        let c=woodland_crown(cell+vec2i(side.x,0),coord,pixel,habitat);
        sampled+=c;
        open*=1.-c.x;
    }
    if edge.y<reach {
        let c=woodland_crown(cell+vec2i(0,side.y),coord,pixel,habitat);
        sampled+=c;
        open*=1.-c.x;
        if edge.x<reach {
            let c=woodland_crown(cell+side,coord,pixel,habitat);
            sampled+=c;
            open*=1.-c.x;
        }
    }
    let crown_area=1.-open;
    let crown_light=crown_area*sampled.y/max(.0001,sampled.x);
    let area=mix(mean_area,crown_area,projected);
    let light=mix(.79*mean_area,crown_light,projected)/max(.0001,area);
    return vec2f(area,light);
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

// Interpolate vertex gradients on the distant grid. Differentiating the
// bilinear depth patches alone produces a sharp normal seam at each grid line.
fn distant_normal(world: vec2f) -> vec3f {
    let grid = outer_grid();
    let g = (world-grid.extent.xy)*grid.extent.z;
    let c = clamp(vec2i(floor(g)),vec2i(1),vec2i(i32(grid.extent.w)-2));
    let uv = clamp(g-vec2f(c),vec2f(0.),vec2f(1.));
    let center = terrain_corners(grid,c);
    let left = vec2f(raw_sample(grid,c+vec2i(-1,0)),raw_sample(grid,c+vec2i(-1,1)));
    let right = vec2f(raw_sample(grid,c+vec2i(2,0)),raw_sample(grid,c+vec2i(2,1)));
    let up = vec2f(raw_sample(grid,c+vec2i(0,-1)),raw_sample(grid,c+vec2i(1,-1)));
    let down = vec2f(raw_sample(grid,c+vec2i(0,2)),raw_sample(grid,c+vec2i(1,2)));
    let x = mix(center.yw-left,right-center.xz,uv.x);
    let z = mix(center.zw-up,down-center.xy,uv.y);
    let grade = vec2f(mix(x.x,x.y,uv.y),mix(z.x,z.y,uv.x))*(.5*grid.extent.z);
    return normalize(vec3f(-grade.x,1.,-grade.y));
}

fn broad_irradiance(p: vec3f) -> vec2f {
    let sun = native_view.sun.xyz;
    if native_view.source.z == 0u { return vec2f(1.); }
    if NATURAL_TERRAIN_LIGHT {return terrain_irradiance(p);}
    if sun.y <= 0. {return vec2f(1.);}
    // A low-frequency horizon test. Wide penumbras keep the coarse cache
    // from printing the shadow samples as hard bands over distant ridges.
    let low = parent_height(p.xz + 48.*sun.xz) - (p.y + 48.*sun.y);
    let high = parent_height(p.xz + 152.*sun.xz) - (p.y + 152.*sun.y);
    var shadow = max(smoothstep(-2.,9.,low),smoothstep(-6.,21.,high));
    if LANDSCAPES {
        let far = parent_height(p.xz + 340.*sun.xz) - (p.y + 340.*sun.y);
        shadow = max(shadow,smoothstep(-13.,47.,far)*.88);
    }
    return vec2f(1.-.81*shadow,1.);
}

fn pigment_sample(cell: vec2i, fine: bool) -> vec4f {
    if fine {
        let address=select(cell,scrolled_vertex(inner_grid(),cell),PROJECTED_MID && view.inner_cache.w==2);
        return textureLoad(inner_pigment,address,0);
    }
    let address=select(cell,scrolled_vertex(outer_grid(),cell),SCROLLED_OUTER);
    return textureLoad(outer_pigment,address,0);
}

struct SurfacePigment { bed_and_depth: vec4f, coverage: f32, grove: f32 }

fn bilerp_channels(a:vec3f,b:vec3f,c:vec3f,d:vec3f,uv:vec2f)->vec3f {
    return mix(mix(a,b,uv.x),mix(c,d,uv.x),uv.y);
}

// Metres of water, fractional wetness, and expected native grove density.
fn pigment_channels(alpha:f32)->vec3f {
    if LANDSCAPES {
        let packed = round(alpha*255.);
        if packed>=128. {return vec3f((packed-128.)*.125,1.,0.);}
        return vec3f(0.,0.,packed/127.);
    }
    return vec3f(alpha*(255./16.),select(0.,1.,alpha>0.),0.);
}

fn filtered_pigment(world: vec2f, grid: Grid) -> SurfacePigment {
    let p=(world-grid.extent.xy)*grid.extent.z;
    let c=clamp(vec2i(floor(p)),vec2i(0),vec2i(i32(grid.extent.w)-1));
    let uv=clamp(p-vec2f(c),vec2f(0.),vec2f(1.));
    let a=pigment_sample(c,grid.fine);
    let b=pigment_sample(c+vec2i(1,0),grid.fine);
    let d=pigment_sample(c+vec2i(0,1),grid.fine);
    let e=pigment_sample(c+1,grid.fine);
    let pigment=bilerp_channels(a.rgb,b.rgb,d.rgb,e.rgb,uv);
    let channels=bilerp_channels(pigment_channels(a.a),pigment_channels(b.a),
                                  pigment_channels(d.a),pigment_channels(e.a),uv);
    return SurfacePigment(vec4f(pigment,channels.x),channels.y,channels.z);
}

fn cached_pigment(world: vec2f, fine: bool) -> SurfacePigment {
    if !fine { return filtered_pigment(world,outer_grid()); }
    let grid=inner_grid();
    let amount=filtered_pigment(world,grid);
    let delta=abs(world-view.eye.xz);
    let distance=max(delta.x,delta.y);
    let bias=terrain_curve_bias(distance);
    let limits=mid_blend_limits(grid,bias);
    if distance<=limits.x && bias>=0. { return amount; }
    let coarse=filtered_pigment(world,outer_grid());
    let w=coarsen_terrain(smoothstep(limits.x,limits.y,distance),bias);
    return SurfacePigment(mix(amount.bed_and_depth,coarse.bed_and_depth,w),
                          mix(amount.coverage,coarse.coverage,w),mix(amount.grove,coarse.grove,w));
}

fn rolling_water(ray:vec3f,normal:vec3f,below:vec3f)->vec3f {
    let sky=water_sky(reflect(ray,normal),native_view.sun.xyz,native_view.sky.x);
    return water_interface(ray,normal,below,sky,native_view.sun.xyz,native_view.sky.x);
}

fn distant_shade(depth: f32, direction: vec3f, fine: bool, detailed: bool) -> vec3f {
    let p = fma(vec3f(depth), direction, view.eye.xyz);
    var normal = terrain_normal(vec2f(depth,select(0.,5.,fine)),p);
    if LANDSCAPES {
        // Use the same parent normal before and after the sampled-ring handoff.
        let delta = abs(p.xz-view.eye.xz);
        let distance=max(delta.x,delta.y);
        let bias=terrain_curve_bias(distance);
        let limits = mid_blend_limits(inner_grid(),bias);
        let weight = select(1.,coarsen_terrain(smoothstep(limits.x,limits.y,distance),bias),fine);
        if weight > 0. {
            normal = normalize(mix(normal,distant_normal(p.xz),weight));
        }
    }
    if PROJECTED_DETAIL && detailed {
        normal = detail_normal(p,detail_grid(),normal);
    }
    if (native_view.source.w&4u)!=0u {return vec3f(.38)*rolling_light_visible(normal,1.);}
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
        if LANDSCAPES {
            let grass = 1.-smoothstep(.07,.21,length(bed-native_view.palette[1].rgb));
            let sand = 1.-smoothstep(.06,.22,length(bed-native_view.palette[8].rgb));
            let snow_cover = 1.-smoothstep(.08,.23,length(bed-native_view.palette[9].rgb));
            let exposed = smoothstep(.62,1.48,slope)*(1.-snow_cover)*(.58-.34*sand);
            let bedding = (stratum-.5)*mix(.22,.52,broad)*(1.-smoothstep(10.,36.,footprint));
            let grain = (medium-.5)*(1.-smoothstep(5.,20.,footprint));
            let grass_tint = mix(vec3f(.86,.76,.81),vec3f(1.16,.95,.75),broad);
            let stone_tint = mix(vec3f(.84,.89,.93),vec3f(1.19,1.06,.85),broad);
            roof = bed*mix(vec3f(.91+.20*broad),grass_tint,grass)*(1.+.23*grain);
            roof *= mix(vec3f(1.),stone_tint*(1.+1.88*bedding),rock);
            let exposed_color = native_view.palette[3].rgb*stone_tint*(1.+2.18*bedding);
            roof = mix(roof,exposed_color,exposed);
            rock = max(rock,exposed);
            if native_view.owner_max.w>.5 {
                // Away from drawable individual trees, carry the broad
                // woodland footprint without inventing vegetation geometry.
                let canopy = sampled.grove*(1.-smoothstep(.58,1.42,slope));
                let distance=length(direction*depth);
                let tree_radius=native_view.world_eye.w;
                let cover_weight=select(smoothstep(570.,1050.,distance),
                    clamp((distance-.78*tree_radius)/max(.22*tree_radius,1.),0.,1.),
                    NATURAL_TERRAIN_LIGHT && tree_radius>0.);
                let leaves = roof*select(vec3f(.40,.62,.48),vec3f(.37,.54,.53),NATURAL_TERRAIN_LIGHT)
                    *(1.+grain*.48);
                if FOREST_AERIAL_LIGHT {
                    // The cached habitat provides regional canopy skylight
                    // attenuation where the close dynamic shadow map ends.
                    let altitude=max(native_view.world_eye.y-p.y,0.);
                    let reach=max(smoothstep(62.,175.,distance),smoothstep(28.,114.,altitude));
                    let cooling=vec3f(.36,.28,.23);
                    roof*=1.-cooling*canopy*reach*(.88+.24*medium);
                }
                if NATURAL_TERRAIN_LIGHT && canopy>.015 && cover_weight>0. {
                    let cover=filtered_woodland(p.xz,footprint,canopy);
                    let floor_color=roof*(1.-select(.27,.075,FOREST_AERIAL_LIGHT)*canopy);
                    let crown_color=leaves*(.64+.42*cover.y);
                    roof=mix(roof,mix(floor_color,crown_color,cover.x),cover_weight);
                } else {
                    roof = mix(roof,leaves,canopy*cover_weight*.66);
                }
            }
            // Preserve partial water coverage at shorelines instead of applying
            // a binary shoreline threshold to the bilinearly filtered mask.
            water = sampled.coverage;
        }
        if water>0. {
            let relative=direction*depth;
            let n=rolling_water_normal(relative,vec3f(0.,1.,0.),ray,length(relative));
            let thickness=sampled.bed_and_depth.a/sampled.coverage;
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
    let irradiance = broad_irradiance(p);
    let sun_visibility = irradiance.x*aircraft_sun_visibility(direction*depth)
        *formation_sun_visibility(direction*depth)*woodland_sun_visibility(direction*depth+normal*.04)
        *canopy_ground_light(direction*depth);
    let light_share = sun_visibility * clamp(dot(normal,sun)/max(.0001,dot(max(normal*sun,vec3f(0.)),vec3f(1.))),0.,1.);
    let filtered = (area.x*side_x*rolling_light_irradiance(vec3f(sign_x,0.,0.),light_share,irradiance.y)
        + area.y*roof*rolling_light_irradiance(vec3f(0.,1.,0.),light_share,irradiance.y)
        + area.z*side_z*rolling_light_irradiance(vec3f(0.,0.,sign_z),light_share,irradiance.y)) / total;
    let voxel_pixels = native_view.voxel.x/footprint;
    let resolved = smoothstep(1.05,2.6,voxel_pixels)*smoothstep(.19,.40,incidence);
    var color = mix(roof*rolling_light_irradiance(normal,sun_visibility,irradiance.y),filtered,smoothstep(0.,.09,total)*.92);
    if resolved > .01 {
        let face = local_stair(p,ray,normal);
        if face.layer >= 0. {
            let pigment = select(select(stone,soil,face.layer<12.),roof,face.layer<1.);
            color = mix(color,pigment*rolling_light_irradiance(face.normal,light_share,irradiance.y),resolved*.86);
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

fn projected_depth(t:f32,far:f32)->f32 {
    return clamp((1.-NEAR/max(t,NEAR))*far/(far-NEAR),0.,bitcast<f32>(0x3f7fffffu));
}

@fragment
fn rolling_fragment(@builtin(position) pixel: vec4f) -> RollingFragment {
    let direction = screen_ray(vec2u(pixel.xy));
    let far = view.eye.w;
    let foreground=foreground_at(vec2u(pixel.xy));
    var stop=INF;
    if foreground<1. {
        // Stop only at interval boundaries. Retain the same polynomial roots
        // and column crossings in front of this conservative foreground bound.
        let padded=min(1.,bitcast<f32>(bitcast<u32>(foreground)+8u));
        let distance=NEAR*far/max(NEAR,far-padded*(far-NEAR));
        stop=distance*1.0001+.01;
    }
    // screen_ray has unit *forward* depth. Clip at a world-space radius so
    // turning cannot move distant terrain through the projection's far plane.
    let ray_limit = far * inverseSqrt(dot(direction, direction));
    let grid = Grid(native_view.grid,vec2i(0),u32(firstTrailingBit(u32(native_view.grid.w))),false);
    var native_limit = ray_limit;
    if PROJECTED_DETAIL {
        let projected = native_view.voxel.x/(2.*native_view.voxel.w*native_view.water.w);
        // Clip along view-space depth; the resized exact cache supplies a
        // bounded fallback whenever its resident edge is closer.
        native_limit = min(ray_limit,projected*1.20);
    }
    let local_far = select(NEAR,native_limit,view.size.w!=0u);
    let local_ray = ray_grid_at(native_view.sky.zw,direction.xz,grid,NEAR,local_far);
    let local = native_hit(direction,local_ray,stop);
    var t = local.x;
    var color = vec3f(0.);
    var sampled_weight = 0.;
    var fine = true;
    var detailed = false;
    var owned = false;
    var stage = vec3f(0.,1.,0.); // exact columns
    if t >= 0. {
        let p = fma(vec3f(t),direction,view.eye.xyz);
        owned = native_view.owner_min.w > .5 && all(p > native_view.owner_min.xyz) && all(p < native_view.owner_max.xyz);
        let depth=projected_depth(t,far);
        let candidate=bitcast<f32>(bitcast<u32>(depth)+select(0u,4u,owned));
        if foreground<1. && candidate>=foreground {discard;}
        let normal = rolling_normal(i32(local.y));
        let relative = direction*t;
        let ray = normalize(direction);
        var sky_light = vec2f(1.);
        if NATURAL_TERRAIN_LIGHT && (native_view.source.w&4u)==0u && local.z!=7. {
            sky_light = broad_irradiance(p);
        }
        color = native_view.palette[u32(local.z)].rgb * rolling_light_irradiance(
            normal,sky_light.x*aircraft_sun_visibility(relative)*formation_sun_visibility(relative)
                *woodland_sun_visibility(relative+normal*.04),sky_light.y);
        if (native_view.source.w&4u)!=0u {
            color = vec3f(.38)*rolling_light_visible(normal,1.);
        } else if local.z == 7. {
            let optics=rolling_column_optics(relative,normal,ray,length(relative));
            color=rolling_water(ray,optics.normal,optics.below);
        }
        if local.z!=7. && LANDSCAPES && (native_view.source.w&2u)!=0u {
            // Match the sampled surface lit just beyond the exact window,
            // including at its border where an exact slope stencil would
            // run out of neighbouring columns.
            var slope_normal = terrain_normal(vec2f(t,5.),p);
            if PROJECTED_DETAIL {
                let g=detail_grid();
                let delta=(p.xz-g.extent.xy)*g.extent.z;
                detailed = all(delta>=vec2f(0.)) && all(delta<vec2f(g.extent.w));
                if detailed {slope_normal=detail_normal(p,g,slope_normal);}
            }
            let resolved = rolling_voxel_resolution(relative,ray,slope_normal);
            sampled_weight = select(1.-resolved,0.,resolved>=.985);
        }
    } else {
        var near = NEAR;
        if local_ray.lo < local_ray.hi { near = local_ray.hi; }
        var counts = vec4u(0);
        let grid = inner_grid();
        let fade=mid_envelope(grid).y;
        var inner_far = select(ray_limit,near,CANONICAL_PIGMENT && fade<=0.);
        if PROJECTED_MID && view.old_forward.y>.5 && fade>0. {
            // Aligned lattices reconstruct the same bilinear patch once
            // all four fine corners have faded to the parent.
            let radius=fade+1./grid.extent.z;
            let limits=Grid(vec4f(view.eye.xz-vec2f(radius),1.,2.*radius),vec2i(0),0u,false);
            inner_far=ray_grid(direction.xz,limits,near,ray_limit).hi;
        }
        let ray = ray_grid(direction.xz,grid,near,inner_far);
        var found = vec2f(-1.,0.);
        if PROJECTED_DETAIL || view.old_right.x>0. {
            let dgrid = detail_grid();
            let limits = detail_envelope(dgrid);
            if limits.y>0. {
                // The close surface is already its parent at the boundary.
                let radius=limits.y+1./dgrid.extent.z;
                let extent = vec4f(view.eye.xz-vec2f(radius),dgrid.extent.z,2.*radius*dgrid.extent.z);
                let clip = ray_grid(direction.xz,Grid(extent,vec2i(0),0u,false),near,ray_limit);
                let d_ray=ray_grid(direction.xz,dgrid,near,clip.hi);
                found.x=trace_detail(direction,d_ray,limits,stop);
                detailed=found.x>=0.;
                if !detailed && d_ray.lo<d_ray.hi {near=d_ray.hi;}
            }
        }
        if !detailed {
            found=trace_until(direction,ray_grid(direction.xz,grid,near,inner_far),stop,&counts);
        }
        fine = found.x >= 0.;
        if !fine && found.x != -2. {
            if ray.lo < ray.hi { near = ray.hi; }
            found = trace_until(direction,ray_grid(direction.xz,outer_grid(),near,ray_limit),stop,&counts);
        }
        if found.x < 0. {
            if found.x == -2. || local.x == -2. { return RollingFragment(vec4f(1.,0.,1.,1.),.999999); }
            discard;
        }
        t = found.x;
        if foreground<1. && projected_depth(t,far)>=foreground {discard;}
        sampled_weight = 1.;
        stage = select(select(vec3f(1.,0.,0.),vec3f(1.,1.,0.),fine),vec3f(0.,0.,1.),detailed);
    }
    if sampled_weight>0. && (native_view.source.w&1u)==0u {
        color = mix(color,distant_shade(t,direction,fine,detailed),sampled_weight);
    }
    // Keep valid hits distinct from the depth clear value even when rounding
    // very close to the far plane. The atmosphere uses 1.0 to identify sky.
    let last_hit_depth = bitcast<f32>(0x3f7fffffu);
    let depth = projected_depth(t,far);
    // Give the previously drawn NAADF face priority for coincident hits.
    // Exact columns still fill any rays that the volume pass fails to cover.
    let composed_depth = select(depth, min(last_hit_depth, bitcast<f32>(bitcast<u32>(depth) + 4u)), owned);
    if CAPTURE_DISTANCE {
        return RollingFragment(vec4f(vec3f(t * length(direction)), 1.), composed_depth);
    }
    // Preserve the fade computed from ray distance before nonlinear depth
    // quantization. Alpha is linear, even in the sRGB scene attachment.
    var transmittance = 1.;
    if view.forward.w > .5 {
        let distance = t * length(direction);
        let start = select(.57,.73,LANDSCAPES);
        let end = select(.965,.996,LANDSCAPES);
        transmittance -= smoothstep(start,end,distance/far);
    }
    return RollingFragment(vec4f(select(color,stage,(native_view.source.w&1u)!=0u),transmittance), composed_depth);
}
