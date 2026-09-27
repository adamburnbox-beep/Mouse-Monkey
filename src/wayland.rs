#![cfg(target_os = "linux")]

use crate::config::AppConfig;
use crate::platform::{InputEvent, KeyKind, MouseButton, PlatformDriver, Rect};
use crate::sprite_renderer::SpriteData;
use log::info;
use std::io;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

// Wayland specific imports for sctk 0.18
use sctk::{
    delegate_compositor, delegate_layer, delegate_output, delegate_pointer,
    delegate_registry, delegate_seat, delegate_shm,
    reexports::client::{
        globals::registry_queue_init,
        protocol::{wl_output, wl_pointer, wl_region, wl_seat, wl_surface},
        Connection, QueueHandle, WEnum,
    },
    registry::{ProvidesRegistryState, RegistryState},
    compositor::{CompositorHandler, CompositorState},
    output::{OutputHandler, OutputState},
    seat::{
        Capability, SeatHandler, SeatState,
        pointer::{PointerEvent, PointerEventKind, PointerHandler},
    },
    shell::wlr_layer::{
        Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler,
        LayerSurface, LayerSurfaceConfigure,
    },
    shm::{Shm, ShmHandler},
};

use evdev::{Device, EventType, RelativeAxisType};

struct WaylandState {
    running: bool,
    /// Surface-local pointer position (when cursor is over our window).
    pointer_pos: (f32, f32),
    /// Global cursor position tracked via evdev REL_X/REL_Y deltas.
    global_cursor_pos: (f32, f32),
    outputs: Vec<Rect>,
    window_size: (u32, u32),
    pending_events: Vec<InputEvent>,
    is_pointer_down: bool,
    evdev_active: bool,
    /// True while the monkey is being dragged. Used by render_frame to expand
    /// the input region so pointer motion events aren't lost mid-drag.
    is_dragging: bool,
}

pub struct WaylandDriver {
    config: AppConfig,
    connection: Connection,
    queue_handle: QueueHandle<WaylandDriverState>,
    event_queue: sctk::reexports::client::EventQueue<WaylandDriverState>,
    layer_surface: Option<LayerSurface>,
    surface: Option<wl_surface::WlSurface>,
    wayland_state: Arc<Mutex<WaylandState>>,
    _evdev_thread_handles: Vec<thread::JoinHandle<()>>,
    evdev_event_sender: crossbeam_channel::Sender<InputEvent>,
    evdev_event_receiver: crossbeam_channel::Receiver<InputEvent>,
    driver_state: WaylandDriverState,
    slot_pool: sctk::shm::slot::SlotPool,
}

/// Global state container required by SCTK 0.18 Dispatch models.
pub struct WaylandDriverState {
    registry_state: RegistryState,
    compositor_state: CompositorState,
    output_state: OutputState,
    shm_state: Shm,
    layer_shell: LayerShell,
    /// sctk-managed seat state — handles wl_seat binding automatically.
    seat_state: SeatState,
    /// The active pointer object (obtained once seat capability is announced).
    pointer: Option<wl_pointer::WlPointer>,
    wayland_state: Arc<Mutex<WaylandState>>,
    /// The layer surface wl_surface — needed to filter pointer events to our window.
    layer_wl_surface: Option<wl_surface::WlSurface>,
}

impl WaylandDriver {
    /// Spawns evdev reader threads for keyboard and ALL mouse devices found.
    ///
    /// **Device classification** (critical):
    /// - Real keyboard: supports KEY events AND has KEY_A (excludes mice that
    ///   report BTN_LEFT as a KEY event).
    /// - Mouse: supports REL_X + REL_Y relative-axis events.
    ///
    /// **Cursor tracking**: Each mouse device gets its own accumulator thread
    /// that reads REL_X/REL_Y deltas and emits MouseMove to `pending_events`.
    /// Using `pending_events` (not the channel) avoids double-emission since
    /// `poll_events` drains both the channel (keyboard) and pending_events.
    ///
    /// **Permissions**: Requires the process user to be in the `input` group
    /// (`sudo usermod -aG input $USER`, then re-login). If the device cannot
    /// be opened, a warning is logged and that device is skipped.
    fn setup_evdev_input(
        event_sender: crossbeam_channel::Sender<InputEvent>,
        wayland_state: Arc<Mutex<WaylandState>>,
    ) -> (Vec<thread::JoinHandle<()>>, bool) {
        let mut key_devices: Vec<Device> = Vec::new();
        let mut mouse_devices: Vec<Device> = Vec::new();

        if let Ok(entries) = std::fs::read_dir("/dev/input") {
            let mut paths: Vec<_> = entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                            .and_then(|n| n.to_str())
                            .map_or(false, |s| s.starts_with("event"))
                })
                .collect();
            paths.sort();

