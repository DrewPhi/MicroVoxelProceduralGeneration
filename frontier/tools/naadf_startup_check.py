#!/usr/bin/env python3
"""Check bounded NAADF startup and walking in the actual X11 game window.

Uses log milestones rather than repeated screenshots, which can miss hotkeys
under load. Run performance benchmarks separately: this is a functional check.
"""
import argparse
import ctypes as C
import hashlib
import json
import subprocess
import time
from pathlib import Path

from record_walk import X11

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--radius', type=int, default=64)
    parser.add_argument('--timeout', type=float, default=900)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if not 1 <= args.radius <= 256 or not 30 <= args.timeout <= 1800:
        parser.error('Use radius 1..256 and timeout 30..1800 seconds')
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=False)
    binary = ROOT / 'target/release/frontier'
    command = [str(binary), '--preset', 'alpine', '--width', '1920', '--height', '1080',
               '--seed', '7', '--msaa', '1', '--pixel-error', '5',
               '--naadf-radius', str(args.radius), '--naadf-budget-mib', '10240',
               '--seconds', str(args.timeout + 120), '--report', str(out / 'game.json')]
    manifest = {'command': command, 'binary_sha256': hashlib.sha256(binary.read_bytes()).hexdigest(),
                'scope': '1080p MSAA1, Alpine seed7, 6.25cm voxels; startup, grounded walking, stop/turn, full-radius follow. Not an isolated performance benchmark.',
                'events': [], 'passed': False}
    app = window = x = None
    start = time.monotonic()

    def event(name):
        manifest['events'].append({'event': name, 'seconds': time.monotonic() - start})
        print(name, flush=True)

    try:
        x = X11()
        with (out / 'game.log').open('w') as log:
            app = subprocess.Popen(command, cwd=out, stdout=log, stderr=subprocess.STDOUT)
            deadline = start + args.timeout
            while time.monotonic() < deadline:
                if app.poll() is not None:
                    raise RuntimeError('Game exited before finding the window')
                window = x.find(app.pid)
                if window and 'fps' in x.title(window).lower():
                    break
                time.sleep(.2)
            else:
                raise RuntimeError('No game window')
            x.focus(window, app.pid)
            x.click(window)
            while 'NAADF residency:' not in (out / 'game.log').read_text():
                if app.poll() is not None or time.monotonic() > deadline:
                    raise RuntimeError('No initial native publication')
                time.sleep(.2)
            event('first_native_region_visible')
            x.key('w', True)
            try:
                time.sleep(8)
            finally:
                x.key('w', False)
            event('walked_during_startup_then_stopped')
            relative = x.t.XTestFakeRelativeMotionEvent
            relative.restype = C.c_int
            relative.argtypes = [C.c_void_p, C.c_int, C.c_int, C.c_ulong]
            relative(x.d, 70, 0, 0)
            x.flush(x.d)
            event('turned_during_startup')
            milestone = f'NAADF residency: {args.radius}/{args.radius} m'
            while milestone not in (out / 'game.log').read_text():
                if app.poll() is not None or time.monotonic() > deadline:
                    raise RuntimeError('Requested native radius did not finish')
                time.sleep(1)
            event('requested_radius_complete')
            x.focus(window, app.pid)
            x.key('w', True)
            try:
                time.sleep(10)
            finally:
                x.key('w', False)
            event('walked_at_full_radius')
            time.sleep(25)
            event('follow_settled')
            x.key('F12', True)
            time.sleep(.3)
            x.key('F12', False)
            time.sleep(3)
            x.request_close(window, app.pid)
            app.wait(timeout=30)
        report = json.loads((out / 'game.json').read_text())
        native = report['naadf']
        publications = native['startup_publications_radius_seconds']
        assert native['refused'] == 0, native['status']
        assert native['resident_radius_m'] == args.radius, native['status']
        assert all(radius <= args.radius for radius, _ in publications)
        assert native['stream_outside_frames'] == 0
        assert native['stream_shifts'] > len(publications) - 1, 'Following did not publish a strip'
        manifest['passed'] = True
    except BaseException as error:
        manifest['failure'] = str(error)
        raise
    finally:
        if x:
            x.release_all()
            if app and app.poll() is None and window:
                x.request_close(window, app.pid)
        if app and app.poll() is None:
            try:
                app.wait(timeout=15)
            except subprocess.TimeoutExpired:
                app.terminate()
                app.wait(timeout=15)
        if x:
            x.close(x.d)
        (out / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')


if __name__ == '__main__':
    main()
