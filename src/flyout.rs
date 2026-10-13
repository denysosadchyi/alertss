//! The detail panel shown when the widget is clicked.
//!
//! A borderless top-level popup with DWM acrylic backdrop and rounded corners, drawn with
//! Direct2D. Created lazily on first open, then hidden/shown. Closes on deactivation.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use windows::core::{w, Interface, BOOL, PCWSTR};
use windows_numerics::{Matrix3x2, Vector2};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::SystemInformation::GetTickCount64;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, GetDpiForSystem, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT, VK_ESCAPE,
};
use windows::Win32::UI::Controls::{MARGINS, WM_MOUSELEAVE};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::state::{with_app, Level, Status};
use crate::map_data::{REGIONS, VIEW_H, VIEW_W};
use crate::menu::CRIMEA;
use crate::icons::{self, Icon};
use crate::{menu, theme, util, widget};

const WIDTH: f32 = 400.0;
const PAD: f32 = 20.0;
const GAP_ABOVE_ANCHOR: f32 = 12.0;
const MAX_ROWS: usize = 5;
const ROW_H: f32 = 22.0;
const REOPEN_GUARD_MS: u64 = 300;
const MAP_URL: PCWSTR = w!("https://alerts.in.ua/");

/// Direct2D / DirectWrite resources.
struct Gfx {
    d2d: ID2D1Factory1,
    dw: IDWriteFactory,
    /// One path geometry per `REGIONS` entry, in map VIEW coordinates. Factory resources, so they
    /// survive render-target recreation and are rebuilt only with the factory.
    geoms: Vec<Option<ID2D1PathGeometry>>,
    /// Round caps and joins for region borders.
    stroke: Option<ID2D1StrokeStyle>,
    target: Option<ID2D1HwndRenderTarget>,
    /// "Everything except the selected region" mask, cached per `REGIONS` index.
    outside: Option<(usize, ID2D1PathGeometry1)>,
}

type GlowCache = RefCell<Option<(usize, (u8, u8, u8), ID2D1RadialGradientBrush)>>;

thread_local! {
    static HWND_CELL: Cell<isize> = const { Cell::new(0) };
    static GFX: RefCell<Option<Gfx>> = const { RefCell::new(None) };
    /// DPI scale of the current placement (1.0 = 96 dpi).
    static SCALE: Cell<f32> = const { Cell::new(1.0) };
    /// GetTickCount64 of the last deactivation-close.
    static CLOSED_AT: Cell<u64> = const { Cell::new(0) };
    static ZONE: Cell<Zone> = const { Cell::new(Zone::None) };
    /// Last mouse position in client pixels.
    static MOUSE: Cell<(f32, f32)> = const { Cell::new((0.0, 0.0)) };
    static FMT: RefCell<HashMap<(u8, u32, i32, bool), IDWriteTextFormat>> = RefCell::new(HashMap::new());
    static ICON_FONT: Cell<Option<bool>> = const { Cell::new(None) };
    static TRACKING: Cell<bool> = const { Cell::new(false) };
    /// True while our region popup menu is up (deactivation must not close the flyout).
    static IN_MENU: Cell<bool> = const { Cell::new(false) };
    /// History rows that fit the monitor (set by `place`).
    static ROW_CAP: Cell<usize> = const { Cell::new(MAX_ROWS) };
    /// Interactive rects in client pixels (l, t, r, b) from the last render.
    static LINK: Cell<[f32; 4]> = const { Cell::new([0.0; 4]) };
    static MAP: Cell<[f32; 4]> = const { Cell::new([0.0; 4]) };
    /// Cached status-glow brush with the (render target, colour) it was built for.
    static GLOW: GlowCache = const { RefCell::new(None) };
    /// Cached coloured-glow linear brush: (render target, state id, brush).
    static LGLOW: RefCell<Option<(usize, u8, ID2D1LinearGradientBrush)>> = const { RefCell::new(None) };
}

/// What the mouse is over.
#[derive(Clone, Copy, PartialEq)]
enum Zone {
    None,
    Link,
    /// Index into `REGIONS`.
    Region(usize),
}

fn hwnd() -> Option<HWND> {
    let v = HWND_CELL.with(|c| c.get());
    (v != 0).then(|| HWND(v as *mut _))
}

fn is_open() -> bool {
    hwnd().is_some_and(|h| unsafe { IsWindowVisible(h).as_bool() })
}

/// Open if closed, close if open.
pub fn toggle() {
    if is_open() {
        close();
        return;
    }
    // The click that deactivated (and closed) us was probably the one that called us.
    let since = unsafe { GetTickCount64() }.wrapping_sub(CLOSED_AT.with(|c| c.get()));
    if since < REOPEN_GUARD_MS {
        return;
    }
    open();
}

/// Redraw if open (no-op if closed).
pub fn refresh() {
    if let Some(h) = hwnd().filter(|_| is_open()) {
        place(h);
        unsafe {
            let _ = InvalidateRect(Some(h), None, false);
        }
    }
}

/// Close if open.
#[allow(dead_code)]
pub fn close() {
    if let Some(h) = hwnd().filter(|_| is_open()) {
        unsafe {
            let _ = ShowWindow(h, SW_HIDE);
        }
    }
}

fn open() {
    let Some(h) = hwnd().or_else(create_window) else { return };
    unsafe {
        let dark: BOOL = (!theme::apps_are_light()).into();
        let _ = DwmSetWindowAttribute(
            h,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &dark as *const _ as *const _,
            size_of::<BOOL>() as u32,
        );
    }
    ZONE.with(|c| c.set(Zone::None));
    place(h);
    unsafe {
        let _ = ShowWindow(h, SW_SHOW);
        let _ = SetForegroundWindow(h);
        let _ = InvalidateRect(Some(h), None, false);
    }
}

fn create_window() -> Option<HWND> {
    unsafe {
        let hinstance: HINSTANCE = GetModuleHandleW(None).ok()?.into();
        let class = w!("SirenBarFlyout");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinstance,
            lpszClassName: class,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            ..Default::default()
        };
        RegisterClassW(&wc); // fails harmlessly if already registered
        let h = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            class,
            w!("SirenBar"),
            WS_POPUP | WS_THICKFRAME,
            0,
            0,
            100,
            100,
            None,
            None,
            Some(hinstance),
            None,
        )
        .ok()?;

        let margins = MARGINS { cxLeftWidth: -1, cxRightWidth: -1, cyTopHeight: -1, cyBottomHeight: -1 };
        let _ = DwmExtendFrameIntoClientArea(h, &margins);
        let backdrop = DWMSBT_TRANSIENTWINDOW;
        let _ = DwmSetWindowAttribute(
            h,
            DWMWA_SYSTEMBACKDROP_TYPE,
            &backdrop as *const _ as *const _,
            size_of::<DWM_SYSTEMBACKDROP_TYPE>() as u32,
        );
        let corner = DWMWCP_ROUND;
        let _ = DwmSetWindowAttribute(
            h,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &corner as *const _ as *const _,
            size_of::<DWM_WINDOW_CORNER_PREFERENCE>() as u32,
        );
        HWND_CELL.with(|c| c.set(h.0 as isize));
        Some(h)
    }
}

// ---------------------------------------------------------------------------------------
// Data + layout
// ---------------------------------------------------------------------------------------

/// Per-region data for the map.
#[derive(Clone, Copy)]
struct Rs {
    alert: bool,
    level: Level,
    partial: bool,
}

