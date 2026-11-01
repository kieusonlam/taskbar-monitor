//! UI language. Follows the Windows display language unless overridden from the menu;
//! the override is stored in HKCU\Software\TaskbarMonitor\Language ("en" or "vi").

use std::sync::atomic::{AtomicU8, Ordering};

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::Globalization::GetUserDefaultUILanguage;
use windows::Win32::System::Registry::*;

/// `tr!("English", "Tiếng Việt")`: pick the string for the current UI language.
macro_rules! tr {
    ($en:expr, $vi:expr $(,)?) => {
        match $crate::i18n::lang() {
            $crate::i18n::Lang::En => $en,
            $crate::i18n::Lang::Vi => $vi,
        }
    };
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    En,
    Vi,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Auto,
    En,
    Vi,
}

const KEY: PCWSTR = w!("Software\\TaskbarMonitor");
const VALUE: PCWSTR = w!("Language");
const LANG_VIETNAMESE: u16 = 0x2a;

static CURRENT: AtomicU8 = AtomicU8::new(0);

pub fn lang() -> Lang {
    if CURRENT.load(Ordering::Relaxed) == 1 {
        Lang::Vi
    } else {
        Lang::En
    }
}

pub fn init() {
    apply(choice());
}

fn apply(c: Choice) {
    let l = match c {
        Choice::En => Lang::En,
        Choice::Vi => Lang::Vi,
        Choice::Auto => system(),
    };
    CURRENT.store((l == Lang::Vi) as u8, Ordering::Relaxed);
}

fn system() -> Lang {
    // Low 10 bits of a LANGID are the primary language
    if unsafe { GetUserDefaultUILanguage() } & 0x3ff == LANG_VIETNAMESE {
        Lang::Vi
    } else {
        Lang::En
    }
}

pub fn choice() -> Choice {
    let mut buf = [0u16; 8];
    let mut size = (buf.len() * 2) as u32;
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            KEY,
            VALUE,
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as _),
            Some(&mut size),
        )
    };
    if r != ERROR_SUCCESS {
        return Choice::Auto;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    match String::from_utf16_lossy(&buf[..len]).as_str() {
        "en" => Choice::En,
        "vi" => Choice::Vi,
        _ => Choice::Auto,
    }
}

pub fn set_choice(c: Choice) {
    unsafe {
        match c {
            Choice::Auto => {
                let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, KEY, VALUE);
            }
            Choice::En | Choice::Vi => {
                let v: Vec<u16> = if c == Choice::En { "en" } else { "vi" }.encode_utf16().chain([0]).collect();
                let _ = RegSetKeyValueW(
                    HKEY_CURRENT_USER,
                    KEY,
                    VALUE,
                    REG_SZ.0,
                    Some(v.as_ptr() as _),
                    (v.len() * 2) as u32,
                );
            }
        }
    }
    apply(c);
}
