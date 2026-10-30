#!/usr/bin/env bash
# Build the release binary first. Run serially on an otherwise idle GPU.
set -euo pipefail
cd -- "$(dirname -- "$0")/.."
check_dir=${1:-output/foundation}
game=${MICROVOXEL_GAME:-./successor/target/release/microvoxel-successor}
mkdir -p -- "$check_dir"
"$game" --stream-test > "$check_dir/streaming.log" 2>&1
cp output/streaming/*.txt output/streaming/*.csv "$check_dir/"
"$game" --terrain-quality "$check_dir/quality" > "$check_dir/quality.log" 2>&1
"$game" --seed 2027 --terrain-quality "$check_dir/holdout" > "$check_dir/holdout.log" 2>&1
"$game" --headless --voxel-size 1/100 --player-test --frames 1 \
    --quality 20 --native-radius 2 --terrain-memory 128 --tiles 128 \
    --screenshot "$check_dir/centimeter_player.png" > "$check_dir/centimeter_player.log" 2>&1
