#!/bin/bash
#SBATCH --job-name=voxel-volume-data
#SBATCH --account=pi_sk2433
#SBATCH --partition=day
#SBATCH --cpus-per-task=16
#SBATCH --mem=32G
#SBATCH --time=02:00:00
#SBATCH --output=logs/volume-data-%j.out
#SBATCH --error=logs/volume-data-%j.err
set -euo pipefail
module load Rust/1.86.0-GCCcore-13.3.0
module load PyTorch/2.9.1-foss-2024a-CUDA-12.8.0
export RUSTFLAGS=-Adeprecated CARGO_HOME="$PWD/.cargo-offline" CARGO_TARGET_DIR="$PWD/build"
export OPENBLAS_NUM_THREADS=1 OMP_NUM_THREADS=1 MKL_NUM_THREADS=1 PYTHONUNBUFFERED=1
cargo build --offline --locked --release --manifest-path gpu_fit/sampler/Cargo.toml --bin node-sampler
python gpu_fit/volume_prepare.py --world world.json --output shared-data --sampler "$PWD/build/release/node-sampler"
