#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
"$ROOT/scripts/bootstrap-fonts.sh"
cargo test --locked --release --manifest-path "$ROOT/Cargo.toml"
cargo build --locked --release --manifest-path "$ROOT/Cargo.toml"
cargo build --locked --release --manifest-path "$ROOT/desktop/Cargo.toml"
echo "Build complete. Native desktop: desktop/target/release/oko-desktop"
echo "Core agent/master: target/release/oko-agent, target/release/oko-master"
