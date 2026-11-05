//! Bounded Hermite samples of the authoritative Rust field. No dense volume.
#![allow(dead_code)]
mod terrain;
use std::{fs,io::{BufWriter,Write},path::PathBuf,thread};
fn hash(mut x:u64)->u64{x=x.wrapping_add(0x9e3779b97f4a7c15);x=(x^(x>>30)).wrapping_mul(0xbf58476d1ce4e5b9);x=(x^(x>>27)).wrapping_mul(0x94d049bb133111eb);x^(x>>31)}
fn uniform(i:u64,tag:u64)->f64{(hash(i^tag)>>11) as f64/(1u64<<53) as f64}
fn gradient(t:&terrain::Terrain,p:[f64;3],h:f64)->[f64;3]{std::array::from_fn(|a|{let mut lo=p;let mut hi=p;lo[a]-=h;hi[a]+=h;(t.density(hi)-t.density(lo))/(2.*h)})}
fn record(p:[f64;3],value:f64,g:[f64;3],kind:f64,origin:[f64;3],edge:f64)->[f32;8]{[( (p[0]-origin[0])/edge) as f32,((p[1]-origin[1])/edge) as f32,((p[2]-origin[2])/edge) as f32,value as f32,g[0] as f32,g[1] as f32,g[2] as f32,kind as f32]}
fn write_record(w:&mut BufWriter<fs::File>,r:[f32;8])->std::io::Result<()>{for v in r{w.write_all(&v.to_le_bytes())?;}Ok(())}
fn run()->Result<(),Box<dyn std::error::Error>>{
 let a:Vec<_>=std::env::args().collect();if a.len()!=6{return Err("hermite-sampler PARAMS OUTPUT OFF_COUNT SURFACE_COUNT SEED".into());}
 let p:Vec<f64>=fs::read_to_string(&a[1])?.split_whitespace().map(str::parse).collect::<Result<_,_>>()?;if p.len()!=22{return Err("Expected22parameters".into());}
 let params=terrain::Params{seed:p[0] as i32,world_m:p[1],ceiling_m:p[2],voxel_cm:p[3],base_m:p[4],relief_m:p[5],wavelength_m:p[6],octaves:p[7] as usize,gain:p[8],lacunarity:p[9],overhang_m:p[10],rock_scale_m:p[11],cave_scale_m:p[12],cave_width:p[13],cave_roof_m:p[14]};params.validate()?;
 let origin=[p[15],p[16],p[17]];let dims=[p[18] as u32,p[19] as u32,p[20] as u32];let edge=p[21];let size=params.voxel_m();let high=std::array::from_fn::<_,3,_>(|i|origin[i]+dims[i] as f64*size);
 let out=PathBuf::from(&a[2]);fs::create_dir_all(&out)?;let off_count:usize=a[3].parse()?;let surface_count:usize=a[4].parse()?;let seed:u64=a[5].parse()?;
 if off_count==0||surface_count==0{return Err("Positive sample counts required".into());}
 let threads=std::env::var("SLURM_CPUS_PER_TASK").ok().and_then(|x|x.parse().ok()).unwrap_or(4).clamp(1,32);
 let mut off=BufWriter::new(fs::File::create(out.join("off.bin.tmp"))?);let mut surface=BufWriter::new(fs::File::create(out.join("surface.bin.tmp"))?);
 let mut off_done=0;let mut surface_done=0;let batch=4096;let maximum=(off_count*8).max(surface_count*16);let started=std::time::Instant::now();
 for first in (0..maximum).step_by(batch){
  let n=batch.min(maximum-first);let mut records=vec![(None,None);n];
  thread::scope(|scope|{
   let chunk=n.div_ceil(threads);
   for (k,part) in records.chunks_mut(chunk).enumerate(){let params=params.clone();scope.spawn(move||{
    let t=terrain::Terrain::new(params.clone());let mut raw=params.clone();raw.cave_width=0.;let uncarved=terrain::Terrain::new(raw);
    for (j,r) in part.iter_mut().enumerate(){
     let index=first+k*chunk+j;let key=hash(index as u64^seed);let x=(uniform(key,11)*dims[0] as f64) as u32;let z=(uniform(key,23)*dims[2] as f64) as u32;
     let xx=origin[0]+(x as f64+0.5)*size;let zz=origin[2]+(z as f64+0.5)*size;let base=t.base_height(xx,zz);
     let y=if index%3==2{((base+(uniform(key,37)*2.-1.)*(params.overhang_m+24.)-origin[1])/size).floor().clamp(0.,dims[1] as f64-1.) as u32}else{(uniform(key,37)*dims[1] as f64) as u32};
     let mut point=[xx,origin[1]+(y as f64+0.5)*size,zz];let density=t.density_with_base_height(point,base);
     if index<off_count {r.0=Some(record(point,density,[0.;3],0.,origin,edge));}
     let h=(size*0.25).min(0.02);let mut valid=false;
     for _ in 0..12 {
      let d=t.density(point);if d.abs()<1e-5{valid=true;break;}
      let g=gradient(&t,point,h);let norm=g.iter().map(|v|v*v).sum::<f64>();if norm<1e-12{break;}
      let length=d.abs()/norm.sqrt();let scale=(4./length.max(1e-12)).min(1.);
      for axis in 0..3{point[axis]-=scale*d*g[axis]/norm;}
      if (0..3).any(|axis|point[axis]<origin[axis]+1e-5||point[axis]>=high[axis]-1e-5){break;}
     }
     if valid {
      let g=gradient(&t,point,h);let norm=g.iter().map(|v|v*v).sum::<f64>().sqrt();
      if norm>1e-7&&point.iter().all(|v|v.is_finite()) {
       let kind=if uncarved.density(point).abs()<0.08 {1.}else{2.};
       r.1=Some(record(point,0.,g.map(|v|v/norm),kind,origin,edge));
      }
     }
    }
   });}
  });
  for (a,b) in records {if let Some(r)=a{write_record(&mut off,r)?;off_done+=1;}if surface_done<surface_count{if let Some(r)=b{write_record(&mut surface,r)?;surface_done+=1;}}}
  if first%(batch*16)==0{println!("Hermite off{off_done}/{off_count},surface{surface_done}/{surface_count}, {:0.2}s",started.elapsed().as_secs_f64());}
  if off_done>=off_count&&surface_done>=surface_count{break;}
 }
 if off_done!=off_count||surface_done!=surface_count{return Err(format!("Insufficient bounded samples: off{off_done},surface{surface_done}").into());}
 off.flush()?;surface.flush()?;drop(off);drop(surface);fs::rename(out.join("off.bin.tmp"),out.join("off.bin"))?;fs::rename(out.join("surface.bin.tmp"),out.join("surface.bin"))?;
 println!("Hermite complete in{:0.2}s",started.elapsed().as_secs_f64());Ok(())
}
fn main(){if let Err(e)=run(){eprintln!("{e}");std::process::exit(1)}}
