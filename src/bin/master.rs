// ОКО — сервер-мастер: принимает агентов по WebSocket, хранит последние кадры,
// рассылает их в веб-консоли учителя, передаёт команды агентам.
use anyhow::{Context, Result};
use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use serde::Deserialize;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use oko::{pack_frame, AgentInfo, AgentStateMsg, Command, CommandReq, Hello, Snapshot, WatchMsg};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, mpsc};

const CONSOLE_HTML: &str = include_str!("../../ui/console.html");

#[derive(Clone)]
struct App(Arc<AppInner>);

struct FrameSlot {
    bytes: Option<Vec<u8>>,
    at: Option<Instant>,
    count: u64,
    win_start: Instant,
    fps: f32,
}

#[derive(Clone)]
struct AgentHandle {
    info: Arc<Mutex<AgentInfo>>,
    frame: Arc<Mutex<FrameSlot>>,
    cmd_tx: mpsc::Sender<Command>,
    connected: Instant,
    seen: Arc<Mutex<Instant>>,
    gen: u64,
}

static GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

struct ConsoleConn {
    tx: mpsc::Sender<Out>,
    watched: Mutex<HashSet<String>>,
}

enum Out {
    State(Arc<String>),
    Frame(String, Vec<u8>),
}

struct AppInner {
    agents: RwLock<HashMap<String, AgentHandle>>,
    consoles: RwLock<Vec<Arc<ConsoleConn>>>,
    state_tx: broadcast::Sender<()>,
    token: Option<String>,
    agent_token: Option<String>,
}

struct MasterArgs {
    port: u16,
    bind: String,
    token: Option<String>,
    agent_token: Option<String>,
    parent_pid: Option<u32>,
}

#[derive(Debug, Default, Deserialize)]
struct TokenQuery { token: Option<String> }

fn is_auth(app: &App, supplied: Option<&str>) -> bool {
    app.0.token.as_deref().map_or(true, |t| supplied == Some(t))
}
fn is_agent_auth(app: &App, supplied: Option<&str>) -> bool {
    match &app.0.agent_token {
        Some(key) => supplied == Some(key),
        None => is_auth(app, supplied),
    }
}
fn header_token(headers: &HeaderMap) -> Option<&str> {
    headers.get("authorization").and_then(|h| h.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
}
fn usage() -> &'static str {
    "oko-master — сервер и веб-консоль
  --port N       HTTP/WS порт (7600)
  --bind IP      адрес прослушивания (127.0.0.1)
  --token TOKEN  административный ключ (OKO_TOKEN)
  --agent-token TOKEN  отдельный ключ учеников (OKO_AGENT_TOKEN)
  --parent-pid PID  завершать сервер после смерти родительского приложения
  --help         помощь"
}
fn parse_args() -> Result<MasterArgs> {
    let mut a = MasterArgs {
        port: 7600,
        bind: "127.0.0.1".into(),
        token: std::env::var("OKO_TOKEN").ok().filter(|x| !x.is_empty()),
        agent_token: std::env::var("OKO_AGENT_TOKEN").ok().filter(|x| !x.is_empty()),
        parent_pid: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--port" => a.port = it.next().context("--port: нужно число")?.parse()?,
            "--bind" => a.bind = it.next().context("--bind: нужен IP")?,
            "--token" => a.token = Some(it.next().context("--token: нужно значение")?),
            "--agent-token" => a.agent_token = Some(it.next().context("--agent-token: нужно значение")?),
            "--parent-pid" => a.parent_pid = Some(it.next().context("--parent-pid: нужен PID")?.parse()?),
            "--help" | "-h" => { println!("{}", usage()); std::process::exit(0); }
            other => anyhow::bail!("неизвестный аргумент: {other}\n{}", usage()),
        }
    }
    if a.bind != "127.0.0.1" && a.bind != "::1" && a.token.is_none() {
        anyhow::bail!("для доступа по сети (--bind {}) требуется --token или OKO_TOKEN", a.bind);
    }
    if a.bind != "127.0.0.1" && a.bind != "::1" && a.agent_token.is_none() {
        anyhow::bail!("при доступе из LAN нужен отдельный --agent-token или OKO_AGENT_TOKEN");
    }
    if let Some(t) = &a.agent_token {
        if t.len() < 16 { anyhow::bail!("ключ учеников должен содержать минимум 16 символов"); }
        if a.token.as_ref() == Some(t) { anyhow::bail!("административный ключ и ключ учеников должны различаться"); }
    }
    if let Some(t) = &a.token {
        if t.len() < 16 {
            anyhow::bail!("ключ доступа должен содержать минимум 16 символов");
        }
    }
    Ok(a)
}

