//! The metrics bar drawn over the taskbar.
//!
//! Windows 11 removed DeskBands, so nothing can be embedded in the taskbar. Instead this is a
//! borderless topmost window placed just left of the notification area, pushed back on top
//! whenever the taskbar reclaims the z-order.

use std::cell::Cell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use windows::core::{w, Result, HSTRING, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::DWMWCP_ROUNDSMALL;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::gdi::{px, rgb, round_corners, Canvas, Font};
use crate::{autostart, popup, update, with_monitor};

const CLASS: PCWSTR = w!("TaskbarMonitor.Overlay");
const TIMER_TICK: usize = 1;
const TIMER_UPDATE: usize = 2;
const ID_AUTOSTART: usize = 101;
const ID_EXIT: usize = 102;
const ID_UPDATE: usize = 103;
const MA_NOACTIVATE: isize = 3;
/// wParam: 0 = up to date, 1 = update available, 2 = error. lParam: 1 if the user asked for the check.
const WM_APP_CHECKED: u32 = WM_APP + 1;
/// wParam: 1 = exe replaced, 0 = error.
const WM_APP_APPLIED: u32 = WM_APP + 2;
const UPDATE_FIRST_CHECK_MS: u32 = 15_000;
const UPDATE_INTERVAL_MS: u32 = 24 * 60 * 60 * 1000;

/// Tag of a release newer than the running one, if any. Written by the checker thread, read on the UI thread.
static LATEST: Mutex<Option<String>> = Mutex::new(None);
static LAST_ERROR: Mutex<String> = Mutex::new(String::new());
static UPDATING: AtomicBool = AtomicBool::new(false);

const BG: COLORREF = rgb(0x1c, 0x1c, 0x1c);
const LABEL: COLORREF = rgb(0xb4, 0xb4, 0xb4);
const VALUE: COLORREF = rgb(0xf2, 0xf2, 0xf2);
const WARN: COLORREF = rgb(0xff, 0xd1, 0x66);
const HOT: COLORREF = rgb(0xff, 0x6b, 0x6b);
const UPDATE_DOT: COLORREF = rgb(0x4f, 0xa3, 0xff);

const LABELS: [&str; 4] = ["CPU", "GPU", "VRAM", "RAM"];

thread_local! {
    static TASKBAR_CREATED: Cell<u32> = const { Cell::new(0) };
}

pub unsafe fn create(hinst: HINSTANCE) -> Result<HWND> {
    let wc = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(wndproc),
        hInstance: hinst,
        hCursor: LoadCursorW(None, IDC_HAND)?,
        lpszClassName: CLASS,
        ..Default::default()
    };
    RegisterClassExW(&wc);
    let hwnd = CreateWindowExW(
        WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED,
        CLASS,
        w!("Taskbar Monitor"),
        WS_POPUP,
        0,
        0,
        0,
        0,
        None,
        None,
        Some(hinst),
        None,
    )?;
    SetLayeredWindowAttributes(hwnd, COLORREF(0), 245, LWA_ALPHA)?;
    round_corners(hwnd, DWMWCP_ROUNDSMALL);
    TASKBAR_CREATED.set(RegisterWindowMessageW(w!("TaskbarCreated")));
    SetTimer(Some(hwnd), TIMER_TICK, 1000, None);
    SetTimer(Some(hwnd), TIMER_UPDATE, UPDATE_FIRST_CHECK_MS, None);
    Ok(hwnd)
}

fn latest() -> Option<String> {
    LATEST.lock().ok()?.clone()
}

fn message(hwnd: HWND, text: &str, flags: MESSAGEBOX_STYLE) -> MESSAGEBOX_RESULT {
    unsafe { MessageBoxW(Some(hwnd), &HSTRING::from(text), w!("Taskbar Monitor"), flags) }
}

/// Ask GitHub on a worker thread and report back with WM_APP_CHECKED.
fn check_async(hwnd: HWND, manual: bool) {
    let h = hwnd.0 as isize;
    std::thread::spawn(move || {
        let code = match update::latest_tag() {
            Ok(tag) if update::is_newer(&tag) => {
                *LATEST.lock().unwrap() = Some(tag);
                1
            }
            Ok(_) => 0,
            Err(e) => {
                *LAST_ERROR.lock().unwrap() = e;
                2
            }
        };
        unsafe {
            let _ = PostMessageW(Some(HWND(h as _)), WM_APP_CHECKED, WPARAM(code), LPARAM(manual as isize));
        }
    });
}

/// Download and replace the exe on a worker thread and report back with WM_APP_APPLIED.
fn apply_async(hwnd: HWND, tag: String) {
    if UPDATING.swap(true, Ordering::SeqCst) {
        return;
    }
    let h = hwnd.0 as isize;
    std::thread::spawn(move || {
        let ok = match update::apply(&tag) {
            Ok(()) => 1,
            Err(e) => {
                *LAST_ERROR.lock().unwrap() = e;
                0
            }
        };
        unsafe {
            let _ = PostMessageW(Some(HWND(h as _)), WM_APP_APPLIED, WPARAM(ok), LPARAM(0));
        }
    });
}

