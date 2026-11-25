# B1 direct coarse height source: bounded negative result

2026-11-07; reuse tier G. This experiment is opt-in and does not alter canonical
source versions, native voxels, renderer selection or saved worlds. The caller
must explicitly invoke `visual_source::generate_visual`. There is no renderer
flag and no candidate is promoted.

The API samples globally aligned native column centers, accepts source and edit
revisions, records seed/complete source parameters, enforces the caller's work
budget, and rejects intersecting edits or volume ownership including a one-cell
halo. Rejected regions need the existing exact/volume representation. Regional
recipes use exact fallback. Legacy truncates only ridge octaves while retaining
the exact two warp calls, broad band and full-source normalization. This is a
source-work reduction, not sparse sampling of an unchanged expensive generator.

The omitted Legacy expression has absolute value <=1. Its omitted height bound
is `0.85 * amplitude * omitted_weight / full_weight + 0.0625m` for nonzero omitted
amplitude. This bound applies to ideal arithmetic plus quantization at each
sample. It is explicitly **not** a certified floating-point enclosure or a
bound on interpolation, unsampled features, normals, materials or silhouettes.
No consumer may substitute it for the adaptive geometric certificate.

## Reproduce

Run `cargo test -p frontier-world`, then
`cargo run --release -p frontier-world --example b1_source_probe` from `frontier/`.
The recorded output is [b1_source_probe.json](b1_source_probe.json). Rust stable
at `/tmp/voxel-rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin` was used with
`CARGO_HOME=/tmp/voxel-cargo RUSTUP_HOME=/tmp/voxel-rustup`.

CPU: AMD Ryzen 5 7640U, 6 cores/12 threads; Linux x86_64. This is single-threaded
CPU page generation, no GPU, no output resolution or frame timings. Seed 7,
source/default parameters, 8 canonical octaves, 6.25cm voxels, 129x129 samples at
16m spacing with column addresses spanning [-1024,1024]m in x/z
(actual sample centers are offset by 0.03125m). Output measures nine repeats after one warmup
for each setting. Noise counts follow the exact source call graph (3+retained
Legacy, 11+8 Regional, 9+8 RegionalUnwarped), exclude materials and lookups, and
are not hardware performance counters. Timings include page allocation, bounds/
ownership validation and sample generation; they exclude all geometry,
appearance, certification, upload and publication.

| Legacy retained octaves | Fewer noise calls | Max sampled error | RMSE | Ideal sample bound | Median source page ms |
|---|---:|---:|---:|---:|---:|
| 2 | 2.20x | 144m | 66.54m | 210.063m | 1.489 |
| 4 | 1.57x | 35.438m | 16.19m | 50.063m | 2.337 |
| 6 | 1.22x | 7.5m | 3.42m | 10.063m | 3.298 |
| 7 | 1.10x | 2.5m | 1.27m | 3.396m | 4.071 |
| 8 (exact) | 1.00x | 0 | 0 | 0 | 5.387 |

Regional and RegionalUnwarped return exact heights and have no reduction in
noise calls. Their fixed warp/province/macro/ridge/foothill work prevents a 2x
reduction by dropping only fine octaves at defaults.

At 1080p, 60-degree vertical FOV and 300m depth, a perpendicular 5px displacement
is approximately `5 * 2 * 300 * tan(30deg) / 1080 = 1.604m`. Even retaining seven
Legacy octaves exceeds that threshold in this survey. At nearer distances the
meter tolerance is smaller. This conversion is illustrative, not image-space
certification: terrain displacement direction, view angle, silhouette and
occlusion matter. Only the exact eight-octave setting passes the survey's
one-voxel max-error check. Point sampling cannot prove full regional quality.

## Decision and next experiment

B1 does not meet the combined >=2x expensive-evaluation and strict quality gate.
Requested-quality adaptive readiness is **unmeasured**, not the source-page
milliseconds above. Do not infer a >20% readiness gain. No draw integration or
new visual capture is justified for this candidate because the source quality
gate already fails; the existing adaptive path remains unchanged.

A potential hook is `renderer/src/column_mesh.rs::ColumnHeights`, consumed by
`adaptive_geometry.rs::native_profile_entry`. That structure contains exact
height and top-material samples with halo and currently supplies both geometry
and appearance profiles. Substituting truncated values there would falsify its
native oracle/appearance contract. A viable future source path needs separate
uncertainty-aware visual data, exact ownership fallback, certified interpolation
and shared-edge rules before integration.

The next lossless hypothesis is sharing canonical column/material samples and
profiles across generation, certification and appearance. Regional material
painting already computes analytic macro-landform derivatives, so measuring
redundant height/landform calls could motivate a combined per-column evaluator.
Batching identical global anchors also avoids repeated full source evaluation.
Neither is implemented or claimed as a measured win by this experiment.
