# Bounded canonical face-summary GPU experiment (#71)

`renderer/src/gpu_summary_probe.rs` and `gpu_summary_probe.wgsl` are a greenfield,
standalone experiment. No production rendering, source generation or geometry
construction calls this path. The exact-appearance CPU stage remains unchanged.

Inputs are CPU canonical unedited FBM height/material snapshots for one to eight
128×128 native-column pages with one-column halos. Canonical integer heights are
rebased to a page-local minimum and sent with the exact deep/soil/top material
IDs sampled from `WorldColumn`; the GPU never evaluates floating-point FBM.
Topology owners and edits are rejected explicitly. Each page retains its source
seed, parameters and revision for independent oracle checks. Input generation
and upload costs are reported separately from resident reuse.

The shader emits 25 exact integer exposed-face counts per patch: five face axes
(+Y,+X,-X,+Z,-Z) by five canonical materials. Patches are row-major, with configurable widths of 16, 32 or 128 canonical
columns. The default 16-column width matches the 1 m profile granularity.
Vertical band intersections count
all native faces without a loop per exposed cell. Before upload, the height
range is bounded so neither signed subtraction nor any u32 face total can
overflow. A 64-thread workgroup aggregates into 25 local atomic counters, then
writes each local counter once into its exclusively owned patch output.
One workgroup owns each patch, without global atomic contention. There is no global queue,
unbounded allocator, compression or GPU-side terrain authority. Eight-page
input payload is 2,163,200 bytes; count output/readback add 102,400 bytes at 16-column patch width and
optional query resolve/readback add 32 bytes. Driver allocations and queue
staging are separate from these logical buffer sizes.

