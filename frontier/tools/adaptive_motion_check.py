#!/usr/bin/env python3
"""Sequential hardware motion diagnostics from the frozen primary camera.

Run with DISPLAY pointing to the laptop session and the hardware GPU accessible.
Quality/readiness in each report are acceptance conditions, not implied by FPS.
"""
import argparse
import json
from pathlib import Path
import subprocess
import time

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, default=ROOT / 'output/adaptive-motion')
    parser.add_argument('--seconds', type=float, default=10.)
    parser.add_argument('--speeds', default='3,15,80')
    parser.add_argument('--pixel-error', type=float, default=2.)
    parser.add_argument('--cache-mib', type=int, default=512)
    parser.add_argument('--msaa', type=int, choices=(1, 4), default=1)
    parser.add_argument('--native-bins', choices=('on', 'off'))
    parser.add_argument('--settled', action='store_true', help='Require initial quality before starting benchmark clocks')
    parser.add_argument('--static-only', action='store_true', help='Measure the fixed primary pose instead of routes')
    args = parser.parse_args()
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=True)
    reference = json.loads((ROOT / 'reference/gallery/player-spiky.json').read_text())['reference']
    vector = lambda values: ','.join(map(str, values))
    runs = []
    for speed in ([3.] if args.static_only else map(float, args.speeds.split(','))):
        name = 'static' if args.static_only else f'{speed:g}mps'
        command = [str(ROOT / 'target/release/frontier'), '--scene', 'adaptive',
                   '--pixel-error', str(args.pixel_error), '--adaptive-cache-mib', str(args.cache_mib), '--terrain-recipe', 'legacy', '--seed', '7',
                   '--width', '1920', '--height', '1080', '--msaa', str(args.msaa),
                   '--voxel-filter', 'on', '--uncapped', '--warmup', '60', '--benchmark-input-lock',
                   '--route', 'static' if args.static_only else 'turn-return', '--route-clock', 'realtime',
                   '--route-speed', str(speed), '--seconds', str(args.seconds),
                   '--capture-far', str(reference['far']),
                   '--eye', vector(reference['eye']), '--target', vector(reference['target']),
                   '--fov-y', str(reference['fov_y_degrees']),
                   '--capture-on-exit', str(out / f'{name}.png'),
                   '--report', str(out / f'{name}.json')]
        if args.settled:
            command.append('--adaptive-wait-quality')
        if args.native_bins is not None:
            command += ['--native-bins', args.native_bins]
        print('+', ' '.join(command), flush=True)
        started = time.monotonic()
        with (out / f'{name}.log').open('w') as log:
            result = subprocess.run(['/usr/bin/time', '-v', '-o', str(out / f'{name}.time.txt')]
                                    + command, cwd=ROOT, stdout=log, stderr=subprocess.STDOUT)
        report_path = out / f'{name}.json'
        if report_path.exists():
            report = json.loads(report_path.read_text())
            log_text = (out / f'{name}.log').read_text()
            report['benchmark_interrupted'] = ('benchmark interrupted' in log_text
                                               or result.returncode < 0 or result.returncode >= 128)
            report['benchmark_requested_seconds'] = args.seconds
            report['benchmark_run_exit_code'] = result.returncode
            report_path.write_text(json.dumps(report, indent=2) + '\n')
        runs.append({'speed_mps': speed, 'command': command,
                     'wall_seconds': time.monotonic() - started, 'exit_code': result.returncode})
        (out / 'runs.json').write_text(json.dumps(runs, indent=2) + '\n')
        if result.returncode:
            raise SystemExit(result.returncode)


if __name__ == '__main__':
    main()
