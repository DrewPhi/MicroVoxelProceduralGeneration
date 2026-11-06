# Loading optimization evidence

See [implementation and interpretation](../../LOADING_OPTIMIZATION.md).
`development-measurements.json` preserves 30 sequential hardware reports with
source/camera, stage and frame timings, quality failures, payload memory and
process peak RSS. Groups are successive development checkpoints, not one frozen
binary or randomized repeated trials. Final memoization/direct-native groups
record the final executable SHA256. Optional direct-native images differ from the
approved default. Default primary float RGB remains byte-identical.

Full raw captures, logs, cached terrain and frozen executables remain ignored in
`frontier/output/`. Reproduction scripts and settings are tracked; no generated
terrain cache is committed. Failed convergence and movement frames remain in the
evidence and must not be counted as acceptance passes.
