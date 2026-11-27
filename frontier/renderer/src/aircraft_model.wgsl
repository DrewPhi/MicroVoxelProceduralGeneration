struct View {
    matrix: mat4x4<f32>,
    joints: array<mat4x4<f32>, 28>,
    sun: vec4f,
    eye: vec4f,
    flight: vec4f, // km/h, altitude, bank, pitch
    nav: vec4f,    // heading, climb rate, home bearing relative to nose, home range
    radio: vec4f,  // COM kHz, flight hold, stall, gear indication or ground flag
    chart: vec4f,
    motion: vec4f,
    wind: vec4f,
    weather_display: vec4f,
    weather_profile: vec4f,
    screen_pages: vec4u,
    inspection: vec4f,
    shadow_matrix: mat4x4<f32>,
}
@group(0) @binding(0) var<uniform> view: View;

@group(0) @binding(1) var terrain_chart: texture_2d<f32>;
@group(0) @binding(2) var weather_chart: texture_2d<f32>;

struct V {
    @builtin(position) clip: vec4f,
    @location(0) normal: vec3f,
    @location(1) color: vec3f,
    @location(2) local: vec3f,
    @location(3) uv: vec2f,
    @location(4) @interpolate(flat) display: u32,
    @location(5) @interpolate(flat) part: u32,
}
struct AnimatedVertex {
    position: vec4f,
    normal: vec3f,
}

fn animate_vertex(p:vec3f, normal:vec3f, part:u32)->AnimatedVertex {
    let joint=view.joints[part];
    var pos=joint*vec4f(p,1.);
    var n=(joint*vec4f(normal,0.)).xyz;
    let twin=view.eye.w>.5;
    let wing=select(abs(p.x)>.60 && p.y>.57 && p.y<1.42 && abs(p.z)<.96,
        abs(p.x)>.72 && abs(p.x)<7.29 && abs(p.z+.32)<1.20 && p.y>-.39 && p.y<.61,twin);
    if (part==0u && wing)
        || part==5u || part==6u || part==20u || part==21u {
        let width=select(4.9,6.675,twin);
        let span=clamp((abs(p.x)-.60)/width,0.,1.);
        let flex=select(view.inspection.z,view.inspection.w,p.x<0.);
        pos.y+=flex*span*span;
        n.x-=2.*span*flex*sign(p.x)*n.y/width;
    }
    return AnimatedVertex(pos,n);
}

@vertex fn vs(
    @location(0) p:vec3f, @location(1) color:vec3f, @location(2) normal:vec3f,
    @location(3) part:u32, @location(4) display:u32, @location(5) uv:vec2f,
) -> V {
    let vertex=animate_vertex(p,normal,part);
    return V(view.matrix*vertex.position,vertex.normal,color,vertex.position.xyz,uv,display,part);
}

@vertex fn vs_shadow(@location(0) p:vec3f, @location(3) part:u32)->@builtin(position) vec4f {
    return view.shadow_matrix*animate_vertex(p,vec3f(0.),part).position;
}

// A three-by-five pixel font, stored in row-major order from the top left.
fn glyph(code:u32) -> u32 {
    switch code {
        case 48u: { return 31599u; } // 0
        case 49u: { return 29850u; } // 1
        case 50u: { return 29671u; } // 2
        case 51u: { return 31207u; } // 3
        case 52u: { return 18925u; } // 4
        case 53u: { return 31183u; } // 5
        case 54u: { return 31695u; } // 6
        case 55u: { return 9383u; } // 7
        case 56u: { return 31727u; } // 8
        case 57u: { return 31215u; } // 9
        case 65u: { return 23530u; } // A
        case 66u: { return 15083u; } // B
        case 67u: { return 29263u; } // C
        case 68u: { return 15211u; } // D
        case 69u: { return 29391u; } // E
        case 70u: { return 4815u; } // F
        case 71u: { return 31567u; } // G
        case 72u: { return 23533u; } // H
        case 73u: { return 29847u; } // I
        case 75u: { return 23277u; } // K
        case 76u: { return 29257u; } // L
        case 77u: { return 23549u; } // M
        case 78u: { return 24573u; } // N
        case 79u: { return 31599u; } // O
        case 89u: { return 9389u; } // Y
        case 80u: { return 4843u; } // P
        case 82u: { return 23275u; } // R
        case 83u: { return 31183u; } // S
        case 84u: { return 9367u; } // T
        case 85u: { return 31597u; } // U
        case 86u: { return 11117u; } // V
        case 88u: { return 23213u; } // X
        case 47u: { return 468u; } // slash
        case 90u: { return 29287u; } // Z
        case 87u: { return 24557u; } // W
        case 45u: { return 448u; } // -
        case 43u: { return 1488u; } // +
        case 46u: { return 8192u; } // .
        default: { return 0u; }
    }
}

