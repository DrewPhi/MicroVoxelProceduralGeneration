//! Portable authoritative CPU target generator; training is on the GPU.
#![allow(dead_code)]
mod terrain;
use std::{fs,io::{BufWriter,Write},path::PathBuf,thread};
fn hash(mut x:u64)->u64{x=x.wrapping_add(0x9e3779b97f4a7c15);x=(x^(x>>30)).wrapping_mul(0xbf58476d1ce4e5b9);x=(x^(x>>27)).wrapping_mul(0x94d049bb133111eb);x^(x>>31)}
fn uniform(i:u64,tag:u64)->f64{(hash(i^tag)>>11) as f64/(1u64<<53) as f64}
fn run()->Result<(),Box<dyn std::error::Error>>{
 let a:Vec<_>=std::env::args().skip(1).collect();if a.len()!=4{return Err("terrain-sampler PARAMS.txt OUTPUT_DIRECTORY SAMPLE_COUNT SEED".into());}
 let p:Vec<f64>=fs::read_to_string(&a[0])?.split_whitespace().map(str::parse).collect::<Result<_,_>>()?;if p.len()!=22{return Err("Expected 22 parameter values".into());}
 let terrain_params=terrain::Params{seed:p[0] as i32,world_m:p[1],ceiling_m:p[2],voxel_cm:p[3],base_m:p[4],relief_m:p[5],wavelength_m:p[6],octaves:p[7] as usize,gain:p[8],lacunarity:p[9],overhang_m:p[10],rock_scale_m:p[11],cave_scale_m:p[12],cave_width:p[13],cave_roof_m:p[14]};terrain_params.validate()?;
 let origin=[p[15],p[16],p[17]];let dims=[p[18] as u32,p[19] as u32,p[20] as u32];let edge=p[21];let size=terrain_params.voxel_m();
 if dims.contains(&0)||origin.iter().any(|v|!v.is_finite())||edge<=0.{return Err("Invalid region".into());}
 let out=PathBuf::from(&a[1]);fs::create_dir_all(&out)?;let count:u64=a[2].parse()?;let seed:u64=a[3].parse()?;let threads=std::env::var("SLURM_CPUS_PER_TASK").ok().and_then(|v|v.parse().ok()).unwrap_or_else(||thread::available_parallelism().map(|n|n.get()).unwrap_or(1)).clamp(1,32);
 let mut file=BufWriter::new(fs::File::create(out.join("samples.bin.tmp"))?);
 let started=std::time::Instant::now();let batch=262144u64;
 for first in (0..count).step_by(batch as usize){
  let n=(count-first).min(batch) as usize;let mut bytes=vec![0u8;n*16];
  thread::scope(|scope|{
   let per=(n+threads-1)/threads;
   for (thread_index,buffer) in bytes.chunks_mut(per*16).enumerate(){let params=terrain_params.clone();scope.spawn(move||{
    let t=terrain::Terrain::new(params);
    for (j,record) in buffer.chunks_exact_mut(16).enumerate(){
     let index=first+(thread_index*per+j) as u64;let key=hash(index^seed);let x=(uniform(key,11)*dims[0] as f64) as u32;let z=(uniform(key,23)*dims[2] as f64) as u32;
     let xx=origin[0]+(x as f64+0.5)*size;let zz=origin[2]+(z as f64+0.5)*size;let base=t.base_height(xx,zz);
     let y=if index%3==2{let yy=base+(uniform(key,37)*2.-1.)*(t.p.overhang_m+24.);((yy-origin[1])/size).floor().clamp(0.,dims[1] as f64-1.) as u32}else{(uniform(key,37)*dims[1] as f64) as u32};
     let density=t.density_with_base_height([xx,origin[1]+(y as f64+0.5)*size,zz],base) as f32;
     for (axis,value) in [x,y,z].iter().enumerate(){record[axis*4..axis*4+4].copy_from_slice(&value.to_le_bytes());}record[12..16].copy_from_slice(&density.to_le_bytes());
    }
   });}
  });file.write_all(&bytes)?;
  if first%(batch*16)==0{println!("sampled {}/{count}, {:.1}s",first+n as u64,started.elapsed().as_secs_f64());}
 }
 file.flush()?;drop(file);fs::rename(out.join("samples.bin.tmp"),out.join("samples.bin"))?;
 let t=terrain::Terrain::new(terrain_params);let nx=(dims[0] as f64*size/edge).round() as usize;let nz=(dims[2] as f64*size/edge).round() as usize;
 let mut base=BufWriter::new(fs::File::create(out.join("base-heights.f32"))?);
 for x in 0..=nx{for z in 0..=nz{base.write_all(&(t.base_height(origin[0]+x as f64*edge,origin[2]+z as f64*edge) as f32).to_le_bytes())?;}}base.flush()?;
 println!("Complete {count} records in {:.2}s",started.elapsed().as_secs_f64());Ok(())
}
fn main(){if let Err(e)=run(){eprintln!("{e}");std::process::exit(1);}}
