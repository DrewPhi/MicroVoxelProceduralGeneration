// Controlled local vegetation experiment: same native cells and lighting for
// raster, exact 4^3 sparse bricks and display-only aggregate coverage.
struct Params {
    inverse:mat4x4<f32>, matrix:mat4x4<f32>, eye:vec4<f32>,
    low:vec4<f32>, high:vec4<f32>, grid:vec4<u32>, image:vec4<u32>,
};
struct Header { low:u32, high:u32, base:u32, pad:u32 };
struct Pixel { color:vec4<f32>, info:vec4<f32> };
@group(0) @binding(0) var<uniform> p:Params;
@group(0) @binding(1) var<storage,read> headers:array<Header>;
@group(0) @binding(2) var<storage,read> cells:array<vec4<f32>>;
@group(0) @binding(3) var<storage,read_write> pixels:array<Pixel>;
fn lighting(n:vec3<f32>)->vec3<f32> {
    return vec3(0.22,0.26,0.30)+vec3(0.8,0.74,0.6)*max(0.,dot(n,normalize(vec3(-0.5,0.8,0.3))));
}
fn background()->vec3<f32> {return vec3(0.34,0.57,0.8);}
fn cell_value(c:vec3<i32>)->vec4<f32> {
    if any(c<vec3(0)) || any(c>=vec3<i32>(p.grid.xyz*4u)) {return vec4(0.);}
    let b=vec3<u32>(c)/4u;
    let h=headers[b.x+p.grid.x*(b.y+p.grid.y*b.z)];
    let q=vec3<u32>(c)%4u;let slot=q.x*16u+q.y*4u+q.z;
    var mask=h.low;var bit=slot;
    var rank=0u;
    if slot>=32u {mask=h.high;bit=slot-32u;rank=countOneBits(h.low);}
    if (mask&(1u<<bit))==0u {return vec4(0.);}
    rank+=countOneBits(mask&((1u<<bit)-1u));
    return cells[h.base+rank];
}
fn trace(ro:vec3<f32>,rd:vec3<f32>)->Pixel {
    let safe=select(vec3(-1e-20),vec3(1e-20),rd>=vec3(0.));
    let direction=select(rd,safe,abs(rd)<vec3(1e-20));
    let a=(p.low.xyz-ro)/direction;let b=(p.high.xyz-ro)/direction;
    let lo=min(a,b);let hi=max(a,b);let enter=max(max(lo.x,lo.y),lo.z);let exit=min(min(hi.x,hi.y),hi.z);
    var result:Pixel;result.color=vec4(background(),0.);result.info=vec4(1.,0.,0.5,0.);
    if exit<=max(enter,0.) {return result;}
    let step=f32(p.image.w)*0.0625;
    var t=max(0.,enter);var normal=vec3(0.);
    // Track the integer DDA address explicitly. Reconstructing it from t+epsilon
    // can round back onto an exit face and terminate a shallow ray prematurely.
    let dimensions=vec3<i32>(p.grid.xyz*4u);
    var c=clamp(vec3<i32>(floor((ro+rd*t-p.low.xyz)/step)),vec3(0),dimensions-vec3(1));
    if lo.x>=lo.y && lo.x>=lo.z {normal.x=-sign(direction.x);}else if lo.y>=lo.z {normal.y=-sign(direction.y);}else {normal.z=-sign(direction.z);}
    var trans=1.;var color=vec3(0.);var first=1.;var hits=0.;var work=0.;var first_edge=0.5;
    for(var iter=0u;iter<8192u;iter++) {
        if t>exit || trans<0.005 {break;}
        work+=1.;

        if any(c<vec3(0)) || any(c>=vec3<i32>(p.grid.xyz*4u)) {break;}
        let value=cell_value(c);
        if value.w>0. {
            let clip=p.matrix*vec4(ro+rd*t,1.);
            if hits==0. {
                first=clip.z/clip.w;
                // Physical native-cell edge clearance, excluding the face's
                // normal axis (which is always on an integer grid plane).
                let native_position=(ro+rd*t)*16.;
                let clearance=abs(native_position-round(native_position));
                for(var d=0u;d<3u;d++) {
                    if abs(normal[d])<0.5 {first_edge=min(first_edge,clearance[d]);}
                }
            }
            let alpha=select(1.,value.w,p.image.z==2u);
            color+=trans*alpha*value.xyz*lighting(normal);trans*=1.-alpha;hits+=1.;
            if p.image.z!=2u {break;}
        }
        var span=1;
        // Skip only proven-empty bricks, not sampled heuristic distances.
        let bb=vec3<u32>(c)/4u;let head=headers[bb.x+p.grid.x*(bb.y+p.grid.y*bb.z)];
        if (head.low|head.high)==0u {span=4;}
        let block=c/vec3(span);
        let edge=p.low.xyz+vec3<f32>((block+select(vec3(0),vec3(1),direction>vec3(0.)))*span)*step;
        let times=(edge-ro)/direction;
        var next=times.x;var axis=0u;
        if times.y<next {next=times.y;axis=1u;}if times.z<next {next=times.z;axis=2u;}
        if next<t {break;}
        let q=(ro+rd*next-p.low.xyz)/step;
        var next_cell=vec3<i32>(floor(q));
        // Tied planes advance together. The tolerance is tiny compared with a
        // native cell; raster parity separately excludes numerical edge rays.
        let tolerance=max(0.000001,abs(next)*0.0000001);
        for(var d=0u;d<3u;d++) {
            if abs(times[d]-next)<=tolerance {
                next_cell[d]=select(block[d]*span-1,(block[d]+1)*span,direction[d]>0.);
            }
        }
        c=next_cell;t=next;normal=vec3(0.);normal[axis]=-sign(direction[axis]);
    }
    result.color=vec4(color+trans*background(),1.-trans);result.info=vec4(first,work,first_edge,hits);return result;
}
@compute @workgroup_size(8,8) fn trace_image(@builtin(global_invocation_id) id:vec3<u32>) {
    if id.x>=p.image.x || id.y>=p.image.y {return;}
    var sum=vec4(0.);var info=vec4(0.);let spp=p.grid.w;
    for(var sample=0u;sample<spp;sample++) {
        let n=u32(sqrt(f32(spp)));let jitter=(vec2<f32>(vec2<u32>(sample%n,sample/n))+0.5)/f32(n);
        let ndc=(vec2<f32>(id.xy)+jitter)/vec2<f32>(p.image.xy)*vec2(2.,-2.)+vec2(-1.,1.);
        // The raster matrix defines two screen-coordinate planes through the
        // optical center. Their intersection gives the same perspective ray
        // without inverse-VP near-plane cancellation (near is only0.025m).
        let row_x=vec3(p.matrix[0][0],p.matrix[1][0],p.matrix[2][0]);
        let row_y=vec3(p.matrix[0][1],p.matrix[1][1],p.matrix[2][1]);
        let row_w=vec3(p.matrix[0][3],p.matrix[1][3],p.matrix[2][3]);
        let plane_x=row_x-ndc.x*row_w;
        let plane_y=row_y-ndc.y*row_w;
        let direction=normalize(cross(plane_y,plane_x));
        let hit=trace(p.eye.xyz,direction);sum+=hit.color;info+=hit.info;
    }
    pixels[id.x+p.image.x*id.y]=Pixel(sum/f32(spp),info/f32(spp));
}
struct Vout {@builtin(position) pos:vec4<f32>,@location(0) color:vec3<f32>,@location(1) normal:vec3<f32>};
@vertex fn vs(@location(0) pos:vec3<f32>,@location(1) normal:vec3<f32>,@location(2) color:vec3<f32>)->Vout {
    var o:Vout;o.pos=p.matrix*vec4(pos,1.);o.color=color;o.normal=normal;return o;
}
@fragment fn fs(i:Vout)->@location(0) vec4<f32> {return vec4(i.color*lighting(i.normal),1.);}
