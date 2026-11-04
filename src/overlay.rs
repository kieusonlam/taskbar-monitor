//! The metrics bar drawn over the taskbar.
//!
//! Windows 11 removed DeskBands, so nothing can be embedded in the taskbar. Instead this is a
//! borderless topmost window placed just left of the notification area, put back above the taskbar
//! whenever the taskbar reclaims the z-order.
//!
//! While Start or Quick Settings is open the shell lifts the taskbar from the desktop z-band (1)
//! into band 6, above every ordinary topmost window, and it can stay there after the panel closes
//! (e.g. when the panel is closed by clicking its taskbar button). Windows owned by the taskbar are
//! lifted with it, so the bar is created owned by the taskbar; the shell then drops the ownership
//! but leaves the bar in band 6 for good.

use std::cell::Cell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use windows::core::{s, w, Result, BOOL, HSTRING, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::DWMWCP_ROUNDSMALL;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows::Win32::System::Registry::*;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::gdi::{px, rgb, round_corners, Canvas, Font};
use crate::i18n::{self, Choice};
use crate::{autostart, popup, update, with_monitor};

const CLASS: PCWSTR = w!("TaskbarMonitor.Overlay");
const TIMER_TICK: usize = 1;
const TIMER_UPDATE: usize = 2;
const ID_AUTOSTART: usize = 101;
const ID_EXIT: usize = 102;
const ID_UPDATE: usize = 103;
const ID_LANG_AUTO: usize = 110;
const ID_LANG_EN: usize = 111;
const ID_LANG_VI: usize = 112;
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
/// Set before destroying the bar on purpose. Otherwise its destruction came from the taskbar (its
/// owner) going away with Explorer, and the bar is recreated instead of quitting.
static QUITTING: AtomicBool = AtomicBool::new(false);
/// Posted to the UI thread (no window) to recreate the bar after it was destroyed with its owner.
pub const WM_APP_RECREATE: u32 = WM_APP + 3;
/// Appbar notifications from the shell. wParam: ABN_* code.
const WM_APP_APPBAR: u32 = WM_APP + 4;

const BG: COLORREF = rgb(0x1c, 0x1c, 0x1c);
const LABEL: COLORREF = rgb(0xb4, 0xb4, 0xb4);
const VALUE: COLORREF = rgb(0xf2, 0xf2, 0xf2);
const WARN: COLORREF = rgb(0xff, 0xd1, 0x66);
const HOT: COLORREF = rgb(0xff, 0x6b, 0x6b);
const UPDATE_DOT: COLORREF = rgb(0x4f, 0xa3, 0xff);

const LABELS: [&str; 4] = ["CPU", "GPU", "VRAM", "RAM"];
const ID_SHOW: usize = 120; // + index into LABELS
const ID_POS_RIGHT: usize = 130;
const ID_POS_LEFT: usize = 131;

/// Which metrics are on the bar, one bit per entry of LABELS. Stored in
/// HKCU\Software\TaskbarMonitor\Metrics; defaults to CPU, GPU and RAM.
const DEFAULT_SHOWN: u32 = 0b1011;
const SETTINGS_KEY: PCWSTR = w!("Software\\TaskbarMonitor");
const SHOWN_VALUE: PCWSTR = w!("Metrics");
/// Where the bar sits on the taskbar: 0 = right, next to the notification area; 1 = left edge.
const POSITION_VALUE: PCWSTR = w!("Position");
const EXPLORER_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Advanced");

thread_local! {
    static TASKBAR_CREATED: Cell<u32> = const { Cell::new(0) };
    static SHOWN: Cell<u32> = Cell::new(load_shown());
    static LEFT: Cell<bool> = Cell::new(read_dword(SETTINGS_KEY, POSITION_VALUE) == Some(1));
}

fn read_dword(key: PCWSTR, name: PCWSTR) -> Option<u32> {
    let mut v = 0u32;
    let mut size = 4u32;
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key,
            name,
            RRF_RT_REG_DWORD,
            None,
            Some(&mut v as *mut u32 as _),
            Some(&mut size),
        )
    };
    (r == ERROR_SUCCESS).then_some(v)
}

fn write_setting(name: PCWSTR, v: u32) {
    unsafe {
        let _ = RegSetKeyValueW(HKEY_CURRENT_USER, SETTINGS_KEY, name, REG_DWORD.0, Some(&v as *const u32 as _), 4);
    }
}

fn load_shown() -> u32 {
    match read_dword(SETTINGS_KEY, SHOWN_VALUE).map(|v| v & 0b1111) {
        Some(bits) if bits != 0 => bits,
        _ => DEFAULT_SHOWN,
    }
}

fn set_shown(bits: u32) {
    SHOWN.set(bits);
    write_setting(SHOWN_VALUE, bits);
}

