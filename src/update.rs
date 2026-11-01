//! Self-update via GitHub Releases: ask the API for the latest release, download the exe, replace the running file, restart.
//!
//! Windows allows renaming a running exe but not overwriting it, so: download to `.exe.new`,
//! rename the current exe to `.exe.old`, rename `.exe.new` to the original name. `.old` is deleted on the next start.

use std::ffi::c_void;
use std::path::PathBuf;

use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Networking::WinHttp::*;

pub const REPO: &str = "kieusonlam/taskbar-monitor";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const ASSET: &str = "taskbar-monitor.exe";

/// Tag of the latest release, e.g. "v0.2.0".
pub fn latest_tag() -> Result<String, String> {
    let body = get("api.github.com", &format!("/repos/{REPO}/releases/latest"))?;
    json_str(&String::from_utf8_lossy(&body), "tag_name").ok_or_else(|| tr!("GitHub response has no tag_name", "Phản hồi GitHub không có tag_name").into())
}

pub fn is_newer(tag: &str) -> bool {
    parse(tag) > parse(VERSION)
}

fn parse(v: &str) -> (u32, u32, u32) {
    let mut it = v.trim_start_matches('v').split(['.', '-']).map(|p| p.parse().unwrap_or(0));
    (it.next().unwrap_or(0), it.next().unwrap_or(0), it.next().unwrap_or(0))
}

/// Download the exe for `tag` and put it in place of the running exe. The current process keeps running the old version until restarted.
pub fn apply(tag: &str) -> Result<(), String> {
    let data = get("github.com", &format!("/{REPO}/releases/download/{tag}/{ASSET}"))?;
    if data.len() < 64 * 1024 || !data.starts_with(b"MZ") {
        return Err(tr!("The download is not a valid exe", "File tải về không phải exe hợp lệ").into());
    }
    let exe = exe_path().ok_or(tr!("Couldn't determine the exe path", "Không xác định được đường dẫn exe"))?;
    let new = exe.with_extension("exe.new");
    let old = exe.with_extension("exe.old");
    std::fs::write(&new, &data).map_err(|e| tr!(format!("Couldn't write {}: {e}", new.display()), format!("Không ghi được {}: {e}", new.display())))?;
    let _ = std::fs::remove_file(&old);
    std::fs::rename(&exe, &old).map_err(|e| tr!(format!("Couldn't rename the current exe: {e}"), format!("Không đổi tên được exe hiện tại: {e}")))?;
    if let Err(e) = std::fs::rename(&new, &exe) {
        let _ = std::fs::rename(&old, &exe);
        return Err(tr!(format!("Couldn't replace the exe: {e}"), format!("Không thay được exe: {e}")));
    }
    Ok(())
}

/// Delete the `.exe.old` left over from a previous update.
pub fn cleanup() {
    if let Some(exe) = exe_path() {
        let _ = std::fs::remove_file(exe.with_extension("exe.old"));
    }
}

/// Real path of the exe. winget installs portable apps and creates a symlink to run them from a terminal;
/// when launched through it, `current_exe` returns the symlink, and replacing that would break the install.
pub fn exe_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let real = std::fs::canonicalize(&exe).unwrap_or(exe);
    // canonicalize returns \\?\C:\...; strip the prefix for the registry and for display
    let s = real.to_string_lossy();
    match s.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with(r"UNC\") => Some(PathBuf::from(rest)),
        _ => Some(real),
    }
}

/// First string value of `key` in a JSON document. Enough for tag_name without a JSON library.
fn json_str(json: &str, key: &str) -> Option<String> {
    let after = &json[json.find(&format!("\"{key}\""))? + key.len() + 2..];
    let after = after.trim_start().strip_prefix(':')?.trim_start().strip_prefix('"')?;
    Some(after[..after.find('"')?].to_string())
}

struct Handle(*mut c_void);

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = WinHttpCloseHandle(self.0);
            }
        }
    }
}

fn open(h: *mut c_void, what: &str) -> Result<Handle, String> {
    if h.is_null() {
        Err(format!("{what}: {}", windows::core::Error::from_thread().message()))
    } else {
        Ok(Handle(h))
    }
}

/// HTTPS GET, following redirects (GitHub asset downloads redirect to a CDN).
fn get(host: &str, path: &str) -> Result<Vec<u8>, String> {
    let err = |what: &str, e: windows::core::Error| format!("{what}: {}", e.message());
    unsafe {
        let agent = HSTRING::from(format!("taskbar-monitor/{VERSION}"));
        let session = open(
            WinHttpOpen(&agent, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, PCWSTR::null(), PCWSTR::null(), 0),
            "WinHttpOpen",
        )?;
        let _ = WinHttpSetTimeouts(session.0, 10_000, 10_000, 15_000, 30_000);
        let conn = open(
            WinHttpConnect(session.0, &HSTRING::from(host), INTERNET_DEFAULT_HTTPS_PORT, 0),
            tr!("Connect", "Kết nối"),
        )?;
        let req = open(
            WinHttpOpenRequest(
                conn.0,
                w!("GET"),
                &HSTRING::from(path),
                PCWSTR::null(),
                PCWSTR::null(),
                std::ptr::null(),
                WINHTTP_FLAG_SECURE,
            ),
            tr!("Open request", "Tạo request"),
        )?;
        WinHttpSendRequest(req.0, None, None, 0, 0, 0).map_err(|e| err(tr!("Send request", "Gửi request"), e))?;
        WinHttpReceiveResponse(req.0, std::ptr::null_mut()).map_err(|e| err(tr!("Receive response", "Nhận phản hồi"), e))?;

        let mut status = 0u32;
        let mut len = size_of::<u32>() as u32;
        WinHttpQueryHeaders(
            req.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some(&mut status as *mut u32 as *mut c_void),
            &mut len,
            std::ptr::null_mut(),
        )
        .map_err(|e| err(tr!("Read status code", "Đọc mã trạng thái"), e))?;
        if status != 200 {
            return Err(tr!(format!("GitHub returned HTTP {status}"), format!("GitHub trả về HTTP {status}")));
        }

        let mut out = Vec::new();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let mut read = 0u32;
            WinHttpReadData(req.0, buf.as_mut_ptr() as *mut c_void, buf.len() as u32, &mut read)
                .map_err(|e| err(tr!("Read data", "Đọc dữ liệu"), e))?;
            if read == 0 {
                break;
            }
            out.extend_from_slice(&buf[..read as usize]);
        }
        Ok(out)
    }
}
