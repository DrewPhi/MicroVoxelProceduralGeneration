# Native/proxy appearance handoff experiment

This opt-in experiment reduces a measured appearance discontinuity in the Alpine
foreground. It does not establish that the seam is gone. Existing defaults remain
unchanged; geometry, canonical voxel identity and ownership are unchanged.

Reuse tier: **R/G**. Diagnosis reads the current renderer and its saved captures;
implementation is greenfield work against this repository's current source.
No external implementation or historical runtime was copied.

## Pass 1: isolate the discontinuity

The fixed Alpine capture is 1280×720, seed 7, native voxels 6.25 cm, atmosphere
and terrain shadows off. The reference uses 64 samples/pixel and combines an
exact-native 512 m foreground square with sampled 1 m far columns. Runtime
captures use one sample/pixel and float32 linear color.

In crop `(0,420)–(640,620)`, all 128,000 pixels hit terrain: 65,994 native and
62,006 proxy. Native first appears at row 500; proxy extends through row 533.
The blend diagnostic exposes a shading-policy change: native faces use resolved
face shading, while adjacent small proxies use full area-profile shading.
Within the tighter seam crop, measured baseline blend values are exactly 0 on
14,794 native pixels and exactly 1 on the selected 11,173 small-proxy pixels.
The material diagnostic has closely matching mean colors across these groups.
These observations identify shading/filtering as a contributor to the seam.

## Pass 2: shared support and balanced shading

Shared support lets eligible near proxies use the native one-metre profile
support. Balanced shading retains resolved face response on near proxies and
converges toward full area shading as native cells minify. These change radiance,
not geometry. Independent captures give the following linear RGB RMSE:

| Variant | RMSE | Change from baseline |
|---|---:|---:|
| Baseline | 0.02514113 | — |
| Shared support only | 0.02524995 | +0.43% |
| Balanced shading only | 0.02127104 | −15.39% |
| Both | 0.02117003 | −15.80% |

The comparison holds the mask fixed: `x=0..640, y=480..540`, proxy tile width
≤128 native cells, fully covered reference pixels, reference mean hit distance
<200 m, and runtime terrain hits; 11,173 pixels qualify. All four depth buffers
are bitwise identical. Balanced shading accounts for most of the improvement;
shared support alone does not improve this metric. Combined proxy blend averages
0.5145 in this subset, rather than 1. The earlier frozen-buffer optimal blend
estimate (~0.522) is diagnostic evidence, not a universal fitted weight.

## Pass 3: bounded material filtering

The candidate adds a radius-one native-cell prefilter for soft ground materials,
preserving point-sampled rock contrast. Two directional map taps reduce elongated
sampling artifacts. At map boundaries, both halves of the sampling footprint
contract symmetrically so the filter stays inside the available support instead
of clamping the two taps independently. Shared boundary data and symmetric
contraction improve consistency; they do not certify cross-level appearance.

Run the explicit candidate from `frontier/`:

```sh
FRONTIER_LOD_APPEARANCE=map \
FRONTIER_NEAR_APPEARANCE=shared \
FRONTIER_NEAR_BLEND=balanced \
FRONTIER_MAP_PREFILTER=on \
FRONTIER_MAP_PREFILTER_RADIUS_CELLS=1 \
FRONTIER_MAP_FILTER_SURFACES=soft \
FRONTIER_MAP_ANISOTROPY=2 \
./run.sh --preset alpine
```

All switches are opt-in. Unset them to retain the existing defaults. Diagnostic
captures and commands are under `output/smear-pass/`; offline inspection uses
`tools/smear_diagnostics.py` with float LOD data and a depth-hit mask.

## Final evidence and limits

The final radius-one soft-surface candidate preserves the measured pass-2 seam
error (0.02117003) and identical depth. Radius two also removes grass lattice,
but takes longer to generate. All-material filtering and four-stratum footprint
filtering dull the desired rocky contrast; the selected soft policy leaves stone
nodes point sampled. Two taps only run for elongated map footprints; support is
bounded to four map intervals. This is not a complete anisotropic mip pyramid.

On the Radeon 760M / Ryzen 5 7640U, sequential six-second forward **fly** runs
at 3 m/s, 1920×1080, MSAA 1, uncapped, no atmosphere or terrain shadows, after
settled quality and 60 warmup frames, measured:

| Measurement | Detailed maps before | Selected candidate |
|---|---:|---:|
| Frame interval p50 / p95 / p99 (ms) | 14.36 / 23.57 / 26.05 | 15.15 / 24.13 / 27.16 |
| Isolated GPU mean (ms) | 9.96 | 10.73 |
| Initial preparation (s) | 39.78 | 53.13 |
| Terrain GPU residency (MiB) | 315.16 | 320.42 |

Both runs finished with coverage/quality ready, no benchmark failures or
interruptions. These are single short runs, not a 60-FPS guarantee or isolated
causal performance estimate. Extra source sampling slows initial preparation;
shader taps cost GPU time. See [recorded measurements](reference/gallery/smear-pass/measurements.json)
for commands, camera, memory, clusters, triangles and pending work.

The mandatory fixed spiky comparison is **bitwise unchanged** with default
settings versus the frozen pre-change binary. Applying the Alpine candidate to
that scene increases fully covered reference RMSE from 0.02884 to 0.03235.
Therefore this is a selectable Alpine experiment, not a universal new default.
The intentional appearance-version change is reported as a provenance mismatch
by the strict A/B harness; it is not a source/camera/geometry change.

Validation: 394 workspace tests passed before the final boundary/budget test
additions; the final renderer suite passes 286 tests (9 ignored), and all seven
Python iteration-tool tests pass. Coverage includes renderer shader validation, exact native/shared
profile parity, prefilter work/phase tests, and shared-edge reconstruction
continuity checks. Full float/debug capture buffers remain ignored under output/.

Inspected final gallery:

- [Matched comparison](reference/gallery/smear-pass/comparison.png)
- [Foreground detail crops](reference/gallery/smear-pass/detail-crops.png)
- [False-color LOD classification](reference/gallery/smear-pass/lod-classification.png)

The seam remains visible in pass-2 crops. Profile face integrals and view weighting
approximate radiance; they do not solve exact patch self-occlusion or visibility.
The far reference is sampled, and even the near color comparison includes
sampling differences. This still-image result does not certify geometry,
invisible transitions in motion, distant topology, or 60 FPS. Keep the fixed
player-height primary comparison and cave/bridge checks as independent coverage.
