#!/usr/bin/env bash
# Remove Oko applications without removing personal data, certs or settings.
set -euo pipefail
MODE="${1:-both}"
PURGE="${2:-}"
[[ "$MODE" =~ ^(teacher|student|both)$ ]] || { echo "Use teacher|student|both [--purge]" >&2; exit 2; }
[[ -z "$PURGE" || "$PURGE" == --purge ]] || { echo "Invalid option" >&2; exit 2; }
if [[ "$PURGE" == --purge && "$MODE" != both ]]; then
  echo "Use uninstall.sh both --purge to remove shared settings" >&2
  exit 2
fi
DATA="${XDG_DATA_HOME:-$HOME/.local/share}"
ROOT="$DATA/oko"
APPS="$DATA/applications"
if [[ ! -d "$ROOT" ]]; then
  echo "Oko is not installed"; exit 0
fi
exec 9>"$ROOT/.installer.lock"; flock -x 9
roles=()
case "$MODE" in
 teacher) roles=(Teacher) ;;
 student) roles=(Student) ;;
 both) roles=(Teacher Student) ;;
esac
for role in "${roles[@]}"; do
  lower=student
  [[ "$role" == Teacher ]] && lower=teacher
  if [[ "$role" == Student ]]; then
    # Do not terminate arbitrary PID: check user-owned process command line.
    pidfile="${XDG_STATE_HOME:-$HOME/.local/state}/oko/student.pid"
    if [[ -r "$pidfile" ]]; then
      pid="$(cat "$pidfile")"
      if [[ "$pid" =~ ^[0-9]+$ && -r "/proc/$pid/cmdline" ]] &&
         tr '\0' ' ' < "/proc/$pid/cmdline" | grep -Fq 'oko-agent'; then
        kill -TERM "$pid" 2>/dev/null || true
      fi
      rm -f "$pidfile"
    fi
    rm -f "${XDG_CONFIG_HOME:-$HOME/.config}/autostart/oko-student.desktop"
    # Legacy 0.3 student GUI created a redundant AppImage copy.
    rm -f "$HOME/.local/opt/oko/OkoStudent.AppImage"
  fi
  rm -f "$ROOT/bin/Oko$role.AppImage" "$ROOT/previous/$role" \
    "$APPS/oko-native-$lower.desktop"
  if [[ -d "$ROOT/versions" ]]; then
    find "$ROOT/versions" -mindepth 2 -maxdepth 2 -type f -name "Oko$role.AppImage" -delete
    find "$ROOT/versions" -mindepth 1 -type d -empty -delete
  fi
  echo "$role uninstalled"
done
if [[ "$MODE" == both ]]; then
  rm -rf "$ROOT/bin" "$ROOT/previous" "$ROOT/versions"
  # Don't delete lock while held. It will be cleared once command exits.
fi
if [[ "$PURGE" == --purge ]]; then
  if [[ "$MODE" == both ]]; then
    rm -rf "${XDG_CONFIG_HOME:-$HOME/.config}/oko" "${XDG_STATE_HOME:-$HOME/.local/state}/oko"
    echo "User settings, logs and TLS keys removed by explicit --purge."
  else
    echo "Purge only supported with mode=both to avoid deleting configuration still in use." >&2
    exit 2
  fi
fi
if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database "$APPS" >/dev/null 2>&1 || true
fi
echo "Uninstall complete; settings preserved unless --purge was requested."
