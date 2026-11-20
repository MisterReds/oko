#!/usr/bin/env python3
"""End-to-end health contract: process != connected != streaming."""
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
BIN = ROOT / "target/release"
ADMIN = "teacher-health-integration-secret-12345"
STUDENT = "student-health-integration-secret-54321"

def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]

def wait_for(fn, label, timeout=12):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        try:
            result = fn()
            if result:
                return result
        except (OSError, ValueError, json.JSONDecodeError):
            pass
        time.sleep(.13)
    raise AssertionError(f"Timeout waiting for {label}")

with tempfile.TemporaryDirectory(prefix="oko-health-") as temp:
    base = Path(temp)
    state_dir = base / "state"
    state_file = state_dir / "oko" / "agent-health.json"
    env = os.environ.copy()
    env.update({
        "HOME": str(base), "XDG_STATE_HOME": str(state_dir),
        "OKO_TOKEN": STUDENT,
    })
    port = free_port()
    agent_log = (base / "agent.log").open("wb")
    master_log = (base / "master.log").open("wb")
    agent = None
    master = None

    def health():
        return json.loads(state_file.read_text()) if state_file.exists() else None

    def start_master():
        m_env = env.copy()
        m_env["OKO_TOKEN"] = ADMIN
        m_env["OKO_AGENT_TOKEN"] = STUDENT
        return subprocess.Popen([str(BIN / "oko-master"), "--bind", "127.0.0.1", "--port", str(port)],
                                env=m_env, stdout=subprocess.DEVNULL, stderr=master_log)

    try:
        agent = subprocess.Popen([
            str(BIN / "oko-agent"), "--demo", "--fps", "3",
            "--name", "Health-Test", "--server", f"ws://127.0.0.1:{port}",
        ], env=env, stdout=subprocess.DEVNULL, stderr=agent_log)
        wait_for(lambda: (h if (h := health()) and h["phase"] == "reconnecting" else None),
                 "reconnecting while teacher is offline")
        h = health()
        assert h["pid"] == agent.pid
        assert not h["last_frame_unix"], "must not claim streaming without frames"
        assert "token" not in str(h).lower()
        assert STUDENT not in str(h)
        assert state_file.stat().st_mode & 0o077 == 0
        assert state_file.parent.stat().st_mode & 0o077 == 0
        print("PASS: offline agent reports real connection error and keeps private state")

        master = start_master()
        first = wait_for(lambda: (h if (h := health()) and h["phase"] == "streaming"
                          and h["last_frame_unix"] else None), "first streamed frame")
        initial_frame = first["last_frame_unix"]
        assert first["backend"] == "demo", first
        assert first["screen"][0] > 0
        print("PASS: streaming is reported only after first sent frame")

        master.terminate()
        master.wait(timeout=5)
        master = None
        wait_for(lambda: (h if (h := health()) and h["phase"] == "reconnecting"
                          else None), "reconnect after shutdown")
        print("PASS: disconnect reported, not incorrectly shown online")

        time.sleep(1)
        master = start_master()
        wait_for(lambda: (h if (h := health()) and h["phase"] == "streaming"
                          and h["last_frame_unix"] > initial_frame
                          else None), "frames resume after restart")
        print("PASS: streaming restored with fresher frame timestamp")

        # A denied Wayland screencast must never be confused with a live desktop.
        denied_state_dir = base / "denied-state"
        denied_status = denied_state_dir / "oko" / "agent-health.json"
        fake_bin = base / "fake-bin"
        fake_bin.mkdir()
        grim = fake_bin / "grim"
        grim.write_text("#!/bin/sh\necho 'Screen capture permission denied' >&2\nexit 1\n")
        grim.chmod(0o755)
        denied_env = env.copy()
        denied_env["XDG_STATE_HOME"] = str(denied_state_dir)
        denied_env["XDG_SESSION_TYPE"] = "wayland"
        denied_env["PATH"] = f"{fake_bin}:{env.get('PATH','/usr/bin:/bin')}"
        denied_log = (base / "wayland-denied.log").open("wb")
        denied = subprocess.Popen([
            str(BIN / "oko-agent"), "--name", "Denied-Wayland",
            "--server", f"ws://127.0.0.1:{port}",
        ], env=denied_env, stdout=subprocess.DEVNULL, stderr=denied_log)
        try:
            def capture_denied():
                if not denied_status.exists():return None
                status = json.loads(denied_status.read_text())
                return status if status["phase"] == "capture_error" else None
            denied_health = wait_for(capture_denied, "Wayland permission failure")
            assert denied_health["last_frame_unix"] is None
            assert "Wayland" in (denied_health["last_error"] or "")
            print("PASS: denied Wayland capture reports failure without demo frames")
        finally:
            denied.terminate()
            denied.wait(timeout=5)
            denied_log.close()

    finally:
        for proc in (agent, master):
            if proc is not None and proc.poll() is None:
                proc.terminate()
                try:
                    proc.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    proc.kill()
        agent_log.close()
        master_log.close()
