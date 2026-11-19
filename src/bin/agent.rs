// ОКО — агент ученического ПК: захват экрана → JPEG → WebSocket к серверу-мастеру,
// приём команд (блокировка экрана, сообщения).
use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use oko::{AgentStateMsg, Capture, Command, Hello};
use rustls::{
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    pki_types::{CertificateDer, ServerName, UnixTime},
    DigitallySignedStruct, SignatureScheme,
};
use sha2::{Digest, Sha256};
use std::sync::mpsc as std_mpsc;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};
use tokio::sync::mpsc as tokio_mpsc;
use tokio::sync::mpsc::error::TrySendError;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, connect_async_tls_with_config, Connector};

struct Args {
    server: String,
    name: String,
    user: String,
    fps: f64,
    quality: u8,
    demo: bool,
    token: Option<String>,
    fingerprint: Option<String>,
    allow_insecure: bool,
}

fn usage() -> &'static str {
    "oko-agent — агент ученического ПК

  --server URL   адрес мастера (по умолчанию ws://127.0.0.1:7600)
  --name ИМЯ     отображаемое имя ученика (по умолчанию: имя хоста)
  --user ИМЯ     имя пользователя (по умолчанию: текущий пользователь)
  --fps N        частота кадров 1..30 (по умолчанию 5)
  --quality N    качество JPEG 30..95 (по умолчанию 70)
  --demo         принудительно демо-экран вместо реального захвата\n  --token TOKEN  ключ сервера (или переменная OKO_TOKEN)
  --fingerprint HEX  SHA-256 сертификата WSS (или OKO_CERT_SHA256)
  --allow-insecure   явно разрешить ws:// вне localhost (для старых стендов)
  -h, --help     эта справка"
}

fn is_local_plaintext_url(url: &str) -> bool {
    for prefix in ["ws://127.0.0.1", "ws://localhost", "ws://[::1]"] {
        if let Some(suffix) = url.strip_prefix(prefix) {
            return suffix.is_empty()
                || suffix.starts_with(':')
                || suffix.starts_with('/')
                || suffix.starts_with('?');
        }
    }
    false
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
        fingerprint: std::env::var("OKO_CERT_SHA256")
            .ok()
            .filter(|s| !s.is_empty()),
        allow_insecure: false,
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
            "--fingerprint" => {
                a.fingerprint = Some(it.next().context("--fingerprint: нужен SHA256")?)
            }
            "--allow-insecure" => a.allow_insecure = true,
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
    let secure = a.server.starts_with("wss://");
    let localhost = is_local_plaintext_url(&a.server);
    if !secure && !localhost && !a.allow_insecure {
        anyhow::bail!("remote screen streaming needs wss:// with --fingerprint; use --allow-insecure only inside isolated test networks");
    }
    if secure {
        let hash = a
            .fingerprint
            .as_ref()
            .context("WSS needs --fingerprint HEX or OKO_CERT_SHA256")?;
        parse_fingerprint(hash)?;
    }
    Ok(a)
}

#[derive(Debug)]
struct FingerprintVerifier {
    expected: [u8; 32],
}

fn parse_fingerprint(raw: &str) -> Result<[u8; 32]> {
    let hex = raw.trim().replace(':', "").to_lowercase();
    anyhow::ensure!(
        hex.len() == 64,
        "SHA256 fingerprint must contain exactly 64 hex digits"
    );
    let mut out = [0u8; 32];
    for (i, item) in out.iter_mut().enumerate() {
        *item = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)
            .context("invalid certificate fingerprint")?;
    }
    Ok(out)
}

impl ServerCertVerifier for FingerprintVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        // Explicit certificate pinning replaces domain-name verification for
        // arbitrary LAN IP addresses. TLS handshake signatures are still verified.
        let hash = Sha256::digest(end_entity.as_ref());
        if hash.as_slice() == self.expected {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General(
                "certificate fingerprint mismatch".into(),
            ))
        }
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

