#!/usr/bin/env python3
"""Migrate an opted-in XDG autostart entry to the stable, updatable Oko binary."""
from pathlib import Path
import os
import sys

file=Path(sys.argv[1])
exe=sys.argv[2]
if not file.is_file():
    raise SystemExit(0)
raw=file.read_text()
lines=raw.splitlines(keepends=True)
if not any(line.startswith("Exec=") for line in lines):
    raise SystemExit("Existing autostart entry has no Exec field")
replacement="Exec="+exe+" --appimage-extract-and-run --background-agent\n"
updated="".join(replacement if line.startswith("Exec=") else line for line in lines)
temp=file.with_name(file.name+".new-"+str(os.getpid()))
try:
    temp.write_text(updated)
    os.chmod(temp,file.stat().st_mode & 0o777)
    os.replace(temp,file)
finally:
    if temp.exists():
        temp.unlink()
