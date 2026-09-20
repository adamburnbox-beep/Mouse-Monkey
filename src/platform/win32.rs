//! Windows driver: a layered, click-through overlay window plus low-level
//! global input hooks.
//!
//! The window uses `WS_EX_LAYERED | WS_EX_TRANSPARENT` and is painted through
//! `UpdateLayeredWindow` from a 32-bit DIB section. That gives real per-pixel
//! alpha — no colour keying, no halo around the sprite — and `WS_EX_TRANSPARENT`
//! means every click passes through to the window underneath. Clicks on the
//! monkey still reach the companion because the low-level mouse hook sees input
//! before it is routed to a window.

#![cfg(target_os = "windows")]

use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::OnceLock;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{
    BOOL, COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, EnumDisplayMonitors,
    GetMonitorInfoW, SelectObject, AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
    BLENDFUNCTION, DIB_RGB_COLORS, HBITMAP, HDC, HGDIOBJ, HMONITOR, MONITORINFO,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetCursorPos,
    PeekMessageW, PostQuitMessage, RegisterClassExW, SetWindowsHookExW, ShowWindow,
    TranslateMessage, UnhookWindowsHookEx, UpdateLayeredWindow, HHOOK, KBDLLHOOKSTRUCT, MSG,
    MSLLHOOKSTRUCT, PM_REMOVE, SW_SHOWNOACTIVATE, ULW_ALPHA, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_CLOSE,
    WM_DESTROY, WM_DISPLAYCHANGE, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEMOVE, WM_QUIT, WM_RBUTTONDOWN, WM_RBUTTONUP,
    WM_SYSKEYDOWN, WM_SYSKEYUP, WNDCLASSEXW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

use crate::config::AppConfig;
use crate::error::{Error, Result};
use crate::geometry::{Rect, ScreenLayout, Vec2};
use crate::platform::{Frame, InputEvent, InputScope, MouseButton, PlatformDriver};

/// Window class name, NUL terminated for `PCWSTR`.
const WINDOW_CLASS: &[u16] = &[
    b'M' as u16,
    b'o' as u16,
    b'n' as u16,
    b'k' as u16,
    b'e' as u16,
    b'y' as u16,
    b'C' as u16,
    b'o' as u16,
    b'm' as u16,
    b'p' as u16,
    b'a' as u16,
    b'n' as u16,
    b'i' as u16,
    b'o' as u16,
    b'n' as u16,
    b'W' as u16,
    b'n' as u16,
    b'd' as u16,
    0,
];

/// Hook callbacks are plain `extern "system"` functions with no user data, so
/// the channel they publish to has to be reachable from a static. It is written
/// once, before any hook is installed.
static EVENT_SENDER: OnceLock<crossbeam_channel::Sender<InputEvent>> = OnceLock::new();
/// Hook handles, as raw pointers, so the callbacks can pass them to
/// `CallNextHookEx` without a lock.
static MOUSE_HOOK: AtomicIsize = AtomicIsize::new(0);
static KEYBOARD_HOOK: AtomicIsize = AtomicIsize::new(0);
/// Set by the window procedure, read by the engine loop.
static RUNNING: AtomicBool = AtomicBool::new(true);
/// Set by `WM_DISPLAYCHANGE`; cleared once the new layout has been cached.
static DISPLAY_CHANGED: AtomicBool = AtomicBool::new(false);

fn hook_from(atomic: &AtomicIsize) -> HHOOK {
    HHOOK(atomic.load(Ordering::SeqCst) as *mut core::ffi::c_void)
}

fn publish(event: InputEvent) {
    if let Some(sender) = EVENT_SENDER.get() {
        let _ = sender.send(event);
    }
}

/// Low-level mouse hook: sees the whole desktop, including clicks that land on
/// other windows (FR-02, FR-03).
unsafe extern "system" fn mouse_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        // SAFETY: for `code >= 0` Windows guarantees `lparam` points at an
        // `MSLLHOOKSTRUCT` that stays valid for the duration of this call.
        let data = unsafe { *(lparam.0 as *const MSLLHOOKSTRUCT) };
        let position = Vec2::new(data.pt.x as f32, data.pt.y as f32);
        let event = match wparam.0 as u32 {
            WM_MOUSEMOVE => Some(InputEvent::MouseMove { position }),
            WM_LBUTTONDOWN => Some(InputEvent::MouseDown {
                position,
                button: MouseButton::Left,
            }),
            WM_LBUTTONUP => Some(InputEvent::MouseUp {
                position,
                button: MouseButton::Left,
            }),
            WM_RBUTTONDOWN => Some(InputEvent::MouseDown {
                position,
                button: MouseButton::Right,
            }),
            WM_RBUTTONUP => Some(InputEvent::MouseUp {
                position,
                button: MouseButton::Right,
            }),
            WM_MBUTTONDOWN => Some(InputEvent::MouseDown {
                position,
                button: MouseButton::Middle,
            }),
            WM_MBUTTONUP => Some(InputEvent::MouseUp {
                position,
                button: MouseButton::Middle,
            }),
            _ => None,
        };
        if let Some(event) = event {
            publish(event);
        }
    }
    // SAFETY: passing the hook on is required; the handle is the one we installed.
    unsafe { CallNextHookEx(hook_from(&MOUSE_HOOK), code, wparam, lparam) }
}