            for path in &paths {
                match Device::open(path) {
                    Err(e) => {
                        // Likely EACCES — user not in `input` group.
                        log::debug!("evdev: cannot open {:?}: {} (hint: add user to `input` group)", path, e);
                    }
                    Ok(dev) => {
                        // A real keyboard has letter keys (KEY_A).  Mice and
                        // other HID devices often report EventType::KEY for
                        // buttons (BTN_LEFT etc.) but NOT KEY_A.
                        let is_keyboard = dev.supported_events().contains(EventType::KEY)
                            && dev
                                .supported_keys()
                                .map_or(false, |keys| keys.contains(evdev::Key::KEY_A));

                        // A mouse/touchpad has X+Y relative axes.
                        let is_mouse = dev
                            .supported_relative_axes()
                            .map_or(false, |axes| {
                                axes.contains(RelativeAxisType::REL_X)
                                    && axes.contains(RelativeAxisType::REL_Y)
                            });

                        if is_keyboard {
                            log::info!("evdev: keyboard device {:?} ({})", path, dev.name().unwrap_or("?"));
                            key_devices.push(dev);
                        } else if is_mouse {
                            // Re-open so keyboard and mouse threads each hold
                            // their own file descriptor for the same device if
                            // it happens to be classified as both (rare).
                            log::info!("evdev: mouse device {:?} ({})", path, dev.name().unwrap_or("?"));
                            mouse_devices.push(dev);
                        }
                    }
                }
            }
        }

        if key_devices.is_empty() {
            let msg = "evdev: no keyboard found — keyboard scratch will not work. Is user in `input` group?";
            log::warn!("{}", msg);
            eprintln!("Warning (WaylandDriver): {}", msg);
        }
        if mouse_devices.is_empty() {
            let msg = "evdev: no mouse found — global cursor tracking disabled. Is user in `input` group?";
            log::warn!("{}", msg);
            eprintln!("Warning (WaylandDriver): {}", msg);
        }

        let mut handles = Vec::new();

        // ── Keyboard thread ────────────────────────────────────────────────────
        // Only needs ONE keyboard device; take the first real keyboard found.
        if let Some(mut dev) = key_devices.into_iter().next() {
            let sender = event_sender.clone();
            let ws = Arc::clone(&wayland_state);
            handles.push(thread::spawn(move || loop {
                if !ws.lock().unwrap().running {
                    break;
                }
                if let Ok(events) = dev.fetch_events() {
                    for ev in events {
                        if ev.event_type() == EventType::KEY {
                            let key_code = ev.code() as u32;
                            if ev.value() == 1 {
                                let kind = match evdev::Key(ev.code()) {
                                    evdev::Key::KEY_SPACE
                                    | evdev::Key::KEY_ENTER
                                    | evdev::Key::KEY_KPENTER => KeyKind::Thump,
                                    _ => KeyKind::Other,
                                };
                                let _ = sender.send(InputEvent::KeyDown { key_code, kind });
                            } else if ev.value() == 0 {
                                let _ = sender.send(InputEvent::KeyUp { key_code });
                            }
                        }
                    }
                }
                thread::sleep(Duration::from_millis(5));
            }));
        }

        // ── Mouse tracking threads (one per device) ────────────────────────────
        // We spawn a thread for EVERY mouse device found (e.g. touchpad AND
        // external USB mouse).  Each accumulates REL deltas independently and
        // writes into the shared global_cursor_pos.  Mouse events go into
        // pending_events ONLY (not the channel) to avoid double-counting in
        // poll_events() which drains both sources.
        let has_mouse = !mouse_devices.is_empty();
        for mut dev in mouse_devices {
            let ws = Arc::clone(&wayland_state);
            handles.push(thread::spawn(move || {
                log::info!("evdev mouse thread started for: {}", dev.name().unwrap_or("?"));
                loop {
                    if !ws.lock().unwrap().running {
                        break;
                    }
                    match dev.fetch_events() {
                        Err(e) => {
                            log::warn!("evdev mouse read error: {}", e);
                            thread::sleep(Duration::from_millis(100));
                        }
                        Ok(events) => {
                            let mut dx = 0i32;
                            let mut dy = 0i32;
                            let mut wheel = 0i32;
                            for ev in events {
                                if ev.event_type() == EventType::RELATIVE {
                                    match RelativeAxisType(ev.code()) {
                                        RelativeAxisType::REL_X => dx += ev.value(),
                                        RelativeAxisType::REL_Y => dy += ev.value(),
                                        RelativeAxisType::REL_WHEEL => wheel += ev.value(),
                                        _ => {}
                                    }
                                }
                            }
                            if wheel != 0 {
                                // evdev reports wheel-up as positive; we use positive = down.
                                ws.lock().unwrap().pending_events.push(InputEvent::Scroll {
                                    delta: -wheel as f32,
                                });
                            }
                            if dx != 0 || dy != 0 {
                                let mut s = ws.lock().unwrap();

                                // Use current global position as base
                                let (mut cx, mut cy) = s.global_cursor_pos;

                                // Calculate bounds from known outputs.
                                // Default to current window size if no outputs are registered yet.
                                // Default to a massive range so the cursor isn't trapped at 0,0 
                                // while waiting for Wayland output globals to bind.
                                let mut min_x = -10000.0f32;
                                let mut min_y = -10000.0f32;
                                let mut max_x = 10000.0f32;
                                let mut max_y = 10000.0f32;

                                if !s.outputs.is_empty() {
                                    min_x = f32::INFINITY;
                                    min_y = f32::INFINITY;
                                    max_x = f32::NEG_INFINITY;
                                    max_y = f32::NEG_INFINITY;
                                    for r in &s.outputs {
                                        min_x = min_x.min(r.x as f32);
                                        min_y = min_y.min(r.y as f32);
                                        max_x = max_x.max((r.x + r.width as i32) as f32);
                                        max_y = max_y.max((r.y + r.height as i32) as f32);
                                    }
                                }

                                cx = (cx + dx as f32).clamp(min_x, max_x - 1.0);
                                cy = (cy + dy as f32).clamp(min_y, max_y - 1.0);

                                s.global_cursor_pos = (cx, cy);
                                s.pending_events.push(InputEvent::MouseMove { x: cx, y: cy });
                            }
                        }
                    }
                    // Reduced sleep to improve responsiveness while maintaining low CPU
                    thread::sleep(Duration::from_millis(5));
                }
            }));
        }

        (handles, has_mouse)
    }
}