fn letters(uv:vec2f, at:vec2f, scale:vec2f, codes:array<u32,8>, count:u32) -> f32 {
    let pos=(uv-at)/scale;
    if any(pos<vec2f(0.)) || pos.x>=f32(4u*count) || pos.y>=5. { return 0.; }
    let cell=vec2u(floor(pos));
    let x=cell.x%4u;
    if x==3u { return 0.; }
    return f32((glyph(codes[cell.x/4u])>>(cell.y*3u+x))&1u);
}

fn label(uv:vec2f, at:vec2f, scale:vec2f, code:vec3u) -> f32 {
    return letters(uv,at,scale,array<u32,8>(code.x,code.y,code.z,0u,0u,0u,0u,0u),3u);
}

fn number(uv:vec2f, at:vec2f, scale:vec2f, value:u32, digits:u32) -> f32 {
    var codes:array<u32,8>;
    var n=value;
    for (var i=digits;i>0u;i-=1u) {
        codes[i-1u]=48u+n%10u;
        n/=10u;
    }
    return letters(uv,at,scale,codes,digits);
}

fn box_at(uv:vec2f,lo:vec2f,hi:vec2f)->bool {
    return all(uv>=lo) && all(uv<=hi);
}

fn rotate(p:vec2f, angle:f32) -> vec2f {
    let s=sin(angle);
    let c=cos(angle);
    return vec2f(c*p.x-s*p.y,s*p.x+c*p.y);
}

const MINT:vec3f=vec3f(.47,.90,.80);
const WHITE:vec3f=vec3f(.77,.86,.79);
const INK:vec3f=vec3f(.012,.025,.038);
const AMBER:vec3f=vec3f(.96,.48,.074);
const FACE:vec3f=vec3f(.025,.034,.041);

const TAU:f32=6.2831853;

