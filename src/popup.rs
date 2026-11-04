//! Detail panel shown when the bar is clicked. Two columns: CPU, RAM and system on the left; GPUs on the right.
//! Closes on focus loss or Esc.

use std::cell::Cell;

use windows::core::{w, Result, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::DWMWCP_ROUND;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::SystemInformation::GetTickCount64;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::VK_ESCAPE;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::gdi::{blend, px, rgb, round_corners, Canvas, Font};
use crate::metrics::{Monitor, Ring, ENGINE_LABELS};
use crate::{with_monitor, OVERLAY, POPUP};

const CLASS: PCWSTR = w!("TaskbarMonitor.Popup");

const BG: COLORREF = rgb(0x20, 0x20, 0x20);
const PANEL: COLORREF = rgb(0x2a, 0x2a, 0x2a);
const TRACK: COLORREF = rgb(0x3a, 0x3a, 0x3a);
const DIV: COLORREF = rgb(0x34, 0x34, 0x34);
const TEXT: COLORREF = rgb(0xf2, 0xf2, 0xf2);
const DIM: COLORREF = rgb(0x9a, 0x9a, 0x9a);
const C_CPU: COLORREF = rgb(0x4f, 0xa3, 0xff);
const C_RAM: COLORREF = rgb(0xb0, 0x7c, 0xff);
const C_GPU: COLORREF = rgb(0x3d, 0xd6, 0x8c);
const C_VRAM: COLORREF = rgb(0xf5, 0xa5, 0x24);

const COL_W: f32 = 300.0;
const GUTTER: f32 = 32.0;
const PAD: f32 = 16.0;

thread_local! {
    static HIDDEN_AT: Cell<u64> = const { Cell::new(0) };
}

pub unsafe fn create(hinst: HINSTANCE) -> Result<HWND> {
    let wc = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(wndproc),
        hInstance: hinst,
        hCursor: LoadCursorW(None, IDC_ARROW)?,
        lpszClassName: CLASS,
        ..Default::default()
    };
    RegisterClassExW(&wc);
    let hwnd = CreateWindowExW(
        WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
        CLASS,
        w!("Taskbar Monitor"),
        WS_POPUP,
        0,
        0,
        10,
        10,
        None,
        None,
        Some(hinst),
        None,
    )?;
    round_corners(hwnd, DWMWCP_ROUND);
    Ok(hwnd)
}

pub fn toggle() {
    let p = POPUP.get();
    unsafe {
        if IsWindowVisible(p).as_bool() {
            hide(p);
            return;
        }
        // If this click just closed the popup by stealing focus, don't reopen it
        if GetTickCount64().saturating_sub(HIDDEN_AT.get()) < 250 {
            return;
        }
        place(p);
        let _ = ShowWindow(p, SW_SHOW);
        let _ = SetForegroundWindow(p);
    }
}

/// Close if open (the menu hangs off this window, so it must not stay on top of it).
pub fn close() {
    let p = POPUP.get();
    unsafe {
        if IsWindowVisible(p).as_bool() {
            hide(p);
        }
    }
}

/// Called every second: refresh if open.
pub fn refresh() {
    let p = POPUP.get();
    unsafe {
        if IsWindowVisible(p).as_bool() {
            place(p);
            let _ = InvalidateRect(Some(p), None, false);
        }
    }
}

unsafe fn hide(p: HWND) {
    let _ = ShowWindow(p, SW_HIDE);
    HIDDEN_AT.set(GetTickCount64());
}

fn scale() -> f32 {
    unsafe { GetDpiForWindow(OVERLAY.get()).max(96) as f32 / 96.0 }
}

fn width(s: f32) -> i32 {
    px(2.0 * PAD + 2.0 * COL_W + GUTTER, s)
}

/// Place the popup just above the bar, aligned with its outer edge, kept inside the work area.
unsafe fn place(p: HWND) {
    let s = scale();
    let w = width(s);
    let h = {
        let c = Canvas::new(w, 1);
        with_monitor(|m| draw(&c, s, m)).unwrap_or(px(200.0, s))
    };
    let mut a = RECT::default();
    let _ = GetWindowRect(OVERLAY.get(), &mut a);
    let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
    let _ = GetMonitorInfoW(MonitorFromRect(&a, MONITOR_DEFAULTTONEAREST), &mut mi);
    let work = mi.rcWork;
    let gap = px(8.0, s);
    let mon = mi.rcMonitor;
    let x = if a.left - mon.left < mon.right - a.right { a.left } else { a.right - w };
    let x = x.min(work.right - w - gap).max(work.left + gap);
    let y = (a.top.min(work.bottom) - h - gap).max(work.top + gap);
    let _ = SetWindowPos(p, Some(HWND_TOPMOST), x, y, w, h, SWP_NOACTIVATE);
}

