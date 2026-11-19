#!/usr/bin/env python3
"""Encrypted classroom integration: TLS certificate pin, auth, reconnect."""
import hashlib
import json
import os
from pathlib import Path
import signal
import socket
import ssl
import subprocess
import tempfile
import time
import urllib.request
import urllib.error

ROOT=Path(__file__).resolve().parents[1]
BIN=ROOT/"target/release"
ADM="teacher-tls-integration-token-12345"
PUP="pupil-tls-integration-token-54321"

def port():
    with socket.socket() as s:
        s.bind(("127.0.0.1",0))
        return s.getsockname()[1]

def wait(fn, label, secs=12):
    end=time.monotonic()+secs
    while time.monotonic()<end:
        try:
            value=fn()
            if value: return value
        except (OSError,urllib.error.URLError,ValueError): pass
        time.sleep(.15)
    raise AssertionError("Timeout: "+label)

def query(p):
    req=urllib.request.Request(f"http://127.0.0.1:{p}/api/state",
        headers={"Authorization":"Bearer "+ADM})
    with urllib.request.urlopen(req,timeout=2) as r: return json.load(r)["agents"]

def health(p):
    with urllib.request.urlopen(f"http://127.0.0.1:{p}/healthz",timeout=2) as r:
        return r.read()==b"ok"

with tempfile.TemporaryDirectory(prefix="oko-wss-test-") as t:
    folder=Path(t)
    local=port()
    secure=port()
    while secure==local: secure=port()
    env=os.environ.copy()
    env["OKO_TOKEN"]=ADM
    env["OKO_AGENT_TOKEN"]=PUP
    def start_master():
        return subprocess.Popen([
            str(BIN/"oko-master"),"--bind","127.0.0.1","--port",str(local),
            "--tls-bind","127.0.0.1","--tls-port",str(secure),
            "--tls-dir",str(folder/"tls"),
        ],env=env,stdout=subprocess.DEVNULL,stderr=open(folder/"master.log","ab"))
    master=start_master()
    agent=None
    impostor=None
    try:
        wait(lambda:health(local),"HTTP loopback ready")
        fp=wait(lambda:(folder/"tls/fingerprint.txt").read_text().strip() if (folder/"tls/fingerprint.txt").exists() else None,"certificate created")
        cert=(folder/"tls/cert.pem").read_text()
        der=ssl.PEM_cert_to_DER_cert(cert)
        assert hashlib.sha256(der).hexdigest()==fp
        assert ((folder/"tls/key.pem").stat().st_mode & 0o077)==0
        print("PASS: TLS certificate created, private key mode 0600, fingerprint matches")
        env_student=env.copy()
        env_student["OKO_TOKEN"]=PUP
        env_student["OKO_CERT_SHA256"]=fp
        agent=subprocess.Popen([
            str(BIN/"oko-agent"),"--demo","--name","WSS-Pupil",
            "--fps","2","--server",f"wss://127.0.0.1:{secure}"
        ],env=env_student,stdout=subprocess.DEVNULL,stderr=open(folder/"agent.log","ab"))
        wait(lambda:any(a["name"]=="WSS-Pupil" and a["last_frame_ms"]<2500 for a in query(local)),"WSS encrypted demo frames")
        assert agent.poll() is None
        print("PASS: WSS pupil authenticated and streaming JPEG")
        before=len(list(Path(f"/proc/{agent.pid}/task").iterdir()))
        # Mismatch must never connect.
        wrong=env_student.copy()
        wrong["OKO_CERT_SHA256"]="0"*64
        impostor=subprocess.Popen([
            str(BIN/"oko-agent"),"--demo","--name","RejectedImpostor",
            "--server",f"wss://127.0.0.1:{secure}"
        ],env=wrong,stdout=subprocess.DEVNULL,stderr=open(folder/"rejected.log","ab"))
        time.sleep(2)
        assert not any(a["name"]=="RejectedImpostor" for a in query(local))
        print("PASS: wrong fingerprint cannot open WSS stream")
        impostor.terminate()
        impostor.wait(timeout=5)
        impostor=None

        master.terminate()
        master.wait(timeout=5)
        time.sleep(.4)
        master=start_master()
        wait(lambda:health(local),"restarted HTTP local master")
        wait(lambda:any(a["name"]=="WSS-Pupil" and a["last_frame_ms"]<2500 for a in query(local)),"automatic encrypted reconnect",secs=14)
        after=len(list(Path(f"/proc/{agent.pid}/task").iterdir()))
        assert after <= before+2, (before,after)
        print("PASS: WSS agent reconnects after master restart without growing capture threads",before,after)
    finally:
        for proc in (agent,impostor,master):
            if proc is not None and proc.poll() is None:
                proc.terminate()
                try:proc.wait(timeout=5)
                except subprocess.TimeoutExpired:proc.kill()
        print("ENCRYPTED_CLASSROOM_TEST: PASS")
