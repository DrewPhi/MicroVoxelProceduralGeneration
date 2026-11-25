# Issue 74 existing-run report

Descriptive only. Numerical readiness and FPS do not establish visual or motion acceptance.

| Run | Group | First coverage / quality / reserve ms | Frame p50 / p95 / p99 ms | GPU p95 ms / scope | Numerical screen | Missing preset fields |
| --- | --- | --- | --- | --- | --- | --- |
| /home/drew/Projects/voxels/MicroVoxelProceduralGeneration/frontier/output/issue74/baseline/static1000/far1000-workers4/static.json | preset-1 | 163.825 / 53446.946 / 71056.560 | 13.822 / 16.906 / 17.871 | 13.263 / unknown; do not assume whole-frame GPU timing | failed/incomplete | visual_settings, upload_budget_bytes_per_frame, experiment environment provenance |
| /home/drew/Projects/voxels/MicroVoxelProceduralGeneration/frontier/output/issue74/baseline/static300-a/far300-workers4/static.json | preset-2 | 171.893 / 10770.899 / 12244.175 | 7.630 / 10.265 / 10.506 | 6.999 / unknown; do not assume whole-frame GPU timing | passes numerical screen only | visual_settings, upload_budget_bytes_per_frame, experiment environment provenance |
| /home/drew/Projects/voxels/MicroVoxelProceduralGeneration/frontier/output/issue74/baseline/static300-b/far300-workers4/static.json | preset-3 | 216.982 / 10853.158 / 12279.913 | 7.576 / 10.170 / 10.409 | 6.915 / unknown; do not assume whole-frame GPU timing | passes numerical screen only | visual_settings, upload_budget_bytes_per_frame, experiment environment provenance |
| /home/drew/Projects/voxels/MicroVoxelProceduralGeneration/frontier/output/issue74/candidate/static300/far300-workers4/static.json | preset-4 | 193.068 / 11105.296 / 12569.553 | 7.739 / 10.187 / 10.824 | 7.114 / terrain_pass_only | passes numerical screen only | none |
| /home/drew/Projects/voxels/MicroVoxelProceduralGeneration/frontier/output/issue74/regional-ab/direct-a/static.json | preset-5 | 238.536 / 7956.678 / 10175.530 | 3.780 / 4.818 / 5.583 | 3.180 / terrain_pass_only | passes numerical screen only | none |
| /home/drew/Projects/voxels/MicroVoxelProceduralGeneration/frontier/output/issue74/regional-ab/direct-b/static.json | preset-5 | 222.723 / 7747.162 / 9914.154 | 3.842 / 5.032 / 5.688 | 3.261 / terrain_pass_only | passes numerical screen only | none |
| /home/drew/Projects/voxels/MicroVoxelProceduralGeneration/frontier/output/issue74/regional-ab/reuse-a/static.json | preset-5 | 211.297 / 7874.046 / 10147.744 | 3.851 / 4.940 / 5.787 | 3.292 / terrain_pass_only | passes numerical screen only | none |
| /home/drew/Projects/voxels/MicroVoxelProceduralGeneration/frontier/output/issue74/regional-ab/reuse-b/static.json | preset-5 | 226.500 / 8167.087 / 10440.320 | 3.791 / 4.754 / 5.592 | 3.187 / terrain_pass_only | passes numerical screen only | none |

Full identity, command provenance, raw fields, RGB hashes, CPU overlap caveats and failures are in the JSON.
