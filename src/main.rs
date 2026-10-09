#![windows_subsystem = "windows"]

mod api;
mod autostart;
mod config;
mod flyout;
mod icons;
mod map_data;
mod menu;
mod notify;
mod state;
mod theme;
mod util;
mod widget;

use state::{with_app_mut, App, FetchResult, HistoryEntry, Level, Status, APP, WM_APP_DATA};
use windows::core::w;
use windows::Win32::Foundation::*;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::WindowsAndMessaging::*;

const TIMER_TASKBAR: usize = 1;
const TIMER_CLOCK: usize = 2;

/// Fetch failures in a row before we show "offline".
const OFFLINE_AFTER: u32 = 3;

static mut TASKBAR_CREATED: u32 = 0;

fn main() {
    let fake = std::env::args().any(|a| a == "--fake-alert");

    unsafe {
        // Single instance.
        let _mutex = CreateMutexW(None, true, w!("Local\\AlertssTaskbarWidget"));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return;
        }

        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);

        APP.with(|a| *a.borrow_mut() = Some(App::new(config::Config::load())));
        notify::init();

        let hinstance: HINSTANCE = GetModuleHandleW(None).unwrap().into();
        let class = w!("AlertssMain");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(main_wndproc),
            hInstance: hinstance,
            lpszClassName: class,
            ..Default::default()
        };
        RegisterClassW(&wc);
        TASKBAR_CREATED = RegisterWindowMessageW(w!("TaskbarCreated"));

        // Hidden top-level window: timers, poller messages, TaskbarCreated broadcast, menu owner.
        let main = CreateWindowExW(
            WINDOW_EX_STYLE(WS_EX_TOOLWINDOW.0),
            class,
            w!("Alertss"),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(hinstance),
            None,
        )
        .expect("main window");

        widget::tick(main);
        SetTimer(Some(main), TIMER_TASKBAR, 500, None);
        SetTimer(Some(main), TIMER_CLOCK, 15_000, None);
        api::spawn_poller(main.0 as isize, fake);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        widget::destroy();
    }
}

unsafe extern "system" fn main_wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_TIMER => {
            match wp.0 {
                TIMER_TASKBAR => widget::tick(hwnd),
                TIMER_CLOCK => {
                    // Durations in the text ("0:23") need periodic refresh.
                    widget::invalidate();
                    flyout::refresh();
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_APP_DATA => {
            let result = *Box::from_raw(lp.0 as *mut FetchResult);
            on_data(result);
            LRESULT(0)
        }
        WM_SETTINGCHANGE | WM_THEMECHANGED | WM_DISPLAYCHANGE | WM_DPICHANGED => {
            widget::tick(hwnd);
            widget::invalidate();
            flyout::refresh();
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ if msg == TASKBAR_CREATED && msg != 0 => {
            // Explorer restarted: our widget died with the old taskbar, re-embed.
            widget::tick(hwnd);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

/// Apply a fetch result to the state, detect transitions, notify, redraw.
fn on_data(result: FetchResult) {
    let now = util::now_unix();
    // (title, body, sound) of a toast to show after the state borrow is released.
    let mut toast: Option<(String, String, bool)> = None;

    with_app_mut(|app| match result {
        FetchResult::Err(_) => {
            app.fail_count += 1;
            if app.fail_count >= OFFLINE_AFTER {
                app.status = Status::Offline;
            }
        }
        FetchResult::Ok(snap) => {
            app.fail_count = 0;
            app.last_ok = Some(now);
            app.region_names = snap.regions.keys().cloned().collect();
            app.last_snapshot = Some(snap.clone());
            let Some(rs) = snap.regions.get(&app.config.region) else {
                app.status = Status::Unknown;
                app.since = None;
                app.level = Level::None;
                app.partial = false;
                app.areas.clear();
                app.has_levels = snap.has_levels;
                return;
            };
            let new_status = if rs.alert { Status::Alert } else { Status::Clear };
            let new_level = if rs.alert { rs.level } else { Level::None };
            let was_known = matches!(app.status, Status::Alert | Status::Clear);
            let changed = app.status != new_status;
            // Prefer the source's timestamp; fall back to "when we noticed".
            let since = match rs.changed {
                Some(t) if t <= now + 120 => Some(t),
                _ if changed && was_known => Some(now),
                _ if changed => None,
                _ => app.since,
            };
            // Offline -> back online counts as known if we had a real status before it.
            let prev_real = app.history.last().map(|h| h.alert);
            let real_transition = changed
                && (was_known || (app.status == Status::Offline && prev_real != Some(rs.alert)));
            let region = app.config.region.clone();
            let body = if rs.alert && rs.partial && !rs.areas.is_empty() {
                truncate(&format!("{} · {}", region, rs.areas.join(", ")), 80)
            } else {
                region
            };
            if real_transition {
                app.history.push(HistoryEntry { at: since.unwrap_or(now), alert: rs.alert, level: new_level });
                toast = Some(if rs.alert {
                    let title = match new_level {
                        Level::Yellow => "🛵 Жовта тривога — дрони",
                        _ if snap.has_levels => "🚀 Червона тривога — ракети",
                        _ => "🚨 Повітряна тривога",
                    };
                    (title.to_string(), body, app.config.sound)
                } else {
                    ("😊 Відбій тривоги".to_string(), body, app.config.sound)
                });
            } else if changed && app.history.is_empty() {
                // First observation: record start point without notifying.
                if let Some(t) = since {
                    app.history.push(HistoryEntry { at: t, alert: rs.alert, level: new_level });
                }
            } else if !changed && rs.alert && app.level != Level::None && app.level != new_level {
                // Level changed while the alert continues.
                app.history.push(HistoryEntry { at: now, alert: true, level: new_level });
                let title = if new_level == Level::Red {
                    "🚀 Тривогу посилено до червоної"
                } else {
                    "🛵 Рівень знижено до жовтої"
                };
                toast = Some((title.to_string(), body, app.config.sound));
            }
            app.status = new_status;
            app.since = since;
            app.level = new_level;
            app.partial = rs.alert && rs.partial;
            app.areas = if app.partial { rs.areas.clone() } else { Vec::new() };
            app.has_levels = snap.has_levels;
        }
    });

    if let Some((title, body, sound)) = toast {
        notify::show(&title, &body, sound);
    }
    widget::invalidate();
    flyout::refresh();
}

/// Called by the menu when the region changes: reset per-region state.
/// Re-applies the last snapshot so the new region shows immediately.
pub fn reset_region_state() {
    let snap = with_app_mut(|app| {
        app.status = Status::Unknown;
        app.since = None;
        app.history.clear();
        app.last_snapshot.clone()
    });
    match snap {
        Some(s) => on_data(FetchResult::Ok(s)),
        None => {
            widget::invalidate();
            flyout::refresh();
        }
    }
}

/// Cut `s` to at most `max` characters, adding an ellipsis when shortened.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}
