#!/usr/bin/env python3
"""Summarize explicitly instrumented NAADF capture metadata, never GPU timings."""
import argparse
import json
from pathlib import Path

EVENTS = {
    0: 'rays', 1: 'root_visits', 2: 'block_visits', 3: 'leaf_visits',
    4: 'root_word_fetches', 5: 'block_word_fetches', 6: 'leaf_word_fetches',
    7: 'height_queries', 8: 'height_level_fetches', 9: 'height_candidates',
    10: 'height_applied', 11: 'height_failed', 12: 'height_blocked_below',
    13: 'height_no_gain', 14: 'height_negative_cache_bypasses',
    19: 'local_exit_repairs', 20: 'numerical_stalls', 21: 'stall_replays', 22: 'boundary_replays',
    23: 'misses', 24: 'hits', 25: 'root_cache_hits', 26: 'block_cache_hits',
    27: 'page_cache_hits', 28: 'diagnostic_exact_step_probes',
    29: 'root_distance_fetches', 30: 'direct_native_replays',
    31: 'direct_native_cells',
}
BEAM = {
    0: 'tiles', 1: 'positive_tiles', 2: 'empty_prefix_tiles',
    3: 'slabs_tested', 4: 'height_fetches', 5: 'obstacle_rejections',
    6: 'work_capped', 9: 'useful_primary_rays', 10: 'primary_rays',
    11: 'whole_volume_skipped_rays',
}


def ratio(a, b):
    return a / b if b else None


def wide(words, index):
    return words[index] + (words[index + 1] << 32)


def summarize(metadata):
    counters = metadata.get('naadf_traversal_diagnostics')
    if not counters or not counters.get('instrumented') or counters.get('valid_for_timing') is not False:
        raise ValueError('requires explicitly instrumented, non-timing capture metadata')
    result = {'instrumented': True, 'valid_for_timing': False,
              'scope': counters['scope'], 'distance_units': counters['distance_units'],
              'camera': {k: metadata.get(k) for k in ('eye', 'target', 'width', 'height')},
              'settings': {k: metadata.get('naadf', {}).get(k) for k in (
                  'local_exit_repair', 'node_cache_mode', 'height_tile_roots', 'height_gate', 'height_mips', 'height_probe_level',
                  'beam_prefix', 'gpu_root_distance_requested')}}
    for name in ('primary', 'shadow'):
        words = counters[name]
        if len(words) != 32 or any(not isinstance(v, int) or v < 0 for v in words):
            raise ValueError('invalid counter lane')
        lane = {label: words[index] for index, label in EVENTS.items()}
        lane.update(saved_native_cells=wide(words, 15),
                    lost_native_cells=wide(words, 17),
                    height_success_fraction=ratio(words[10], words[7]),
                    height_lookups_per_query=ratio(words[8], words[7]),
                    saved_native_cells_per_applied_skip=ratio(wide(words, 15), words[10]),
                    root_fetches_per_ray=ratio(words[4], words[0]),
                    block_fetches_per_ray=ratio(words[5], words[0]),
                    leaf_fetches_per_ray=ratio(words[6], words[0]))
        if words[0] != words[23] + words[24]:
            raise ValueError(f'{name} calls/hits/misses inconsistent (overflow or incomplete capture)')
        if words[7] != words[9] + words[11] or words[10] > words[9]:
            raise ValueError(f'{name} height counters inconsistent')
        result[name] = lane
    beam = counters['beam']
    if len(beam) != 32 or beam[0] != beam[1] + beam[2] or beam[9] > beam[10]:
        raise ValueError('beam counters inconsistent')
    result['beam'] = {label: beam[index] for index, label in BEAM.items()}
    result['beam'].update(positive_tile_fraction=ratio(beam[1], beam[0]),
                          useful_ray_fraction=ratio(beam[9], beam[10]),
                          tile_prefix_native_cells=wide(beam, 15),
                          ray_saved_native_cells=wide(beam, 17))
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('captures', nargs='+', type=Path)
    parser.add_argument('--output', type=Path)
    args = parser.parse_args()
    result = {str(path): summarize(json.loads(path.read_text())) for path in args.captures}
    payload = json.dumps(result, indent=2) + '\n'
    if args.output:
        args.output.write_text(payload)
    else:
        print(payload, end='')


if __name__ == '__main__':
    main()
