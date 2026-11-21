#!/usr/bin/env python3
"""Compare native and distant GPU depth for an excavation and a suspended deck."""
import argparse
import hashlib
import json
import math
from pathlib import Path
import struct
import subprocess

ROOT = Path(__file__).resolve().parents[1]
WIDTH, HEIGHT = 193, 129


def write_fixture(template, destination, edits):
    # Reuse the source identity, inventory and fluid history from the regular
    # player edit smoke test. Replace only the explicit terrain edit records.
    data = template.read_bytes()
    if data[:8] != b'FVWATER3':
        raise ValueError('expected the default 6.25 cm regional edit-smoke checkpoint')
    count = struct.unpack_from('<Q', data, 72)[0]
    tail = 80 + count * 25
    if tail > len(data):
        raise ValueError('truncated terrain edit table')
    records = b''.join(struct.pack('<qqqB', *edit) for edit in edits)
    destination.write_bytes(data[:72] + struct.pack('<Q', len(edits)) + records + data[tail:])


def capture(out, name, save, eye, target, recipe="regional", visual=(), flight=False, far=96.):
    checkpoint = out / (save + '.save')
    before = hashlib.sha256(checkpoint.read_bytes()).digest()
    png = out / (name + '.png')
    args = [
        ROOT / 'target/release/frontier', *(['--preset', 'flight', '--seed', '7'] if flight else []),
        '--scene', 'adaptive', '--terrain-recipe', recipe,
        '--amplitude', '0', '--vegetation', 'off', '--walk', 'off',
        '--world-save', checkpoint, '--naadf', 'on', '--naadf-radius', '8',
        '--capture-far', str(far), '--width', str(WIDTH), '--height', str(HEIGHT), '--msaa', '1',
        '--eye', ','.join(map(str, eye)), '--target', ','.join(map(str, target)),
        *visual, *([] if flight else ['--capture-linear']),
        '--capture-unresolved', '--no-adaptive-disk-cache', '--capture', png,
    ]
    with png.with_suffix('.log').open('w') as log:
        subprocess.run(args, check=True, stdout=log, stderr=log, timeout=150)
    if before != hashlib.sha256(checkpoint.read_bytes()).digest():
        raise RuntimeError('headless capture changed the save')
    encoded = png.with_suffix('.depth.f32').read_bytes()
    raw = struct.unpack_from('<f', encoded, 4 * ((HEIGHT // 2) * WIDTH + WIDTH // 2))[0]
    near = 0.025
    return far * near / (far - raw * (far - near))

def horizon_edits(out, template):
    # One feature lies in front of the sky. The other is below the procedural
    # ground plane and can only appear when the horizon honors its column mask.
    mast = [(x, y, -32320, 9) for x in range(-160, 160) for y in range(1488, 1712)]
    pit = [(x, y, z, 9 if y == -4 else 0)
           for x in range(-128, 128) for y in range(-4, 0) for z in range(-128, 128)]
    results = {}
    for name, edits, eye, target in [
        ('horizon-mast', mast, (0, 100, 0), (0, 100, -2020)),
        ('horizon-floor', pit, (0, 620, 0), (0, 0, -.14)),
    ]:
        write_fixture(template, out / (name + '.save'), edits)
        rgb = []
        for state in ['stock', name]:
            tag = name + '-' + state
            depth = capture(out, tag, state, eye, target, flight=True, visual=[
                '--atmosphere', 'on', '--beauty', 'on',
                '--fog-density', '0', '--cloud-coverage', '0', '--fov-y', '12',
                '--terrain-shadows', 'off',
            ])
            if abs(depth - 96.) > .01:
                raise AssertionError(f'{tag} hit active geometry at {depth} m')
            image = (out / (tag + '.linear_rgb.f32')).read_bytes()
            rgb.append(struct.unpack_from('<fff', image, 12 * ((HEIGHT // 2) * WIDTH + WIDTH // 2)))
        if max(abs(a - b) for a, b in zip(*rgb)) < .035:
            raise AssertionError(f'{name} missing from the flight horizon: {rgb}')
        results[name] = dict(zip(['stock_linear_rgb', 'edited_linear_rgb'], rgb))
    # Keep the observer and the sun fixed. Only the clip changes, so the
    # two surfaces should agree before and after the horizon hands off.
    write_fixture(template, out / 'horizon-wood.save', [(x, y, z, 4) for x, y, z, _ in mast])
    for save, elevation, overcast in [('horizon-mast', 8, 0.), ('horizon-wood', 35, .85)]:
        rgb = []
        for far in [256., 4000.]:
            tag = f'{save}-light-{int(far)}'
            depth = capture(out, tag, save, (0, 100, -1020), (0, 100, -2020),
                            flight=True, far=far, visual=[
                                '--sun-elevation', str(elevation), '--sun-azimuth', '0',
                                '--overcast', str(overcast), '--fog-density', '0',
                                '--cloud-coverage', '0', '--ground-bounce', '0',
                                '--terrain-shadows', 'off', '--fov-y', '12',
                            ])
            expected = min(999.9375, far)
            if abs(depth - expected) > 3.:
                raise AssertionError(f'{tag} measured {depth} m, expected {expected} m')
            image = (out / (tag + '.linear_rgb.f32')).read_bytes()
            rgb.append(struct.unpack_from('<fff', image, 12 * ((HEIGHT // 2) * WIDTH + WIDTH // 2)))
        if max(abs(a - b) for a, b in zip(*rgb)) > .05:
            raise AssertionError(f'{save} changed illumination across the active clip: {rgb}')
        results[save + '-light'] = dict(zip(['horizon_linear_rgb', 'active_linear_rgb'], rgb))
    return results

def sunlit_excavation(out, template):
    # A flat Alpine source exercises the actual directional shadow caster pass.
    # Both recipe identity words are part of the versioned save format.
    save = out / 'sun-pit.save'
    cut = [(x, y, z, 0) for x in range(-28, 28) for y in range(-16, 0) for z in range(-28, 28)]
    write_fixture(template, save, cut)
    data = bytearray(save.read_bytes())
    struct.pack_into('<QQ', data, 16, 3, 4)  # Alpine recipe, source version 4
    save.write_bytes(data)
    samples = {}
    for shadows in ['on', 'off']:
        name = 'pit-shadow-' + shadows
        visual = ['--beauty', 'on', '--terrain-shadows', shadows, '--atmosphere', 'off',
                  '--sun-elevation', '85', '--sun-azimuth', '0', '--ground-bounce', '0']
        depth = capture(out, name, 'sun-pit', (0, 16, 0), (0, 0, .001), 'alpine', visual)
        meta = json.loads((out / (name + '.capture.json')).read_text())
        if abs(depth - 17.) > .025 or meta['visual']['terrain_shadows_active'] != (shadows == 'on'):
            raise AssertionError(f'{name} missed the floor or used the wrong shadow configuration')
        rgb = (out / (name + '.linear_rgb.f32')).read_bytes()
        pixels = [struct.unpack_from('<fff', rgb, 12 * (y * WIDTH + x))
                  for y in range(HEIGHT // 2 - 3, HEIGHT // 2 + 4)
                  for x in range(WIDTH // 2 - 3, WIDTH // 2 + 4)]
        samples[shadows] = [sum(p[c] for p in pixels) / len(pixels) for c in range(3)]
    if min(samples['off']) <= .25 or max(abs(a-b) for a, b in zip(samples['on'], samples['off'])) > .02:
        raise AssertionError(f'excavated ground casts a shadow across the sunlit floor: {samples}')
    return samples


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--checkpoint', required=True, type=Path,
                        help='world.save from the 6.25 cm terrain-only player edit smoke test')
    parser.add_argument('--output', required=True, type=Path)
    parser.add_argument('--horizon', action='store_true',
                        help='also check saved native surfaces past the active camera clip')
    args = parser.parse_args()
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=False)
    cut = [(x,y,z,0) for x in range(-40,-24) for y in range(-24,0) for z in range(-8,8)]
    deck = [(x,y,z,4) for x in range(24,40) for y in range(44,48) for z in range(-8,8)]
    write_fixture(args.checkpoint, out / 'stock.save', [])
    write_fixture(args.checkpoint, out / 'edited.save', cut + deck)
    views = [
        ('original', 'stock', (-2,20,0), (-2,0,.002), 20.),
        ('distant-floor', 'edited', (-2,20,0), (-2,0,.002), 21.5),
        ('distant-deck', 'edited', (2,20,0), (2,0,.002), 17.),
        ('native-floor', 'edited', (-2,4,0), (-2,0,.002), 5.5),
        ('native-deck', 'edited', (2,5,0), (2,0,.002), 2.),
        ('distant-soffit', 'edited', (2,.7,-20), (2,2.75,0), math.hypot(20,2.05)),
        ('through-opening', 'edited', (2,1,-20), (2,1,0), 96.),
    ]
    results = {}
    for name, save, eye, target, expected in views:
        depth = capture(out, name, save, eye, target)
        if abs(depth - expected) > .025:
            raise AssertionError(f'{name} measured {depth:.5f} m, expected {expected:.5f} m')
        results[name] = {'depth_m': depth, 'expected_m': expected}
        print(f'{name}  {depth:.4f} m', flush=True)
    results['sunlit_floor_linear_rgb'] = sunlit_excavation(out, args.checkpoint)
    if args.horizon:
        results.update(horizon_edits(out, args.checkpoint))
    (out / 'result.json').write_text(json.dumps(results, indent=2) + '\n')


if __name__ == '__main__':
    main()
