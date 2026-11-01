#![windows_subsystem = "windows"]

mod autostart;
mod gdi;
mod metrics;
mod overlay;
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
}

/// The taskbar tends to reclaim the top of the z-order when the foreground window changes: push the bar back up at once.
unsafe extern "system" fn on_foreground(_: HWINEVENTHOOK, _: u32, _: HWND, _: i32, _: i32, _: u32, _: u32) {
    overlay::update_position(OVERLAY.get());
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
        update_cli();
    }
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);

        let Some(_instance) = single_instance(args.iter().any(|a| a == "--after-update")) else {
            return Ok(());
        };
        update::cleanup();

        let mut monitor = Monitor::new();
        monitor.tick();
        MONITOR.with(|m| *m.borrow_mut() = Some(monitor));

        let hinst: HINSTANCE = GetModuleHandleW(None)?.into();
        POPUP.set(popup::create(hinst)?);
        OVERLAY.set(overlay::create(hinst)?);
        overlay::update_position(OVERLAY.get());

        let hook = SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            None,
            Some(on_foreground),
            0,
            0,
            WINEVENT_OUTOFCONTEXT,
        );

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        let _ = UnhookWinEvent(hook);
    }
    Ok(())
}