// Dial coordinates run clockwise from twelve o'clock, in units of face radius.
fn radial(a:f32)->vec2f {return vec2f(sin(a),-cos(a));}
fn stroke(p:vec2f,a:vec2f,b:vec2f,width:f32,aa:f32)->f32 {
    let edge=b-a;
    let d=length(p-a-edge*clamp(dot(p-a,edge)/max(dot(edge,edge),1.e-7),0.,1.));
    return 1.-smoothstep(width,width+aa,d);
}
fn dial(p:vec2f,kind:u32)->vec3f {
    let r=length(p);
    let aa=clamp(fwidth(r),.009,.085);
    let rim=vec3f(.14,.16,.165);
    var color=mix(FACE,rim,smoothstep(.88,.95,r));
    if r>.98 {return mix(rim,vec3f(.062,.075,.08),smoothstep(.98,1.02,r));}
    let angle=atan2(p.x,-p.y);
    let uv=p*.5+.5;
    if kind==2u {
        let horizon=rotate(p,view.flight.z).y-view.flight.w*1.7;
        color=mix(vec3f(.085,.32,.53),vec3f(.44,.21,.083),smoothstep(-aa,aa,horizon));
        if abs(horizon)<aa {color=WHITE;}
        let ladder=abs(horizon-round(horizon/.25)*.25);
        if abs(p.x)<.26 && abs(horizon)<.6 && ladder<.023 {color=WHITE;}
        let wings= max(stroke(p,vec2f(-.61,0.),vec2f(-.19,0.),.036,aa),stroke(p,vec2f(.19,0.),vec2f(.61,0.),.036,aa));
        color=mix(color,AMBER,max(wings,1.-smoothstep(.05,.05+aa,r)));
        return mix(color,rim,smoothstep(.86,.96,r));
    }
    var sweep=TAU*.83;
    var start=-sweep*.5;
    var major=9.;
    var step_value=20u;
    var reading=clamp(view.flight.x/1.852/180.,0.,1.); // knots
    var unit=vec3u(75,84,83);
    if kind==3u {
        start=0.; sweep=TAU; major=10.; step_value=1u;
        reading=fract(view.flight.y*3.28084/1000.);
        unit=vec3u(70,84,0);

    }
    let along=fract((angle-start)/TAU+1.)*TAU/sweep;
    if along<=1. {
        let m=along*major;
        let major_tick=abs(m-round(m));
        let minor_tick=abs(m*4.-round(m*4.));
        if r<.89 && ((r>.70 && major_tick<.055) || (r>.79 && minor_tick<.12)) {color=WHITE;}
        if kind==1u && r>.90 && r<.95 && along>.27 && along<.91 {
            color=select(vec3f(.11,.62,.27),AMBER,along>.76);
        }
    }
    for(var i=0u;i<=10u;i++){
        if f32(i)>major || (kind==3u && i==10u) {break;}
        let val=i*step_value;
        let pos=radial(start+sweep*f32(i)/major)*.306+.5;
        let digits=select(select(1u,2u,val>=10u),3u,val>=100u);
        color=mix(color,WHITE,number(uv,pos-vec2f(f32(digits)*.029,.041),vec2f(.016,.017),val,digits));
    }
    color=mix(color,MINT,label(uv,vec2f(.36,.61),vec2f(.024,.017),unit));
    if kind==3u {
        let short=radial(fract(view.flight.y*3.28084/10000.)*TAU);
        color=mix(color,WHITE,stroke(p,vec2f(0.),short*.46,.051,aa));
    }
    let arrow=radial(start+sweep*reading);
    color=mix(color,WHITE,stroke(p,-arrow*.13,arrow*.77,.023,aa));
    return mix(color,AMBER,1.-smoothstep(.049,.049+aa,r));
}

