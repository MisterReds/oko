#!/usr/bin/env bash
# Atomically return to the previously installed Oko binary per role.
set -euo pipefail
MODE="${1:-both}"
[[ "$MODE" =~ ^(teacher|student|both)$ ]] || { echo "Use teacher|student|both" >&2; exit 2; }
ROOT="${XDG_DATA_HOME:-$HOME/.local/share}/oko"
BIN="$ROOT/bin"; PREVIOUS="$ROOT/previous"; VERSIONS="$ROOT/versions"
[[ -d "$ROOT" ]] || { echo "Oko is not installed" >&2; exit 1; }
exec 9>"$ROOT/.installer.lock"; flock -x 9
roles=()
case "$MODE" in
 teacher) roles=(Teacher) ;;
 student) roles=(Student) ;;
 both) roles=(Teacher Student) ;;
esac
for role in "${roles[@]}"; do
  current="$BIN/Oko$role.AppImage"
  prev="$PREVIOUS/$role"
  [[ -e "$current" && -L "$current" && -e "$prev" && -L "$prev" ]] || {
    echo "No previous version available for $role; nothing changed" >&2; exit 1;
  }
  target="$(readlink -f "$prev")"
  [[ "$target" == "$VERSIONS/"* && -f "$target" ]] || {
    echo "Previous path for $role is invalid; nothing changed" >&2; exit 1;
  }
done
for role in "${roles[@]}"; do
  current="$BIN/Oko$role.AppImage"; previous="$PREVIOUS/$role"
  old="$(readlink -f "$current")"; restored="$(readlink -f "$previous")"
  next="$BIN/.Oko$role.AppImage.rollback-$$"
  nextprev="$PREVIOUS/.$role.rollback-$$"
  ln -s "$restored" "$next"
  ln -s "$old" "$nextprev"
  mv -fT "$next" "$current"
  mv -fT "$nextprev" "$previous"
  echo "$role: rolled back to $(basename "$(dirname "$restored")")"
done
echo "Rollback complete. Restart Oko to load the selected version."
