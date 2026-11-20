#!/usr/bin/env python3
"""Regression: GitHub source archive contains a usable, unmodified project layout."""
from pathlib import Path
import subprocess
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parents[1]
version = next(
    line.split('"')[1] for line in (ROOT / "Cargo.toml").read_text().splitlines()
    if line.startswith("version = ")
)
archive = ROOT / "dist" / f"OkoNativeSource-{version}.tar.gz"
required = [
    "Cargo.toml", "Cargo.lock", "README.md", "SECURITY.md", "LICENSE",
    "src/bin/master.rs", "src/bin/agent.rs", "src/status.rs",
    "desktop/Cargo.toml", "desktop/Cargo.lock", "desktop/src/main.rs",
    "scripts/build-local.sh", "scripts/bootstrap-fonts.sh", "scripts/doctor.sh",
    "tests/tls-surface.py", "tests/agent-health.py", "docs/RELEASE-1.0-CHECKLIST.md",
]
with tarfile.open(archive, "r:gz") as source:
    members = source.getmembers()
    names = {m.name for m in members}
    for entry in required:
        assert f"OkoNativeSource/{entry}" in names, f"Missing {entry} from source archive"
    assert not any(
        "/target/" in p or "/.git/" in p or p.endswith("DejaVuSans.ttf")
        or p.endswith(".pem") or p.endswith(".env")
        for p in names
    ), "Source archive accidentally includes build artifacts, keys or fonts"
    assert all(
        (m.isfile() or m.isdir()) and not m.name.startswith("/")
        and ".." not in Path(m.name).parts for m in members
    ), "Unsafe symlinks/path traversal in source archive"

    with tempfile.TemporaryDirectory(prefix="oko-source-verify-") as folder:
        # Every member was verified above; compatible with Python 3.10+.
        source.extractall(folder)
        project = Path(folder) / "OkoNativeSource"
        for manifest in [project / "Cargo.toml", project / "desktop/Cargo.toml"]:
            completed = subprocess.run(
                ["cargo", "metadata", "--offline", "--locked", "--no-deps",
                 "--format-version", "1", "--manifest-path", str(manifest)],
                stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
                text=True, timeout=25,
            )
            assert completed.returncode == 0, (
                f"Source archive is not a valid Cargo project: {completed.stderr}"
            )
        doctor = subprocess.run(
            ["bash", "-n", str(project / "scripts/doctor.sh")], timeout=5
        )
        assert doctor.returncode == 0
print(f"PASS: source package v{version} preserves runnable Cargo layout, docs and tests")