fn flight_display(uv:vec2f)->vec3f {
    // The attitude field rotates under a fixed aircraft symbol.
    let p=(uv-vec2f(.5,.49))*vec2f(1.6,1.);
    let attitude=rotate(p,view.flight.z);
    let horizon=attitude.y-view.flight.w*.72;
    let aa=clamp(fwidth(horizon),.002,.018);
    let sky=mix(vec3f(.030,.19,.39),vec3f(.125,.40,.58),clamp(1.+2.*horizon,0.,1.));
    let ground=mix(vec3f(.44,.24,.095),vec3f(.27,.132,.053),clamp(horizon*2.,0.,1.));
    var color=mix(sky,ground,smoothstep(-aa,aa,horizon));
    let interval=radians(10.)*.72;
    let mark=round(horizon/interval);
    let ladder=abs(horizon-mark*interval);
    let half_width=select(.084,.158,mark==0.);
    if abs(attitude.x)<half_width && abs(mark)<=5. {
        color=mix(color,WHITE,1.-smoothstep(.003,.003+aa,ladder));
    }
    let wings=max(stroke(p,vec2f(-.18,0.),vec2f(-.054,0.),.009,aa),
                  stroke(p,vec2f(.054,0.),vec2f(.18,0.),.009,aa));
    color=mix(color,AMBER,max(wings,1.-smoothstep(.013,.013+aa,length(p))));
    for (var i=-2;i<=2;i+=1) {
        let tick=radial(f32(i)*.5236);
        color=mix(color,WHITE,stroke(p,tick*.288,tick*.312,.002,aa));
    }
    let bank=radial(-view.flight.z);
    color=mix(color,AMBER,stroke(p,bank*.25,bank*.28,.005,aa));

    let knots=view.flight.x/1.852;
    let feet=view.flight.y*3.28084;
    let speed=knots+(uv.y-.49)*-85.;
    let altitude=feet+(uv.y-.49)*-900.;
    if uv.x<.197 || uv.x>.785 {
        let tick=select(abs(fract(speed/10.+.5)-.5),abs(fract(altitude/100.+.5)-.5),uv.x>.785);
        color=vec3f(.037,.068,.090);
        if tick<.055 && ((uv.x>.15 && uv.x<.185) || (uv.x>.795 && uv.x<.832)) {color=WHITE;}
    }
    if box_at(uv,vec2f(.019,.421),vec2f(.182,.550))
        || box_at(uv,vec2f(.793,.421),vec2f(.984,.550)) {color=INK;}
    if uv.y<.129 || uv.y>.84 {color=INK;}
    let small=vec2f(.011,.014);
    color=mix(color,MINT,label(uv,vec2f(.034,.033),small,vec3u(70,76,84)));
    color=mix(color,MINT,label(uv,vec2f(.034,.181),small,vec3u(75,84,83)));
    color=mix(color,MINT,label(uv,vec2f(.861,.181),small,vec3u(70,84,0)));
    color=mix(color,WHITE,number(uv,vec2f(.032,.453),vec2f(.012,.014),u32(round(knots)),3u));
    color=mix(color,WHITE,number(uv,vec2f(.817,.453),vec2f(.008,.014),u32(round(abs(feet))),5u));
    if feet<0. {
        color=mix(color,WHITE,letters(uv,vec2f(.794,.453),vec2f(.006,.014),
            array<u32,8>(45,0,0,0,0,0,0,0),1u));
    }
    color=mix(color,MINT,label(uv,vec2f(.317,.888),small,vec3u(72,68,71)));
    color=mix(color,WHITE,number(uv,vec2f(.516,.875),vec2f(.018,.018),
        u32(round(view.nav.x))%360u,3u));
    let vertical=view.nav.y*196.85;
    let sign=select(43u,45u,vertical<0.);
    color=mix(color,MINT,label(uv,vec2f(.465,.033),small,vec3u(86,47,83)));
    color=mix(color,WHITE,letters(uv,vec2f(.616,.033),small,
        array<u32,8>(sign,0,0,0,0,0,0,0),1u));
    color=mix(color,WHITE,number(uv,vec2f(.664,.033),small,
        u32(clamp(round(abs(vertical)),0.,9999.)),4u));
    color=mix(color,MINT,label(uv,vec2f(.846,.033),small,vec3u(70,80,77)));
    if view.radio.z>.5 {
        color=mix(color,AMBER,letters(uv,vec2f(.198,.033),small,array<u32,8>(83,84,65,76,76,0,0,0),5u));
    } else if view.radio.y>.5 {
        color=mix(color,MINT,label(uv,vec2f(.198,.033),small,vec3u(65,80,0)));
    }
    return color;
}

fn navigation(uv:vec2f)->vec3f {
    // +X is east, +Z is south. North stays up as the aircraft banks and turns.
    let delta=(uv-.5)*vec2f(view.chart.w,view.chart.w*.625);
    let texel=(view.chart.xy+delta)/view.chart.z+vec2f(textureDimensions(terrain_chart))/2.;
    let cell=vec2i(floor(texel));
    let f=fract(texel);
    let a=textureLoad(terrain_chart,cell,0).rg;
    let b=textureLoad(terrain_chart,cell+vec2i(1,0),0).rg;
    let c=textureLoad(terrain_chart,cell+vec2i(0,1),0).rg;
    let d=textureLoad(terrain_chart,cell+vec2i(1),0).rg;
    let terrain=mix(mix(a,b,f.x),mix(c,d,f.x),f.y);
    let shade=clamp(.8+(b.x-a.x+c.x-a.x)*.022,.54,1.15);
    var color=mix(vec3f(.10,.25,.14),vec3f(.51,.37,.22),clamp((terrain.x-view.flight.y+320.)/600.,0.,1.))*shade;
    let contour=abs(fract(terrain.x/25.+.5)-.5);
    color*=mix(.64,1.,smoothstep(.015,.09,contour));
    if terrain.y>.15 {color=vec3f(.045,.23,.38);}
    let home_angle=radians(view.nav.x)+view.nav.z;
    let home=radial(home_angle)*view.nav.w*1000.;
    if abs(length((delta-home)/view.chart.w)-.027)<.006 {color=AMBER;}
    let icon=rotate(delta/view.chart.w,-radians(view.nav.x));
    if stroke(icon,vec2f(0.,-.060),vec2f(0.,.049),.005,.002)+stroke(icon,vec2f(-.046,.008),vec2f(.046,.008),.007,.002)>.5 {color=vec3f(.98,.94,.76);}
    if uv.y<.13 {color=INK;}
    color=mix(color,MINT,label(uv,vec2f(.04,.03),vec2f(.014,.014),vec3u(78,65,86)));
    let distance=u32(clamp(round(view.nav.w*10.),0.,999.));
    let text=array<u32,8>(48u+distance/100u,48u+distance/10u%10u,46u,48u+distance%10u,0u,75u,77u,0u);
    return mix(color,WHITE,letters(uv,vec2f(.60,.035),vec2f(.012,.013),text,7u));
}

