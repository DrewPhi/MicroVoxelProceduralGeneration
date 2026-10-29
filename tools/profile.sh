#!/usr/bin/env bash
# Run serially on an otherwise idle GPU. Results and screenshots stay ignored.
set -euo pipefail
cd -- "$(dirname -- "$0")/.."
profile_dir=${1:-output/performance}
mkdir -p -- "$profile_dir"
game=${MICROVOXEL_GAME:-./native/target/release/microvoxel-game}
"$game" --stream-test > "$profile_dir/streaming.log" 2>&1
cp output/streaming/*.txt output/streaming/*.csv "$profile_dir/"
MICROVOXEL_GAME="$game" ./tools/capture_renderers.sh "$profile_dir"
