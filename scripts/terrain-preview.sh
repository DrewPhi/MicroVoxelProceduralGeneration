#!/usr/bin/env bash
# Render the 3D source terrain to output/terrain/{height,shaded,overhangs,iso}.png.
#   scripts/terrain-preview.sh                      # use params/terrain.conf
#   scripts/terrain-preview.sh --watch              # re-render whenever the conf is saved
#   scripts/terrain-preview.sh --overhang-amplitude 24 --iso-extent 48 --iso-yaw 45
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"
exec cargo run --release --quiet -p terrain-tools --bin terrain_preview -- "$@"