/// Everything the panel shows, copied out of the app state.
struct View {
    region: String,
    status: Status,
    level: Level,
    has_levels: bool,
    /// Lines under the status row (level hint, duration).
    detail: Vec<String>,
    /// "Тривога в: ..." (wrapped), with its estimated line count (<= 3).
    areas: String,
    areas_lines: usize,
    /// (HH:MM, alert, level) newest first, today only.
    rows: Vec<(String, bool, Level)>,
    updated: String,
    /// Region name -> state, from the latest snapshot.
    alerts: HashMap<String, Rs>,
}

const AREAS_CHARS_PER_LINE: usize = 44;
const AREAS_LINE_H: f32 = 17.0;

fn snapshot() -> View {
    let now = util::now_unix();
    with_app(|app| {
        let timed = |label: &str| {
            app.since
                .map(|s| format!("{label} {} · з {}", util::fmt_long(now - s), util::local_hm(s)))
                .unwrap_or_default()
        };
        let mut detail = Vec::new();
        if app.status == Status::Alert && app.has_levels {
            match app.level {
                Level::Red => detail.push("Ракетна небезпека — негайно в укриття".to_string()),
                Level::Yellow => detail.push("Атака дронів — будьте уважні".to_string()),
                Level::None => {}
            }
        }
        let t = match app.status {
            Status::Alert => timed("Триває"),
            Status::Clear => timed("Без тривоги"),
            Status::Offline => "Не вдається отримати дані".to_string(),
            Status::Unknown => String::new(),
        };
        if !t.is_empty() {
            detail.push(t);
        }
        let mut areas = String::new();
        let mut areas_lines = 0;
        if app.status == Status::Alert && app.partial && !app.areas.is_empty() {
            areas = format!("Тривога в: {}", app.areas.join(", "));
            let max = 3 * AREAS_CHARS_PER_LINE;
            if areas.chars().count() > max {
                areas = areas.chars().take(max - 1).collect::<String>().trim_end().to_string() + "…";
            }
            areas_lines = areas.chars().count().div_ceil(AREAS_CHARS_PER_LINE).clamp(1, 3);
        }
        let rows = app
            .history
            .iter()
            .rev()
            .filter(|h| util::is_today(h.at))
            .take(ROW_CAP.with(|c| c.get()))
            .map(|h| (util::local_hm(h.at), h.alert, h.level))
            .collect();
        View {
            region: app.config.region.clone(),
            status: app.status,
            level: app.level,
            has_levels: app.has_levels,
            detail,
            areas,
            areas_lines,
            rows,
            updated: app.last_ok.map(util::local_hm).unwrap_or_else(|| "—".to_string()),
            alerts: app
                .last_snapshot
                .as_ref()
                .map(|s| {
                    s.regions
                        .iter()
                        .map(|(k, v)| (k.clone(), Rs { alert: v.alert, level: v.level, partial: v.partial }))
                        .collect()
                })
                .unwrap_or_default(),
        }
    })
}

/// Height of the detail block under the status row.
fn detail_h(v: &View) -> f32 {
    let mut h = 18.0 * v.detail.len() as f32;
    if v.areas_lines > 0 {
        h += 2.0 + AREAS_LINE_H * v.areas_lines as f32;
    }
    h
}

/// Vertical positions in logical px.
struct Layout {
    sel_y: f32,
    status_y: f32,
    detail_y: Option<f32>,
    map_y: f32,
    map_h: f32,
    legend_y: f32,
    div1_y: f32,
    today_y: f32,
    rows_y: f32,
    height: f32,
}

fn layout(v: &View) -> Layout {
    let sel_y = PAD + 16.0;
    let status_y = sel_y + 26.0 + 8.0;
    let mut y = status_y + 40.0;
    let dh = detail_h(v);
    let detail_y = if dh == 0.0 {
        None
    } else {
        let d = y + 4.0;
        y = d + dh;
        Some(d)
    };
    let map_y = y + 14.0;
    let map_h = (WIDTH - 2.0 * PAD) * VIEW_H / VIEW_W;
    let legend_y = map_y + map_h + 10.0;
    let div1_y = legend_y + 18.0 + 12.0;
    let today_y = div1_y + 13.0;
    let rows_y = today_y + 16.0 + 6.0;
    let height = rows_y + ROW_H * v.rows.len().max(1) as f32 + PAD;
    Layout { sel_y, status_y, detail_y, map_y, map_h, legend_y, div1_y, today_y, rows_y, height }
}

fn monitor_scale(m: HMONITOR) -> f32 {
    let (mut x, mut y) = (0u32, 0u32);
    let dpi = unsafe {
        match GetDpiForMonitor(m, MDT_EFFECTIVE_DPI, &mut x, &mut y) {
            Ok(()) if x > 0 => x,
            _ => GetDpiForSystem(),
        }
    };
    dpi as f32 / 96.0
}

/// Size and position the window above the widget, clamped to the monitor work area.
fn place(h: HWND) {
    unsafe {
        let anchor = widget::screen_rect().unwrap_or_else(|| {
            let mut wa = RECT::default();
            let _ = SystemParametersInfoW(
                SPI_GETWORKAREA,
                0,
                Some(&mut wa as *mut _ as *mut _),
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
            );
            RECT { left: wa.left, top: wa.bottom, right: wa.left, bottom: wa.bottom }
        });
        let mon = MonitorFromRect(&anchor, MONITOR_DEFAULTTONEAREST);
        let scale = monitor_scale(mon);
        SCALE.with(|c| c.set(scale));

        let wpx = (WIDTH * scale).round() as i32;
        let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        let work = if GetMonitorInfoW(mon, &mut mi).as_bool() {
            mi.rcWork
        } else {
            RECT { left: 0, top: 0, right: 1920, bottom: 1080 }
        };
        // Drop history rows until the panel fits between the work-area top and the widget.
        ROW_CAP.with(|c| c.set(MAX_ROWS));
        let mut view = snapshot();
        let avail = (anchor.top - (GAP_ABOVE_ANCHOR * scale).round() as i32 - work.top).max(0) as f32;
        while !view.rows.is_empty() && layout(&view).height * scale > avail {
            view.rows.pop();
        }
        ROW_CAP.with(|c| c.set(view.rows.len()));
        let hpx = (layout(&view).height * scale).round() as i32;
        let gap = (GAP_ABOVE_ANCHOR * scale).round() as i32;
        let mut x = anchor.left;
        let mut y = anchor.top - gap - hpx;
        x = x.min(work.right - wpx).max(work.left);
        y = y.min(work.bottom - hpx).max(work.top);

        let mut cur = RECT::default();
        let _ = GetWindowRect(h, &mut cur);
        if cur.left != x || cur.top != y || cur.right - cur.left != wpx || cur.bottom - cur.top != hpx {
            let _ = SetWindowPos(h, Some(HWND_TOPMOST), x, y, wpx, hpx, SWP_NOACTIVATE);
        }
    }
}

// ---------------------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------------------

fn color(r: u8, g: u8, b: u8, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r: r as f32 / 255.0, g: g as f32 / 255.0, b: b as f32 / 255.0, a }
}

struct Palette {
    tint: D2D1_COLOR_F,
    primary: D2D1_COLOR_F,
    secondary: D2D1_COLOR_F,
    divider: D2D1_COLOR_F,
    hover: D2D1_COLOR_F,
    link: D2D1_COLOR_F,
    tip_bg: D2D1_COLOR_F,
}