fn systems(uv:vec2f)->vec3f {
    var color=INK;
    if uv.y>.16 && uv.y<.96 {
        color=vec3f(.034,.055,.072);
    }
    let unit=vec2f(.012,.018);
    color=mix(color,MINT,letters(uv,vec2f(.048,.036),unit,
        array<u32,8>(83,89,83,84,69,77,83,0),7u));

    color=mix(color,MINT,label(uv,vec2f(.050,.209),unit,vec3u(67,79,77)));
    let f=u32(view.radio.x);
    let mhz=f/1000u;
    let khz=f%1000u;
    let com=array<u32,8>(48u+mhz/100u,48u+mhz/10u%10u,48u+mhz%10u,46u,
                        48u+khz/100u,48u+khz/10u%10u,48u+khz%10u,0u);
    color=mix(color,WHITE,letters(uv,vec2f(.336,.194),vec2f(.018,.023),com,7u));

    let twin=view.eye.w>.5;
    color=mix(color,MINT,label(uv,vec2f(.05,.394),unit,select(vec3u(84,72,82),vec3u(76,80,87),twin)));
    color=mix(color,WHITE,number(uv,vec2f(.76,.382),vec2f(.014,.022),
        u32(round(view.motion.y*100.)),3u));
    if box_at(uv,vec2f(.052,.52),vec2f(.945,.587)) {
        color=select(vec3f(.069,.112,.125),MINT,uv.x<=mix(.052,.945,view.motion.y));
    }
    color=mix(color,MINT,letters(uv,vec2f(.05,.675),unit,
        array<u32,8>(70,76,65,80,83,0,0,0),5u));
    color=mix(color,WHITE,number(uv,vec2f(.370,.675),unit,u32(round(view.motion.z)),2u));
    color=mix(color,MINT,letters(uv,vec2f(.55,.675),unit,
        array<u32,8>(84,82,73,77,0,0,0,0),4u));
    let sign=select(43u,45u,view.motion.w<0.);
    color=mix(color,WHITE,letters(uv,vec2f(.785,.675),unit,
        array<u32,8>(sign,0,0,0,0,0,0,0),1u));
    color=mix(color,WHITE,number(uv,vec2f(.834,.675),unit,u32(round(abs(view.motion.w)*100.)),3u));
    color=mix(color,MINT,label(uv,vec2f(.05,.843),unit,select(vec3u(77,73,88),vec3u(82,80,87),twin)));
    color=mix(color,WHITE,number(uv,vec2f(.251,.843),unit,
        u32(round(view.motion.x*100.)),3u));
    if view.screen_pages.w==0u {
        color=mix(color,AMBER,letters(uv,vec2f(.55,.843),unit,
            array<u32,8>(69,78,71,0,79,70,70,0),7u));
    } else if view.radio.y>.5 {
        color=mix(color,MINT,letters(uv,vec2f(.55,.843),unit,
            array<u32,8>(65,80,0,72,79,76,68,0),7u));
    }
    return color;
}

