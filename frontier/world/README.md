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
remain valid for canonical integer sampling but not exact f64 roundtrips.

Run `cargo test -p frontier-world` from the Frontier workspace. Tests cover
negative boundaries, 100 km coordinate precision/raycasting, tunnel continuity,
bridge occupancy, immutable edits/revisions, DDA ties and inclusive endpoints,
and 500 randomized rays against an independent analytic box intersection oracle.
