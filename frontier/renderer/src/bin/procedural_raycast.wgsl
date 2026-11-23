struct View {
    eye: vec4f,
    forward: vec4f,
    right: vec4f,
    up: vec4f,
    grid: vec4i,
    params: vec4u,
    image: vec4u,
    controls: vec4f,
    lattice: array<vec4i,9>,
}
@group(0) @binding(0) var<uniform> view: View;
@group(0) @binding(1) var<storage, read_write> bounds: array<f32>;
@group(0) @binding(2) var<storage, read_write> pixels: array<vec4f>;
@group(0) @binding(3) var<storage, read_write> telemetry: array<vec4u>;

@group(0) @binding(4) var<storage, read_write> gradients: array<u32>;

override INSTRUMENT: bool = false;
override FINE_BOUNDS: bool = false;
override USE_ATLAS: bool = false;
override MODE: u32 = 1u;
override REPAIR: bool = false;
override NUMERICAL: bool = false;
const SIDE: u32 = 256u;
const BLOCK: f32 = 80.;
const MEDIUM: u32 = SIDE*SIDE;
const COARSE: u32 = MEDIUM+64u*64u;
const FINE_OFFSET: u32 = COARSE + 16u*16u;
const INF: f32 = 1.e20;
const PAD: f32 = 0.0015;
const NORMALIZATION: f32 = (1.-0.325*0.325*0.325*0.325*0.325*0.325*0.325*0.325)/(1.-0.325);

// h(s) = center + slope*s + error, with |s| <= 1 and |error| <= radius.
alias Jet = vec3f;
fn constant(x: f32) -> Jet { return vec3(x,0.,0.); }
fn extent(a: Jet) -> f32 { return abs(a.y)+a.z; }
fn range(a: Jet) -> vec2f { return a.x+vec2(-1.,1.)*extent(a); }
fn interval(r: vec2f) -> Jet { return vec3((r.x+r.y)*.5,0.,(r.y-r.x)*.5); }
fn scaled(a: Jet, k: f32) -> Jet { return a * vec3(k,k,abs(k)); }
fn product(a: Jet, b: Jet) -> Jet {
    return vec3(a.x*b.x, a.x*b.y+a.y*b.x,
        abs(a.x)*b.z+abs(b.x)*a.z+extent(a)*extent(b));
}
fn clamped_noise(s: Jet) -> Jet {
    let r = range(s);
    if r.x>=-1. && r.y<=1. { return s; }
    return interval(clamp(r,vec2(-1.),vec2(1.)));
}
fn band_jet(p: vec2f, half_span: vec2f, wave: f32, salt: u32, correlated: bool) -> Jet {
    let delta = half_span/wave;
    let d = dot(abs(delta),vec2(1.));
    if d>=0.45 { return vec3(0.,0.,1.); }
    let point = p/wave+vec2(12.7,-29.1);
    let v = noise_dual(point,salt);
    let slope = dot(v.yz,delta);
    // All Hessian entries of the unclamped noise are bounded by 48.
    // Quintic interpolation is C2 even across lattice boundaries.
    var a = clamped_noise(vec3(v.x,slope,24.*d*d+0.000075));
    if !correlated { a = interval(range(a)); }
    return a;
}
fn bump_jet(a: Jet) -> Jet {
    let r = range(a);
    let nearest = max(0.,max(r.x,-r.y));
    let farthest = max(abs(r.x),abs(r.y));
    let enclosing = interval(vec2(bump(farthest),bump(nearest)));
    if enclosing.z==0. { return enclosing; }
    let x=a.x;
    let derivative=-4.*x*max(0.,1.-x*x);
    // The derivative is zero outside support and its Lipschitz constant is 8.
    let taylor=vec3(bump(x),derivative*a.y,abs(derivative)*a.z+4.*extent(a)*extent(a));
    if extent(taylor)<extent(enclosing) { return taylor; }
    return enclosing;
}
fn hill_jet(p: vec2f, delta: vec2f) -> Jet {
    let lo = max(abs(p)-abs(delta),vec2(0.));
    let hi = abs(p)+abs(delta);
    let enclosing = interval(vec2(hill(hi),hill(lo)));
    if enclosing.z==0. { return enclosing; }
    let slope = -4.*dot(p,delta)*max(0.,1.-dot(p,p));
    let taylor = vec3(hill(p),slope,4.*dot(delta,delta));
    if extent(taylor)<extent(enclosing) { return taylor; }
    return enclosing;
}
struct Base { terrain: Jet, detail_scale: Jet }
fn terrain_base(p: vec2f, delta: vec2f, correlated: bool) -> Base {
    var h=scaled(band_jet(p,delta,750.,0xa101u,correlated),42.)
         +scaled(band_jet(p,delta,240.,0xa102u,correlated),12.);
    var env=constant(0.);
    if p.y+abs(delta.y)>=1059.99 && p.y-abs(delta.y)<=5540.01 {
        let disp=scaled(band_jet(vec2(p.x,0.),vec2(delta.x,0.),1800.,0xa103u,correlated),340.);
        env=bump_jet(scaled(vec3(p.y-3300.,delta.y,0.)-vec3(disp.xy,-disp.z),1./1900.));
    }
    if extent(env)+env.x>0. {
        let pe=vec3(p.x,delta.x,0.);
        let peaks=scaled(bump_jet(scaled(pe+constant(1800.),1./1100.)),650.)
                 +scaled(bump_jet(scaled(pe+constant(650.),1./850.)),470.)
                 +scaled(bump_jet(scaled(pe-constant(1100.),1./1000.)),800.)
                 +scaled(bump_jet(scaled(pe-constant(2600.),1./1000.)),600.);
        let crest=constant(500.)+peaks
                +scaled(band_jet(p,delta,850.,0xa104u,correlated),100.)
                +scaled(band_jet(p,delta,180.,0xa105u,correlated),55.)
                +scaled(band_jet(p,delta,480.,0xa106u,correlated),25.);
        h+=product(env,crest);
    }
    h+=scaled(hill_jet((p-vec2(-900.,1500.))/vec2(1500.,900.),delta/vec2(1500.,900.)),190.)
       +scaled(hill_jet((p-vec2(1550.,2000.))/vec2(1400.,1100.),delta/vec2(1400.,1100.)),290.);
    return Base(h,constant(0.45)+scaled(env,7.5));
}