The implementation uses the [AMD RDNA performance guide](https://gpuopen.com/learn/rdna-performance-guide/)
for the initial workgroup/batching approach, and the locally installed wgpu
24.0.5 compute pipeline/pass/query APIs. Published vendor advice is not a speed
claim for this prototype. It uses only existing workspace dependencies.

CPU tests validate WGSL through Naga, reject unsupported inputs, and compare
per-patch band-count summaries at all three patch widths with independent direct canonical face enumeration for
Legacy, Regional and RegionalUnwarped sources at negative/positive origins.
GPU execution is an explicitly ignored manual experiment:

```sh
cargo test -p frontier-renderer gpu_summary_probe --lib --locked
cargo test -p frontier-renderer measure_gpu_summary_batches --lib --locked \
  -- --ignored --nocapture
```

The manual run records adapter identity, CPU canonical page generation, CPU
summary time, upload enqueue time, command encode/submit time, blocking
completion/readback time and optional isolated GPU compute timestamps. It checks
all returned counts before reporting cold and five resident-repeat measurements
for batches of 1, 4 and 8 pages, using 16-column patches (64 profiles/page). The blocking API belongs only to this experiment;
it must never be called from the production render loop. First dispatch completion
includes the queued input upload; upload enqueue time alone is not transfer time.
Shader/pipeline creation is outside repeated dispatch timings.

The patch counts match the spatial ownership of production appearance profiles,
but do not replace that production path. Integer counts converted to float can
change the CPU accumulation order. Exact count parity is not bitwise floating appearance parity, a visual
quality pass, or a demonstrated end-to-end renderer speedup. Production adoption
requires measured generation/upload/dispatch/readback savings, matching profile
partition semantics and source identity, float/appearance validation and bounded
asynchronous integration. Keep the existing CPU path unless those gates pass.

## Radeon 760M bounded hardware result

The explicit manual test passed on AMD Radeon 760M Graphics, RADV PHOENIX,
Vulkan, Mesa 25.2.8-0ubuntu0.24.04.2. Every returned per-patch integer count matched
the independent CPU canonical face oracle for all tested batches and all five
resident repeats. Raw output: `output/loading-gpu-probe/run.log`.

These are development-profile standalone measurements using Legacy seed 7,
default FBM, pages at `[(i-4)*128,-512]` native cells, 128×128 columns/page,
16-column patches and 64 profiles/page. There was no simultaneous scene rendering.
The resident column is the median of five encode/submit plus blocking
completion/readback measurements. Cold GPU total includes upload enqueue plus
encode/submit plus completion/readback; these are CPU wall durations, with the
queued transfer completed before readback. Isolated kernel timestamps exclude
that upload and CPU submission work.

| Pages | Canonical CPU page generation | CPU integer summary | Cold GPU total | Resident GPU total, median | Cold GPU kernel |
| --- | ---: | ---: | ---: | ---: | ---: |
| 1 | 5.833 ms | 0.209 ms | 2.256 ms | 0.188 ms | 0.020 ms |
| 4 | 24.012 ms | 0.799 ms | 1.536 ms | 0.346 ms | 0.066 ms |
| 8 | 39.670 ms | 1.325 ms | 1.069 ms | 0.294 ms | 0.120 ms |

The first one-page submission has 1.655 ms of encode/submit overhead, versus
0.138–0.145 ms for the later cold batches. Treat it as a first-use observation;
shader/pipeline construction itself occurs outside these timings. The small
sample does not establish latency tails or an optimal batch size.

Adding the recorded generation and summary stages gives CPU totals of
6.042/24.811/40.995 ms, versus 8.089/25.548/40.739 ms with the cold GPU path.
Thus a very short kernel does not remove the dominant canonical generation cost;
only the eight-page cold batch has a small summed-stage advantage here. Resident
reuse is more promising, saving about 0.02/0.45/1.03 ms against the standalone
CPU count loop for 1/4/8 pages, respectively. These are stage arithmetic and
bounded standalone results, not measured whole-view preparation speedups.

The CPU comparator already uses direct dense canonical columns and integer band
intersections. Production appearance additionally performs source/local lookup,
per-column material-color floating accumulation and profile integration. Its
previous several-millisecond stage timings are therefore not interchangeable
with this 0.209–1.325 ms CPU count loop. A future CPU integer-count profile path
is a useful alternative before GPU integration, but converting counts to colors
changes floating accumulation order and requires separate profile/image parity
validation. No production algorithm was changed for this experiment.

This validates the bounded GPU summary prototype, including per-patch ownership
and integer parity. It does not establish faster game GPU terrain generation:
canonical FBM still runs on the CPU, production appearance is unchanged, and
full-scene GPU contention, asynchronous scheduling, upload/readback tails and
floating appearance agreement remain untested.

## Future source-generation experiment: native Vulkan f64 feasibility

A read-only investigation found a possible optional Vulkan-only experiment,
with the unchanged CPU canonical generator as the fallback. This is a design
note, not an implemented source path or a claim of exact GPU/CPU parity.

The installed wgpu 24.0.5 exposes `SHADER_F64`, `SHADER_INT64` and
`SPIRV_SHADER_PASSTHROUGH`; its documentation restricts f64 to native Vulkan and
warns that hardware f64 arithmetic can be substantially slower than f32.
[wgpu 24.0.5 feature documentation](https://docs.rs/wgpu/24.0.5/wgpu/struct.Features.html).
Support on the measured 760M was not queried during this investigation; no
adapter/device was created and no shader was compiled or dispatched.

The cached Naga 24 SPIR-V reader does not preserve the needed precision contract:
`front/spv/mod.rs::next_decoration` treats `NoContraction` as an unknown decoration,
and its execution-mode parser rejects rounding/denormal modes. Ordinary SPIR-V
input through Naga therefore cannot be assumed to retain exact operation order.
A future probe would need externally validated SPIR-V and explicit layouts via
the native unsafe passthrough API, which performs no SPIR-V validation itself.
[wgpu raw SPIR-V API](https://docs.rs/wgpu/24.0.5/wgpu/struct.Device.html#method.create_shader_module_spirv).

`glslangValidator`, `glslc`, `spirv-as`, `spirv-val` and `spirv-dis` were absent
from PATH and the inspected system binary directories; no compiler package or
new dependency was installed. A reproducible offline toolchain would be needed
before implementation. Khronos glslang provides the relevant GLSL/SPIR-V frontend;
its tool sources have per-file license terms documented in the project's license
file, which must be reviewed if vendored or redistributed. An external build
tool is preferable to adding a runtime compiler dependency for this experiment.
[Khronos glslang](https://github.com/KhronosGroup/glslang),
[glslang licenses](https://github.com/KhronosGroup/glslang/blob/main/LICENSE.txt).

A bounded Legacy-first probe could reproduce the existing wrapping u64 hash,
then the exact Rust f64 arithmetic graph and canonical floor-to-native-cell
operation. Preserve constant bit patterns explicitly: an unsuffixed GLSL decimal
literal can first become f32 even when assigned to a double. Inspect generated
SPIR-V for `NoContraction` on every relevant arithmetic instruction, avoid fast
math, and request only supported precision execution modes. `precise` alone
is insufficient: Vulkan rounding, denormal handling and operation accuracy still
need capability checks and validation. Query the Vulkan float-control properties
for f64 rounding/denormal/signed-zero preservation before making stronger
assumptions. Regional sqrt composition adds another operation to validate.
[Vulkan floating-point requirements](https://docs.vulkan.org/spec/latest/appendices/spirvenv.html).

Validation should proceed from exact hash results to noise intermediates, then
quantized heights and material identity across seeds, negative coordinates,
noise-cell boundaries, parameter extremes, near-voxel-plane values and the
source's documented address range. Any mismatch must retain CPU generation;
a finite passing corpus is not proof of universal canonical equality. If parity
is established for a qualified domain, benchmark full canonical page generation,
upload/readback and source-cache admission at bounded batch sizes under actual
render contention. Keep this investigation separate from the integer summary
prototype and from the production source contract.

## Scope of the #71 result

The bounded per-patch summary experiment is implemented, oracle-validated and
measured on the target integrated GPU. That is a completed feasibility milestone.
Production retains CPU canonical generation and appearance, because the measured
cold end-to-end advantage is small and floating appearance parity, asynchronous
integration and render-contention validation are still required. Treat a broader
GPU terrain-generation or production-speedup requirement as remaining work;
the successful standalone kernel does not satisfy it.

## Drawing contention and decision

`FRONTIER_GPU_SUMMARY_CONTENTION=1` adds a bounded diagnostic to a normal GPU
frame: eight resident canonical pages are summarized with no per-frame readback,
CPU page generation or blocking wait. Startup verifies integer parity once.
The summaries are not consumed by production geometry. Reports expose the
startup time, logical buffer bytes, dispatch count and readback policy.

The matched primary pair records 9.930 ms frame p95 without the diagnostic and
10.102 ms with it; both images are byte-identical. This is one sequential pair,
not a statistically established overhead estimate. Combined with the standalone
source/transfer timings above, it does not demonstrate a whole-frame preparation
benefit. The experiment is complete as a **do-not-promote-yet** result: retain the
CPU fallback and keep GPU preparation opt-in until a useful asynchronous consumer
and equivalent floating appearance pipeline are demonstrated. Exact commands,
hardware and timings are in [follow-up evidence](reference/loading/remaining-issues-notes.md).
