# Vegetation disappearing on camera turns — 2026-11-08

Follow-up to `641b002`. The earlier walk/73-degree turn check was insufficient:
it did not adequately exercise cached trees competing with nearby ground cover
through full yaw and pitch sweeps. This is a selector/accounting repair, not a
change to canonical vegetation, density, pigments, meshing, shaders or voxel size.

## Reproduced failures

A stationary player at the normal Alpine spawn performed a full yaw circle,
looked down, reversed the yaw circle and looked up, then returned to the original
view. Before the fix, native ground-patch count fell from 12 to 1. Two grass
patches 3.58 m and 5.46 m away ended with no selected mesh and zero queued jobs.

1. Tree refinement respected its 65% class ceiling but ignored existing ground
   reservations. Looking toward already cached detailed trees spent that reserved
   room. Ground detail could coarsen or vanish even though the eye had not moved.
2. A missing, known-cost ground baseline had no reservation. If it was too
   expensive to queue, the global coverage flag could remain false indefinitely,
   preventing unrelated near patches from refining with no work pending.

Tree refinement now also respects the unreserved global remainder after both
classes' reservations. An affordable evicted baseline reserves its known cost
while rebuilding; an unaffordable baseline cannot globally freeze refinement.
Nearby detail stays within the same CPU/triangle budgets, rather than raising
limits or disabling culling to hide the bug.

## Verification

- The regression `turning_toward_cached_tree_cannot_spend_retained_native_flower_reservations`
  failed before the change (four native flower patches became one at 120 degrees)
  and passes afterward.
- `unaffordable_missing_far_baseline_does_not_freeze_near_refinement` verifies
  progress despite an impossible distant baseline.
- All 535 workspace tests pass, including 408 renderer tests; 15 opt-in tests
  remain ignored. The release launcher is rebuilt.
- Actual X11 full yaw/pitch recordings at 1920×1080/MSAA4 use seed 7, Alpine,
  6.25 cm canonical voxels, 4.5 px terrain error, 20-second preload, and matching
  resource budgets. CPU/GPU terrain: 700/466 MiB; build: 700 MiB; CPU/GPU plants:
  186/280 MiB; requested triangles: 4 million; near plants: 12 m; native band: 4 m.
- Before: native patches dropped 12→1; 203 selected ground patches, two missing,
  16 coarse fallbacks. After: all 12 native patches remain through the sweep;
  205 selected ground patches, none missing, zero coarse fallbacks or queued jobs.
- Matching returned-view frames were inspected. The fixed Legacy primary repeat
  captures match byte-for-byte in PNG, depth and float color. The first old
  capture differed at four color pixels from both repeat captures; the unchanged
  old binary reproduced that variation, so no terrain change is inferred from it.

[Returned-view comparison](reference/gallery/vegetation-turn-fix/returned-view.png)
· [Commands, hashes and counters](reference/gallery/vegetation-turn-fix/measurements.json)
· [After video](output/vegetation-turn-fix/after-turn/walking.mp4)

Reproduce a stationary turn recording from `frontier/`:

```sh
FRONTIER_VEGETATION_TRACE=1 python3 tools/record_walk.py --turn-only --width 1920 --height 1080 --settle-seconds 20 --output output/my-turn-check -- --preset alpine --resource-profile development --pixel-error 4.5
```

The retained before/after manifests contain exact budget overrides and binary
hashes. Recording runs at 20 FPS, perturbs timings and can miss subframe defects;
this is visual/selection evidence, not a new FPS benchmark or a guarantee for
every seed. Distant tree detail can still vary under finite budgets. Reuse tier
**G**: changes stay within Frontier's existing reservation algorithm; no external
or legacy runtime code was copied.
