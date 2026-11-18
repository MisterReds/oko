// Захват Wayland на wlroots через установленный пользователем grim.
// Для GNOME/KDE нужен XDG ScreenCast portal — этот backend их не заменяет.
use crate::{Capture, Command};
use anyhow::{bail, Context, Result};
use std::process::Command as ProcessCommand;

pub struct GrimCapture {
    size: [u32; 2],
    first: Option<Vec<u8>>,
}
impl GrimCapture {
    fn capture() -> Result<Vec<u8>> {
        let result = ProcessCommand::new("grim").args(["-t", "jpeg", "-"]).output()
            .context("grim не найден в PATH")?;
        if !result.status.success() || result.stdout.len() < 4 ||
            result.stdout[..2] != [0xff, 0xd8] {
            bail!("grim не смог захватить экран: {}", String::from_utf8_lossy(&result.stderr));
        }
        Ok(result.stdout)
    }
    pub fn new() -> Result<Self> {
        let first = Self::capture()?;
        let img = image::load_from_memory(&first)?;
        Ok(Self { size: [img.width(), img.height()], first: Some(first) })
    }
}
impl Capture for GrimCapture {
    fn backend_name(&self) -> &'static str { "wayland-grim" }
    fn screen(&self) -> [u32; 2] { self.size }
    fn grab(&mut self, _t: f64) -> Result<Vec<u8>> {
        if let Some(x) = self.first.take() { return Ok(x); }
        Self::capture()
    }
    fn handle(&mut self, cmd: &Command) {
        if cmd.action == "message" {
            crate::notify_desktop(cmd.text.as_deref().unwrap_or(""));
        } else {
            eprintln!("[oko-agent] Wayland: команда '{}' не реализована, рабочий стол не изменён", cmd.action);
        }
    }
}