/// With left-aligned taskbar icons the Start button is at the left edge, so the bar can't go there.
fn taskbar_centered() -> bool {
    read_dword(EXPLORER_KEY, w!("TaskbarAl")) != Some(0)
}

fn on_left() -> bool {
    LEFT.get() && taskbar_centered()
}

fn shown(i: usize) -> bool {
    SHOWN.get() & (1 << i) != 0
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
    let owner = FindWindowW(w!("Shell_TrayWnd"), PCWSTR::null()).ok();
    let hwnd = CreateWindowExW(
        WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED,
        CLASS,
        w!("Taskbar Monitor"),
        WS_POPUP,
        0,
        0,
        0,
        0,
        owner,
        None,
        Some(hinst),
        None,
    )?;
    SetLayeredWindowAttributes(hwnd, COLORREF(0), 245, LWA_ALPHA)?;
    round_corners(hwnd, DWMWCP_ROUNDSMALL);
    TASKBAR_CREATED.set(RegisterWindowMessageW(w!("TaskbarCreated")));
    SetTimer(Some(hwnd), TIMER_TICK, 1000, None);
    SetTimer(Some(hwnd), TIMER_UPDATE, UPDATE_FIRST_CHECK_MS, None);
    appbar(hwnd, ABM_NEW);
    Ok(hwnd)
}

/// Registered as an appbar (without reserving any screen space) only to get ABN_FULLSCREENAPP the
/// moment an app goes fullscreen or leaves it, e.g. a browser video, which resizes its window
/// without a foreground change.
unsafe fn appbar(hwnd: HWND, msg: u32) {
    let mut abd = APPBARDATA {
        cbSize: size_of::<APPBARDATA>() as u32,
        hWnd: hwnd,
        uCallbackMessage: WM_APP_APPBAR,
        ..Default::default()
    };
    SHAppBarMessage(msg, &mut abd);
}

unsafe fn quit(hwnd: HWND) {
    QUITTING.store(true, Ordering::SeqCst);
    let _ = DestroyWindow(hwnd);
}

fn latest() -> Option<String> {
    LATEST.lock().ok()?.clone()
}

/// Owner for menus and message boxes. Windows refuses to open them (access denied) from a window in
/// band 6, so they hang off the detail panel, which stays in the ordinary band.
fn host() -> HWND {
    crate::POPUP.get()
}

fn message(text: &str, flags: MESSAGEBOX_STYLE) -> MESSAGEBOX_RESULT {
    unsafe { MessageBoxW(Some(host()), &HSTRING::from(text), w!("Taskbar Monitor"), flags) }
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
    let v = update::VERSION;
    match code {
        1 => {
            let tag = latest().unwrap_or_default();
            let text = tr!(
                format!("Version {tag} is available (current v{v}).\n\nUpdate now?"),
                format!("Có bản mới {tag} (đang dùng v{v}).\n\nCập nhật ngay?"),
            );
            if message(&text, MB_YESNO | MB_ICONQUESTION) == IDYES {
                apply_async(hwnd, tag);
            }
        }
        0 => {
            let text = tr!(
                format!("You're on the latest version (v{v})."),
                format!("Bạn đang dùng bản mới nhất (v{v})."),
            );
            message(&text, MB_OK | MB_ICONINFORMATION);
        }
        _ => {
            let e = LAST_ERROR.lock().map(|e| e.clone()).unwrap_or_default();
            let text = tr!(
                format!("Couldn't check for updates.\n\n{e}"),
                format!("Không kiểm tra được cập nhật.\n\n{e}"),
            );
            message(&text, MB_OK | MB_ICONWARNING);
        }
    }
}

