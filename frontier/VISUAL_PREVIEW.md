# Interactive visual preview

From `frontier/`, launch `./run.sh`. The default preview uses the adaptive
renderer and Regional terrain, with 6.25cm native voxels, voxel appearance
filtering and 4×MSAA. Rebuild with `cargo build --release --locked` after source
changes: the launcher prefers an existing release binary.

Press **Esc** to open or close settings. Sun elevation/azimuth, exposure, haze
density/height falloff, cloud coverage/speed and overcast update live. **Clear air
· disable atmosphere** removes atmospheric compositing for an unobscured terrain
check. Clouds are a placeholder sheet, not volumetric weather or weather physics.

Image quality and loading controls take effect with **Apply quality & loading**.
Applying rebuilds the streaming configuration while retaining the current seed
and camera pose; allow terrain to refine after a change. The overlay shows frame
time, pending work, refinement readiness and memory counters.

| Control | Effect |
|---|---|
| Terrain error (px) | Lower values request finer geometry; this may reduce FPS. |
| View distance (m) | Changes how far terrain coverage is requested. |
| Render scale | Sets internal scene resolution; the settings overlay stays at output resolution. |
| MSAA | Selects off or 4× edge coverage sampling. |
| Filter distant voxel detail | Filters distant appearance while keeping canonical voxel identity. |
| Workers | Sets CPU preparation concurrency within the available worker limit. |
| Refinements / batch | Sets refinement selections per coherent loading transaction. |
| Cache budget (MiB) | Sets the in-memory terrain cache budget. |
| Upload / frame (MiB) | Sets the per-frame upload budget. |

Click the view to capture the mouse. Use WASD to move, the mouse to look,
Space/Ctrl to move vertically and Shift to accelerate. This is a free-fly
preview without player collision. Press **F12** to save a PNG capture.

The landforms are the existing Regional composition. The new painting derives
grass, soil and exposed rock from landform slope, elevation and continuous
moisture; it does not generate independent brown/gray lowland patches. Each
native voxel keeps one material on all its faces apart from lighting. Legacy
remains available with `--terrain-recipe legacy`; see the
[painting checkpoint](REGIONAL_PAINTING.md) for source identity and tests.

This checkpoint focuses on visual review; optimization is postponed. The workspace passes 276 tests (7 deliberately ignored), including shader validation.
The fixed Legacy player-view check differs from the previous baseline in only
two pixels by one 8-bit level; geometry and source remain unchanged.
GPU menu smoke checks exercised Esc, Resume and changing MSAA through Apply.
A separate GPU capture verified clear air at 75% render scale with MSAA off.
This is not a sustained performance or production-quality certification. Water, drainage simulation and weather physics are not implemented.


Final preview captures (generated locally, ignored by Git):

- `output/visual-preview/player-final.png`: seed 7, 1280×720, 4×MSAA,
  3px requested terrain error, 4km clip, eye 1.7m above terrain.
- `output/visual-preview/aerial-final.png`: same source, aerial camera, 6km clip.

These show the current Regional landforms with conservative meadow cover.
Bare rock requires steep slopes; altitude alone no longer paints a hill gray.
