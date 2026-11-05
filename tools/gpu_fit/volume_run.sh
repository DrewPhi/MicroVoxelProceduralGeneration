#!/bin/bash
set -euo pipefail
volume_kind="$1"
module load PyTorch/2.9.1-foss-2024a-CUDA-12.8.0
export OPENBLAS_NUM_THREADS=1 OMP_NUM_THREADS=4 MKL_NUM_THREADS=4 PYTHONUNBUFFERED=1
python gpu_fit/volume_fit.py --kind "$volume_kind" --world world.json \
  --nodes shared-data/nodes.f32 --data train-data --surface geometric-data \
  --validation validation-data --test test-data --output "result-$volume_kind" \
  --device cuda --steps 6000 --seconds 1800 --batch 8192 --surface-batch 2048 \
  --validate-every 250 --resume &
volume_pid=$!
trap 'kill -USR1 "$volume_pid" 2>/dev/null || true' USR1
trap 'kill -TERM "$volume_pid" 2>/dev/null || true' TERM
set +e
while true; do
  wait "$volume_pid"
  volume_status=$?
  if ! kill -0 "$volume_pid" 2>/dev/null; then break; fi
done
exit "$volume_status"
