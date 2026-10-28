#!/usr/bin/env bash
# Dependency-free fallback for systems with rustc but no Cargo.
set -eu
cd "$(dirname "$0")/.."
mkdir -p build
rustc --edition=2021 --crate-name microvoxel --crate-type rlib src/lib.rs -O -o build/libmicrovoxel.rlib
for tool in noise bench; do
  rustc --edition=2021 "src/bin/$tool.rs" -O --extern microvoxel=build/libmicrovoxel.rlib -o "build/$tool"
done
