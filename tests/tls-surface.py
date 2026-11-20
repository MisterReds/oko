#!/usr/bin/env python3
"""Security regression: the pupil-facing TLS listener must NOT expose teacher APIs."""
import json
import os
from pathlib import Path
import socket
import ssl
import subprocess
import tempfile
import time
import urllib.error
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
BIN = ROOT / "target/release/oko-master"
ADMIN = "teacher-integration-token-1234567890"
STUDENT = "student-integration-token-0987654321"

def unused_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]

def request(url, admin=False, method="GET"):
    data = b'{"agent":"bogus","action":"lock"}' if method == "POST" else None
    headers = {"Authorization": f"Bearer {ADMIN}"} if admin else {}
    if data:
        headers["Content-Type"] = "application/json"
    req = urllib.request.Request(url, headers=headers, method=method, data=data)
    try:
        with urllib.request.urlopen(req, context=ssl._create_unverified_context(), timeout=3) as reply:
            return reply.status
    except urllib.error.HTTPError as error:
        return error.code

def websocket_handshake(host, port, path, token, use_tls, bearer=None):
    sock = socket.create_connection((host, port), timeout=3)
    try:
        if use_tls:
            sock = ssl._create_unverified_context().wrap_socket(sock, server_hostname="localhost")
        sock.settimeout(3)
        suffix = f"?token={token}" if token else ""
        text = (f"GET {path}{suffix} HTTP/1.1\r\n"
                f"Host: localhost:{port}\r\n"
                "Upgrade: websocket\r\n"
                "Connection: Upgrade\r\n"
                "Sec-WebSocket-Version: 13\r\n"
                "Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n"
                + (f"Authorization: Bearer {bearer}\r\n" if bearer else "")
                + "\r\n")
        sock.sendall(text.encode("ascii"))
        first = sock.recv(2048).split(b"\r\n", 1)[0]
        return int(first.split()[1])
    finally:
        sock.close()

with tempfile.TemporaryDirectory(prefix="oko-network-boundary-") as work:
    http = unused_port()
    wss = unused_port()
    while wss == http:
        wss = unused_port()
    env = os.environ.copy()
    env["OKO_TOKEN"] = ADMIN
    env["OKO_AGENT_TOKEN"] = STUDENT
    log = (Path(work) / "server.log").open("wb")
    master = subprocess.Popen([str(BIN), "--port", str(http), "--bind", "127.0.0.1",
        "--tls-bind", "127.0.0.1", "--tls-port", str(wss),
        "--tls-dir", str(Path(work) / "tls")],
        env=env, stdout=subprocess.DEVNULL, stderr=log)
    try:
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            if master.poll() is not None:
                raise AssertionError("Master terminated: " + (Path(work) / "server.log").read_text())
            try:
                if request(f"http://127.0.0.1:{http}/healthz") == 200 and \
                   request(f"https://127.0.0.1:{wss}/healthz") == 200:
                    break
            except (OSError, urllib.error.URLError):
                pass
            time.sleep(.12)
        else:
            raise AssertionError("Master did not start both listeners")

        # Even a valid teacher credential MUST NOT grant routes on the pupil network.
        assert request(f"https://127.0.0.1:{wss}/") == 404
        assert request(f"https://127.0.0.1:{wss}/api/state", admin=True) == 404
        assert request(f"https://127.0.0.1:{wss}/api/command", admin=True, method="POST") == 404
        assert websocket_handshake("127.0.0.1", wss, "/ws/console", ADMIN, True) == 404
        print("PASS: no teacher web page, REST API or WebSocket on pupil TLS port")

        assert websocket_handshake("127.0.0.1", wss, "/ws/agent", None, True) == 401
        assert websocket_handshake("127.0.0.1", wss, "/ws/agent", ADMIN, True) == 401
        assert websocket_handshake("127.0.0.1", wss, "/ws/agent", STUDENT, True) == 101
        assert websocket_handshake("127.0.0.1", wss, "/ws/agent", None, True, bearer=STUDENT) == 101
        assert websocket_handshake("127.0.0.1", wss, "/ws/agent", None, True, bearer=ADMIN) == 401
        assert websocket_handshake("127.0.0.1", wss, "/ws/agent", STUDENT, True, bearer=ADMIN) == 401
        print("PASS: TLS accepts pupil token via Authorization or legacy URL; rejects teacher key")

        assert request(f"http://127.0.0.1:{http}/") == 200
        assert request(f"http://127.0.0.1:{http}/api/state", admin=True) == 200
        assert request(f"http://127.0.0.1:{http}/api/state") == 401
        assert websocket_handshake("127.0.0.1", http, "/ws/console", ADMIN, False) == 101
        print("PASS: local teacher UI and authorized API remain functional")
    finally:
        master.terminate()
        try:
            master.wait(timeout=4)
        except subprocess.TimeoutExpired:
            master.kill()
        log.close()
