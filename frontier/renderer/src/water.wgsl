// Water surface optics in linear light. Distances are in metres.
fn water_ripple(p:vec3<f32>,normal:vec3<f32>,time:f32,amplitude:f32)->vec3<f32> {
    // Integer harmonics of a 128 m repeat let the CPU rebase the phase
    // without jumping when the native snapshot changes its world origin.
    let wave=0.049087385;
    let warp=sin(dot(p.xz,vec2(6.,-11.)*wave)+time*0.34)*1.8;
    let a=dot(p.xz,vec2(79.,35.)*wave)+time*1.8+warp;
    let b=dot(p.xz,vec2(-43.,100.)*wave)-time*1.45-warp*0.8;
    let c=dot(p.xz,vec2(147.,-88.)*wave)+time*2.1+warp*1.2;
    let slope=vec2(sin(a)+0.50*sin(b)+0.19*sin(c),
        0.48*sin(a)-sin(b)-0.21*sin(c));
    return normalize(normal+vec3(slope.x,0.,slope.y)*amplitude*abs(normal.y));
}

fn water_fresnel(cosine:f32)->f32 {
    let edge=1.-clamp(cosine,0.,1.);
    let square=edge*edge;
    return 0.0204+0.9796*square*square*edge;
}

fn water_medium(color:vec3<f32>,distance:f32)->vec3<f32> {
    let transmission=exp(-vec3(0.88,0.43,0.39)*distance);
    return mix(vec3(0.013,0.063,0.056),color,transmission);
}

fn water_sky(ray:vec3<f32>,sun:vec3<f32>,overcast:f32)->vec3<f32> {
    let dusk=1.-smoothstep(0.05,0.5,sun.y);
    let horizon=mix(vec3(0.37,0.52,0.63),vec3(0.65,0.45,0.33),dusk*0.6);
    let zenith=vec3(0.09,0.27,0.50);
    let sky=mix(horizon,zenith,sqrt(clamp(ray.y,0.,1.)));
    return mix(sky,vec3(0.39,0.45,0.49),overcast*0.65);
}

fn water_glint(direction:vec3<f32>,normal:vec3<f32>,sun:vec3<f32>,overcast:f32)->vec3<f32> {
    let alignment=max(dot(reflect(direction,normal),sun),0.);
    let glint=pow(alignment,95.)*0.17+pow(alignment,420.)*0.37;
    return vec3(1.,0.82,0.57)*glint*(1.-0.9*overcast);
}

// Metre-scale slope bands for distant water. All harmonics repeat at 2048 m
// so a rebased camera retains phase. Unresolved slopes fade with the footprint.
fn water_swell(p:vec2<f32>,normal:vec3<f32>,time:f32,pixel:f32)->vec3<f32> {
    let phase=p*(6.283185307/2048.);
    let warp=sin(dot(phase,vec2(8.,-11.))+.18*time)*1.4
        +.65*sin(dot(phase,vec2(5.,14.))-.12*time);
    let packet=.59+.23*sin(dot(phase,vec2(9.,4.))+.16*time)
        +.18*sin(dot(phase,vec2(-4.,13.))-.23*time);
    let a=sin(dot(phase,vec2(38.,17.))+.9*time+warp)*(1.-smoothstep(7.,22.,pixel));
    let b=sin(dot(phase,vec2(-31.,69.))-.74*time-warp)*(1.-smoothstep(4.,12.,pixel));
    let c=sin(dot(phase,vec2(99.,-71.))+1.22*time+.6*warp)*(1.-smoothstep(2.,7.,pixel));
    let slope=packet*vec2(.024*a-.016*b+.009*c,.011*a+.027*b-.007*c);
    return normalize(normal+vec3(slope.x,0.,slope.y)*normal.y);
}

