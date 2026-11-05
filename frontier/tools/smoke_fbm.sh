#!/usr/bin/env bash
set -euo pipefail

TOOLS_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
FRONTIER_DIR="$(cd -- "$TOOLS_DIR/.." && pwd)"
BIN="$FRONTIER_DIR/target/release/frontier"
OUT_DIR="$FRONTIER_DIR/output/fbm-checkpoint"
WIDTH="${FBM_WIDTH:-1920}"
HEIGHT="${FBM_HEIGHT:-1080}"

if [[ ! -x "$BIN" ]]; then
    printf 'Missing release binary: %s\nBuild it with: cd %q && cargo build --release --locked -p frontier-tools\n' \
        "$BIN" "$FRONTIER_DIR" >&2
    exit 1
fi

if [[ ! "$WIDTH" =~ ^[0-9]+$ || ! "$HEIGHT" =~ ^[0-9]+$ ]] || \
    (( WIDTH < 1 || WIDTH > 8192 || HEIGHT < 1 || HEIGHT > 8192 )); then
    printf 'FBM_WIDTH and FBM_HEIGHT must be integers in 1..=8192 (got %s x %s)\n' \
        "$WIDTH" "$HEIGHT" >&2
    exit 2
fi

mkdir -p -- "$OUT_DIR"

run_capture() {
    local name="$1"
    shift
    "$BIN" "$@" --width "$WIDTH" --height "$HEIGHT" \
        >"$OUT_DIR/$name.log" 2>&1
}

run_capture fbm-seed7-valley \
    --scene fbm --seed 7 --camera valley \
    --capture "$OUT_DIR/fbm-seed7-valley.png"

run_capture fbm-seed7-aerial \
    --scene fbm --seed 7 --camera aerial \
    --capture "$OUT_DIR/fbm-seed7-aerial.png"

run_capture fbm-seed42-aerial \
    --scene fbm --seed 42 --camera aerial \
    --capture "$OUT_DIR/fbm-seed42-aerial.png"

run_capture fixture-seed7 \
    --scene fixture --seed 7 \
    --capture "$OUT_DIR/fixture-seed7.png"

"$BIN" \
    --scene fbm --seed 7 --camera valley \
    --frames 360 --route stop-resume --route-step 0.016666666666666666 \
    --warmup 30 --uncapped --report "$OUT_DIR/stop-resume-seed7.json" \
    --width "$WIDTH" --height "$HEIGHT" \
    >"$OUT_DIR/stop-resume-seed7.log" 2>&1

printf 'FBM smoke sweep complete. Artifacts: %s\n' "$OUT_DIR"
