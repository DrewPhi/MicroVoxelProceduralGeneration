# Independent two-patch shared-target experiment

2026-11-03. Issue #23, local experiment only. No renderer or active world changed.

Run: `python3 tools/fit_seam_targets.py --output output/seam-target-pair-final`
Requires the original `output/fit-occupancy` fixture and built `target/release/fbm_points`.

Two original 8 m cubes, five quadrics each, 6.25 cm native voxels. Both fits receive authoritative FBM face crossings/normals and three native layers of collar occupancy supervision. Coefficients remain independent. Positive branch gauge normalization precedes active-branch least squares. Candidate selection uses tuning samples followed by exhaustive native checks; it allows at most 10% relative interior-error increase and 5% total-error increase. This is fixture optimization, not an independent generalization benchmark.

| Metric | Original | Shared targets |
|---|---:|---:|
| Native errors / 4,194,304 | 29,198 | 29,925 |
| Native error fraction | 0.6961% | 0.7135% |
| Interior errors | 25,994 | 27,157 |
| Three-layer collar errors | 3,204 | 2,768 |
| Shared face disagreements / 16,384 | 1,027 | 877 |
| Outer collar edge errors | 1,551 | 1,332 |
| Missed / extra native face crossing edges | 556 / 587 | 574 / 627 |

Elapsed 14.54 seconds, fitting 3.04 seconds. Selected boundary weight 2, iteration 1. A stronger first run reduced disagreements to 785 but raised total errors to 36,296; rejected as excessive interior damage. The final candidate passes the numerical fidelity guard but introduces a disconnected face component visible in the comparison. Some unsupervised neighboring face disagreements also increase. These results do not establish continuity, topology preservation, or a production seam solution.

`output/seam-target-pair-final/index.html` contains previews and full metrics. Isometric previews sample at 12.5 cm; exhaustive metrics use 6.25 cm. The face plot is more diagnostic than the small overview.

Residual collar occupancy corrections: 2,768 records, 11,072 raw payload bytes versus 480 bytes for the two patches' GPU coefficients; zlib level 9 compresses the record stream to 4,284 bytes, excluding lookup metadata and decode/runtime cost. Records are saved, NOT applied by the renderer. Each u32 packs `(local_xyz_linear_index << 1) | solid`, with index `x + 128*(z + 128*y)` and cropped dimensions XYZ 128,256,128. There are no duplicate correction indices. This only specifies exact occupancy at native collar centers, not continuous seam continuity or safety at the collar's outer edge.

Conclusion: soft common boundary geometry helps measured agreement but is insufficient alone. Keep the original world; next experiments should address boundary topology and evaluate sparse corrections including their outer-edge behavior before scaling up.
