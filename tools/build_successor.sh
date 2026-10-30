#!/usr/bin/env bash
# Build the isolated successor and its double-click Linux launcher.
set -eu
cd -- "$(dirname -- "$0")/.."
cargo build --locked --release --manifest-path successor/Cargo.toml -p microvoxel-successor
launcher_tmp=$(mktemp ./MicroVoxel-Successor.XXXXXX.tmp)
trap 'rm -f "$launcher_tmp"' EXIT
MICROVOXEL_ENGINE=successor/target/release/microvoxel-successor \
MICROVOXEL_LOG=output/successor-launcher.log \
    rustc --edition=2021 -O -C strip=symbols tools/launcher.rs -o "$launcher_tmp"
chmod 755 "$launcher_tmp"
mv -f -- "$launcher_tmp" MicroVoxel-Successor
printf 'Ready: %s/MicroVoxel-Successor\n' "$PWD"