// Cache scalar upper bounds. The signed diagonal parameter in a Jet is
// discarded here. Use an independent range per coefficient for the whole
// square, including both possible signs of the x/z slope.
fn rectangle_band(p: vec2f, r: vec2f, w: f32, salt: u32) -> vec2f {
    let d=r/w;
    let span=d.x+d.y;
    if span>=0.45 {return vec2(-1.,1.);}
    let v=noise_dual(p/w+vec2(12.7,-29.1),salt);
    let radius=dot(abs(v.yz),d)+24.*span*span+0.000075;
    return clamp(v.x+vec2(-radius,radius),vec2(-1.),vec2(1.));
}
fn rect_bump(a: vec2f) -> vec2f {
    return vec2(bump(max(abs(a.x),abs(a.y))),bump(max(0.,max(a.x,-a.y))));
}
fn rect_hill(p: vec2f,r: vec2f) -> vec2f {
    return vec2(hill(abs(p)+r),hill(max(abs(p)-r,vec2(0.))));
}
fn rect_upper(p: vec2f,r: vec2f) -> f32 {
    let x=p.x+vec2(-r.x,r.x);
    let z=p.y+vec2(-r.y,r.y);
    var h=(42.*rectangle_band(p,r,750.,0xa101u)+12.*rectangle_band(p,r,240.,0xa102u)).y;
    var e=0.;
    if z.y>=1059.99 && z.x<=5540.01 {
        let center=3300.+340.*rectangle_band(vec2(p.x,0.),vec2(r.x,0.),1800.,0xa103u);
        e=rect_bump((z-center.yx)/1900.).y;
    }
    if e>0. {
        let crest=500.+(650.*rect_bump((x+1800.)/1100.)+470.*rect_bump((x+650.)/850.)
           +800.*rect_bump((x-1100.)/1000.)+600.*rect_bump((x-2600.)/1000.)
           +100.*rectangle_band(p,r,850.,0xa104u)+55.*rectangle_band(p,r,180.,0xa105u)
           +25.*rectangle_band(p,r,480.,0xa106u)).y;
        h+=e*crest;
    }
    h+=(190.*rect_hill((p-vec2(-900.,1500.))/vec2(1500.,900.),r/vec2(1500.,900.))
       +290.*rect_hill((p-vec2(1550.,2000.))/vec2(1400.,1100.),r/vec2(1400.,1100.))).y;
    if !FINE_BOUNDS { return h+e*7.5+0.45+0.04+PAD; }
    var detail = 1.;
    {
        // Bound each octave that can contribute a narrower interval than its
        // full [-1,1] range. The omitted higher-frequency weight stays present.
        let span = r.x+r.y;
        var wave = 45.;
        var weight = 1./NORMALIZATION;
        for (var octave=0u; octave<8u; octave+=1u) {
            if span/wave >= .45 { break; }
            detail += weight*(rectangle_band(p,r,wave,0xa110u+octave).y-1.);
            wave *= .5;
            weight *= .325;
        }
    }
    let scale = select(.45, .45+e*7.5, detail >= 0.);
    return h+scale*detail+0.04+PAD;
}
@compute @workgroup_size(8,8)
fn build(@builtin(global_invocation_id) id: vec3u) {
    let p=(vec2f(view.grid.xy+vec2i(id.xy))+.5)*BLOCK;
    bounds[id.y*SIDE+id.x]=rect_upper(p,vec2(BLOCK*.5+.002));
}
@compute @workgroup_size(8,8)
fn build_fine(@builtin(global_invocation_id) id: vec3u) {
    let p=(vec2f(4*view.grid.xy+vec2i(id.xy))+.5)*(BLOCK/4.);
    bounds[FINE_OFFSET+id.y*(SIDE*4u)+id.x]=rect_upper(p,vec2(BLOCK/8.+.003));
}
fn reduce(g:vec2u, size:u32, src:u32, dst:u32) {
    var h=-INF;
    for (var z=0u;z<4u;z+=1u) {
        for(var x=0u;x<4u;x+=1u){
            h=max(h,bounds[src+(g.y*4u+z)*size+g.x*4u+x]);
        }
    }
    bounds[dst+g.y*(size/4u)+g.x]=h;
}
@compute @workgroup_size(8,8)
fn reduce_medium(@builtin(global_invocation_id) id:vec3u){ reduce(id.xy,SIDE,0u,MEDIUM); }
@compute @workgroup_size(8,8)
fn reduce_coarse(@builtin(global_invocation_id) id:vec3u){ reduce(id.xy,64u,MEDIUM,COARSE); }