fn snapshot(app: &App) -> Snapshot {
    let now = Instant::now();
    let agents = app.0.agents.read().unwrap();
    let mut list: Vec<AgentInfo> = agents
        .values()
        .map(|h| {
            let mut info = h.info.lock().unwrap().clone();
            let f = h.frame.lock().unwrap();
            info.last_frame_ms = f
                .at
                .map(|a| now.duration_since(a).as_millis() as u64)
                .unwrap_or(u64::MAX);
            info.fps = f.fps;
            info.connected_ms = now.duration_since(h.connected).as_millis() as u64;
            info
        })
        .collect();
    list.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
    Snapshot { agents: list }
}

async fn console_handler() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], CONSOLE_HTML)
}

async fn api_state(State(app): State<App>, headers: HeaderMap) -> Response {
    if !is_auth(&app, header_token(&headers)) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    Json(snapshot(&app)).into_response()
}

async fn api_command(State(app): State<App>, headers: HeaderMap, Json(req): Json<CommandReq>) -> Response {
    if !is_auth(&app, header_token(&headers)) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if !matches!(req.action.as_str(), "lock" | "unlock" | "message") {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "действие должно быть lock | unlock | message"})),
        )
            .into_response();
    }
    if req.action == "message"
        && req.text.as_deref().map(|s| s.trim().is_empty()).unwrap_or(true)
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "текст сообщения пуст"})),
        )
            .into_response();
    }
    let agents = app.0.agents.read().unwrap();
    match agents.get(&req.agent) {
        Some(h) => {
            let cmd = Command {
                action: req.action.clone(),
                text: req.text.clone(),
            };
            match h.cmd_tx.try_send(cmd) {
                Ok(()) => (StatusCode::OK, Json(json!({"queued": true}))).into_response(),
                Err(_) => (
                    StatusCode::TOO_MANY_REQUESTS,
                    Json(json!({"error": "агент не принимает команды"})),
                )
                    .into_response(),
            }
        }
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("агент «{}» не подключен", req.agent)})),
        )
            .into_response(),
    }
}

async fn ws_agent(State(app): State<App>, Query(query): Query<TokenQuery>, upgrade: WebSocketUpgrade) -> Response {
    if !is_agent_auth(&app, query.token.as_deref()) { return StatusCode::UNAUTHORIZED.into_response(); }
    upgrade.on_upgrade(move |sock| agent_socket(app, sock))
}

async fn agent_socket(app: App, mut ws: WebSocket) {
    // первое сообщение — приветствие
    let first = tokio::time::timeout(Duration::from_secs(5), ws.recv()).await;
    let hello: Hello = match first {
        Ok(Some(Ok(WsMessage::Text(t)))) => match serde_json::from_str(&t) {
            Ok(h) => h,
            Err(e) => {
                eprintln!("[oko-master] плохое приветствие: {e}");
                return;
            }
        },
        _ => return,
    };
    if hello.kind != "hello" {
        return;
    }
    // id: безопасное имя хоста; при коллизии (тот же хост ещё «жив» в карте) — суффикс -2, -3…
    let base = oko::sanitize_id(&hello.hostname);
    let my_gen = GEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut id = base.clone();
    {
        let mut agents = app.0.agents.write().unwrap();
        let mut n = 1u32;
        while let Some(existing) = agents.get(&id) {
            let fresh = existing.seen.lock().unwrap().elapsed() < Duration::from_secs(15);
            if !fresh {
                agents.remove(&id);
                break;
            }
            n += 1;
            id = format!("{base}-{n}");
        }
    }
    let seen = Arc::new(Mutex::new(Instant::now()));
    let info = Arc::new(Mutex::new(AgentInfo {
        id: id.clone(),
        name: hello.name,
        hostname: hello.hostname,
        user: hello.user,
        backend: hello.backend,
        screen: hello.screen,
        locked: hello.locked,
        last_frame_ms: u64::MAX,
        fps: 0.0,
        connected_ms: 0,
    }));
    let (cmd_tx, mut cmd_rx) = mpsc::channel::<Command>(32);
    let handle = AgentHandle {
        info: info.clone(),
        frame: Arc::new(Mutex::new(FrameSlot {
            bytes: None,
            at: None,
            count: 0,
            win_start: Instant::now(),
            fps: 0.0,
        })),
        cmd_tx,
        connected: Instant::now(),
        seen: seen.clone(),
        gen: my_gen,
    };
    app.0.agents.write().unwrap().insert(id.clone(), handle.clone());
    let _ = app.0.state_tx.send(());
    eprintln!("[oko-master] агент подключен: {id}");

    loop {
        tokio::select! {
            msg = ws.recv() => {
                match msg {
                    Some(Ok(WsMessage::Binary(b))) => {
                        *handle.seen.lock().unwrap() = Instant::now();
                        {
                            let mut f = handle.frame.lock().unwrap();
                            f.bytes = Some(b.to_vec());
                            f.at = Some(Instant::now());
                            f.count += 1;
                            let el = f.win_start.elapsed();
                            if el >= Duration::from_secs(5) {
                                f.fps = f.count as f32 / el.as_secs_f32();
                                f.count = 0;
                                f.win_start = Instant::now();
                            }
                        }
                        let consoles = app.0.consoles.read().unwrap().clone();
                        for c in consoles {
                            if c.watched.lock().unwrap().contains(&id) {
                                let _ = c.tx.try_send(Out::Frame(id.clone(), b.to_vec()));
                            }
                        }
                    }
                    Some(Ok(WsMessage::Text(t))) => {
                        *handle.seen.lock().unwrap() = Instant::now();
                        if let Ok(st) = serde_json::from_str::<AgentStateMsg>(&t) {
                            if st.kind == "state" {
                                info.lock().unwrap().locked = st.locked;
                                let _ = app.0.state_tx.send(());
                            }
                        }
                    }
                    Some(Ok(_)) => {}
                    Some(Err(_)) | None => break,
                }
            }
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(c) => {
                        let text = serde_json::to_string(&c).unwrap_or_default();
                        if ws.send(WsMessage::Text(text.into())).await.is_err() {
                            break;
                        }
                    }
                    None => break,
                }
            }
        }
    }
    // удаляем только свою запись (новое подключение могло занять id уже с другим gen)
    {
        let mut agents = app.0.agents.write().unwrap();
        if let Some(h) = agents.get(&id) {
            if h.gen == my_gen {
                agents.remove(&id);
            }
        }
    }
    let _ = app.0.state_tx.send(());
    eprintln!("[oko-master] агент отключен: {id}");
}

