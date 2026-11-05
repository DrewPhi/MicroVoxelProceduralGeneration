#!/bin/bash
#SBATCH --job-name=voxel-hermite-check
#SBATCH --account=pi_sk2433
#SBATCH --partition=gpu_devel
#SBATCH --gpus=1
#SBATCH --cpus-per-task=4
#SBATCH --mem=16G
#SBATCH --time=00:15:00
#SBATCH --output=logs/hermite-check-%j.out
#SBATCH --error=logs/hermite-check-%j.err
set -euo pipefail
module load Rust/1.86.0-GCCcore-13.3.0
module load PyTorch/2.9.1-foss-2024a-CUDA-12.8.0
export OPENBLAS_NUM_THREADS=1 OMP_NUM_THREADS=4 MKL_NUM_THREADS=4
export RUSTFLAGS=-Adeprecated CARGO_HOME="$PWD/.cargo-offline" CARGO_TARGET_DIR="$PWD/build" PYTHONUNBUFFERED=1
cargo build --offline --locked --release --manifest-path gpu_fit/sampler/Cargo.toml --bins
python gpu_fit/hermite.py test --device cuda
python gpu_fit/prepare.py --world pilot-world.json --output pilot-validation --sampler "$PWD/build/release/terrain-sampler" --samples 65536 --seed 91237
python gpu_fit/hermite.py prepare --world pilot-world.json --output pilot-data --sampler "$PWD/build/release/hermite-sampler" --off 8192 --surface 3500 --seed 77123
python gpu_fit/hermite.py fit --world pilot-world.json --data pilot-data --validation pilot-validation --output pilot-result --device cuda --outer 24 --cg 160 --preview-width 128 --gate
python - <<'PY'
import json
from pathlib import Path
m=json.loads(Path('pilot-result/metrics.json').read_text())
assert m['best_step']>0 and m['validation']['all']['fraction']<=.08
assert m['validation']['all']['fraction']<.5*m['initial']['all']['fraction']
assert m['seams']['occupancy_disagreements']==0
Path('hermite-preflight-passed.json').write_text(json.dumps(m,indent=2))
print('HERMITE GPU LEARNING PREFLIGHT PASSED')
PY
