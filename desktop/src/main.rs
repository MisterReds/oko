use eframe::egui::{self, Color32, RichText, TextureHandle};
use serde::{Deserialize, Serialize};
use std::{collections::{HashMap, HashSet}, fs, io::{Read, Write}, net::{TcpStream, ToSocketAddrs}, path::PathBuf, process::{Child, Command, Stdio}, sync::mpsc::{self, Receiver, Sender}, thread, time::{Duration, Instant}};

const BG: Color32 = Color32::from_rgb(12, 16, 23);
const PANEL: Color32 = Color32::from_rgb(22, 28, 39);
const ACCENT: Color32 = Color32::from_rgb(39, 205, 181);
const MUTED: Color32 = Color32::from_rgb(148, 163, 184);
const WARNING: Color32 = Color32::from_rgb(248, 180, 80);

#[derive(Debug, Clone, Deserialize)]
struct AgentInfo {
    id: String,
    name: String,
    hostname: String,
    user: String,
    backend: String,
    screen: [u32; 2],
    locked: bool,
    last_frame_ms: u64,
    fps: f32,
}
#[derive(Debug, Deserialize)]
struct Snapshot { agents: Vec<AgentInfo> }

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TeacherCfg {
    token: String,
    #[serde(default)]
    agent_token: String,
    bind_lan: bool,
    port: u16,
}
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct StudentCfg {
    server: String,
    token: String,
    name: String,
    autostart: bool,
}
enum NetworkEvent {
    Snapshot(Vec<AgentInfo>),
    Frame(String, Vec<u8>),
    Status(String),
}
struct Frame {
    texture: TextureHandle,
    last_update: Instant,
}
#[derive(PartialEq, Eq, Clone, Copy)]
enum Mode { Teacher, Student }

struct OkoApp {
    mode: Mode,
    teacher: TeacherCfg,
    student: StudentCfg,
    master: Option<Child>,
    agents: Vec<AgentInfo>,
    images: HashMap<String, Frame>,
    selection: HashSet<String>,
    expanded: Option<String>,
    filter: String,
    message_text: String,
    message_open: bool,
    show_key: bool,
    status: String,
    last_feedback: String,
    online: bool,
    rx: Receiver<NetworkEvent>,
    tx: Sender<NetworkEvent>,
    last_start: Instant,
    student_running: bool,
    student_pid: Option<u32>,
}

