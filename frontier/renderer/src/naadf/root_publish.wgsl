// The only writable resource in this pass is the exact root texture.
// Occupied words (including child pointers/materials) stay untouched.
// Empty words are monotone expansions of the seed proof, never geometry.
struct Tile { root:vec4<u32>, ring:vec4<u32>, base:vec4<u32> };
@group(0) @binding(0) var<storage,read> proof:array<u32>;
@group(0) @binding(1) var<uniform> tile:Tile;
@group(0) @binding(2) var roots:texture_storage_3d<r32uint,write>;
@group(0) @binding(3) var<storage,read> original:array<u32>;
// CPU roots use 00=air, 01=full or 10=child. Reserved 11 records that a
// GPU proof actually changed this empty root; strip uploads clear it naturally.
const REPAIRED:u32=0xc0000000u;
const SIDE:u32=32u;
@compute @workgroup_size(4,4,4)
fn publish_roots(@builtin(global_invocation_id) p:vec3<u32>) {
    let logical=bitcast<vec3<i32>>(tile.base.xyz)+vec3<i32>(p);
    if any(logical<vec3(0)) || any(logical>=vec3(i32(tile.root.x))) {return;}
    let index=p.x+SIDE*(p.y+SIDE*p.z);
    let word=proof[index];
    if (word&REPAIRED)!=0u {return;}
    let before=original[index];
    let tagged=(before&REPAIRED)==REPAIRED;
    if !tagged && (before&REPAIRED)!=0u {return;}
    if word==(before&~REPAIRED) {return;} // Preserve both plain and tagged unchanged words.
    let slot=(vec3<u32>(logical)+tile.ring.xyz)%vec3(tile.root.x);
    textureStore(roots,vec3<i32>(slot.yxz),vec4(word|REPAIRED,0u,0u,0u));
}
