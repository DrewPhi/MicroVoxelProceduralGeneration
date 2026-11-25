# Bounded topology ownership prototype (checkpoint 3)

This is a greenfield (G) early FV23/FV31 smoke experiment. No legacy runtime or
source algorithm was transplanted. It does not pass the full G3 production gate.

`World::fbm_topology(seed, params)` uses the same canonical FBM height generator
plus a tunnel and excavated entrance, a trench and bridge deck/supports, and
sparse canonical add/remove overrides. All topology is inside x/z=[-8,8)m.
`height_m` deliberately continues to expose the underlying unedited landscape
source; callers must not treat it as the topology surface. `column_mesh` rejects
topology sources. The hybrid's ordinary chunks use an independent, unedited FBM
snapshot and are always excluded over the explicit topology owner.

The exact volume is extracted once and retained at every distance, independent
of moving native/proxy cover epochs. Both proxy and ordinary native/collar
fragment paths discard the same closed owner rectangle in canonical coordinates.
The volume owns the whole vertical column, including tunnel air and bridge
undersides. It does not substitute a heightfield for topology when recentering.
Extraction includes one neighboring solid-cell border, then clips exposed quads
to the ownership rectangle. This preserves the outside neighbor wall exposed by
removing the final owned voxel; a sampling-only halo would miss that wall. The
ordinary native path also discards boundary-plane faces to avoid double owners.
Canonical source samples cull all crop caps, including the bottom of the bounded
vertical extraction. The bottom lies below all fixture caves and trenches.

Interactive clicks query canonical DDA; only the bounded owner permits edits.
A background worker builds an immutable candidate world. The world revision and
replacement mesh publish together; gameplay queries keep using the previous
revision during construction. New edits wait for that publication. The unchanged
heightfield chunks remain valid behind the permanent ownership mask. This is a
bounded edit rebuild, not a production incremental edit system.

Reproduce from `frontier/` with the configured Rust toolchain:

```sh
cargo test -p frontier-world topology_ --release -- --nocapture
cargo test -p frontier-renderer volume::tests --release -- --nocapture
cargo run -p frontier-tools --release -- --scene hybrid-topology --camera topology
cargo run -p frontier-tools --release -- --scene hybrid-topology --camera cave --capture /tmp/topology-cave.png --width 1920 --height 1080
cargo run -p frontier-tools --release -- --scene hybrid-topology --camera bridge --capture /tmp/topology-bridge.png --width 1920 --height 1080
```

CPU smoke evidence: seed 7, one FBM octave, 0.0625m exact voxels; cave opening and
real back wall, bridge underside, add/remove cycles, boundary removal, and rays
from moving/far positions compare triangle hit depth/material to canonical DDA.
Every emitted quad is independently checked for canonical solid behind and air
ahead. The three edit revisions produced 132,890–132,914 triangles and
11,162,760–11,164,776 vertex/index bytes; the combined test took 1.95s in release
on this session host. This is CPU correctness evidence, not a GPU timing or an
image-wide coverage oracle. World tests additionally check footprint boundaries,
negative coordinates, unchanged source heights, and canonical tunnel/bridge hits.

Limits: the dense vertical extraction is restricted to this 16x16m fixture and
rejects relief exceeding 2048 cells. Ordinary moving terrain still uses cached
column spans. Retaining exact far topology does not scale to a whole planet.
When the moving native window leaves the topology region, its perimeter meets
the filtered distant proxy directly. Filtered proxy heights can disagree with
canonical boundary tops; no certified distant perimeter seam/depth bound or
image-wide zero-gap claim is made. Feature-aware shared boundary constraints or
a distance-aware fixed guard/collar are still needed before passing full G3.
Appearance quality and the laptop 60 FPS contract remain unproven.