/// Low-level keyboard hook. Key codes are only used to notice that *a* key was
/// pressed (FR-06); nothing is logged or interpreted as text.
unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        // SAFETY: as above, `lparam` points at a valid `KBDLLHOOKSTRUCT`.
        let data = unsafe { *(lparam.0 as *const KBDLLHOOKSTRUCT) };
        let key_code = data.vkCode;
        let event = match wparam.0 as u32 {
            WM_KEYDOWN | WM_SYSKEYDOWN => Some(InputEvent::KeyDown { key_code }),
            WM_KEYUP | WM_SYSKEYUP => Some(InputEvent::KeyUp { key_code }),
            _ => None,
        };
        if let Some(event) = event {
            publish(event);
        }
    }
    // SAFETY: as above.
    unsafe { CallNextHookEx(hook_from(&KEYBOARD_HOOK), code, wparam, lparam) }
}

/// Window procedure. The overlay has no UI, so it only needs lifecycle and
/// display-change messages.
unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_CLOSE => {
            RUNNING.store(false, Ordering::SeqCst);
            publish(InputEvent::CloseRequested);
            // SAFETY: `hwnd` is this window, which is still alive here.
            let _ = unsafe { DestroyWindow(hwnd) };
            LRESULT(0)
        }
        WM_DESTROY => {
            RUNNING.store(false, Ordering::SeqCst);
            // SAFETY: no preconditions.
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        WM_DISPLAYCHANGE => {
            DISPLAY_CHANGED.store(true, Ordering::SeqCst);
            LRESULT(0)
        }
        // SAFETY: forwarding unhandled messages is the documented contract.
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

/// Callback for `EnumDisplayMonitors`, collecting one rectangle per monitor.
unsafe extern "system" fn monitor_enum(
    monitor: HMONITOR,
    _dc: HDC,
    _clip: *mut RECT,
    lparam: LPARAM,
) -> BOOL {
    // SAFETY: `lparam` is the `&mut Vec<Rect>` handed to `EnumDisplayMonitors`
    // below, and the enumeration is single threaded.
    let monitors = unsafe { &mut *(lparam.0 as *mut Vec<Rect>) };
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: `info.cbSize` is set as the API requires.
    if unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
        let bounds = info.rcMonitor;
        monitors.push(Rect::new(
            bounds.left,
            bounds.top,
            (bounds.right - bounds.left).max(0) as u32,
            (bounds.bottom - bounds.top).max(0) as u32,
        ));
    } else {
        log::warn!("GetMonitorInfoW failed for one monitor; it will be ignored");
    }
    BOOL(1)
}

/// Enumerate the monitors that make up the desktop (FR-09).
fn enumerate_monitors() -> Vec<Rect> {
    let mut monitors: Vec<Rect> = Vec::new();
    // SAFETY: the callback matches the expected signature and `lparam` points
    // at `monitors`, which outlives the call.
    unsafe {
        let _ = EnumDisplayMonitors(
            None,
            None,
            Some(monitor_enum),
            LPARAM(std::ptr::addr_of_mut!(monitors) as isize),
        );
    }
    monitors
}

/// A 32-bit top-down DIB section the sprite is drawn into before being handed
/// to `UpdateLayeredWindow`.
struct LayeredCanvas {
    dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
    pixels: *mut u8,
    width: i32,
    height: i32,
}

impl LayeredCanvas {
    fn new(width: i32, height: i32) -> Result<Self> {
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                // Negative height requests a top-down bitmap, so row 0 is the
                // top row and the blit maths matches every other backend.
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };

