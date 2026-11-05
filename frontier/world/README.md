# Canonical world smoke fixture

Greenfield implementation (reuse tier G), zero dependencies. No legacy sources
were copied or inspected. Source version 1 is a small deterministic test scene,
not a production terrain or renderer decision. Native cells are 0.0625 m and use
half-open integer bounds. `height_m` returns the base column top; it deliberately
cannot describe bridge occupancy, tunnel air, or edit overrides. Use `material`
for authoritative queries.

Fixture coordinates in meters:

| Feature | Bounds / location |
| --- | --- |
| Quadratic mountain | center x=-3,z=2; radii x=8,z=10; top about y=9 |
| Tunnel | axis x=-3,y=3; radius 1.5; z=-12..16 |
| Wooden bridge deck | x=[2,7), y=[4,4.25), z=[-2,-1) |
| Stone bridge supports | endpoint strips, y=[0,4) |
| Trench below bridge | x=[3,6), z=[-4,0), terrain top y=-1 |

The mountain extends beyond the initial x/z=[-8,8] smoke view. Broad deterministic
integer-interpolated noise supplies the surrounding ground. Cave circumference
uses integer cell addresses; all LODs must derive occupancy from the same source.

`World::clone` shares immutable sparse edits. Mutation copies the edit map when
shared. Effective edits obtain unique revisions across all clones in that source
lineage. Separately constructed worlds need a separate runtime identity in cache
keys: seed/version/revision alone does not identify arbitrary independently
edited worlds. There is no persistence, edit journal, brick storage, undo, or
production streaming yet; copying a large sparse map is not a scalability claim.

Reference raycast normalizes its direction and returns meter distances. It starts
in the forward half-open cell on exact grid boundaries, advances tied axes
together (edge-only contacts do not hit), and chooses the lowest tied axis for
the reported face normal. An initial solid cell has distance zero and zero normal.
Maximum distance is inclusive; invalid/nonfinite rays return no hit. Traversal
is an unaccelerated correctness oracle, unsuitable for long per-pixel rendering.

`LocalOrigin` subtracts integer addresses before producing local GPU floats.
Meter conversion through f64 should remain within 2^53 cells; full i64 coordinates
remain valid for fixture integer sampling but not exact f64 roundtrips. The FBM
source additionally uses f64 meter coordinates internally and shares the 2^53-cell
precision limit; it does not promise distinct adjacent columns beyond that limit.

Run `cargo test -p frontier-world` from the Frontier workspace. Tests cover
negative boundaries, 100 km coordinate precision/raycasting, tunnel continuity,
bridge occupancy, immutable edits/revisions, DDA ties and inclusive endpoints,
and 500 randomized rays against an independent analytic box intersection oracle.

## Procedural FBM renderer test source

`World::fbm(seed, FbmParams::default())` selects an independent, CPU-canonical
mountain/valley heightfield. `World::new` retains the original fixture. Defaults
are 1800 m horizontal scale, 1000 m relief scale, eight octaves and 0.5 octave
amplitude gain. Validated finite parameter ranges are documented on `FbmParams`.
This is a renderer test source, not erosion, rivers, biome generation or caves.
It contains no bridge or fixture tunnel. Sparse edits and reference raycasts
continue to use authoritative native materials.

`FbmTerrain::height_m(x,z)` selects a half-open 6.25 cm native column, evaluates
seeded warped mixed/ridged FBM at its center, then floors its top to the native
vertical lattice. Every solid source cell satisfies `y < height_cells`; the
returned height is the top plane, not the center of the highest occupied cell.
`surface_material` and `surface_color` match the native top cell (grass or stone).
The source has fixed world-meter frequencies and never takes a display spacing;
CPU-generated surface pages sample this same function at every level. Pages are
unedited heightfield proxies and cannot claim to represent arbitrary edits.
`World::column(x,z)` computes the source height once for repeated `material(y)`
queries, retaining exact fixture-volume and sparse-edit behavior.

Noise and fractal composition are greenfield, with no added dependencies or
copied source. Algorithm references: [Perlin's improved-noise reference](https://cs.nyu.edu/~perlin/noise/)
for gradient interpolation and [FastNoiseLite's documentation](https://github.com/Auburn/FastNoiseLite/wiki/Documentation)
for octave frequency/gain and ridged composition concepts. The CPU alone defines
noise; shaders consume uploaded samples. There is no CPU/GPU noise-parity claim,
certified interpolation error, erosion constraint, or omitted-band rejection
bound. The current source uses all configured octaves at every display sample;
proper footprint filtering and geometric-error measurement remain renderer work.
Cache/save identities must include source kind, parameters, seed and version,
plus the existing independent-world/edit revision identity requirements.
