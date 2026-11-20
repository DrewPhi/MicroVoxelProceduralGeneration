#!/usr/bin/env python3
"""Drive only our game window: scoop, carry, pour, and optionally pocket, bury and grow.
The MP4 records actual input and simulation. No scripted water state is injected.
"""
import argparse,ctypes as C,hashlib,json,os,subprocess,time
from pathlib import Path
from record_walk import X11,ROOT,I,P,U

def main():
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('--output',type=Path,required=True);p.add_argument('--plant',action='store_true',help='Gather/stow handfuls, place a black seed, bury, water and watch it grow');a=p.parse_args();out=a.output.resolve();out.mkdir(parents=True,exist_ok=False)
 binary=ROOT/'target/release/frontier';save=out/'world.bowl';cmd=[str(binary),'--preset','desert','--seed','19','--width','1920','--height','1080','--naadf-radius','16','--bowl-save',str(save),'--report',str(out/'game.json')]
 app=rec=x=window=None;events=[];start=time.monotonic();failure=None
 def note(s):events.append({'seconds':time.monotonic()-start,'action':s})
 def pause(t):
  end=time.monotonic()+t
  while time.monotonic()<end:
   if app.poll() is not None:raise RuntimeError('game exited during bowl demonstration')
   time.sleep(min(.05,max(0,end-time.monotonic())))
 def key(k,t):
  x.focus(window,app.pid);x.key(k,True)
  try:pause(t)
  finally:x.key(k,False)
 def click():x.button(x.d,1,1,0);x.button(x.d,1,0,0);x.flush(x.d)
 def get_pose():
  before=(out/'game.log').read_text().count('hand_pose ')
  x.button(x.d,3,1,0);x.button(x.d,3,0,0);x.flush(x.d)
  deadline=time.monotonic()+2
  while time.monotonic()<deadline:
   lines=[line for line in (out/'game.log').read_text().splitlines() if line.startswith('hand_pose ')]
   if len(lines)>before:return json.loads(lines[-1][len('hand_pose '):])
   pause(.05)
  raise RuntimeError('no interaction pose telemetry')
 def move_mouse(dx,dy):
  steps=max(1,int(max(abs(dx),abs(dy))/10))
  previous_x=previous_y=0
  for step in range(1,steps+1):
   px=round(dx*step/steps);py=round(dy*step/steps)
   relative(x.d,px-previous_x,py-previous_y,0);x.flush(x.d);pause(.015)
   previous_x,previous_y=px,py
 def scoop():
  before=(out/'game.log').read_text().count('bowl scoop accepted')
  x.key('Control_L',True)
  x.button(x.d,1,1,0);x.flush(x.d)
  try:
   for _ in range(18):
    pause(.32)
    if (out/'game.log').read_text().count('bowl scoop accepted')>before:
     pause(.24)
     return
    key('w',.36)
  finally:
   x.button(x.d,1,0,0);x.flush(x.d);pause(.25)
   x.key('Control_L',False)
  raise RuntimeError('could not dip the bowl rim into pond water through actual input')
 try:
  with (out/'game.log').open('w') as log,(out/'record.log').open('w') as rlog:
   app=subprocess.Popen(cmd,cwd=ROOT,stdout=log,stderr=subprocess.STDOUT);x=X11();deadline=time.monotonic()+120
   while time.monotonic()<deadline:
    if app.poll() is not None:raise RuntimeError('game startup failed')
    window=x.find(app.pid)
    if window and 'fps' in x.title(window).lower():break
    time.sleep(.1)
   else:raise RuntimeError('no owned game window')
   x.focus(window,app.pid);x.click(window);key('2',.08);pause(10.)
   relative=x.t.XTestFakeRelativeMotionEvent;relative.restype=I;relative.argtypes=[P,I,I,U]
   original=get_pose();move_mouse(20,20);calibrated=get_pose()
   sx=(calibrated[3]-original[3])/20;sy=(calibrated[4]-original[4])/20
   if abs(sx)<.0001 or abs(sy)<.0001:raise RuntimeError('mouse motion calibration failed')
   move_mouse((original[3]-calibrated[3])/sx,(original[4]-calibrated[4])/sy);pause(1.)
   rec=subprocess.Popen(['ffmpeg','-y','-f','x11grab','-framerate','30','-window_id',str(window),'-video_size','1920x1080','-draw_mouse','0','-i',os.environ['DISPLAY'],'-t','120','-an','-c:v','libx264','-preset','ultrafast','-crf','21','-threads','2','-pix_fmt','yuv420p',str(out/'bowl.mp4')],stdout=rlog,stderr=subprocess.STDOUT)
   note('record_start');pause(1.);note('walk_to_shore');key('w',4.)
   current=get_pose()
   for _ in range(4):
    if abs(current[4]+1.15)<.015:break
    move_mouse(0,(-1.15-current[4])/sy);current=get_pose()
   pause(.7);note('scoop');scoop();pause(1.8)
   note('carry_to_sand');key('s',1.4);pause(1.)
   if a.plant:
    # Face the rising dry bank: downhill digging extends beyond arm reach.
    move_mouse(3.141592653589793/sx,0);pause(.5)
    note('pocket_the_filled_bowl');key('e',.08);pause(.5)
    note('quick_single_voxel');click();pause(.7)
    x.button(x.d,3,1,0);x.button(x.d,3,0,0);x.flush(x.d);pause(.7)
    note('charge_full_handful');x.button(x.d,1,1,0);x.flush(x.d);pause(1.15)
    x.button(x.d,1,0,0);x.flush(x.d);pause(.5);note('held_handful');pause(1.)
    note('stow_sand_in_pocket');key('e',.08);pause(1.);key('3',.08);pause(.4)
    move_mouse(-.8/sx,0)
    note('set_sand_aside');x.button(x.d,3,1,0);x.button(x.d,3,0,0);x.flush(x.d);pause(1.)
    move_mouse(.8/sx,0)
    note('black_seed_in_hand');key('1',.08);pause(1.5)
    note('seed_in_hole');x.button(x.d,3,1,0);x.button(x.d,3,0,0);x.flush(x.d);pause(1.5)
    move_mouse(.8/sx,0)
    note('gather_burial_handful');x.button(x.d,1,1,0);x.flush(x.d);pause(1.15);x.button(x.d,1,0,0);x.flush(x.d);pause(.5)
    move_mouse(-.8/sx,0)
    note('bury_with_real_sand');x.button(x.d,3,1,0);x.button(x.d,3,0,0);x.flush(x.d);pause(1.5)
    key('2',.08);pause(.3)
    note('step_closer_to_water_seed');key('w',.13);pause(.4)
   note('pour');x.button(x.d,3,1,0);x.flush(x.d)
   try:pause(2.)
   finally:x.button(x.d,3,0,0);x.flush(x.d)
   pause(3.);note('inspect_wet_sand');pause(2.)
   if a.plant:
    key('e',.08);note('watch_growth');pause(28.)
    note('step_back_for_view');key('s',.8)
    move_mouse(0,.5/sy)
    note('seedling_view');pause(5.)
   else:
    note('return_to_pond');key('w',1.4);pause(.6);scoop();pause(1.5)
    note('carry_second_bowl');key('s',1.2);pause(.8)
    note('tip_and_spill');key('q',2.);pause(4.)
   note('close_and_save');x.request_close(window,app.pid);app.wait(timeout=90)
   if app.returncode:raise RuntimeError('game failed on exit')
   rec.terminate();rec.wait(timeout=15)
   report=json.loads((out/'game.json').read_text());b=report.get('bowl',{})
   if not b.get('conserved') or b.get('scooped_units',0)<8 or (not a.plant and b.get('spilled_units',0)==0) or (b.get('absorbed_units',0)+(b.get('plant_transferred',0) if a.plant else 0))==0 or b.get('units')!=0:raise RuntimeError(f'bowl action checks failed: {b}')
   if a.plant:
    pockets=b.get('pockets',{})
    if not pockets.get('inventory_valid') or not pockets.get('loose_conserved') or pockets.get('picked_voxels',0)<8 or pockets.get('placed_voxels',0)<8:raise RuntimeError(f'pocket/handful actions failed: {pockets}')
    garden=b.get('garden',{})
    if garden.get('plants')!=1 or garden.get('germinated')!=1 or garden.get('tissue_cells',0)<6 or not garden.get('water_ledger_matches'):raise RuntimeError(f'planting/growth failed: {garden}')
   pictures=[('filled','carry_to_sand'),('wet_sand','inspect_wet_sand')]+([('seedling','seedling_view'),('black_seed','black_seed_in_hand'),('handful','held_handful'),('pocketed_sand','stow_sand_in_pocket'),('placed_seed','seed_in_hole'),('buried_seed','bury_with_real_sand')] if a.plant else [('spill','tip_and_spill')])
   for label,action in pictures:
    begin=next(e['seconds'] for e in events if e['action']=='record_start');at=next(e['seconds'] for e in events if e['action']==action)-begin+(3. if label=='seedling' else .3)
    subprocess.run(['ffmpeg','-y','-loglevel','error','-ss',str(at),'-i',str(out/'bowl.mp4'),'-frames:v','1',str(out/f'{label}.png')],check=True)
   reload_cmd=cmd.copy();reload_cmd[reload_cmd.index('--report')+1]=str(out/'reload.json');reload_cmd+=['--seconds','4']
   with (out/'reload.log').open('w') as reload_log:
    subprocess.run(reload_cmd,cwd=ROOT,stdout=reload_log,stderr=subprocess.STDOUT,timeout=120,check=True)
   restored=json.loads((out/'reload.json').read_text()).get('bowl',{})
   persistent=['units','scooped_units','emitted_units','spilled_units','absorbed_units','damp_cells','deposited_units','conserved']
   if a.plant:
    rg=restored.get('garden',{})
    if not restored.get('conserved') or not rg.get('water_ledger_matches') or rg.get('plants')!=1 or rg.get('tissue_cells',0)<b['garden']['tissue_cells'] or restored['scooped_units']!=b['scooped_units']:raise RuntimeError('disk reload lost plant or water state')
   elif any(restored.get(k)!=b[k] for k in persistent):raise RuntimeError('disk reload changed bowl/moisture ledger')
   (out/'checks.json').write_text(json.dumps({'passed':True,'bowl':b,'frames':report['frames'],'reload_passed':True,'restored_bowl':restored},indent=2)+'\n')
 except Exception as e:failure=str(e);raise
 finally:
  if x:
   x.release_all();x.button(x.d,3,0,0);x.flush(x.d)
   if app and app.poll() is None and window:x.request_close(window,app.pid)
  for process in [rec,app]:
   if process and process.poll() is None:process.terminate();process.wait(timeout=15)
  if x:x.close(x.d)
  (out/'manifest.json').write_text(json.dumps({'command':cmd,'events':events,'failure':failure,'binary_sha256':hashlib.sha256(binary.read_bytes()).hexdigest(),'scope':__doc__,'recording_has_overhead':True},indent=2)+'\n')
if __name__=='__main__':main()
