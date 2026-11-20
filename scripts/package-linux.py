from pathlib import Path
from zipfile import ZipFile, ZIP_DEFLATED
import tempfile, shutil, subprocess, os, tarfile, hashlib, re

root=Path(__file__).resolve().parent.parent
core=root
desktop=root/'desktop'
out=root/'dist'
out.mkdir(parents=True,exist_ok=True)
def cargo_version(manifest):
    match = re.search(r'(?m)^version\s*=\s*"([^"]+)"', manifest.read_text())
    if not match: raise ValueError(f"No package version in {manifest}")
    return match.group(1)
version = cargo_version(core/'Cargo.toml')
if version != cargo_version(desktop/'Cargo.toml'):
    raise ValueError("Server and desktop version mismatch")
if os.environ.get("GITHUB_REF_TYPE") == "tag" and os.environ.get("GITHUB_REF_NAME") != "v"+version:
    raise ValueError("Release Git tag does not match Cargo version")
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
    shutil.copy2(root/'scripts/doctor.sh', portable/'scripts/doctor.sh')
    readme='''ОКО Native 0.3.0 — Linux x86-64 (Rust/egui)

Два полноценных графических окна — преподаватель и ученик.
Браузер не нужен, в пакет включены сервер и агент.

APPIMAGE:
 chmod +x OkoTeacher-0.3.0-x86_64.AppImage OkoStudent-0.3.0-x86_64.AppImage
 ./OkoTeacher-0.3.0-x86_64.AppImage
 ./OkoStudent-0.3.0-x86_64.AppImage
Если нет FUSE: добавить --appimage-extract-and-run.

TAR.GZ (установка для текущего пользователя без sudo и systemd):
 tar -xf OkoNative-0.3.0-linux-x86_64.tar.gz
 bash OkoDesktop/scripts/install-user.sh both

Преподаватель: открывает окно и запускает мастер внутри приложения.
При первом запуске создаются ДВА разных ключа ~/.config/oko/native-teacher.json.
По умолчанию сервер прослушивает только 127.0.0.1:7600.
Для класса включите опцию LAN в окне преподавателя.
Для класса включите TLS-LAN: локальное управление работает на 127.0.0.1:7600,
для учеников открывается защищённый WSS: 7601.
Ученику необходимы три значения из окна учителя:
  1) wss://IP_ПРЕПОДАВАТЕЛЯ:7601
  2) отдельный ключ ученика (НЕ ключ администратора)
  3) SHA-256 отпечаток сертификата (64 символа).
Сверяйте отпечаток с преподавателем непосредственно, не доверяйте неизвестным адресам.
Ученик: в отдельном окне вводит адрес, ключ и имя, затем подключается.
Автозапуск — по выбору пользователя через XDG.

ОГРАНИЧЕНИЯ И БЕЗОПАСНОСТЬ:
 - В версии 0.3 для класса включён TLS с проверкой отпечатка сертификата.
   Прежний незащищённый ws:// допускается только для localhost или при явно
   заданном --allow-insecure в CLI агента. Не раскрывайте ключи.
 - X11: реальный захват и перехват клавиатуры/мыши при блокировке.
   При закрытии агента ввод освобождается, а через 120 секунд автоматически снимается блокировка.
 - Wayland/wlroots требует grim. GNOME/KDE ScreenCast Portal не реализован.
 - На X11/Wayland сообщения используют notify-send, если доступен; иначе — в журнал.
 - GUI требует x86_64 Linux, glibc 2.35+ и OpenGL/Mesa.
 - В окне ученика отображаются достоверные состояния: соединение, передача кадров, переподключение и ошибка захвата.
 - Тестирование реального Calculate Linux ещё не проводилось.
'''
    readme=readme.replace('0.3.0',version)
    readme += """
ОБНОВЛЕНИЕ/ОТКАТ:
 Установка из GitHub: bash scripts/install.sh both
 Откат: bash scripts/rollback.sh both
 Удаление без удаления ключей: bash scripts/uninstall.sh both
 Настройки и TLS ключи хранятся отдельно от программы.
"""
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
    # Ship a genuinely buildable repository-shaped source tree, rather than
    # separate core/desktop folders missing build scripts and tests.
    source=tmp/'OkoNativeSource'
    for rel in ('src','ui','assets','desktop/src','scripts','tests','docs','.github'):
        shutil.copytree(root/rel, source/rel,
            ignore=shutil.ignore_patterns('*.ttf','__pycache__','*.pyc','*.log','.env'))
    for rel in ('Cargo.toml','Cargo.lock','README.md','SECURITY.md','LICENSE',
                'desktop/Cargo.toml','desktop/Cargo.lock','desktop/Dockerfile.build'):
        file=root/rel
        if file.is_file():
            (source/rel).parent.mkdir(parents=True,exist_ok=True)
            shutil.copy2(file,source/rel)
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
