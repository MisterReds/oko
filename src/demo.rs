// ОКО — демо-бэкенд: синтетический «экран ученика» для быстрого теста без реального рабочего стола.
use crate::{encode_jpeg, Capture, Command};
use ab_glyph::{point, Font, FontArc, Glyph, PxScale, ScaleFont};
use image::RgbImage;
use std::time::{Duration, Instant};

pub const W: u32 = 1280;
pub const H: u32 = 800;

type C = [u8; 3];

const FONT: &[u8] = include_bytes!("../assets/DejaVuSans.ttf");

fn fract(x: f32) -> f32 {
    x - x.floor()
}

/// Детерминированный «шум» без внешних зависимостей.
fn hash(n: u32) -> f32 {
    fract((n.wrapping_mul(12_989_817).wrapping_add(7_823_317) as f32 / 4_294_967_295.0).abs())
}

fn lerp(a: C, b: C, k: f32) -> C {
    let k = k.clamp(0.0, 1.0);
    [
        (a[0] as f32 + (b[0] as f32 - a[0] as f32) * k) as u8,
        (a[1] as f32 + (b[1] as f32 - a[1] as f32) * k) as u8,
        (a[2] as f32 + (b[2] as f32 - a[2] as f32) * k) as u8,
    ]
}

// ---------- примитивы рисования ----------

fn blend(img: &mut RgbImage, x: i32, y: i32, c: C, a: f32) {
    let a = a.clamp(0.0, 1.0);
    if a <= 0.0 || x < 0 || y < 0 || x >= img.width() as i32 || y >= img.height() as i32 {
        return;
    }
    let p = img.get_pixel_mut(x as u32, y as u32);
    for i in 0..3 {
        let o = p.0[i] as f32;
        p.0[i] = (o + (c[i] as f32 - o) * a).round().clamp(0.0, 255.0) as u8;
    }
}

fn fill_rect(img: &mut RgbImage, x: i32, y: i32, w: i32, h: i32, c: C, a: f32) {
    for yy in y..y + h {
        for xx in x..x + w {
            blend(img, xx, yy, c, a);
        }
    }
}

fn fill_circle(img: &mut RgbImage, cx: i32, cy: i32, r: i32, c: C, a: f32) {
    for yy in cy - r..=cy + r {
        for xx in cx - r..=cx + r {
            let dx = (xx - cx) as f32;
            let dy = (yy - cy) as f32;
            if dx * dx + dy * dy <= (r * r) as f32 {
                blend(img, xx, yy, c, a);
            }
        }
    }
}

fn ring(img: &mut RgbImage, cx: i32, cy: i32, r: i32, th: i32, c: C, a: f32) {
    for yy in cy - r - th..=cy + r + th {
        for xx in cx - r - th..=cx + r + th {
            let dx = (xx - cx) as f32;
            let dy = (yy - cy) as f32;
            let d = (dx * dx + dy * dy).sqrt();
            if (r - th) as f32 <= d && d <= (r + th) as f32 {
                blend(img, xx, yy, c, a);
            }
        }
    }
}

fn round_rect(img: &mut RgbImage, x: i32, y: i32, w: i32, h: i32, r: i32, c: C, a: f32) {
    let r = r.min(w / 2).min(h / 2).max(0);
    for yy in y..y + h {
        for xx in x..x + w {
            let ex = if xx < x + r {
                x + r - xx
            } else if xx > x + w - 1 - r {
                xx - (x + w - 1 - r)
            } else {
                0
            };
            let ey = if yy < y + r {
                y + r - yy
            } else if yy > y + h - 1 - r {
                yy - (y + h - 1 - r)
            } else {
                0
            };
            let inside = ex == 0 || ey == 0 || ex * ex + ey * ey <= r * r;
            if inside {
                blend(img, xx, yy, c, a);
            }
        }
    }
}

