# Alpine image artifact audit

Subsequent reference-driven appearance/LOD changes and their measured limits are
recorded in [ALPINE_LOD_TUNING.md](ALPINE_LOD_TUNING.md). The diagnosis below is
historical; equal-resolution spatial profile boundaries are now shared, while
cross-resolution filtering and exact self-occlusion remain approximate.

Read-only diagnosis, 2026-11-07. Images inspected:
`reference/gallery/alpine/meadow.png` and
`output/alpine-scene/reference-final/color.png`. This note records evidence and
follow-up requirements; it does not certify an implemented fix.

## Ground pinholes

The gallery contains isolated pale foreground pixels: 52 pixels below row 800
exceed the mean RGB brightness of their four direct neighbors by more than 30
8-bit levels. Examples are `(240,803)`, `(432,803)`, `(161,810)`, and
`(1167,851)`; typical grass fleck RGB is approximately `(134,160,140)`.
Their desaturated color alone cannot distinguish background leakage from shading.

A separate diagnostic render found four interior-ground samples with depth
exactly `1` at 1280-pixel width and one MSAA sample. This establishes actual
background coverage holes in that diagnostic, rather than merely unusual
material colors. Reconstructed locations lie on canonical native face
boundaries; greedy-mesh T junction rasterization is the current suspect. This
does not prove that every pale gallery pixel has that cause. The gallery capture
is postprocessed and has no depth attachment available for retrospective proof.

Any repair must preserve canonical exposed surfaces and ownership. Validate with
interior coverage/depth diagnostics, the fixed primary visual comparison, and
matched Alpine captures; inspect both remaining holes and native geometry cost.
Changing terrain color, fog, or appearance filtering cannot repair missing
coverage. No repair is validated by this note.

## Coarse material coverage

`renderer/src/adaptive_geometry.rs` assigns one five-direction appearance profile
to all triangles in each proxy patch (`Generator::patch`). Far patches compute
that profile from approximately 4 by 4 canonical sample points
(`patch_profile`, stride `s / 4`). The result preserves a sampled aggregate but
discards the samples' spatial distribution. Large square steps at rock/turf
boundaries and intermittent path coverage are expected consequences. The current
geometry error certificate does not bound this appearance error.

The offline reference uses 1 m sampled far columns and shows smoother distant
material boundaries. It also shows foreground contour bands and native path
stair steps. Those are consistent with quantized voxel geometry; their presence
alone is not evidence of cracks. The reference is an artistic approximation,
with its own finite-domain and sampling limitations, rather than an exact
full-scene visibility oracle.

Increasing sample count while retaining one constant patch profile cannot remove
its spatial blocks. Naively subdividing patches using newly sampled terrain
midpoints can introduce cracks against adjacent unsplit edges and violate the
existing edge-stride contract. Repainting canonical source materials to hide
these display artifacts would change the world rather than repair the proxy.

## Follow-up appearance work

Develop source-derived spatial appearance pages on a neighbor-consistent,
world-aligned multiresolution grid, including halos and explicit cross-level
filtering. Retain unnormalized directional color/area integrals through
interpolation and normalize once when evaluating radiance. Establish material
coverage and narrow-feature tests independently of geometry error; a narrow path
missed by every source sample cannot be recovered by bilinear filtering.

Budget this work before enabling it. The gallery reports approximately 27 MB of
resident profile data. Expanding every current profile to a header plus sixteen
profiles could approach 459 MB of profile storage alone, before geometry and
other allocations, and adds several storage reads per fragment. A packed spatial
encoding or bounded page residency is needed; measure CPU generation, GPU cost,
cache behavior, and memory rather than assuming the same source-query count makes
the representation inexpensive. Version derived caches and retain Legacy/native
behavior during an explicitly scoped experiment. Do not promote a patch-local
bilinear grid without resolving neighboring-patch continuity and mean/coverage
preservation.

## Implemented repair and measured follow-up

Native packed rectangles now tessellate into matching canonical unit faces.
This preserves the exact face/material/profile set and eliminates the tested
interior T-junction gaps without expanding faces or filling the image afterward.
The reasoning follows the edge-conformity problem discussed in
[Blackflux's meshing article](https://blackflux.wordpress.com/2014/03/02/meshing-in-voxel-engines-part-3/);
implementation is original Rust, with no imported code or triangulation library.
Allocation is bounded before packing; worker reservations account for output,
and derived disk representation version 4 rejects old packed geometry.

Alpine source version 4 adds coherent relief bounded to ±4 cm at default scale.
Appearance version 2 narrows the path, varies its width, and gives it connected,
irregular turf margins. These source changes do not alter Regional or Legacy.
Scalar evaluation, derivatives and conservative affine bounds share the relief
expression. Native heights differ from the prior Alpine source by at most one
6.25 cm cell in the tested survey; the continuous displacement is bounded by
construction, not by the survey.

Validation artifacts are in `output/alpine-fix/`. At 1280×720, MSAA1, no atmosphere,
all pixels below row 503 are ground in this camera. Uncovered samples in that mask
fell from four to zero. The updated capture includes the new Alpine source, so
this is a scene follow-up rather than a source-identical rendering A/B. The
canonical face-set test independently verifies tessellation preserves surfaces.

The source-identical Legacy primary A/B filled 23 previously uncovered samples;
two newly missing samples lie on the skyline. Only 221 of 2,073,600 encoded RGB
pixels differ; p99 depth change on common hits is approximately 1.19e-7 NDC.
Native geometry adds about 23.2 MB GPU residency in that comparison. In the
Alpine diagnostic, native payload grew from 0.52 MB to 6.80 MB; total GPU residency
grew from 76.8 MB to 85.0 MB, including source/selection changes.

A short four-second static 1080p Legacy window check on the Radeon 760M measured
frame-interval p50/p95/p99 of 5.08/7.80/8.28 ms before and 6.30/9.42/10.02 ms after.
This accepts a measured cost for correctness; it is not a motion, streaming,
thermal, or full-game 60 FPS certification. Conforming greedy triangulation could
later reduce this native triangle cost without restoring T junctions.

Spatial material-page work remains deferred; this repair does not claim to remove
all distant material blocks or all physically present voxel steps.
