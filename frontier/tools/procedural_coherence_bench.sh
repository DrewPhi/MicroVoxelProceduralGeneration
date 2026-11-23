#!/usr/bin/env bash
set -euo pipefail

# Run from the Frontier workspace after building frontier and procedural_raycast.
out=${1:-output/procedural-coherence}
group=${2:-all}
mkdir -p "$out"
ray=(target/release/procedural_raycast --gradient-cache --no-repair
  --numerical-fallback --tile 240)
packed=(target/release/frontier --preset alpine --walk off --seed 7
  --voxel-size 0.0625 --msaa 1 --vegetation off --terrain-shadows off --beauty off
  --atmosphere off --fog-density 0 --streaming-haze 0 --no-adaptive-disk-cache
  --capture-far 6000 --naadf-radius-voxels 512 --naadf-follow off
  --eye -43.625,75,-289.5625 --target 0,490,3300 --fov-y 60)

run() {
    local name=$1
    shift
    "${ray[@]}" --output "$out/$name" "$@" > "$out/$name.log" 2>&1
}
stationary() {
    for width in 320 640 1920; do
        local height=$((width*9/16))
        for shade in geometry filtered; do
            local args=(--width "$width" --height "$height" --shading "$shade" --frames 16 --warmup 4)
            run "baseline-$width-$shade" "${args[@]}"
            run "packets-$width-$shade" "${args[@]}" --packets
            run "repair-$width-$shade" "${args[@]}" --packets --compact-repair
            run "preview-$width-$shade" "${args[@]}" --sparse preview
            run "compact-$width-$shade" "${args[@]}" --sparse preview --compact
        done
        local error=7.5
        case $width in
            320) error=1.25 ;;
            640) error=2.5 ;;
        esac
        "${packed[@]}" --width "$width" --height "$height" --pixel-error "$error" \
            --adaptive-wait-quality --benchmark-input-lock --warmup 25 --seconds 7 \
            --report "$out/packed-$width.report.json" --capture-on-exit "$out/packed-$width.png" \
            > "$out/packed-$width.log" 2>&1
    done
}
profile() {
    local small=(--width 640 --height 360 --shading geometry --frames 6 --warmup 2 --instrument)
    local full=(--width 1920 --height 1080 --shading geometry --frames 6 --warmup 2 --instrument)
    run prof-none-640 "${small[@]}"
    run packet-640-profile "${small[@]}" --packets
    run prof-fine-640 "${small[@]}" --fine-bounds
    run prof-both-640 "${small[@]}" --fine-bounds --packets
    run baseline-1920-profile "${full[@]}"
    run packet-1920-profile "${full[@]}" --packets
    run preview-1920-profile "${full[@]}" --sparse preview
    run compact-1920-profile "${full[@]}" --sparse preview --compact
    run certified-640 "${small[@]}" --sparse certified
    # An incomplete output isolates the approximate time spent by the first
    # pass. It is diagnostic and cannot be compared as a rendering result.
    target/release/procedural_raycast --gradient-cache --no-repair --tile 240 \
        "${full[@]}" --output "$out/primary-only-1920" > "$out/primary-only-1920.log" 2>&1
}
reference_checks() {
    local args=(--width 1920 --height 1080 --frames 1 --warmup 0 --oracle 256)
    run baseline-1920-oracle "${args[@]}"
    run packets-1920-oracle "${args[@]}" --packets
    run preview-1920-oracle "${args[@]}" --sparse preview
}
routes() {
    if (( $# == 0 )); then
        set -- baseline packets compact repair
    fi
    for mode in "$@"; do
        local candidate=()
        case $mode in
            packets) candidate=(--packets) ;;
            compact) candidate=(--sparse preview --compact) ;;
            repair) candidate=(--packets --compact-repair) ;;
        esac
        local args=(--width 640 --height 360 --frames 36 --warmup 3 "${candidate[@]}")
        run "$mode-grazing" --width 1920 --height 1080 --frames 12 --warmup 3 \
            --eye -1156,995,2460 --target 900,650,3900 "${candidate[@]}"
        run "$mode-walking" "${args[@]}" --walk --eye -43.625,31.075,-289.5625 \
            --target 0,40,2200 --approach-m 12 --turn-deg 14 --capture-motion
        run "$mode-turning" "${args[@]}" --turn-deg 42 --capture-motion
        run "$mode-flight" "${args[@]}" --approach-m 1100 --turn-deg 36 --capture-motion
    done
}

repair_ablation() {
    for width in 320 640 1920; do
        for shade in geometry filtered; do
            run "repair-$width-$shade" --width "$width" --height "$((width*9/16))" \
                --shading "$shade" --frames 16 --warmup 4 --packets --compact-repair
        done
    done
    local full=(--width 1920 --height 1080 --frames 16 --warmup 4)
    run repair-only-1920 "${full[@]}" --shading geometry --compact-repair
    run repair-preview-1920 "${full[@]}" --sparse preview --compact --compact-repair
    run repair-1920-profile "${full[@]}" --shading geometry --packets --compact-repair --instrument
    run repair-1920-oracle --width 1920 --height 1080 --frames 1 --warmup 0 --oracle 256 \
        --packets --compact-repair
    routes repair
}

case $group in
    all) stationary; profile; reference_checks; routes ;;
    stationary) stationary ;;
    profile) profile ;;
    reference) reference_checks ;;
    routes) routes ;;
    repair) repair_ablation ;;
    *) echo 'usage: bash tools/procedural_coherence_bench.sh [output_dir] [all|stationary|profile|reference|routes|repair]' >&2; exit 2 ;;
esac
