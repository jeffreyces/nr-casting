//! Where the panel window is, and how big: the screen, the saved geometry, and the fit.
//!
//! Separate from the panel because none of it needs `App` or a device, and all of it is arithmetic
//! about a `HWND` that the window procedure and the frame loop both have to agree on.

use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClientRect, GetSystemMetrics, GetWindowRect, SetWindowPos, SM_CXSCREEN, SM_CYSCREEN,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOZORDER, SWP_NOSIZE,
};

use nr_casting_core::{DESIGN_HEIGHT, DESIGN_WIDTH, MAX_UI_SCALE, MIN_UI_SCALE};

/// The scale to use when nothing has been chosen: the screen's height over 1080.
///
/// The panel is its own window, so this is the desktop's own resolution — under Proton that is the
/// virtual desktop, which is what the drawn window is measured against anyway.
pub(crate) fn monitor_scale() -> f32 {
    let height = unsafe { GetSystemMetrics(SM_CYSCREEN) } as f32;
    if height <= 0.0 {
        return 1.0;
    }
    (height / 1080.0).clamp(MIN_UI_SCALE, MAX_UI_SCALE)
}

/// Resize the panel window so the design fits at `scale`.
///
/// Called from the frame loop, never from inside the `App` borrow: `SetWindowPos` delivers
/// `WM_SIZE` synchronously and the window procedure needs that same `RefCell`.
pub(crate) fn size_window(hwnd: HWND, scale: f32) {
    let (width, height) = design_size(scale);
    // NOZORDER keeps the panel topmost; NOACTIVATE keeps focus where it is (the panel owns it
    // already, and stealing it would close the panel through WM_ACTIVATE).
    let _ = unsafe {
        SetWindowPos(
            hwnd,
            None,
            0,
            0,
            width as i32,
            height as i32,
            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        )
    };
}

/// The window size that holds the design at `scale`.
pub(crate) fn design_size(scale: f32) -> (u32, u32) {
    (
        (DESIGN_WIDTH * scale).round() as u32,
        (DESIGN_HEIGHT * scale).round() as u32,
    )
}

/// The panel's rectangle, in screen pixels.
///
/// The window and the screen are read in one place here: every caller wanted the same rectangle
/// or the same two `GetSystemMetrics` calls, and each was free to disagree about the clamping.
pub(crate) fn window_rect(hwnd: HWND) -> Option<RECT> {
    let mut rect = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut rect) }.ok()?;
    Some(rect)
}

/// The panel's current outer size, in pixels.
pub(crate) fn window_size(hwnd: HWND) -> Option<(u32, u32)> {
    let rect = window_rect(hwnd)?;
    let width = (rect.right - rect.left).max(1) as u32;
    let height = (rect.bottom - rect.top).max(1) as u32;
    Some((width, height))
}

/// The primary screen, or `None` when there is no display to measure.
pub(crate) fn screen_size() -> Option<(i32, i32)> {
    let width = unsafe { GetSystemMetrics(SM_CXSCREEN) };
    let height = unsafe { GetSystemMetrics(SM_CYSCREEN) };
    (width > 0 && height > 0).then_some((width, height))
}

/// `(x, y)` moved so at least `visible` pixels of the window stay on the desktop.
///
/// A saved corner from a monitor that is no longer attached would otherwise put the panel
/// somewhere the player cannot reach it.
pub(crate) fn clamp_to_screen(x: i32, y: i32, visible: (i32, i32)) -> (i32, i32) {
    let Some((screen_width, screen_height)) = screen_size() else {
        return (x, y);
    };
    (
        x.clamp(0, (screen_width - visible.0).max(0)),
        y.clamp(0, (screen_height - visible.1).max(0)),
    )
}

/// The panel window's current top-left corner, in screen pixels.
pub(crate) fn window_pos(hwnd: HWND) -> Option<(i32, i32)> {
    let rect = window_rect(hwnd)?;
    Some((rect.left, rect.top))
}

/// Put the panel back in the middle of the screen, clamped so no part of it leaves the desktop.
pub(crate) fn center_window(hwnd: HWND) {
    let Some(((width, height), (screen_width, screen_height))) =
        window_size(hwnd).zip(screen_size())
    else {
        return;
    };
    // The whole window, since the point of centring is that all of it is visible.
    let (x, y) = clamp_to_screen(
        ((screen_width - width as i32) / 2).max(0),
        ((screen_height - height as i32) / 2).max(0),
        (width as i32, height as i32),
    );
    let _ = unsafe {
        SetWindowPos(hwnd, None, x, y, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE)
    };
}

/// Resize the window so its client area is exactly as tall as the layout used.
///
/// The non-client height (the WS_THICKFRAME sizing border) has to be added back, or the content
/// would lose those few pixels off the bottom every time the panel is fitted.
pub(crate) fn fit_window_height(hwnd: HWND, content_height: f32) {
    let mut outer = RECT::default();
    let mut client = RECT::default();
    let measured = unsafe {
        (
            GetWindowRect(hwnd, &mut outer),
            GetClientRect(hwnd, &mut client),
        )
    };
    if measured.0.is_err() || measured.1.is_err() {
        return;
    }
    let frame_height = (outer.bottom - outer.top) - (client.bottom - client.top);
    let height = content_height.ceil() as i32 + frame_height.max(0);
    let width = outer.right - outer.left;
    let _ = unsafe {
        SetWindowPos(
            hwnd,
            None,
            0,
            0,
            width,
            height,
            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        )
    };
}

/// Fit a saved size to this screen: a panel saved on a 4K TV must not open bigger than a 1080p one.
pub(crate) fn fit_to_screen(size: (u32, u32)) -> (u32, u32) {
    let screen_width = unsafe { GetSystemMetrics(SM_CXSCREEN) }.max(0) as u32;
    let screen_height = unsafe { GetSystemMetrics(SM_CYSCREEN) }.max(0) as u32;
    if screen_width == 0 || screen_height == 0 {
        return size;
    }
    // Leave room for the spot the panel opens at (80,80), so a full-screen-sized panel does not
    // open with its bottom edge below the screen; never below what the layout needs.
    let max_width = screen_width.saturating_sub(160).max(DESIGN_WIDTH as u32);
    let max_height = screen_height.saturating_sub(160).max(DESIGN_HEIGHT as u32);
    (size.0.min(max_width), size.1.min(max_height))
}

/// The size the panel window is created at.
///
/// A pinned `ui_scale` decides it (so the layout cannot be clipped), then a size the player saved,
/// then the screen: the design scaled by the screen height over 1080, which is what makes a 4K
/// display open at 2x instead of at a 900x620 postage stamp.
pub(crate) fn initial_window_size() -> (i32, i32) {
    let config = crate::config::load();
    // A size the player dragged is the window they want, whatever the text scale is: a pinned
    // `ui_scale` only fixes the text, it does not get to override the window.
    let (width, height) = match config.panel_size {
        Some(saved) => fit_to_screen(saved),
        None => design_size(config.ui_scale.unwrap_or_else(monitor_scale)),
    };
    (width as i32, height as i32)
}

/// The corner the panel opens at: the one the player left it at, or the default spot.
///
/// A saved corner from a monitor that is no longer attached would otherwise open the panel somewhere
/// the player cannot see it, so it is clamped to leave the title band reachable.
pub(crate) fn initial_window_pos() -> (i32, i32) {
    let (x, y) = crate::config::load().panel_pos.unwrap_or((80, 80));
    // Enough of the title band left on screen to grab and drag.
    clamp_to_screen(x, y, (200, 120))
}
