#![cfg(target_os = "windows")]

use crate::config::AppConfig;
use crate::platform::{InputEvent, MouseButton, PlatformDriver, Rect};
use crate::sprite_renderer::SpriteData;
use log::{debug, error, info};
use std::io::{self, Write};
use std::ptr;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use windows::{
    core::PCWSTR,
    Win32::{
        Foundation::{BOOL, HWND, LPARAM, LRESULT, RECT, WPARAM},
        Graphics::Gdi::{
            EnumDisplayMonitors, GetMonitorInfoW, MONITORINFOEXW, HMONITOR, HDC,
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Input::KeyboardAndMouse::{
                SetWindowsHookExW, KBDLLHOOKSTRUCT, MSLLHOOKSTRUCT,
                WH_KEYBOARD_LL, WH_MOUSE_LL, HHOOK,
            },
            WindowsAndMessaging::{
                CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetCursorPos,
                GetWindowLongPtrW, RegisterClassExW, SetWindowLongPtrW,
                SetWindowPos, TranslateMessage, CS_HREDRAW, CS_VREDRAW, GWLP_USERDATA, MSG,
                SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, WM_CLOSE, WM_CREATE,
                WM_DISPLAYCHANGE, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN, WM_LBUTTONUP,
                WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEMOVE, WM_RBUTTONDOWN, WM_RBUTTONUP,
                WM_QUIT, WM_SYSKEYDOWN, WM_SYSKEYUP, WNDCLASSEXW, WS_EX_LAYERED,
                WS_EX_TRANSPARENT, WS_POPUP, PeekMessageW, PM_REMOVE,
            },
        },
    },
};

// Global hook IDs
static mut MOUSE_HOOK_HANDLE: HHOOK = HHOOK(0);
static mut KEYBOARD_HOOK_HANDLE: HHOOK = HHOOK(0);

// Channel for sending events from hook procedures to the main thread
static mut GLOBAL_EVENT_SENDER: Option<crossbeam_channel::Sender<InputEvent>> = None;

// Callback for low-level mouse hook
unsafe extern "system" fn mouse_hook_proc(n_code: i32, w_param: WPARAM, l_param: LPARAM) -> LRESULT {
    if n_code >= 0 {
        let mouse_struct = *(l_param.0 as *const MSLLHOOKSTRUCT);
        let (x, y) = (mouse_struct.pt.x as f32, mouse_struct.pt.y as f32);

        let event = match w_param.0 as u32 {
            WM_MOUSEMOVE => Some(InputEvent::MouseMove { x, y }),
            WM_LBUTTONDOWN => Some(InputEvent::MouseDown { x, y, button: MouseButton::Left }),
            WM_LBUTTONUP => Some(InputEvent::MouseUp { x, y, button: MouseButton::Left }),
            WM_RBUTTONDOWN => Some(InputEvent::MouseDown { x, y, button: MouseButton::Right }),
            WM_RBUTTONUP => Some(InputEvent::MouseUp { x, y, button: MouseButton::Right }),
            WM_MBUTTONDOWN => Some(InputEvent::MouseDown { x, y, button: MouseButton::Middle }),
            WM_MBUTTONUP => Some(InputEvent::MouseUp { x, y, button: MouseButton::Middle }),
            _ => None,
        };

        if let Some(ev) = event {
            if let Some(sender) = GLOBAL_EVENT_SENDER.as_ref() {
                let _ = sender.send(ev);
            }
        }
    }
    windows::Win32::UI::Input::KeyboardAndMouse::CallNextHookEx(MOUSE_HOOK_HANDLE, n_code, w_param, l_param)
}

// Callback for low-level keyboard hook
unsafe extern "system" fn keyboard_hook_proc(n_code: i32, w_param: WPARAM, l_param: LPARAM) -> LRESULT {
    if n_code >= 0 {
        let kbd_struct = *(l_param.0 as *const KBDLLHOOKSTRUCT);
        let key_code = kbd_struct.vkCode; // Virtual key code

        let event = match w_param.0 as u32 {
            WM_KEYDOWN | WM_SYSKEYDOWN => Some(InputEvent::KeyDown { key_code }),
            WM_KEYUP | WM_SYSKEYUP => Some(InputEvent::KeyUp { key_code }),
            _ => None,
        };

        if let Some(ev) = event {
            if let Some(sender) = GLOBAL_EVENT_SENDER.as_ref() {
                let _ = sender.send(ev);
            }
        }
    }
    windows::Win32::UI::Input::KeyboardAndMouse::CallNextHookEx(KEYBOARD_HOOK_HANDLE, n_code, w_param, l_param)
}

