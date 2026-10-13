//! "Run at Windows startup": HKCU\Software\Microsoft\Windows\CurrentVersion\Run, value "SirenBar",
//! or the package's StartupTask when installed from the Store (the Run key is virtualized there).

use crate::package;
use windows::core::{w, HSTRING};
use windows::ApplicationModel::{StartupTask, StartupTaskState};
use windows::Win32::System::Registry::*;

fn startup_task() -> Option<StartupTask> {
    StartupTask::GetAsync(&HSTRING::from(package::STARTUP_TASK)).ok()?.join().ok()
}

pub fn is_enabled() -> bool {
    if package::is_packaged() {
        return startup_task()
            .and_then(|t| t.State().ok())
            .is_some_and(|s| s == StartupTaskState::Enabled || s == StartupTaskState::EnabledByPolicy);
    }
    let mut size = 0u32;
    unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run"),
            w!("SirenBar"),
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
    if package::is_packaged() {
        // If the user turned it off in Task Manager / Settings, Windows keeps it off
        // (DisabledByUser) and RequestEnableAsync just reports that.
        if let Some(t) = startup_task() {
            if on {
                let _ = t.RequestEnableAsync().and_then(|op| op.join());
            } else {
                let _ = t.Disable();
            }
        }
        return;
    }
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
                        w!("SirenBar"),
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
                w!("SirenBar"),
            );
        }
    }
}

/// The app used to be called Alertss: drop its autostart value and its notification id,
/// and keep autostart on under the new name if the old one had it.
pub fn migrate_old() {
    if package::is_packaged() {
        return;
    }
    unsafe {
        let mut size = 0u32;
        let had = RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run"),
            w!("Alertss"),
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut size),
        )
        .is_ok();
        if had {
            let _ = RegDeleteKeyValueW(
                HKEY_CURRENT_USER,
                w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run"),
                w!("Alertss"),
            );
            set_enabled(true);
        }
        let _ = RegDeleteTreeW(HKEY_CURRENT_USER, w!("Software\\Classes\\AppUserModelId\\Alertss.Widget"));
    }
}
