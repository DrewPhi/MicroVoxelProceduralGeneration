@group(0) @binding(11) var cloud_shape:texture_3d<f32>;
@group(0) @binding(12) var cloud_repeat:sampler;

const CLOUD_BASE:f32=1720.;
const THERMAL_CLOUD_DETAIL:bool=false;

fn cloud_thickness()->f32 {
    return select(680.,1040.,atmosphere.sun.w>.5);
}

fn cloud_texture(position:vec3f)->vec4f {
    return textureSampleLevel(cloud_shape,cloud_repeat,position,0.);
}

fn hash3(p:vec3<f32>)->f32 {
    var q=fract(p*vec3(.1031,.1030,.0973));
    q+=dot(q,q.yxz+vec3(33.33));return fract((q.x+q.y)*q.z);
}
fn noise3(p:vec3<f32>)->f32 {
    let i=floor(p);let f=fract(p);let u=f*f*f*(f*(f*6.-15.)+10.);
    let a=mix(mix(hash3(i),hash3(i+vec3(1.,0.,0.)),u.x),mix(hash3(i+vec3(0.,1.,0.)),hash3(i+vec3(1.,1.,0.)),u.x),u.y);
    let b=mix(mix(hash3(i+vec3(0.,0.,1.)),hash3(i+vec3(1.,0.,1.)),u.x),mix(hash3(i+vec3(0.,1.,1.)),hash3(i+vec3(1.,1.,1.)),u.x),u.y);
    return mix(a,b,u.z);
}
fn density(p:vec3<f32>, octave_weights:vec2f)->f32 {
    let height=(p.y-CLOUD_BASE)/cloud_thickness();
    let wind=vec3(1.,0.,.32)*atmosphere.weather.w*atmosphere.weather.y;
    if atmosphere.sun.w>.5 {
        if height<=0. || height>=1. {return 0.;}
        // A horizontally continuous moisture field fixes each cloud base and
        // its lifting depth. The wind advects that base while the tops lean
        // into increasing directional shear.
        let horizontal=p.xz*vec2f(.000080,.000087)+wind.xz*.14;
        let phase=atmosphere.cloud_phase.xyz;
        let weather_band=cloud_texture(vec3f(horizontal.x*.36,.31,horizontal.y*.36)+phase);
        let column=cloud_texture(vec3f(horizontal.x,.27,horizontal.y)+phase);
        let moisture=mix(.45,.69*column.r+.31*weather_band.r,octave_weights.x);
        let threshold=mix(.779,.294,atmosphere.weather.x);
        let buoyancy=smoothstep(threshold-.16,threshold+.15,moisture);
        let shear=height*(.027+.053*height);
        if THERMAL_CLOUD_DETAIL {
            // Cumulus grows from the advected moisture column. Give the 3D
            // billows approximately the same wavelength in each axis, and
            // concentrate smaller eroded lobes toward the upper boundary.
            let axis=horizontal*1.57+vec2f(shear,-.49*shear);
            let q=vec3f(axis.x,.141*height+.21,axis.y)+phase;
            let shape=cloud_texture(q);
            let billows=mix(shape.r-.49,shape.g-.44,.72)*octave_weights.y;
            let pocket=(.48-.80*shape.b-.20*shape.a)*octave_weights.y;
            let depth=clamp(.13+.85*buoyancy+.39*billows-.21*pocket,.14,1.);
            let base=.015+.10*(1.-weather_band.r);
            let column=smoothstep(base,base+.092,height);
            let top=1.-smoothstep(.78*depth,depth,height);
            let lifting=smoothstep(.12,.77,height);
            let body=moisture+(.32+.46*lifting)*billows-(.16+.29*lifting)*pocket;
            return smoothstep(threshold-.061,threshold+.070,body)*column*top;
        }
        let q=vec3f(horizontal.x+shear,.34*height+.21,horizontal.y-.49*shear)+phase;
        let shape=cloud_texture(q);
        let bulges=(shape.g-.47)*octave_weights.y;
        let pocket=(.52-.86*shape.b-.14*shape.a)*octave_weights.y;
        let crown=clamp(.14+.82*buoyancy+.26*bulges-.29*pocket,.13,1.);
        let base=.02+.11*(1.-weather_band.r);
        let envelope=smoothstep(base,base+.083,height)*(1.-smoothstep(.77*crown,crown,height));
        let body=moisture+(.39+.26*height)*bulges-(.16+.23*height)*pocket;
        return smoothstep(threshold-.057,threshold+.069,body)*envelope;
    }
    // Broad individual cloud bodies leave stretches of open sky. The second
    // frequency erodes their edges while the base stays close to level.
    let q=p*vec3(.00110,.00195,.00110)+wind;
    var body=.5;
    var erosion=.5;
    if octave_weights.x>0. {body=mix(.5,noise3(q),octave_weights.x);}
    if octave_weights.y>0. {erosion=mix(.5,noise3(q*3.08+vec3(17.,3.,11.)),octave_weights.y);}
    let threshold=mix(.81,.32,atmosphere.weather.x);
    let crown=0.085*height*height;
    let eroded=body-(1.-erosion)*.13-crown;
    let envelope=smoothstep(0.,.10,height)*(1.-smoothstep(.72,1.,height));
    return smoothstep(threshold-.055,threshold+.10,eroded)*envelope;
}
