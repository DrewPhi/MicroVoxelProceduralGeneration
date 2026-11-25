// Greenfield five-tap contrast-adaptive spatial comparison, inspired by the
// contrast gate/subpixel rationale of Lottes' FXAA white paper. This is NOT
// full FXAA: no edge endpoint search. Strength defaults to zero in Rust.
// It cannot recover undersampled geometry; compare loss of voxel contrast.
fn aa_luma(c:vec3<f32>)->f32 { return dot(c,vec3(.2126,.7152,.0722)); }
fn edge_post_aa(pixel:vec2<i32>)->vec3<f32> {
    let size=vec2<i32>(textureDimensions(scene));
    let center=aa_color(pixel);
    let n=aa_color(clamp(pixel+vec2(0,-1),vec2(0),size-vec2(1)));
    let s=aa_color(clamp(pixel+vec2(0,1),vec2(0),size-vec2(1)));
    let e=aa_color(clamp(pixel+vec2(1,0),vec2(0),size-vec2(1)));
    let w=aa_color(clamp(pixel+vec2(-1,0),vec2(0),size-vec2(1)));
    let lm=aa_luma(center);
    let ln=aa_luma(n);let ls=aa_luma(s);let le=aa_luma(e);let lw=aa_luma(w);
    let low=min(lm,min(min(ln,ls),min(le,lw)));
    let high=max(lm,max(max(ln,ls),max(le,lw)));
    let contrast=high-low;
    if contrast<max(.03125,high*.125) { return center; }
    // Only isolated/subpixel contrast gets a small low-pass contribution.
    // Coherent broad gradients remain unchanged; a full-strength maximum is
    // capped at one quarter to keep the comparison useful for voxel detail.
    let average=(n+s+e+w)*.25;
    let isolation=clamp(abs(aa_luma(average)-lm)/max(contrast,.00001),0.,1.);
    let amount=atmosphere.beauty.w*.25*isolation*isolation;
    return mix(center,average,amount);
}
