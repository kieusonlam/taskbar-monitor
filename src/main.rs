#![windows_subsystem = "windows"]

#[macro_use]
mod i18n;

mod autostart;
mod gdi;
mod metrics;
mod overlay;
mod package;
mod popup;
mod update;

use std::cell::{Cell, RefCell};
use std::time::Duration;

use windows::core::{w, Result};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::InvalidateRect;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::WindowsAndMessaging::*;

use metrics::Monitor;

// The whole app runs on a single UI thread, so state lives in thread_locals.
thread_local! {
    static MONITOR: RefCell<Option<Monitor>> = const { RefCell::new(None) };
    pub static OVERLAY: Cell<HWND> = Cell::new(HWND::default());
    pub static POPUP: Cell<HWND> = Cell::new(HWND::default());
}

/// Read the metrics state. Returns None if already borrowed (never happens on the UI thread, but avoids a panic).
pub fn with_monitor<T>(f: impl FnOnce(&Monitor) -> T) -> Option<T> {
    MONITOR.with(|m| m.try_borrow().ok().and_then(|m| m.as_ref().map(f)))
}

/// Every second: sample, reposition the bar, repaint.
pub fn tick() {
    MONITOR.with(|m| {
        if let Ok(mut m) = m.try_borrow_mut() {
            if let Some(m) = m.as_mut() {
                m.tick();
            }
        }
    });
    let ov = OVERLAY.get();
    overlay::update_position(ov);
    unsafe {
        let _ = InvalidateRect(Some(ov), None, false);
    }
    popup::refresh();
    overlay::ensure_lifted(ov);
}

/// The taskbar reclaims the top of the z-order on foreground changes, and also on its own (e.g. when
/// Quick Settings closes). Top-level z-order changes raise EVENT_OBJECT_REORDER on the desktop
/// window, so react to those as well as to foreground changes.
unsafe extern "system" fn on_win_event(_: HWINEVENTHOOK, event: u32, hwnd: HWND, _: i32, _: i32, _: u32, _: u32) {
    if event == EVENT_SYSTEM_FOREGROUND {
        overlay::update_position(OVERLAY.get());
    } else if hwnd == GetDesktopWindow() {
        overlay::on_reorder(OVERLAY.get());
    }
}

/// `taskbar-monitor.exe --update`: headless update (for scripts and mass deployment).
/// Exit code: 0 = already up to date, 10 = updated, 1 = error.
fn update_cli() -> ! {
    let code = match update::latest_tag() {
        Ok(tag) if update::is_newer(&tag) => match update::apply(&tag) {
            Ok(()) => 10,
            Err(_) => 1,
        },
        Ok(_) => 0,
        Err(_) => 1,
    };
    std::process::exit(code)
}

/// Hold the single-instance lock. After a self-update the new version waits for the old one to exit (up to 10 s).
unsafe fn single_instance(wait: bool) -> Option<HANDLE> {
    for _ in 0..50 {
        let h = CreateMutexW(None, true, w!("Local\\TaskbarMonitor.SingleInstance")).ok()?;
        if GetLastError() != ERROR_ALREADY_EXISTS {
            return Some(h);
        }
        let _ = CloseHandle(h);
        if !wait {
            return None;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    None
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--update") {
        if package::packaged() {
            // The Store updates the package
            std::process::exit(0);
        }
        update_cli();
    }
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);

        let Some(_instance) = single_instance(args.iter().any(|a| a == "--after-update")) else {
            return Ok(());
        };
        if !package::packaged() {
            update::cleanup();
        }
        i18n::init();

        let mut monitor = Monitor::new();
        monitor.tick();
        MONITOR.with(|m| *m.borrow_mut() = Some(monitor));

        let hinst: HINSTANCE = GetModuleHandleW(None)?.into();
        POPUP.set(popup::create(hinst)?);
        OVERLAY.set(overlay::create(hinst)?);
        overlay::update_position(OVERLAY.get());

        let hooks = [EVENT_SYSTEM_FOREGROUND, EVENT_OBJECT_REORDER]
            .map(|e| SetWinEventHook(e, e, None, Some(on_win_event), 0, 0, WINEVENT_OUTOFCONTEXT));

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if msg.hwnd.is_invalid() && msg.message == overlay::WM_APP_RECREATE {
                // The bar went away with Explorer, or was dropped to be owned by the taskbar again
                match overlay::create(hinst) {
                    Ok(h) => {
                        OVERLAY.set(h);
                        overlay::update_position(h);
                    }
                    Err(_) => break,
                }
                continue;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        for h in hooks {
            let _ = UnhookWinEvent(h);
        }
    }
    Ok(())
}
