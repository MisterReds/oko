#!/usr/bin/env python3
"""Full native launcher WSS/LAN pairing under a real virtual X11 screen."""
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

ROOT=pathlib.Path(__file__).resolve().parents[1]
DESKTOP=ROOT/"desktop/target/release/oko-desktop"
CORE=ROOT/"target/release"
APPIMAGE_TEST=os.environ.get("OKO_TEST_APPIMAGE")=="1"
TEACHER_CMD=([str(ROOT/"dist/OkoTeacher-0.4.0-x86_64.AppImage"),"--appimage-extract-and-run"]
             if APPIMAGE_TEST else [str(DESKTOP),"--teacher"])
STUDENT_CMD=([str(ROOT/"dist/OkoStudent-0.4.0-x86_64.AppImage"),"--appimage-extract-and-run"]
             if APPIMAGE_TEST else [str(DESKTOP),"--student"])

def free_pair():
    for _ in range(100):
        with socket.socket() as a:
            a.bind(("127.0.0.1",0))
            p=a.getsockname()[1]
        if p>=65000:continue
        with socket.socket() as b:
            try:b.bind(("127.0.0.1",p+1))
            except OSError:continue
        return p
    raise RuntimeError("No free adjacent ports")

def wait(fn,label,t=15):
    end=time.monotonic()+t
    while time.monotonic()<end:
        try:
            v=fn()
            if v:return v
        except (OSError,ValueError,urllib.error.URLError):pass
        time.sleep(.15)
    raise AssertionError("Timeout "+label)

with tempfile.TemporaryDirectory(prefix="oko-native-wss-gui-") as home:
    d=pathlib.Path(home)
    binaries=d/"bin"
    binaries.mkdir()
    for b in (DESKTOP,CORE/"oko-master",CORE/"oko-agent"):
        (binaries/b.name).symlink_to(b)
    port=free_pair()
    admin="teacher-ci-integration-abcdef-123"
    pupil="pupil-ci-integration-123-abcdef"
    tdir=d/"teacher";tdir.mkdir()
    sdir=d/"student";sdir.mkdir()
    cfgdir=tdir/".config/oko";cfgdir.mkdir(parents=True)
    (cfgdir/"native-teacher.json").write_text(json.dumps({
        "token":admin,"agent_token":pupil,"bind_lan":True,"port":port,
    }))
    tenv=os.environ.copy()
    tenv.update({"HOME":str(tdir),"XDG_CONFIG_HOME":str(tdir/".config"),
                 "XDG_STATE_HOME":str(tdir/".local/state"),"OKO_BIN_DIR":str(binaries),
                 "LIBGL_ALWAYS_SOFTWARE":"1"})
    teacher=subprocess.Popen(TEACHER_CMD,env=tenv,
        stdout=subprocess.DEVNULL,stderr=open(d/"teacher.log","wb"))
    student_ui=None
    student_pid=None
    try:
        fp=wait(lambda:(cfgdir/"tls/fingerprint.txt").read_text().strip() if (cfgdir/"tls/fingerprint.txt").exists() else None,"teacher generates fingerprint")
        assert len(fp)==64
        student_config=sdir/".config/oko";student_config.mkdir(parents=True)
        (student_config/"native-student.json").write_text(json.dumps({
            "server":f"wss://127.0.0.1:{port+1}","token":pupil,
            "fingerprint":fp,"name":"CI-X11-Student","autostart":False,
        }))
        senv=tenv.copy()
        senv.update({"HOME":str(sdir),"XDG_CONFIG_HOME":str(sdir/".config"),
                     "XDG_STATE_HOME":str(sdir/".local/state")})
        subprocess.run(STUDENT_CMD+["--background-agent"],env=senv,
            check=True,timeout=7)
        pidfile=sdir/".local/state/oko/student.pid"
        student_pid=int(pidfile.read_text())
        student_ui=subprocess.Popen(STUDENT_CMD,env=senv,
            stdout=subprocess.DEVNULL,stderr=open(d/"student-ui.log","wb"))
        def state():
            req=urllib.request.Request(f"http://127.0.0.1:{port}/api/state",
                headers={"Authorization":"Bearer "+admin})
            with urllib.request.urlopen(req,timeout=2) as f:
                agents=json.load(f)["agents"]
            return next((a for a in agents if a["name"]=="CI-X11-Student" and a["last_frame_ms"]<2500 and a["backend"]=="x11"),None)
        a=wait(state,"student native GUI configuration streams over WSS")
        assert a["screen"]==[800,600] or (a["screen"][0]>0 and a["screen"][1]>0)
        assert teacher.poll() is None and student_ui.poll() is None
        print("PASS native Teacher+Student GUI, TLS fingerprint pairing, X11 screen frames")
    finally:
        if student_pid:
            try:os.kill(student_pid,signal.SIGTERM)
            except ProcessLookupError:pass
        for p in (student_ui,teacher):
            if p and p.poll() is None:
                p.terminate()
                try:p.wait(timeout=6)
                except subprocess.TimeoutExpired:p.kill()
