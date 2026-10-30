//! Offline previews, reproducible benchmarks and experimental representations.
//! Normal gameplay uses these only when an explicit diagnostic flag is supplied.
pub mod benchmark;
pub mod brick_study;
pub mod character_preview;
pub mod editing_check;
pub mod lod_check;
pub mod quality;
pub mod ray_study;
mod ray_study_gpu;
pub mod streaming_check;
