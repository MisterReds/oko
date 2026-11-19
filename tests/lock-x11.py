#!/usr/bin/env python3
"""Real X11 input lock/unlock, fail-safe, and authenticated agent integration."""
import ctypes
import json
import os
import pathlib
import signal
import socket
import subprocess
import tempfile
import time
import urllib.request
import urllib.error

ROOT = pathlib.Path(__file__).resolve().parents[1]
BIN = ROOT / "target/release"
TK = "admin-integration-secret-abcdef123456"
AK = "pupil-integration-secret-123456abcdef"

def free_port():
    s=socket.socket()
    s.bind(("127.0.0.1",0))
    port=s.getsockname()[1]
    s.close()
    return port

def wait_for(fn, timeout=10, label="condition"):
    deadline=time.monotonic()+timeout
    while time.monotonic()<deadline:
        try:
            result=fn()
            if result: return result
        except (OSError,urllib.error.URLError,ValueError): pass
        time.sleep(.12)
    raise AssertionError(f"Timed out waiting for {label}")

def state(port):
    req=urllib.request.Request(
        f"http://127.0.0.1:{port}/api/state",
        headers={"Authorization":"Bearer "+TK})
    with urllib.request.urlopen(req,timeout=2) as f:
        return json.load(f)["agents"]

def command(port,id,action):
    data=json.dumps({"agent":id,"action":action}).encode()
    req=urllib.request.Request(
        f"http://127.0.0.1:{port}/api/command",
        data=data, headers={"Content-Type":"application/json","Authorization":"Bearer "+TK})
    with urllib.request.urlopen(req,timeout=3) as f:
        assert f.status==200

def keyboard_available(xlib, display, root):
    status=xlib.XGrabKeyboard(display,root,0,1,1,0)
    if status==0:
        xlib.XUngrabKeyboard(display,0)
        xlib.XFlush(display)
        return True
    if status==1: return False
    raise AssertionError(f"Unexpected XGrabKeyboard status {status}")

def pointer_available(xlib, display, root):
    status=xlib.XGrabPointer(display,root,0,0,1,1,0,0,0)
    if status==0:
        xlib.XUngrabPointer(display,0)
        xlib.XFlush(display)
        return True
    if status==1:return False
    raise AssertionError(f"Unexpected XGrabPointer status {status}")

def check():
    port=free_port()
    num=122 + (port % 100)
    display_name=f":{num}"
    with tempfile.TemporaryDirectory(prefix="oko-x11-test-") as folder:
        log=pathlib.Path(folder)
        xvfb_log=open(log/"xvfb.log","wb")
        master_log=open(log/"master.log","wb")
        agent_log=open(log/"agent.log","wb")
        xvfb=subprocess.Popen(["Xvfb",display_name,"-screen","0","800x600x24","-nolisten","tcp"],stdout=xvfb_log,stderr=subprocess.STDOUT)
        master=None
        agent=None
        display=None
        try:
            env=os.environ.copy()
            env["DISPLAY"]=display_name
            env["XDG_SESSION_TYPE"]="x11"
            env["OKO_TOKEN"]=TK
            env["OKO_AGENT_TOKEN"]=AK
            env["LIBGL_ALWAYS_SOFTWARE"]="1"
            wait_for(lambda:pathlib.Path(f"/tmp/.X11-unix/X{num}").exists(),label="Xvfb socket")
            xlib=ctypes.CDLL("libX11.so.6")
            xlib.XOpenDisplay.argtypes=[ctypes.c_char_p]
            xlib.XOpenDisplay.restype=ctypes.c_void_p
            xlib.XDefaultRootWindow.argtypes=[ctypes.c_void_p]
            xlib.XDefaultRootWindow.restype=ctypes.c_ulong
            xlib.XGrabKeyboard.argtypes=[ctypes.c_void_p,ctypes.c_ulong,ctypes.c_int,ctypes.c_int,ctypes.c_int,ctypes.c_ulong]
            xlib.XGrabKeyboard.restype=ctypes.c_int
            xlib.XUngrabKeyboard.argtypes=[ctypes.c_void_p,ctypes.c_ulong]
            xlib.XGrabPointer.argtypes=[ctypes.c_void_p,ctypes.c_ulong,ctypes.c_int,
                ctypes.c_uint,ctypes.c_int,ctypes.c_int,ctypes.c_ulong,ctypes.c_ulong,ctypes.c_ulong]
            xlib.XGrabPointer.restype=ctypes.c_int
            xlib.XUngrabPointer.argtypes=[ctypes.c_void_p,ctypes.c_ulong]
            xlib.XFlush.argtypes=[ctypes.c_void_p]
            xlib.XCloseDisplay.argtypes=[ctypes.c_void_p]
            display=xlib.XOpenDisplay(display_name.encode())
            assert display,"Cannot open test X server"
            root=xlib.XDefaultRootWindow(display)
            assert keyboard_available(xlib,display,root)
            assert pointer_available(xlib,display,root)
            master=subprocess.Popen([str(BIN/"oko-master"),"--port",str(port)],env=env,stdout=subprocess.DEVNULL,stderr=master_log)
            wait_for(lambda:bool(state(port)==[]),label="master state")
            pupil_env=env.copy();pupil_env["OKO_TOKEN"]=AK
            agent=subprocess.Popen([str(BIN/"oko-agent"),"--server",f"ws://127.0.0.1:{port}","--fps","5","--name","LockTest"],env=pupil_env,stdout=subprocess.DEVNULL,stderr=agent_log)
            info=wait_for(lambda:next((a for a in state(port) if a["backend"]=="x11" and a["last_frame_ms"]<3000),None),label="real X11 frames")
            id=info["id"]
            command(port,id,"lock")
            wait_for(lambda:next((a for a in state(port) if a["id"]==id and a["locked"]),None),label="lock ack")
            assert not keyboard_available(xlib,display,root),"Keyboard not actually grabbed"
            assert not pointer_available(xlib,display,root),"Pointer not actually grabbed"
            print("PASS: real X11 input is grabbed after acknowledged lock")
            command(port,id,"unlock")
            wait_for(lambda:next((a for a in state(port) if a["id"]==id and not a["locked"]),None),label="unlock ack")
            assert keyboard_available(xlib,display,root),"Keyboard remained grabbed"
            assert pointer_available(xlib,display,root),"Pointer remained grabbed"
            print("PASS: real X11 input is released after unlock")
            command(port,id,"lock")
            wait_for(lambda:next((a for a in state(port) if a["id"]==id and a["locked"]),None),label="lock before stop")
            agent.terminate()
            agent.wait(timeout=5)
            wait_for(lambda:keyboard_available(xlib,display,root),label="keyboard fail-safe after agent SIGTERM")
            assert pointer_available(xlib,display,root)
            print("PASS: X11 input grab released when agent exits")
            wait_for(lambda:not state(port),label="server removes disconnected agent")
            print("PASS: server removes disconnected computer")
        except:
            if agent_log: agent_log.flush()
            print((log/"agent.log").read_text()[-1400:])
            raise
        finally:
            # Close Xlib handle while Xvfb is still alive; otherwise libX11 calls exit(1).
            if display:
                xlib.XCloseDisplay(display)
                display=None
            for proc in (agent,master,xvfb):
                if proc and proc.poll() is None:
                    proc.terminate()
                    try:proc.wait(timeout=3)
                    except subprocess.TimeoutExpired:proc.kill()
            for f in (xvfb_log,master_log,agent_log):f.close()

if __name__=="__main__":
    check()