/// Панель со скруглением только сверху (заголовок окна).
fn top_bar(img: &mut RgbImage, x: i32, y: i32, w: i32, h: i32, r: i32, c: C) {
    round_rect(img, x, y, w, h, r, c, 1.0);
    fill_rect(img, x, y + r, w, (h - r).max(0), c, 1.0);
}

fn fill_tri(img: &mut RgbImage, p0: (i32, i32), p1: (i32, i32), p2: (i32, i32), c: C, a: f32) {
    let (ax, ay) = (p0.0 as f32, p0.1 as f32);
    let (bx, by) = (p1.0 as f32, p1.1 as f32);
    let (cx, cy) = (p2.0 as f32, p2.1 as f32);
    let minx = ax.min(bx).min(cx).floor() as i32;
    let maxx = ax.max(bx).max(cx).ceil() as i32;
    let miny = ay.min(by).min(cy).floor() as i32;
    let maxy = ay.max(by).max(cy).ceil() as i32;
    let edge = |px: f32, py: f32, x1: f32, y1: f32, x2: f32, y2: f32| {
        (px - x1) * (y2 - y1) - (py - y1) * (x2 - x1)
    };
    for yy in miny..=maxy {
        for xx in minx..=maxx {
            let (px, py) = (xx as f32 + 0.5, yy as f32 + 0.5);
            let e1 = edge(px, py, ax, ay, bx, by);
            let e2 = edge(px, py, bx, by, cx, cy);
            let e3 = edge(px, py, cx, cy, ax, ay);
            let inside =
                (e1 >= 0.0 && e2 >= 0.0 && e3 >= 0.0) || (e1 <= 0.0 && e2 <= 0.0 && e3 <= 0.0);
            if inside {
                blend(img, xx, yy, c, a);
            }
        }
    }
}

// ---------- текст ----------

fn text_w(font: &FontArc, scale: f32, s: &str) -> i32 {
    let sf = font.as_scaled(PxScale::from(scale));
    (s.chars()
        .map(|ch| sf.h_advance(font.glyph_id(ch)))
        .sum::<f32>())
    .round() as i32
}

fn text(img: &mut RgbImage, font: &FontArc, scale: f32, x: i32, y: i32, c: C, s: &str) -> i32 {
    let sf = font.as_scaled(PxScale::from(scale));
    let ascent = sf.ascent();
    let mut pen = x as f32;
    for ch in s.chars() {
        let gid = font.glyph_id(ch);
        let g = Glyph {
            id: gid,
            scale: PxScale::from(scale),
            position: point(pen, y as f32 + ascent),
        };
        if let Some(og) = font.outline_glyph(g) {
            let b = og.px_bounds();
            let (bx, by) = (b.min.x.round() as i32, b.min.y.round() as i32);
            og.draw(|gx, gy, v| {
                blend(img, bx + gx as i32, by + gy as i32, c, v);
            });
        }
        pen += sf.h_advance(gid);
    }
    (pen - x as f32).round() as i32
}

fn text_center(img: &mut RgbImage, font: &FontArc, scale: f32, cx: i32, y: i32, c: C, s: &str) {
    let w = text_w(font, scale, s);
    text(img, font, scale, cx - w / 2, y, c, s);
}

