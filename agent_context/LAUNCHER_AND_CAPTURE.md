# Desktop launcher and maximum-quality capture — 2026-10-28

`tools/build_launcher.sh` builds the small native Linux `MicroVoxel` executable
and a local `MicroVoxel.desktop` entry. The executable finds the repository from
its own location, launches the current release engine without opening a terminal,
uses the repository as its working directory and saves diagnostics in
`output/launcher.log`. Both generated entry points are ignored; source and the
SVG icon are maintained. There are no new Rust dependencies. A trusted shortcut
was installed at `/home/drew/Desktop/MicroVoxel.desktop` on this machine.

Capture controls: `--size WIDTHxHEIGHT`, `--camera X,Y,Z,YAW,PITCH` and `--no-hud`.
Coordinates are meters; angles are degrees. Explicit cameras enable flight and
snap to integer native cells. Validation rejects non-finite/out-of-world positions,
near-vertical pitches and excessive output dimensions. Hiding the crosshair
preserves sky, shadows, haze and the normal terrain pipeline. Explicit-camera
headless exports build the final requested cover directly, avoiding temporary
interactive layouts, with a longer bounded load deadline and periodic progress.
Backpressure skips request sorting when the bounded worker queue is already full.
The interactive initial-ready flag also waits for its background plan, so startup
does not accidentally enable fades before the first final cover. Saved preferences
are untouched by headless captures.

The native test suite includes camera/size validation. Launcher compilation,
argument forwarding, actual GPU execution, shell syntax and desktop-entry
validation were checked. `gio` metadata on this desktop uses string-valued
`metadata::trusted=true`, not a boolean-valued attribute.

Scenic viewpoints were selected from actual seed-42 terrain height/climate
samples and previewed with the native renderer. Final PNGs and reproduction
commands live in ignored `screenshots/`. They are real engine captures, not
AI-generated or retouched images. Maximum requested graphics remain subject to
normal adaptive LOD and residency limits.

Final capture: `screenshots/max_graphics_alpine_lake.png`, 2560x1440, camera
[1250, 567.8125, 8750] m, yaw 22.5 / pitch -6 degrees, detail 100, native radius
32 m, terrain cache 1024 MiB and tile limit 8192. Requested/effective cell size
1.00/3.05 px; 7654 tiles, 469.54 MiB retained meshes. Direct final-cover export
finished in 21.17 s; the earlier interactive-layout export hit its 180 s timeout.
The final five synchronized offscreen frames averaged 43.79 ms at this view on
Radeon 760M. These maximum settings are intentionally more expensive than the
balanced interactive default. The PNG passed decoding/dimension checks and was
visually inspected. Temporary scouting renders and the stale failed-export error
file were removed after the final capture succeeded.

An additional greener valley composition is saved as
`screenshots/max_graphics_valley.png`, also 2560x1440 at the same maximum settings.
Camera [4250, 200, 6250] m, yaw 125 / pitch -5.14 degrees. It prepared 6303 tiles
in 15.28 s, retaining 171.80 MiB of terrain meshes. Its reproduction command is
saved beside the PNG. Both final images were decoded and visually checked.