async fn ws_console(State(app): State<App>, Query(query): Query<TokenQuery>, upgrade: WebSocketUpgrade) -> Response {
    if !is_auth(&app, query.token.as_deref()) { return StatusCode::UNAUTHORIZED.into_response(); }
    upgrade.on_upgrade(move |sock| console_socket(app, sock))
}

async fn console_socket(app: App, mut ws: WebSocket) {
    let (tx, mut rx) = mpsc::channel::<Out>(256);
    let conn = Arc::new(ConsoleConn {
        tx,
        watched: Mutex::new(HashSet::new()),
    });
    app.0.consoles.write().unwrap().push(conn.clone());
    let _ = app.0.state_tx.send(());

    loop {
        tokio::select! {
            out = rx.recv() => {
                match out {
                    Some(Out::State(s)) => {
                        if ws.send(WsMessage::Text(s.as_str().to_string().into())).await.is_err() {
                            break;
                        }
                    }
                    Some(Out::Frame(id, jpeg)) => {
                        if ws.send(WsMessage::Binary(pack_frame(&id, &jpeg).into())).await.is_err() {
                            break;
                        }
                    }
                    None => break,
                }
            }
            msg = ws.recv() => {
                match msg {
                    Some(Ok(WsMessage::Text(t))) => {
                        if let Ok(w) = serde_json::from_str::<WatchMsg>(&t) {
                            *conn.watched.lock().unwrap() = w.ids.into_iter().collect();
                        }
                    }
                    Some(Ok(WsMessage::Close(_))) | None => break,
                    _ => {}
                }
            }
        }
    }
    app.0.consoles.write().unwrap().retain(|c| !Arc::ptr_eq(c, &conn));
    let _ = app.0.state_tx.send(());
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = parse_args()?;
    if let Some(pid) = args.parent_pid {
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(2)).await;
                if !std::path::PathBuf::from(format!("/proc/{pid}/cmdline")).exists() {
                    eprintln!("[oko-master] родительское приложение завершилось, сервер остановлен");
                    std::process::exit(0);
                }
            }
        });
    }
    let (state_tx, _) = broadcast::channel::<()>(64);
    let app = App(Arc::new(AppInner {
        agents: RwLock::new(HashMap::new()),
        consoles: RwLock::new(Vec::new()),
        state_tx,
        token: args.token,
        agent_token: args.agent_token,
    }));

    // рассылка снимка состояния консолям при изменениях + keepalive
    let pusher = app.clone();
    tokio::spawn(async move {
        let mut rx = pusher.0.state_tx.subscribe();
        let mut tick = tokio::time::interval(Duration::from_secs(10));
        loop {
            tokio::select! {
                _ = rx.recv() => {}
                _ = tick.tick() => {}
            }
            let snap = Arc::new(serde_json::to_string(&snapshot(&pusher)).unwrap_or_default());
            let consoles = pusher.0.consoles.read().unwrap().clone();
            for c in consoles {
                let _ = c.tx.try_send(Out::State(snap.clone()));
            }
        }
    });

    let router = Router::new()
        .route("/", get(console_handler))
        .route("/healthz", get(|| async { "ok" }))
        .route("/api/state", get(api_state))
        .route("/api/command", post(api_command))
        .route("/ws/agent", get(ws_agent))
        .route("/ws/console", get(ws_console))
        .with_state(app);
    let listener = tokio::net::TcpListener::bind((args.bind.as_str(), args.port)).await?;
    eprintln!("[oko-master] консоль учителя: http://{}:{}", args.bind, args.port);
    axum::serve(listener, router).await?;
    Ok(())
}