@compute @workgroup_size(64)
fn fill_atlas(@builtin(global_invocation_id) id:vec3u) {
    let atlas=view.lattice[id.y];
    let i=i32(id.x);
    if i>=atlas.z*atlas.z {return;}
    let c=atlas.xy+vec2(i%atlas.z,i/atlas.z);
    let salt=select(0xa101u+id.y,0xa110u+id.y-6u,id.y>=6u);
    gradients[u32(atlas.w+i)]=hash_corners(c,salt);
}
fn gap_band(p:vec2f, delta:vec2f, gap:Jet, detail:Jet, counts:ptr<function,vec4u>) -> Jet {
    var guess=gap;
    let scale=max(0.,detail.x+extent(detail));
    var remaining=1.;
    var wave=45.;
    var weight=1./NORMALIZATION;
    // In eager mode the exact same interval leaves evaluate every band as a
    // control. In lazy mode an omitted tail can prove above or below before
    // expensive hashes and lattice arithmetic.
    for(var i=0u;i<8u;i+=1u) {
        let uncertainty=scale*remaining+0.04+PAD;
        let rough=range(guess)+vec2(-uncertainty,uncertainty);
        if MODE!=0u && (rough.x>0. || rough.y<0.) { return guess+vec3(0.,0.,uncertainty); }
        let a=band_jet(p,delta,wave,0xa110u+i,MODE!=2u);
        let contribution=scaled(product(detail,a),weight);
        guess-=vec3(contribution.xy,-contribution.z);
        remaining=max(0.,remaining-weight);
        weight*=.325;
        wave*=.5;
        if INSTRUMENT { (*counts).z+=1u; }
    }
    for(var i=0u;i<2u;i+=1u) {
        let uncertainty=select(.04,.01,i==1u)+PAD;
        let rough=range(guess)+vec2(-uncertainty,uncertainty);
        if MODE!=0u && (rough.x>0. || rough.y<0.) { return guess+vec3(0.,0.,uncertainty); }
        let a=scaled(band_jet(p,delta,select(1.2,.75,i==1u),0xa130u+i,MODE!=2u),
                    select(.03,.01,i==1u));
        guess-=vec3(a.xy,-a.z);
        if INSTRUMENT { (*counts).z+=1u; }
    }
    return guess+vec3(0.,0.,PAD);
}
fn gap_at(o:vec3f,d:vec3f,t:f32,counts:ptr<function,vec4u>) -> f32 {
    if INSTRUMENT { (*counts).y+=1u; }
    let p=fma(d,vec3(t),o);
    return p.y-elevation(p.xz,true);
}
fn block_at(p:vec2f,d:vec2f) -> vec2i {
    return vec2i(floor(p/BLOCK+d*.000002));
}
fn clip_interval(o:vec3f,d:vec3f,start:f32,stop:f32,first:ptr<function,f32>,
                 counts:ptr<function,vec4u>) -> f32 {
    var l=start;
    var r=stop;
    var endpoints:array<f32,24>;
    var top=0u;
    let budget=select(192u,1536u,NUMERICAL);
    for(var i=0u;i<budget;i+=1u) {
        let t=(l+r)*.5;
        let half_span=(r-l)*.5;
        if INSTRUMENT { (*counts).x+=1u; }
        let p=fma(d,vec3(t),o);
        let span=d.xz*half_span;
        let base=terrain_base(p.xz,span,MODE!=2u);
        let ray=vec3(p.y,d.y*half_span,0.);
        let gap=ray-vec3(base.terrain.xy,-base.terrain.z);
        let certificate=gap_band(p.xz,span,gap,base.detail_scale,counts);
        let bound=range(certificate);
        let tolerance=select(clamp(t*view.controls.x,.008,4.),
                             clamp(t*view.controls.x*.5,.004,.35),REPAIR || NUMERICAL);
        var min_step=select(tolerance,clamp(t*.0000015,.002,.012),REPAIR);
        if NUMERICAL { min_step=.05; }
        if bound.y<0. || certificate.x-certificate.y < -certificate.z {
            if l-*first>2.*tolerance && *first<INF {return -2.;}
            return l;
        }
        if bound.x<=0. {
            if MODE==3u && certificate.y<0. {
                let l_clip=t+half_span*(certificate.z-certificate.x)/certificate.y;
                let r_clip=t+half_span*(-certificate.z-certificate.x)/certificate.y;
                let start=max(l,l_clip-0.001);
                let stop=min(r,r_clip+0.001);
                // The ray begins above the entire affine enclosure before
                // start, and falls below it beyond stop. Narrow the segment
                // substantially before paying for additional detailed jets.
                if stop>start && stop-start<0.7*(r-l) && r-l>tolerance {
                    l=start;
                    r=stop;
                    continue;
                }
            }
            if r-l<=tolerance {
                if gap_at(o,d,l,counts)<=0. {
                    if l-*first>2.*tolerance && *first<INF {return -2.;}
                    return l;
                }
                if gap_at(o,d,r,counts)<=0. {
                    if r-*first>2.*tolerance && *first<INF {return -2.;}
                    return r;
                }
            }
            if r-l>min_step && top<23u && t>l && t<r {
                endpoints[top]=r;
                top+=1u;
                r=t;
                continue;
            }
            if !NUMERICAL { *first=min(*first,l); }
        }
        if top==0u {return -1.;}
        top-=1u;
        l=r;
        r=endpoints[top];
    }
    return -2.;
}
fn trace(o:vec3f,d:vec3f,start:f32,stop:f32,counts:ptr<function,vec4u>) -> f32 {
    var l=start;
    let sub=select(1,4,FINE_BOUNDS);
    let cell=BLOCK/f32(sub);
    let grid=view.grid.xy*sub;
    let size=SIDE*u32(sub);
    let levels=select(3u,4u,FINE_BOUNDS);
    var block=select(block_at(fma(d.xz,vec2(l),o.xz),d.xz),
        vec2i(floor(fma(d.xz,vec2(l),o.xz)/cell+d.xz*.000002)),FINE_BOUNDS);
    var ambiguous=INF;
    for(var iteration=0u;iteration<384u;iteration+=1u){
        let slot=block-grid;
        if any(slot<vec2i(0)) || any(slot>=vec2i(i32(size))) {return -2.;}
        var advanced=false;
        for(var level=0u;level<levels;level+=1u){
            let shift=2u*(levels-1u-level);
            let span=1i<<shift;
            let region=slot>>vec2u(shift);
            let corner=grid+(region<<vec2u(shift));
            let boundary=corner+select(vec2i(0),vec2i(span),d.xz>vec2(0.));
            let edge=select((vec2f(boundary)*cell-o.xz)/d.xz,vec2(INF),abs(d.xz)<vec2(1.e-10));
            let hi=min(stop,min(edge.x,edge.y));
            let offset=array<u32,4>(COARSE,MEDIUM,0u,FINE_OFFSET)[level];
            let top=bounds[offset+u32(region.y)*(size>>shift)+u32(region.x)];
            if INSTRUMENT { (*counts).w+=1u; }
            let y=min(fma(d.y,l,o.y),fma(d.y,hi,o.y));
            if hi>l && y<=top+PAD {
                if level+1u<levels {continue;}
                let t=clip_interval(o,d,l,hi,&ambiguous,counts);
                if t != -1. {return t;}
            } else if hi<=l && level+1u<levels {continue;}
            if hi>=stop {return select(-2.,-1.,ambiguous==INF);}
            let pos=fma(d.xz,vec2(hi),o.xz);
            let next=select(block_at(pos,d.xz), vec2i(floor(pos/cell+d.xz*.000002)), FINE_BOUNDS);
            block=clamp(next,corner,corner+span-1);
            if edge.x<=edge.y {block.x=boundary.x-select(1,0,d.x>0.);}
            if edge.y<=edge.x {block.y=boundary.y-select(1,0,d.z>0.);}
            l=max(l,hi);
            advanced=true;
            break;
        }
        if !advanced {return -2.;}
    }
    return -2.;
}
fn shade(p:vec3f,d:vec3f,t:f32) -> vec3f {
    let macro_x=0.4;
    let a=elevation(p.xz+vec2(macro_x,0.),false)-elevation(p.xz-vec2(macro_x,0.),false);
    let b=elevation(p.xz+vec2(0.,macro_x),false)-elevation(p.xz-vec2(0.,macro_x),false);
    let macro_normal=normalize(vec3(-a,.8,-b));
    var n=macro_normal;
    if view.params.w!=0u {
        var grad=-macro_normal.xz/macro_normal.y;
        let width=t*view.controls.y / max(.16,abs(dot(d,macro_normal)));
        let displacement=7.5*bump((p.z-(3300.+340.*band(vec2(p.x,0.),1800.,0xa103u)))/1900.)+.45;
        var weight=displacement/NORMALIZATION;
        var wave=45.;
        for(var i=0u;i<8u;i+=1u) {
            let f=1.-smoothstep(.25,.75,width/wave);
            if f<.0001 { break; }
            let v=noise_dual(p.xz/wave+vec2(12.7,-29.1),0xa110u+i);
            if abs(v.x)<1. { grad+=v.yz*(weight*f/wave); }
            wave*=.5;
            weight*=.325;
        }
        for(var i=0u;i<2u;i+=1u) {
            let w=select(1.2,.75,i==1u);
            let f=1.-smoothstep(.25,.75,width/w);
            if f>0. {
                let v=noise_dual(p.xz/w+vec2(12.7,-29.1),0xa130u+i);
                if abs(v.x)<1. {grad+=v.yz*(select(.03,.01,i==1u)*f/w);}
            }
        }
        n=normalize(vec3(-grad.x,1.,-grad.y));
    }
    let rough=length(macro_normal.xz)/max(.025,macro_normal.y);
    let noise_cover=.75*noise(p.xz/120.+vec2(3.7,-8.1),0xac01u)
                   +.25*noise(p.xz/28.+vec2(-17.2,4.8),0xac02u);
    let stone=smoothstep(.52,.77,rough+.13*noise_cover);
    let snow=smoothstep(1160.,1320.,p.y)*(1.-smoothstep(.6,1.4,rough));
    var pigment=mix(vec3(.18,.34,.10)*(1.+.23*noise_cover),vec3(.35,.37,.39),stone);
    pigment=mix(pigment,vec3(.74,.78,.83),snow);
    let trail=16.+60.*sin(p.z/310.)+.055*p.z;
    if p.z>-550. && p.z<1500. && abs(p.x-trail)<.55 {
        pigment=vec3(.32,.22,.13);
    }
    let sun=normalize(vec3(-.54,.66,-.52));
    let light=.39+.16*max(n.y,0.)+.52*max(dot(n,sun),0.);
    let haze=1.-exp(-t*view.controls.z);
    return mix(pigment*light,vec3(.37,.52,.67),haze);
}
fn store_pixel(index:u32, d:vec3f, t:f32, prediction_kind:f32) {
    var rgb=mix(vec3(.59,.70,.83),vec3(.23,.44,.73),smoothstep(0.,.65,max(d.y,0.)));
    if t>=0. {rgb=vec3(.5); if view.params.w!=2u {rgb=shade(fma(d,vec3(t),view.eye.xyz),d,t);}}
    if t==-2. {rgb=vec3(1.,0.,1.);}
    var stored=t;
    if NUMERICAL && t!=-2. {stored=select(-3.,t+32768.,t>=0.);}
    // Keep the exact floating depth. The color sign bits tag preview data and
    // are stripped by the PNG writer.
    if prediction_kind>0. {rgb.r=-rgb.r;}
    if prediction_kind==2. {rgb.g=-rgb.g;}
    pixels[index]=vec4(rgb,stored);
}

