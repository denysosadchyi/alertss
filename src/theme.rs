//! System theme detection.

use windows::core::w;
use windows::Win32::System::Registry::*;

/// Taskbar/system theme: HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize
/// "SystemUsesLightTheme" (DWORD, missing -> false).
pub fn taskbar_is_light() -> bool {
    read_dword(w!("SystemUsesLightTheme"), false)
}

/// Apps theme: same key, "AppsUseLightTheme" (missing -> true).
pub fn apps_are_light() -> bool {
    read_dword(w!("AppsUseLightTheme"), true)
}

fn read_dword(name: windows::core::PCWSTR, default: bool) -> bool {
    let mut value: u32 = 0;
    let mut size = std::mem::size_of::<u32>() as u32;

    unsafe {
        let result = RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            name,
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut _ as *mut _),
            Some(&mut size),
        );

        if result.is_ok() {
            value != 0
        } else {
            default
        }
    }
}
