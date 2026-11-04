//! Toggle start-with-Windows: via HKCU\...\Run (no admin rights needed), or through the package's
//! StartupTask when installed from the Store.

use windows::core::{w, HSTRING, PCWSTR};
use windows::ApplicationModel::{StartupTask, StartupTaskState};
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::*;

use crate::package::packaged;

const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const VALUE: PCWSTR = w!("TaskbarMonitor");
/// TaskId of the StartupTask in the package manifest.
const TASK_ID: &str = "TaskbarMonitor";

fn task() -> Option<StartupTask> {
    StartupTask::GetAsync(&HSTRING::from(TASK_ID)).ok()?.join().ok()
}

fn is_on(s: StartupTaskState) -> bool {
    s == StartupTaskState::Enabled || s == StartupTaskState::EnabledByPolicy
}

pub fn enabled() -> bool {
    if packaged() {
        return task().and_then(|t| t.State().ok()).is_some_and(is_on);
    }
    unsafe { RegGetValueW(HKEY_CURRENT_USER, RUN_KEY, VALUE, RRF_RT_REG_SZ, None, None, None) == ERROR_SUCCESS }
}

/// Returns false if Windows refused, e.g. the user turned the app off in Settings > Apps > Startup,
/// which only they can undo.
pub fn set(on: bool) -> bool {
    if packaged() {
        let Some(t) = task() else { return false };
        return if on {
            t.RequestEnableAsync().and_then(|op| op.join()).is_ok_and(is_on)
        } else {
            t.Disable().is_ok()
        };
    }
    unsafe {
        if !on {
            let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, VALUE);
            return true;
        }
        let Some(exe) = crate::update::exe_path() else { return false };
        let cmd: Vec<u16> = format!("\"{}\"", exe.display()).encode_utf16().chain([0]).collect();
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            RUN_KEY,
            VALUE,
            REG_SZ.0,
            Some(cmd.as_ptr() as _),
            (cmd.len() * 2) as u32,
        ) == ERROR_SUCCESS
    }
}