fn palette(light: bool) -> Palette {
    if light {
        Palette {
            tint: color(0xF3, 0xF3, 0xF3, 0.6),
            primary: color(0x1A, 0x1A, 0x1A, 1.0),
            secondary: color(0, 0, 0, 0.6),
            divider: color(0x1A, 0x1A, 0x1A, 0.12),
            hover: color(0x1A, 0x1A, 0x1A, 0.08),
            link: color(0x00, 0x5F, 0xB8, 1.0),
            tip_bg: color(0xFA, 0xFA, 0xFA, 0.96),
        }
    } else {
        Palette {
            tint: color(0x20, 0x20, 0x20, 0.55),
            primary: color(255, 255, 255, 1.0),
            secondary: color(255, 255, 255, 0.65),
            divider: color(255, 255, 255, 0.12),
            hover: color(255, 255, 255, 0.08),
            link: color(0x4C, 0xC2, 0xFF, 1.0),
            tip_bg: color(0x2C, 0x2C, 0x2C, 0.96),
        }
    }
}

/// Alert red (slightly coral) and yellow.
const RED: (u8, u8, u8) = (0xF0, 0x50, 0x4A);
const YELLOW: (u8, u8, u8) = (0xFF, 0xB9, 0x00);

/// Status glow behind the map: peak alpha (dark / light theme) and radius as a fraction of map width.
const GLOW_ALPHA_DARK: f32 = 0.22;
const GLOW_ALPHA_LIGHT: f32 = 0.28;
const GLOW_RADIUS_K: f32 = 0.75;
/// Outline of the selected region (2 px), both themes.
const SELECTED_BORDER: (u8, u8, u8, f32) = (255, 255, 255, 1.0);
/// White glow around the selected region: (stroke width in logical px, alpha), widest first.
const SELECTED_GLOW: [(f32, f32); 8] = [
    (44.0, 0.02), (38.0, 0.03), (32.0, 0.04), (26.0, 0.055),
    (20.0, 0.07), (14.0, 0.09), (9.0, 0.115), (6.0, 0.14),
];
/// Pulse of the selected-region glow.
const PULSE_TIMER: usize = 1;
const PULSE_MS: u32 = 33;
const PULSE_PERIOD_MS: f64 = 1800.0;
/// Border of map regions: white, both themes.
const MAP_BORDER: (u8, u8, u8, f32) = (255, 255, 255, 0.95);
/// Glow gradient pairs (start top-left -> end bottom-right).
const GLOW_GREEN_LIGHT: ((u8, u8, u8), (u8, u8, u8)) = ((0x16, 0xC6, 0x0C), (0x00, 0xB7, 0xA8));
const GLOW_GREEN_DARK: ((u8, u8, u8), (u8, u8, u8)) = ((0x6C, 0xCB, 0x5F), (0x3D, 0xD6, 0xC4));
const GLOW_YELLOW: ((u8, u8, u8), (u8, u8, u8)) = ((0xFF, 0xB9, 0x00), (0xFF, 0x8A, 0x00));
const GLOW_RED: ((u8, u8, u8), (u8, u8, u8)) = ((0xF0, 0x50, 0x4A), (0xF2, 0x50, 0x7E));
/// Gaussian-like falloff: (t, fraction of peak alpha).
/// How much white is mixed into the level colour for partial alerts (0 = full colour).
const PARTIAL_WHITE_MIX: f32 = 0.55;
const GLOW_STOPS: [(f32, f32); 6] = [(0.0, 1.0), (0.25, 0.85), (0.5, 0.5), (0.75, 0.18), (0.9, 0.04), (1.0, 0.0)];

fn rgb(c: (u8, u8, u8), a: f32) -> D2D1_COLOR_F {
    color(c.0, c.1, c.2, a)
}

/// Opaque pastel of `c` (mixed with white) for partial alerts, so the glow behind the map
/// doesn't tint it the way a translucent fill would.
fn pastel(c: (u8, u8, u8)) -> D2D1_COLOR_F {
    let mix = |v: u8| (v as f32 + (255.0 - v as f32) * PARTIAL_WHITE_MIX) as u8;
    color(mix(c.0), mix(c.1), mix(c.2), 0.95)
}

/// Cached radial falloff brush (rebuilt per render target / colour), positioned at (cx, cy).
fn radial_brush(
    rt: &ID2D1HwndRenderTarget,
    cache: &'static std::thread::LocalKey<GlowCache>,
    hue: (u8, u8, u8),
    stops: &[(f32, f32)],
    cx: f32,
    cy: f32,
    radius: f32,
) -> Option<ID2D1RadialGradientBrush> {
    let key = rt.as_raw() as usize;
    cache.with(|g| unsafe {
        let mut g = g.borrow_mut();
        if !matches!(&*g, Some((k, c, _)) if *k == key && *c == hue) {
            *g = None;
            let stops: Vec<D2D1_GRADIENT_STOP> = stops
                .iter()
                .map(|&(t, f)| D2D1_GRADIENT_STOP { position: t, color: rgb(hue, f) })
                .collect();
            let props = D2D1_RADIAL_GRADIENT_BRUSH_PROPERTIES {
                center: Vector2 { X: cx, Y: cy },
                gradientOriginOffset: Vector2 { X: 0.0, Y: 0.0 },
                radiusX: radius,
                radiusY: radius,
            };
            if let Ok(coll) = rt.CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP) {
                if let Ok(b) = rt.CreateRadialGradientBrush(&props, None, &coll) {
                    *g = Some((key, hue, b));
                }
            }
        }
        if let Some((_, _, b)) = &*g {
            b.SetCenter(Vector2 { X: cx, Y: cy });
            b.SetRadiusX(radius);
            b.SetRadiusY(radius);
            Some(b.clone())
        } else {
            None
        }
    })
}

/// Coloured glow: a diagonal two-colour linear gradient masked by the radial falloff.
fn draw_gradient_glow(
    rt: &ID2D1HwndRenderTarget,
    state: u8,
    pair: ((u8, u8, u8), (u8, u8, u8)),
    opacity: f32,
    cx: f32,
    cy: f32,
    radius: f32,
) {
    let Some(mask) = radial_brush(rt, &GLOW, (255, 255, 255), &GLOW_STOPS, cx, cy, radius) else { return };
    let key = rt.as_raw() as usize;
    let bounds = rect(cx - radius, cy - radius, cx + radius, cy + radius);
    let (p0, p1) = (Vector2 { X: bounds.left, Y: bounds.top }, Vector2 { X: bounds.right, Y: bounds.bottom });
    let lin = LGLOW.with(|c| unsafe {
        let mut c = c.borrow_mut();
        if !matches!(&*c, Some((k, st, _)) if *k == key && *st == state) {
            *c = None;
            let stops = [
                D2D1_GRADIENT_STOP { position: 0.0, color: rgb(pair.0, 1.0) },
                D2D1_GRADIENT_STOP { position: 1.0, color: rgb(pair.1, 1.0) },
            ];
            let props = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES { startPoint: p0, endPoint: p1 };
            if let Ok(coll) = rt.CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP) {
                if let Ok(b) = rt.CreateLinearGradientBrush(&props, None, &coll) {
                    *c = Some((key, state, b));
                }
            }
        }
        c.as_ref().map(|(_, _, b)| {
            b.SetStartPoint(p0);
            b.SetEndPoint(p1);
            b.clone()
        })
    });
    let Some(lin) = lin else { return };
    unsafe {
        let Ok(layer) = rt.CreateLayer(None) else { return };
        let Ok(mask_brush) = mask.cast::<ID2D1Brush>() else { return };
        let params = D2D1_LAYER_PARAMETERS {
            contentBounds: bounds,
            geometricMask: std::mem::ManuallyDrop::new(None),
            maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
            maskTransform: Matrix3x2 { M11: 1.0, M12: 0.0, M21: 0.0, M22: 1.0, M31: 0.0, M32: 0.0 },
            opacity: opacity.clamp(0.0, 1.0),
            opacityBrush: std::mem::ManuallyDrop::new(Some(mask_brush)),
            layerOptions: D2D1_LAYER_OPTIONS_NONE,
        };
        rt.PushLayer(&params, &layer);
        rt.FillRectangle(&bounds, &lin);
        rt.PopLayer();
        std::mem::ManuallyDrop::into_inner(params.opacityBrush);
    }
}

