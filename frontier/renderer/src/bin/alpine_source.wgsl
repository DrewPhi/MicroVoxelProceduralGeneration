// Selective port of the prior Frontier tracer's scalar Alpine source and noise.
fn high_product(a: u32, b: u32) -> u32 {
    let a0 = a & 65535u;
    let a1 = a >> 16u;
    let b0 = b & 65535u;
    let b1 = b >> 16u;
    let u = a0*b0;
    let t = a1*b0 + (u >> 16u);
    let v = a0*b1 + (t & 65535u);
    return a1*b1 + (t >> 16u) + (v >> 16u);
}
fn mul64(a: vec2<u32>, b: vec2<u32>) -> vec2<u32> {
    return vec2(a.x*b.x, high_product(a.x,b.x) + a.x*b.y + a.y*b.x);
}
fn shr64(a: vec2<u32>, n: u32) -> vec2<u32> {
    return vec2((a.x >> n) | (a.y << (32u-n)), a.y >> n);
}
fn signed64(x: i32) -> vec2<u32> {
    return vec2(bitcast<u32>(x), select(0u, 0xffffffffu, x < 0));
}
fn gradient_code(cell: vec2<i32>, salt: u32) -> u32 {
    var h = view.params.xy ^ vec2(salt,0u)
        ^ mul64(signed64(cell.x), vec2(0x7f4a7c15u,0x9e3779b9u))
        ^ mul64(signed64(cell.y), vec2(0x1ce4e5b9u,0xbf58476du));
    h = mul64(h ^ shr64(h,30u), vec2(0x1ce4e5b9u,0xbf58476du));
    h = mul64(h ^ shr64(h,27u), vec2(0x133111ebu,0x94d049bbu));
    return (h ^ shr64(h,31u)).x & 7u;
}
fn decode_gradient(k: u32) -> vec2<f32> {
    let s = 0.7071067811865476;
    let gradients = array<vec2<f32>,8>(vec2(1.,0.), vec2(-1.,0.), vec2(0.,1.), vec2(0.,-1.),
                          vec2(s,s), vec2(s,-s), vec2(-s,s), vec2(-s,-s));
    return gradients[k];
}
fn hash_corners(c: vec2<i32>, salt: u32) -> u32 {
    return gradient_code(c,salt)
        | (gradient_code(c+vec2(1,0),salt)<<3u)
        | (gradient_code(c+vec2(0,1),salt)<<6u)
        | (gradient_code(c+vec2(1,1),salt)<<9u);
}
fn cached_corners(c:vec2i,salt:u32)->u32 {
    if USE_ATLAS {
        let index=select(
            select(9u,salt-0xa110u+6u,salt>=0xa110u && salt<=0xa112u),
            salt-0xa101u,salt>=0xa101u && salt<=0xa106u);
        if index<9u {
            let atlas=view.lattice[index];
            let p=c-atlas.xy;
            if all(p>=vec2i(0)) && all(p<vec2i(atlas.z)) {
                return gradients[u32(atlas.w+p.y*atlas.z+p.x)];
            }
        }
    }
    return hash_corners(c,salt);
}
fn fade(t: vec2<f32>) -> vec2<f32> {
    return t*t*t*(t*(t*6.0-15.0)+10.0);
}
fn noise(p: vec2<f32>, salt: u32) -> f32 {
    let c = vec2<i32>(floor(p));
    let f = fract(p);
    let w = fade(f);
    let code = cached_corners(c,salt);
    let a = dot(decode_gradient(code&7u), f);
    let b = dot(decode_gradient((code>>3u)&7u), f-vec2(1.,0.));
    let d = dot(decode_gradient((code>>6u)&7u), f-vec2(0.,1.));
    let e = dot(decode_gradient((code>>9u)&7u), f-vec2(1.));
    return clamp(1.6*mix(mix(a,b,w.x),mix(d,e,w.x),w.y),-1.,1.);
}
// Value and analytic lattice derivatives of the unclamped gradient noise.
fn noise_dual(p: vec2<f32>, salt: u32) -> vec3<f32> {
    let c = vec2<i32>(floor(p));
    let f = fract(p);
    let w = fade(f);
    let dw = 30.*f*f*(f-1.)*(f-1.);
    let code = cached_corners(c,salt);
    let ga = decode_gradient(code&7u);
    let gb = decode_gradient((code>>3u)&7u);
    let gd = decode_gradient((code>>6u)&7u);
    let ge = decode_gradient((code>>9u)&7u);
    let a = dot(ga, f);
    let b = dot(gb, f-vec2(1.,0.));
    let d = dot(gd, f-vec2(0.,1.));
    let e = dot(ge, f-vec2(1.));
    let low = mix(a,b,w.x);
    let high = mix(d,e,w.x);
    let grad = mix(mix(ga,gb,w.x),mix(gd,ge,w.x),w.y);
    return 1.6*vec3(mix(low,high,w.y),grad + vec2(
        dw.x*mix(b-a,e-d,w.y),dw.y*(high-low)));
}
fn band(p: vec2<f32>, wavelength: f32, salt: u32) -> f32 {
    return noise(p/wavelength + vec2(12.7,-29.1), salt);
}
fn bump(d: f32) -> f32 {
    let q = clamp(1.-d*d,0.,1.);
    return q*q;
}
fn hill(p: vec2<f32>) -> f32 {
    let q = clamp(1.-dot(p,p),0.,1.);
    return q*q;
}
fn elevation(p: vec2<f32>, detail: bool) -> f32 {
    let meadow = 42.*band(p,750.,0xa101u) + 12.*band(p,240.,0xa102u);
    var env = 0.;
    var h = meadow;
    if p.y >= 1060. && p.y <= 5540. {
        let center = 3300. + 340.*band(vec2(p.x,0.),1800.,0xa103u);
        env = bump((p.y-center)/1900.);
        if env > 0. {
            let peaks = 650.*bump((p.x+1800.)/1100.) + 470.*bump((p.x+650.)/850.)
                        + 800.*bump((p.x-1100.)/1000.) + 600.*bump((p.x-2600.)/1000.);
            let crest = 500. + peaks + 100.*band(p,850.,0xa104u)
                        + 55.*band(p,180.,0xa105u) + 25.*band(p,480.,0xa106u);
            h += env*crest;
        }
    }
    h += 190.*hill((p-vec2(-900.,1500.))/vec2(1500.,900.))
       + 290.*hill((p-vec2(1550.,2000.))/vec2(1400.,1100.));
    if detail {
        var sum = 0.;
        var norm = 0.;
        var weight = 1.;
        var wavelength = 45.;
        for (var i = 0u; i < 8u; i += 1u) {
            sum += weight*band(p,wavelength,0xa110u+i);
            norm += weight;
            weight *= 0.65*0.5;
            wavelength *= 0.5;
        }
        h += sum/norm*(env*7.5+0.45);
        h += 0.03*band(p,1.2,0xa130u) + 0.01*band(p,0.75,0xa131u);
    }
    return h;
}
