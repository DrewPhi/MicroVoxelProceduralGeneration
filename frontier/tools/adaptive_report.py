#!/usr/bin/env python3
"""Summarize adaptive timing without mistaking unresolved frames for a quality pass.

This is a numerical screen, not a visual/temporal acceptance test. Run image
comparisons separately. A fast final frame cannot excuse coarse route frames.
"""
import argparse
import json
import math
from pathlib import Path


def summarize(report, target_fps=60.0):
    frames = report.get('frames', {})
    quality = report.get('adaptive_route_quality', {})
    adaptive = report.get('adaptive', {})
    count = frames.get('sample_count', 0)
    accepted = frames.get('accepted_frames', count)
    measured = quality.get('measured_frames', 0)
    failures = []
    if report.get('benchmark_interrupted') or report.get('benchmark_run_exit_code', 0) != 0:
        failures.append('benchmark interrupted or process failed')
    requested_seconds = report.get('benchmark_requested_seconds')
    if requested_seconds is not None and report.get('route_elapsed_seconds', 0) + 0.1 < requested_seconds:
        failures.append('requested route duration was not completed')
    if count <= 0 or measured <= 0:
        failures.append('no measured frames after warmup/readiness')
    if measured != accepted:
        failures.append('timing and quality sample counts disagree')
    if count != accepted:
        failures.append('timing distribution covers a rolling window, not the complete route')
    if frames.get('rejected_inputs', 0):
        failures.append('invalid timing samples were rejected')
    if quality.get('coverage_ready_frames', 0) != measured:
        failures.append('coverage missing during measured route')
    if quality.get('quality_ready_frames', 0) != measured:
        failures.append('requested quality missed during measured route')
    if quality.get('unbounded_error_frames', 0):
        failures.append('unbounded error during measured route')
    limit = adaptive.get('pixel_error')
    maximum = quality.get('max_finite_visible_error_px')
    if maximum is None or limit is None or not math.isfinite(maximum):
        failures.append('missing finite route error measurement')
    elif maximum > limit + max(1e-6, abs(limit) * 1e-6):
        failures.append('route exceeds requested geometric error')
    if not adaptive.get('coverage_ready') or not adaptive.get('quality_ready'):
        failures.append('final view has not settled')
    p95 = frames.get('p95_ms')
    if p95 is None or not math.isfinite(p95) or p95 > 1000.0 / target_fps:
        failures.append(f'frame p95 misses {target_fps:g} FPS budget')
    return {
        'numerical_screen_pass': not failures,
        'failures': failures,
        'visual_acceptance': 'not evaluated; inspect matched images and motion separately',
        'target_fps': target_fps,
        'frame_ms': {key: frames.get(key) for key in ('p50_ms', 'p95_ms', 'p99_ms')},
        'gpu_ms': report.get('gpu_render_pass', {}),
        'measured_frames': measured,
        'coverage_ready_frames': quality.get('coverage_ready_frames'),
        'quality_ready_frames': quality.get('quality_ready_frames'),
        'requested_error_px': limit,
        'max_route_error_px': maximum,
        'final_triangles': adaptive.get('triangles'),
        'final_gpu_resident_bytes': adaptive.get('gpu_resident_bytes'),
        'final_gpu_pool_bytes': adaptive.get('gpu_allocated_bytes'),
        'final_cpu_cache_bytes': adaptive.get('cpu_cache_bytes'),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('reports', type=Path, nargs='+')
    parser.add_argument('--target-fps', type=float, default=60.0)
    args = parser.parse_args()
    if not math.isfinite(args.target_fps) or args.target_fps <= 0:
        parser.error('--target-fps must be finite and positive')
    print(json.dumps({str(path): summarize(json.loads(path.read_text()), args.target_fps)
                      for path in args.reports}, indent=2))


if __name__ == '__main__':
    main()