// Window procedure
unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, w_param: WPARAM, l_param: LPARAM) -> LRESULT {
    let driver_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Win32Driver;
    let driver = if driver_ptr.is_null() {
        None
    } else {
        Some(&mut *driver_ptr)
    };

    match msg {
        WM_CREATE => {
            let create_struct = *(l_param.0 as *const windows::Win32::UI::WindowsAndMessaging::CREATESTRUCTW);
            let driver_ptr_from_create = create_struct.lpCreateParams as *mut Win32Driver;
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, driver_ptr_from_create as isize);
            LRESULT(0)
        }
        WM_CLOSE => {
            if let Some(d) = driver {
                d.running = false;
                if let Some(sender) = GLOBAL_EVENT_SENDER.as_ref() {
                    let _ = sender.send(InputEvent::CloseRequested);
                }
            }
            DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_QUIT => {
            if let Some(d) = driver {
                d.running = false;
            }
            LRESULT(0)
        }
        WM_DISPLAYCHANGE => {
            info!("WM_DISPLAYCHANGE detected. Recalculating screen bounds.");
            if let Some(d) = driver {
                d.update_screen_bounds();
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, w_param, l_param),
    }
}

// Monitor enumeration callback
unsafe extern "system" fn monitor_enum_proc(
    h_monitor: HMONITOR,
    _hdc_monitor: HDC,
    _lprc_monitor: *mut RECT,
    l_param: LPARAM,
) -> BOOL {
    let bounds_vec = &mut *(l_param.0 as *mut Vec<Rect>);
    let mut monitor_info: MONITORINFOEXW = std::mem::zeroed();
    monitor_info.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;

    if GetMonitorInfoW(h_monitor, &mut monitor_info as *mut _ as *mut _) == BOOL(0) {
        error!("Failed to get monitor info.");
        return BOOL(1); // Continue enumeration
    }

    let rect = monitor_info.rcMonitor;
    bounds_vec.push(Rect {
        x: rect.left,
        y: rect.top,
        width: (rect.right - rect.left) as u32,
        height: (rect.bottom - rect.top) as u32,
    });

    BOOL(1) // Continue enumeration
}

pub struct Win32Driver {
    config: AppConfig,
    hwnd: HWND,
    running: bool,
    screen_bounds: Vec<Rect>,
    event_receiver: crossbeam_channel::Receiver<InputEvent>,
    event_sender: crossbeam_channel::Sender<InputEvent>, // Keep sender for internal use if needed
}

impl Win32Driver {
    fn update_screen_bounds(&mut self) {
        let mut bounds = Vec::new();
        unsafe {
            EnumDisplayMonitors(
                None,
                None,
                Some(monitor_enum_proc),
                LPARAM(&mut bounds as *mut _ as isize),
            )
            .expect("Failed to enumerate monitors");
        }
        self.screen_bounds = bounds;
        info!("Updated screen bounds: {:?}", self.screen_bounds);
    }
}

impl PlatformDriver for Win32Driver {
    fn new(config: &AppConfig) -> io::Result<Self> {
        info!("Initializing Win32Driver...");
        let (tx, rx) = crossbeam_channel::unbounded();

        unsafe {
            GLOBAL_EVENT_SENDER = Some(tx.clone()); // Set global sender for hooks

            let h_instance = GetModuleHandleW(None).map_err(|e| {
                io::Error::new(
                    io::ErrorKind::Other,
                    format!("Failed to get module handle: {}", e),
                )
            })?;
            let window_class_name = "MonkeyCompanionWindowClass\0";
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(wnd_proc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: h_instance,
                hIcon: Default::default(),
                hCursor: Default::default(),
                hbrBackground: Default::default(),
                lpszMenuName: PCWSTR::null(),
                lpszClassName: PCWSTR::from_raw(window_class_name.as_ptr()),
                hIconSm: Default::default(),
            };

            RegisterClassExW(&wc);

            // Set up global mouse hook
            MOUSE_HOOK_HANDLE = SetWindowsHookExW(
                WH_MOUSE_LL,
                Some(mouse_hook_proc),
                h_instance,
                0, // 0 for global hook
            )
            .map_err(|e: windows::core::Error| { // Explicitly type the error for clarity
                io::Error::new(
                    io::ErrorKind::Other,
                    format!("Failed to set mouse hook: {}", e),
                )
            })?;
            info!("Global mouse hook set.");

            // Set up global keyboard hook
            KEYBOARD_HOOK_HANDLE = SetWindowsHookExW(
                WH_KEYBOARD_LL,
                Some(keyboard_hook_proc),
                h_instance,
                0, // 0 for global hook
            )
            .map_err(|e: windows::core::Error| { // Explicitly type the error for clarity
                io::Error::new(
                    io::ErrorKind::Other,
                    format!("Failed to set keyboard hook: {}", e),
                )
            })?;
            info!("Global keyboard hook set.");
        }

        let mut driver = Self {
            config: config.clone(),
            hwnd: HWND(0), // Will be set in create_window
            running: true,
            screen_bounds: Vec::new(),
            event_receiver: rx,
            event_sender: tx,
        };

        driver.update_screen_bounds(); // Initial screen bounds

        Ok(driver)
    }

    fn create_window(&mut self, width: u32, height: u32, title: &str) -> io::Result<()> {
        info!(
            "Creating Win32 window: {}x{} '{}'",
            width, height, title
        );
        let window_class_name = "MonkeyCompanionWindowClass\0";
        let window_title_wide: Vec<u16> = title.encode_utf16().chain(Some(0)).collect();

        unsafe {
            let h_instance = GetModuleHandleW(None).map_err(|e| {
                io::Error::new(
                    io::ErrorKind::Other,
                    format!("Failed to get module handle for window creation: {}", e),
                )
            })?;

            let hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TRANSPARENT, // Extended styles for transparency and click-through
                PCWSTR::from_raw(window_class_name.as_ptr()),
                PCWSTR::from_raw(window_title_wide.as_ptr()),
                WS_POPUP, // No border, no title bar
                0,
                0, // Initial position
                width as i32,
                height as i32,
                None,
                None,
                h_instance,
                Some(self as *mut _ as *mut std::ffi::c_void), // Pass `self` to WM_CREATE
            );

            if hwnd.0 == 0 {
                return Err(io::Error::new(io::ErrorKind::Other, "Failed to create window."));
            }

            self.hwnd = hwnd;

            // For a fully transparent window, the rendering backend (wgpu) should handle alpha.
            // If a specific color key was needed for transparency:
            // SetLayeredWindowAttributes(hwnd, COLORREF(0), 0, LWA_COLORKEY); // Example for black transparency

            info!("Win32 window created with HWND: {:?}", hwnd);
        }
        Ok(())
    }

    fn poll_events(&mut self) -> Vec<InputEvent> {
        let mut events = Vec::new();
        unsafe {
            let mut msg = MSG::default();
            // Use PeekMessageW to avoid blocking if no messages are available
            while PeekMessageW(&mut msg, self.hwnd, 0, 0, PM_REMOVE).into() {
                if msg.message == WM_QUIT {
                    self.running = false;
                    events.push(InputEvent::CloseRequested);
                    break;
                }
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }

        // Collect events from the global hook channel
        while let Ok(ev) = self.event_receiver.try_recv() {
            events.push(ev);
        }

        events
    }

    fn render_frame(&mut self, sprite_data: &SpriteData) -> io::Result<()> {
        // Placeholder for the layered-window blit.
        // A full implementation would build a 32-bit premultiplied DIB from the
        // sprite tile and push it to the layered window via UpdateLayeredWindow.
        debug!(
            "Win32Driver: render frame at {:?} scale {:?} uv {:?}",
            sprite_data.position, sprite_data.scale, sprite_data.uv_rect
        );
        Ok(())
    }

    fn get_screen_bounds(&self) -> Vec<Rect> {
        self.screen_bounds.clone()
    }

    fn set_window_position(&mut self, x: i32, y: i32) {
        unsafe {
            if let Err(e) = SetWindowPos(
                self.hwnd,
                None,
                x,
                y,
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            ) {
                error!("Failed to set window position: {}", e);
            }
        }
    }

    fn set_window_size(&mut self, width: u32, height: u32) {
        unsafe {
            if let Err(e) = SetWindowPos(
                self.hwnd,
                None,
                0,
                0,
                width as i32,
                height as i32,
                SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
            ) {
                error!("Failed to set window size: {}", e);
            }
        }
    }

    fn is_running(&self) -> bool {
        self.running
    }

    fn log_warning_stderr(&self, message: &str) {
        eprintln!("Warning (Win32Driver): {}", message);
        let _ = io::stderr().flush();
    }

    fn get_cursor_pos(&self) -> (f32, f32) {
        let mut point = windows::Win32::Foundation::POINT::default();
        unsafe {
            if let Err(e) = GetCursorPos(&mut point) {
                error!("Failed to get cursor position: {}", e);
            }
        }
        (point.x as f32, point.y as f32)
    }
}