        // SAFETY: the handles are created here and released in `Drop`; the
        // bitmap description above matches the pointer we ask for.
        unsafe {
            let dc = CreateCompatibleDC(None);
            if dc.is_invalid() {
                return Err(Error::platform("CreateCompatibleDC failed"));
            }
            let mut pixels: *mut core::ffi::c_void = std::ptr::null_mut();
            let bitmap = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut pixels, None, 0)
                .map_err(|error| Error::platform(format!("CreateDIBSection failed: {error}")))?;
            if pixels.is_null() {
                let _ = DeleteObject(bitmap);
                let _ = DeleteDC(dc);
                return Err(Error::platform("CreateDIBSection returned no pixel buffer"));
            }
            let previous = SelectObject(dc, bitmap);
            Ok(Self {
                dc,
                bitmap,
                previous,
                pixels: pixels.cast(),
                width,
                height,
            })
        }
    }

    /// The pixel buffer as BGRA bytes, premultiplied.
    fn pixels_mut(&mut self) -> &mut [u8] {
        // SAFETY: `CreateDIBSection` allocated exactly `width * height * 4`
        // bytes and we own the bitmap for the lifetime of `self`.
        unsafe {
            std::slice::from_raw_parts_mut(self.pixels, (self.width * self.height * 4) as usize)
        }
    }
}

impl Drop for LayeredCanvas {
    fn drop(&mut self) {
        // SAFETY: each handle was created in `new` and is released exactly once.
        unsafe {
            SelectObject(self.dc, self.previous);
            let _ = DeleteObject(self.bitmap);
            let _ = DeleteDC(self.dc);
        }
    }
}

/// The Windows platform driver.
pub struct Win32Driver {
    hwnd: HWND,
    canvas: Option<LayeredCanvas>,
    size: (i32, i32),
    origin: (i32, i32),
    monitors: Vec<Rect>,
    events: crossbeam_channel::Receiver<InputEvent>,
    input_scope: InputScope,
    /// Rectangle painted last frame, cleared before the next one is drawn.
    dirty: Option<(i32, i32, i32, i32)>,
}

impl std::fmt::Debug for Win32Driver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Win32Driver")
            .field("size", &self.size)
            .field("origin", &self.origin)
            .field("monitors", &self.monitors)
            .field("input_scope", &self.input_scope)
            .finish_non_exhaustive()
    }
}

impl Win32Driver {
    /// Install the global hooks and prepare the driver.
    pub fn new(_config: &AppConfig) -> Result<Self> {
        RUNNING.store(true, Ordering::SeqCst);
        let (sender, receiver) = crossbeam_channel::unbounded();
        // Publishing the sender before the hooks are installed means a callback
        // can never observe an empty cell.
        let _ = EVENT_SENDER.set(sender);

        // SAFETY: the hook procedures have the required signature, and the
        // handles are stored so `Drop` can remove them.
        let input_scope = unsafe {
            let module = GetModuleHandleW(None)
                .map_err(|error| Error::platform(format!("GetModuleHandleW failed: {error}")))?;

            let mouse = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), module, 0);
            let keyboard = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), module, 0);

            match (mouse, keyboard) {
                (Ok(mouse), Ok(keyboard)) => {
                    MOUSE_HOOK.store(mouse.0 as isize, Ordering::SeqCst);
                    KEYBOARD_HOOK.store(keyboard.0 as isize, Ordering::SeqCst);
                    log::info!("global mouse and keyboard hooks installed");
                    InputScope::Global
                }
                (mouse, keyboard) => {
                    // Partial success still leaves the companion usable, so warn
                    // and carry on rather than refusing to start.
                    if let Ok(mouse) = mouse {
                        MOUSE_HOOK.store(mouse.0 as isize, Ordering::SeqCst);
                    }
                    if let Ok(keyboard) = keyboard {
                        KEYBOARD_HOOK.store(keyboard.0 as isize, Ordering::SeqCst);
                    }
                    eprintln!(
                        "warning: could not install both global input hooks; the monkey will \
                         only react to some input. Another application may be blocking hooks."
                    );
                    InputScope::WindowLocal
                }
            }
        };

        Ok(Self {
            hwnd: HWND::default(),
            canvas: None,
            size: (0, 0),
            origin: (0, 0),
            monitors: enumerate_monitors(),
            events: receiver,
            input_scope,
            dirty: None,
        })
    }

    /// The desktop rectangle the overlay should cover: every monitor at once.
    fn desktop_bounds(&self) -> Rect {
        ScreenLayout::new(self.monitors.clone())
            .bounding_box()
            .unwrap_or_else(|| Rect::new(0, 0, 1920, 1080))
    }

    /// Push the current DIB to the screen with per-pixel alpha.
    fn present(&mut self) -> Result<()> {
        let Some(canvas) = self.canvas.as_ref() else {
            return Ok(());
        };
        let position = POINT {
            x: self.origin.0,
            y: self.origin.1,
        };
        let size = SIZE {
            cx: canvas.width,
            cy: canvas.height,
        };
        let source = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };

        // SAFETY: every handle belongs to this driver and the structures are
        // fully initialised above.
        unsafe {
            UpdateLayeredWindow(
                self.hwnd,
                None,
                Some(&position),
                Some(&size),
                canvas.dc,
                Some(&source),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            )
            .map_err(|error| Error::platform(format!("UpdateLayeredWindow failed: {error}")))
        }
    }
}

