//! The widget embedded into the Windows 11 taskbar.
//!
//! Windows 11 has no deskband API, so the widget is a layered child window of `Shell_TrayWnd`
//! (the TrafficMonitor technique). It is drawn with Direct2D into a 32bpp DIB and pushed with
//! `UpdateLayeredWindow` (per-pixel alpha), mimicking the native weather widget:
//! status icon + two lines of text, hover highlight, click = flyout, right click = menu.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;

use windows::core::{w, Interface, HRESULT, PCWSTR};
use windows_numerics::Vector2;
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::*;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::state::{short_region, with_app, Level, Status};
use crate::icons::{self, Icon};
use crate::{theme, util};

const CLASS_NAME: PCWSTR = w!("SirenBarTaskbarWidget");
const TIMER_PULSE: usize = 1;
const PULSE_MS: u32 = 40;
/// Length of one pulse cycle in milliseconds.
const PULSE_CYCLE_MS: f32 = 1400.0;
/// D2DERR_RECREATE_TARGET
const RECREATE_TARGET: HRESULT = HRESULT(0x8899000Cu32 as i32);

/// Off-screen drawing surface: memory DC + DIB section + Direct2D DC render target.
struct Surface {
    dc: HDC,
    bmp: HBITMAP,
    old: HGDIOBJ,
    w: i32,
    h: i32,
    rt: ID2D1DCRenderTarget,
}

impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.old);
            let _ = DeleteObject(self.bmp.into());
            let _ = DeleteDC(self.dc);
        }
    }
}

#[derive(Default)]
struct Widget {
    hwnd: HWND,
    main: HWND,
    hovered: bool,
    /// Pulse animation phase, 0..1.
    phase: f32,
    timer_on: bool,
    /// Last applied rect (client coords of the taskbar): x, y, w, h.
    rect: (i32, i32, i32, i32),
    dpi: u32,
    d2d: Option<ID2D1Factory1>,
    dwrite: Option<IDWriteFactory>,
    surface: Option<Surface>,
}

thread_local! {
    static W: RefCell<Widget> = RefCell::new(Widget::default());
    static FMT: RefCell<HashMap<(u8, u32, i32), IDWriteTextFormat>> = RefCell::new(HashMap::new());
    static CLASS_REGISTERED: RefCell<bool> = const { RefCell::new(false) };
}

/// The widget window, if it still exists.
fn alive() -> Option<HWND> {
    let h = W.with(|w| w.borrow().hwnd);
    (!h.0.is_null() && unsafe { IsWindow(Some(h)) }.as_bool()).then_some(h)
}

