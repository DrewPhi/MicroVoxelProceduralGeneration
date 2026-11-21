// A local plane is known from the metre-spaced canonical height samples.
// Quantize its height at native cell centers and traverse the actual square
// roofs and risers of that reconstructed plane. Coordinates are rebased to
// one nearby integer voxel so that the lattice remains stable far from spawn.
struct StairHit { normal:vec3<f32>, distance:f32, band:f32 };
fn trace_stair_plane(start:vec3<f32>,ray:vec3<f32>,plane:vec3<f32>,n:vec3<f32>,end:f32)->StairHit {
    var cell=floor(start.xz);
    let step=sign(ray.xz);
    let delta=min(vec2(1e8),1./max(abs(ray.xz),vec2(1e-8)));
    var next=select(start.xz-cell,cell+1.-start.xz,ray.xz>vec2(0.))*delta;
    next=select(next,vec2(1e8),abs(ray.xz)<vec2(1e-8));
    var enter=0.;
    var side=vec3(0.);
    for(var i=0u;i<12u;i++){
        let roof=floor(plane.y-dot(n.xz,cell+.5-plane.xz)/n.y+.5);
        let y=start.y+enter*ray.y;
        if y<roof-.00001 {return StairHit(side,enter,max(0.,roof-1.-floor(y)));}
        let finish=min(end,min(next.x,next.y));
        if ray.y<-.00001 {
            let t=(roof-start.y)/ray.y;
            if t>=enter && t<finish {return StairHit(vec3(0.,1.,0.),t,0.);}
        }
        if finish>=end {break;}
        let x=next.x<=next.y;
        // At a corner both columns change before the next positive-length span.
        let z=next.y<=next.x;
        if x {cell.x+=step.x;next.x+=delta.x;side=vec3(-step.x,0.,0.);}
        if z {cell.y+=step.y;next.y+=delta.y;side=vec3(0.,0.,-step.y);}
        enter=finish;
    }
    return StairHit(vec3(0.),end,-1.);
}

fn stair_pixel(plane:vec3<f32>,ray:vec3<f32>,n:vec3<f32>,sun:vec3<f32>,fallback:f32)->vec4<f32> {
    let extent=1.01*dot(abs(n),vec3(.5));
    let range=extent/max(.001,-dot(n,ray))+.01;
    let start=plane-range*ray;
    let hit=trace_stair_plane(start,ray,plane,n,2.*range);
    if hit.band<0. || dot(hit.normal,hit.normal)<.5 {return vec4(0.,0.,0.,-1.);}
    let cosine=max(0.,dot(hit.normal,sun));
    var lit=0.;
    let facing=dot(n,sun);
    if cosine>0. && facing>0. {
        let reach=2.*extent/facing+.02;
        lit=cosine*fallback;
        if reach<10. {
            let point=start+hit.distance*ray+.001*hit.normal;
            let blocker=trace_stair_plane(point,sun,plane,n,reach);
            lit=select(0.,cosine,blocker.band<0.);
        }
    }
    let axis=select(select(2.,0.,abs(hit.normal.x)>.5),1.,hit.normal.y>.5);
    return vec4(hit.normal.y,lit,axis,hit.band);
}
