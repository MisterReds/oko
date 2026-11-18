// ОКО — агент ученического ПК: захват экрана → JPEG → WebSocket к серверу-мастеру,
// приём команд (блокировка экрана, сообщения).
use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use oko::{AgentStateMsg, Capture, Command, Hello};
use std::sync::mpsc as std_mpsc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc as tokio_mpsc;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

struct Args {
    server: String,
    name: String,
    user: String,
    fps: f64,
    quality: u8,
    demo: bool,
    token: Option<String>,
}

fn usage() -> &'static str {
    "oko-agent — агент ученического ПК

  --server URL   адрес мастера (по умолчанию ws://127.0.0.1:7600)
  --name ИМЯ     отображаемое имя ученика (по умолчанию: имя хоста)
  --user ИМЯ     имя пользователя (по умолчанию: текущий пользователь)
  --fps N        частота кадров 1..30 (по умолчанию 5)
  --quality N    качество JPEG 30..95 (по умолчанию 70)
  --demo         принудительно демо-экран вместо реального захвата\n  --token TOKEN  ключ сервера (или переменная OKO_TOKEN)
  -h, --help     эта справка"
}

fn parse_args() -> Result<Args> {
    let user_default = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "pupil".to_string());
    let mut a = Args {
        server: "ws://127.0.0.1:7600".into(),
        name: oko::hostname(),
        user: user_default,
        fps: 5.0,
        quality: 70,
        demo: false,
        token: std::env::var("OKO_TOKEN").ok().filter(|s| !s.is_empty()),
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--server" => a.server = it.next().context("--server: нужен URL")?,
            "--name" => a.name = it.next().context("--name: нужно имя")?,
            "--user" => a.user = it.next().context("--user: нужно имя")?,
            "--fps" => a.fps = it.next().context("--fps: нужно число")?.parse()?,
            "--quality" => a.quality = it.next().context("--quality: нужно число")?.parse()?,
            "--demo" => a.demo = true,
            "--token" => a.token = Some(it.next().context("--token: нужно значение")?),
            "--help" | "-h" => {
                println!("{}", usage());
                std::process::exit(0);
            }
            other => anyhow::bail!("неизвестный аргумент: {other}\n{}", usage()),
        }
    }
    if !(1.0..=30.0).contains(&a.fps) {
        anyhow::bail!("--fps должен быть в диапазоне 1..30");
    }
    if !(30..=95).contains(&a.quality) {
        anyhow::bail!("--quality должен быть в диапазоне 30..95");
    }
    Ok(a)
}

fn make_backend(name: &str, quality: u8, demo: bool) -> Result<Box<dyn Capture>> {
    if demo {
        eprintln!("[oko-agent] демонстрационный режим включён явно");
        return Ok(Box::new(oko::demo::DemoCapture::new(name, quality)?));
    }
    if std::env::var("XDG_SESSION_TYPE").ok().as_deref() == Some("wayland") {
        match oko::grim::GrimCapture::new() {
            Ok(b) => {
                eprintln!("[oko-agent] Wayland / grim, кадры экрана разрешены");
                return Ok(Box::new(b));
            }
            Err(e) => anyhow::bail!("Wayland: захват недоступен ({e:#}). Для wlroots установите grim; для GNOME/KDE требуется будущая реализация ScreenCast Portal. Демо доступно только с --demo"),
        }
    }
    match oko::x11::X11Capture::new(quality) {
        Ok(b) => Ok(Box::new(b)),
        Err(e) => anyhow::bail!("X11-захват недоступен ({e:#}). Проверьте DISPLAY, XAUTHORITY и вход в графический сеанс; для тестов укажите --demo"),
    }
}

fn spawn_capture(
    mut backend: Box<dyn Capture>,
    fps: f64,
    cmd_rx: std_mpsc::Receiver<Command>,
    jpeg_tx: tokio_mpsc::Sender<Vec<u8>>,
) {
    std::thread::spawn(move || {
        let start = Instant::now();
        let frame_dur = Duration::from_secs_f64(1.0 / fps);
        loop {
            while let Ok(cmd) = cmd_rx.try_recv() {
                backend.handle(&cmd);
            }
            let t = start.elapsed().as_secs_f64();
            match backend.grab(t) {
                Ok(jpeg) => {
                    if jpeg_tx.blocking_send(jpeg).is_err() {
                        return; // канал закрыт — соединение умерло
                    }
                }
                Err(e) => eprintln!("[oko-agent] захват: {e:#}"),
            }
            std::thread::sleep(frame_dur);
        }
    });
}

async fn connect_and_run(args: &Args) -> Result<()> {
    // адрес агента: если путь не указан, добавляем /ws/agent
    let url = if args.server.contains("/ws/agent") {
        args.server.clone()
    } else {
        format!("{}/ws/agent", args.server.trim_end_matches('/'))
    };
    let (jpeg_tx, mut jpeg_rx) = tokio_mpsc::channel::<Vec<u8>>(8);
    let (cmd_tx, cmd_rx) = std_mpsc::channel::<Command>();
    let backend = make_backend(&args.name, args.quality, args.demo)?;
    let screen = backend.screen();
    let backend_name = backend.backend_name().to_string();
    spawn_capture(backend, args.fps, cmd_rx, jpeg_tx);

    let url = match &args.token {
        Some(t) => format!("{}{}token={}", url, if url.contains('?') {"&"} else {"?"}, t),
        None => url,
    };
    let (ws, _) = connect_async(&url)
        .await
        .with_context(|| format!("подключение к {}", args.server))?;
    let (mut sink, mut stream) = ws.split();
    eprintln!("[oko-agent] подключено к {}", args.server);

    let hello = Hello {
        kind: "hello".into(),
        name: args.name.clone(),
        hostname: oko::hostname(),
        user: args.user.clone(),
        backend: backend_name.clone(),
        screen,
        locked: false,
    };
    sink.send(Message::Text(serde_json::to_string(&hello)?.into())).await?;

    loop {
        tokio::select! {
            jpeg = jpeg_rx.recv() => {
                match jpeg {
                    Some(j) => sink.send(Message::Binary(j.into())).await?,
                    None => break,
                }
            }
            msg = stream.next() => {
                match msg {
                    Some(Ok(Message::Text(t))) => {
                        let cmd: Command = serde_json::from_str(&t).context("некорректная команда")?;
                        let is_lock = matches!(cmd.action.as_str(), "lock" | "unlock")
                            && (backend_name == "x11" || args.demo);
                        let _ = cmd_tx.send(cmd.clone());
                        if is_lock {
                            let st = AgentStateMsg {
                                kind: "state".into(),
                                locked: cmd.action == "lock",
                            };
                            sink.send(Message::Text(serde_json::to_string(&st)?.into())).await?;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => {}
                    Some(Err(e)) => return Err(e.into()),
                }
            }
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = parse_args()?;
    eprintln!("[oko-agent] имя «{}» · сервер {}", args.name, args.server);
    loop {
        match connect_and_run(&args).await {
            Ok(()) => eprintln!("[oko-agent] соединение закрыто"),
            Err(e) => eprintln!("[oko-agent] ошибка: {e:#}"),
        }
        eprintln!("[oko-agent] переподключение через 3 с…");
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}
