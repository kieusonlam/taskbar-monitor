//! Tự cập nhật qua GitHub Releases: hỏi API bản mới nhất, tải exe, thay file đang chạy, khởi động lại.
//!
//! Windows cho đổi tên exe đang chạy nhưng không cho ghi đè, nên: tải về `.exe.new`,
//! đổi exe hiện tại thành `.exe.old`, đổi `.exe.new` thành tên gốc. File `.old` bị xóa ở lần chạy sau.

use std::ffi::c_void;
use std::path::PathBuf;

use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Networking::WinHttp::*;

pub const REPO: &str = "kieusonlam/taskbar-monitor";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const ASSET: &str = "taskbar-monitor.exe";

/// Tag của bản phát hành mới nhất, ví dụ "v0.2.0".
pub fn latest_tag() -> Result<String, String> {
    let body = get("api.github.com", &format!("/repos/{REPO}/releases/latest"))?;
    json_str(&String::from_utf8_lossy(&body), "tag_name").ok_or_else(|| "Phản hồi GitHub không có tag_name".into())
}

pub fn is_newer(tag: &str) -> bool {
    parse(tag) > parse(VERSION)
}

fn parse(v: &str) -> (u32, u32, u32) {
    let mut it = v.trim_start_matches('v').split(['.', '-']).map(|p| p.parse().unwrap_or(0));
    (it.next().unwrap_or(0), it.next().unwrap_or(0), it.next().unwrap_or(0))
}

/// Tải exe của `tag` và thay vào chỗ exe đang chạy. Tiến trình hiện tại vẫn chạy bản cũ cho tới khi khởi động lại.
pub fn apply(tag: &str) -> Result<(), String> {
    let data = get("github.com", &format!("/{REPO}/releases/download/{tag}/{ASSET}"))?;
    if data.len() < 64 * 1024 || !data.starts_with(b"MZ") {
        return Err("File tải về không phải exe hợp lệ".into());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let new = exe.with_extension("exe.new");
    let old = exe.with_extension("exe.old");
    std::fs::write(&new, &data).map_err(|e| format!("Không ghi được {}: {e}", new.display()))?;
    let _ = std::fs::remove_file(&old);
    std::fs::rename(&exe, &old).map_err(|e| format!("Không đổi tên được exe hiện tại: {e}"))?;
    if let Err(e) = std::fs::rename(&new, &exe) {
        let _ = std::fs::rename(&old, &exe);
        return Err(format!("Không thay được exe: {e}"));
    }
    Ok(())
}

/// Xóa `.exe.old` còn sót từ lần cập nhật trước.
pub fn cleanup() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::fs::remove_file(exe.with_extension("exe.old"));
    }
}

pub fn exe_path() -> Option<PathBuf> {
    std::env::current_exe().ok()
}

/// Lấy giá trị chuỗi đầu tiên của `key` trong JSON. Đủ cho tag_name, không cần thư viện JSON.
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

/// HTTPS GET, tự theo redirect (link tải asset của GitHub chuyển hướng sang CDN).
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
            "Kết nối",
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
            "Tạo request",
        )?;
        WinHttpSendRequest(req.0, None, None, 0, 0, 0).map_err(|e| err("Gửi request", e))?;
        WinHttpReceiveResponse(req.0, std::ptr::null_mut()).map_err(|e| err("Nhận phản hồi", e))?;

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
        .map_err(|e| err("Đọc mã trạng thái", e))?;
        if status != 200 {
            return Err(format!("GitHub trả về HTTP {status}"));
        }

        let mut out = Vec::new();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let mut read = 0u32;
            WinHttpReadData(req.0, buf.as_mut_ptr() as *mut c_void, buf.len() as u32, &mut read)
                .map_err(|e| err("Đọc dữ liệu", e))?;
            if read == 0 {
                break;
            }
            out.extend_from_slice(&buf[..read as usize]);
        }
        Ok(out)
    }
}