fn config_root() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/tmp".into())).join(".config"))
        .join("oko")
}
fn state_root() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME").map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/tmp".into())).join(".local/state"))
        .join("oko")
}
fn secure_write(file: &str, value: &impl Serialize) -> Result<(), String> {
    let dir = config_root();
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(file);
    let tmp = dir.join(format!(".{file}.tmp-{}", std::process::id()));
    let data = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)] {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(&tmp).map_err(|e| e.to_string())?;
    f.write_all(&data).map_err(|e| e.to_string())?;
    f.sync_all().map_err(|e| e.to_string())?;
    fs::rename(tmp, path).map_err(|e| e.to_string())
}
fn load_cfg<T: serde::de::DeserializeOwned>(file: &str) -> Option<T> {
    serde_json::from_slice(&fs::read(config_root().join(file)).ok()?).ok()
}
fn random_token() -> String {
    let mut data = [0u8; 32];
    if let Ok(mut f) = fs::File::open("/dev/urandom") {
        if f.read_exact(&mut data).is_ok() {
            return data.iter().map(|v| format!("{v:02x}")).collect();
        }
    }
    // Без CSPRNG приложение не должно запускать доступный по сети сервер.
    String::new()
}
fn binary(name: &str) -> PathBuf {
    if let Some(p) = std::env::var_os("OKO_BIN_DIR") {
        let x = PathBuf::from(p).join(name);
        if x.is_file() { return x; }
    }
    let dir = std::env::current_exe().ok().and_then(|p| p.parent().map(|x| x.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));
    let found = dir.join(name);
    if found.is_file() { return found; }
    dir.join("..").join("bin").join(name)
}
fn pid_file() -> PathBuf { state_root().join("student.pid") }
fn pid_running(pid: u32) -> bool {
    #[cfg(target_os="linux")]
    { fs::read(format!("/proc/{pid}/cmdline")).map(|b|b.windows(b"oko-agent".len()).any(|v|v==b"oko-agent")).unwrap_or(false) }
    #[cfg(not(target_os="linux"))]
    { let _ = pid; false }
}
fn student_pid() -> Option<u32> {
    fs::read_to_string(pid_file()).ok()?.trim().parse::<u32>().ok().filter(|x| pid_running(*x))
}
fn spawn_agent(cfg: &StudentCfg) -> Result<u32, String> {
    if cfg.server.trim().is_empty() || cfg.token.len() < 16 {
        return Err("Укажите адрес сервера и ключ длиной от 16 символов".into());
    }
    if !cfg.server.starts_with("ws://") && !cfg.server.starts_with("wss://") {
        return Err("Адрес должен начинаться с ws:// или wss://".into());
    }
    if cfg.server.starts_with("wss://") {
        return Err("В этой версии TLS WebSocket ещё не поддерживается; используйте ws:// в доверенной сети/VPN".into());
    }
    if let Some(pid) = student_pid() { return Ok(pid); }
    let dir = state_root();
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let log = fs::OpenOptions::new().create(true).append(true).open(dir.join("student.log")).map_err(|e| e.to_string())?;
    let mut command = Command::new(binary("oko-agent"));
    command.arg("--server").arg(&cfg.server).env("OKO_TOKEN",&cfg.token)
        .stdout(Stdio::null()).stderr(Stdio::from(log))
        .stdin(Stdio::null());
    if !cfg.name.trim().is_empty() { command.arg("--name").arg(cfg.name.trim()); }
    let child = command.spawn().map_err(|e|format!("Агент не запустился: {e}"))?;
    let pid = child.id();
    fs::write(pid_file(),pid.to_string()).map_err(|e| e.to_string())?;
    Ok(pid)
}
fn stop_agent() -> Result<(), String> {
    if let Some(pid) = student_pid() {
        #[cfg(unix)] {
            let status = Command::new("kill").args(["-TERM",&pid.to_string()]).status()
                .map_err(|e|e.to_string())?;
            if !status.success() { return Err("Агент не удалось остановить".into()); }
        }
    }
    let _ = fs::remove_file(pid_file());
    Ok(())
}
fn set_autostart(enabled: bool) -> Result<(),String> {
    let dir = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config"))
        .join("autostart");
    fs::create_dir_all(&dir).map_err(|e|e.to_string())?;
    let file = dir.join("oko-student.desktop");
    if !enabled { let _ = fs::remove_file(file); return Ok(()); }
    let executable = std::env::var("APPIMAGE").ok().map(PathBuf::from)
        .or_else(||std::env::current_exe().ok()).ok_or("Не удалось определить путь к приложению")?;
    let path = if std::env::var_os("APPIMAGE").is_some() {
        let dest = PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".local/opt/oko/OkoStudent.AppImage");
        fs::create_dir_all(dest.parent().unwrap()).map_err(|e|e.to_string())?;
        fs::copy(&executable,&dest).map_err(|e|e.to_string())?;
        dest
    } else { executable };
    if path.to_string_lossy().contains('\n') { return Err("Некорректный путь запуска".into()); }
    let cmd = if path.to_string_lossy().ends_with(".AppImage") {
        format!("{} --appimage-extract-and-run --background-agent",path.display())
    } else {format!("{} --background-agent",path.display())};
    fs::write(file,format!("[Desktop Entry]\nType=Application\nName=ОКО — агент ученика\nComment=Автозапуск разрешён пользователем\nExec={cmd}\nTerminal=false\nX-GNOME-Autostart-enabled=true\n")).map_err(|e|e.to_string())
}
fn ws_worker(addr: String, token: String, tx: Sender<NetworkEvent>, ctx: egui::Context) {
    thread::spawn(move || {
        loop {
            let url = format!("ws://127.0.0.1:{addr}/ws/console?token={token}");
            match tungstenite::connect(url.as_str()) {
                Ok((mut socket, _)) => {
                    let _ = tx.send(NetworkEvent::Status("Подключено".into()));
                    loop {
                        match socket.read() {
                            Ok(tungstenite::Message::Text(s)) => {
                                if let Ok(snapshot) = serde_json::from_str::<Snapshot>(&s) {
                                    let ids: Vec<String> = snapshot.agents.iter().map(|a|a.id.clone()).collect();
                                    let watch = serde_json::json!({"type":"watch","ids":ids}).to_string();
                                    let _ = socket.send(tungstenite::Message::Text(watch));
                                    let _ = tx.send(NetworkEvent::Snapshot(snapshot.agents));
                                    ctx.request_repaint();
                                }
                            }
                            Ok(tungstenite::Message::Binary(data)) => {
                                if data.len()<3 {continue;}
                                let n=u16::from_be_bytes([data[0],data[1]]) as usize;
                                if data.len()<n+4 {continue;}
                                if let Ok(id)=std::str::from_utf8(&data[2..2+n]) {
                                    let _=tx.send(NetworkEvent::Frame(id.to_owned(),data[2+n..].to_vec()));
                                    ctx.request_repaint();
                                }
                            }
                            Ok(tungstenite::Message::Ping(data)) => {let _=socket.send(tungstenite::Message::Pong(data));}
                            Ok(_) => {}
                            Err(_) => break,
                        }
                    }
                }
                Err(_) => {}
            }
            let _=tx.send(NetworkEvent::Status("Нет связи — переподключение".into()));
            ctx.request_repaint();
            thread::sleep(Duration::from_secs(2));
        }
    });
}
fn send_command(port: u16, token: String, agent: String, action: String, text: String, tx: Sender<NetworkEvent>) {
    thread::spawn(move || {
        let result = (|| -> Result<String, String> {
            let addr = ("127.0.0.1",port).to_socket_addrs().map_err(|e|e.to_string())?.next().ok_or("Адрес не найден")?;
            let mut stream=TcpStream::connect_timeout(&addr,Duration::from_secs(3)).map_err(|e|e.to_string())?;
            let _=stream.set_read_timeout(Some(Duration::from_secs(3)));
            let body=serde_json::json!({"agent":agent,"action":action,"text":text}).to_string();
            let request=format!("POST /api/command HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len());
            stream.write_all(request.as_bytes()).map_err(|e|e.to_string())?;
            let mut response=String::new();
            stream.read_to_string(&mut response).map_err(|e|e.to_string())?;
            if !response.starts_with("HTTP/1.1 200") {return Err(format!("Сервер отклонил команду: {}",response.lines().next().unwrap_or("?"))); }
            Ok("Команда принята сервером".into())
        })();
        let _ = tx.send(NetworkEvent::Status(result.unwrap_or_else(|e|format!("Ошибка: {e}"))));
    });
}

impl OkoApp {
    fn new(cc: &eframe::CreationContext<'_>, mode: Mode) -> Self {
        let mut fonts = egui::FontDefinitions::default();
        fonts.font_data.insert(
            "DejaVu".into(),
            std::sync::Arc::new(egui::FontData::from_static(include_bytes!("../../assets/DejaVuSans.ttf"))),
        );
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            fonts.families.entry(family).or_default().insert(0,"DejaVu".into());
        }
        cc.egui_ctx.set_fonts(fonts);
        let mut visuals=egui::Visuals::dark();
        visuals.panel_fill=BG;
        visuals.window_fill=PANEL;
        visuals.widgets.noninteractive.bg_fill=PANEL;
        visuals.selection.bg_fill=ACCENT.gamma_multiply(0.30);
        visuals.selection.stroke.color=ACCENT;
        cc.egui_ctx.set_visuals(visuals);
        let mut teacher = load_cfg::<TeacherCfg>("native-teacher.json").unwrap_or_else(|| TeacherCfg {
            token:random_token(),agent_token:random_token(),bind_lan:false,
            port:std::env::var("OKO_NATIVE_PORT").ok().and_then(|s|s.parse().ok()).unwrap_or(7600),
        });
        if teacher.agent_token.len()<16 || teacher.agent_token==teacher.token { teacher.agent_token=random_token(); }
        let _=secure_write("native-teacher.json",&teacher);
        let student = load_cfg::<StudentCfg>("native-student.json").unwrap_or_default();
        let (tx,rx)=mpsc::channel();
        let mut app=Self {
            mode, teacher, student, master:None, agents:vec![],images:HashMap::new(),
            selection:HashSet::new(), expanded:None, filter:String::new(),
            message_text:String::new(), message_open:false,show_key:false,
            status:"Инициализация".into(), last_feedback:String::new(),online:false,
            rx,tx,last_start:Instant::now(),            student_running:false,student_pid:None,
        };
        if mode==Mode::Teacher {app.start_master(&cc.egui_ctx);}
        app
    }
    fn start_master(&mut self, ctx:&egui::Context) {
        if let Some(mut m)=self.master.take() {let _=m.kill();let _=m.wait();}
        if self.teacher.token.len()<16 || self.teacher.agent_token.len()<16 {
            self.status="Не удалось сгенерировать безопасные ключи".into();return;
        }
        let log_dir=state_root();
        let _=fs::create_dir_all(&log_dir);
        let log=fs::OpenOptions::new().create(true).append(true).open(log_dir.join("native-master.log"));
        let bind=if self.teacher.bind_lan{"0.0.0.0"}else{"127.0.0.1"};
        let mut cmd=Command::new(binary("oko-master"));
        cmd.args(["--bind",bind,"--port",&self.teacher.port.to_string(),"--parent-pid",&std::process::id().to_string()])
            .env("OKO_TOKEN",&self.teacher.token)
            .env("OKO_AGENT_TOKEN",&self.teacher.agent_token)
            .stdout(Stdio::null()).stdin(Stdio::null())
            .stderr(log.map(Stdio::from).unwrap_or(Stdio::null()));
        match cmd.spawn() {
            Ok(child)=>{
                self.status=format!("Сервер запущен на порту {}",self.teacher.port);
                self.master=Some(child);
                ws_worker(self.teacher.port.to_string(),self.teacher.token.clone(),self.tx.clone(),ctx.clone());
                self.last_start=Instant::now();
            }
            Err(e)=>self.status=format!("Ошибка запуска сервера: {e}"),
        }
    }
    fn command_ids(&mut self,action:&str,ids:Vec<String>) {
        if ids.is_empty(){self.last_feedback="Выберите компьютер".into();return;}
        for agent in ids {
            send_command(self.teacher.port,self.teacher.token.clone(),agent,
                action.into(),self.message_text.clone(),self.tx.clone());
        }
        self.last_feedback=format!("Команда «{action}» отправлена");
    }
    fn read_events(&mut self,ctx:&egui::Context) {
        for _ in 0..45 {
            let event=match self.rx.try_recv(){Ok(x)=>x,Err(_)=>break};
            match event {
                NetworkEvent::Status(s)=>{
                    if s=="Подключено" { self.online=true; }
                    else if s.starts_with("Нет связи") { self.online=false; }
                    self.status=s.clone();
                    self.last_feedback=s;
                }
                NetworkEvent::Snapshot(agents)=>{
                    let ids:HashSet<_>=agents.iter().map(|a|a.id.clone()).collect();
                    self.images.retain(|id,_|ids.contains(id));
                    self.selection.retain(|id|ids.contains(id));
                    self.agents=agents;
                    self.online=true;
                }
                NetworkEvent::Frame(id,jpeg)=>{
                    if let Ok(image)=image::load_from_memory(&jpeg) {
                        let max_dim=900u32;
                        let rgb=if image.width()>max_dim || image.height()>max_dim {
                            image.resize(max_dim,max_dim,image::imageops::FilterType::Triangle).to_rgba8()
                        } else { image.to_rgba8() };
                        let size=[rgb.width() as usize,rgb.height() as usize];
                        let pixels=rgb.as_raw();
                        let col=egui::ColorImage::from_rgba_unmultiplied(size,pixels);
                        if let Some(frame)=self.images.get_mut(&id) {
                            frame.texture.set(col,egui::TextureOptions::LINEAR);
                            frame.last_update=Instant::now();
                        } else {
                            let texture=ctx.load_texture(format!("oko-frame-{id}"),col,egui::TextureOptions::LINEAR);
                            self.images.insert(id,Frame{texture,last_update:Instant::now()});
                        }
                    }
                }
            }
        }
    }
    fn topbar(&mut self,ctx:&egui::Context) {
        egui::TopBottomPanel::top("top").exact_height(63.0).frame(egui::Frame::default().fill(PANEL)
            .inner_margin(egui::Margin::symmetric(20,10))).show(ctx,|ui|{
            ui.horizontal_centered(|ui|{
                ui.label(RichText::new("◉").color(ACCENT).size(26.0));
                ui.label(RichText::new("ОКО").size(23.0).strong().color(Color32::WHITE));
                ui.add_space(12.0);
                ui.label(RichText::new(if self.mode==Mode::Teacher{"ПРЕПОДАВАТЕЛЬ"}else{"УЧЕНИК"}).color(MUTED).size(12.0));
                ui.add_space(18.0);
                let text=if self.mode==Mode::Teacher {
                    if self.online{format!("●  Подключено · {} ПК",self.agents.len())}
                    else{"●  Ожидание соединения".to_string()}
                } else if self.student_running {
                    "●  Агент запущен".to_string()
                } else {"●  Агент остановлен".to_string()};
                ui.label(RichText::new(text).color(if self.online||self.student_running{ACCENT}else{MUTED}));
            });
        });
    }
    fn teacher_sidebar(&mut self,ctx:&egui::Context) {
        egui::SidePanel::left("sidebar").resizable(false).exact_width(248.0)
            .frame(egui::Frame::default().fill(PANEL).inner_margin(egui::Margin::same(16)))
            .show(ctx,|ui|{
                ui.add_space(12.0);
                ui.heading(RichText::new("Управление классом").color(Color32::WHITE));
                ui.add_space(8.0);
                ui.label(RichText::new("Подключённые компьютеры").color(MUTED));
                ui.heading(RichText::new(self.agents.len().to_string()).size(35.0).color(ACCENT));
                ui.separator();
                ui.add_space(12.0);
                ui.label(RichText::new(format!("Выбрано: {}",self.selection.len())).color(MUTED));
                ui.add_space(6.0);
                if ui.add_sized([213.0,38.0],egui::Button::new("🔒  Заблокировать")).clicked() {
                    self.command_ids("lock",self.selection.iter().cloned().collect());
                }
                if ui.add_sized([213.0,38.0],egui::Button::new("🔓  Разблокировать")).clicked() {
                    self.command_ids("unlock",self.selection.iter().cloned().collect());
                }
                if ui.add_sized([213.0,38.0],egui::Button::new("✉  Сообщение")).clicked() {
                    if !self.selection.is_empty(){self.message_open=true;}
                }
                if ui.button("Снять выделение").clicked(){self.selection.clear();}
                ui.add_space(16.0);
                ui.separator();
                ui.add_space(10.0);
                ui.label(RichText::new("Сервер класса").strong());
                ui.monospace(format!("Порт: {}",self.teacher.port));
                ui.horizontal(|ui|{
                    let mut checked=self.teacher.bind_lan;
                    if ui.checkbox(&mut checked,"Разрешить подключения из LAN").changed(){
                        self.teacher.bind_lan=checked;
                        let _=secure_write("native-teacher.json",&self.teacher);
                        self.start_master(ctx);
                    }
                });
                if self.teacher.bind_lan {ui.colored_label(WARNING,"Только доверенная сеть / VPN. Без TLS.");}
                ui.add_space(6.0);
                ui.horizontal(|ui|{
                    if ui.button(if self.show_key{"Скрыть ключ ученика"}else{"Показать ключ ученика"}).clicked(){self.show_key=!self.show_key;}
                });
                if self.show_key {
                    ui.add(egui::TextEdit::multiline(&mut self.teacher.agent_token.clone()).desired_rows(3).interactive(false));
                    if ui.button("Скопировать ключ ученика").clicked(){ctx.copy_text(self.teacher.agent_token.clone());}
                }
                ui.add_space(12.0);
                ui.separator();
                ui.add_space(7.0);
                ui.label(RichText::new("Состояние").strong());
                ui.label(RichText::new(&self.status).color(MUTED).size(12.0));
                if ui.button("Перезапустить сервер").clicked(){self.start_master(ctx);}
                ui.add_space(10.0);
                ui.label(RichText::new(&self.last_feedback).size(11.0).color(ACCENT));
            });
    }
    fn card(ui:&mut egui::Ui, a:&AgentInfo, frame:Option<&Frame>,selected:bool)->(bool,bool,bool) {
        let mut toggle=false;let mut open=false;let mut lock=false;
        egui::Frame::default().fill(PANEL).stroke(egui::Stroke::new(if selected{2.0}else{1.0},if selected{ACCENT}else{Color32::from_rgb(46,57,72)}))
            .corner_radius(egui::CornerRadius::same(12)).inner_margin(egui::Margin::same(10))
            .show(ui,|ui|{
                ui.set_min_width(ui.available_width());
                let width=ui.available_width();
                let height=width*0.61;
                if let Some(f)=frame {
                    let size=egui::vec2(width,height);
                    if ui.add(egui::ImageButton::new((f.texture.id(),size))
                        .frame(false)).clicked(){toggle=true;}
                } else {
                    let (rect,response)=ui.allocate_exact_size(egui::vec2(width,height),egui::Sense::click());
                    ui.painter().rect_filled(rect,8.0,Color32::from_rgb(12,17,25));
                    ui.painter().text(rect.center(),egui::Align2::CENTER_CENTER,"Ожидание кадра",egui::FontId::proportional(14.0),MUTED);
                    if response.clicked(){toggle=true;}
                }
                ui.horizontal(|ui|{
                    let indicator=if a.last_frame_ms<4000{ACCENT}else{WARNING};
                    ui.label(RichText::new("●").color(indicator));
                    ui.label(RichText::new(&a.name).strong().color(Color32::WHITE));
                    if a.locked {ui.colored_label(WARNING,"🔒");}
                });
                ui.label(RichText::new(format!("{} · {} · {}×{} · {:.1} fps",a.hostname,a.backend,a.screen[0],a.screen[1],a.fps)).size(11.0).color(MUTED));
                ui.horizontal(|ui|{
                    if ui.small_button("⛶ Открыть").clicked(){open=true;}
                    if ui.small_button(if a.locked{"🔓 Снять"}else{"🔒 Блок"}).clicked(){lock=true;}
                });
            });
        (toggle,open,lock)
    }
    fn teacher_main(&mut self,ctx:&egui::Context) {
        self.teacher_sidebar(ctx);
        egui::CentralPanel::default().frame(egui::Frame::default().fill(BG).inner_margin(egui::Margin::same(18)))
        .show(ctx,|ui|{
            if let Some(id)=self.expanded.clone() {
                ui.horizontal(|ui|{
                    if ui.button("← К компьютерам").clicked(){self.expanded=None;}
                    ui.heading(self.agents.iter().find(|a|a.id==id).map(|a|a.name.clone()).unwrap_or(id.clone()));
                });
                ui.add_space(10.0);
                if let Some(f)=self.images.get(&id) {
                    let size=f.texture.size_vec2();
                    let ratio=(ui.available_width()/size.x).min((ui.available_height()-50.0)/size.y).min(1.0);
                    ui.image((f.texture.id(),size*ratio));
                } else {ui.label("Ожидание изображения...");}
                ui.horizontal(|ui|{
                    if ui.button("🔒 Блокировать").clicked(){self.command_ids("lock",vec![id.clone()]);}
                    if ui.button("🔓 Разблокировать").clicked(){self.command_ids("unlock",vec![id.clone()]);}
                    if ui.button("✉ Сообщение").clicked(){self.selection.insert(id.clone());self.message_open=true;}
                });
                return;
            }
            ui.horizontal(|ui|{
                ui.heading(RichText::new("Экраны компьютеров").size(23.0));
                ui.add_space(18.0);
                ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("Поиск: ПК, имя, хост").desired_width(280.0));
            });
            ui.add_space(15.0);
            let filtered:Vec<AgentInfo>=self.agents.iter()
                .filter(|a|format!("{} {} {} {}",a.id,a.name,a.hostname,a.user).to_lowercase().contains(&self.filter.to_lowercase()))
                .cloned().collect();
            if filtered.is_empty(){ui.add_space(30.0);ui.colored_label(MUTED,if self.online{"Нет подходящих компьютеров"}else{"Ожидание подключения учеников..."});return;}
            let columns=if ui.available_width()>930.0{3}else if ui.available_width()>530.0{2}else{1};
            let spacing=12.0;
            let width=(ui.available_width()-(columns as f32-1.0)*spacing)/columns as f32;
            egui::ScrollArea::vertical().show(ui,|ui|{
                for chunk in filtered.chunks(columns) {
                    ui.horizontal_top(|ui|{
                        for a in chunk {
                            ui.allocate_ui_with_layout(egui::vec2(width,width*0.61+100.0),egui::Layout::top_down(egui::Align::Min),|ui|{
                                let (toggle,open,lock)=Self::card(ui,a,self.images.get(&a.id),self.selection.contains(&a.id));
                                if toggle {if !self.selection.insert(a.id.clone()){self.selection.remove(&a.id);}}
                                if open {self.expanded=Some(a.id.clone());}
                                if lock {self.command_ids(if a.locked{"unlock"}else{"lock"},vec![a.id.clone()]);}
                            });
                        }
                    });
                    ui.add_space(spacing);
                }
            });
        });
        if self.message_open {
            let mut show=self.message_open;
            let mut send=false;
            egui::Window::new("Сообщение ученикам").collapsible(false).resizable(false)
                .open(&mut show).show(ctx,|ui|{
                    ui.label("Текст сообщения:");
                    ui.add_sized([380.0,110.0],egui::TextEdit::multiline(&mut self.message_text));
                    if ui.button("Отправить выбранным").clicked(){send=true;}
                });
            self.message_open=show;
            if send {self.command_ids("message",self.selection.iter().cloned().collect());self.message_open=false;}
        }
    }

    fn student_main(&mut self,ctx:&egui::Context) {
        let actual=student_pid();
        self.student_running=actual.is_some();
        self.student_pid=actual;
        egui::CentralPanel::default().frame(egui::Frame::default().fill(BG).inner_margin(egui::Margin::same(24)))
            .show(ctx,|ui|{
                ui.add_space(10.0);
                ui.heading(RichText::new("ОКО · Компьютер ученика").size(26.0).color(Color32::WHITE));
                ui.label(RichText::new("Настройка передачи экрана преподавателю").color(MUTED));
                ui.add_space(20.0);
                egui::Frame::default().fill(PANEL).corner_radius(12.0).inner_margin(egui::Margin::same(22)).show(ui,|ui|{
                    ui.set_max_width(650.0);
                    ui.label(RichText::new("Подключение").size(20.0).strong().color(ACCENT));
                    ui.add_space(10.0);
                    ui.label("Адрес сервера (например, ws://192.168.1.10:7600)");
                    ui.add_sized([530.0,31.0],egui::TextEdit::singleline(&mut self.student.server).hint_text("ws://192.168.1.10:7600"));
                    ui.add_space(7.0);
                    ui.label("Ключ доступа (выдаёт преподаватель)");
                    ui.horizontal(|ui|{
                        ui.add_sized([445.0,31.0],egui::TextEdit::singleline(&mut self.student.token).password(!self.show_key));
                        if ui.button(if self.show_key{"Скрыть"}else{"Показать"}).clicked(){self.show_key=!self.show_key;}
                    });
                    ui.add_space(7.0);
                    ui.label("Имя компьютера в консоли");
                    ui.add_sized([360.0,31.0],egui::TextEdit::singleline(&mut self.student.name).hint_text("Например, ПК-12"));
                    ui.add_space(10.0);
                    let mut auto=self.student.autostart;
                    if ui.checkbox(&mut auto,"Автозапуск при входе в рабочий стол").changed(){
                        self.student.autostart=auto;
                        if let Err(e)=set_autostart(auto){self.last_feedback=format!("Автозагрузка: {e}");self.student.autostart=false;}
                        let _=secure_write("native-student.json",&self.student);
                    }
                    ui.label(RichText::new("Автозапуск работает через XDG и не требует systemd.").color(MUTED).size(11.0));
                    ui.add_space(16.0);
                    ui.horizontal(|ui|{
                        if ui.add_sized([150.0,36.0],egui::Button::new("Сохранить")).clicked() {
                            match secure_write("native-student.json",&self.student){
                                Ok(_)=>self.last_feedback="Настройки сохранены".into(),
                                Err(e)=>self.last_feedback=format!("Ошибка записи: {e}"),
                            }
                        }
                        if ui.add_sized([185.0,36.0],egui::Button::new("▶ Подключиться").fill(ACCENT)).clicked(){
                            match secure_write("native-student.json",&self.student)
                                .and_then(|_|spawn_agent(&self.student).map(|pid|{self.student_pid=Some(pid)})){
                                    Ok(_)=>self.last_feedback="Агент запущен; соединение проверяйте по журналу".into(),
                                    Err(e)=>self.last_feedback=e,
                                };
                        }
                        if ui.add_sized([160.0,36.0],egui::Button::new("■ Остановить")).clicked(){
                            match stop_agent(){
                                Ok(_)=>self.last_feedback="Передача экрана остановлена".into(),
                                Err(e)=>self.last_feedback=e,
                            }
                        }
                    });
                });
                ui.add_space(18.0);
                egui::Frame::default().fill(PANEL).corner_radius(12.0).inner_margin(egui::Margin::same(20)).show(ui,|ui|{
                    ui.label(RichText::new("Диагностика").size(18.0).strong());
                    ui.add_space(7.0);
                    ui.horizontal(|ui|{
                        ui.label(RichText::new(if self.student_running{"● Агент запущен"}else{"● Агент не запущен"})
                            .color(if self.student_running{ACCENT}else{WARNING}));
                        if let Some(pid)=self.student_pid{ui.label(RichText::new(format!("PID {pid}")).color(MUTED));}
                    });
                    ui.label(format!("Графический сеанс: {}",std::env::var("XDG_SESSION_TYPE").unwrap_or_else(|_|"не определён".into())));
                    let is_wayland=std::env::var("XDG_SESSION_TYPE").ok().as_deref()==Some("wayland");
                    if is_wayland {
                        ui.colored_label(WARNING,"Wayland: для wlroots нужен grim; GNOME/KDE ScreenCast Portal пока отсутствует.");
                    } else if std::env::var_os("DISPLAY").is_some() {
                        ui.label(RichText::new("X11: захват доступен при наличии прав на экран").color(ACCENT));
                    } else {
                        ui.colored_label(WARNING,"Графический сеанс не обнаружен — захват может быть недоступен.");
                    }
                    ui.add_space(6.0);
                    ui.label(RichText::new("Данные передаются только при запущенном агенте. При отсутствии захвата демонстрационный экран не включается.").color(MUTED).size(12.0));
                });
                ui.add_space(14.0);
                if !self.last_feedback.is_empty(){ui.colored_label(ACCENT,&self.last_feedback);}
            });
    }
}

