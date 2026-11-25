#!/usr/bin/env python3
"""Finite owned-window pickup/hold/place acceptance; not a frame-time benchmark."""
import argparse
import hashlib
import json
from pathlib import Path
import struct
import subprocess
import time
from record_walk import ROOT, X11


def inventory(path):
    data = path.read_bytes()
    if data[:8] != b'FVWATER3':
        raise RuntimeError('expected inventory checkpoint version 3')
    pos = 8 + 8 * 8
    count = struct.unpack_from('<Q', data, pos)[0]
    pos += 8 + count * 25
    demo = data[pos]
    pos += 1 + (24 if demo else 0) + 8
    values = struct.unpack_from('<13Q', data, pos)
    return {'counts': list(values[:10]), 'capacity': values[10],
            'selected': values[11], 'sequence': values[12]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=False)
    binary = ROOT / 'target/release/frontier'
    cmd = [str(binary), '--scene', 'adaptive', '--terrain-recipe', 'regional',
           '--amplitude', '0', '--vegetation', 'off', '--walk', 'off',
           '--live-water', 'on', '--naadf', 'on', '--naadf-radius', '8',
           '--width', '1280', '--height', '720', '--eye', '0,1.7,0',
           '--target', '0,0,0.5', '--water-save', str(out/'world.save'),
           '--capture-on-exit', str(out/'exit.png'), '--report', str(out/'game.json')]
    x = None
    app = None
    window = None
    events = []
    started = time.monotonic()
    def note(action):
        events.append({'action': action, 'seconds': time.monotonic()-started})
    try:
        with (out/'game.log').open('w') as log:
            app = subprocess.Popen(cmd, cwd=ROOT, stdout=log, stderr=subprocess.STDOUT)
            x = X11()
            deadline = time.monotonic()+90
            while time.monotonic() < deadline:
                if app.poll() is not None:
                    raise RuntimeError('viewer exited before native readiness')
                window = x.find(app.pid)
                if window and 'water simulation started' in (out/'game.log').read_text():
                    break
                time.sleep(.1)
            else:
                raise RuntimeError('native readiness timeout')
            x.focus(window, app.pid)
            # First click captures input and must not edit.
            x.click(window)
            time.sleep(.5)
            for duration, label in [(.06, 'quick'), (1.1, 'handful')]:
                x.focus(window, app.pid)
                x.button(x.d, 1, 1, 0); x.flush(x.d)
                time.sleep(duration)
                x.button(x.d, 1, 0, 0); x.flush(x.d)
                note(label)
                time.sleep(.7)
            x.button(x.d, 3, 1, 0); x.button(x.d, 3, 0, 0); x.flush(x.d)
            note('place')
            time.sleep(1.)
            x.request_close(window, app.pid)
            app.wait(timeout=30)
            if app.returncode:
                raise RuntimeError(f'viewer exit {app.returncode}')
            state = inventory(out/'world.save')
            # Three picked solid cells, one returned: no capture-click edit.
            if sum(state['counts']) != 2 or state['sequence'] != 3:
                raise RuntimeError(f'pickup/hold/place result failed: {state}')
            (out/'result.json').write_text(json.dumps(state, indent=2)+'\n')
    finally:
        if x:
            x.release_all()
            if app and app.poll() is None and window:
                x.request_close(window, app.pid)
        if app and app.poll() is None:
            app.terminate(); app.wait(timeout=15)
        if x:
            x.close(x.d)
        (out/'manifest.json').write_text(json.dumps({
            'command': cmd, 'binary_sha256': hashlib.sha256(binary.read_bytes()).hexdigest(),
            'events': events, 'scope': __doc__,
        }, indent=2)+'\n')


if __name__ == '__main__':
    main()
