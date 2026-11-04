//! Whether the app runs from its MSIX package (the Microsoft Store build).
//!
//! The same exe ships on GitHub, where it updates itself and starts with Windows through the Run
//! key. The Store updates packaged apps itself and doesn't allow self-updating, and a package
//! starts with Windows through the StartupTask declared in its manifest.

use std::sync::OnceLock;

use windows::Win32::Foundation::APPMODEL_ERROR_NO_PACKAGE;
use windows::Win32::Storage::Packaging::Appx::GetCurrentPackageFullName;

pub fn packaged() -> bool {
    static PACKAGED: OnceLock<bool> = OnceLock::new();
    *PACKAGED.get_or_init(|| {
        let mut len = 0u32;
        unsafe { GetCurrentPackageFullName(&mut len, None) != APPMODEL_ERROR_NO_PACKAGE }
    })
}