unsafe fn on_checked(hwnd: HWND, code: usize, manual: bool) {
    let _ = InvalidateRect(Some(hwnd), None, false);
    if !manual {
        return;
    }
    match code {
        1 => {
            let tag = latest().unwrap_or_default();
            let text = format!("Có bản mới {tag} (đang dùng v{}).\n\nCập nhật ngay?", update::VERSION);
            if message(hwnd, &text, MB_YESNO | MB_ICONQUESTION) == IDYES {
                apply_async(hwnd, tag);
            }
        }
        0 => {
            message(hwnd, &format!("Bạn đang dùng bản mới nhất (v{}).", update::VERSION), MB_OK | MB_ICONINFORMATION);
        }
        _ => {
            let e = LAST_ERROR.lock().map(|e| e.clone()).unwrap_or_default();
            message(hwnd, &format!("Không kiểm tra được cập nhật.\n\n{e}"), MB_OK | MB_ICONWARNING);
        }
    }
}

unsafe fn on_applied(hwnd: HWND, ok: bool) {
    UPDATING.store(false, Ordering::SeqCst);
    if !ok {
        let e = LAST_ERROR.lock().map(|e| e.clone()).unwrap_or_default();
        message(hwnd, &format!("Cập nhật thất bại.\n\n{e}"), MB_OK | MB_ICONWARNING);
        return;
    }
    // The exe has been replaced: start the new version (it waits for this one to exit before taking the single-instance lock)
    if let Some(exe) = update::exe_path() {
        let _ = std::process::Command::new(exe).arg("--after-update").spawn();
    }
    let _ = DestroyWindow(hwnd);
}

struct Layout {
    pad: i32,
    gap_label: i32,
    gap_item: i32,
    label_w: [i32; 4],
    value_w: i32,
    width: i32,
}

fn font(scale: f32) -> Font {
    Font::new(px(14.0, scale), 700)
}

fn layout(scale: f32) -> Layout {
    let f = font(scale);
    let c = Canvas::new(1, 1);
    c.font(&f);
    let p = |v| px(v, scale);
    let label_w = LABELS.map(|l| c.measure(l).0);
    // Value slots are sized for "100%" so the bar doesn't change width as numbers change
    let value_w = c.measure("100%").0;
    let (pad, gap_label, gap_item) = (p(12.0), p(6.0), p(16.0));
    let width = 2 * pad
        + label_w.iter().sum::<i32>()
        + 4 * (gap_label + value_w)
        + 3 * gap_item;
    Layout { pad, gap_label, gap_item, label_w, value_w, width }
}

fn load_color(v: f32) -> COLORREF {
    if v >= 85.0 {
        HOT
    } else if v >= 60.0 {
        WARN
    } else {
        VALUE
    }
}

/// Reposition, resize and push above the taskbar. Called every second and on every foreground change.
pub fn update_position(hwnd: HWND) {
    unsafe {
        let Some((x, y, w, h)) = target_rect() else {
            let _ = ShowWindow(hwnd, SW_HIDE);
            return;
        };
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), x, y, w, h, SWP_NOACTIVATE | SWP_SHOWWINDOW);
    }
}

unsafe fn target_rect() -> Option<(i32, i32, i32, i32)> {
    let tray = FindWindowW(w!("Shell_TrayWnd"), PCWSTR::null()).ok()?;
    if !IsWindowVisible(tray).as_bool() || fullscreen_app_active(tray) {
        return None;
    }
    let mut tr = RECT::default();
    GetWindowRect(tray, &mut tr).ok()?;

    // Auto-hidden taskbar is tucked away: hide too
    let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
    if GetMonitorInfoW(MonitorFromWindow(tray, MONITOR_DEFAULTTONEAREST), &mut mi).as_bool()
        && tr.bottom > mi.rcMonitor.bottom + 1
    {
        return None;
    }

    let scale = GetDpiForWindow(tray) as f32 / 96.0;
    let l = layout(scale);
    let right = match FindWindowExW(Some(tray), None, w!("TrayNotifyWnd"), PCWSTR::null()) {
        Ok(n) => {
            let mut r = RECT::default();
            let _ = GetWindowRect(n, &mut r);
            r.left
        }
        Err(_) => tr.right - px(300.0, scale),
    };
    let th = tr.bottom - tr.top;
    let h = (th - px(12.0, scale)).max(px(24.0, scale)).min(th);
    Some((right - px(8.0, scale) - l.width, tr.top + (th - h) / 2, l.width, h))
}