fn wx_image(uv:vec2f)->vec3f {
    var color=INK;
    let mode=u32(view.weather_display.x);
    if uv.y>.23 && uv.y<.84 {
        let size=textureDimensions(weather_chart);
        let px=clamp(vec2i(vec2f(uv.x,(uv.y-.23)/.61)*vec2f(size)),vec2i(0),vec2i(size)-1);
        color=textureLoad(weather_chart,px,0).rgb;
    }
    var codes=array<u32,8>(68,65,84,65,76,73,78,75);
    var count=8u;
    if mode==2u { codes=array<u32,8>(87,73,78,68,0,0,0,0);count=4u; }
    if mode==3u { codes=array<u32,8>(87,88,0,83,87,69,69,80); count=8u; }
    if mode==4u { codes=array<u32,8>(80,82,79,70,73,76,69,0); count=7u; }
    color=mix(color,MINT,letters(uv,vec2f(.032,.035),vec2f(.018,.031),codes,count));
    color=mix(color,WHITE,number(uv,vec2f(.741,.035),vec2f(.024,.030),u32(view.weather_display.y),1u));
    color=mix(color,MINT,label(uv,vec2f(.841,.055),vec2f(.016,.025),vec3u(78,77,0)));
    let z=view.weather_display.z;
    let age=view.weather_display.w;
    if mode==1u {
        if age < 0. {
            color=mix(color,AMBER,letters(uv,vec2f(.040,.865),vec2f(.017,.024),
                array<u32,8>(78,79,0,68,65,84,65,0),7u));
        } else {
            color=mix(color,MINT,label(uv,vec2f(.032,.865),vec2f(.017,.024),vec3u(65,71,69)));
            color=mix(color,WHITE,number(uv,vec2f(.28,.865),vec2f(.018,.024),u32(age/60.),2u));
            color=mix(color,MINT,label(uv,vec2f(.44,.865),vec2f(.016,.024),vec3u(77,73,78)));
        }
    } else {
        var label_code=vec3u(77,83,76);
        if mode==3u { label_code=vec3u(84,73,76); }
        if mode==4u { label_code=vec3u(65,90,0); }
        color=mix(color,MINT,label(uv,vec2f(.032,.865),vec2f(.017,.024),label_code));
        let sign=select(43u,45u,z<0.);
        color=mix(color,WHITE,letters(uv,vec2f(.284,.865),vec2f(.018,.024),
            array<u32,8>(sign,0,0,0,0,0,0,0),1u));
        let value=abs(z)*select(1.,3.,mode==4u);
        color=mix(color,WHITE,number(uv,vec2f(.38,.865),vec2f(.018,.024),u32(value),select(2u,5u,mode==2u)));
    }
    if mode==4u {
        for(var i=0u;i<3u;i+=1u) {
            let t=f32(i)*.5;
            let h=round(mix(view.weather_profile.y,view.weather_profile.x,t));
            let y=.25+t*.49;
            if box_at(uv,vec2f(.015,y-.014),vec2f(.209,y+.103)) {color=INK;}
            color=mix(color,WHITE,number(uv,vec2f(.024,y),vec2f(.009,.017),u32(abs(h)),5u));
        }
        color=mix(color,MINT,label(uv,vec2f(.720,.865),vec2f(.017,.024),vec3u(70,84,0)));
    } else if mode==2u {
        color=mix(color,MINT,label(uv,vec2f(.720,.865),vec2f(.017,.024),vec3u(70,84,0)));
    }
    return color;
}