unsafe fn on_applied(hwnd: HWND, ok: bool) {
    UPDATING.store(false, Ordering::SeqCst);
    if !ok {
        let e = LAST_ERROR.lock().map(|e| e.clone()).unwrap_or_default();
        let text = tr!(format!("Update failed.\n\n{e}"), format!("Cập nhật thất bại.\n\n{e}"));
        message(&text, MB_OK | MB_ICONWARNING);
        return;
    }
    // The exe has been replaced: start the new version (it waits for this one to exit before taking the single-instance lock)
    if let Some(exe) = update::exe_path() {
        let _ = std::process::Command::new(exe).arg("--after-update").spawn();
    }
    quit(hwnd);
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
    let n = (0..LABELS.len()).filter(|&i| shown(i)).count() as i32;
    let width = 2 * pad
        + (0..LABELS.len()).filter(|&i| shown(i)).map(|i| label_w[i]).sum::<i32>()
        + n * (gap_label + value_w)
        + (n - 1) * gap_item;
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
///
/// Only moves in the z-order when the taskbar has covered the bar, and then lands directly above
/// the taskbar rather than at the very top, so it never jumps over a shell flyout.
pub fn update_position(hwnd: HWND) {
    unsafe {
        let visible = IsWindowVisible(hwnd).as_bool();
        let Some((tray, (x, y, w, h))) = target_rect() else {
            if visible {
                let _ = ShowWindow(hwnd, SW_HIDE);
            }
            return;
        };
        let mut flags = SWP_NOACTIVATE;
        if !visible {
            flags |= SWP_SHOWWINDOW;
        }
        let after = if !visible || is_above(tray, hwnd) {
            match GetWindow(tray, GW_HWNDPREV) {
                Ok(p) if p != hwnd => p,
                _ => HWND_TOPMOST,
            }
        } else {
            flags |= SWP_NOZORDER;
            let mut r = RECT::default();
            let _ = GetWindowRect(hwnd, &mut r);
            if (r.left, r.top, r.right - r.left, r.bottom - r.top) == (x, y, w, h) {
                return;
            }
            HWND::default()
        };
        let _ = SetWindowPos(hwnd, Some(after), x, y, w, h, flags);
    }
}

/// Some top-level window changed z-order (fires often): only do the full update when the taskbar
/// has just come over the bar.
pub fn on_reorder(hwnd: HWND) {
    unsafe {
        if let Ok(tray) = FindWindowW(w!("Shell_TrayWnd"), PCWSTR::null()) {
            if IsWindowVisible(hwnd).as_bool() && is_above(tray, hwnd) {
                update_position(hwnd);
            }
        }
    }
}

/// The bar is only lifted with the taskbar while the taskbar owns it. If the shell dropped the
/// ownership without lifting it (the bar was created while the taskbar was already up in band 6),
/// recreate it, owned again, once the taskbar is back down.
pub fn ensure_lifted(hwnd: HWND) {
    unsafe {
        let Ok(tray) = FindWindowW(w!("Shell_TrayWnd"), PCWSTR::null()) else { return };
        if band(hwnd) == Some(1) && band(tray) == Some(1) && GetWindow(hwnd, GW_OWNER).is_err() {
            // WM_DESTROY asks the UI thread to create a new one
            let _ = DestroyWindow(hwnd);
        }
    }
}

/// The z-band a window is in (undocumented user32 GetWindowBand). 1 is the ordinary desktop band.
fn band(hwnd: HWND) -> Option<u32> {
    type GetWindowBand = unsafe extern "system" fn(HWND, *mut u32) -> BOOL;
    static PROC: OnceLock<Option<GetWindowBand>> = OnceLock::new();
    let f = (*PROC.get_or_init(|| unsafe {
        let user32 = GetModuleHandleW(w!("user32.dll")).ok()?;
        GetProcAddress(user32, s!("GetWindowBand")).map(|p| std::mem::transmute::<_, GetWindowBand>(p))
    }))?;
    let mut b = 0;
    unsafe { f(hwnd, &mut b).as_bool().then_some(b) }
}

/// Whether `a` is above `b` in the z-order.
unsafe fn is_above(a: HWND, b: HWND) -> bool {
    let mut h = b;
    while let Ok(p) = GetWindow(h, GW_HWNDPREV) {
        if p == a {
            return true;
        }
        h = p;
    }
    false
}

unsafe fn target_rect() -> Option<(HWND, (i32, i32, i32, i32))> {
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
    let th = tr.bottom - tr.top;
    let h = (th - px(12.0, scale)).max(px(24.0, scale)).min(th);
    let y = tr.top + (th - h) / 2;
    if on_left() {
        // The Widgets button (on unless turned off) takes the left edge of a centered taskbar
        let widgets = read_dword(EXPLORER_KEY, w!("TaskbarDa")) != Some(0);
        let x = tr.left + px(if widgets { 220.0 } else { 8.0 }, scale);
        return Some((tray, (x, y, l.width, h)));
    }
    let right = match FindWindowExW(Some(tray), None, w!("TrayNotifyWnd"), PCWSTR::null()) {
        Ok(n) => {
            let mut r = RECT::default();
            let _ = GetWindowRect(n, &mut r);
            r.left
        }
        Err(_) => tr.right - px(300.0, scale),
    };
    Some((tray, (right - px(8.0, scale) - l.width, y, l.width, h)))
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
    for (i, label) in LABELS.iter().enumerate().filter(|&(i, _)| shown(i)) {
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
    popup::close();
    let Ok(menu) = CreatePopupMenu() else { return };
    let latest = latest();
    let v = update::VERSION;
    let update_label = match &latest {
        Some(tag) => tr!(
            format!("Update to {tag} (current v{v})"),
            format!("Cập nhật lên {tag} (đang dùng v{v})"),
        ),
        None => tr!(format!("Check for updates (v{v})"), format!("Kiểm tra cập nhật (v{v})")),
    };
    let item = |m: HMENU, flags: MENU_ITEM_FLAGS, id: usize, text: &str| {
        let _ = AppendMenuW(m, flags, id, &HSTRING::from(text));
    };
    item(menu, MF_STRING, ID_UPDATE, &update_label);
    item(menu, MF_SEPARATOR, 0, "");
    let check = |on: bool| if on { MF_CHECKED } else { MF_UNCHECKED };
    if let Ok(sub) = CreatePopupMenu() {
        let last = SHOWN.get().count_ones() == 1;
        for (i, label) in LABELS.iter().enumerate() {
            // The last metric left can't be turned off, or the bar would be empty
            let grayed = if last && shown(i) { MF_GRAYED } else { MF_ENABLED };
            item(sub, MF_STRING | check(shown(i)) | grayed, ID_SHOW + i, label);
        }
        item(menu, MF_POPUP, sub.0 as usize, tr!("Show on taskbar", "Hiển thị trên taskbar"));
    }
    if let Ok(sub) = CreatePopupMenu() {
        let left = on_left();
        let left_ok = if taskbar_centered() { MF_ENABLED } else { MF_GRAYED };
        item(sub, MF_STRING | check(!left), ID_POS_RIGHT, tr!("Right, next to the notification area", "Bên phải, cạnh khay hệ thống"));
        item(sub, MF_STRING | check(left) | left_ok, ID_POS_LEFT, tr!("Left edge of the taskbar", "Mép trái taskbar"));
        item(menu, MF_POPUP, sub.0 as usize, tr!("Position", "Vị trí"));
    }
    item(menu, MF_STRING | check(autostart::enabled()), ID_AUTOSTART, tr!("Start with Windows", "Khởi động cùng Windows"));
    if let Ok(sub) = CreatePopupMenu() {
        // Language names stay in their own language so anyone can find theirs
        let current = i18n::choice();
        item(sub, MF_STRING | check(current == Choice::Auto), ID_LANG_AUTO, tr!("Automatic (Windows)", "Tự động (theo Windows)"));
        item(sub, MF_STRING | check(current == Choice::En), ID_LANG_EN, "English");
        item(sub, MF_STRING | check(current == Choice::Vi), ID_LANG_VI, "Tiếng Việt");
        item(menu, MF_POPUP, sub.0 as usize, tr!("Language", "Ngôn ngữ"));
    }
    item(menu, MF_SEPARATOR, 0, "");
    item(menu, MF_STRING, ID_EXIT, tr!("Exit", "Thoát"));
    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    // Open above the bar: the menu is in the ordinary band, so the taskbar may be drawn over it
    let mut r = RECT::default();
    let _ = GetWindowRect(hwnd, &mut r);
    // Must be foreground or the menu won't close when clicking elsewhere
    let _ = SetForegroundWindow(host());
    let cmd = TrackPopupMenu(
        menu,
        TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN | TPM_RIGHTALIGN,
        pt.x,
        r.top,
        None,
        host(),
        None,
    );
    let _ = DestroyMenu(menu);
    match cmd.0 as usize {
        ID_UPDATE => match latest {
            Some(tag) => apply_async(hwnd, tag),
            None => check_async(hwnd, true),
        },
        ID_AUTOSTART => autostart::set(!autostart::enabled()),
        id if (ID_SHOW..ID_SHOW + LABELS.len()).contains(&id) => {
            set_shown(SHOWN.get() ^ (1 << (id - ID_SHOW)));
            update_position(hwnd);
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
        id @ (ID_POS_RIGHT | ID_POS_LEFT) => {
            LEFT.set(id == ID_POS_LEFT);
            write_setting(POSITION_VALUE, LEFT.get() as u32);
            update_position(hwnd);
        }
        id @ (ID_LANG_AUTO | ID_LANG_EN | ID_LANG_VI) => {
            i18n::set_choice(match id {
                ID_LANG_EN => Choice::En,
                ID_LANG_VI => Choice::Vi,
                _ => Choice::Auto,
            });
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
        ID_EXIT => quit(hwnd),
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
        WM_APP_APPBAR if wp.0 as u32 == ABN_FULLSCREENAPP => update_position(hwnd),
        WM_DESTROY => {
            appbar(hwnd, ABM_REMOVE);
            if QUITTING.load(Ordering::SeqCst) {
                PostQuitMessage(0);
            } else {
                let _ = PostThreadMessageW(GetCurrentThreadId(), WM_APP_RECREATE, WPARAM(0), LPARAM(0));
            }
        }
        m if m == TASKBAR_CREATED.get() => {
            // A new Explorer has forgotten the appbar registration
            appbar(hwnd, ABM_REMOVE);
            appbar(hwnd, ABM_NEW);
            update_position(hwnd);
        }
        _ => return DefWindowProcW(hwnd, msg, wp, lp),
    }
    LRESULT(0)
}
