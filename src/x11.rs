// ОКО — X11-бэкенд: захват экрана через XGetImage и визуальная блокировка оверлеем.
// Реальный X11 захват; демонстрационный режим включается исключительно явно через --demo.
use crate::{encode_jpeg, Capture, Command};
use anyhow::{anyhow, Context, Result};
use image::RgbImage;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    ConnectionExt, CreateWindowAux, Drawable, ImageFormat, WindowClass,
};
use x11rb::rust_connection::RustConnection;

pub struct X11Capture {
    conn: RustConnection,
    root: Drawable,
    w: u32,
    h: u32,
    black: u32,
    depth: u8,
    visual: u32,
    overlay: Option<u32>,
    quality: u8,
}

impl X11Capture {
    pub fn new(quality: u8) -> Result<Self> {
        let (conn, idx) = x11rb::connect(None).context("не удалось подключиться к X-серверу")?;
        let screen = &conn.setup().roots[idx];
        let (root, w, h, black, depth, visual) = (
            screen.root,
            screen.width_in_pixels as u32,
            screen.height_in_pixels as u32,
            screen.black_pixel,
            screen.root_depth,
            screen.root_visual,
        );
        Ok(Self {
            conn,
            root,
            w,
            h,
            black,
            depth,
            visual,
            overlay: None,
            quality,
        })
    }

    fn lock_screen(&mut self) -> Result<()> {
        if self.overlay.is_some() {
            return Ok(());
        }
        let win = self.conn.generate_id()?;
        let aux = CreateWindowAux::new()
            .background_pixel(self.black)
            .override_redirect(1);
        self.conn.create_window(
            self.depth,
            win,
            self.root,
            0,
            0,
            self.w as u16,
            self.h as u16,
            0,
            WindowClass::INPUT_OUTPUT,
            self.visual,
            &aux,
        )?;
        self.conn.map_window(win)?;
        self.conn.flush()?;
        self.overlay = Some(win);
        Ok(())
    }

    fn unlock_screen(&mut self) -> Result<()> {
        if let Some(win) = self.overlay.take() {
            self.conn.unmap_window(win)?;
            self.conn.destroy_window(win)?;
            self.conn.flush()?;
        }
        Ok(())
    }
}

impl Capture for X11Capture {
    fn backend_name(&self) -> &'static str {
        "x11"
    }

    fn screen(&self) -> [u32; 2] {
        [self.w, self.h]
    }

    fn grab(&mut self, _t: f64) -> Result<Vec<u8>> {
        let reply = self
            .conn
            .get_image(ImageFormat::Z_PIXMAP, self.root, 0, 0, self.w as u16, self.h as u16, u32::MAX)?
            .reply()?;
        // ZPixmap 32bpp: B, G, R, pad
        let mut rgb = Vec::with_capacity((self.w * self.h * 3) as usize);
        for px in reply.data.chunks_exact(4) {
            rgb.push(px[2]);
            rgb.push(px[1]);
            rgb.push(px[0]);
        }
        let img = RgbImage::from_raw(self.w, self.h, rgb).ok_or_else(|| anyhow!("размер буфера"))?;
        encode_jpeg(&img, self.quality)
    }

    fn handle(&mut self, cmd: &Command) {
        let r = match cmd.action.as_str() {
            "lock" => self.lock_screen(),
            "unlock" => self.unlock_screen(),
            "message" => {
                // Desktop notification (XDG); журнал сохраняется как fallback.
                let text=cmd.text.as_deref().unwrap_or("");
                eprintln!("[oko-agent] сообщение: {text}");
                crate::notify_desktop(text);
                Ok(())
            }
            _ => Ok(()),
        };
        if let Err(e) = r {
            eprintln!("[oko-agent] x11: {e:#}");
        }
    }
}