fn resolve_pixel(ij:vec2u) {
    let width=view.image.x;
    let index=ij.y*width+ij.x;
    if (REPAIR || NUMERICAL) && pixels[index].w != -2. {return;}
    let d=pixel_ray(ij);
    var counts=vec4u(0);
    let forward=dot(d,view.forward.xyz);
    var start=.025;
    var sky=false;
    if USE_PACKETS {
        let packet=packets[packet_index(ij/PACKET_EDGE)];
        start=packet.x;
        sky=packet.w==1.;
    }
    var t=-1.;
    var prediction_kind=0.;
    if !sky {
        if SPARSE_MODE!=0u && !COMPACT && !REPAIR && !NUMERICAL {
            let preview=preview_intersection(ij,d,forward,start,&counts);
            if preview.y==2. || SPARSE_MODE==2u {
                t=preview.x;
                prediction_kind=preview.y;
            }
        }
        if prediction_kind==0. {
            var lower=.025/forward;
            if USE_PACKETS {
                lower=aligned_packet_start(view.eye.xyz,d,start/forward,lower);
            }
            t=trace(view.eye.xyz,d,lower,view.eye.w/forward,&counts);
        }
    }
    store_pixel(index,d,t,prediction_kind);
    if INSTRUMENT {
        if REPAIR || NUMERICAL || COMPACT {counts+=telemetry[index];}
        telemetry[index]=counts;
    }
}

@compute @workgroup_size(8,8)
fn render(@builtin(global_invocation_id) id:vec3u) {
    let ij=id.xy+view.image.zw;
    if id.x>=u32(view.controls.w) || ij.x>=view.image.x || ij.y>=view.image.y {return;}
    resolve_pixel(ij);
}
