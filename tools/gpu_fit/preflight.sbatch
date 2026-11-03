#!/bin/bash
#SBATCH --job-name=voxel-fit-check
#SBATCH --account=pi_sk2433
#SBATCH --partition=gpu_devel
#SBATCH --qos=normal
#SBATCH --gpus=1
#SBATCH --cpus-per-task=4
#SBATCH --mem=16G
#SBATCH --time=00:15:00
#SBATCH --output=logs/preflight-%j.out
#SBATCH --error=logs/preflight-%j.err
set -euo pipefail
module load Rust/1.86.0-GCCcore-13.3.0
module load PyTorch/2.9.1-foss-2024a-CUDA-12.8.0
export OPENBLAS_NUM_THREADS=1 OMP_NUM_THREADS=4 MKL_NUM_THREADS=4
export RUSTFLAGS=-Adeprecated
export CARGO_HOME="$PWD/.cargo-offline"
export CARGO_TARGET_DIR="$PWD/build"
export PYTHONUNBUFFERED=1
nvidia-smi
cargo build --offline --locked --release --manifest-path gpu_fit/sampler/Cargo.toml
SAMPLER="$PWD/build/release/terrain-sampler"
python gpu_fit/prepare.py --world gpu_fit/fixtures/pilot-world.json --output pilot-train --sampler "$SAMPLER" --samples 262144 --seed 42
python gpu_fit/prepare.py --world gpu_fit/fixtures/pilot-world.json --output pilot-validation --sampler "$SAMPLER" --samples 65536 --seed 1337
python gpu_fit/train.py --world gpu_fit/fixtures/pilot-world.json --train pilot-train --validation pilot-validation --output pilot-result --device cuda --steps 200 --min-steps 200 --evaluate 50 --batch 8192 --preview-width 128 --initialize gpu_fit/fixtures/pilot-initial.json --resume
python gpu_fit/preflight.py "$PWD"
