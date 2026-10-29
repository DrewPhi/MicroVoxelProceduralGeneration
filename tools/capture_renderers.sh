#!/usr/bin/env bash
# Matching viewpoints/settings; headless exports do not change saved preferences.
set -euo pipefail
cd -- "$(dirname -- "$0")/.."
capture_dir=${1:-screenshots/renderer_comparison}
mkdir -p -- "$capture_dir"
game=${MICROVOXEL_GAME:-./native/target/release/microvoxel-game}
common=(--headless --seed 42 --size 1920x1080 --quality 75 --native-radius 6
        --terrain-memory 256 --tiles 768 --no-hud --frames "${MICROVOXEL_FRAMES:-60}")
for renderer in mesh ray; do
    "$game" "${common[@]}" --renderer "$renderer" \
        --camera 4250,200,6250,125,-5.14 \
        --screenshot "$capture_dir/${renderer}_valley.png" \
        > "$capture_dir/${renderer}_valley.log" 2>&1
    "$game" "${common[@]}" --renderer "$renderer" \
        --camera 4250,200,6250,125,-2 --camera-ground --view rear \
        --screenshot "$capture_dir/${renderer}_ground.png" \
        > "$capture_dir/${renderer}_ground.log" 2>&1
done
