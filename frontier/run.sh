#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")"
if [[ -x target/release/frontier ]]; then
    exec target/release/frontier "$@"
fi
if ! command -v cargo >/dev/null 2>&1; then
    echo 'No built launcher or Cargo found. Build with Rust 1.99.1: cd frontier && cargo build --release --locked' >&2
    exit 1
fi
exec cargo run --release --locked -p frontier-tools --bin frontier -- "$@"
