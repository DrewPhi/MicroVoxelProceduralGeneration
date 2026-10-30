# Volcan Mountain real-terrain experiment

Source: Marks, M., Coulter, L., Madonia, S., Baer, N., Franklin, J., Sousa, D.
(2026). **UAV Lidar Survey of Volcan Mountain Wilderness Preserve, CA 2024**.
Distributed by OpenTopography. [DOI: 10.5069/G99P2ZV5](https://doi.org/10.5069/G99P2ZV5).
License: [Creative Commons Attribution 4.0](https://creativecommons.org/licenses/by/4.0/).
Retrieved 2026-10-29. [Dataset metadata](https://portal.opentopography.org/datasetMetadata?otCollectionID=OT.042026.32611.1).

The provider lists the survey date as 2024-10-25. Its actual raster filenames
contain `20231025`; we preserve those published filenames and record both rather
than inventing a corrected acquisition date.

## Local crop and modifications

[crop.json](crop.json) records original URLs, EPSG:32611 bounds, source elevation
range, local vertical offset, selected spawn and the imported file's SHA-256.
The 200 m square contains 4,000 × 4,000 original 5 cm samples. Runtime X points
east and Z points south. Y is original WGS84 ellipsoidal elevation minus 1,107 m;
this translation retains actual relief without vertical exaggeration. Occupancy
heights round down to complete 5 cm cells.

The matching orthophoto has a slightly different origin. The importer aligns
native pixel centers in projected coordinates and verifies grid agreement and
coverage. Each photographed RGB is quantized to RGB565 to fit the existing
four-byte voxel value. The same stored base color appears on every face. This
is photographed appearance, not measured material identity; the image contains
baked sunlight/shadows. Subsurface column colors are extended from the surface.

The DSM includes vegetation. Trees/bushes become solid height columns, without
hollow crowns, trunks or branches. The selected spawn is on open dry grass, but
this is not a vegetation-filtered bare-earth terrain or full 3D reconstruction.

Large data/captures remain ignored under `output/earth/`. The 96 MB `.mvearth`
file contains height/color samples, not all solid voxels. It needs no network or
Python at launch. Runtime builds a filtered height/color/extrema pyramid, measured
at 162.76 MiB payload for this crop, separately from mesh/GPU caches.

## Reproduce and launch

With Python rasterio/numpy available, run from the repo root:

```sh
python3 tools/import_volcan.py
./tools/build_launcher.sh
./MicroVoxel --world earth
# Explicit alternatives:
./MicroVoxel --world earth
./MicroVoxel --world procedural
./MicroVoxel --world earth --fly
```

The importer uses Cloud Optimized GeoTIFF HTTP range reads rather than fetching
the two approximately 3 GB source rasters in full. Launch defaults to procedural
terrain. Select the local crop explicitly using `--world earth` or the World menu;
`--earth PATH` loads another compatible import. Missing explicitly requested data
produces an error rather than silently switching worlds.

Tests cover invalid samples, native occupancy/run agreement, filtered colors,
edited/moved color preservation and source-specific save compatibility. Real GPU
captures and the walking/jumping/F5-view test are in `output/earth/`. New imported
appearance uses material schema 3; older schema-2 edit saves are incompatible.
