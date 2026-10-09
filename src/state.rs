//! Shared application state. Lives only on the UI thread (thread_local).
//!
//! RULE: never hold a `borrow()`/`borrow_mut()` of `APP` across a Win32 call that can
//! dispatch messages synchronously (SetWindowPos, TrackPopupMenu, ShowWindow, DestroyWindow,
//! SetForegroundWindow, MessageBox, ...). Copy what you need out of the state first.

use std::cell::RefCell;
use std::collections::BTreeMap;

use crate::config::Config;

/// Custom message posted by the poller thread to the main window.
/// lParam = `Box::into_raw(Box<FetchResult>)` — the receiver must take ownership with `Box::from_raw`.
pub const WM_APP_DATA: u32 = 0x8000 + 1; // WM_APP + 1

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// No data yet (just started).
    Unknown,
    Alert,
    Clear,
    /// 3+ failed fetches in a row.
    Offline,
}

/// Alert level (since Sept 2026): yellow = drone attack, red = missiles / mass strike.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    #[default]
    None,
    Yellow,
    Red,
}

#[derive(Clone, Debug, Default)]
pub struct RegionState {
    pub alert: bool,
    /// Unix seconds (UTC) of the last state change, if the source gave a valid one.
    pub changed: Option<i64>,
    /// Highest level among the region's active alerts. `None` when `alert` is false.
    /// Sources without level data report `Red` for any alert (official rule: no level = red).
    pub level: Level,
    /// True when the alert covers only part of the region (some raions/hromadas).
    pub partial: bool,
    /// Names of the affected sub-areas when `partial` (e.g. "Вінницький район"), else empty.
    pub areas: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    /// Region name exactly as the source spells it (e.g. "Вінницька область") -> state.
    pub regions: BTreeMap<String, RegionState>,
    /// Unix seconds (UTC) when fetched.
    #[allow(dead_code)] // part of the shared contract; kept for diagnostics
    pub fetched_at: i64,
    /// True if the snapshot carries real alert levels (official API), false for fallback.
    pub has_levels: bool,
}

pub enum FetchResult {
    Ok(Snapshot),
    #[allow(dead_code)] // error text is reserved for diagnostics; only the failure count is used
    Err(String),
}

#[derive(Clone, Copy, Debug)]
pub struct HistoryEntry {
    /// Unix seconds (UTC).
    pub at: i64,
    /// true = alert started (or level changed while alerting), false = all clear.
    pub alert: bool,
    /// Level at that moment (`None` for all clear).
    pub level: Level,
}

pub struct App {
    pub config: Config,
    pub status: Status,
    /// Unix seconds (UTC) since when the current Alert/Clear status holds, if known.
    pub since: Option<i64>,
    /// Level/partial/areas of the selected region (meaningful when status == Alert).
    pub level: Level,
    pub partial: bool,
    pub areas: Vec<String>,
    /// True when the latest data came from the official alerts.in.ua API (has levels);
    /// false for the token-less fallback source (levels unknown, shown as red).
    pub has_levels: bool,
    /// Unix seconds (UTC) of the last successful fetch.
    pub last_ok: Option<i64>,
    pub fail_count: u32,
    /// Transitions seen this session for the selected region (oldest first).
    pub history: Vec<HistoryEntry>,
    /// All region names from the latest snapshot (for the region menu).
    pub region_names: Vec<String>,
    /// Latest successful snapshot (re-applied when the region changes).
    pub last_snapshot: Option<Snapshot>,
}

impl App {
    pub fn new(config: Config) -> Self {
        App {
            config,
            status: Status::Unknown,
            since: None,
            level: Level::None,
            partial: false,
            areas: Vec::new(),
            has_levels: false,
            last_ok: None,
            fail_count: 0,
            history: Vec::new(),
            region_names: Vec::new(),
            last_snapshot: None,
        }
    }
}

thread_local! {
    pub static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// Run `f` with a shared reference to the app state. Panics if not initialised.
pub fn with_app<R>(f: impl FnOnce(&App) -> R) -> R {
    APP.with(|a| f(a.borrow().as_ref().expect("app not initialised")))
}

/// Run `f` with a mutable reference to the app state. Panics if not initialised.
pub fn with_app_mut<R>(f: impl FnOnce(&mut App) -> R) -> R {
    APP.with(|a| f(a.borrow_mut().as_mut().expect("app not initialised")))
}

/// Short display name: "Вінницька область" -> "Вінницька обл.", others unchanged.
pub fn short_region(name: &str) -> String {
    name.replace(" область", " обл.")
}
