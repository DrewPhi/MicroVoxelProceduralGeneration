#!/bin/bash
#SBATCH --job-name=voxel-world-1km
#SBATCH --account=pi_sk2433
#SBATCH --partition=gpu
#SBATCH --qos=normal
#SBATCH --gpus=1
#SBATCH --cpus-per-task=8
#SBATCH --mem=32G
#SBATCH --time=02:00:00
#SBATCH --signal=B:USR1@180
#SBATCH --output=logs/world-%j.out
#SBATCH --error=logs/world-%j.err
set -euo pipefail
module load Rust/1.86.0-GCCcore-13.3.0
module load PyTorch/2.9.1-foss-2024a-CUDA-12.8.0
export OPENBLAS_NUM_THREADS=1 OMP_NUM_THREADS=8 MKL_NUM_THREADS=8
export RUSTFLAGS=-Adeprecated
export CARGO_HOME="$PWD/.cargo-offline"
export CARGO_TARGET_DIR="$PWD/build"
export PYTHONUNBUFFERED=1
# Dependency AND explicit marker: never run the large fit after a bad pilot.
test -s preflight-passed.json
nvidia-smi
cargo build --offline --locked --release --manifest-path gpu_fit/sampler/Cargo.toml
SAMPLER="$PWD/build/release/terrain-sampler"
python gpu_fit/prepare.py --world world.json --output train-data --sampler "$SAMPLER" --samples 16777216 --seed 4242
python gpu_fit/prepare.py --world world.json --output validation-data --sampler "$SAMPLER" --samples 1048576 --seed 90210
# Run in the background so the shell forwards Slurm's early warning to Python.
python gpu_fit/train.py --world world.json --train train-data --validation validation-data --output result --device cuda --steps 40000 --min-steps 4000 --batch 65536 --evaluate 500 --patience 30 --seconds 4500 --preview-width 512 --resume &
fit_pid=$!
trap 'kill -USR1 "$fit_pid" 2>/dev/null || true' USR1
trap 'kill -TERM "$fit_pid" 2>/dev/null || true' TERM
# A trapped signal interrupts wait; wait again until checkpointing has finished.
set +e
fit_status=0
while true; do
    wait "$fit_pid"
    fit_status=$?
    if ! kill -0 "$fit_pid" 2>/dev/null; then break; fi
done
exit "$fit_status"
