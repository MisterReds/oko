#!/usr/bin/env python3
"""Offline regression tests for GitHub installer atomicity, rollback, uninstall."""
import hashlib
import os
from pathlib import Path
import subprocess
import tempfile

ROOT=Path(__file__).resolve().parents[1]
def call(*args,env=None,success=True):
    r=subprocess.run(["bash",str(ROOT/"scripts"/args[0]),*args[1:]],
                     env=env,capture_output=True,text=True,timeout=30)
    if success and r.returncode:
        raise AssertionError(f"command failed: {args}\n{r.stdout}\n{r.stderr}")
    if not success and r.returncode==0:
        raise AssertionError(f"command unexpectedly passed: {args}")
    return r
def asset(role,ver):
    return f"Oko{role}-{ver}-x86_64.AppImage"

with tempfile.TemporaryDirectory(prefix="oko-install-ci-") as t:
    base=Path(t)
    home=base/"home with spaces"
    home.mkdir()
    data=base/"data with spaces"
    repos=base/"releases"
    config=base/"config"
    config.mkdir()
    (config/"native-student.json").write_text('{"server":"wss://example.invalid","token":"test-not-real"}')
    release_files={}
    for version in ("0.3.0","0.4.0"):
        stage=repos/version
        stage.mkdir(parents=True)
        lines=[]
        for role in ("Teacher","Student"):
            name=asset(role,version)
            contents=f"mock AppImage {role} version {version}".encode()
            (stage/name).write_bytes(contents)
            lines.append(hashlib.sha256(contents).hexdigest()+"  "+name+"\n")
        (stage/"SHA256SUMS").write_text("".join(lines))
    env=os.environ.copy()
    env.update({"HOME":str(home),"XDG_DATA_HOME":str(data),
                "XDG_CONFIG_HOME":str(config),"OKO_VERSION":"v0.3.0",
                "OKO_RELEASE_BASE":"file://"+str(repos/"0.3.0")})
    call("install.sh","both",env=env)
    bin=data/"oko/bin"
    assert (bin/"OkoTeacher.AppImage").read_bytes()==b"mock AppImage Teacher version 0.3.0"
    assert (bin/"OkoStudent.AppImage").read_bytes()==b"mock AppImage Student version 0.3.0"
    assert (data/"applications/oko-native-student.desktop").is_file()
    print("PASS first install with spaces in HOME / XDG_DATA_HOME")
    (repos/"0.4.0"/asset("Student","0.4.0")).write_bytes(b"corrupted")
    env.update({"OKO_VERSION":"v0.4.0","OKO_RELEASE_BASE":"file://"+str(repos/"0.4.0")})
    call("install.sh","both",env=env,success=False)
    assert (bin/"OkoTeacher.AppImage").read_bytes()==b"mock AppImage Teacher version 0.3.0"
    assert (bin/"OkoStudent.AppImage").read_bytes()==b"mock AppImage Student version 0.3.0"
    print("PASS failed checksum prevents partial update")
    (repos/"0.4.0"/asset("Student","0.4.0")).write_bytes(b"mock AppImage Student version 0.4.0")
    # Simulate consented v0.3.0 XDG autostart that used a shadow copy.
    autostart=config/"autostart/oko-student.desktop"
    autostart.parent.mkdir(exist_ok=True)
    autostart.write_text("[Desktop Entry]\\nType=Application\\nExec=/old/opt/OkoStudent.AppImage --background-agent\\nX-GNOME-Autostart-enabled=true\\n".replace("\\n","\n"))
    call("install.sh","both",env=env)
    migrated=autostart.read_text()
    assert str(bin/"OkoStudent.AppImage") in migrated
    assert "/old/opt/" not in migrated
    print("PASS v0.3 enabled XDG autostart migrates to stable updater path")
    assert (bin/"OkoTeacher.AppImage").read_bytes()==b"mock AppImage Teacher version 0.4.0"
    assert (bin/"OkoStudent.AppImage").read_bytes()==b"mock AppImage Student version 0.4.0"
    assert (data/"oko/previous/Teacher").exists()
    assert (data/"oko/previous/Student").exists()
    assert (config/"native-student.json").is_file()
    print("PASS update both roles and preserve user config")
    call("rollback.sh","both",env=env)
    assert (bin/"OkoTeacher.AppImage").read_bytes()==b"mock AppImage Teacher version 0.3.0"
    assert (bin/"OkoStudent.AppImage").read_bytes()==b"mock AppImage Student version 0.3.0"
    call("install.sh","status",env=env)
    print("PASS rollback both roles")
    call("uninstall.sh","student","--purge",env=env,success=False)
    assert (bin/"OkoStudent.AppImage").is_file()
    print("PASS invalid partial purge rejected before mutation")
    call("uninstall.sh","student",env=env)
    assert not (bin/"OkoStudent.AppImage").exists()
    assert (bin/"OkoTeacher.AppImage").exists()
    assert (config/"native-student.json").is_file()
    print("PASS uninstall student without touching teacher and settings")
    call("uninstall.sh","both",env=env)
    assert not (bin/"OkoTeacher.AppImage").exists()
    assert (config/"native-student.json").is_file()
    print("PASS full uninstall preserves config")
    call("uninstall.sh","both","--purge",env=env)
    assert not config.joinpath("oko").exists()
    print("PASS explicit purge")
print("INSTALLER_LIFECYCLE_TESTS: PASS")
