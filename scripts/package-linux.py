from pathlib import Path
from zipfile import ZipFile, ZIP_DEFLATED
import tempfile, shutil, subprocess, os, tarfile, hashlib

root=Path(__file__).resolve().parent.parent
core=root
desktop=root/'desktop'
out=root/'dist'
out.mkdir(parents=True,exist_ok=True)
version='0.2.0'
appimagetool=Path(os.environ.get('APPIMAGETOOL_PATH', str(root/'.cache/appimagetool-x86_64.AppImage')))
if not appimagetool.is_file():
    raise SystemExit('AppImageTool не найден: укажите APPIMAGETOOL_PATH или скачайте инструмент в .cache/')

def put(path,content,mode=0o644):
    path.parent.mkdir(parents=True,exist_ok=True)
    path.write_text(content,encoding='utf-8')
    path.chmod(mode)

with tempfile.TemporaryDirectory(prefix='oko-native-release-') as td:
    tmp=Path(td)
    portable=tmp/'OkoDesktop'
    (portable/'bin').mkdir(parents=True)
    for name,source in [
        ('oko-desktop',desktop/'target/release/oko-desktop'),
        ('oko-master',core/'target/release/oko-master'),
        ('oko-agent',core/'target/release/oko-agent')]:
        dest=portable/'bin'/name
        shutil.copy2(source,dest)
        dest.chmod(0o755)

    svg='''<svg xmlns="http://www.w3.org/2000/svg" width="128" height="128" viewBox="0 0 128 128">
<rect width="128" height="128" rx="30" fill="#0c1017"/>
<circle cx="64" cy="64" r="39" fill="none" stroke="#27cdb5" stroke-width="7"/>
<path d="M25 64C43 35 85 35 103 64 85 93 43 93 25 64Z" fill="#17282f" stroke="#27cdb5" stroke-width="5"/>
<circle cx="64" cy="64" r="15" fill="#27cdb5"/><circle cx="70" cy="58" r="5" fill="#e7faf5"/>
</svg>'''
    put(portable/'oko.svg',svg)
    install=r'''#!/usr/bin/env bash
set -eu
role="$1"
if [ "$role" != teacher ] && [ "$role" != student ] && [ "$role" != both ]; then
  echo "Укажите teacher|student|both"; exit 2
fi
if [ "$(uname -s)" != Linux ] || [ "$(uname -m)" != x86_64 ]; then
  echo "Требуется Linux x86-64"; exit 2
fi
here="$(cd "$(dirname "$0")/.." && pwd)"
target="$HOME/.local/share/oko"
apps="$HOME/.local/share/applications"
mkdir -p "$target/bin" "$apps"
for b in oko-desktop oko-master oko-agent; do
  install -m 755 "$here/bin/$b" "$target/bin/$b"
done
for r in teacher student; do
  cat > "$apps/oko-native-$r.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=ОКО — $r
Exec=$target/bin/oko-desktop --$r
Icon=computer
Terminal=false
Categories=Education;Network;
EOF
done
echo "ОКО установлен: $target/bin"
echo "Запуск: $target/bin/oko-desktop --teacher"
echo "Запуск: $target/bin/oko-desktop --student"
'''
    put(portable/'scripts/install-user.sh',install,0o755)
    readme='''ОКО Native 0.2.0 — Linux x86-64 (Rust/egui)

Два полноценных графических окна — преподаватель и ученик.
Браузер не нужен, в пакет включены сервер и агент.

APPIMAGE:
 chmod +x OkoTeacher-0.2.0-x86_64.AppImage OkoStudent-0.2.0-x86_64.AppImage
 ./OkoTeacher-0.2.0-x86_64.AppImage
 ./OkoStudent-0.2.0-x86_64.AppImage
Если нет FUSE: добавить --appimage-extract-and-run.

TAR.GZ (установка для текущего пользователя без sudo и systemd):
 tar -xf OkoNative-0.2.0-linux-x86_64.tar.gz
 bash OkoDesktop/scripts/install-user.sh both

Преподаватель: открывает окно и запускает мастер внутри приложения.
При первом запуске создаются ДВА разных ключа ~/.config/oko/native-teacher.json.
По умолчанию сервер прослушивает только 127.0.0.1:7600.
Для класса включите опцию LAN в окне преподавателя.
Ученикам сообщите ws://IP_ПРЕПОДАВАТЕЛЯ:7600 и ключ ученика из боковой панели.
Административный ключ не передавайте ученикам.
Ученик: в отдельном окне вводит адрес, ключ и имя, затем подключается.
Автозапуск — по выбору пользователя через XDG.

ОГРАНИЧЕНИЯ И БЕЗОПАСНОСТЬ:
 - Нет TLS, трафик ws://. Используйте только доверенную сеть или VPN.
 - X11: реальный захват. Блокировка пока визуальная, не выключает ввод.
 - Wayland/wlroots требует grim. GNOME/KDE ScreenCast Portal не реализован.
 - На X11/Wayland сообщения используют notify-send, если доступен; иначе — в журнал.
 - GUI требует x86_64 Linux, glibc 2.35+ и OpenGL/Mesa.
 - Тестирование реального Calculate Linux ещё не проводилось.
'''
    put(portable/'README.txt',readme)
    with tarfile.open(out/f'OkoNative-{version}-linux-x86_64.tar.gz','w:gz') as t:
        t.add(portable,arcname='OkoDesktop')

    deb=tmp/'deb'
    (deb/'DEBIAN').mkdir(parents=True)
    (deb/'opt/oko/bin').mkdir(parents=True)
    (deb/'usr/share/applications').mkdir(parents=True)
    for src in (portable/'bin').iterdir():
        shutil.copy2(src,deb/'opt/oko/bin'/src.name)
    control=f'''Package: oko-native
Version: {version}
Section: education
Priority: optional
Architecture: amd64
Maintainer: Oko Native Project
Depends: libc6 (>= 2.35), libgcc-s1, libxkbcommon0, libxkbcommon-x11-0, libgl1
Description: Native Linux desktop classroom monitor
 Teacher and student GUI, bundled server and agent, no browser.
'''
    put(deb/'DEBIAN/control',control)
    for role in ('teacher','student'):
        put(deb/f'usr/share/applications/oko-native-{role}.desktop',f'''[Desktop Entry]
Type=Application
Name=ОКО — {"Преподаватель" if role=="teacher" else "Ученик"}
Exec=/opt/oko/bin/oko-desktop --{role}
Icon=computer
Categories=Education;Network;
Terminal=false
''')
    subprocess.run(['dpkg-deb','--build','--root-owner-group',str(deb),str(out/f'oko-native_{version}_amd64.deb')],check=True,stdout=subprocess.DEVNULL)
    for role in ('Teacher','Student'):
        app=tmp/f'Oko{role}.AppDir'
        (app/'usr/bin').mkdir(parents=True)
        for src in (portable/'bin').iterdir():
            shutil.copy2(src,app/'usr/bin'/src.name)
        # winit loads XKB libraries dynamically; bundle them for Calculate/Gentoo.
        (app/'usr/lib').mkdir(parents=True)
        for soname in ('libxkbcommon-x11.so.0', 'libxkbcommon.so.0', 'libxcb-xkb.so.1'):
            candidates=[Path('/usr/lib/x86_64-linux-gnu')/soname, Path('/lib/x86_64-linux-gnu')/soname]
            source=next((x for x in candidates if x.exists()),None)
            if source is None:
                raise RuntimeError(f'{soname} missing; install libxkbcommon-x11-0 and libxcb-xkb1')
            shutil.copy2(source, app/'usr/lib'/soname,follow_symlinks=True)
        shutil.copy2(portable/'oko.svg',app/'oko.svg')
        (app/'.DirIcon').symlink_to('oko.svg')
        lower=role.lower()
        put(app/f'oko-{lower}.desktop',f'''[Desktop Entry]
Type=Application
Name=ОКО — {role}
Exec=AppRun
Icon=oko
Categories=Education;Network;
Terminal=false
''')
        put(app/'AppRun',f'''#!/bin/sh
BASE="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
export OKO_BIN_DIR="$BASE/usr/bin"
export LD_LIBRARY_PATH="$BASE/usr/lib:${{LD_LIBRARY_PATH:-}}"
exec "$BASE/usr/bin/oko-desktop" --{lower} "$@"
''',0o755)
        file=out/f'Oko{role}-{version}-x86_64.AppImage'
        env=os.environ.copy()
        env['ARCH']='x86_64'
        with (out/f'build-{lower}.log').open('wb') as log:
            subprocess.run([str(appimagetool),'--appimage-extract-and-run',str(app),str(file)],env=env,stdout=log,stderr=subprocess.STDOUT,check=True)
        file.chmod(0o755)
    source=tmp/'OkoNativeSource'
    shutil.copytree(desktop/'src',source/'desktop/src')
    for name in ('Cargo.toml','Cargo.lock','Dockerfile.build'):
        shutil.copy2(desktop/name,source/'desktop'/name)
    shutil.copytree(core/'src',source/'core/src')
    shutil.copytree(core/'ui',source/'core/ui')
    shutil.copytree(core/'assets',source/'core/assets',ignore=shutil.ignore_patterns('*.ttf'))
    for name in ('Cargo.toml','Cargo.lock','README.md'):
        shutil.copy2(core/name,source/'core'/name)
    shutil.copy2(portable/'README.txt',source/'README-START.txt')
    with tarfile.open(out/f'OkoNativeSource-{version}.tar.gz','w:gz') as t:
        t.add(source,arcname='OkoNativeSource')
    shutil.copy2(portable/'README.txt',out/'README-START.txt')

names=[f'OkoTeacher-{version}-x86_64.AppImage',f'OkoStudent-{version}-x86_64.AppImage',f'OkoNative-{version}-linux-x86_64.tar.gz',f'oko-native_{version}_amd64.deb',f'OkoNativeSource-{version}.tar.gz']
with (out/'SHA256SUMS').open('w') as f:
    for name in names:
        digest=hashlib.sha256((out/name).read_bytes()).hexdigest()
        f.write(f'{digest}  {name}\n')
with ZipFile(out/f'OkoNative-{version}-FULL.zip','w',compression=ZIP_DEFLATED,compresslevel=3) as z:
    for name in names+['SHA256SUMS','README-START.txt']:
        z.write(out/name,name)
with ZipFile(out/f'OkoNative-{version}-FULL.zip') as z:
    assert z.testzip() is None
print('RELEASE_BUILT:')
for name in names+[f'OkoNative-{version}-FULL.zip']:
    p=out/name
    print(name,round(p.stat().st_size/1024/1024,2),'MB')
