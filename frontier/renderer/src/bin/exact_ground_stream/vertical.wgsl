// A level camera's vertical strip has one horizontal ray. Walk it once for
// several pixel rows, testing each row's own radial clip and native first hit.
override VERTICAL: u32 = 8u;

fn vertical_min_y(row_slope:f32, start:f32, end:f32) -> f32 {
    return fma(row_slope,select(start,end,row_slope<0.),view.eye.y);
}

@compute @workgroup_size(16,4)
fn vertical_visibility(@builtin(global_invocation_id) invocation:vec3u) {
    let x=invocation.x;
    let top=invocation.y*VERTICAL;
    if x>=view.size.x || top>=view.size.y {return;}
    let rows=min(VERTICAL,view.size.y-top);
    let d=ray_vector(vec2f(f32(x)+.5,f32(top)+.5));
    let horizontal=length(d.xz);
    let planar=d.xz/horizontal;
    let slope0=d.y/horizontal;
    let slope_step=-2.*view.up.y/(f32(view.size.y)*horizontal);
    var slopes:array<f32,16>;
    var metrics:array<f32,16>;
    var stop=0.;
    let range=view.eye.w;
    for(var i=0u;i<rows;i+=1u) {
        slopes[i]=fma(f32(i),slope_step,slope0);
        metrics[i]=sqrt(fma(slopes[i],slopes[i],1.));
        stop=max(stop,range/metrics[i]);
    }
    var unresolved_rows=(1u<<rows)-1u;
    let root=view.size.z;
    let page_log=view.size.w;
    let side=1i<<root;
    let origin=view.camera_cell.xy;
    let sign=select(vec2i(1),vec2i(-1),planar<vec2f(0.));
    let v=abs(planar)*16.;
    let ray=Ray(select(vec2i(side)-origin,origin,sign>vec2i(0)),
        view.eye.xz*vec2f(sign),v,
        select(1./v,vec2f(INF),v==vec2f(0.)),sign,side);
    var start=0.;
    var fine=clamp(column_at(ray,start),vec2i(0),vec2i(side-1));
    var face=0u;
    var level=root;
    var id=0u;
    var exhausted=false;
    for(var step=0u;step<32768u && unresolved_rows!=0u;step+=1u) {
        let edge=((fine>>vec2u(level))+1)<<vec2u(level);
        let exits=exit_times(ray,edge);
        let end=max(start,min(stop,min(exits.x,exits.y)));
        let node=nodes[id];
        if end>start && vertical_min_y(slopes[firstLeadingBit(unresolved_rows)],start,end)<=node.ceiling {
            if node.link==0u {
                var pending=unresolved_rows;
                while pending!=0u {
                    let row=firstTrailingBit(pending);
                    let bit=1u<<row;
                    let a=max(start,.025/metrics[row]);
                    let b=min(end,range/metrics[row]);
                    let minimum=vertical_min_y(slopes[row],a,b);
                    if b>a && minimum<=node.ceiling {
                        pixels[(top+row)*view.size.x+x]=result(minimum,PAGE|id);
                        unresolved_rows&=~bit;
                    }
                    pending&=~bit;
                }
            } else if node.link<PAGE {
                level-=1u;
                let which=vec2u((world_column(ray,fine)>>vec2u(level))&vec2i(1));
                id=node.link+which.x+2u*which.y;
                continue;
            } else {
                let width=1u<<page_log;
                let mask=vec2i(i32(width)-1);
                let offset=node.link&~PAGE;
                let bits=u32(columns[offset+1u]);
                let blocks=width>>2u;
                let first=fine;
                for(var block=0u;block<2u*blocks+2u && unresolved_rows!=0u;block+=1u) {
                    let block_edge=((fine>>vec2u(2u))+1)<<vec2u(2u);
                    let block_exits=exit_times(ray,block_edge);
                    let block_end=min(end,max(start,min(block_exits.x,block_exits.y)));
                    let block_coord=vec2u(world_column(ray,fine)&mask)>>vec2u(2u);
                    let block_max=f32(columns[offset+2u+block_coord.y*blocks+block_coord.x])*.0625;
                    if block_end>start && vertical_min_y(slopes[firstLeadingBit(unresolved_rows)],start,block_end)<=block_max+.003 {
                        let encoding=read_block(offset,width,bits,block_coord);
                        for(var leaf=0u;leaf<10u && unresolved_rows!=0u;leaf+=1u) {
                            let next_exits=exit_times(ray,fine+1);
                            let t=min(block_end,max(start,min(next_exits.x,next_exits.y)));
                            let index=vec2u(world_column(ray,fine)&mask);
                            let column=read_column(encoding,width,index);
                            let height=f32(column>>8)*.0625;
                            var candidates=unresolved_rows;
                            while candidates!=0u {
                                let row=firstLeadingBit(candidates);
                                let bit=1u<<row;
                                // Higher rows have higher Y over the entire
                                // forward interval and can reject together.
                                if vertical_min_y(slopes[row],start,t)>height {break;}
                                let a=max(start,.025/metrics[row]);
                                let b=min(t,range/metrics[row]);
                                if b>a {
                                    let mat=u32(column&255);
                                    let slope=slopes[row];
                                    if fma(a,slope,view.eye.y)<=height {
                                        pixels[(top+row)*view.size.x+x]=result(a*metrics[row],mat|(face<<8u));
                                        unresolved_rows&=~bit;
                                    } else if slope<0. && fma(b,slope,view.eye.y)<=height {
                                        let hit=clamp((height-view.eye.y)/slope,a,b);
                                        pixels[(top+row)*view.size.x+x]=result(hit*metrics[row],mat);
                                        unresolved_rows&=~bit;
                                    }
                                }
                                candidates&=~bit;
                            }
                            if t>=block_end {break;}
                            face=entry_face(ray,next_exits);
                            fine=advance_column(ray,fine+1,t);
                            start=t;
                        }
                    }
                    if block_end>=end || unresolved_rows==0u {break;}
                    face=entry_face(ray,block_exits);
                    fine=advance_column(ray,block_edge,block_end);
                    start=block_end;
                    if any((first>>vec2u(page_log))!=(fine>>vec2u(page_log))) {break;}
                }
            }
        }
        if end>=stop {
            exhausted=true;
            break;
        }
        let next=advance_column(ray,edge,end);
        if any(next>=vec2i(side)) {break;}
        let difference=u32((fine.x^next.x)|(fine.y^next.y));
        if difference==0u {break;}
        let ancestor=min(root,firstLeadingBit(difference)+1u);
        for(var up=level;up<ancestor;up+=1u) {id=nodes[id].parent;}
        level=ancestor;
        start=end;
        fine=next;
        face=entry_face(ray,exits);
    }
    while unresolved_rows!=0u {
        let row=firstTrailingBit(unresolved_rows);
        pixels[(top+row)*view.size.x+x]=result(select(FAILED,SKY,exhausted),0u);
        unresolved_rows&=~(1u<<row);
    }
}
