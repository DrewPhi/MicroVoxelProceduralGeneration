#!/usr/bin/env python3
"""Summarize matched experiment runs without treating GPU spans as additive costs."""
import argparse,json,statistics
from pathlib import Path

def summarize(path):
    data=json.loads(path.read_text());runs=[]
    for r in data['runs']:
        m=r.get('case',{}).get('measurements',{})
        n=m.get('naadf',{});stages=n.get('stream_stage_ms',[])
        median=lambda i:statistics.median([s[i] for s in stages]) if stages else None
        runs.append({
            'variant':r['name'],'repeat':r['repeat'],'error':r.get('error'),
            'frames':m.get('frames'),'gpu_full_render':m.get('gpu_full_render'),
            'naadf_gpu_pass':m.get('naadf_gpu_pass'),'process_memory':m.get('process_memory'),
            'coverage':m.get('adaptive_route_quality'),
            'published_shifts':n.get('stream_shifts'),'outside_exact_frames':n.get('stream_outside_frames'),
            'camera_margin_min_m':n.get('min_camera_margin_m'),
            'prepare_ready_p50_ms':median(0),'upload_ready_p50_ms':median(1),
            'fallback_ready_p50_ms':median(2),'publication_p50_ms':median(3),
            'native_column_cache':n.get('native_column_cache'),
            'gpu_payload_bytes':n.get('gpu_payload_bytes'),
            'no_voxel_shadow_clusters':m.get('adaptive',{}).get('no_voxel_shadow_clusters'),
            'camera_start':m.get('camera_start'),'projection':m.get('projection'),
        })
    return {'binary_sha256':data['binary_sha256'],'variants':data['variants'],
            'route':data['route'],'radius_m':data['radius_m'],'runs':runs}
def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('summaries',type=Path,nargs='+');p.add_argument('--output',type=Path,required=True);a=p.parse_args()
    a.output.write_text(json.dumps({str(s):summarize(s) for s in a.summaries},indent=2)+'\n')
if __name__=='__main__':main()