/// Geometry covering everything except `region` (in the same coordinates), used to keep the
/// selected-region glow on the outside only.
fn outside_mask(d2d: &ID2D1Factory1, region: &ID2D1PathGeometry) -> Option<ID2D1PathGeometry1> {
    unsafe {
        let all = d2d.CreateRectangleGeometry(&rect(-1.0e6, -1.0e6, 1.0e6, 1.0e6)).ok()?;
        let out = d2d.CreatePathGeometry().ok()?;
        let sink = out.Open().ok()?;
        all.CombineWithGeometry(region, D2D1_COMBINE_MODE_EXCLUDE, None, 0.25, &sink).ok()?;
        sink.Close().ok()?;
        Some(out)
    }
}

/// Effective level of an alert (sources without levels report red).
fn eff(has_levels: bool, l: Level) -> Level {
    if has_levels && l == Level::Yellow {
        Level::Yellow
    } else {
        Level::Red
    }
}

fn ensure_gfx(g: &mut Option<Gfx>) -> Option<&mut Gfx> {
    if g.is_none() {
        unsafe {
            let d2d: ID2D1Factory1 =
                D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).ok()?;
            let dw: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).ok()?;
            let geoms = build_geoms(&d2d);
            let props = D2D1_STROKE_STYLE_PROPERTIES {
                startCap: D2D1_CAP_STYLE_ROUND,
                endCap: D2D1_CAP_STYLE_ROUND,
                dashCap: D2D1_CAP_STYLE_ROUND,
                lineJoin: D2D1_LINE_JOIN_ROUND,
                miterLimit: 1.0,
                dashStyle: D2D1_DASH_STYLE_SOLID,
                dashOffset: 0.0,
            };
            let base: &ID2D1Factory = &d2d;
            let stroke = base.CreateStrokeStyle(&props, None).ok();
            *g = Some(Gfx { d2d, dw, geoms, stroke, target: None, outside: None });
        }
    }
    g.as_mut()
}

fn build_geoms(d2d: &ID2D1Factory) -> Vec<Option<ID2D1PathGeometry>> {
    let mut geoms: Vec<Option<ID2D1PathGeometry>> = REGIONS
        .iter()
        .map(|r| unsafe {
            let g = d2d.CreatePathGeometry().ok()?;
            let sink = g.Open().ok()?;
            sink.SetFillMode(D2D1_FILL_MODE_WINDING);
            for ring in r.rings {
                if ring.len() < 3 {
                    continue;
                }
                let pt = |p: &(i16, i16)| Vector2 { X: p.0 as f32, Y: p.1 as f32 };
                sink.BeginFigure(pt(&ring[0]), D2D1_FIGURE_BEGIN_FILLED);
                let rest: Vec<Vector2> = ring[1..].iter().map(pt).collect();
                sink.AddLines(&rest);
                sink.EndFigure(D2D1_FIGURE_END_CLOSED);
            }
            sink.Close().ok()?;
            Some(g)
        })
        .collect();
    // Cities are separate regions lying on top of their oblast. Cut them out of the oblast,
    // otherwise a clear (transparent) city shows the oblast's alert colour through it.
    for (outer, inner) in ENCLAVES {
        let idx = |n: &str| REGIONS.iter().position(|r| r.name == n);
        let (Some(o), Some(i)) = (idx(outer), idx(inner)) else { continue };
        let (Some(og), Some(ig)) = (geoms[o].clone(), geoms[i].clone()) else { continue };
        let cut = unsafe {
            (|| {
                let g = d2d.CreatePathGeometry().ok()?;
                let sink = g.Open().ok()?;
                og.CombineWithGeometry(&ig, D2D1_COMBINE_MODE_EXCLUDE, None, 0.25, &sink).ok()?;
                sink.Close().ok()?;
                Some(g)
            })()
        };
        if cut.is_some() {
            geoms[o] = cut;
        }
    }
    geoms
}

/// (oblast, city inside it).
const ENCLAVES: [(&str, &str); 2] = [("Київська область", "м. Київ"), ("Автономна Республіка Крим", "Севастополь")];

fn measure(dw: &IDWriteFactory, f: &Option<IDWriteTextFormat>, text: &str) -> f32 {
    let Some(f) = f else { return 0.0 };
    let wide: Vec<u16> = text.encode_utf16().collect();
    unsafe {
        if let Ok(l) = dw.CreateTextLayout(&wide, f, 4000.0, 100.0) {
            let mut m = DWRITE_TEXT_METRICS::default();
            if l.GetMetrics(&mut m).is_ok() {
                return m.widthIncludingTrailingWhitespace.ceil();
            }
        }
    }
    0.0
}

const FAM_TEXT: u8 = 0;
const FAM_DISPLAY: u8 = 1;
const FAM_ICON: u8 = 2;

/// True if "Segoe Fluent Icons" is installed (else "Segoe MDL2 Assets" is used). Cached.
fn has_fluent(dw: &IDWriteFactory) -> bool {
    if let Some(b) = ICON_FONT.with(|c| c.get()) {
        return b;
    }
    let found = unsafe {
        (|| {
            let mut coll = None;
            dw.GetSystemFontCollection(&mut coll, false).ok()?;
            let coll = coll?;
            let (mut idx, mut exists) = (0u32, BOOL(0));
            coll.FindFamilyName(w!("Segoe Fluent Icons"), &mut idx, &mut exists).ok()?;
            Some(exists.as_bool())
        })()
        .unwrap_or(false)
    };
    ICON_FONT.with(|c| c.set(Some(found)));
    found
}

/// Cached text format. The icon format are centred in their box.
fn cached_format(dw: &IDWriteFactory, fam: u8, px: f32, weight: DWRITE_FONT_WEIGHT, wrap: bool) -> Option<IDWriteTextFormat> {
    let key = (fam, px.to_bits(), weight.0, wrap);
    if let Some(f) = FMT.with(|m| m.borrow().get(&key).cloned()) {
        return Some(f);
    }
    let family = match fam {
        FAM_DISPLAY => w!("Segoe UI Variable Display"),
        FAM_ICON if has_fluent(dw) => w!("Segoe Fluent Icons"),
        FAM_ICON => w!("Segoe MDL2 Assets"),
        _ => w!("Segoe UI Variable Text"),
    };
    unsafe {
        let f = dw
            .CreateTextFormat(family, None, weight, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, px, w!("uk-ua"))
            .ok()?;
        let _ = f.SetWordWrapping(if wrap { DWRITE_WORD_WRAPPING_WRAP } else { DWRITE_WORD_WRAPPING_NO_WRAP });
        if fam == FAM_ICON {
            let _ = f.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER);
            let _ = f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
        }
        FMT.with(|m| m.borrow_mut().insert(key, f.clone()));
        Some(f)
    }
}