impl Drop for Win32Driver {
    fn drop(&mut self) {
        // SAFETY: the handles were installed in `new`; removing them twice is
        // prevented by swapping the stored value out first.
        unsafe {
            let mouse = MOUSE_HOOK.swap(0, Ordering::SeqCst);
            if mouse != 0 {
                let _ = UnhookWindowsHookEx(HHOOK(mouse as *mut core::ffi::c_void));
            }
            let keyboard = KEYBOARD_HOOK.swap(0, Ordering::SeqCst);
            if keyboard != 0 {
                let _ = UnhookWindowsHookEx(HHOOK(keyboard as *mut core::ffi::c_void));
            }
            if !self.hwnd.is_invalid() {
                let _ = DestroyWindow(self.hwnd);
            }
        }
    }
}

impl PlatformDriver for Win32Driver {
    fn create_window(&mut self, _width: u32, _height: u32, title: &str) -> Result<()> {
        // The overlay always spans the whole virtual desktop so the monkey can
        // walk between monitors; the configured size is only a starting hint.
        let bounds = self.desktop_bounds();
        let title: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();

        // SAFETY: the class is registered before it is used, and every pointer
        // passed below outlives the call.
        unsafe {
            let module = GetModuleHandleW(None)
                .map_err(|error| Error::platform(format!("GetModuleHandleW failed: {error}")))?;

            let class = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(window_proc),
                hInstance: module.into(),
                lpszClassName: PCWSTR(WINDOW_CLASS.as_ptr()),
                ..Default::default()
            };
            // A zero return usually means "already registered", which is fine
            // on a restart within the same process.
            if RegisterClassExW(&class) == 0 {
                log::debug!("window class already registered");
            }

            let hwnd = CreateWindowExW(
                WS_EX_LAYERED
                    | WS_EX_TRANSPARENT
                    | WS_EX_TOPMOST
                    | WS_EX_TOOLWINDOW
                    | WS_EX_NOACTIVATE,
                PCWSTR(WINDOW_CLASS.as_ptr()),
                PCWSTR(title.as_ptr()),
                WS_POPUP,
                bounds.x,
                bounds.y,
                bounds.width as i32,
                bounds.height as i32,
                None,
                None,
                module,
                None,
            )
            .map_err(|error| Error::platform(format!("CreateWindowExW failed: {error}")))?;

            self.hwnd = hwnd;
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }

        self.origin = (bounds.x, bounds.y);
        self.size = (bounds.width as i32, bounds.height as i32);
        self.canvas = Some(LayeredCanvas::new(self.size.0, self.size.1)?);
        log::info!(
            "overlay created at {},{} ({}x{})",
            bounds.x,
            bounds.y,
            bounds.width,
            bounds.height
        );
        Ok(())
    }

    fn poll_events(&mut self) -> Vec<InputEvent> {
        let mut events = Vec::new();

        // The message pump has to run on the thread that installed the hooks,
        // otherwise Windows silently stops calling them.
        // SAFETY: `msg` is fully initialised by `PeekMessageW` before use.
        unsafe {
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                if msg.message == WM_QUIT {
                    RUNNING.store(false, Ordering::SeqCst);
                    events.push(InputEvent::CloseRequested);
                    break;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }

        if DISPLAY_CHANGED.swap(false, Ordering::SeqCst) {
            self.monitors = enumerate_monitors();
            log::info!(
                "display topology changed: {} monitor(s)",
                self.monitors.len()
            );
            let bounds = self.desktop_bounds();
            events.push(InputEvent::SurfaceResized {
                width: bounds.width,
                height: bounds.height,
            });
        }

        events.extend(self.events.try_iter());
        events
    }

    fn render_frame(&mut self, frame: &Frame<'_>) -> Result<()> {
        if self.canvas.is_none() {
            return Ok(());
        }
        let (width, height) = self.size;
        let origin = self.origin;
        let dirty = self.dirty.take();

        let drawn = {
            let canvas = self.canvas.as_mut().expect("canvas checked above");
            let pixels = canvas.pixels_mut();

            if let Some(rect) = dirty {
                clear_rect(pixels, width, height, rect);
            } else {
                pixels.fill(0);
            }

            blit(
                pixels,
                width,
                height,
                frame,
                Vec2::new(origin.0 as f32, origin.1 as f32),
            )
        };

        self.dirty = Some(drawn);
        self.present()
    }

    fn screen_layout(&self) -> ScreenLayout {
        ScreenLayout::new(self.monitors.clone())
    }

    fn cursor_position(&self) -> Vec2 {
        let mut point = POINT::default();
        // SAFETY: `point` is a valid, writable POINT.
        match unsafe { GetCursorPos(&mut point) } {
            Ok(()) => Vec2::new(point.x as f32, point.y as f32),
            Err(error) => {
                log::debug!("GetCursorPos failed: {error}");
                Vec2::ZERO
            }
        }
    }

    fn is_running(&self) -> bool {
        RUNNING.load(Ordering::SeqCst)
    }

    fn request_shutdown(&mut self) {
        RUNNING.store(false, Ordering::SeqCst);
    }

    fn input_scope(&self) -> InputScope {
        self.input_scope
    }
}