impl PlatformDriver for WaylandDriver {
    fn new(config: &AppConfig) -> io::Result<Self> {
        info!("Initializing WaylandDriver...");
        let connection = Connection::connect_to_env().map_err(|e| {
            io::Error::new(
                io::ErrorKind::Other,
                format!("Failed to connect to Wayland env: {}", e),
            )
        })?;

        let (globals, event_queue) = registry_queue_init(&connection).map_err(|e| {
            io::Error::new(
                io::ErrorKind::Other,
                format!("Failed to query registry: {}", e),
            )
        })?;

        let queue_handle = event_queue.handle();
        let registry_state = RegistryState::new(&globals);
        let compositor_state =
            CompositorState::bind(&globals, &queue_handle).map_err(|e| {
                io::Error::new(
                    io::ErrorKind::Other,
                    format!("Failed to bind Compositor: {}", e),
                )
            })?;
        let output_state = OutputState::new(&globals, &queue_handle);
        let shm_state = Shm::bind(&globals, &queue_handle).map_err(|e| {
            io::Error::new(io::ErrorKind::Other, format!("Failed to bind Shm: {}", e))
        })?;
        let layer_shell =
            LayerShell::bind(&globals, &queue_handle).map_err(|e| {
                io::Error::new(
                    io::ErrorKind::Other,
                    format!("Failed to bind LayerShell: {}", e),
                )
            })?;
        // Bind seat state — this registers wl_seat with the registry so pointer
        // capability events are delivered correctly.
        let seat_state = SeatState::new(&globals, &queue_handle);

        let wayland_state = Arc::new(Mutex::new(WaylandState {
            running: true,
            pointer_pos: (0.0, 0.0),
            global_cursor_pos: (
                (config.window_width / 2) as f32,
                (config.window_height / 2) as f32,
            ),
            outputs: Vec::new(),
            window_size: (config.window_width, config.window_height),
            pending_events: Vec::new(),
            is_pointer_down: false,
            evdev_active: false,
            is_dragging: false,
        }));

        let driver_state = WaylandDriverState {
            registry_state,
            compositor_state,
            output_state,
            shm_state,
            layer_shell,
            seat_state,
            pointer: None,
            wayland_state: wayland_state.clone(),
            layer_wl_surface: None,
        };

        let (evdev_tx, evdev_rx) = crossbeam_channel::unbounded();
        let (evdev_thread_handles, evdev_active) =
            Self::setup_evdev_input(evdev_tx.clone(), Arc::clone(&wayland_state));

        {
            let mut s = wayland_state.lock().unwrap();
            s.evdev_active = evdev_active;
        }

        let slot_pool = sctk::shm::slot::SlotPool::new(
            (config.window_width * config.window_height * 4) as usize,
            &driver_state.shm_state,
        )
        .map_err(|e| {
            io::Error::new(
                io::ErrorKind::Other,
                format!("Failed to create SlotPool: {}", e),
            )
        })?;

        Ok(Self {
            config: config.clone(),
            connection,
            queue_handle,
            event_queue,
            layer_surface: None,
            surface: None,
            wayland_state,
            _evdev_thread_handles: evdev_thread_handles,
            evdev_event_sender: evdev_tx,
            evdev_event_receiver: evdev_rx,
            driver_state,
            slot_pool,
        })
    }