unsafe fn paint(hwnd: HWND) {
    let mut ps = PAINTSTRUCT::default();
    let hdc = BeginPaint(hwnd, &mut ps);
    let mut rc = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc);
    let c = Canvas::new(rc.right, rc.bottom);
    with_monitor(|m| draw(&c, scale(), m));
    c.blit(hdc);
    let _ = EndPaint(hwnd, &ps);
}

/// Draw all content and return the height needed. Call with a 1px-tall canvas to measure.
fn draw(c: &Canvas, s: f32, m: &Monitor) -> i32 {
    let snap = &m.snap;
    c.fill(0, 0, c.w, c.h, BG);
    let pad = px(PAD, s);
    let col_w = px(COL_W, s);
    let mut d = Doc::new(c, s, pad, col_w);

    // Left column
    d.header("CPU", &pct(snap.cpu), C_CPU);
    d.line(&m.cpu_name, "");
    let ghz = if snap.cpu_mhz > 0.0 { format!("{:.2} GHz", snap.cpu_mhz / 1000.0) } else { String::new() };
    let n = snap.cores.len();
    d.line(&tr!(format!("{n} threads"), format!("{n} luồng")), &ghz);
    d.spark(&m.cpu_hist, C_CPU, None);
    d.cores(&snap.cores);
    d.divider();
    d.header("RAM", &pct(snap.ram_pct()), C_RAM);
    d.line(tr!("In use", "Đang dùng"), &format!("{} / {} GB", gb(snap.ram_used), gb(snap.ram_total)));
    d.line(tr!("Available", "Còn trống"), &format!("{} GB", gb(snap.ram_total.saturating_sub(snap.ram_used))));
    d.line(tr!("Committed", "Bộ nhớ cam kết"), &format!("{} / {} GB", gb(snap.commit_used), gb(snap.commit_limit)));
    d.spark(&m.ram_hist, C_RAM, None);
    d.divider();
    d.header(tr!("System", "Hệ thống"), "", TEXT);
    d.line(tr!("Processes", "Tiến trình"), &thousands(snap.processes as u64));
    d.line(tr!("Threads", "Luồng"), &thousands(snap.threads as u64));
    d.line(tr!("Handles", "Handle"), &thousands(snap.handles as u64));
    d.line(tr!("Up time", "Thời gian chạy"), &uptime(snap.uptime_s));
    let left_h = d.y;

    // Right column
    d.x = pad + col_w + px(GUTTER, s);
    d.y = pad;
    if snap.gpus.is_empty() {
        d.header("GPU", "–", DIM);
        d.line(tr!("GPU information unavailable", "Không đọc được thông tin GPU"), "");
    }
    for (i, g) in snap.gpus.iter().enumerate() {
        if i > 0 {
            d.divider();
        }
        let title = if snap.gpus.len() > 1 { format!("GPU {i}") } else { "GPU".into() };
        d.header(&title, &pct(g.util), C_GPU);
        d.line(&g.name, "");
        for &(k, v) in &g.engines {
            d.meter(ENGINE_LABELS[k], v, C_GPU);
        }
        d.line("VRAM", &format!("{} / {} GB", gb(g.vram_used), gb(g.vram_total)));
        d.thin_bar(g.vram_pct(), C_VRAM);
        d.line(tr!("Shared memory", "Bộ nhớ dùng chung"), &format!("{} GB", gb(g.shared_used)));
        d.spark(&m.gpu_hist[i], C_GPU, Some((&m.vram_hist[i], C_VRAM)));
    }
    let right_h = d.y;

    let h = left_h.max(right_h) + pad;
    // Column divider
    c.fill(pad + col_w + px(GUTTER / 2.0, s), pad, px(1.0, s).max(1), h - 2 * pad, DIV);
    h
}

/// Line-by-line drawing cursor within a column.
struct Doc<'a> {
    c: &'a Canvas,
    s: f32,
    x: i32,
    y: i32,
    w: i32,
    title: Font,
    big: Font,
    body: Font,
}

impl<'a> Doc<'a> {
    fn new(c: &'a Canvas, s: f32, pad: i32, w: i32) -> Self {
        Self {
            c,
            s,
            x: pad,
            y: pad,
            w,
            title: Font::new(px(14.0, s), 700),
            big: Font::new(px(20.0, s), 700),
            body: Font::new(px(12.5, s), 400),
        }
    }

    fn p(&self, v: f32) -> i32 {
        px(v, self.s)
    }

