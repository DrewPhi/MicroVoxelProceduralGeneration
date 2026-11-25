#!/usr/bin/env python3
"""Record the native water demo and extract window frames before/after tick-20 breach.
Uses the same owned-child X11 window validation as record_walk.py. Recording adds
CPU/GPU overhead; benchmark separately. Run with the project release binary ready.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import struct
import subprocess
import time
from record_walk import X11, ROOT


def demo_flow(save_path):
    """Read the versioned demo checkpoint and verify mass crossed the dam.

    This diagnostic intentionally understands FVWATER2/3 and FVFLUID1; it must
    fail explicitly if the runtime changes its save layout.
    """
    data = save_path.read_bytes()
    if data[:8] not in (b'FVWATER2', b'FVWATER3'):
        raise RuntimeError('unexpected demo envelope')
    pos = 8 + 8 * 8
    edits = struct.unpack_from('<Q', data, pos)[0]
    pos += 8 + 25 * edits
    if data[pos] != 1:
        raise RuntimeError('save has no demo center')
    pos += 1
    center = struct.unpack_from('<qqq', data, pos)
    pos += 24 + 8
    if data[:8] == b'FVWATER3':
        pos += 104  # Thirteen u64 inventory/sequence fields.
    if data[pos:pos + 8] != b'FVFLUID1':
        raise RuntimeError('unexpected fluid schema')
    pos += 8
    meta = struct.unpack_from('<9Q', data, pos)
    pos += 72
    left = right = 0
    for _ in range(meta[8]):
        cx, cy, cz, mass = struct.unpack_from('<qqqQ', data, pos)
        pos += 32
        if center[2] - 8 < cz < center[2] + 8 and center[1] <= cy < center[1] + 12:
            if center[0] < cx < center[0] + 16:
                right += mass
            elif center[0] - 16 < cx < center[0]:
                left += mass
    if right == 0:
        raise RuntimeError('no real fluid mass crossed the dam')
    return {
        'dam_center_cells': center,
        'left_mass': left,
        'right_mass': right,
        'tick': meta[4],
        'initial_mass': meta[5],
        'additions': meta[6],
        'removals': meta[7],
    }


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--output', type=Path, required=True)
    p.add_argument('--model', choices=['ca', 'discrete', 'procedural', 'moving'], default='ca')
    p.add_argument('--duration', type=float, default=15.)
    p.add_argument('--width', type=int, default=1280)
    p.add_argument('--height', type=int, default=720)
    p.add_argument('--binary', type=Path, default=ROOT / 'target/release/frontier')
    p.add_argument('game_args', nargs=argparse.REMAINDER)
    a = p.parse_args()
    out = a.output.resolve()
    out.mkdir(parents=True, exist_ok=False)
    extra = a.game_args[1:] if a.game_args[:1] == ['--'] else a.game_args
    cmd = [
        str(a.binary.resolve()), '--preset', 'seeded-world', '--seed', '19',
        '--water-demo', '--width', str(a.width), '--height', str(a.height), '--naadf-radius', '8',
        '--water-save', str(out / 'world.save'),
        '--capture-on-exit', str(out / 'exit.png'),
        '--report', str(out / 'game.json'),
    ] + extra
    if a.model != 'ca':
        cmd += ['--water-model', a.model]
    x = None
    app = None
    rec = None
    window = None
    events = []
    start = time.monotonic()

    def note(action):
        events.append({'seconds': round(time.monotonic() - start, 4), 'action': action})
    try:
        with (out / 'game.log').open('w') as log, (out / 'record.log').open('w') as rlog:
            app = subprocess.Popen(cmd, cwd=out, stdout=log, stderr=subprocess.STDOUT)
            x = X11()
            deadline = time.monotonic() + 120
            while time.monotonic() < deadline:
                if app.poll() is not None:
                    raise RuntimeError('game exited before window')
                window = x.find(app.pid)
                if window and 'fps' in x.title(window).lower():
                    break
                time.sleep(0.1)
            if not window:
                raise RuntimeError('no child window')
            x.focus(window, app.pid)
            rec = subprocess.Popen([
                'ffmpeg', '-y', '-f', 'x11grab', '-framerate', '20',
                '-window_id', str(window), '-video_size', f'{a.width}x{a.height}',
                '-draw_mouse', '0', '-i', os.environ['DISPLAY'], '-t', str(120 + a.duration), '-an',
                '-c:v', 'libx264', '-preset', 'ultrafast', '-crf', '22',
                '-threads', '2', '-pix_fmt', 'yuv420p', str(out / 'water.mp4'),
            ], stdout=rlog, stderr=subprocess.STDOUT)
            note('record_start')
            markers = ([(a.model + ' water simulation started', 'before')] if a.model != 'ca' else [('water simulation started', 'before'), ('water demo dam opened', 'breach')])
            for marker, label in markers:
                while marker not in (out / 'game.log').read_text():
                    if app.poll() is not None:
                        raise RuntimeError('game exited while awaiting ' + marker)
                    if time.monotonic() > deadline:
                        raise RuntimeError('timeout awaiting ' + marker)
                    time.sleep(0.02)
                note(label)
            time.sleep(a.duration if a.model != 'ca' else 6)
            note('after')
            x.request_close(window, app.pid)
            app.wait(timeout=30)
            note('exit')
            if rec.poll() is None:
                rec.terminate()
            rec.wait(timeout=10)
            marks = {event['action']: event['seconds'] for event in events}
            # Export recording frames afterward, without GPU readbacks during flow.
            frame_times = {
                'before': min(
                    marks['before'] - marks['record_start'] + 0.7,
                    marks.get('breach', marks['after']) - marks['record_start'] - 0.5,
                ),
                'after': marks['after'] - marks['record_start'] - 0.3,
            }
            for label, seconds in frame_times.items():
                subprocess.run([
                    'ffmpeg', '-y', '-ss', str(max(seconds, 0)),
                    '-i', str(out / 'water.mp4'), '-frames:v', '1',
                    str(out / (label + '.png')), '-loglevel', 'error',
                ], check=True)
            report = json.loads((out / 'game.json').read_text())
            if not report.get('live_water', {}).get('conserved'):
                raise RuntimeError('water report conservation failed')
            if a.model == 'discrete':
                flow = report['live_water']
                if flow.get('additions', 0) <= 0 or flow.get('removals', 0) <= 0:
                    raise RuntimeError('continuous flow did not reach both source and outlet')
                if not (out / 'world.save').stat().st_size:
                    raise RuntimeError('empty discrete checkpoint')
            elif a.model in ('procedural', 'moving'):
                flow = report['live_water']
                if not (out / 'world.save').stat().st_size:
                    raise RuntimeError('empty procedural checkpoint')
            else:
                flow = demo_flow(out / 'world.save')
            (out / 'flow.json').write_text(json.dumps(flow, indent=2) + '\n')
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
        manifest = {
            'command': cmd,
            'binary_sha256': hashlib.sha256(a.binary.resolve().read_bytes()).hexdigest(),
            'events': events,
            'recording_changes_frame_times': True,
            'before_after_source': (
                '20 FPS X11 child-window video; exit.png is direct GPU capture'
            ),
        }
        (out / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')


if __name__ == '__main__':
    main()
