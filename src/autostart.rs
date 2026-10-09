//! "Run at Windows startup" via HKCU\Software\Microsoft\Windows\CurrentVersion\Run, value "Alertss".

use windows::core::w;
use windows::Win32::System::Registry::*;

pub fn is_enabled() -> bool {
    let mut size = 0u32;
    unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run"),
            w!("Alertss"),
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut size),
        )
        .is_ok()
    }
}

/// Enable writes "\"<current exe path>\""; disable deletes the value. Errors ignored.
pub fn set_enabled(on: bool) {
    unsafe {
        if on {
            if let Ok(path) = std::env::current_exe() {
                // Convert to UTF-16 and quote it
                if let Some(path_str) = path.to_str() {
                    let quoted = format!("\"{}\"", path_str);
                    let utf16: Vec<u16> = quoted.encode_utf16().chain(std::iter::once(0)).collect();
                    let _ = RegSetKeyValueW(
                        HKEY_CURRENT_USER,
                        w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run"),
                        w!("Alertss"),
                        REG_SZ.0,
                        Some(utf16.as_ptr() as *const _),
                        (utf16.len() * std::mem::size_of::<u16>()) as u32,
                    );
                }
            }
        } else {
            let _ = RegDeleteKeyValueW(
                HKEY_CURRENT_USER,
                w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run"),
                w!("Alertss"),
            );
        }
    }
}
