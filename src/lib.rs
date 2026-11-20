// ОКО — типы протокола и общие утилиты.
pub mod demo;
pub mod grim;
pub mod status;
pub mod x11;

use serde::{Deserialize, Serialize};

/// Уведомление преподавателя в активной пользовательской сессии Linux.
/// Не запускает shell; notify-send — опциональный системный компонент.
pub fn notify_desktop(text: &str) {
    if text.trim().is_empty() {
        return;
    }
    if let Err(e) = std::process::Command::new("notify-send")
        .args(["-t", "6000", "ОКО — сообщение преподавателя", text])
        .spawn()
    {
        eprintln!("[oko-agent] notify-send недоступен: {e}");
    }
}

/// Первое сообщение агента мастеру (JSON по WebSocket).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hello {
    #[serde(rename = "type")]
    pub kind: String, // "hello"
    pub name: String,
    pub hostname: String,
    pub user: String,
    pub backend: String, // "demo" | "x11"
    pub screen: [u32; 2],
    pub locked: bool,
}

/// Агент → мастер: обновление состояния (kind = "state").
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentStateMsg {
    #[serde(rename = "type")]
    pub kind: String,
    pub locked: bool,
}

/// Мастер → агент: команда (action = "lock" | "unlock" | "message").
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Command {
    pub action: String,
    #[serde(default)]
    pub text: Option<String>,
}

/// Консоль → мастер: команда по конкретному агенту.
#[derive(Debug, Clone, Deserialize)]
pub struct CommandReq {
    pub agent: String,
    pub action: String,
    #[serde(default)]
    pub text: Option<String>,
}

/// Консоль → мастер: подписка на кадры выбранных агентов.
#[derive(Debug, Deserialize)]
pub struct WatchMsg {
    #[serde(rename = "type")]
    pub kind: String, // "watch"
    pub ids: Vec<String>,
}

/// Агент в снимке состояния для консоли.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInfo {
    pub id: String,
    pub name: String,
    pub hostname: String,
    pub user: String,
    pub backend: String,
    pub screen: [u32; 2],
    pub locked: bool,
    pub last_frame_ms: u64,
    pub fps: f32,
    pub connected_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub agents: Vec<AgentInfo>,
}

/// Трейс бэкенда захвата: агент отдаёт готовые JPEG-кадры и принимает команды.
pub trait Capture: Send {
    fn backend_name(&self) -> &'static str;
    fn screen(&self) -> [u32; 2];
    /// t — секунды с момента старта (для анимации демо-экрана).
    fn grab(&mut self, t: f64) -> anyhow::Result<Vec<u8>>;
    /// Return the actual lock state only after the command succeeded.
    /// A failed command must never be acknowledged as successfully applied.
    fn handle(&mut self, cmd: &Command) -> anyhow::Result<Option<bool>>;
}

/// Упаковка кадра для консоли: [u16 BE: длина id][id utf8][jpeg].
pub fn pack_frame(id: &str, jpeg: &[u8]) -> Vec<u8> {
    let idb = id.as_bytes();
    let mut out = Vec::with_capacity(2 + idb.len() + jpeg.len());
    out.extend_from_slice(&(idb.len() as u16).to_be_bytes());
    out.extend_from_slice(idb);
    out.extend_from_slice(jpeg);
    out
}

/// Обратная распаковка: (id, jpeg-байты).
pub fn unpack_frame(buf: &[u8]) -> Option<(&str, &[u8])> {
    if buf.len() < 2 {
        return None;
    }
    let n = u16::from_be_bytes([buf[0], buf[1]]) as usize;
    if buf.len() < 2 + n {
        return None;
    }
    let id = std::str::from_utf8(&buf[2..2 + n]).ok()?;
    Some((id, &buf[2 + n..]))
}

/// Кодирование кадра в JPEG.
pub fn encode_jpeg(img: &image::RgbImage, quality: u8) -> anyhow::Result<Vec<u8>> {
    use image::ImageEncoder;
    let mut buf = Vec::new();
    {
        let mut cursor = std::io::Cursor::new(&mut buf);
        let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut cursor, quality);
        enc.write_image(
            img.as_raw(),
            img.width(),
            img.height(),
            image::ExtendedColorType::Rgb8,
        )?;
    }
    Ok(buf)
}

/// Идентификатор агента: безопасные ASCII-символы, всё прочее → '-'.
pub fn sanitize_id(s: &str) -> String {
    let out: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "pc".to_string()
    } else {
        trimmed
    }
}

/// Имя хоста текущей машины без внешних крейтов.
pub fn hostname() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| "unknown-pc".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mux_roundtrip() {
        let jpeg = vec![1u8, 2, 3, 250, 0, 77];
        let packed = pack_frame("pc-01", &jpeg);
        let (id, data) = unpack_frame(&packed).expect("распаковка");
        assert_eq!(id, "pc-01");
        assert_eq!(data, jpeg.as_slice());
    }

    #[test]
    fn mux_rejects_garbage() {
        assert!(unpack_frame(&[0, 9, 1, 2]).is_none());
        assert!(unpack_frame(&[]).is_none());
        assert!(unpack_frame(&[0, 2, b'a']).is_none());
    }

    #[test]
    fn sanitize() {
        assert_eq!(sanitize_id("Lab-PC_01"), "lab-pc_01");
        assert_eq!(sanitize_id("ПК-01"), "01"); // кириллица → '-', затем обрезка
        assert_eq!(sanitize_id("$$$"), "pc");
    }

    #[test]
    fn jpeg_roundtrip() {
        let img = image::RgbImage::from_fn(64, 48, |x, y| {
            image::Rgb([(x * 4) as u8, (y * 5) as u8, 128])
        });
        let bytes = encode_jpeg(&img, 80).expect("кодирование");
        let decoded = image::load_from_memory(&bytes).expect("декодирование");
        assert_eq!((decoded.width(), decoded.height()), (64, 48));
    }
}