fn text_format(dw: &IDWriteFactory, display: bool, px: f32, semibold: bool, wrap: bool) -> Option<IDWriteTextFormat> {
    let weight = if semibold { DWRITE_FONT_WEIGHT_SEMI_BOLD } else { DWRITE_FONT_WEIGHT_NORMAL };
    cached_format(dw, if display { FAM_DISPLAY } else { FAM_TEXT }, px, weight, wrap)
}

fn rect(l: f32, t: f32, r: f32, b: f32) -> D2D_RECT_F {
    D2D_RECT_F { left: l, top: t, right: r, bottom: b }
}

/// Render the whole panel. Drops the render target on device loss so the next paint recreates it.
fn render(h: HWND) {
    let view = snapshot();
    let lay = layout(&view);
    let light = theme::apps_are_light();
    let pal = palette(light);
    let s = SCALE.with(|c| c.get());
    let zone = ZONE.with(|c| c.get());
    let mouse = MOUSE.with(|c| c.get());
    let animate = with_app(|a| a.config.animation);

    GFX.with(|cell| {
        let mut guard = cell.borrow_mut();
        let Some(gfx) = ensure_gfx(&mut guard) else { return };
        let mut rc = RECT::default();
        unsafe {
            let _ = GetClientRect(h, &mut rc);
        }
        let size = D2D_SIZE_U { width: (rc.right - rc.left).max(1) as u32, height: (rc.bottom - rc.top).max(1) as u32 };

        if gfx.target.is_none() {
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: 96.0,
                dpiY: 96.0,
                usage: D2D1_RENDER_TARGET_USAGE_NONE,
                minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
            };
            let hprops = D2D1_HWND_RENDER_TARGET_PROPERTIES {
                hwnd: h,
                pixelSize: size,
                presentOptions: D2D1_PRESENT_OPTIONS_NONE,
            };
            gfx.target = unsafe { gfx.d2d.CreateHwndRenderTarget(&props, &hprops).ok() };
        }
        let dw = gfx.dw.clone();
        let Some(rt) = gfx.target.clone() else { return };

        unsafe {
            if rt.GetPixelSize() != size {
                let _ = rt.Resize(&size);
            }
            rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            let Ok(brush) = rt.CreateSolidColorBrush(&pal.primary, None) else { return };

            rt.BeginDraw();
            rt.Clear(Some(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }));
            brush.SetColor(&pal.tint);
            rt.FillRectangle(&rect(0.0, 0.0, size.width as f32, size.height as f32), &brush);

            let x0 = PAD * s;
            let x1 = (WIDTH - PAD) * s;

            // 0. Soft status glow behind everything, centred on the map (Offline/Unknown: none).
            let glow = match view.status {
                Status::Clear => Some(if light { (0u8, GLOW_GREEN_LIGHT) } else { (1u8, GLOW_GREEN_DARK) }),
                Status::Alert if eff(view.has_levels, view.level) == Level::Yellow => Some((2u8, GLOW_YELLOW)),
                Status::Alert => Some((3u8, GLOW_RED)),
                _ => None,
            };
            if let Some((state, pair)) = glow {
                let peak = if light { GLOW_ALPHA_LIGHT } else { GLOW_ALPHA_DARK };
                let (cx, cy) = ((x0 + x1) / 2.0, (lay.map_y + lay.map_h / 2.0) * s);
                draw_gradient_glow(&rt, state, pair, peak, cx, cy, GLOW_RADIUS_K * (x1 - x0));
            }

            let fmt = |px: f32, semi: bool, display: bool| text_format(&dw, display, px * s, semi, false);
            let draw = |text: &str, f: &Option<IDWriteTextFormat>, r: D2D_RECT_F, c: &D2D1_COLOR_F| {
                if let Some(f) = f {
                    brush.SetColor(c);
                    let wide: Vec<u16> = text.encode_utf16().collect();
                    rt.DrawText(&wide, f, &r, &brush, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
                }
            };
            // Monochrome Fluent status icon of `px` logical size centred at (cx, cy).
            let base: Option<ID2D1Factory> = gfx.d2d.cast().ok();
            let status_icon = |ic: Icon, px: f32, cx: f32, cy: f32| {
                if let Some(f) = &base {
                    let hb = px * s / 2.0;
                    icons::draw(&rt, f, ic, rect(cx - hb, cy - hb, cx + hb, cy + hb), ic.tint(light));
                }
            };
            // Fluent icon glyph centred at (cx, cy).
            let glyph = |g: &str, px: f32, cx: f32, cy: f32, c: &D2D1_COLOR_F| {
                if let Some(f) = cached_format(&dw, FAM_ICON, px * s, DWRITE_FONT_WEIGHT_NORMAL, false) {
                    brush.SetColor(c);
                    let hb = px * s;
                    let wide: Vec<u16> = g.encode_utf16().collect();
                    rt.DrawText(&wide, &f, &rect(cx - hb, cy - hb, cx + hb, cy + hb), &brush, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
                }
            };
            let dot = |cx: f32, cy: f32, r: f32, c: D2D1_COLOR_F| {
                brush.SetColor(&c);
                let e = D2D1_ELLIPSE { point: Vector2 { X: cx, Y: cy }, radiusX: r * s, radiusY: r * s };
                rt.FillEllipse(&e, &brush);
            };
            let divider = |y: f32| {
                brush.SetColor(&pal.divider);
                rt.FillRectangle(&rect(x0, y * s, x1, y * s + (1.0f32).max(s.floor())), &brush);
            };

            let f12 = fmt(12.0, false, false);
            let f13 = fmt(13.0, false, false);
            let f15b = fmt(15.0, true, false);
            let f28 = fmt(28.0, true, true);
            let f13w = text_format(&dw, false, 13.0 * s, false, true);

            // 1. Caption + region name (the region is chosen by clicking the map).
            draw("Повітряна тривога", &f12, rect(x0, PAD * s, x1, (PAD + 16.0) * s), &pal.secondary);
            // Top right: "Відкрити карту" link on the caption line, update time under it.
            let link_text = "Відкрити карту";
            let tw = measure(&dw, &f13, link_text);
            let lr = rect(x1 - tw - 26.0 * s, (PAD - 5.0) * s, x1 + 6.0 * s, (PAD + 19.0) * s);
            LINK.with(|c| c.set([lr.left, lr.top, lr.right, lr.bottom]));
            if zone == Zone::Link {
                brush.SetColor(&pal.hover);
                rt.FillRoundedRectangle(
                    &D2D1_ROUNDED_RECT { rect: lr, radiusX: 4.0 * s, radiusY: 4.0 * s },
                    &brush,
                );
            }
            draw(link_text, &f13, rect(lr.left + 6.0 * s, (PAD - 3.0) * s, x1, (PAD + 17.0) * s), &pal.link);
            glyph("\u{E8A7}", 6.0, lr.left + 6.0 * s + tw + 10.0 * s, (PAD + 7.0) * s, &pal.link);
            let upd = format!("Оновлено о {}", view.updated);
            let uw = measure(&dw, &f12, &upd);
            draw(&upd, &f12, rect(x1 - uw - 1.0 * s, (lay.sel_y + 5.0) * s, x1 + 2.0 * s, (lay.sel_y + 24.0) * s), &pal.secondary);
            draw(&view.region, &f15b, rect(x0, (lay.sel_y + 2.0) * s, x1 - uw - 12.0 * s, (lay.sel_y + 26.0) * s), &pal.primary);

            // 2. Status row.
            let (label, ic) = match view.status {
                Status::Alert => match (view.has_levels, view.level) {
                    (true, Level::Red) => ("Червона тривога", Icon::Rocket),
                    (true, Level::Yellow) => ("Жовта тривога", Icon::Drone),
                    _ => ("Повітряна тривога", Icon::Urgent),
                },
                Status::Clear if view.detail.is_empty() => ("Немає тривоги", Icon::Smile),
                Status::Clear => ("Відбій", Icon::Smile),
                Status::Offline => ("Немає зв'язку", Icon::Unknown),
                Status::Unknown => ("Завантаження…", Icon::Unknown),
            };
            status_icon(ic, 28.0, x0 + 14.0 * s, (lay.status_y + 20.0) * s);
            draw(label, &f28, rect(x0 + 40.0 * s, lay.status_y * s, x1, (lay.status_y + 40.0) * s), &pal.primary);

            // 3. Detail lines.
            if let Some(mut dy) = lay.detail_y {
                for line in &view.detail {
                    draw(line, &f13, rect(x0, dy * s, x1, (dy + 18.0) * s), &pal.secondary);
                    dy += 18.0;
                }
                if view.areas_lines > 0 {
                    dy += 2.0;
                    draw(&view.areas, &f13w, rect(x0, dy * s, x1, (dy + AREAS_LINE_H * view.areas_lines as f32) * s), &pal.secondary);
                }
            }

            // 4. Map.
            let map_w = x1 - x0;
            let map_top = lay.map_y * s;
            let k = map_w / VIEW_W;
            MAP.with(|c| c.set([x0, map_top, x1, map_top + lay.map_h * s]));
            let hair = s.max(1.0) / k; // ~1 device px in VIEW units
            let selected = REGIONS.iter().position(|r| r.name == view.region);
            let hovered = if let Zone::Region(i) = zone { Some(i) } else { None };
            rt.SetTransform(&Matrix3x2 { M11: k, M12: 0.0, M21: 0.0, M22: k, M31: x0, M32: map_top });
            let border = color(255, 255, 255, 0.95);
            let grey_line = if light { color(0x9A, 0x9A, 0x9A, 0.9) } else { color(0x6A, 0x6A, 0x6A, 0.9) };
            let nodata = D2D1_COLOR_F { a: 0.06, ..pal.primary };
            let hov = D2D1_COLOR_F { a: 0.08, ..pal.primary };
            for (i, r) in REGIONS.iter().enumerate() {
                let Some(Some(g)) = gfx.geoms.get(i) else { continue };
                let fill = match view.alerts.get(r.name) {
                    _ if r.name == CRIMEA => nodata,
                    Some(rs) if rs.alert => {
                        let hue = if eff(view.has_levels, rs.level) == Level::Yellow { YELLOW } else { RED };
                        if rs.partial { pastel(hue) } else { rgb(hue, 0.85) }
                    }
                    Some(_) => continue, // no alert: transparent
                    None => nodata,
                };
                brush.SetColor(&fill);
                rt.FillGeometry(g, &brush, None);
            }
            brush.SetColor(&color(MAP_BORDER.0, MAP_BORDER.1, MAP_BORDER.2, MAP_BORDER.3));
            for g in gfx.geoms.iter().flatten() {
                rt.DrawGeometry(g, &brush, hair, gfx.stroke.as_ref());
            }
            if let Some(Some(g)) = hovered.and_then(|i| gfx.geoms.get(i)) {
                brush.SetColor(&hov);
                rt.FillGeometry(g, &brush, None);
            }
            if let Some(Some(g)) = selected.and_then(|i| gfx.geoms.get(i)) {
                // Soft white glow outside the region only: progressively narrower, more opaque
                // strokes, clipped by a mask of "everything except the region".
                let si = selected.unwrap_or(0);
                if !matches!(&gfx.outside, Some((i, _)) if *i == si) {
                    gfx.outside = outside_mask(&gfx.d2d, g).map(|m| (si, m));
                }
                let outside = gfx.outside.as_ref().map(|(_, m)| m.clone());
                let (wk, ak) = if animate {
                    let t = GetTickCount64() as f64 % PULSE_PERIOD_MS;
                    let p = (0.5 + 0.5 * (std::f64::consts::TAU * t / PULSE_PERIOD_MS).sin()) as f32;
                    (0.80 + 0.35 * p, 0.65 + 0.55 * p)
                } else {
                    (0.80 + 0.35 * 0.5, 0.65 + 0.55 * 0.5)
                };
                let layer = rt.CreateLayer(None).ok();
                if let (Some(mask), Some(layer)) = (&outside, &layer) {
                    let params = D2D1_LAYER_PARAMETERS {
                        contentBounds: rect(-1.0e6, -1.0e6, 1.0e6, 1.0e6),
                        geometricMask: std::mem::ManuallyDrop::new(mask.cast::<ID2D1Geometry>().ok()),
                        maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                        maskTransform: Matrix3x2 { M11: 1.0, M12: 0.0, M21: 0.0, M22: 1.0, M31: 0.0, M32: 0.0 },
                        opacity: 1.0,
                        opacityBrush: std::mem::ManuallyDrop::new(None),
                        layerOptions: D2D1_LAYER_OPTIONS_NONE,
                    };
                    rt.PushLayer(&params, layer);
                    for (w, a) in SELECTED_GLOW {
                        brush.SetColor(&color(255, 255, 255, a * ak));
                        rt.DrawGeometry(g, &brush, w * wk * s.max(1.0) / k, gfx.stroke.as_ref());
                    }
                    rt.PopLayer();
                    std::mem::ManuallyDrop::into_inner(params.geometricMask);
                }
                brush.SetColor(&color(SELECTED_BORDER.0, SELECTED_BORDER.1, SELECTED_BORDER.2, SELECTED_BORDER.3));
                rt.DrawGeometry(g, &brush, 2.0 * s.max(1.0) / k, gfx.stroke.as_ref());
            }
            rt.SetTransform(&Matrix3x2 { M11: 1.0, M12: 0.0, M21: 0.0, M22: 1.0, M31: 0.0, M32: 0.0 });

            // Hover label near the cursor, clamped into the map.
            if let Some(i) = hovered {
                let name = REGIONS[i].name;
                let state = match view.alerts.get(name) {
                    _ if name == CRIMEA => "Немає даних".to_string(),
                    Some(rs) if rs.alert => {
                        let base = if !view.has_levels {
                            "Тривога"
                        } else if rs.level == Level::Yellow {
                            "Жовта"
                        } else {
                            "Червона"
                        };
                        if rs.partial { format!("{base} (частково)") } else { base.to_string() }
                    }
                    Some(_) => "Немає тривоги".to_string(),
                    None => "Немає даних".to_string(),
                };
                let text = format!("{} · {}", crate::state::short_region(name), state);
                let tw = measure(&dw, &f12, &text);
                let (bw, bh) = (tw + 16.0 * s, 24.0 * s);
                let bx = (mouse.0 + 12.0 * s).min(x1 - bw).max(x0);
                let by = (mouse.1 + 16.0 * s).min(map_top + lay.map_h * s - bh).max(map_top);
                let tr = rect(bx, by, bx + bw, by + bh);
                let rr = D2D1_ROUNDED_RECT { rect: tr, radiusX: 4.0 * s, radiusY: 4.0 * s };
                brush.SetColor(&pal.tip_bg);
                rt.FillRoundedRectangle(&rr, &brush);
                brush.SetColor(&pal.divider);
                rt.DrawRoundedRectangle(&rr, &brush, 1.0, None);
                draw(&text, &f12, rect(bx + 8.0 * s, by + 4.0 * s, bx + bw, by + bh), &pal.primary);
            }

            // 5. Legend.
            let ly = lay.legend_y;
            let cy = (ly + 9.0) * s;
            let mut lx = x0;
            let item = |lx: &mut f32, sw: D2D1_COLOR_F, ring: bool, text: &str| {
                if ring {
                    // Hollow: white ring with a thin grey outline so it shows on any backdrop.
                    let c = Vector2 { X: *lx + 4.0 * s, Y: cy };
                    brush.SetColor(&grey_line);
                    rt.DrawEllipse(&D2D1_ELLIPSE { point: c, radiusX: 4.5 * s, radiusY: 4.5 * s }, &brush, 1.0, None);
                    brush.SetColor(&border);
                    rt.DrawEllipse(&D2D1_ELLIPSE { point: c, radiusX: 3.5 * s, radiusY: 3.5 * s }, &brush, 1.5 * s, None);
                } else {
                    dot(*lx + 4.0 * s, cy, 4.0, sw);
                }
                draw(text, &f12, rect(*lx + 13.0 * s, ly * s, x1, (ly + 18.0) * s), &pal.secondary);
                *lx += 13.0 * s + measure(&dw, &f12, text) + 12.0 * s;
            };
            let item_text = |lx: &mut f32, text: &str| {
                draw(text, &f12, rect(*lx + 13.0 * s, ly * s, x1, (ly + 18.0) * s), &pal.secondary);
                *lx += 13.0 * s + measure(&dw, &f12, text) + 12.0 * s;
            };
            let n = view.alerts.values().filter(|a| a.alert).count();
            let count_text = format!("{n} у тривозі");
            let count_w = measure(&dw, &f12, &count_text);
            if view.has_levels {
                item(&mut lx, rgb(RED, 0.85), false, "Червона");
                item(&mut lx, rgb(YELLOW, 0.85), false, "Жовта");
                // "Partial": half pale yellow, half pale red, like the map's partial regions.
                let (px, pr) = (lx + 4.0 * s, 4.0 * s);
                let half = |left: bool, c: D2D1_COLOR_F| {
                    brush.SetColor(&c);
                    let clip = if left {
                        rect(px - pr, cy - pr, px, cy + pr)
                    } else {
                        rect(px, cy - pr, px + pr, cy + pr)
                    };
                    rt.PushAxisAlignedClip(&clip, D2D1_ANTIALIAS_MODE_ALIASED);
                    rt.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: px, Y: cy }, radiusX: pr, radiusY: pr }, &brush);
                    rt.PopAxisAlignedClip();
                };
                half(true, pastel(YELLOW));
                half(false, pastel(RED));
                item_text(&mut lx, "Частково");
            } else {
                item(&mut lx, rgb(RED, 0.85), false, "Тривога");
            }
            let full = "Немає тривоги";
            let calm = if lx + 13.0 * s + measure(&dw, &f12, full) + 8.0 * s + count_w <= x1 { full } else { "Немає" };
            item(&mut lx, color(0, 0, 0, 0.0), true, calm);
            if !view.alerts.is_empty() && lx + count_w <= x1 + 12.0 * s {
                draw(&count_text, &f12, rect(x1 - count_w - 2.0 * s, ly * s, x1, (ly + 18.0) * s), &pal.secondary);
            }

            // 6. History.
            divider(lay.div1_y);
            draw("Сьогодні", &f12, rect(x0, lay.today_y * s, x1, (lay.today_y + 16.0) * s), &pal.secondary);
            if view.rows.is_empty() {
                draw("Змін не зафіксовано", &f13, rect(x0, lay.rows_y * s, x1, (lay.rows_y + ROW_H) * s), &pal.secondary);
            }
            for (i, (hm, alert, level)) in view.rows.iter().enumerate() {
                let ry = lay.rows_y + ROW_H * i as f32;
                draw(hm, &f13, rect(x0, ry * s, x0 + 48.0 * s, (ry + ROW_H) * s), &pal.secondary);
                let (ic, t) = if !*alert {
                    (Icon::Smile, "Відбій")
                } else if !view.has_levels {
                    (Icon::Urgent, "Тривога")
                } else if *level == Level::Yellow {
                    (Icon::Drone, "Жовта")
                } else if *level == Level::Red {
                    (Icon::Rocket, "Червона")
                } else {
                    (Icon::Urgent, "Тривога")
                };
                status_icon(ic, 14.0, x0 + 62.0 * s, (ry + 11.0) * s);
                draw(t, &f13, rect(x0 + 80.0 * s, ry * s, x1, (ry + ROW_H) * s), &pal.primary);
            }

            if let Err(e) = rt.EndDraw(None, None) {
                if e.code() == D2DERR_RECREATE_TARGET {
                    drop(brush);
                    gfx.target = None;
                }
            }
        }
    });
}

