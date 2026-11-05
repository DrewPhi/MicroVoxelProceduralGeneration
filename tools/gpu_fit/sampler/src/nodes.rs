//! Authoritative density at the globally shared 4m P2 nodes. No voxel volume.
#![allow(dead_code)]
mod terrain;
use std::{fs,io::{BufWriter,Write},thread};
fn run()->Result<(),Box<dyn std::error::Error>>{
 let a:Vec<_>=std::env::args().collect();if a.len()!=3{return Err("node-sampler PARAMS OUTPUT.f32".into());}
 let p:Vec<f64>=fs::read_to_string(&a[1])?.split_whitespace().map(str::parse).collect::<Result<_,_>>()?;
 if p.len()!=22{return Err("Expected 22 parameters".into());}
 let params=terrain::Params{seed:p[0] as i32,world_m:p[1],ceiling_m:p[2],voxel_cm:p[3],base_m:p[4],relief_m:p[5],wavelength_m:p[6],octaves:p[7] as usize,gain:p[8],lacunarity:p[9],overhang_m:p[10],rock_scale_m:p[11],cave_scale_m:p[12],cave_width:p[13],cave_roof_m:p[14]};params.validate()?;
 let origin=[p[15],p[16],p[17]];let edge=p[21];let size=params.voxel_m();
 let counts=std::array::from_fn::<_,3,_>(|i|{let n=p[18+i]*size/edge;assert!((n-n.round()).abs()<1e-8);n.round() as usize});
 let shape=counts.map(|n|n*2+1);let total=shape.iter().product::<usize>();
 let threads=std::env::var("SLURM_CPUS_PER_TASK").ok().and_then(|s|s.parse().ok()).unwrap_or(4).clamp(1,32);
 let temporary=format!("{}.tmp",a[2]);let mut file=BufWriter::new(fs::File::create(&temporary)?);
 for first in (0..total).step_by(32768){
  let n=(total-first).min(32768);let mut values=vec![0f32;n];
  thread::scope(|scope|{let chunk=n.div_ceil(threads);for (part_index,part) in values.chunks_mut(chunk).enumerate(){let params=params.clone();scope.spawn(move||{
   let terrain=terrain::Terrain::new(params);
   for (j,value) in part.iter_mut().enumerate(){let i=first+part_index*chunk+j;let x=i%shape[0];let z=(i/shape[0])%shape[2];let y=i/(shape[0]*shape[2]);let xyz=[x,y,z];let point=std::array::from_fn(|axis|origin[axis]+xyz[axis] as f64*edge*0.5);*value=(terrain.density(point)/edge) as f32;}
  });}});
  for v in values{if !v.is_finite(){return Err("Nonfinite density".into());}file.write_all(&v.to_le_bytes())?;}
 }
 file.flush()?;drop(file);fs::rename(temporary,&a[2])?;println!("Wrote {total} shared density nodes, XYZ {shape:?}");Ok(())
}
fn main(){if let Err(error)=run(){eprintln!("{error}");std::process::exit(1)}}