/// Called every 500 ms and on TaskbarCreated / setting changes. Ensures the widget window
/// exists, is a child of the current Shell_TrayWnd, is correctly positioned/sized for the
/// current DPI and on top of the taskbar's own content. `main` is the hidden main window
/// (owner for the context menu).
pub fn tick(main: HWND) {
    unsafe {
        let Ok(taskbar) = FindWindowW(w!("Shell_TrayWnd"), None) else { return };
        if taskbar.0.is_null() {
            return;
        }
        W.with(|w| w.borrow_mut().main = main);

        // (Re)create when missing or when explorer restarted (new taskbar window).
        let cur = alive();
        let stale = match cur {
            Some(h) => GetParent(h).map(|p| p != taskbar).unwrap_or(true),
            None => true,
        };
        if stale {
            if let Some(h) = cur {
                let _ = DestroyWindow(h); // WM_DESTROY clears our handle
            }
            if create(taskbar).is_none() {
                return;
            }
        }
        let Some(hwnd) = alive() else { return };

        // Geometry.
        let dpi = match GetDpiForWindow(taskbar) {
            0 => 96,
            d => d,
        };
        let scale = dpi as f32 / 96.0;
        let mut client = RECT::default();
        if GetClientRect(taskbar, &mut client).is_err() {
            return;
        }
        let (x, y, wd, ht) = compute_rect(taskbar, &client, scale);
        let changed = W.with(|w| {
            let mut s = w.borrow_mut();
            let c = s.rect != (x, y, wd, ht) || s.dpi != dpi;
            s.rect = (x, y, wd, ht);
            s.dpi = dpi;
            c
        });
        if changed {
            let _ = SetWindowPos(hwnd, None, x, y, wd, ht, SWP_NOZORDER | SWP_NOACTIVATE);
            render();
        }

        // Stay above the XAML taskbar content (skip if already topmost).
        if GetWindow(hwnd, GW_HWNDPREV).map(|h| !h.0.is_null()).unwrap_or(false) {
            let _ = SetWindowPos(hwnd, Some(HWND_TOP), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
        sync_timer(hwnd);
    }
}

/// Redraw with the current state.
pub fn invalidate() {
    if let Some(h) = alive() {
        render();
        sync_timer(h);
    }
}

/// Screen rectangle of the widget, if it exists (used to anchor the flyout).
pub fn screen_rect() -> Option<RECT> {
    let h = alive()?;
    let mut r = RECT::default();
    unsafe { GetWindowRect(h, &mut r).ok()? };
    Some(r)
}

/// Destroy the widget window (on exit).
pub fn destroy() {
    if let Some(h) = alive() {
        unsafe {
            let _ = DestroyWindow(h);
        }
    }
}

/// Widget rectangle in taskbar client coordinates.
fn compute_rect(taskbar: HWND, client: &RECT, scale: f32) -> (i32, i32, i32, i32) {
    let px = |v: f32| (v * scale).round() as i32;
    let wd = px(150.0);
    let ht = client.bottom - client.top;
    let x = if taskbar_centered() {
        px(4.0)
    } else {
        unsafe {
            let tray = FindWindowExW(Some(taskbar), None, w!("TrayNotifyWnd"), None).unwrap_or_default();
            let mut tr = RECT::default();
            let mut tb = RECT::default();
            if !tray.0.is_null() && GetWindowRect(tray, &mut tr).is_ok() && GetWindowRect(taskbar, &mut tb).is_ok() {
                tr.left - tb.left - wd - px(8.0)
            } else {
                client.right - wd - px(320.0)
            }
        }
    };
    (x.max(0), 0, wd, ht.max(1))
}

/// HKCU\...\Explorer\Advanced "TaskbarAl": 0 = left, 1 (default) = centered.
fn taskbar_centered() -> bool {
    let mut value: u32 = 1;
    let mut size = size_of::<u32>() as u32;
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Advanced"),
            w!("TaskbarAl"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut c_void),
            Some(&mut size),
        )
    };
    r.is_err() || value != 0
}

/// Create the widget as a layered child of `taskbar`.
fn create(taskbar: HWND) -> Option<HWND> {
    unsafe {
        let hinst: HINSTANCE = GetModuleHandleW(None).ok()?.into();
        if !CLASS_REGISTERED.with(|c| *c.borrow()) {
            let wc = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(wndproc),
                hInstance: hinst,
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                lpszClassName: CLASS_NAME,
                ..Default::default()
            };
            if RegisterClassExW(&wc) == 0 {
                return None;
            }
            CLASS_REGISTERED.with(|c| *c.borrow_mut() = true);
        }

        let ex = WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
        let child = WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS;
        let hwnd = match CreateWindowExW(ex, CLASS_NAME, w!(""), child, 0, 0, 1, 1, Some(taskbar), None, Some(hinst), None) {
            Ok(h) => h,
            Err(_) => {
                // Fallback: create as popup, then reparent and switch to child style.
                let h = CreateWindowExW(ex, CLASS_NAME, w!(""), WS_POPUP, 0, 0, 1, 1, None, None, Some(hinst), None).ok()?;
                if SetParent(h, Some(taskbar)).is_err() {
                    let _ = DestroyWindow(h);
                    return None;
                }
                SetWindowLongPtrW(h, GWL_STYLE, child.0 as isize);
                let _ = ShowWindow(h, SW_SHOWNOACTIVATE);
                h
            }
        };
        W.with(|w| {
            let mut s = w.borrow_mut();
            s.hwnd = hwnd;
            s.hovered = false;
            s.rect = (0, 0, 0, 0); // force a resize + render on this tick
            s.dpi = 0;
            s.surface = None;
        });
        Some(hwnd)
    }
}

/// Run the pulse timer only while an animated alert is showing.
fn sync_timer(hwnd: HWND) {
    let want = with_app(|a| a.status == Status::Alert && a.config.animation);
    let have = W.with(|w| w.borrow().timer_on);
    if want == have {
        return;
    }
    unsafe {
        if want {
            SetTimer(Some(hwnd), TIMER_PULSE, PULSE_MS, None);
        } else {
            let _ = KillTimer(Some(hwnd), TIMER_PULSE);
        }
    }
    W.with(|w| {
        let mut s = w.borrow_mut();
        s.timer_on = want;
        if !want {
            s.phase = 0.0;
        }
    });
}

