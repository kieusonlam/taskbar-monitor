//! Lớp vẽ mỏng trên GDI: mọi thứ vẽ vào bitmap trong bộ nhớ rồi BitBlt một lần, không nháy.

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWM_WINDOW_CORNER_PREFERENCE};
use windows::Win32::Graphics::Gdi::*;

use crate::metrics::{Ring, HISTORY_LEN};

pub const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF(r as u32 | (g as u32) << 8 | (b as u32) << 16)
}

/// Trộn `a` vào `b` theo tỉ lệ `t` (0 = b, 1 = a).
pub fn blend(a: COLORREF, b: COLORREF, t: f32) -> COLORREF {
    let ch = |c: COLORREF, s: u32| ((c.0 >> s) & 0xff) as f32;
    let mix = |s| (ch(a, s) * t + ch(b, s) * (1.0 - t)).round() as u32;
    COLORREF(mix(0) | mix(8) << 8 | mix(16) << 16)
}

pub fn px(v: f32, scale: f32) -> i32 {
    (v * scale).round() as i32
}

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

pub fn round_corners(hwnd: HWND, pref: DWM_WINDOW_CORNER_PREFERENCE) {
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &pref as *const _ as *const _,
            size_of::<DWM_WINDOW_CORNER_PREFERENCE>() as u32,
        );
    }
}

pub struct Font(HFONT);

impl Font {
    pub fn new(px: i32, weight: i32) -> Self {
        let mut lf = LOGFONTW {
            lfHeight: -px,
            lfWeight: weight,
            lfCharSet: DEFAULT_CHARSET,
            lfQuality: CLEARTYPE_QUALITY,
            ..Default::default()
        };
        for (i, c) in "Segoe UI".encode_utf16().enumerate() {
            lf.lfFaceName[i] = c;
        }
        Font(unsafe { CreateFontIndirectW(&lf) })
    }
}

impl Drop for Font {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteObject(self.0.into());
        }
    }
}

/// Bitmap trong bộ nhớ. Font phải được tạo TRƯỚC Canvas để bị hủy SAU nó:
/// Drop của Canvas trả DC về trạng thái gốc, nhả font ra rồi font mới xóa được.
pub struct Canvas {
    dc: HDC,
    bmp: HBITMAP,
    pub w: i32,
    pub h: i32,
}

impl Canvas {
    pub fn new(w: i32, h: i32) -> Self {
        let (w, h) = (w.max(1), h.max(1));
        unsafe {
            let screen = GetDC(None);
            let dc = CreateCompatibleDC(Some(screen));
            let bmp = CreateCompatibleBitmap(screen, w, h);
            ReleaseDC(None, screen);
            SaveDC(dc);
            SelectObject(dc, bmp.into());
            SetBkMode(dc, TRANSPARENT);
            Self { dc, bmp, w, h }
        }
    }

    pub fn blit(&self, dst: HDC) {
        unsafe {
            let _ = BitBlt(dst, 0, 0, self.w, self.h, Some(self.dc), 0, 0, SRCCOPY);
        }
    }

    pub fn font(&self, f: &Font) {
        unsafe { SelectObject(self.dc, f.0.into()) };
    }

    /// Chọn font hệ thống vào DC để font đang dùng có thể bị xóa.
    pub fn release_font(&self) {
        unsafe { SelectObject(self.dc, GetStockObject(DEFAULT_GUI_FONT)) };
    }

    pub fn measure(&self, s: &str) -> (i32, i32) {
        let mut size = SIZE::default();
        unsafe {
            let _ = GetTextExtentPoint32W(self.dc, &wide(s), &mut size);
        }
        (size.cx, size.cy)
    }

    pub fn text(&self, x: i32, y: i32, s: &str, color: COLORREF) {
        unsafe {
            SetTextColor(self.dc, color);
            let _ = TextOutW(self.dc, x, y, &wide(s));
        }
    }

