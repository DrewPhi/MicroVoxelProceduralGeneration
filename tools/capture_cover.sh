#!/usr/bin/env bash
# The README cover is an actual native frame, reproducible without saved settings.
set -euo pipefail
cd -- "$(dirname -- "$0")/.."
mkdir -p output
./native/target/release/microvoxel-game \
    --headless --renderer mesh --seed 42 --size 2560x1440 \
    --quality 100 --native-radius 12 --terrain-memory 512 --tiles 4096 \
    --haze 0.65 --no-hud --frames 60 --camera 4250,200,6250,125,-5.14 \
    --screenshot assets/cover.png > output/cover.log 2>&1