/// Alert red (slightly coral).
const RED: (u8, u8, u8) = (0xF0, 0x50, 0x4A);
const YELLOW: (u8, u8, u8) = (0xFF, 0xB9, 0x00);
const GREEN: (u8, u8, u8) = (0x16, 0xC6, 0x0C);
const GRAY: (u8, u8, u8) = (0x8A, 0x8A, 0x8A);

/// Everything needed for one frame, copied out of the shared state.
struct Frame {
    status: Status,
    icon: Icon,
    /// Pulse ring colour.
    hue: (u8, u8, u8),
    line1: String,
    line2: String,
    /// `line2` plus " · частково" (only when the alert is partial).
    line2_partial: Option<String>,
    animation: bool,
}

fn snapshot() -> Frame {
    with_app(|a| {
        let age = a.since.map(|s| util::fmt_short((util::now_unix() - s).max(0)));
        let line1 = match a.status {
            Status::Alert => {
                let label = match (a.has_levels, a.level) {
                    (true, Level::Red) => "Ракети",
                    (true, Level::Yellow) => "Дрони",
                    _ => "Тривога",
                };
                match &age {
                    Some(t) => format!("{label} · {t}"),
                    None => label.to_string(),
                }
            }
            Status::Clear => match &age {
                Some(t) => format!("Відбій · {t}"),
                None => "Немає тривоги".to_string(),
            },
            Status::Offline => "Немає зв'язку".to_string(),
            Status::Unknown => "Завантаження…".to_string(),
        };
        let (icon, hue) = match a.status {
            Status::Alert => match (a.has_levels, a.level) {
                (true, Level::Yellow) => (Icon::Drone, YELLOW),
                (true, Level::Red) => (Icon::Rocket, RED),
                _ => (Icon::Urgent, RED),
            },
            Status::Clear => (Icon::Smile, GREEN),
            Status::Offline | Status::Unknown => (Icon::Unknown, GRAY),
        };
        let line2 = short_region(&a.config.region);
        let line2_partial = (a.status == Status::Alert && a.partial).then(|| format!("{line2} · частково"));
        Frame { status: a.status, icon, hue, line1, line2, line2_partial, animation: a.config.animation }
    })
}

/// Render the current state into the layered window.
fn render() {
    let Some(hwnd) = alive() else { return };
    let (wd, ht, dpi, hovered, phase) = W.with(|w| {
        let s = w.borrow();
        (s.rect.2, s.rect.3, s.dpi, s.hovered, s.phase)
    });
    if wd <= 0 || ht <= 0 {
        return;
    }
    let frame = snapshot();
    let light = theme::taskbar_is_light();

    // Take the heavy objects out so no borrow is held while drawing / calling Win32.
    let (d2d, dwrite, mut surface) = W.with(|w| {
        let mut s = w.borrow_mut();
        (s.d2d.clone(), s.dwrite.clone(), s.surface.take())
    });
    let d2d = d2d.or_else(|| unsafe { D2D1CreateFactory::<ID2D1Factory1>(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).ok() });
    let dwrite = dwrite.or_else(|| unsafe { DWriteCreateFactory::<IDWriteFactory>(DWRITE_FACTORY_TYPE_SHARED).ok() });

    if let (Some(d2d_f), Some(dw_f)) = (&d2d, &dwrite) {
        if surface.as_ref().map(|s| s.w != wd || s.h != ht).unwrap_or(true) {
            surface = Surface::new(d2d_f, wd, ht);
        }
        if let Some(sf) = &surface {
            let scale = dpi.max(96) as f32 / 96.0;
            match unsafe { draw(sf, d2d_f, dw_f, &frame, scale, light, hovered, phase) } {
                Ok(()) => unsafe { present(hwnd, sf) },
                Err(e) if e.code() == RECREATE_TARGET => surface = None,
                Err(_) => {}
            }
        }
    }
    W.with(|w| {
        let mut s = w.borrow_mut();
        s.d2d = d2d;
        s.dwrite = dwrite;
        s.surface = surface;
    });
}

impl Surface {
    /// DIB section of `w`x`h` plus a render target bound to it.
    fn new(factory: &ID2D1Factory1, w: i32, h: i32) -> Option<Surface> {
        unsafe {
            let dc = CreateCompatibleDC(None);
            if dc.is_invalid() {
                return None;
            }
            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h, // top-down
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut c_void = std::ptr::null_mut();
            let Ok(bmp) = CreateDIBSection(Some(dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) else {
                let _ = DeleteDC(dc);
                return None;
            };
            let old = SelectObject(dc, bmp.into());
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: 96.0,
                dpiY: 96.0,
                usage: D2D1_RENDER_TARGET_USAGE_NONE,
                minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
            };
            match factory.CreateDCRenderTarget(&props) {
                Ok(rt) => Some(Surface { dc, bmp, old, w, h, rt }),
                Err(_) => {
                    SelectObject(dc, old);
                    let _ = DeleteObject(bmp.into());
                    let _ = DeleteDC(dc);
                    None
                }
            }
        }
    }
}

