# Regional lighting oracle smoke test

This is a **320×180 crop of a 1920×1080 camera**, not a resized full scene or a
new approved artistic target. It verifies the new independent CPU Regional
lighting path on exact 6.25 cm columns. The Legacy oracle remains unchanged.

64 stratified samples/pixel; seed 7, source version 2, appearance version 3;
Regional sun elevation 35°, azimuth 135°, overcast 0.15; atmosphere off. The
finite native domain covers x[-32,32), z[0,64) metres with unbounded solid
columns below the surface. All sampled rays hit inside that domain:
`finite_domain_miss_fraction = 0`. This does not certify the rest of the view,
caves, proxy filtering, shadows or atmospheric lighting.

The image shows nearby grass and individual terrace side faces. It is a shader
contract smoke test, not a claim that new scenery or transitions look better.
CPU lighting uses f64 arithmetic; GPU f32 and pixel sampling need tolerances in
later image comparisons. Native material/color comes from canonical data, not
runtime proxy summaries.

From `frontier/` after building `frontier-reference --bin landscape`:

```sh
mkdir -p output/issue74/regional-oracle-near
target/release/landscape --output output/issue74/regional-oracle-near --cache output/issue74/regional-oracle-near/native-columns.bin --terrain-recipe regional --lighting regional --sun-elevation 35 --sun-azimuth 135 --overcast .15 --seed 7 --center 0,32 --extent 64 --eye 0,93.95,0 --target 0,93.87008531838656,0.9968017339706421 --fov-y 60 --near .025 --far 1000 --width 1920 --height 1080 --crop 800,800,320,180 --spp 64 --sample-seed 19 --threads 4
```

The committed manifest records the actual original cache build/render timing;
the run reused the same native cache from the preceding 64×64 smoke crop.