// ---------------------------------------------------------------------------------------
// Window procedure
// ---------------------------------------------------------------------------------------

fn xy(lp: LPARAM) -> (f32, f32) {
    ((lp.0 & 0xFFFF) as i16 as f32, ((lp.0 >> 16) & 0xFFFF) as i16 as f32)
}

fn inside(r: [f32; 4], x: f32, y: f32) -> bool {
    x >= r[0] && x < r[2] && y >= r[1] && y < r[3]
}

/// Topmost region under a client-pixel point (enclaves are last in `REGIONS`, so go in reverse).
fn hit_region(x: f32, y: f32) -> Option<usize> {
    let m = MAP.with(|c| c.get());
    let k = (m[2] - m[0]) / VIEW_W;
    if k <= 0.0 {
        return None;
    }
    let p = Vector2 { X: (x - m[0]) / k, Y: (y - m[1]) / k };
    GFX.with(|g| {
        let g = g.try_borrow().ok()?;
        let g = g.as_ref()?;
        (0..REGIONS.len()).rev().find(|&i| {
            g.geoms.get(i).and_then(|o| o.as_ref()).is_some_and(|geo| unsafe {
                geo.FillContainsPoint(p, None, 0.25).map(|b| b.as_bool()).unwrap_or(false)
            })
        })
    })
}