    fn create_window(&mut self, width: u32, height: u32, _title: &str) -> io::Result<()> {
        let surface = self
            .driver_state
            .compositor_state
            .create_surface(&self.queue_handle);

        let layer_surface = self.driver_state.layer_shell.create_layer_surface(
            &self.queue_handle,
            surface.clone(),
            Layer::Overlay,
            Some("monkey_companion"),
            None,
        );

        layer_surface.set_size(width, height);
        layer_surface.set_anchor(Anchor::TOP | Anchor::LEFT | Anchor::RIGHT | Anchor::BOTTOM);
        layer_surface.set_keyboard_interactivity(KeyboardInteractivity::None);

        // Store the wl_surface so PointerHandler can filter events to this surface.
        self.driver_state.layer_wl_surface = Some(surface.clone());

        surface.commit();
        self.connection
            .flush()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

        self.surface = Some(surface);
        self.layer_surface = Some(layer_surface);

        // Perform a roundtrip to get the initial configure event (and seat capabilities).
        self.event_queue
            .roundtrip(&mut self.driver_state)
            .map_err(|e| {
                io::Error::new(
                    io::ErrorKind::Other,
                    format!("Roundtrip failed: {}", e),
                )
            })?;

        Ok(())
    }

    fn poll_events(&mut self) -> Vec<InputEvent> {
        let mut events = Vec::new();

        // Flush any pending outgoing requests.
        let _ = self.connection.flush();

        // Non-blocking drain of all available Wayland events.
        use std::os::fd::{AsFd, AsRawFd};
        let fd = self.connection.as_fd().as_raw_fd();
        loop {
            let mut fds = [libc::pollfd {
                fd,
                events: libc::POLLIN,
                revents: 0,
            }];
            let poll_ret = unsafe { libc::poll(fds.as_mut_ptr(), 1, 0) };
            if poll_ret > 0 && (fds[0].revents & libc::POLLIN) != 0 {
                if let Some(guard) = self.event_queue.prepare_read() {
                    let _ = guard.read();
                }
                let _ = self.event_queue.dispatch_pending(&mut self.driver_state);
            } else {
                // No more data available right now.
                break;
            }
        }
        // Final dispatch of anything queued but not yet dispatched.
        let _ = self.event_queue.dispatch_pending(&mut self.driver_state);

        // Collect keyboard events from evdev thread.
        while let Ok(ev) = self.evdev_event_receiver.try_recv() {
            events.push(ev);
        }

        // Collect pointer/window events from Wayland handlers.
        let mut s = self.wayland_state.lock().unwrap();
        events.append(&mut s.pending_events);
        events
    }