impl eframe::App for OkoApp {
    fn update(&mut self,ctx:&egui::Context,_frame:&mut eframe::Frame) {
        self.read_events(ctx);
        if self.mode==Mode::Teacher {
            if let Some(master)=self.master.as_mut() {
                if let Ok(Some(status))=master.try_wait(){
                    self.status=format!("Сервер завершился: {status}. Проверьте, свободен ли порт.");
                    self.online=false;
                    self.master=None;
                }
            }
        }
        self.topbar(ctx);
        match self.mode {
            Mode::Teacher=>self.teacher_main(ctx),
            Mode::Student=>self.student_main(ctx),
        }
        ctx.request_repaint_after(if self.mode==Mode::Student {Duration::from_secs(1)}else{Duration::from_millis(120)});
    }
}
impl Drop for OkoApp {
    fn drop(&mut self) {
        if let Some(mut master)=self.master.take() {
            let _=master.kill();
            let _=master.wait();
        }
    }
}
fn main(){
    let args:Vec<_>=std::env::args().collect();
    if args.iter().any(|x|x=="--background-agent") {
        if let Some(cfg)=load_cfg::<StudentCfg>("native-student.json") {
            match spawn_agent(&cfg) {
                Ok(pid)=>eprintln!("ОКО: агент запущен с PID {pid}"),
                Err(e)=>eprintln!("ОКО: автозапуск невозможен: {e}"),
            }
        }
        return;
    }
    if args.iter().any(|x|x=="--help" || x=="-h"){
        println!("ОКО — настольное приложение (Rust/egui)\n--teacher  открыть окно преподавателя\n--student  открыть окно ученика\n--background-agent  запустить агент без окна\n--help  помощь");
        return;
    }
    let mode=if args.iter().any(|x|x=="--student"){Mode::Student}else{Mode::Teacher};
    let options=eframe::NativeOptions{
        viewport:egui::ViewportBuilder::default().with_inner_size(if mode==Mode::Teacher{[1180.0,790.0]}else{[820.0,740.0]})
            .with_min_inner_size([620.0,530.0]),
        ..Default::default()
    };
    if let Err(e)=eframe::run_native("ОКО · Управление классом",options,Box::new(move |cc| Ok(Box::new(OkoApp::new(cc,mode))))) {
        eprintln!("ОКО: не удалось открыть графическое окно: {e}");
        std::process::exit(1);
    }
}
