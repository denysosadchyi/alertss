//! Running from the Microsoft Store (MSIX) package or as a plain exe.
//!
//! Inside a package Windows virtualizes %APPDATA% and HKCU\...\Run, gives the process its own
//! notification identity and only allows autostart through a StartupTask, so a few modules
//! branch on `is_packaged()`.

use std::path::PathBuf;
use std::sync::OnceLock;
use windows::Storage::ApplicationData;
use windows::Win32::Foundation::{APPMODEL_ERROR_NO_PACKAGE, ERROR_INSUFFICIENT_BUFFER};
use windows::Win32::Storage::Packaging::Appx::GetCurrentPackageFullName;

/// StartupTask id declared in AppxManifest.xml.
pub const STARTUP_TASK: &str = "SirenBarStartup";

pub fn is_packaged() -> bool {
    static P: OnceLock<bool> = OnceLock::new();
    *P.get_or_init(|| {
        let mut len = 0u32;
        let rc = unsafe { GetCurrentPackageFullName(&mut len, None) };
        rc != APPMODEL_ERROR_NO_PACKAGE && (rc == ERROR_INSUFFICIENT_BUFFER || rc.is_ok())
    })
}

/// The package's real LocalState folder (not virtualized, so Notepad sees the same file).
pub fn local_state() -> Option<PathBuf> {
    let folder = ApplicationData::Current().ok()?.LocalFolder().ok()?;
    Some(PathBuf::from(folder.Path().ok()?.to_string_lossy()))
}
