#!/usr/bin/env python3
"""Drive only our own game window: scoop pond water, carry, pour, refill, spill.
The MP4 records actual input and simulation. No scripted water state is injected.
"""
import argparse,ctypes as C,hashlib,json,os,subprocess,time
from pathlib import Path
from record_walk import X11,ROOT,I,P,U

def main():
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('--output',type=Path,required=True);a=p.parse_args();out=a.output.resolve();out.mkdir(parents=True,exist_ok=False)
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
 def scoop():
  before=(out/'game.log').read_text().count('bowl scoop accepted')
  for i in range(7):
   click();pause(.45)
   if (out/'game.log').read_text().count('bowl scoop accepted')>before:return
   key('w',.4);pause(.2)
  raise RuntimeError('could not scoop pond water through actual input')
 try:
  with (out/'game.log').open('w') as log,(out/'record.log').open('w') as rlog:
   app=subprocess.Popen(cmd,cwd=ROOT,stdout=log,stderr=subprocess.STDOUT);x=X11();deadline=time.monotonic()+120
   while time.monotonic()<deadline:
    if app.poll() is not None:raise RuntimeError('game startup failed')
    window=x.find(app.pid)
    if window and 'fps' in x.title(window).lower():break
    time.sleep(.1)
   else:raise RuntimeError('no owned game window')
   x.focus(window,app.pid);x.click(window);pause(12.)
   rec=subprocess.Popen(['ffmpeg','-y','-f','x11grab','-framerate','30','-window_id',str(window),'-video_size','1920x1080','-draw_mouse','0','-i',os.environ['DISPLAY'],'-t','120','-an','-c:v','libx264','-preset','ultrafast','-crf','21','-threads','2','-pix_fmt','yuv420p',str(out/'bowl.mp4')],stdout=rlog,stderr=subprocess.STDOUT)
   note('record_start');pause(1.);note('walk_to_shore');key('w',4.)
   relative=x.t.XTestFakeRelativeMotionEvent;relative.restype=I;relative.argtypes=[P,I,I,U]
   for _ in range(30):relative(x.d,0,18,0);x.flush(x.d);pause(.025)
   pause(.7);note('scoop');scoop();pause(1.8)
   note('carry_to_sand');key('s',1.4);pause(1.)
   note('pour');x.button(x.d,3,1,0);x.flush(x.d)
   try:pause(2.)
   finally:x.button(x.d,3,0,0);x.flush(x.d)
   pause(3.);note('inspect_wet_sand');pause(2.)
   note('return_to_pond');key('w',1.4);pause(.6);scoop();pause(1.5)
   note('carry_second_bowl');key('s',1.2);pause(.8)
   note('tip_and_spill');key('q',2.);pause(4.)
   note('close_and_save');x.request_close(window,app.pid);app.wait(timeout=90)
   if app.returncode:raise RuntimeError('game failed on exit')
   rec.terminate();rec.wait(timeout=15)
   report=json.loads((out/'game.json').read_text());b=report.get('bowl',{})
   if not b.get('conserved') or b.get('scooped_units',0)<8 or b.get('spilled_units',0)==0 or b.get('absorbed_units',0)==0 or b.get('units')!=0:raise RuntimeError(f'bowl action checks failed: {b}')
   for label,action in [('filled','carry_to_sand'),('wet_sand','inspect_wet_sand'),('spill','tip_and_spill')]:
    begin=next(e['seconds'] for e in events if e['action']=='record_start');at=next(e['seconds'] for e in events if e['action']==action)-begin+.3
    subprocess.run(['ffmpeg','-y','-loglevel','error','-ss',str(at),'-i',str(out/'bowl.mp4'),'-frames:v','1',str(out/f'{label}.png')],check=True)
   reload_cmd=cmd.copy();reload_cmd[reload_cmd.index('--report')+1]=str(out/'reload.json');reload_cmd+=['--seconds','4']
   with (out/'reload.log').open('w') as reload_log:
    subprocess.run(reload_cmd,cwd=ROOT,stdout=reload_log,stderr=subprocess.STDOUT,timeout=120,check=True)
   restored=json.loads((out/'reload.json').read_text()).get('bowl',{})
   persistent=['units','scooped_units','emitted_units','spilled_units','absorbed_units','damp_cells','deposited_units','conserved']
   if any(restored.get(k)!=b[k] for k in persistent):raise RuntimeError('disk reload changed bowl/moisture ledger')
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
