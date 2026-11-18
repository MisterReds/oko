#!/usr/bin/env python3
"""Headless smoke: actually open teacher + student egui windows under Xvfb."""
import json
import os
import pathlib
import socket
import subprocess
import tempfile
import time
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parents[1]
DESKTOP = ROOT / "desktop" / "target" / "release" / "oko-desktop"
CORE = ROOT / "target" / "release"
assert DESKTOP.is_file() and (CORE / "oko-master").is_file()

def port_free():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]

with tempfile.TemporaryDirectory(prefix="oko-native-smoke-") as td:
    d = pathlib.Path(td)
    binary_dir = d / "bin"
    binary_dir.mkdir()
    for source in (DESKTOP, CORE / "oko-master", CORE / "oko-agent"):
        (binary_dir / source.name).symlink_to(source)
    port = port_free()
    env = os.environ.copy()
    env["HOME"] = str(d / "teacher")
    env["XDG_CONFIG_HOME"] = str(d / "teacher" / ".config")
    env["XDG_STATE_HOME"] = str(d / "teacher" / ".local" / "state")
    env["OKO_BIN_DIR"] = str(binary_dir)
    env["OKO_NATIVE_PORT"] = str(port)
    env["LIBGL_ALWAYS_SOFTWARE"] = "1"
    pathlib.Path(env["HOME"]).mkdir()
    teacher = subprocess.Popen(
        [str(DESKTOP), "--teacher"],
        env=env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    student = None
    try:
        for _ in range(30):
            if teacher.poll() is not None:
                raise AssertionError("Teacher UI crashed: " + teacher.stderr.read().decode()[:1000])
            try:
                with urllib.request.urlopen(f"http://127.0.0.1:{port}/healthz", timeout=1) as r:
                    if r.read() == b"ok":
                        break
            except Exception:
                time.sleep(.2)
        else:
            raise AssertionError("Teacher UI did not launch master")
        cfg = json.loads((d / "teacher" / ".config/oko/native-teacher.json").read_text())
        assert len(cfg["token"]) >= 32 and len(cfg["agent_token"]) >= 32
        assert cfg["token"] != cfg["agent_token"]
        assert cfg["bind_lan"] is False
        env["HOME"] = str(d / "student")
        env["XDG_CONFIG_HOME"] = str(d / "student" / ".config")
        env["XDG_STATE_HOME"] = str(d / "student" / ".local" / "state")
        pathlib.Path(env["HOME"]).mkdir()
        student = subprocess.Popen(
            [str(DESKTOP), "--student"], env=env,
            stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        time.sleep(2)
        assert student.poll() is None, "Student GUI exited unexpectedly"
        teacher.terminate()
        teacher.wait(timeout=5)
        for _ in range(20):
            try:
                urllib.request.urlopen(f"http://127.0.0.1:{port}/healthz", timeout=.4)
            except Exception:
                break
            time.sleep(.3)
        else:
            raise AssertionError("Master survived its parent unexpectedly")
        print("PASS: native teacher/student GUI, distinct keys, server lifecycle")
    finally:
        if student is not None and student.poll() is None:
            student.terminate()
            student.wait(timeout=5)
        if teacher.poll() is None:
            teacher.terminate()
            teacher.wait(timeout=5)