fn wrap(font: &FontArc, scale: f32, s: &str, maxw: i32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    for word in s.split_whitespace() {
        let probe = if cur.is_empty() {
            word.to_string()
        } else {
            format!("{cur} {word}")
        };
        if text_w(font, scale, &probe) > maxw && !cur.is_empty() {
            lines.push(std::mem::take(&mut cur));
            cur = word.to_string();
        } else {
            cur = probe;
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

// ---------- демо-захват ----------

struct Theme {
    accent: C,
}

fn theme_for(name: &str) -> Theme {
    let seed: u32 = name.bytes().map(|b| b as u32).sum::<u32>().max(1);
    let k = hash(seed);
    let teal: C = [45, 212, 191];
    let violet: C = [139, 124, 246];
    let amber: C = [251, 191, 36];
    let accent = if k < 0.5 {
        lerp(teal, violet, k * 2.0)
    } else {
        lerp(violet, amber, (k - 0.5) * 2.0)
    };
    Theme { accent }
}

pub struct DemoCapture {
    name: String,
    font: FontArc,
    img: RgbImage,
    locked: bool,
    msg: Option<(String, Instant)>,
    theme: Theme,
    quality: u8,
}

impl DemoCapture {
    pub fn new(name: &str, quality: u8) -> anyhow::Result<Self> {
        let font = FontArc::try_from_slice(FONT)?;
        Ok(Self {
            name: name.to_string(),
            font,
            img: RgbImage::new(W, H),
            locked: false,
            msg: None,
            theme: theme_for(name),
            quality,
        })
    }

    fn render(&mut self, t: f64) {
        let (w, h) = (W as i32, H as i32);
        let img = &mut self.img;
        let acc = self.theme.accent;
        let name = self.name.clone();

        // фон: вертикальный градиент
        let top: C = [26, 32, 47];
        let bot: C = [14, 17, 26];
        for (yy, row) in img.chunks_exact_mut((W * 3) as usize).enumerate() {
            let c = lerp(top, bot, yy as f32 / (H - 1) as f32);
            for px in row.chunks_exact_mut(3) {
                px.copy_from_slice(&c);
            }
        }
        // мягкое пятно акцента на «обоях»
        let bx = (w as f32 * 0.68) as i32;
        let by = (h as f32 * 0.28) as i32;
        let br = w as f32 * 0.30;
        for yy in by - br as i32..=by + br as i32 {
            for xx in bx - br as i32..=bx + br as i32 {
                let dx = (xx - bx) as f32;
                let dy = (yy - by) as f32;
                let d = (dx * dx + dy * dy).sqrt();
                if d < br {
                    let k = 1.0 - d / br;
                    blend(img, xx, yy, acc, 0.12 * k * k);
                }
            }
        }

        // главное окно «редактор»
        let (wx, wy, ww, wh) = (200, 120, 620, 520);
        round_rect(
            img,
            wx - 10,
            wy - 10,
            ww + 20,
            wh + 20,
            16,
            [30, 37, 48],
            0.6,
        );
        round_rect(img, wx, wy, ww, wh, 12, [34, 41, 52], 1.0);
        top_bar(img, wx, wy, ww, 36, 12, [42, 50, 63]);
        for k in 0..3 {
            fill_circle(img, wx + 18 + k * 18, wy + 18, 5, [70, 78, 90], 1.0);
        }
        text(
            img,
            &self.font,
            13.0,
            wx + 76,
            wy + 11,
            [152, 160, 172],
            "Конспект урока — заметки",
        );

        // «текст» в редакторе: полосы, длина растёт со временем, курсор мигает
        let (cx0, cy0, maxw) = (wx + 18, wy + 54, ww - 36);
        let step = 2.5;
        let n_lines = 8 + ((t / step) as u32 % 8);
        let active = (t / step) % 8.0;
        for i in 0..n_lines as i32 {
            let lw = (maxw as f32
                * (0.30 + 0.62 * hash((i as u32).wrapping_mul(7).wrapping_add(3))))
                as i32;
            let y = cy0 + i * 17;
            fill_rect(img, cx0, y, lw, 9, [72, 83, 100], 1.0);
            if (i as f64) == active.floor() {
                let blink = (t.fract() as f32) < 0.5;
                if blink {
                    fill_rect(img, cx0 + lw + 3, y - 2, 2, 13, acc, 1.0);
                }
            }
        }

        // окно «браузер» поверх справа
        let (bx0, by0, bw, bh) = (700, 60, 420, 320);
        round_rect(
            img,
            bx0 - 10,
            by0 - 10,
            bw + 20,
            bh + 20,
            16,
            [30, 37, 48],
            0.6,
        );
        round_rect(img, bx0, by0, bw, bh, 12, [27, 33, 42], 1.0);
        top_bar(img, bx0, by0, bw, 34, 12, [36, 44, 56]);
        round_rect(img, bx0 + 12, by0 + 8, bw - 24, 18, 9, [52, 62, 77], 1.0);
        text(
            img,
            &self.font,
            11.0,
            bx0 + 22,
            by0 + 11,
            [130, 139, 152],
            "oko.lab/урок-01",
        );
        round_rect(img, bx0 + 14, by0 + 50, bw - 28, 84, 8, acc, 0.30);
        for i in 0..3 {
            let lw = ((bw - 28) as f32
                * (0.45 + 0.4 * hash((i as u32).wrapping_mul(13).wrapping_add(9))))
                as i32;
            fill_rect(img, bx0 + 14, by0 + 150 + i * 22, lw, 10, [66, 77, 94], 1.0);
        }

        // панель задач
        let th = 34;
        fill_rect(img, 0, h - th, w, th, [19, 23, 30], 1.0);
        for k in 0..4 {
            let c = if k == 0 { acc } else { [40, 48, 60] };
            round_rect(
                img,
                16 + k * 44,
                h - th + 8,
                30,
                18,
                5,
                c,
                if k == 0 { 0.85 } else { 1.0 },
            );
        }
        // часы
        let secs = 9 * 3600 + t as u64;
        let clock = format!("{:02}:{:02}", secs / 3600 % 24, secs % 3600 / 60);
        let cw = text_w(&self.font, 14.0, &clock);
        text(
            img,
            &self.font,
            14.0,
            w - cw - 16,
            h - th + 9,
            [142, 150, 162],
            &clock,
        );

        // чип с именем ученика
        let chip_w = text_w(&self.font, 13.0, &name) + 46;
        round_rect(img, 16, 16, chip_w, 26, 13, [17, 22, 30], 0.94);
        fill_circle(img, 16 + 14, 29, 4, [74, 222, 128], 1.0);
        text(img, &self.font, 13.0, 16 + 26, 20, [226, 232, 240], &name);

        // курсор мыши
        let cxx = (w as f32 * 0.52
            + (t as f32 * 0.7).sin() * w as f32 * 0.16
            + (t as f32 * 1.7).sin() * 14.0) as i32;
        let cyy = (h as f32 * 0.42
            + (t as f32 * 0.53).cos() * h as f32 * 0.10
            + (t as f32 * 1.1).cos() * 10.0) as i32;
        fill_tri(
            img,
            (cxx + 1, cyy + 1),
            (cxx + 1, cyy + 17),
            (cxx + 12, cyy + 7),
            [0, 0, 0],
            0.55,
        );
        fill_tri(
            img,
            (cxx, cyy),
            (cxx, cyy + 16),
            (cxx + 11, cyy + 6),
            [240, 243, 248],
            1.0,
        );
        let phase = t % 6.0;
        if phase < 0.35 {
            ring(
                img,
                cxx + 2,
                cyy + 2,
                6 + (phase * 60.0) as i32,
                2,
                [240, 243, 248],
                0.8 * (1.0 - phase / 0.35) as f32,
            );
        }

        // тост с сообщением преподавателя
        let expired = self
            .msg
            .as_ref()
            .map_or(false, |(_, until)| Instant::now() > *until);
        if expired {
            self.msg = None;
        }
        if let Some((msg, _)) = &self.msg {
            let lines = wrap(&self.font, 15.0, msg, 560);
            let box_h = lines.len() as i32 * 22 + 40;
            let box_w = lines
                .iter()
                .map(|l| text_w(&self.font, 15.0, l))
                .max()
                .unwrap_or(200)
                .clamp(220, 620)
                + 56;
            let x = (w - box_w) / 2;
            let y = h - th - 24 - box_h;
            round_rect(img, x - 3, y - 3, box_w + 6, box_h + 6, 13, acc, 0.95);
            round_rect(img, x, y, box_w, box_h, 10, [12, 16, 22], 0.97);
            fill_rect(img, x + 1, y + 12, 3, box_h - 24, acc, 1.0);
            text(
                img,
                &self.font,
                11.0,
                x + 18,
                y + 11,
                [126, 135, 148],
                "Сообщение преподавателя",
            );
            for (i, l) in lines.iter().enumerate() {
                text(
                    img,
                    &self.font,
                    15.0,
                    x + 18,
                    y + 28 + i as i32 * 22,
                    [232, 237, 244],
                    l,
                );
            }
        }

        // замок поверх всего
        if self.locked {
            fill_rect(img, 0, 0, w, h, [5, 7, 10], 0.88);
            let cx = w / 2;
            let cy = h / 2 - 46;
            ring(img, cx, cy - 6, 12, 4, [231, 234, 240], 1.0);
            round_rect(img, cx - 24, cy + 2, 48, 38, 8, [231, 234, 240], 1.0);
            fill_circle(img, cx, cy + 21, 5, [5, 7, 10], 1.0);
            fill_rect(img, cx - 1, cy + 18, 2, 10, [5, 7, 10], 1.0);
            text_center(
                img,
                &self.font,
                26.0,
                cx,
                cy + 62,
                [231, 234, 240],
                "Экран заблокирован",
            );
            text_center(
                img,
                &self.font,
                14.0,
                cx,
                cy + 104,
                [120, 130, 144],
                "ОКО · преподаватель",
            );
        }
    }
}

impl Capture for DemoCapture {
    fn backend_name(&self) -> &'static str {
        "demo"
    }

    fn screen(&self) -> [u32; 2] {
        [W, H]
    }

    fn grab(&mut self, t: f64) -> anyhow::Result<Vec<u8>> {
        self.render(t);
        encode_jpeg(&self.img, self.quality)
    }

    fn handle(&mut self, cmd: &Command) -> anyhow::Result<Option<bool>> {
        match cmd.action.as_str() {
            "lock" => {
                self.locked = true;
                Ok(Some(true))
            }
            "unlock" => {
                self.locked = false;
                Ok(Some(false))
            }
            "message" => {
                if let Some(t) = cmd.text.clone() {
                    self.msg = Some((t, Instant::now() + Duration::from_secs(6)));
                }
                Ok(None)
            }
            _ => anyhow::bail!("unknown command {}", cmd.action),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn avg_luma(img: &image::DynamicImage) -> f32 {
        let img = img.to_luma8();
        let sum: u32 = img.as_raw().iter().map(|v| *v as u32).sum();
        sum as f32 / img.as_raw().len() as f32
    }

    #[test]
    fn frames_change_and_decode() {
        let mut cap = DemoCapture::new("ПК-01", 70).expect("демо-бэкенд");
        let f0 = cap.grab(0.0).expect("кадр 0");
        let f3 = cap.grab(3.0).expect("кадр 3");
        assert_ne!(f0, f3, "кадры должны отличаться со временем");
        let img = image::load_from_memory(&f0).expect("декодирование JPEG");
        assert_eq!((img.width(), img.height()), (W, H));
    }

    #[test]
    fn lock_darkens_screen() {
        let mut cap = DemoCapture::new("ПК-02", 70).expect("демо-бэкенд");
        let open = image::load_from_memory(&cap.grab(5.0).expect("кадр")).expect("декод");
        cap.handle(&Command {
            action: "lock".into(),
            text: None,
        })
        .unwrap();
        let locked = image::load_from_memory(&cap.grab(5.0).expect("кадр")).expect("декод");
        assert!(avg_luma(&locked) < avg_luma(&open), "замок затемняет экран");
    }

    #[test]
    fn text_is_rendered() {
        let mut cap = DemoCapture::new("ПК-03", 90).expect("демо-бэкенд");
        let img = image::load_from_memory(&cap.grab(1.0).expect("кадр")).expect("декод");
        // чип имени в левом верхнем углу: там должны быть светлые пиксели (текст)
        let crop = img.crop_imm(30, 16, 120, 30).to_luma8();
        let max = crop.as_raw().iter().max().copied().unwrap_or(0);
        assert!(max > 200, "текст имени должен быть виден (max={max})");
    }
}