// One world-anchored slope field for the exact water volume and its raster
// continuation. Only the optical normal moves; the voxel roof stays level.
fn water_surface_normal(p:vec2<f32>,normal:vec3<f32>,time:f32,pixel:f32,distance:f32,course:vec4<f32>)->vec3<f32> {
    if normal.y<.5 {return normal;}
    let swell=water_swell(p,normal,time,pixel);
    let detail=.020*(1.-smoothstep(.08,.38,pixel))*(1.-smoothstep(22.,78.,distance));
    let phase=p-128.*floor(p/128.);
    var wind=swell;
    if detail>0. {wind=water_ripple(vec3(phase.x,0.,phase.y),swell,time,detail);}
    if course.w<=0. {return wind;}
    // Counter-propagating wavelets have fixed world phase. Changes in course
    // steer their amplitudes, so neighbouring reaches agree even late in a run.
    let q=phase*.049087385;
    let eddy=sin(dot(q,vec2(6.,-8.))+.34*time)+.64*sin(dot(q,vec2(11.,3.))-.25*time);
    let waves=vec4(dot(q,vec2(43.,9.)),dot(q,vec2(-11.,49.)),
        dot(q,vec2(57.,52.)),dot(q,vec2(64.,-51.)))+eddy*vec4(1.8,-1.1,.9,-1.6);
    let ax=vec4(.979,-.219,.739,.782);
    let az=vec4(.205,.976,.674,-.623);
    let alignment=ax*course.x+az*course.y;
    let speed=time*vec4(1.96,2.15,3.8,4.2);
    let running=mix(sin(waves+speed),sin(waves-speed),smoothstep(vec4(-.32),vec4(.32),alignment));
    let packet=.77+.23*sin(dot(q,vec2(13.,-8.))+.27*time+.8*eddy);
    let resolved=1.-smoothstep(vec4(.25,.23,.14,.13),vec4(1.10,1.,.64,.60),vec4(pixel));
    let bands=running*(.12+.88*alignment*alignment)*resolved;
    let slope=vec2(dot(ax,bands),dot(az,bands))*packet*mix(.014,.047,course.z);
    let river=normalize(normal+vec3(slope.x,0.,slope.y));
    return normalize(mix(wind,river,course.w));
}

// Below one pixel, a single wet-to-wet riser and its adjacent water roof
// share a sample. Filter their optical response while retaining the face.
fn stream_step_normal(side:vec3<f32>, height:f32, pixel_height:f32)->vec3<f32> {
    let span=height/max(pixel_height,.000001);
    let roof=.92*(1.-smoothstep(.6,3.3,span));
    return normalize(mix(side,vec3(0.,1.,0.),roof));
}

// Seamless value noise with a power-of-two cell count along each side of
// the shared 512 m water origin. Evaluate all four corners together.
fn foam_noise(p:vec2<f32>,period:u32)->f32 {
    let i=bitcast<vec2<u32>>(vec2<i32>(floor(p)));
    let x=(vec4(i.x)+vec4(0u,1u,0u,1u))&vec4(period-1u);
    let z=(vec4(i.y)+vec4(0u,0u,1u,1u))&vec4(period-1u);
    var h=x*0x8da6b343u ^ z*0xd8163841u;
    h=(h^(h>>vec4(16u)))*0x7feb352du;
    let v=vec4<f32>((h^(h>>vec4(15u)))&vec4(65535u))/65535.;
    let f=fract(p);
    let w=f*f*(3.-2.*f);
    return mix(mix(v.x,v.y,w.x),mix(v.z,v.w,w.x),w.y);
}
// A steep reach contains patches of entrained air. Current-aligned patches
// stretch downstream and break into smaller flecks as the footprint shrinks.
fn rapid_foam(p:vec2<f32>,time:f32,pixel:f32,strength:f32,course:vec2<f32>)->f32 {
    if strength<.001 {return 0.;}
    let world=p-512.*floor(p/512.);
    let pool=foam_noise(world*.125,64u);
    let turbulence=smoothstep(.24,.77,pool);
    let mean=strength*mix(.02,.28,turbulence);
    let resolve=1.-smoothstep(.15,1.25,pixel);
    if resolve<=0. {return mean;}
    let angle=world*.0122718463;
    let bend=vec2(sin(dot(angle,vec2(43.,-39.))+time*.46),
                  cos(dot(angle,vec2(31.,56.))-time*.37));
    let q=(world+.34*bend-.54*course*sin(time*1.4+pool*1.8))*.5;
    let a=abs(course);
    let diagonal=smoothstep(.12,.84,min(a.x,a.y)/max(.001,max(a.x,a.y)));
    let cardinal=select(q.yx,q,a.x>=a.y)*vec2(1.,4.);
    let across=select(-q.y,q.y,course.x*course.y>=0.);
    let slant=vec2(q.x+across,3.*(q.x-across));
    let streak=mix(foam_noise(cardinal,256u),foam_noise(slant,256u),diagonal);
    let churn=foam_noise(q*5.+vec2(7.3,-2.9),256u);
    let bubbles=foam_noise(q*15.+vec2(3.7,5.1),256u);
    let density=.51*streak+.31*churn+.18*mix(bubbles,.5,smoothstep(.035,.16,pixel));
    let threshold=mix(.76,.43,turbulence);
    let spray=smoothstep(threshold-.055,threshold+.15,density);
    return mix(mean,.74*strength*spray,resolve);
}
fn foam_color(lit:vec3<f32>,light:vec3<f32>,amount:f32)->vec3<f32> {
    return mix(lit,vec3(.72,.83,.80)*light,amount);
}
