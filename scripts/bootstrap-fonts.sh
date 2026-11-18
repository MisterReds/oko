#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET="$ROOT/assets/DejaVuSans.ttf"
if [[ -s "$TARGET" ]]; then
  echo "DejaVu Sans: installed for local build"
  exit 0
fi
for FONT in /usr/share/fonts/truetype/dejavu/DejaVuSans.ttf /usr/share/fonts/TTF/DejaVuSans.ttf; do
  if [[ -f "$FONT" ]]; then
    cp "$FONT" "$TARGET"
    echo "DejaVu Sans copied from installed system font package for compilation"
    exit 0
  fi
done
echo "DejaVu Sans not found. Install fonts-dejavu-core (Debian/Ubuntu) or media-fonts/dejavu (Gentoo/Calculate), then rerun." >&2
exit 2