fn zone_at(x: f32, y: f32) -> Zone {
    if inside(LINK.with(|c| c.get()), x, y) {
        Zone::Link
    } else if inside(MAP.with(|c| c.get()), x, y) {
        hit_region(x, y).map_or(Zone::None, Zone::Region)
    } else {
        Zone::None
    }
}

fn wants_hand() -> bool {
    match ZONE.with(|c| c.get()) {
        Zone::Link => true,
        Zone::Region(i) => REGIONS[i].name != CRIMEA,
        Zone::None => false,
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_NCCALCSIZE if wp.0 != 0 => LRESULT(0),
        WM_NCHITTEST => LRESULT(HTCLIENT as isize),
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            BeginPaint(hwnd, &mut ps);
            render(hwnd);
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        WM_SIZE => {
            GFX.with(|g| {
                // try_borrow: render() holds GFX mutably and D2D calls there may re-enter via WM_SIZE.
                let Ok(g) = g.try_borrow() else { return };
                if let Some(rt) = g.as_ref().and_then(|g| g.target.as_ref()) {
                    let _ = rt.Resize(&D2D_SIZE_U {
                        width: (lp.0 & 0xFFFF).max(1) as u32,
                        height: ((lp.0 >> 16) & 0xFFFF).max(1) as u32,
                    });
                }
            });
            LRESULT(0)
        }
        WM_SHOWWINDOW => {
            // Pulse timer runs only while visible (and only if animations are on).
            if wp.0 != 0 && with_app(|a| a.config.animation) {
                SetTimer(Some(hwnd), PULSE_TIMER, PULSE_MS, None);
            } else {
                let _ = KillTimer(Some(hwnd), PULSE_TIMER);
            }
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        WM_TIMER if wp.0 == PULSE_TIMER => {
            if IsWindowVisible(hwnd).as_bool() {
                let _ = InvalidateRect(Some(hwnd), None, false);
            } else {
                let _ = KillTimer(Some(hwnd), PULSE_TIMER);
            }
            LRESULT(0)
        }
        WM_ACTIVATE => {
            if (wp.0 & 0xFFFF) as u32 == WA_INACTIVE && !IN_MENU.with(|c| c.get()) {
                CLOSED_AT.with(|c| c.set(GetTickCount64()));
                let _ = ShowWindow(hwnd, SW_HIDE);
            }
            LRESULT(0)
        }
        WM_KEYDOWN if wp.0 == VK_ESCAPE.0 as usize => {
            let _ = ShowWindow(hwnd, SW_HIDE);
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            if !TRACKING.with(|c| c.get()) {
                let mut tme = TRACKMOUSEEVENT {
                    cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                if TrackMouseEvent(&mut tme).is_ok() {
                    TRACKING.with(|c| c.set(true));
                }
            }
            let (x, y) = xy(lp);
            MOUSE.with(|c| c.set((x, y)));
            set_zone(hwnd, zone_at(x, y));
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            TRACKING.with(|c| c.set(false));
            set_zone(hwnd, Zone::None);
            LRESULT(0)
        }
        WM_SETCURSOR if wants_hand() => {
            SetCursor(LoadCursorW(None, IDC_HAND).ok());
            LRESULT(1)
        }
        WM_LBUTTONUP => {
            let (x, y) = xy(lp);
            match zone_at(x, y) {
                Zone::Link => {
                    ShellExecuteW(None, w!("open"), MAP_URL, None, None, SW_SHOWNORMAL);
                    let _ = ShowWindow(hwnd, SW_HIDE);
                }
                Zone::Region(i) if REGIONS[i].name != CRIMEA => {
                    menu::select_region(REGIONS[i].name);
                    refresh();
                }
                _ => {}
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

fn set_zone(h: HWND, z: Zone) {
    let changed = ZONE.with(|c| c.replace(z)) != z;
    // The hover label follows the cursor while over a region.
    if changed || matches!(z, Zone::Region(_)) {
        unsafe {
            let _ = InvalidateRect(Some(h), None, false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geoms() -> Vec<Option<ID2D1PathGeometry>> {
        let d2d: ID2D1Factory1 =
            unsafe { D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None) }.unwrap();
        build_geoms(&d2d)
    }

    fn contains(g: &Option<ID2D1PathGeometry>, x: f32, y: f32) -> bool {
        let g = g.as_ref().unwrap();
        unsafe { g.FillContainsPoint(Vector2 { X: x, Y: y }, None, 0.25) }.unwrap().as_bool()
    }

    /// Grid of sample points over the whole map.
    fn grid(step: usize) -> impl Iterator<Item = (f32, f32)> {
        (0..VIEW_H as usize)
            .step_by(step)
            .flat_map(move |y| (0..VIEW_W as usize).step_by(step).map(move |x| (x as f32, y as f32)))
    }

    #[test]
    fn every_region_builds_and_has_area() {
        let g = geoms();
        assert_eq!(g.len(), REGIONS.len());
        for (i, r) in REGIONS.iter().enumerate() {
            assert!(g[i].is_some(), "no geometry for {}", r.name);
            let hits = grid(10).filter(|&(x, y)| contains(&g[i], x, y)).count();
            assert!(hits > 0, "{} has no area on the map", r.name);
        }
    }

    #[test]
    fn enclave_names_exist() {
        for (o, i) in ENCLAVES {
            assert!(REGIONS.iter().any(|r| r.name == o), "missing {o}");
            assert!(REGIONS.iter().any(|r| r.name == i), "missing {i}");
        }
    }

    /// Kyiv city must be a hole in the oblast (the "yellow Kyiv" bug), same for Sevastopol.
    #[test]
    fn enclaves_are_cut_out_of_their_oblast() {
        let g = geoms();
        let idx = |n: &str| REGIONS.iter().position(|r| r.name == n).unwrap();
        for (o, i) in ENCLAVES {
            let (o, i) = (idx(o), idx(i));
            let city: Vec<_> = grid(2).filter(|&(x, y)| contains(&g[i], x, y)).collect();
            assert!(!city.is_empty());
            let leaked = city.iter().filter(|&&(x, y)| contains(&g[o], x, y)).count();
            // Only boundary samples may touch both (tolerance 0.25 view units).
            assert!(leaked * 20 < city.len(), "{}: {leaked}/{} city points still in oblast", REGIONS[o].name, city.len());
        }
    }

    /// No two regions overlap: the intersection area of any pair is ~0 (only shared borders).
    #[test]
    fn regions_do_not_overlap() {
        let d2d: ID2D1Factory1 =
            unsafe { D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None) }.unwrap();
        let g = build_geoms(&d2d);
        let area = |geo: &ID2D1Geometry| unsafe { geo.ComputeArea(None, 0.25) }.unwrap();
        let mut bad = Vec::new();
        for a in 0..g.len() {
            for b in a + 1..g.len() {
                let (ga, gb) = (g[a].as_ref().unwrap(), g[b].as_ref().unwrap());
                let both = unsafe {
                    let out = d2d.CreatePathGeometry().unwrap();
                    let sink = out.Open().unwrap();
                    ga.CombineWithGeometry(gb, D2D1_COMBINE_MODE_INTERSECT, None, 0.25, &sink).unwrap();
                    sink.Close().unwrap();
                    out
                };
                let ov = area(&both.cast().unwrap());
                let smaller = area(&ga.cast().unwrap()).min(area(&gb.cast().unwrap()));
                if ov > smaller * 0.001 {
                    bad.push(format!("{} / {}: {:.2}%", REGIONS[a].name, REGIONS[b].name, 100.0 * ov / smaller));
                }
            }
        }
        assert!(bad.is_empty(), "overlapping regions: {bad:?}");
    }

    #[test]
    fn hit_test_picks_city_over_oblast() {
        let g = geoms();
        let kyiv = REGIONS.iter().position(|r| r.name == "м. Київ").unwrap();
        let (x, y) = grid(2).find(|&(x, y)| contains(&g[kyiv], x, y)).unwrap();
        // Same loop as hit_region, without the window state.
        let top = (0..REGIONS.len()).rev().find(|&i| contains(&g[i], x, y));
        assert_eq!(top, Some(kyiv));
    }

    #[test]
    fn link_and_map_zones() {
        LINK.with(|c| c.set([100.0, 10.0, 200.0, 30.0]));
        MAP.with(|c| c.set([0.0, 50.0, 0.0, 50.0])); // empty map
        assert!(zone_at(150.0, 20.0) == Zone::Link);
        assert!(zone_at(200.0, 20.0) == Zone::None, "right edge is exclusive");
        assert!(zone_at(50.0, 20.0) == Zone::None);
    }
}