/// Clear a rectangle of a BGRA buffer back to fully transparent.
fn clear_rect(pixels: &mut [u8], width: i32, height: i32, rect: (i32, i32, i32, i32)) {
    let (x, y, w, h) = rect;
    if w <= 0 || h <= 0 {
        return;
    }
    let x0 = x.max(0);
    let y0 = y.max(0);
    let x1 = (x + w).min(width);
    let y1 = (y + h).min(height);
    for row in y0..y1 {
        let start = ((row * width + x0) * 4) as usize;
        let end = ((row * width + x1) * 4) as usize;
        if let Some(slice) = pixels.get_mut(start..end) {
            slice.fill(0);
        }
    }
}

/// Draw one sprite cell into a BGRA buffer with premultiplied alpha, returning
/// the rectangle that was touched.
fn blit(
    pixels: &mut [u8],
    width: i32,
    height: i32,
    frame: &Frame<'_>,
    origin: Vec2,
) -> (i32, i32, i32, i32) {
    let source = frame.sheet.texture();
    let rect = frame.sheet.frame_rect(frame.cell);
    let size = frame.size();

    let dest_x = (frame.position.x - origin.x).round() as i32;
    let dest_y = (frame.position.y - origin.y).round() as i32;
    let dest_w = size.x.round() as i32;
    let dest_h = size.y.round() as i32;
    if dest_w <= 0 || dest_h <= 0 {
        return (dest_x, dest_y, 0, 0);
    }

    let scale_x = dest_w as f32 / rect.width as f32;
    let scale_y = dest_h as f32 / rect.height as f32;

    for row in 0..dest_h {
        let target_y = dest_y + row;
        if target_y < 0 || target_y >= height {
            continue;
        }
        let source_y = rect.y + ((row as f32 / scale_y) as u32).min(rect.height - 1);
        for column in 0..dest_w {
            let target_x = dest_x + column;
            if target_x < 0 || target_x >= width {
                continue;
            }
            let source_x = rect.x + ((column as f32 / scale_x) as u32).min(rect.width - 1);
            let pixel = source.get_pixel(source_x, source_y);
            let alpha = u32::from(pixel[3]);
            if alpha == 0 {
                continue;
            }
            let offset = ((target_y * width + target_x) * 4) as usize;
            let Some(slot) = pixels.get_mut(offset..offset + 4) else {
                continue;
            };
            slot[0] = ((u32::from(pixel[2]) * alpha) / 255) as u8;
            slot[1] = ((u32::from(pixel[1]) * alpha) / 255) as u8;
            slot[2] = ((u32::from(pixel[0]) * alpha) / 255) as u8;
            slot[3] = alpha as u8;
        }
    }
    (dest_x, dest_y, dest_w, dest_h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clearing_clips_to_the_buffer() {
        let (width, height) = (8, 4);
        let mut pixels = vec![0xFFu8; (width * height * 4) as usize];
        clear_rect(&mut pixels, width, height, (-4, -4, 6, 6));
        assert_eq!(&pixels[0..8], &[0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(pixels[(width * 4 - 4) as usize], 0xFF);
    }

    #[test]
    fn clearing_off_buffer_is_a_no_op() {
        let mut pixels = vec![0xAAu8; 64];
        clear_rect(&mut pixels, 4, 4, (100, 100, 10, 10));
        assert!(pixels.iter().all(|byte| *byte == 0xAA));
    }
}
