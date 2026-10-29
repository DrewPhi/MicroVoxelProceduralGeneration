#!/usr/bin/env bash
# Optional shareable formats for the native exporter's actual rendered APNGs.
# Requires ffmpeg; the Rust runtime itself does not depend on it.
set -eu
preview_dir=${1:-output/character}
for clip in idle walk jump wave; do
  ffmpeg -hide_banner -loglevel error -ignore_loop 1 -i "$preview_dir/$clip.png" \
    -filter_complex '[0:v]split[a][b];[a]palettegen[p];[b][p]paletteuse=dither=none' \
    -loop 0 -y "$preview_dir/$clip.gif"
  ffmpeg -hide_banner -loglevel error -ignore_loop 1 -i "$preview_dir/$clip.png" \
    -c:v libx264 -pix_fmt yuv420p -crf 18 -movflags +faststart -y "$preview_dir/$clip.mp4"
done
