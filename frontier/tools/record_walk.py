#!/usr/bin/env python3
"""Record actual grounded walking/stop/resume/turns in Frontier's own X11 window.
Requires ffmpeg, libX11 and libXtst; uses no Python packages. Recording adds
CPU/GPU overhead: benchmark separately. Captures only its exact child PID window.
"""
import argparse
import ctypes as C
import ctypes.util
import json
import hashlib
import math
import os
from pathlib import Path
import subprocess
import time
U=C.c_ulong; I=C.c_int; P=C.c_void_p
ROOT=Path(__file__).resolve().parents[1]
class MessageData(C.Union):
    _fields_ = [("b", C.c_char * 20), ("s", C.c_short * 10), ("l", C.c_long * 5)]


class ClientMessage(C.Structure):
    _fields_ = [("type", I), ("serial", U), ("send_event", I), ("display", P),
                ("window", U), ("message_type", U), ("format", I), ("data", MessageData)]


class Event(C.Union):
    _fields_ = [("client", ClientMessage), ("pad", C.c_long * 24)]


class X11:
    def __init__(self):
        self.x = C.CDLL(ctypes.util.find_library("X11") or "libX11.so.6")
        self.t = C.CDLL(ctypes.util.find_library("Xtst") or "libXtst.so.6")
        def bind(lib, name, result, *args):
            f = getattr(lib, name); f.restype = result; f.argtypes = args
            return f
        self.open = bind(self.x, "XOpenDisplay", P, C.c_char_p)
        self.close = bind(self.x, "XCloseDisplay", I, P)
        self.root = bind(self.x, "XDefaultRootWindow", U, P)
        self.atom = bind(self.x, "XInternAtom", U, P, C.c_char_p, I)
        self.prop = bind(self.x, "XGetWindowProperty", I, P, U, U, C.c_long, C.c_long,
                         I, U, C.POINTER(U), C.POINTER(I), C.POINTER(U), C.POINTER(U), C.POINTER(P))
        self.tree = bind(self.x, "XQueryTree", I, P, U, C.POINTER(U), C.POINTER(U),
                         C.POINTER(C.POINTER(U)), C.POINTER(C.c_uint))
        self.name = bind(self.x, "XFetchName", I, P, U, C.POINTER(P))
        self.free = bind(self.x, "XFree", I, P)
        self.raise_ = bind(self.x, "XRaiseWindow", I, P, U)
        self.focus_ = bind(self.x, "XSetInputFocus", I, P, U, I, U)
        self.get_focus = bind(self.x, "XGetInputFocus", I, P, C.POINTER(U), C.POINTER(I))
        self.flush = bind(self.x, "XFlush", I, P)
        self.keysym = bind(self.x, "XStringToKeysym", U, C.c_char_p)
        self.keycode = bind(self.x, "XKeysymToKeycode", C.c_ubyte, P, U)
        self.key_event = bind(self.t, "XTestFakeKeyEvent", I, P, C.c_uint, I, U)
        self.button = bind(self.t, "XTestFakeButtonEvent", I, P, C.c_uint, I, U)
        self.motion = bind(self.t, "XTestFakeMotionEvent", I, P, I, I, I, U)
        self.translate = bind(self.x, "XTranslateCoordinates", I, P, U, U, I, I,
                              C.POINTER(I), C.POINTER(I), C.POINTER(U))
        self.send = bind(self.x, "XSendEvent", I, P, U, I, C.c_long, C.POINTER(Event))
        self.d = self.open(None)
        if not self.d:
            raise RuntimeError("Cannot open X11 DISPLAY; this script requires the local X11 session")
        # Stale window handles during discovery must not invoke Xlib's process-exit handler.
        self.error_callback = C.CFUNCTYPE(I, P, P)(lambda _display, _event: 0)
        self.x.XSetErrorHandler.argtypes = [C.CFUNCTYPE(I, P, P)]
        self.x.XSetErrorHandler(self.error_callback)
        self.pid_atom = self.atom(self.d, b"_NET_WM_PID", 0)
        self.held = set()

    def pid(self, window):
        kind = U(); fmt = I(); count = U(); remaining = U(); data = P()
        status = self.prop(self.d, window, self.pid_atom, 0, 1, 0, 0,
                           C.byref(kind), C.byref(fmt), C.byref(count), C.byref(remaining), C.byref(data))
        try:
            return C.cast(data, C.POINTER(U))[0] if status == 0 and data and fmt.value == 32 and count.value else None
        finally:
            if data: self.free(data)

    def title(self, window):
        data = P()
        try:
            return C.string_at(data).decode(errors="replace") if self.name(self.d, window, C.byref(data)) and data else ""
        finally:
            if data: self.free(data)

    def children(self, window):
        root = U(); parent = U(); children = C.POINTER(U)(); count = C.c_uint()
        if not self.tree(self.d, window, C.byref(root), C.byref(parent), C.byref(children), C.byref(count)):
            return []
        try: return [children[i] for i in range(count.value)]
        finally:
            if children: self.free(C.cast(children, P))

    def find(self, pid):
        queue = [(self.root(self.d), 0)]
        for window, depth in queue:
            # PID is mandatory: a matching title alone never authorizes input.
            if self.pid(window) == pid and "frontier" in self.title(window).lower():
                return window
            if depth < 5 and len(queue) < 2000:
                queue.extend((child, depth + 1) for child in self.children(window))
        return None

    def focus(self, window, pid):
        if self.pid(window) != pid: raise RuntimeError("Frontier window no longer belongs to our child")
        self.raise_(self.d, window); self.focus_(self.d, window, 1, 0); self.flush(self.d)
        time.sleep(0.08)
        actual = U(); revert = I(); self.get_focus(self.d, C.byref(actual), C.byref(revert))
        if actual.value != window and self.pid(actual.value) != pid:
            raise RuntimeError("Focus did not reach our Frontier window; refusing keyboard input")

    def key(self, name, down):
        code = self.keycode(self.d, self.keysym(name.encode()))
        if not code: raise RuntimeError("Unknown X11 key: " + name)
        self.key_event(self.d, code, int(down), 0); self.flush(self.d)
        if down: self.held.add(name)
        else: self.held.discard(name)

    def release_all(self):
        for name in list(self.held): self.key(name, False)
        self.button(self.d, 1, 0, 0); self.flush(self.d)

    def click(self, window):
        x = I(); y = I(); child = U()
        self.translate(self.d, window, self.root(self.d), 640, 360, C.byref(x), C.byref(y), C.byref(child))
        self.motion(self.d, -1, x.value, y.value, 0)
        self.button(self.d, 1, 1, 0); self.button(self.d, 1, 0, 0); self.flush(self.d)

    def request_close(self, window, pid):
        if self.pid(window) != pid: return
        event = Event(); event.client.type = 33; event.client.display = self.d
        event.client.window = window; event.client.message_type = self.atom(self.d, b"WM_PROTOCOLS", 0)
        event.client.format = 32; event.client.data.l[0] = self.atom(self.d, b"WM_DELETE_WINDOW", 0)
        event.client.data.l[1] = 0
        self.send(self.d, window, 0, 0, C.byref(event)); self.flush(self.d)


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',type=Path,default=ROOT/'target/release/frontier')
    parser.add_argument('--output',type=Path,required=True)
    parser.add_argument('--settle-seconds',type=float,default=0.,help='Wait before recording and inputs; does not lock interactive input')
    parser.add_argument('game_args',nargs=argparse.REMAINDER,help='Optional game arguments after --')
    args=parser.parse_args()
    if not math.isfinite(args.settle_seconds) or not 0 <= args.settle_seconds <= 120:
        parser.error('--settle-seconds must be between 0 and 120')
    out=args.output.resolve()
    binary=args.binary.resolve();extra=args.game_args
    if extra[:1]==['--']:extra=extra[1:]
    if '--adaptive-wait-quality' in extra or '--benchmark-input-lock' in extra:
        parser.error('These benchmark flags lock player input; use --settle-seconds for a walking recording')
    out.mkdir(parents=True,exist_ok=False)
    binary_hash=hashlib.sha256(binary.read_bytes()).hexdigest()
    command=[str(binary),'--preset','alpine','--width','1280','--height','720','--seconds',str(28+args.settle_seconds),'--report',str(out/'game.json')]+extra
    events=[];app=rec=x=window=None;started=time.monotonic();failure=None
    movement=None
    def note(action):events.append({'seconds_from_launch':round(time.monotonic()-started,4),'action':action})
    try:
        with (out/'game.log').open('w') as log,(out/'record.log').open('w') as rlog:
            app=subprocess.Popen(command,cwd=ROOT,stdout=log,stderr=subprocess.STDOUT);x=X11()
            deadline=time.monotonic()+120
            while time.monotonic()<deadline:
                if app.poll() is not None:raise RuntimeError('Game exited before its window was ready')
                window=x.find(app.pid)
                if window and 'fps' in x.title(window).lower():break
                time.sleep(.15)
            else:raise RuntimeError('No rendered Frontier window for child PID')
            x.focus(window,app.pid);x.click(window)
            if args.settle_seconds:
                note('settle_begin')
                time.sleep(args.settle_seconds)
                if app.poll() is not None:raise RuntimeError('Game exited during settling')
                note('settle_end')
            note('record_start')
            rec=subprocess.Popen(['ffmpeg','-y','-f','x11grab','-framerate','20','-window_id',str(window),'-video_size','1280x720','-i',os.environ['DISPLAY'],'-t','20','-an','-c:v','libx264','-preset','ultrafast','-crf','25','-threads','2','-pix_fmt','yuv420p',str(out/'walking.mp4')],stdout=rlog,stderr=subprocess.STDOUT)
            time.sleep(1)
            for key,duration in [('w',3),(None,2),('w',3)]:
                x.focus(window,app.pid);note('hold_'+str(key))
                if key:x.key(key,True)
                try:time.sleep(duration)
                finally:
                    if key:x.key(key,False)
            # A gradual mouse turn, then real grounded forward walking again.
            x.focus(window,app.pid);note('mouse_turn')
            relative=x.t.XTestFakeRelativeMotionEvent
            relative.restype=I;relative.argtypes=[P,I,I,U]
            for _ in range(40):relative(x.d,8,0,0);x.flush(x.d);time.sleep(.04)
            x.focus(window,app.pid);x.key('w',True);note('walk_after_turn')
            try:time.sleep(3)
            finally:x.key('w',False)
            note('stop_after_turn');rec.wait(timeout=15)
            if rec.returncode:raise RuntimeError('ffmpeg failed; see record.log')
            note('record_end');app.wait(timeout=15)
            report=json.loads((out/'game.json').read_text())
            observations=report['frame_observations']['frames']
            first=observations[0]['eye']
            displacement=max(math.dist(first,frame['eye']) for frame in observations)
            yaw_range=max(frame['yaw'] for frame in observations)-min(frame['yaw'] for frame in observations)
            movement={'max_displacement_m':displacement,'yaw_range_radians':yaw_range,
                      'walk_verified':displacement>1.,'turn_verified':yaw_range>0.2}
            if not movement['walk_verified'] or not movement['turn_verified']:
                raise RuntimeError(f'Walking/turn inputs did not move the camera: {movement}')
    except Exception as error:
        failure=str(error);raise
    finally:
        if x:
            x.release_all()
            if app and app.poll() is None and window:x.request_close(window,app.pid)
        for process in [rec,app]:
            if process and process.poll() is None:
                try:process.wait(timeout=4)
                except subprocess.TimeoutExpired:process.terminate();process.wait(timeout=5)
        if x:x.close(x.d)
        (out/'manifest.json').write_text(json.dumps({'command':command,'binary_sha256':binary_hash,'settle_seconds':args.settle_seconds,'movement':movement,'failure':failure,'events':events,'scope':'Actual X11 inputs and 20 FPS child-window recording; normal startup unless an explicit settle delay is given. Recording perturbs timings; inspect synchronized game.json observations and benchmark separately. Mouse turn delivery depends on X11 input support.'},indent=2)+'\n')
if __name__=='__main__':main()