    fn render_frame(&mut self, sprite_data: &SpriteData) -> io::Result<()> {
        let surface = match &self.surface {
            Some(s) => s,
            None => return Ok(()),
        };

        let (width, height) = {
            let s = self.wayland_state.lock().unwrap();
            (s.window_size.0 as i32, s.window_size.1 as i32)
        };
        let stride = width * 4;

        // Allocate a fresh buffer each frame via SlotPool.
        let (buffer, canvas) = self
            .slot_pool
            .create_buffer(
                width,
                height,
                stride,
                sctk::reexports::client::protocol::wl_shm::Format::Argb8888,
            )
            .map_err(|e| {
                io::Error::new(
                    io::ErrorKind::Other,
                    format!("Failed to create buffer from SlotPool: {}", e),
                )
            })?;

        // Clear to fully transparent.
        canvas.fill(0);

        // Blit the sprite tile onto the canvas.
        let src_tex = &sprite_data.sprite_sheet.texture;
        let (src_w, src_h) = src_tex.dimensions();
        let (uv_x, uv_y, uv_w, uv_h) = sprite_data.uv_rect;
        let pos_x = sprite_data.position.0;
        let pos_y = sprite_data.position.1;
        let scale_x = sprite_data.scale.0 * sprite_data.sprite_sheet.render_scale;
        let scale_y = sprite_data.scale.1 * sprite_data.sprite_sheet.render_scale;

        let dest_w = (uv_w as f32 * scale_x) as i32;
        let dest_h = (uv_h as f32 * scale_y) as i32;

        if dest_w > 0 && dest_h > 0 {
            for dy in 0..dest_h {
                let target_y = pos_y as i32 + dy;
                if target_y < 0 || target_y >= height {
                    continue;
                }
                let src_rel_y = (dy as f32 / scale_y) as u32;
                let src_y = uv_y + src_rel_y.min(uv_h - 1);
                if src_y >= src_h {
                    continue;
                }

                for dx in 0..dest_w {
                    let target_x = pos_x as i32 + dx;
                    if target_x < 0 || target_x >= width {
                        continue;
                    }
                    let src_rel_x = (dx as f32 / scale_x) as u32;
                    let src_x = uv_x + src_rel_x.min(uv_w - 1);
                    if src_x >= src_w {
                        continue;
                    }

                    let pixel = src_tex.get_pixel(src_x, src_y);
                    let alpha = pixel[3];
                    if alpha > 0 {
                        let offset = ((target_y * width + target_x) * 4) as usize;
                        if offset + 3 < canvas.len() {
                            // WL_SHM_FORMAT_ARGB8888 in memory: [B, G, R, A]
                            canvas[offset + 3] = alpha;
                            canvas[offset + 2] = ((pixel[0] as u32 * alpha as u32) / 255) as u8;
                            canvas[offset + 1] = ((pixel[1] as u32 * alpha as u32) / 255) as u8;
                            canvas[offset] = ((pixel[2] as u32 * alpha as u32) / 255) as u8;
                        }
                    }
                }
            }
        }

        buffer
            .attach_to(surface)
            .map_err(|e| {
                io::Error::new(
                    io::ErrorKind::Other,
                    format!("Failed to attach buffer: {:?}", e),
                )
            })?;
        surface.damage(0, 0, width, height);

        // Input region strategy:
        // Always restrict the input region to the monkey sprite area to allow
        // click-through to other apps. When evdev is inactive, add a tracking
        // margin around the monkey so Wayland delivers motion events nearby
        // for eye-follow. During an active drag, expand to the full surface
        // so we don't lose pointer events mid-drag.
        let (evdev_active, is_dragging) = {
            let s = self.wayland_state.lock().unwrap();
            (s.evdev_active, s.is_dragging)
        };
        let region = self.driver_state.compositor_state.wl_compositor().create_region(&self.queue_handle, ());
        if is_dragging {
            // During drag, accept input everywhere so pointer motion isn't lost.
            region.add(0, 0, width, height);
        } else {
            let (pos_x, pos_y) = sprite_data.position;
            let (scale_x, scale_y) = sprite_data.scale;
            let frame_w = sprite_data.sprite_sheet.frame_width as f32;
            let frame_h = sprite_data.sprite_sheet.frame_height as f32;
            let monkey_w = (frame_w * scale_x).round() as i32;
            let monkey_h = (frame_h * scale_y).round() as i32;

            if evdev_active {
                // Tight input region — evdev handles global tracking.
                region.add(pos_x.round() as i32, pos_y.round() as i32, monkey_w, monkey_h);
            } else {
                // Wider margin to capture nearby motion events for eye-follow.
                // The margin extends 300px in every direction around the sprite.
                let margin = 300i32;
                let rx = (pos_x.round() as i32 - margin).max(0);
                let ry = (pos_y.round() as i32 - margin).max(0);
                let rw = (monkey_w + margin * 2).min(width - rx);
                let rh = (monkey_h + margin * 2).min(height - ry);
                region.add(rx, ry, rw, rh);
            }
        }
        surface.set_input_region(Some(&region));
        region.destroy();

        surface.commit();
        self.connection
            .flush()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

        Ok(())
    }

