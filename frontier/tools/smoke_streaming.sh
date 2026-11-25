#!/usr/bin/env bash
set -euo pipefail

TOOLS_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
FRONTIER_DIR="$(cd -- "$TOOLS_DIR/.." && pwd)"
BIN="$FRONTIER_DIR/target/release/frontier"
OUT_DIR="$FRONTIER_DIR/output/streaming-checkpoint"
WIDTH="${STREAMING_WIDTH:-1920}"
HEIGHT="${STREAMING_HEIGHT:-1080}"
DURATION="${STREAMING_SECONDS:-15}"

if [[ ! -x "$BIN" ]]; then
    printf 'Missing release binary: %s\nBuild it with: cd %q && cargo build --release --locked -p frontier-tools\n' \
        "$BIN" "$FRONTIER_DIR" >&2
    exit 1
fi

if [[ ! "$WIDTH" =~ ^[0-9]+$ || ! "$HEIGHT" =~ ^[0-9]+$ ]] || \
    (( WIDTH < 1 || WIDTH > 8192 || HEIGHT < 1 || HEIGHT > 8192 )); then
    printf 'STREAMING_WIDTH and STREAMING_HEIGHT must be integers in 1..=8192 (got %s x %s)\n' \
        "$WIDTH" "$HEIGHT" >&2
    exit 2
fi

if [[ ! "$DURATION" =~ ^[0-9]+$ ]] || (( DURATION < 1 || DURATION > 3600 )); then
    printf 'STREAMING_SECONDS must be an integer in 1..=3600 (got %s)\n' "$DURATION" >&2
    exit 2
fi

mkdir -p -- "$OUT_DIR"

run_route() {
    local route="$1"
    "$BIN" \
        --scene fbm --seed 7 --camera valley \
        --seconds "$DURATION" --route "$route" --route-clock realtime \
        --warmup 30 --uncapped \
        --capture-on-exit "$OUT_DIR/$route-seed7.png" \
        --report "$OUT_DIR/$route-seed7.json" \
        --width "$WIDTH" --height "$HEIGHT" \
        >"$OUT_DIR/$route-seed7.log" 2>&1
}

run_route stop-resume
run_route turn-return

printf 'Streaming smoke sweep complete. Artifacts: %s\n' "$OUT_DIR"