    fn header(&mut self, label: &str, value: &str, color: COLORREF) {
        self.c.font(&self.title);
        self.c.text(self.x, self.y + self.p(5.0), label, TEXT);
        self.c.font(&self.big);
        self.c.text_right(self.x + self.w, self.y, value, color);
        self.y += self.p(30.0);
    }

    /// Dim label on the left, bright value on the right.
    fn line(&mut self, left: &str, right: &str) {
        self.c.font(&self.body);
        let rw = if right.is_empty() { 0 } else { self.c.measure(right).0 + self.p(8.0) };
        self.c.text_fit(self.x, self.y, self.w - rw, self.p(18.0), left, DIM);
        if !right.is_empty() {
            self.c.text_right(self.x + self.w, self.y, right, TEXT);
        }
        self.y += self.p(19.0);
    }

    fn meter(&mut self, label: &str, v: f32, color: COLORREF) {
        self.c.font(&self.body);
        self.c.text(self.x, self.y, label, DIM);
        let (bx, bw, bh) = (self.x + self.p(96.0), self.w - self.p(96.0 + 44.0), self.p(6.0));
        self.c.bar(bx, self.y + self.p(7.0), bw, bh, v, color, TRACK);
        self.c.text_right(self.x + self.w, self.y, &pct(v), TEXT);
        self.y += self.p(20.0);
    }

    fn thin_bar(&mut self, v: f32, color: COLORREF) {
        self.c.bar(self.x, self.y + self.p(2.0), self.w, self.p(6.0), v, color, TRACK);
        self.y += self.p(14.0);
    }

    fn spark(&mut self, data: &Ring, color: COLORREF, second: Option<(&Ring, COLORREF)>) {
        let (y, h) = (self.y + self.p(6.0), self.p(44.0));
        self.c.fill(self.x, y, self.w, h, PANEL);
        let thick = self.p(1.5);
        self.c.spark(self.x, y, self.w, h, data, color, Some(blend(color, PANEL, 0.22)), thick);
        if let Some((d2, c2)) = second {
            self.c.spark(self.x, y, self.w, h, d2, c2, None, thick);
        }
        self.y = y + h + self.p(8.0);
    }

    /// One vertical bar per CPU thread, wrapping to more rows when there are many.
    fn cores(&mut self, cores: &[f32]) {
        let n = cores.len();
        if n == 0 {
            return;
        }
        let gap = self.p(2.0).max(1);
        let per_row = (((self.w + gap) / (self.p(6.0) + gap)).max(1) as usize).min(n);
        let rows = n.div_ceil(per_row);
        let bw = (self.w - gap * (per_row as i32 - 1)) / per_row as i32;
        let bh = if rows == 1 { self.p(28.0) } else { self.p(18.0) };
        for (i, &v) in cores.iter().enumerate() {
            let bx = self.x + (i % per_row) as i32 * (bw + gap);
            let by = self.y + (i / per_row) as i32 * (bh + gap);
            self.c.fill(bx, by, bw, bh, TRACK);
            let fh = (bh as f32 * v / 100.0).round() as i32;
            self.c.fill(bx, by + bh - fh, bw, fh, C_CPU);
        }
        self.y += rows as i32 * (bh + gap) + self.p(2.0);
    }

    fn divider(&mut self) {
        self.y += self.p(10.0);
        self.c.fill(self.x, self.y, self.w, self.p(1.0).max(1), DIV);
        self.y += self.p(12.0);
    }
}

impl Drop for Doc<'_> {
    // Deselect Doc's fonts before they are deleted (DeleteObject fails on a selected font)
    fn drop(&mut self) {
        self.c.release_font();
    }
}

fn pct(v: f32) -> String {
    format!("{v:.0}%")
}

fn gb(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / (1u64 << 30) as f64)
}

fn thousands(n: u64) -> String {
    let s = n.to_string();
    let sep = tr!(',', '.');
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(sep);
        }
        out.push(ch);
    }
    out
}

fn uptime(secs: u64) -> String {
    let (d, h, m, s) = (secs / 86400, secs / 3600 % 24, secs / 60 % 60, secs % 60);
    if d > 0 {
        let days = tr!(if d == 1 { "day" } else { "days" }, "ngày");
        format!("{d} {days} {h:02}:{m:02}:{s:02}")
    } else {
        format!("{h:02}:{m:02}:{s:02}")
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => paint(hwnd),
        WM_ERASEBKGND => return LRESULT(1),
        WM_ACTIVATE if (wp.0 & 0xffff) as u32 == WA_INACTIVE => hide(hwnd),
        WM_KEYDOWN if wp.0 == VK_ESCAPE.0 as usize => hide(hwnd),
        _ => return DefWindowProcW(hwnd, msg, wp, lp),
    }
    LRESULT(0)
}