    fn get_screen_bounds(&self) -> Vec<Rect> {
        self.wayland_state.lock().unwrap().outputs.clone()
    }

    fn set_window_position(&mut self, _x: i32, _y: i32) {}

    fn set_window_size(&mut self, width: u32, height: u32) {
        if let Some(layer_surface) = self.layer_surface.as_ref() {
            layer_surface.set_size(width, height);
            if let Some(surface) = self.surface.as_ref() {
                surface.commit();
            }
            let _ = self.connection.flush();
        }
    }

    fn is_running(&self) -> bool {
        self.wayland_state.lock().unwrap().running
    }

    fn log_warning_stderr(&self, message: &str) {
        eprintln!("Warning (WaylandDriver): {}", message);
    }

    fn get_cursor_pos(&self) -> (f32, f32) {
        // Prefer the evdev-tracked global position; it updates whenever the
        // mouse moves anywhere on the desktop, not just over our surface.
        self.wayland_state.lock().unwrap().global_cursor_pos
    }
}

// ── sctk delegate boilerplate ─────────────────────────────────────────────────

impl ProvidesRegistryState for WaylandDriverState {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    sctk::registry_handlers![
        OutputState,
        SeatState, // ensures wl_seat is auto-bound from the global registry
    ];
}

impl CompositorHandler for WaylandDriverState {
    fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _scale_factor: i32,
    ) {
    }

    fn transform_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _transform: wl_output::Transform,
    ) {
    }

    fn frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _time: u32,
    ) {
    }
}

impl OutputHandler for WaylandDriverState {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }
    fn new_output(
        &mut self,
        conn: &Connection,
        qh: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        self.update_output(conn, qh, output);
    }

    fn update_output(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _output: wl_output::WlOutput) {
        let mut s = self.wayland_state.lock().unwrap();
        s.outputs = self
            .output_state
            .outputs()
            .filter_map(|output| {
                self.output_state.info(&output).and_then(|info| {
                    let (x, y) = info.logical_position?;
                    let (w, h) = info.logical_size?;
                    Some(Rect {
                        x,
                        y,
                        width: w as u32,
                        height: h as u32,
                    })
                })
            })
            .collect();
        info!("Output topology updated: {} monitor(s) found.", s.outputs.len());
    }
    fn output_destroyed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
    }
}

impl ShmHandler for WaylandDriverState {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm_state
    }
}

impl LayerShellHandler for WaylandDriverState {
    fn closed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _layer: &LayerSurface,
    ) {
        self.wayland_state.lock().unwrap().running = false;
    }

    fn configure(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        let mut s = self.wayland_state.lock().unwrap();
        info!(
            "Layer surface configure: new_size = {:?}",
            configure.new_size
        );
        if configure.new_size.0 > 0 && configure.new_size.1 > 0 {
            s.window_size = configure.new_size;
        }
    }
}

// ── Seat + Pointer via sctk's high-level handlers ────────────────────────────

impl SeatHandler for WaylandDriverState {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _seat: wl_seat::WlSeat,
    ) {
    }

    fn new_capability(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer && self.pointer.is_none() {
            info!("Pointer capability available — binding wl_pointer.");
            match self.seat_state.get_pointer(qh, &seat) {
                Ok(pointer) => self.pointer = Some(pointer),
                Err(e) => log::warn!("Failed to get pointer: {}", e),
            }
        }
    }

    fn remove_capability(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer {
            if let Some(pointer) = self.pointer.take() {
                pointer.release();
                info!("Pointer released.");
            }
        }
    }

    fn remove_seat(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _seat: wl_seat::WlSeat,
    ) {
    }
}