fn weather(uv:vec2f)->vec3f {
    if view.weather_display.x>.5 { return wx_image(uv); }
    let speed=length(view.wind.xz);
    let calm=speed<.13;
    let bearing=(u32(round(degrees(atan2(-view.wind.x,view.wind.z))+360.)))%360u;
    let vertical=view.wind.y*196.85;
    var color=INK;
    if uv.y<.25 { color=vec3f(.044,.105,.145); }
    color=mix(color,MINT,letters(uv,vec2f(.05,.058),vec2f(.017,.029),
        array<u32,8>(87,73,78,68,0,0,0,0),4u));
    color=mix(color,vec3f(.57,.68,.71),letters(uv,vec2f(.57,.058),vec2f(.015,.029),
        array<u32,8>(87,88,0,78,47,65,0,0),6u));
    let p=(uv-vec2f(.18,.61))*vec2f(1.6,1.);
    let r=length(p);
    let ring=1.-smoothstep(.006,.018,abs(r-.244));
    color=mix(color,vec3f(.21,.38,.44),ring);
    if !calm {
        let d=radial(radians(f32(bearing)));
        let tip=d*.186;
        let bar=stroke(p,-d*.14,tip,.009,.012);
        let pr=vec2f(-d.y,d.x);
        let head=max(stroke(p,tip,tip-d*.077+pr*.049,.009,.009),
            stroke(p,tip,tip-d*.077-pr*.049,.009,.009));
        color=mix(color,MINT,max(bar,head));
    } else {
        color=mix(color,MINT,1.-smoothstep(.012,.029,r));
    }
    if calm {
        color=mix(color,WHITE,letters(uv,vec2f(.40,.40),vec2f(.021,.047),
            array<u32,8>(67,65,76,77,0,0,0,0),4u));
    } else {
        color=mix(color,WHITE,number(uv,vec2f(.40,.37),vec2f(.018,.048),bearing,3u));
        color=mix(color,WHITE,number(uv,vec2f(.67,.37),vec2f(.017,.048),u32(round(speed*1.94384)),3u));
        color=mix(color,MINT,label(uv,vec2f(.89,.42),vec2f(.013,.031),vec3u(75,84,0)));
    }
    color=mix(color,MINT,label(uv,vec2f(.40,.71),vec2f(.014,.028),vec3u(86,82,84)));
    let sign=select(43u,45u,vertical<0.);
    color=mix(color,WHITE,letters(uv,vec2f(.60,.71),vec2f(.015,.028),
        array<u32,8>(sign,0,0,0,0,0,0,0),1u));
    color=mix(color,WHITE,number(uv,vec2f(.66,.71),vec2f(.015,.028),u32(round(abs(vertical))),4u));
    color=mix(color,MINT,label(uv,vec2f(.89,.71),vec2f(.009,.028),vec3u(70,80,77)));
    return color;
}

fn screen(uv:vec2f, page:u32)->vec3f {
    // Keep these page IDs in step with aircraft::DisplayPage.
    switch page {
        case 0u: { return flight_display(uv); }
        case 1u: { return navigation(uv); }
        case 2u: { return weather(uv); }
        default: { return systems(uv); }
    }
}

fn section_hidden(v:V)->bool {
    // The cutaway removes the roof and one side of the airframe over the cabin.
    if view.inspection.x==0. {return false;}
    if view.eye.w>.5 {
        let cabin_shell=v.part==0u || v.part==3u || v.part==4u || v.part==27u;
        return cabin_shell && abs(v.local.x)<.85 && v.local.z> -3.12 && v.local.z<1.58
            && (v.local.y>.51 || (v.local.x*view.inspection.x>.36 && v.local.y>-.395));
    }
    // Overhead lighting and trim follow the removed roof in a section view.
    if v.part==18u && v.local.y>.60 {return true;}
    return v.local.z> -2.15 && v.local.z<1.1 &&
        (v.part==0u || v.part==3u || v.part==4u || v.part==5u || v.part==6u ||
         v.part==20u || v.part==21u) &&
        (v.local.y>.61 || (v.local.x*view.inspection.x>.56 && v.local.y>-.44));
}
@fragment fn fs_opaque(v:V)->@location(0) vec4f {
    if section_hidden(v) {discard;}
    if v.display>0u {
        let uv=v.uv;
        if v.display>=10u {
            var text=array<u32,8>(69,78,71,73,78,69,83,0); // ENGINES
            var count=7u;
            if v.display==11u { text=array<u32,8>(76,73,71,72,84,83,0,0); count=6u; }
            if v.display==12u { text=array<u32,8>(71,69,65,82,0,0,0,0); count=4u; }
            let ink=letters(uv,vec2f(.08,.15),vec2f(.84/f32(4u*count),.14),text,count);
            if ink==0. {discard;}
            return vec4f(vec3f(.67,.70,.68)*(.50+.5*max(dot(normalize(v.normal),view.sun.xyz),0.)),1.);
        }
        if v.display<=3u {
            let p=uv*2.-1.;
            let color=dial(p,v.display);
            if dot(p,p)>1.04 {discard;}
            return vec4f(color,1.);
        }
        if any(uv<vec2f(.017)) || any(uv>vec2f(.983)) {return vec4f(FACE,1.);}
        if view.screen_pages.z==0u {return vec4f(INK,1.);}
        return vec4f(screen(uv,view.screen_pages[v.display-7u]),1.);
    }
    let n=normalize(v.normal);
    let eye=normalize(view.eye.xyz-v.local);
    let half_vec=normalize(eye+view.sun.xyz);
    let sky=vec3f(.34,.40,.48)*(.78+.22*n.y);
    let direct=vec3f(.77,.71,.63)*max(dot(n,view.sun.xyz),0.);
    let gloss=pow(max(dot(n,half_vec),0.),35.)*.075;
    var paint=v.color;
    if view.eye.w>.5 && v.part==18u && v.color.g>.5 && v.color.r<.2
        && v.local.x>.12 && v.local.x<.23 && abs(v.local.y+.062)<.015
        && abs(v.local.z-.855)<.030 {
        if view.radio.w<.5 { paint=vec3f(.037,.067,.044); }
        if view.radio.w>1.5 { paint=vec3f(.75,.30,.04); }
    }
    return vec4f(paint*(sky+direct)+gloss,1.);
}

