#!/usr/bin/env bash
set -euo pipefail
umask 077
REPO="MisterReds/oko"
MODE="${1:-both}"
if [[ ! "$MODE" =~ ^(teacher|student|both)$ ]]; then
  echo 'Использование: bash scripts/install.sh [teacher|student|both]' >&2
  exit 2
fi
[[ "$(uname -s)" == Linux ]] || { echo 'Требуется Linux' >&2; exit 2; }
ARCH="$(uname -m)"
[[ "$ARCH" == x86_64 ]] || {
  echo "Для $ARCH AppImage пока не собрана; см. scripts/build-local.sh" >&2
  exit 2
}
for c in curl sha256sum python3 install; do
  command -v "$c" >/dev/null || { echo "Не найден $c" >&2; exit 2; }
done
DEST="${XDG_DATA_HOME:-$HOME/.local/share}/oko"
BIN="$DEST/bin"
APPS="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
mkdir -p "$BIN" "$APPS"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
if [[ -n "${OKO_VERSION:-}" ]]; then
  TAG="$OKO_VERSION"
  [[ "$TAG" == v* ]] || TAG="v$TAG"
else
  META="$(curl -fsSL --retry 2 "https://api.github.com/repos/$REPO/releases/latest")" || {
    echo "Нет доступного опубликованного GitHub Release." >&2; exit 1;
  }
  TAG="$(printf '%s' "$META" | python3 -c 'import sys,json;print(json.load(sys.stdin).get("tag_name",""))')"
fi
[[ "$TAG" =~ ^v[0-9]+\.[0-9]+\.[0-9]+([.-][0-9A-Za-z.-]+)?$ ]] || {
  echo "Некорректная версия: $TAG" >&2; exit 2;
}
VERSION="${TAG#v}"
BASE="https://github.com/$REPO/releases/download/$TAG"
curl -fL --retry 2 "$BASE/SHA256SUMS" -o "$TMP/SHA256SUMS" || {
  echo "Не удалось скачать SHA256SUMS для $TAG" >&2; exit 1;
}
download() {
  local role="$1" name="Oko$1-$VERSION-x86_64.AppImage"
  echo "Загрузка $name"
  curl -fL --retry 2 "$BASE/$name" -o "$TMP/$name"
  grep -Fx "$(grep -F "  $name" "$TMP/SHA256SUMS" | head -1)" "$TMP/SHA256SUMS" > "$TMP/checksum"
  [[ -s "$TMP/checksum" ]] || { echo "Нет SHA256 для $name" >&2; exit 1; }
  (cd "$TMP" && sha256sum -c checksum)
  local dst="$BIN/Oko$role.AppImage"
  install -m 755 "$TMP/$name" "$dst"
  local r="student" label="ОКО — Ученик"
  if [[ "$role" == Teacher ]]; then r=teacher; label="ОКО — Преподаватель"; fi
  cat > "$APPS/oko-native-$r.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=$label
Comment=Нативная программа ОКО
Exec="$dst" --appimage-extract-and-run
Icon=computer
Categories=Education;Network;
Terminal=false
EOF
  echo "Установлено: $dst"
}
case "$MODE" in
 teacher) download Teacher ;;
 student) download Student ;;
 both) download Teacher; download Student ;;
esac
if command -v update-desktop-database >/dev/null; then
  update-desktop-database "$APPS" >/dev/null 2>&1 || true
fi
echo "ОКО $VERSION установлено без sudo. Найдите ОКО в меню приложений."
