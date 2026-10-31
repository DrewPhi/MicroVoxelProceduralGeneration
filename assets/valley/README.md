# Frozen valley pass: generator 4, field cache v8

Build with `cargo build --release --manifest-path successor/Cargo.toml -p microvoxel-successor`.
Run `bash assets/valley/capture.sh` for the three ground views, then
`bash output/valley-pass/compare.sh` for the larger budget comparison and overview.
Final launcher build: `bash tools/build_launcher.sh` (passed).

Seed 42 atlas, world scale 10, authoritative voxel edge 6.25 cm. Ground placement
uses authoritative terrain; camera Y is ignored. First/rear share player root
and orientation. Default atlas launch uses this route's start.

| View | Source X,Z (m) | Yaw/pitch | Quality | Requested tiles/memory |
|---|---|---|---|---|
| first / rear | 975761.75,475600.52 | 137 / 3 deg | 95 | 4096 / 384 MiB |
| downstream | 976795.77,474281.46 | 128 / 3 deg | 95 | 4096 / 384 MiB |
| first-highbudget | 975761.75,475600.52 | 137 / 3 deg | 100 | 8192 / 768 MiB |

Game X/Z divides source coordinates by 10. Further downstream source landmarks:
977658.59,473395.25 and 978301.66,472411.87. Other world scales divide the same
source coordinates by their scale while retaining native voxel size.

All ground images are 1280x720, HUD off, three measured offscreen frames, native
radius requested 32 m. They wait for the desired stitched cover to publish;
settled alone does not prove native occupancy. All four final logs also confirm
10/10 actual resident native samples: center, eight points at 6 m and 5 m forward.
Jobs were 0. These ten checks do not prove a continuous native disk.

| View | Visible P95 px | Target px | Active tiles | Cache MiB | Planning native radius |
|---|---:|---:|---:|---:|---:|
| first | 4.15 | 1.04 | 3066 | 179.9 | 31.27 m |
| rear | 4.29 | 1.04 | 2994 | 173.3 | 31.27 m |
| downstream | 4.06 | 1.04 | 3046 | 186.5 | 31.27 m |
| first-highbudget | 3.92 | 1.00 | 3431 | 203.0 | 32.00 m |

The larger budget run encountered memory pressure and reduced its effective tile
budget to 3456. It improved visible P95 by only 0.23 px and did not materially
remove distant hillside facets in this frame. It is not evidence that native
terrain detail improves by simply increasing the requested budget.

Actual adapter: llvmpipe Vulkan CPU software rendering. Reported frame costs are
not laptop GPU performance measurements. Atlas resident data reported 194.11 MiB;
immutable v8 valley field loaded in 2–4 ms, total atlas load around 3 s. Exact
per-run timings and pressure flags are in manifest.json and the full logs.

## Visual review

`rear.png` best shows the final river change: curved branches meet at a shared
junction, replacing the previous straight symmetric fork in `pass1/rear.png`.
`downstream.png` follows a curved central channel; the broad blue hillside strip
visible in pass1 is absent. Native transverse river terraces remain conspicuous.
Foreground color and height variation are modest; distant gray slopes still
show broad facets. `first-highbudget.png` looks close to `first.png` at those
slopes. The full player-scale visual acceptance target is not established.

`overview-2km.png` is the actual 2 km x 2 km game region centered at
97000,47500 m, origin 96000,46500 m. It is 4096x3072, true-scale height, 32 source
meter display cells, 256 mesh tiles, 20.09 MiB mesh data. It shows the drainage
network and broad cliff bands across the whole selected refinement region;
it does not resolve native river occupancy or seams.

Prior generation captures and their review are preserved under `pass1/`.
`manifest.json` records final measurements; `source-sha256.txt` records final
binary, source and immutable field hashes. No shader changes were made for these
captures. The final release and `tools/build_launcher.sh` both succeeded.