fn color(r: u8, g: u8, b: u8, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r: r as f32 / 255.0, g: g as f32 / 255.0, b: b as f32 / 255.0, a }
}

fn pt(x: f32, y: f32) -> Vector2 {
    Vector2 { X: x, Y: y }
}

/// Draw one frame into the surface's DIB.
#[allow(clippy::too_many_arguments)]
unsafe fn draw(
    sf: &Surface,
    d2d: &ID2D1Factory1,
    dw: &IDWriteFactory,
    f: &Frame,
    s: f32,
    light: bool,
    hovered: bool,
    phase: f32,
) -> windows::core::Result<()> {
    let rt = &sf.rt;
    let (wf, hf) = (sf.w as f32, sf.h as f32);
    rt.BindDC(sf.dc, &RECT { left: 0, top: 0, right: sf.w, bottom: sf.h })?;
    rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
    let brush = rt.CreateSolidColorBrush(&color(0, 0, 0, 1.0), None)?;

    rt.BeginDraw();
    // Alpha 1/255 instead of 0 so the whole rect receives mouse input.
    rt.Clear(Some(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 1.0 / 255.0 }));

    if hovered {
        brush.SetColor(&if light { color(0, 0, 0, 0.05) } else { color(255, 255, 255, 0.07) });
        let rr = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F { left: 2.0 * s, top: 4.0 * s, right: wf - 2.0 * s, bottom: hf - 4.0 * s },
            radiusX: 4.0 * s,
            radiusY: 4.0 * s,
        };
        rt.FillRoundedRectangle(&rr, &brush);
    }

    // Icon: monochrome Fluent glyph tinted by state, pulse ring behind it.
    let (cx, cy, r) = (10.0 * s + 10.0 * s, hf / 2.0, 10.0 * s);
    let fill = color(f.hue.0, f.hue.1, f.hue.2, 1.0);
    if f.status == Status::Alert && f.animation {
        let ring_r = r + 2.0 * s + 5.0 * s * phase;
        brush.SetColor(&D2D1_COLOR_F { a: 0.6 * (1.0 - phase), ..fill });
        rt.DrawEllipse(&D2D1_ELLIPSE { point: pt(cx, cy), radiusX: ring_r, radiusY: ring_r }, &brush, 1.5 * s, None);
    }
    if let Ok(factory) = d2d.cast::<ID2D1Factory>() {
        let rect = D2D_RECT_F { left: cx - r, top: cy - r, right: cx + r, bottom: cy + r };
        icons::draw(rt, &factory, f.icon, rect, f.icon.tint(light));
    }

    // Text: two lines centered vertically as a block.
    let tx = cx + r + 8.0 * s;
    let max_w = (wf - tx - 6.0 * s).max(1.0);
    let (c1, c2) = if light {
        (color(0, 0, 0, 0.9), color(0, 0, 0, 0.62))
    } else {
        (color(255, 255, 255, 1.0), color(255, 255, 255, 0.72))
    };
    let l1 = text_layout(dw, &f.line1, 12.0 * s, DWRITE_FONT_WEIGHT_SEMI_BOLD, max_w, hf);
    let mut l2 = text_layout(dw, &f.line2, 12.0 * s, DWRITE_FONT_WEIGHT_NORMAL, max_w, hf);
    if let Some(full) = &f.line2_partial {
        // Append " · частково" only if the whole string fits.
        if let Some(wide) = text_layout(dw, full, 12.0 * s, DWRITE_FONT_WEIGHT_NORMAL, 4000.0, hf) {
            let mut m = DWRITE_TEXT_METRICS::default();
            if wide.GetMetrics(&mut m).is_ok() && m.widthIncludingTrailingWhitespace <= max_w {
                l2 = text_layout(dw, full, 12.0 * s, DWRITE_FONT_WEIGHT_NORMAL, max_w, hf);
            }
        }
    }
    let height = |l: &Option<IDWriteTextLayout>| {
        l.as_ref()
            .and_then(|l| {
                let mut m = DWRITE_TEXT_METRICS::default();
                l.GetMetrics(&mut m).ok().map(|_| m.height)
            })
            .unwrap_or(0.0)
    };
    let (h1, h2) = (height(&l1), height(&l2));
    let top = ((hf - (h1 + h2)) / 2.0).max(0.0);
    if let Some(l) = &l1 {
        brush.SetColor(&c1);
        rt.DrawTextLayout(pt(tx, top), l, &brush, D2D1_DRAW_TEXT_OPTIONS_NONE);
    }
    if let Some(l) = &l2 {
        brush.SetColor(&c2);
        rt.DrawTextLayout(pt(tx, top + h1), l, &brush, D2D1_DRAW_TEXT_OPTIONS_NONE);
    }

    rt.EndDraw(None, None)
}

