//! Windows toast notifications for an unpackaged app.

use windows::core::{w, HSTRING};
use windows::Data::Xml::Dom::XmlDocument;
use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};
use windows::Win32::System::Registry::*;
use windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;

const DISPLAY_NAME: &str = "Тривоги";

/// Register AUMID "Alertss.Widget" under HKCU\Software\Classes\AppUserModelId\Alertss.Widget
/// (DisplayName = "Тривоги") and call SetCurrentProcessExplicitAppUserModelID. Errors ignored.
pub fn init() {
    unsafe {
        // Write DisplayName value
        let display_name_utf16: Vec<u16> = DISPLAY_NAME.encode_utf16().chain(std::iter::once(0)).collect();
        let _ = RegSetKeyValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Classes\\AppUserModelId\\Alertss.Widget"),
            w!("DisplayName"),
            REG_SZ.0,
            Some(display_name_utf16.as_ptr() as *const _),
            (display_name_utf16.len() * std::mem::size_of::<u16>()) as u32,
        );
    }

    // Set the AUMID for the process
    let _ = unsafe { SetCurrentProcessExplicitAppUserModelID(w!("Alertss.Widget")) };
}

/// Show a toast. `sound` = false -> silent. Must not panic; errors ignored.
pub fn show(title: &str, body: &str, sound: bool) {
    // Wrap everything in a catch-all to never panic
    let _ = (|| -> windows::core::Result<()> {
        // Escape XML special characters
        let title_escaped = escape_xml(title);
        let body_escaped = escape_xml(body);

        let audio_xml = if sound {
            r#"<audio src="ms-winsoundevent:Notification.Reminder"/>"#
        } else {
            r#"<audio silent="true"/>"#
        };

        let toast_xml = format!(
            r#"<toast><visual><binding template="ToastGeneric"><text>{}</text><text>{}</text></binding></visual>{}</toast>"#,
            title_escaped, body_escaped, audio_xml
        );

        let xml_doc = XmlDocument::new()?;
        xml_doc.LoadXml(&HSTRING::from(&toast_xml))?;

        let toast = ToastNotification::CreateToastNotification(&xml_doc)?;
        let notifier = ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from("Alertss.Widget"))?;
        notifier.Show(&toast)?;

        Ok(())
    })();
}

/// Escape XML special characters
fn escape_xml(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '&' => "&amp;".to_string(),
            '<' => "&lt;".to_string(),
            '>' => "&gt;".to_string(),
            '"' => "&quot;".to_string(),
            '\'' => "&apos;".to_string(),
            c => c.to_string(),
        })
        .collect()
}
