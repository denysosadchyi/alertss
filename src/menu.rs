//! Right-click context menu of the widget.
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::Win32::UI::Shell::ShellExecuteW;

/// Show the context menu at the cursor, owned by the hidden main window `owner`.
pub fn show(owner: HWND) {
    // Copy state BEFORE calling TrackPopupMenu
    let (region, sound, animation, autostart_enabled) = crate::state::with_app(|app| {
        (app.config.region.clone(), app.config.sound, app.config.animation, crate::autostart::is_enabled())
    });
    let regions = region_list();

    unsafe {
        let menu = CreatePopupMenu().expect("CreatePopupMenu");

        // "Область" submenu
        let region_menu = build_region_menu(&regions, &region).expect("CreatePopupMenu for regions");

        let region_str = to_wide_string("Область");
        let _ = AppendMenuW(
            menu,
            MF_STRING | MF_POPUP,
            region_menu.0 as usize,
            PCWSTR(region_str.as_ptr()),
        );

        // Separator
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());

        // Sound
        let sound_str = to_wide_string("Звук сповіщень");
        let sound_flags = if sound { MF_STRING | MF_CHECKED } else { MF_STRING };
        let _ = AppendMenuW(menu, sound_flags, 1, PCWSTR(sound_str.as_ptr()));

        // Animation
        let animation_str = to_wide_string("Анімація під час тривоги");
        let animation_flags = if animation { MF_STRING | MF_CHECKED } else { MF_STRING };
        let _ = AppendMenuW(menu, animation_flags, 2, PCWSTR(animation_str.as_ptr()));

        // Autostart
        let autostart_str = to_wide_string("Запускати з Windows");
        let autostart_flags = if autostart_enabled { MF_STRING | MF_CHECKED } else { MF_STRING };
        let _ = AppendMenuW(menu, autostart_flags, 3, PCWSTR(autostart_str.as_ptr()));

        // Separator
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());

        // API token (opens the config file)
        let token_set = !crate::config::Config::load().api_token.trim().is_empty();
        let token_str = to_wide_string("Токен alerts.in.ua…");
        let token_flags = if token_set { MF_STRING | MF_CHECKED } else { MF_STRING };
        let _ = AppendMenuW(menu, token_flags, 6, PCWSTR(token_str.as_ptr()));

        // Open alerts map
        let map_str = to_wide_string("Відкрити карту тривог");
        let _ = AppendMenuW(menu, MF_STRING, 4, PCWSTR(map_str.as_ptr()));

        // Exit
        let exit_str = to_wide_string("Вийти");
        let _ = AppendMenuW(menu, MF_STRING, 5, PCWSTR(exit_str.as_ptr()));

        // Show menu at cursor
        let _ = SetForegroundWindow(owner);

        let mut cursor = std::mem::zeroed::<windows::Win32::Foundation::POINT>();
        let _ = GetCursorPos(&mut cursor);

        let result = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN,
            cursor.x,
            cursor.y,
            Some(0),
            owner,
            None,
        );

        // Clean up menu
        let _ = DestroyMenu(menu);

        // NOW safe to mutate state
        // result is BOOL, convert to i32 for comparison
        let cmd = result.0 as usize;
        if cmd > 0 {
            handle_menu_command(cmd, regions, owner);
        }
    }
}

fn handle_menu_command(cmd: usize, regions: Vec<String>, owner: HWND) {
    if cmd >= 1000 && cmd < 1000 + regions.len() {
        // Region selected
        select_region(&regions[cmd - 1000]);
    } else {
        match cmd {
            1 => {
                // Toggle sound
                crate::state::with_app_mut(|app| {
                    app.config.sound = !app.config.sound;
                    app.config.save();
                });
                crate::widget::invalidate();
            }
            2 => {
                // Toggle animation
                crate::state::with_app_mut(|app| {
                    app.config.animation = !app.config.animation;
                    app.config.save();
                });
                crate::widget::invalidate();
            }
            3 => {
                // Toggle autostart
                let currently_enabled = crate::autostart::is_enabled();
                crate::autostart::set_enabled(!currently_enabled);
            }
            4 => {
                // Open alerts map
                unsafe {
                    let _ = ShellExecuteW(
                        None,
                        w!("open"),
                        w!("https://alerts.in.ua/"),
                        None,
                        None,
                        SW_SHOWNORMAL,
                    );
                }
            }
            6 => {
                // Make sure the config file exists, then open it in Notepad.
                let path = crate::config::Config::path();
                if !path.exists() {
                    crate::state::with_app(|app| app.config.save());
                }
                let arg = to_wide_string(&format!("\"{}\"", path.display()));
                unsafe {
                    let _ = ShellExecuteW(
                        None,
                        w!("open"),
                        w!("notepad.exe"),
                        PCWSTR(arg.as_ptr()),
                        None,
                        SW_SHOWNORMAL,
                    );
                }
            }
            5 => {
                // Exit - destroy the main window which will post WM_QUIT
                unsafe {
                    let _ = DestroyWindow(owner);
                }
            }
            _ => {}
        }
    }
}

