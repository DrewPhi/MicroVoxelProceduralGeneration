# Regional material painting checkpoint

Regional and RegionalUnwarped now use appearance version 3. Their elevation
composition remains source version 2, with the same canonical 6.25cm column
heights, native material authority and certified height bounds. Legacy retains
its exact elevation, surface classification, palette and appearance version 0.
No inherited runtime or external algorithm implementation was copied (tier G).

Cover follows the slope of the existing macro, province-masked ridge and
foothill composition. Analytic gradient noise derivatives include the continuous
warp and province-mask derivatives. The fine 40m-and-smaller detail band is
excluded from the cover slope so small grass hummocks do not create a mosaic of
rock. Slope is measured in physical meters and responds to source amplitude and
horizontal scale, independent of display resolution.

Rolling hills and gentle summits stay meadow at all elevations. Broad continuous
moisture at 900m wavelength mildly modulates how much cover steep land retains;
moisture alone cannot create brown or gray lowland patches. Exposed rock requires
a physical slope of approximately 35–39 degrees. Elevation lowers that threshold
by at most 0.045 slope units; there is no altitude-only rock cutoff. Soil occupies
only a narrow 0.045-slope-unit transition immediately below the rock threshold
on dry faces. These are authored cover choices, not erosion or climatic physics.
There is no new material ID or save format. The canonical top voxel has one
material on all faces; the eleven soil voxels below it and deeper stone remain
the existing three-run stratigraphy used by exact face summaries.

The renderer may apply Regional-only lighting/color treatment, but it must keep
native-cell color constant on every face apart from lighting and filter the same
palette in near and far representations. `Material::color()` remains the frozen
shared base palette. Persistent derived cache identities already include recipe,
seed, source parameters, source version and appearance version; old Regional
appearance pages therefore do not alias this paint. Actual water, drainage,
erosion, snow accumulation and material-dependent soil depth are future work.

The final conservative meadow tuning passes `cargo test --locked -p frontier-world`: 34 tests (33 unit and one integration); one existing expensive survey
remains ignored. Coverage
includes Legacy bit fixtures, finite-difference verification of analytic noise
and physical landform gradients at multiple scales, material determinism,
negative native-column addressing, shared near/far source anchors, coherent
adjacent-cell material selection and certified height enclosures. The material
survey covers a 16km square across three seeds and both Regional recipes; a
gentle province is allowed to contain no rock. The updated survey requires meadow
to dominate rather than forcing a minimum count of bare soil and rock.

A sequential optimized CPU microbenchmark of 100,000 seed-7 Regional columns
(317-wide survey, 17.3m/17.9m spacing) measured height-only 62.64ms before and
61.95ms after, with identical height checksum 6832146.875. Height plus surface
classification measured 67.86ms before and 91.72ms after, about 35% more CPU
work for this workload. The new surface classifier adds 11 noise-gradient
samples plus one moisture sample; it avoids four extra full height evaluations.
These are single process timings, not frame-time or sustained performance claims.
Visual judgment and GPU route measurements belong to the renderer integration.
