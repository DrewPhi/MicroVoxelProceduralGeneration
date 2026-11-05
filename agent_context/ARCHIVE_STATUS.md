# Analytic branch archived — 2026-11-04

User chose to retire the analytic approach and use `frontier-voxel-engine`.
The local worktree is removed only after source, notes and generated research
artifacts are pushed and verified. Remote branches and history are retained.

The final source includes local Hermite fitting, seam-target experiments,
conforming P2 and shared CSG comparisons, their measured limitations, and the
scalable Bouchet volume fitting jobs. Do not interpret archival as evidence that
these models are production ready or faster than the old engine.

Release: https://github.com/DrewPhi/MicroVoxelProceduralGeneration/releases/tag/legacy-research-archive-2026-11-04

- `analytic-experiments-artifacts.tar.gz`: complete former analytic `output/`,
  including recipes, models, density caches, previews, benchmarks and job receipts.
- `legacy-main-artifacts.tar.gz`: former main `output/`, screenshots, viewer
  diagnostic output and ignored settings. Restore into the corresponding main
  checkout rather than the new engine workspace.
- `archive-manifest.json` and `SHA256SUMS`: file inventory and integrity hashes.

Archives deliberately exclude regenerable Rust targets and compiled launchers.
Source code is in Git history. Old binaries in diagnostic output, if present,
are research artifacts rather than current launchers.

Bouchet jobs 31935788 (preparation),31935789 (P2),31935790 (CSG) and their isolated
remote directory are independent of local cleanup and are not canceled or deleted
by it. See BOUCHET_VOLUME_JOBS.md for provenance, paths and retrieval instructions.
Current scheduler status must be checked rather than inferred from this archive.

The new branch initially contains an architecture and inherited reference source.
Its `frontier/AGENTS.md` and `frontier/ARCHITECTURE.md` govern future implementation.
No old fitting/traversal system is a prerequisite for that work.