const FAM_TEXT: u8 = 0;

/// Cached single-line text format.
unsafe fn cached_format(dw: &IDWriteFactory, fam: u8, size: f32, weight: DWRITE_FONT_WEIGHT) -> Option<IDWriteTextFormat> {
    let key = (fam, size.to_bits(), weight.0);
    if let Some(f) = FMT.with(|m| m.borrow().get(&key).cloned()) {
        return Some(f);
    }
    let _ = fam;
    let family = w!("Segoe UI Variable Text");
    let fmt = dw
        .CreateTextFormat(family, None, weight, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, size, w!("uk-ua"))
        .ok()?;
    let _ = fmt.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
    FMT.with(|m| m.borrow_mut().insert(key, fmt.clone()));
    Some(fmt)
}

/// Single-line, ellipsis-trimmed text layout.
unsafe fn text_layout(
    dw: &IDWriteFactory,
    text: &str,
    size: f32,
    weight: DWRITE_FONT_WEIGHT,
    max_w: f32,
    max_h: f32,
) -> Option<IDWriteTextLayout> {
    let fmt = cached_format(dw, FAM_TEXT, size, weight)?;
    let wide: Vec<u16> = text.encode_utf16().collect();
    let layout = dw.CreateTextLayout(&wide, &fmt, max_w, max_h).ok()?;
    let trimming = DWRITE_TRIMMING { granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER, delimiter: 0, delimiterCount: 0 };
    if let Ok(sign) = dw.CreateEllipsisTrimmingSign(&fmt) {
        let _ = layout.SetTrimming(&trimming, &sign);
    }
    Some(layout)
}

/// Push the DIB to the layered window.
unsafe fn present(hwnd: HWND, sf: &Surface) {
    let size = SIZE { cx: sf.w, cy: sf.h };
    let src = POINT { x: 0, y: 0 };
    let blend = BLENDFUNCTION {
        BlendOp: AC_SRC_OVER as u8,
        BlendFlags: 0,
        SourceConstantAlpha: 255,
        AlphaFormat: AC_SRC_ALPHA as u8,
    };
    let _ = UpdateLayeredWindow(hwnd, None, None, Some(&size), Some(sf.dc), Some(&src), COLORREF(0), Some(&blend), ULW_ALPHA);
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEMOVE => {
            let first = W.with(|w| !std::mem::replace(&mut w.borrow_mut().hovered, true));
            if first {
                let mut tme = TRACKMOUSEEVENT {
                    cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                let _ = TrackMouseEvent(&mut tme);
                render();
            }
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            W.with(|w| w.borrow_mut().hovered = false);
            render();
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            crate::flyout::toggle();
            LRESULT(0)
        }
        WM_RBUTTONUP => {
            let main = W.with(|w| w.borrow().main);
            crate::menu::show(main);
            LRESULT(0)
        }
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_SETCURSOR => {
            SetCursor(LoadCursorW(None, IDC_ARROW).ok());
            LRESULT(1)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_TIMER if wp.0 == TIMER_PULSE => {
            W.with(|w| {
                let mut s = w.borrow_mut();
                s.phase = (s.phase + PULSE_MS as f32 / PULSE_CYCLE_MS).fract();
            });
            render();
            LRESULT(0)
        }
        WM_DESTROY => {
            let _ = KillTimer(Some(hwnd), TIMER_PULSE);
            // Only clear if the handle is ours (a replacement may already be registered).
            let _ = W.try_with(|w| {
                if let Ok(mut s) = w.try_borrow_mut() {
                    if s.hwnd == hwnd {
                        s.hwnd = HWND::default();
                        s.timer_on = false;
                        s.hovered = false;
                        s.surface = None;
                    }
                }
            });
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}
