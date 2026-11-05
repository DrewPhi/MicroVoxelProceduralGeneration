#!/bin/bash
#SBATCH --job-name=voxel-hermite-1km
#SBATCH --account=pi_sk2433
#SBATCH --partition=gpu
#SBATCH --gpus=1
#SBATCH --cpus-per-task=8
#SBATCH --mem=32G
#SBATCH --time=02:00:00
#SBATCH --signal=B:USR1@180
#SBATCH --output=logs/hermite-world-%j.out
#SBATCH --error=logs/hermite-world-%j.err
set -euo pipefail
module load Rust/1.86.0-GCCcore-13.3.0
module load PyTorch/2.9.1-foss-2024a-CUDA-12.8.0
export OPENBLAS_NUM_THREADS=1 OMP_NUM_THREADS=8 MKL_NUM_THREADS=8
export RUSTFLAGS=-Adeprecated CARGO_HOME="$PWD/.cargo-offline" CARGO_TARGET_DIR="$PWD/build" PYTHONUNBUFFERED=1
# Require an actually learned cave fit, not merely a good warm start.
test -s hermite-preflight-passed.json
cargo build --offline --locked --release --manifest-path gpu_fit/sampler/Cargo.toml --bins
python gpu_fit/hermite.py prepare --world world.json --output geometric-data --sampler "$PWD/build/release/hermite-sampler" --off 1048576 --surface 524288 --seed 77123
# Isolated validation and untouched test sets, both authoritative native centers.
python gpu_fit/prepare.py --world world.json --output validation-data --sampler "$PWD/build/release/terrain-sampler" --samples 1048576 --seed 90210
python gpu_fit/prepare.py --world world.json --output test-data --sampler "$PWD/build/release/terrain-sampler" --samples 1048576 --seed 11937
python gpu_fit/hermite.py fit --world world.json --data geometric-data --validation validation-data --test test-data --output result --device cuda --outer 32 --cg 180 --seconds 4500 --preview-width 512 --resume &
fit_pid=$!
trap 'kill -USR1 "$fit_pid" 2>/dev/null || true' USR1
trap 'kill -TERM "$fit_pid" 2>/dev/null || true' TERM
set +e
while true; do
    wait "$fit_pid"
    fit_status=$?
    if ! kill -0 "$fit_pid" 2>/dev/null; then break; fi
done
exit "$fit_status"
