# Authorized 1km GPU fitting job submitted

User explicitly authorized building GPU fitter and submitting Bouchet jobs. SSH:
ssh -S /tmp/bouchet-ssh-20261102-%C -o BatchMode=yes -o ConnectTimeout=15 dss227@bouchet.ycrc.yale.edu
Network/control socket require sandbox escalation on this host; supplied session works.

Deployment: /nfs/roberts/scratch/pi_sk2433/dss227/voxels/analytic-world-1km-20261102-gpu-v1
Account pi_sk2433 (normal research account, not paid priority account). Own isolated directory.
Preflight job31745863 gpu_devel, one GPU/4CPU/16GB/15min.
World job31746201, afterok:31745863, one compatible GPU/8CPU/32GB/2h, optimization budget75min.
Preflight completed successfully; see final section. Full job was PENDING after dependency cleared. Never duplicate these
submissions blindly. H100-only test-only request estimated several days; full job uses the general gpu partition after test-only estimates were earlier there
than the H100/multi-partition request. Superseded queued job31745865 was canceled
and replaced by31746201; no other jobs touched. Queue time is separate from fitting time.

Local records output/bouchet-world-1km/submission.json, job-bundle.tar.gz, bundle-sha256.txt.
Source tools/gpu_fit/ (model, prepare, train, preflight, sbatch scripts, offline Rust sampler).
See tools/gpu_fit/README.md for representation, data and restart details.

Scalable basis: per scalar branch, sum bilinear XY, XZ, YZ planes + univariate quadratic
bubbles per axis. Exact total degree2 per cube, exact face compatibility, no tetrahedra.
2x2x2 scalar span rank25 = previous constrained nullspace. This avoids dense whole-world QR.
Branches retain globally consistent labels. Full grid XYZ125x53x125, n5, ~149k trainable
parameters incl redundant gauges vs dense8m coefficients. Global constraints can limit
fidelity; do not promise same local error/expressivity. Training is PyTorch CUDA float32;
CPU Rust samples are authoritative. No neural net and no separate GPU noise port.

Local verification: scalar span25, face jumps~1e-15 in double, gradients numerical check,
2048 portable Rust sample records match authoritative fbm_points bit-for-bit; CPU end-to-end
pilot train/validate/checkpoint/export/preview succeeds. Pilot initialization exactly maps
old group coefficients to the new basis (before float32 rounding). Pilot GPU preflight
checks this again on a node plus source record parity and <=5% sampled error.

Full job generates16,777,216 train +1,048,576 independent validation native-center samples,
not exhaustive1.7trillion voxels. Mix2/3volume +1/3surface. Source world is approved
output/world-1km/world.json (1000x424x1000m,6.25cm cells, original seed42). Source recipe,
region and samples are isolated from existing demo. Output result/ includes metrics,
preview, compact shared-parameters.npz, best/latest.pt with RNG+optimizer and binary
quadratic coefficients (~198.75MB), plus old-format center-crop.json. Full binary header
is NOT yet readable by the current viewer; need add large binary scene loading/paged GPU
binding or justified limits before full-world exploration after job completes. User asked
for build+submission this turn, not automatic default-world replacement or edits/weather.

Checkpoint handling: scripts use early Slurm USR1, forward to Python; save latest and exit75
on interruption. Resume safely using same deployment/scripts (--resume already supplied).
No other user's or existing research jobs touched. No push/commit requested/performed.

## Preflight completed successfully

Job31745863 COMPLETED exit0 in34sec on NVIDIA RTX5000Ada. CPU+CUDA double model math
checks passed (rank25; gradient check; coefficient/direct error~1.33e-15; joins~1.78e-15).
Source-parity2048records passed. Pilot best-step50 sampled error553/65536=.843811%;
not directly comparable to prior exhaustive error due the volume/surface sampling mix.
Seam_check36k face samples: zero sign disagreements; max baked/direct discrepancy~7.15e-7.
Full job31746201 dependency cleared, last PENDING in gpu queue. Initial queued fulljob
31745865 canceled/replaced before starting, to fix exit/signal handling and use the earlier
normal GPU queue. No other research jobs modified.

Pulled outputs to output/bouchet-world-1km/preflight/ including preflight-passed.json,
pilot-result/center-crop.json, metrics.json, submission.json. The small exported scene
renders locally with80GPU/f64 reference rays matching. IMPORTANT environment discovery:
headless ./analytic run with sandbox escalation and WITHOUT LIBGL_ALWAYS_SOFTWARE uses
actual AMD Radeon760M (RADV PHOENIX). Prior sandboxed tests used llvmpipe, and GUI tests
explicitly forced software. New result render-check.json records hardware adapter; only2
frames were run, so do not claim it is a steady-state performance benchmark. For future
real GPU profiling use an escalated exec without forcing software rendering.

## Full run completed; NOT a successful improved fit

31746201 completed exit0, Slurm elapsed99sec on NVIDIA RTX A5000. Trainer elapsed66.94sec,
15000 updates, plateau30 validation checks, best_step0. Exported best checkpoint is the
initial base-height model, not an improved learned terrain. Independent sampled error
128284/1048576=12.2341%; uniform12.4874%; surface sampling11.7276%; density-band(<1m)
49.8752%. Last model error15.2864%, worse than initialization. Thus technically functional
GPU pipeline and zero sampled seam sign mismatches, but optimization/representation did
NOT achieve a satisfactory full-world fit. Do not present this as ready-to-explore fitted
FBM or replace default scene. Need diagnose objective, initialization, optimizer and global
constraint capacity; these data do not distinguish them. No automatic rerun submitted.
Downloaded small reports/preview to output/bouchet-world-1km/completed/. Large198.75MB
coefficient file remains remote. Local submission.json updated with result caveat.
