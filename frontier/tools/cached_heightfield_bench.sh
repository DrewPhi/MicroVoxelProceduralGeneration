#!/usr/bin/env bash
set -euo pipefail
# Run from frontier/ after building cached_heightfield in release mode.
out=${1:-output/cached-heightfield}
group=${2:-all}
bin=target/release/cached_heightfield
mkdir -p "$out"
run() {
    local name=$1
    shift
    "$bin" --output "$out/$name" "$@" >"$out/$name.log" 2>&1
}
stationary() {
    for w in 320 640 1920; do
        local size=(--width "$w" --height "$((w*9/16))" --cells 2048)
        run "geometry-$w" "${size[@]}" --modes max,dick,dda --bilinear
        run "shaded-$w" "${size[@]}" --modes max,dick,dda --bilinear --shaded
        run "level-$w" "${size[@]}" --modes max,dick,dda,vertical --level --shaded
        run "level-bilinear-$w" "${size[@]}" --modes dda,vertical --level --bands 32 --bilinear --shaded
    done
}
profile() {
    local full=(--width 1920 --height 1080 --modes dick,dda --trials 2 --frames 16)
    run pitch-16 "${full[@]}" --cells 1024 --bilinear --shaded --oracle 256
    run pitch-4 "${full[@]}" --cells 4096 --bilinear --shaded --oracle 256
    run pitched-oracle "${full[@]}" --cells 2048 --bilinear --shaded --oracle 256
    run directed-coarse "${full[@]}" --modes dda --cells 2048 --bilinear \
        --probe 378,467 --probe 1880,617 --probe 1819,605 --probe 1840,609 --probe 1802,602 \
        --probe 674,342 --probe 1547,390 --probe 510,407
    run pitched-counts "${full[@]}" --cells 2048 --modes max,dick,dda --bilinear --instrument
    run stepped-max "${full[@]}" --cells 2048
    run stepped-minmax "${full[@]}" --cells 2048 --minmax
    run mip-4x "${full[@]}" --cells 2048 --bilinear --mip-step 2 --oracle 256
    run native "${full[@]}" --cells 512 --native --shaded --oracle 484
    for band in 16 32 64 128 1080; do
        run "bands-$band" "${full[@]}" --cells 2048 --modes dick,dda,vertical --level --bands "$band" --instrument
    done
}
routes() {
    for route in grazing walk turn high low bank banked revisit; do
        run "route-$route" --width 1920 --height 1080 --frames 36 --cells 2048 \
            --modes dick,dda --bilinear --shaded --route "$route" --oracle 144
    done
    for tier in finite nested; do
        local args=(--bilinear)
        if [[ "$tier" == nested ]]; then args=(--nested); fi
        for route in walk bank; do
            run "video-$tier-$route" --width 960 --height 540 --frames 60 --trials 1 --cells 2048 \
                --modes dda "${args[@]}" --shaded --route "$route" --capture-motion
        done
    done
}
streams() {
    local full=(--width 1920 --height 1080 --cells 2048 --modes dda --shaded --frames 48 --trials 2)
    for route in still revisit teleport; do
        run "stream-$route" "${full[@]}" --stream --route "$route" --oracle 144
    done
    for route in still walk low bank banked revisit teleport; do
        run "nested-$route" "${full[@]}" --nested --route "$route" --oracle 256
    done
    run nested-shifted "${full[@]}" --nested --shift-window --oracle 256
    run gpu-native-audit --width 320 --height 180 --cells 1024 --nested --audit-native --frames 4
    for route in turn bank teleport; do
        for pass in 0 1 2 3; do
            local temporal=()
            if [[ "$pass" == 1 || "$pass" == 2 ]]; then temporal=(--history); fi
            run "history-$route-$pass" "${full[@]}" --nested --route "$route" --trials 1 \
                --instrument --oracle 144 "${temporal[@]}"
        done
    done
}
reference() {
    target/release/landscape --terrain-recipe alpine --seed 7 --center -39,-288 \
        --extent 32 --voxel-size 0.0625 --eye -43.625,31.075,-289.5625 \
        --target -43.325,29.075,-281.5625 --width 1920 --height 1080 --spp 1 \
        --center-sample --near .025 --far 10 --output "$out/native-reference" >"$out/native-reference.log" 2>&1
}
case "$group" in
    stationary) stationary ;;
    profile) profile ;;
    routes) routes ;;
    streams) streams ;;
    reference) reference ;;
    all) stationary; profile; routes; streams; reference ;;
    *) echo "usage: $0 [output] [all|stationary|profile|routes|streams|reference]" >&2; exit 2 ;;
esac
