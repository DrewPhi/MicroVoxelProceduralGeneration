#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."
mkdir -p output/moving-native-checkpoint
for spec in 'walk-off stop-resume 3 20' 'sprint-off stop-resume 15 15' 'return-off turn-return 15 8'; do
    read -r name route speed seconds <<< "$spec"
    ./run.sh --scene hybrid --seed 7 --width 1920 --height 1080 \
        --msaa 4 --voxel-filter off --route "$route" --route-speed "$speed" \
        --route-clock realtime --seconds "$seconds" --warmup 30 --uncapped \
        --report "output/moving-native-checkpoint/$name.json" \
        --capture-on-exit "output/moving-native-checkpoint/$name.png"
done
