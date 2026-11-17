#!/usr/bin/env python3
"""Record grounded moving-water walk/return/turn and saved-world reactivation.

Uses only its own PID-validated X11 window. Checks migration, conservation,
continued simulation and actual camera motion; does not certify general lakes
or frame time without recording overhead. Requires a ready release binary.
"""
import argparse
import ctypes as C
import hashlib
import json
import math
import os
from pathlib import Path
import shutil
import subprocess
import time
from record_walk import X11, ROOT, I, P, U


def observations(report):
    frames = report.get('frame_observations', {}).get('frames', [])
    if not frames:
        raise RuntimeError('no frame observations; camera motion cannot be verified')
    first = frames[0]['eye']
    return {
        'max_displacement_m': max(math.dist(first, frame['eye']) for frame in frames),
        'yaw_range_radians': max(f['yaw'] for f in frames)-min(f['yaw'] for f in frames),
    }


def run_phase(args, root, save, name, reverse=False):
    out = root / name
    out.mkdir()
    cmd = [str(args.binary.resolve()), '--preset', 'seeded-world', '--seed', str(args.seed),
           '--water-model', 'moving', '--walk', 'on', '--width', str(args.width),
           '--height', str(args.height), '--naadf-radius', '8',
           '--water-save', str(save), '--report', str(out / 'game.json'),
           '--capture-on-exit', str(out / 'exit.png')] + args.extra
    x = app = rec = window = None
    events = []
    started = time.monotonic()
    failure = None

    def note(action):
        events.append({'seconds_from_launch': round(time.monotonic()-started, 4), 'action': action})

    def pause(seconds):
        deadline = time.monotonic()+seconds
        while time.monotonic() < deadline:
            if app.poll() is not None:
                raise RuntimeError('viewer exited during replay')
            time.sleep(min(.1, max(0., deadline-time.monotonic())))

    def hold(key):
        x.focus(window, app.pid)
        note('hold_' + key)
        x.key(key, True)
        try:
            pause(args.move_seconds)
        finally:
            x.key(key, False)
        note('release_' + key)
        pause(args.pause_seconds)

    try:
        with (out / 'game.log').open('w') as log, (out / 'record.log').open('w') as rlog:
            app = subprocess.Popen(cmd, cwd=out, stdout=log, stderr=subprocess.STDOUT)
            x = X11()
            deadline = time.monotonic()+args.startup_timeout
            while time.monotonic() < deadline:
                if app.poll() is not None:
                    raise RuntimeError('viewer exited before moving-water readiness')
                window = x.find(app.pid)
                text = (out / 'game.log').read_text()
                if window and 'fps' in x.title(window).lower() and 'moving water simulation started' in text:
                    break
                time.sleep(.1)
            else:
                raise RuntimeError('moving-water initial native publication timeout')
            note('moving_water_ready')
            x.focus(window, app.pid)
            x.click(window)  # Initial capture click must not edit terrain.
            pause(args.settle_seconds)
            rec = subprocess.Popen([
                'ffmpeg', '-y', '-f', 'x11grab', '-framerate', '20', '-window_id', str(window),
                '-video_size', f'{args.width}x{args.height}', '-draw_mouse', '0',
                '-i', os.environ['DISPLAY'], '-t', '180', '-an', '-c:v', 'libx264',
                '-preset', 'ultrafast', '-crf', '22', '-threads', '2', '-pix_fmt', 'yuv420p',
                str(out / 'walking.mp4')], stdout=rlog, stderr=subprocess.STDOUT)
            note('record_start')
            pause(.5)
            outward, inward = ('s', 'w') if reverse else ('w', 's')
            hold(outward)
            hold(inward)
            relative = x.t.XTestFakeRelativeMotionEvent
            relative.restype = I
            relative.argtypes = [P, I, I, U]
            x.focus(window, app.pid)
            note('turn_begin')
            for _ in range(40):
                relative(x.d, 8, 0, 0)
                x.flush(x.d)
                pause(.025)
            note('turn_end')
            hold(outward)
            hold(inward)
            note('final_settle')
            pause(args.settle_seconds)
            x.request_close(window, app.pid)
            app.wait(timeout=60)
            if app.returncode:
                raise RuntimeError(f'viewer exit {app.returncode}')
            note('saved_and_closed')
            rec.terminate()
            rec.wait(timeout=15)
            report = json.loads((out / 'game.json').read_text())
            flow = report.get('live_water', {})
            if flow.get('model') != 'moving' or not flow.get('conserved'):
                raise RuntimeError('moving-water conservation report failed')
            if flow.get('paused_error'):
                raise RuntimeError('moving-water controller paused: ' + flow['paused_error'])
            if flow.get('migrations', 0) < 1 or flow.get('simtime', 0.) <= 0.:
                raise RuntimeError('water did not migrate and advance')
            motion = observations(report)
            if motion['max_displacement_m'] <= 1. or motion['yaw_range_radians'] <= .2:
                raise RuntimeError(f'walking/turn input not verified: {motion}')
            if save.read_bytes()[:8] not in (b'FVMWATR1', b'FVMWATR2'):
                raise RuntimeError('expected nonempty moving-water checkpoint')
            shutil.copyfile(save, out / 'world.save')
            duration = next(e['seconds_from_launch'] for e in events if e['action'] == 'saved_and_closed')
            begin = next(e['seconds_from_launch'] for e in events if e['action'] == 'record_start')
            for label, second in [('before', .5), ('after', max(.5, duration-begin-.5))]:
                subprocess.run(['ffmpeg', '-y', '-loglevel', 'error', '-ss', str(second),
                                '-i', str(out / 'walking.mp4'), '-frames:v', '1',
                                str(out / f'{label}.png')], check=True)
            summary = {'flow': flow, 'motion': motion,
                       'naadf_stream_shifts': report.get('naadf', {}).get('stream_shifts', 0)}
            (out / 'checks.json').write_text(json.dumps(summary, indent=2)+'\n')
            return summary
    except Exception as exc:
        failure = str(exc)
        raise
    finally:
        if x:
            x.release_all()
            if app and app.poll() is None and window:
                x.request_close(window, app.pid)
        for process in [rec, app]:
            if process and process.poll() is None:
                process.terminate()
                process.wait(timeout=15)
        if x:
            x.close(x.d)
        (out / 'manifest.json').write_text(json.dumps({
            'command': cmd, 'events': events, 'failure': failure,
            'binary_sha256': hashlib.sha256(args.binary.resolve().read_bytes()).hexdigest(),
            'recording_perturbs_frame_times': True, 'scope': __doc__,
        }, indent=2)+'\n')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--binary', type=Path, default=ROOT / 'target/release/frontier')
    parser.add_argument('--seed', type=int, default=19)
    parser.add_argument('--width', type=int, default=1280)
    parser.add_argument('--height', type=int, default=720)
    parser.add_argument('--move-seconds', type=float, default=.45)
    parser.add_argument('--pause-seconds', type=float, default=3.)
    parser.add_argument('--settle-seconds', type=float, default=3.)
    parser.add_argument('--startup-timeout', type=float, default=180.)
    parser.add_argument('game_args', nargs=argparse.REMAINDER)
    args = parser.parse_args()
    args.extra = args.game_args[1:] if args.game_args[:1] == ['--'] else args.game_args
    if args.width < 1280 or args.height < 720:
        parser.error('owned-window replay requires at least 1280x720')
    for name in ['move_seconds', 'pause_seconds', 'settle_seconds', 'startup_timeout']:
        value = getattr(args, name)
        if not math.isfinite(value) or value <= 0.:
            parser.error(f'--{name.replace("_", "-")} must be positive and finite')
    if args.move_seconds > 2. or args.pause_seconds > 15. or args.settle_seconds > 30.:
        parser.error('replay durations exceed the bounded short-walk gate')
    blocked = {'--benchmark-input-lock', '--adaptive-wait-quality', '--water-model', '--water-save', '--walk'}
    if blocked.intersection(args.extra):
        parser.error('extra arguments must preserve interactive moving-water mode, walking and its save path')
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=False)
    save = out / 'world.save'
    first = run_phase(args, out, save, 'walk')
    second = run_phase(args, out, save, 'reload_return', reverse=True)
    a, b = first['flow'], second['flow']
    if b['simtime'] <= a['simtime'] or b['migrations'] <= a['migrations']:
        raise RuntimeError('restored controller did not continue simulation and migration')
    if b['history_tiles'] < a['history_tiles'] or b['initial'] < a['initial']:
        raise RuntimeError('reload discarded recorded native water history')
    (out / 'result.json').write_text(json.dumps({'passed': True, 'walk': first,
        'reload_return': second, 'scope': __doc__}, indent=2)+'\n')


if __name__ == '__main__':
    main()
