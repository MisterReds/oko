#!/usr/bin/env bash
# Install/update Oko per user, retain previous version and preserve settings.
set -euo pipefail
umask 077
REPO="MisterReds/oko"
MODE="${1:-both}"
if [[ "$MODE" == --help || "$MODE" == -h ]]; then
  echo "Usage: bash scripts/install.sh [teacher|student|both|status]"
  echo "Rollback: bash scripts/rollback.sh [teacher|student|both]"
  exit 0
fi
[[ "$MODE" =~ ^(teacher|student|both|status)$ ]] || { echo "Invalid role" >&2; exit 2; }
[[ "$(uname -s)" == Linux && "$(uname -m)" == x86_64 ]] || { echo "Requires Linux x86-64" >&2; exit 2; }
for tool in curl sha256sum python3 install mv ln flock readlink; do
  command -v "$tool" >/dev/null || { echo "Missing: $tool" >&2; exit 2; }
done
DATA="${XDG_DATA_HOME:-$HOME/.local/share}"
ROOT="$DATA/oko"
BIN="$ROOT/bin"
VERSIONS="$ROOT/versions"
PREVIOUS="$ROOT/previous"
APPS="$DATA/applications"
mkdir -p "$BIN" "$VERSIONS" "$PREVIOUS" "$APPS"
exec 9>"$ROOT/.installer.lock"
flock -x 9
status() {
  local role="$1" current="$BIN/Oko${1}.AppImage" target
  if [[ -L "$current" && -e "$current" ]]; then
    target="$(readlink -f "$current")"
    if [[ "$target" == "$VERSIONS/"* ]]; then
      target="${target#"$VERSIONS/"}"
      echo "$role: ${target%%/*}"
    else
      echo "$role: previous format"
    fi
  elif [[ -f "$current" ]]; then
    echo "$role: previous format"
  else
    echo "$role: not installed"
  fi
}
if [[ "$MODE" == status ]]; then status Teacher; status Student; exit 0; fi

if [[ -n "${OKO_VERSION:-}" ]]; then
  TAG="$OKO_VERSION"; [[ "$TAG" == v* ]] || TAG="v$TAG"
else
  META="$(curl -fsSL --retry 3 "https://api.github.com/repos/$REPO/releases/latest")" || {
    echo "Cannot retrieve latest GitHub release" >&2; exit 1;
  }
  TAG="$(printf '%s' "$META" | python3 -c 'import json,sys;print(json.load(sys.stdin).get("tag_name",""))')"
fi
[[ "$TAG" =~ ^v[0-9]+\.[0-9]+\.[0-9]+([.-][0-9A-Za-z.-]+)?$ ]] || { echo "Invalid version: $TAG" >&2; exit 2; }
VERSION="${TAG#v}"
BASE="${OKO_RELEASE_BASE:-https://github.com/$REPO/releases/download/$TAG}"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
curl -fsSL --retry 3 "$BASE/SHA256SUMS" -o "$TMP/SHA256SUMS" || {
  echo "Cannot fetch SHA256SUMS" >&2; exit 1;
}
roles=()
case "$MODE" in
  teacher) roles=(Teacher) ;;
  student) roles=(Student) ;;
  both) roles=(Teacher Student) ;;
esac

# Verify every requested AppImage before any installed version changes.
for role in "${roles[@]}"; do
  name="Oko$role-$VERSION-x86_64.AppImage"
  echo "Downloading $name"
  curl -fsSL --retry 3 "$BASE/$name" -o "$TMP/$name" || {
    echo "Download failed: $name; installed versions unchanged" >&2; exit 1;
  }
  expected="$(awk -v target="$name" 'NF==2 && $2==target {print $1}' "$TMP/SHA256SUMS")"
  [[ "$expected" =~ ^[0-9a-fA-F]{64}$ ]] || { echo "Invalid SHA256SUMS for $name" >&2; exit 1; }
  got="$(sha256sum "$TMP/$name")"; got="${got%% *}"
  [[ "${got,,}" == "${expected,,}" ]] || { echo "Checksum mismatch: $name; installed versions unchanged" >&2; exit 1; }
  [[ -s "$TMP/$name" ]] || { echo "Empty file: $name" >&2; exit 1; }
  echo "$name: SHA256 OK"
done

# Stage all verified binaries to immutable version paths.
for role in "${roles[@]}"; do
  path="$VERSIONS/$VERSION"
  mkdir -p "$path"
  temp="$path/.Oko$role.AppImage.tmp-$$"
  install -m 755 "$TMP/Oko$role-$VERSION-x86_64.AppImage" "$temp"
  mv -fT "$temp" "$path/Oko$role.AppImage"
done

for role in "${roles[@]}"; do
  target="$VERSIONS/$VERSION/Oko$role.AppImage"
  current="$BIN/Oko$role.AppImage"
  if [[ -e "$current" || -L "$current" ]]; then
    if [[ -L "$current" ]]; then
      old="$(readlink -f "$current")"
    else
      legacy="$VERSIONS/legacy-$(date +%s)-$$"
      mkdir -p "$legacy"
      old="$legacy/Oko$role.AppImage"
      mv "$current" "$old"
    fi
    if [[ "$old" != "$target" && -f "$old" ]]; then
      temp_prev="$PREVIOUS/.$role.tmp-$$"
      ln -s "$old" "$temp_prev"
      mv -fT "$temp_prev" "$PREVIOUS/$role"
    fi
  fi
  next="$BIN/.Oko$role.AppImage.next-$$"
  ln -s "$target" "$next"
  mv -fT "$next" "$current"
  label="ОКО — Ученик"; lower="student"
  if [[ "$role" == Teacher ]]; then label="ОКО — Преподаватель"; lower="teacher"; fi
  desktop_exec="$(python3 - "$current" <<'PY'
import sys
s=sys.argv[1]
if '\n' in s or '\r' in s: raise SystemExit('Invalid path')
print('"'+s.replace('\\','\\\\').replace('"','\\"').replace('%','%%')+'"')
PY
)"
  draft="$APPS/.oko-native-$lower.desktop.tmp-$$"
  cat > "$draft" <<EOF
[Desktop Entry]
Type=Application
Version=1.0
Name=$label
Comment=ОКО — мониторинг учебного класса
Exec=$desktop_exec --appimage-extract-and-run
Icon=computer
Categories=Education;Network;
Terminal=false
EOF
  chmod 644 "$draft"
  mv -fT "$draft" "$APPS/oko-native-$lower.desktop"
  if [[ "$role" == Student ]]; then
    autostart="${XDG_CONFIG_HOME:-$HOME/.config}/autostart/oko-student.desktop"
    if [[ -f "$autostart" ]]; then
      python3 "$(dirname "${BASH_SOURCE[0]}")/migrate_autostart.py" "$autostart" "$desktop_exec"
    fi
  fi
  echo "Installed $role $VERSION"
done

if command -v update-desktop-database >/dev/null; then
  update-desktop-database "$APPS" >/dev/null 2>&1 || true
fi
echo "Updated without sudo. Previous binaries kept for rollback; configurations preserved."