    pub fn text_right(&self, right: i32, y: i32, s: &str, color: COLORREF) {
        let (w, _) = self.measure(s);
        self.text(right - w, y, s, color);
    }

    /// Một dòng, cắt bằng "…" nếu dài quá `w`.
    pub fn text_fit(&self, x: i32, y: i32, w: i32, h: i32, s: &str, color: COLORREF) {
        let mut r = RECT { left: x, top: y, right: x + w, bottom: y + h };
        unsafe {
            SetTextColor(self.dc, color);
            DrawTextW(self.dc, &mut wide(s), &mut r, DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX);
        }
    }

    pub fn fill(&self, x: i32, y: i32, w: i32, h: i32, color: COLORREF) {
        if w <= 0 || h <= 0 {
            return;
        }
        unsafe {
            let b = CreateSolidBrush(color);
            FillRect(self.dc, &RECT { left: x, top: y, right: x + w, bottom: y + h }, b);
            let _ = DeleteObject(b.into());
        }
    }

    pub fn dot(&self, x: i32, y: i32, d: i32, color: COLORREF) {
        unsafe {
            let brush = CreateSolidBrush(color);
            let old_b = SelectObject(self.dc, brush.into());
            let old_p = SelectObject(self.dc, GetStockObject(NULL_PEN));
            let _ = Ellipse(self.dc, x, y, x + d + 1, y + d + 1);
            SelectObject(self.dc, old_p);
            SelectObject(self.dc, old_b);
            let _ = DeleteObject(brush.into());
        }
    }

    /// Thanh ngang, `pct` 0..100.
    pub fn bar(&self, x: i32, y: i32, w: i32, h: i32, pct: f32, fg: COLORREF, track: COLORREF) {
        self.fill(x, y, w, h, track);
        let fw = (w as f32 * pct.clamp(0.0, 100.0) / 100.0).round() as i32;
        self.fill(x, y, fw, h, fg);
    }

    /// Đồ thị 60 giây, mẫu mới nhất ở mép phải. `area`: tô vùng dưới đường.
    pub fn spark(&self, x: i32, y: i32, w: i32, h: i32, data: &Ring, color: COLORREF, area: Option<COLORREF>, thick: i32) {
        if data.len() < 2 {
            return;
        }
        let step = (w - 1) as f32 / (HISTORY_LEN - 1) as f32;
        let off = HISTORY_LEN - data.len();
        let pts: Vec<POINT> = data
            .iter()
            .enumerate()
            .map(|(i, v)| POINT {
                x: x + ((off + i) as f32 * step).round() as i32,
                y: y + h - 1 - ((v / 100.0).clamp(0.0, 1.0) * (h - 1) as f32).round() as i32,
            })
            .collect();
        unsafe {
            if let Some(fc) = area {
                let mut poly = pts.clone();
                poly.push(POINT { x: pts[pts.len() - 1].x, y: y + h });
                poly.push(POINT { x: pts[0].x, y: y + h });
                let brush = CreateSolidBrush(fc);
                let old_b = SelectObject(self.dc, brush.into());
                let old_p = SelectObject(self.dc, GetStockObject(NULL_PEN));
                let _ = Polygon(self.dc, &poly);
                SelectObject(self.dc, old_p);
                SelectObject(self.dc, old_b);
                let _ = DeleteObject(brush.into());
            }
            let pen = CreatePen(PS_SOLID, thick.max(1), color);
            let old = SelectObject(self.dc, pen.into());
            let _ = Polyline(self.dc, &pts);
            SelectObject(self.dc, old);
            let _ = DeleteObject(pen.into());
        }
    }
}

impl Drop for Canvas {
    fn drop(&mut self) {
        unsafe {
            let _ = RestoreDC(self.dc, -1);
            let _ = DeleteObject(self.bmp.into());
            let _ = DeleteDC(self.dc);
        }
    }
}
