#!/usr/bin/env bash
# Non-invasive Linux suitability check for Oko. Does not start screen sharing.
set -euo pipefail
version="0.4.2"
echo "ОКО $version — диагностика совместимости (без доступа к экрану)"
echo "ОС: $(uname -s) / $(uname -m)"
if [[ -r /etc/os-release ]]; then
  system="$(sed -n 's/^PRETTY_NAME=//p' /etc/os-release | head -1 | tr -d '"')"
  echo "Дистрибутив: ${system:-не определён}"
fi

fail=0
if [[ "$(uname -s)" != "Linux" || "$(uname -m)" != "x86_64" ]]; then
  echo "НЕ СОВМЕСТИМО: готовые пакеты предназначены для Linux x86-64."
  fail=1
else
  echo "Архитектура: совместима с опубликованными пакетами."
fi

libc="$(getconf GNU_LIBC_VERSION 2>/dev/null || true)"
if [[ "$libc" =~ ^glibc\ ([0-9]+)\.([0-9]+)$ ]]; then
  major="${BASH_REMATCH[1]}"
  minor="${BASH_REMATCH[2]}"
  echo "Библиотека C: glibc $major.$minor"
  if (( major < 2 || (major == 2 && minor < 35) )); then
    echo "НЕ СОВМЕСТИМО: AppImage 0.4.x нужен glibc 2.35+."
    fail=1
  fi
else
  echo "ВНИМАНИЕ: glibc не определён; для musl и других libc готовый GUI не гарантирован."
fi

if command -v ldconfig >/dev/null 2>&1; then
  if ldconfig -p 2>/dev/null | grep -q 'libGL.so.1'; then
    echo "OpenGL: системная библиотека обнаружена (работу GPU это не подтверждает)."
  else
    echo "ВНИМАНИЕ: libGL.so.1 не найдена в ldconfig; проверьте Mesa/драйверы."
  fi
else
  echo "OpenGL: ldconfig недоступен; проверьте libGL.so.1 вручную."
fi

session="${XDG_SESSION_TYPE:-не определён}"
echo "Графический сеанс: $session"
case "$session" in
  x11)
    if [[ -n "${DISPLAY:-}" ]]; then
      echo "X11: DISPLAY задан. Захват требует доступа к сеансу и разрешений X11."
    else
      echo "ВНИМАНИЕ: DISPLAY не задан; запуск приложения не гарантирует захват экрана."
    fi
    ;;
  wayland)
    echo "Wayland: текущий backend использует grim на совместимых wlroots-композиторах."
    if command -v grim >/dev/null 2>&1; then
      echo "grim: найден. Реальная выдача кадров НЕ проверялась."
    else
      echo "grim: не найден."
    fi
    echo "GNOME/KDE Wayland: XDG ScreenCast Portal пока не реализован в ОКО."
    ;;
  *)
    echo "ВНИМАНИЕ: тип графического сеанса неизвестен; проверьте DISPLAY/XDG_SESSION_TYPE."
    ;;
esac

if command -v update-desktop-database >/dev/null 2>&1; then
  echo "Меню приложений: update-desktop-database доступен."
else
  echo "Меню приложений: update-desktop-database отсутствует; XDG-ярлыки могут работать без него."
fi
if [[ -e /dev/fuse ]]; then
  echo "FUSE: устройство найдено. При проблемах используйте --appimage-extract-and-run."
else
  echo "FUSE: устройство отсутствует; используйте --appimage-extract-and-run."
fi
echo "Эта проверка диагностическая: она НЕ подтверждает поддержку Calculate Linux или доступ к экрану."
exit "$fail"