@fragment fn fs_glass(v:V)->@location(0) vec4f {
    if v.display!=9u && (view.inspection.y>.5 || section_hidden(v)) {discard;}
    if v.display==9u {
        let radius=length(v.uv*2.-1.);
        let aa=max(fwidth(radius),.008);
        let disc=(1.-smoothstep(.97-aa,.97+aa,radius))*smoothstep(.09,.24,radius);
        let tip_offset=(radius-.90)/.08;
        let tips=exp(-tip_offset*tip_offset);
        let color=mix(vec3f(.105,.13,.16),vec3f(.74,.77,.72),tips*.72);
        let opacity=(.12+.09*view.motion.y)*disc*(1.-.58*radius);
        return vec4f(color,opacity);
    }
    let eye=normalize(view.eye.xyz-v.local);
    let face=normalize(v.normal);
    let n=select(-face,face,dot(eye,face)>=0.);
    let grazing=1.-clamp(dot(n,eye),0.,1.);
    let fresnel=.04+.75*pow(grazing,4.);
    let mirror=reflect(-eye,n);
    let sky=mix(vec3f(.21,.39,.54),vec3f(.095,.23,.44),sqrt(clamp(mirror.y,0.,1.)));
    let glint=.60*pow(max(dot(mirror,view.sun.xyz),0.),130.);
    if view.eye.w>.5 && dot(face,eye)<0. {
        return vec4f(sky, .035 + .12*pow(grazing,5.));
    }
    let tint=select(.05,.17,view.eye.w>.5);
    return vec4f(sky+vec3f(glint,glint*.82,glint*.60),clamp(tint+fresnel*.56+glint*.35,0.,.72));
}

struct MarkerV {
    @builtin(position) clip: vec4f,
    @location(0) color: vec3f,
}

@vertex fn vs_marker(
    @builtin(vertex_index) vertex:u32,
    @location(0) origin:vec4f, @location(1) axis:vec4f, @location(2) tint:vec4f,
) -> MarkerV {
    let faces=array<vec3f,6>(vec3f(1.,0.,0.),vec3f(-1.,0.,0.),
        vec3f(0.,1.,0.),vec3f(0.,-1.,0.),vec3f(0.,0.,1.),vec3f(0.,0.,-1.));
    let corners=array<vec2f,6>(vec2f(-1.,-1.),vec2f(1.,-1.),vec2f(1.,1.),
        vec2f(-1.,-1.),vec2f(1.,1.),vec2f(-1.,1.));
    let normal=faces[vertex/6u];
    let across=normal.zxy;
    let up=cross(normal,across);
    let uv=corners[vertex%6u];
    let shape=(normal+uv.x*across+uv.y*up)*vec3f(origin.w,axis.w,origin.w);
    let tangent=normalize(cross(axis.xyz,select(vec3f(0.,1.,0.),vec3f(0.,0.,1.),
        abs(axis.y)>.96)));
    let p=origin.xyz+tangent*shape.x+axis.xyz*shape.y+cross(tangent,axis.xyz)*shape.z;
    let tint_scale=.67+.33*abs(dot(normal,vec3f(.5,1.,.71)));
    return MarkerV(view.matrix*vec4f(p,1.),tint.xyz*tint_scale);
}
@fragment fn fs_marker(v:MarkerV)->@location(0) vec4f {
    return vec4f(v.color,1.);
}