impl PointerHandler for WaylandDriverState {
    fn pointer_frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _pointer: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        for event in events {
            // Only handle events targeting our layer surface.
            if let Some(our_surface) = &self.layer_wl_surface {
                if &event.surface != our_surface {
                    continue;
                }
            }

            let (x, y) = (event.position.0 as f32, event.position.1 as f32);
            let mut s = self.wayland_state.lock().unwrap();

            match event.kind {
                PointerEventKind::Enter { .. } => {
                    s.pointer_pos = (x, y);
                    // Always sync global position from Wayland — our full-screen
                    // layer surface means surface-local coords ARE screen coords.
                    // This recalibrates evdev tracking (which drifts due to
                    // compositor cursor acceleration differences).
                    s.global_cursor_pos = (x, y);
                    if !s.evdev_active || s.is_dragging {
                        s.pending_events
                            .push(InputEvent::MouseMove { x, y });
                    }
                }
                PointerEventKind::Leave { .. } => {
                    // Cursor left our input region. evdev (if active) continues
                    // tracking independently.
                }
                PointerEventKind::Motion { .. } => {
                    s.pointer_pos = (x, y);
                    // Recalibrate evdev from Wayland's authoritative position.
                    s.global_cursor_pos = (x, y);
                    if !s.evdev_active || s.is_dragging {
                        // During drag, use authoritative Wayland position.
                        // Without evdev, this is the only source of motion.
                        s.pending_events
                            .push(InputEvent::MouseMove { x, y });
                    }
                }
                PointerEventKind::Press { button, .. } => {
                    let mouse_button = linux_button_to_mouse_button(button);
                    if mouse_button == MouseButton::Left {
                        s.is_pointer_down = true;
                        s.is_dragging = true; // Assume drag until release
                    }
                    // Use Wayland's authoritative surface-local position for
                    // clicks — the pointer IS on our surface for this event.
                    s.global_cursor_pos = (x, y);
                    s.pending_events.push(InputEvent::MouseDown {
                        x,
                        y,
                        button: mouse_button,
                    });
                }
                PointerEventKind::Release { button, .. } => {
                    let mouse_button = linux_button_to_mouse_button(button);
                    if mouse_button == MouseButton::Left {
                        s.is_pointer_down = false;
                        s.is_dragging = false;
                    }
                    s.global_cursor_pos = (x, y);
                    s.pending_events.push(InputEvent::MouseUp {
                        x,
                        y,
                        button: mouse_button,
                    });
                }
                PointerEventKind::Axis { vertical, .. } => {
                    // With evdev active the wheel is already read globally.
                    if !s.evdev_active {
                        let delta = if vertical.discrete != 0 {
                            vertical.discrete as f32
                        } else {
                            // ~10 px of continuous scroll per wheel notch
                            (vertical.absolute / 10.0) as f32
                        };
                        if delta != 0.0 {
                            s.pending_events.push(InputEvent::Scroll { delta });
                        }
                    }
                }
            }
        }
    }
}

fn linux_button_to_mouse_button(button: u32) -> MouseButton {
    match button {
        0x110 => MouseButton::Left,   // BTN_LEFT
        0x111 => MouseButton::Right,  // BTN_RIGHT
        0x112 => MouseButton::Middle, // BTN_MIDDLE
        other => MouseButton::Other(other),
    }
}

// Dispatch impl for wl_region (no events, but required by the compositor wl_compositor.create_region call)
impl sctk::reexports::client::Dispatch<wl_region::WlRegion, ()> for WaylandDriverState {
    fn event(
        _state: &mut Self,
        _region: &wl_region::WlRegion,
        _event: wl_region::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

delegate_registry!(WaylandDriverState);
delegate_compositor!(WaylandDriverState);
delegate_output!(WaylandDriverState);
delegate_shm!(WaylandDriverState);
delegate_layer!(WaylandDriverState);
delegate_seat!(WaylandDriverState);
delegate_pointer!(WaylandDriverState);