/// Whether a fullscreen app (game, video) is active on the taskbar's monitor.
unsafe fn fullscreen_app_active(tray: HWND) -> bool {
    let fg = GetForegroundWindow();
    if fg.is_invalid() {
        return false;
    }
    let mut cls = [0u16; 64];
    let n = GetClassNameW(fg, &mut cls) as usize;
    let cls = String::from_utf16_lossy(&cls[..n]);
    if matches!(cls.as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd")
        || cls.starts_with("TaskbarMonitor")
    {
        return false;
    }
    let mon = MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST);
    if mon != MonitorFromWindow(tray, MONITOR_DEFAULTTONEAREST) {
        return false;
    }
    let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
    let mut r = RECT::default();
    if !GetMonitorInfoW(mon, &mut mi).as_bool() || GetWindowRect(fg, &mut r).is_err() {
        return false;
    }
    let m = mi.rcMonitor;
    r.left <= m.left && r.top <= m.top && r.right >= m.right && r.bottom >= m.bottom
}

unsafe fn paint(hwnd: HWND) {
    let mut ps = PAINTSTRUCT::default();
    let hdc = BeginPaint(hwnd, &mut ps);
    let mut rc = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc);
    let scale = GetDpiForWindow(hwnd) as f32 / 96.0;
    let l = layout(scale);

    let f = font(scale);
    let c = Canvas::new(rc.right, rc.bottom);
    c.fill(0, 0, c.w, c.h, BG);
    c.font(&f);

    let vals = with_monitor(|m| {
        let g = m.snap.main_gpu();
        [Some(m.snap.cpu), g.map(|g| g.util), g.map(|g| g.vram_pct()), Some(m.snap.ram_pct())]
    })
    .unwrap_or([None; 4]);

    let y = (c.h - c.measure("0").1) / 2;
    let mut x = l.pad;
    for (i, label) in LABELS.iter().enumerate() {
        c.text(x, y, label, LABEL);
        x += l.label_w[i] + l.gap_label + l.value_w;
        match vals[i] {
            Some(v) => c.text_right(x, y, &format!("{v:.0}%"), load_color(v)),
            None => c.text_right(x, y, "–", LABEL),
        }
        x += l.gap_item;
    }
    if latest().is_some() {
        // Update-available dot
        let d = px(6.0, scale);
        c.dot(c.w - d - px(4.0, scale), px(4.0, scale), d, UPDATE_DOT);
    }
    c.blit(hdc);
    let _ = EndPaint(hwnd, &ps);
}

unsafe fn context_menu(hwnd: HWND) {
    let Ok(menu) = CreatePopupMenu() else { return };
    let latest = latest();
    let update_label = match &latest {
        Some(tag) => format!("Cập nhật lên {tag} (đang dùng v{})", update::VERSION),
        None => format!("Kiểm tra cập nhật (v{})", update::VERSION),
    };
    let _ = AppendMenuW(menu, MF_STRING, ID_UPDATE, &HSTRING::from(update_label));
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
    let check = if autostart::enabled() { MF_CHECKED } else { MF_UNCHECKED };
    let _ = AppendMenuW(menu, MF_STRING | check, ID_AUTOSTART, w!("Khởi động cùng Windows"));
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
    let _ = AppendMenuW(menu, MF_STRING, ID_EXIT, w!("Thoát"));
    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    // Must be foreground or the menu won't close when clicking elsewhere
    let _ = SetForegroundWindow(hwnd);
    let cmd = TrackPopupMenu(
        menu,
        TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN | TPM_RIGHTALIGN,
        pt.x,
        pt.y,
        None,
        hwnd,
        None,
    );
    let _ = DestroyMenu(menu);
    match cmd.0 as usize {
        ID_UPDATE => match latest {
            Some(tag) => apply_async(hwnd, tag),
            None => check_async(hwnd, true),
        },
        ID_AUTOSTART => autostart::set(!autostart::enabled()),
        ID_EXIT => {
            let _ = DestroyWindow(hwnd);
        }
        _ => {}
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_TIMER if wp.0 == TIMER_TICK => crate::tick(),
        WM_TIMER if wp.0 == TIMER_UPDATE => {
            // First check after 15 seconds, then once a day
            SetTimer(Some(hwnd), TIMER_UPDATE, UPDATE_INTERVAL_MS, None);
            if latest().is_none() {
                check_async(hwnd, false);
            }
        }
        WM_APP_CHECKED => on_checked(hwnd, wp.0, lp.0 != 0),
        WM_APP_APPLIED => on_applied(hwnd, wp.0 != 0),
        WM_PAINT => paint(hwnd),
        WM_ERASEBKGND => return LRESULT(1),
        WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE),
        WM_LBUTTONUP => popup::toggle(),
        WM_RBUTTONUP => context_menu(hwnd),
        WM_DPICHANGED | WM_DISPLAYCHANGE | WM_SETTINGCHANGE => update_position(hwnd),
        WM_DESTROY => PostQuitMessage(0),
        m if m == TASKBAR_CREATED.get() => update_position(hwnd),
        _ => return DefWindowProcW(hwnd, msg, wp, lp),
    }
    LRESULT(0)
}
