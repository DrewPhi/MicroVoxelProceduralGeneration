#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
binary=successor/target/release/microvoxel-successor
common=(--world atlas --seed 42 --world-scale 10 --headless --renderer mesh --quality 95 --native-radius 32 --tiles 4096 --terrain-memory 384 --size 1280x720 --frames 3 --camera-ground --no-hud)
"$binary" "${common[@]}" --camera 97576.175,273.64,47560.052,137,3 --view first --screenshot output/valley-pass/first.png > output/valley-pass/first.log 2>&1
"$binary" "${common[@]}" --camera 97576.175,273.64,47560.052,137,3 --view rear --screenshot output/valley-pass/rear.png > output/valley-pass/rear.log 2>&1
"$binary" "${common[@]}" --camera 97679.577,273.64,47428.146,128,3 --view first --screenshot output/valley-pass/downstream.png > output/valley-pass/downstream.log 2>&1
sha256sum "$binary" successor/world/src/generation/atlas/refine.rs successor/world/src/generation/atlas/mod.rs successor/world/src/render/coherent.rs successor/viewer/src/camera.rs > output/valley-pass/source-sha256.txt