fn to_wide_string(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Name of the autonomous republic that has no alert data (never selectable).
pub const CRIMEA: &str = "Автономна Республіка Крим";

/// All selectable regions, sorted alphabetically (ignoring the "м. " prefix).
pub fn region_list() -> Vec<String> {
    let mut v = crate::state::with_app(|app| {
        if app.region_names.is_empty() {
            crate::map_data::REGIONS.iter().map(|r| r.name.to_string()).collect()
        } else {
            app.region_names.clone()
        }
    });
    v.retain(|n| n != CRIMEA);
    v.sort_by_key(|n| uk_key(n.strip_prefix("м. ").unwrap_or(n)));
    v
}

/// Sort key in Ukrainian alphabet order (Rust compares code points, which puts "І" after "Я").
fn uk_key(s: &str) -> Vec<usize> {
    const ABC: &str = "абвгґдеєжзиіїйклмнопрстуфхцчшщьюя";
    s.to_lowercase().chars().map(|c| ABC.chars().position(|a| a == c).unwrap_or(100 + c as usize)).collect()
}

/// Persist `name` as the watched region and reset per-region state.
pub fn select_region(name: &str) {
    crate::state::with_app_mut(|app| {
        app.config.region = name.to_string();
        app.config.save();
    });
    crate::reset_region_state();
}

/// Popup menu with one item per region (ids 1000+index), the current one checked.
unsafe fn build_region_menu(regions: &[String], current: &str) -> Option<HMENU> {
    let menu = CreatePopupMenu().ok()?;
    for (i, r) in regions.iter().enumerate() {
        let wide = to_wide_string(r);
        let flags = if r == current { MF_STRING | MF_CHECKED } else { MF_STRING };
        let _ = AppendMenuW(menu, flags, 1000 + i, PCWSTR(wide.as_ptr()));
    }
    Some(menu)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sorted(mut v: Vec<&str>) -> Vec<&str> {
        v.sort_by_key(|n| uk_key(n.strip_prefix("м. ").unwrap_or(n)));
        v
    }

    #[test]
    fn ukrainian_alphabet_order() {
        // Code-point order would put І/Ї/Є after Я.
        assert_eq!(
            sorted(vec!["Черкаська", "Івано-Франківська", "Житомирська", "Вінницька", "Запорізька", "Їжакова", "Єнакієве", "Ґалаґанівка", "Гадяч"]),
            vec!["Вінницька", "Гадяч", "Ґалаґанівка", "Єнакієве", "Житомирська", "Запорізька", "Івано-Франківська", "Їжакова", "Черкаська"]
        );
    }

    #[test]
    fn city_prefix_ignored_and_case_insensitive() {
        // "Київ" is a prefix of "Київська", so the city goes first; "м. " must not push it to the end.
        assert_eq!(sorted(vec!["Львівська", "Київська", "м. Київ"]), vec!["м. Київ", "Київська", "Львівська"]);
        assert!(uk_key("Київ") == uk_key("київ"));
    }

    #[test]
    fn region_list_without_snapshot_uses_map_and_skips_crimea() {
        crate::state::APP.with(|a| *a.borrow_mut() = Some(crate::state::App::new(Default::default())));
        let v = region_list();
        assert_eq!(v.len(), crate::map_data::REGIONS.len() - 1);
        assert!(!v.iter().any(|n| n == CRIMEA));
        assert_eq!(v.first().map(String::as_str), Some("Вінницька область"));
        assert_eq!(v.last().map(String::as_str), Some("Чернігівська область"));
        let mut s = v.clone();
        s.sort_by_key(|n| uk_key(n.strip_prefix("м. ").unwrap_or(n)));
        assert_eq!(s, v);
    }
}