async fn connect_server(
    url: &str,
    fingerprint: Option<&str>,
) -> Result<(
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    tokio_tungstenite::tungstenite::handshake::client::Response,
)> {
    if url.starts_with("wss://") {
        let expected =
            parse_fingerprint(fingerprint.context("missing WSS certificate fingerprint")?)?;
        let provider = rustls::crypto::ring::default_provider();
        let config = rustls::ClientConfig::builder_with_provider(std::sync::Arc::new(provider))
            .with_safe_default_protocol_versions()?
            .dangerous()
            .with_custom_certificate_verifier(std::sync::Arc::new(FingerprintVerifier { expected }))
            .with_no_client_auth();
        Ok(connect_async_tls_with_config(
            url,
            None,
            false,
            Some(Connector::Rustls(std::sync::Arc::new(config))),
        )
        .await?)
    } else {
        Ok(connect_async(url).await?)
    }
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
    state_tx: tokio_mpsc::UnboundedSender<AgentStateMsg>,
    active: Arc<AtomicBool>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let start = Instant::now();
        let frame_dur = Duration::from_secs_f64(1.0 / fps);
        let mut lock_expiry: Option<Instant> = None;
        while active.load(Ordering::Relaxed) {
            let tick = Instant::now();
            while let Ok(cmd) = cmd_rx.try_recv() {
                match backend.handle(&cmd) {
                    Ok(Some(locked)) => {
                        lock_expiry = if locked {
                            Some(Instant::now() + Duration::from_secs(120))
                        } else {
                            None
                        };
                        let _ = state_tx.send(AgentStateMsg {
                            kind: "state".into(),
                            locked,
                        });
                    }
                    Ok(None) => {}
                    Err(e) => eprintln!("[oko-agent] command '{}' failed: {e:#}", cmd.action),
                }
            }
            if !active.load(Ordering::Relaxed) {
                break;
            }
            if lock_expiry.is_some_and(|deadline| Instant::now() >= deadline) {
                let unlock = Command {
                    action: "unlock".into(),
                    text: None,
                };
                match backend.handle(&unlock) {
                    Ok(Some(false)) => {
                        lock_expiry = None;
                        let _ = state_tx.send(AgentStateMsg {
                            kind: "state".into(),
                            locked: false,
                        });
                        eprintln!("[oko-agent] lock auto-released after 120 seconds");
                    }
                    Ok(_) => {
                        lock_expiry = None;
                    }
                    Err(e) => eprintln!("[oko-agent] unlock failsafe error: {e:#}"),
                }
            }
            match backend.grab(start.elapsed().as_secs_f64()) {
                Ok(jpeg) => match jpeg_tx.try_send(jpeg) {
                    Ok(()) | Err(TrySendError::Full(_)) => {}
                    Err(TrySendError::Closed(_)) => break,
                },
                Err(e) => eprintln!("[oko-agent] capture: {e:#}"),
            }
            let remaining = frame_dur.saturating_sub(tick.elapsed());
            if !remaining.is_zero() {
                std::thread::sleep(remaining);
            }
        }
        // backend is dropped here: X11 releases overlay and keyboard/mouse grabs.
    })
}

async fn connect_and_run(args: &Args) -> Result<()> {
    let url = if args.server.contains("/ws/agent") {
        args.server.clone()
    } else {
        format!("{}/ws/agent", args.server.trim_end_matches('/'))
    };

    // Connect before creating capture threads, so failed handshakes never
    // accumulate orphan screen-grabbers.
    let url = match &args.token {
        Some(t) => format!(
            "{}{}token={}",
            url,
            if url.contains('?') { "&" } else { "?" },
            t
        ),
        None => url,
    };
    let (ws, _) = connect_server(&url, args.fingerprint.as_deref())
        .await
        .with_context(|| format!("connection to {}", args.server))?;
    let (mut sink, mut stream) = ws.split();
    eprintln!("[oko-agent] connected to {}", args.server);

    let backend = make_backend(&args.name, args.quality, args.demo)?;
    let screen = backend.screen();
    let backend_name = backend.backend_name().to_string();
    let hello = Hello {
        kind: "hello".into(),
        name: args.name.clone(),
        hostname: oko::hostname(),
        user: args.user.clone(),
        backend: backend_name,
        screen,
        locked: false,
    };
    sink.send(Message::Text(serde_json::to_string(&hello)?.into()))
        .await?;

    let (jpeg_tx, mut jpeg_rx) = tokio_mpsc::channel::<Vec<u8>>(2);
    let (cmd_tx, cmd_rx) = std_mpsc::channel::<Command>();
    let (state_tx, mut state_rx) = tokio_mpsc::unbounded_channel::<AgentStateMsg>();
    let active = Arc::new(AtomicBool::new(true));
    let worker = spawn_capture(backend, args.fps, cmd_rx, jpeg_tx, state_tx, active.clone());

    let result: Result<()> = async {
        loop {
            tokio::select! {
                jpeg = jpeg_rx.recv() => {
                    match jpeg {
                        Some(j) => sink.send(Message::Binary(j.into())).await?,
                        None => break,
                    }
                }
                state = state_rx.recv() => {
                    match state {
                        Some(s) => sink.send(Message::Text(serde_json::to_string(&s)?.into())).await?,
                        None => break,
                    }
                }
                msg = stream.next() => {
                    match msg {
                        Some(Ok(Message::Text(t))) => {
                            let cmd: Command = serde_json::from_str(&t).context("invalid command")?;
                            if cmd_tx.send(cmd).is_err() {break;}
                        }
                        Some(Ok(Message::Close(_))) | None => break,
                        Some(Ok(_)) => {}
                        Some(Err(e)) => return Err(e.into()),
                    }
                }
            }
        }
        Ok(())
    }.await;

    // Even on network errors release screen locks before reconnecting.
    active.store(false, Ordering::Relaxed);
    drop(jpeg_rx);
    drop(state_rx);
    drop(cmd_tx);
    if worker.join().is_err() {
        eprintln!("[oko-agent] capture worker panicked");
    }
    result
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

#[cfg(test)]
mod security_tests {
    use super::is_local_plaintext_url;
    #[test]
    fn only_exact_loopback_allows_plaintext() {
        assert!(is_local_plaintext_url("ws://127.0.0.1:7600"));
        assert!(is_local_plaintext_url("ws://localhost/ws/agent"));
        assert!(is_local_plaintext_url("ws://[::1]:7600"));
        assert!(!is_local_plaintext_url(
            "ws://127.0.0.1.attacker.invalid:7600"
        ));
        assert!(!is_local_plaintext_url("ws://localhost.evil"));
        assert!(!is_local_plaintext_url("ws://127.0.0.1@evil.invalid"));
        assert!(!is_local_plaintext_url("ws://192.168.0.10:7600"));
    }
}
