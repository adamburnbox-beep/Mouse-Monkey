//! Linux driver: a `wlr-layer-shell` overlay plus global input from `evdev`.
//!
//! The overlay is an ordinary SHM surface on the overlay layer with an input
//! region restricted to the sprite, so every click that is not on the monkey
//! goes straight through to whatever is underneath.
//!
//! Global input needs read access to `/dev/input/event*`. When that is refused
//! — the common case on a freshly installed system — the driver logs an
//! explicit warning to stderr and falls back to pointer events delivered to its
//! own surface, exactly as the PRD requires.

#![cfg(target_os = "linux")]

use std::os::fd::{AsFd, AsRawFd, RawFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use evdev::{Device, EventType, RelativeAxisType};
use sctk::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_layer, delegate_output, delegate_pointer, delegate_registry,
    delegate_seat, delegate_shm,
    output::{OutputHandler, OutputState},
    reexports::client::{
        globals::registry_queue_init,
        protocol::{wl_output, wl_pointer, wl_region, wl_seat, wl_shm, wl_surface},
        Connection, EventQueue, Proxy, QueueHandle,
    },
    registry::{ProvidesRegistryState, RegistryState},
    seat::{
        pointer::{PointerEvent, PointerEventKind, PointerHandler},
        Capability, SeatHandler, SeatState,
    },
    shell::wlr_layer::{
        Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
        LayerSurfaceConfigure,
    },
    shm::{
        slot::{Buffer, SlotPool},
        Shm, ShmHandler,
    },
};

use crate::config::AppConfig;
use crate::error::{Error, Result};
use crate::geometry::{Rect, ScreenLayout, Vec2};
use crate::platform::{Frame, InputEvent, InputScope, MouseButton, PlatformDriver};

/// How long an input reader thread waits for data before re-checking whether
/// the application is shutting down.
const INPUT_POLL_TIMEOUT_MS: i32 = 100;

/// A rectangle of buffer pixels: x, y, width, height.
type PixelRect = (i32, i32, i32, i32);

/// One SHM buffer and the sprite rectangle it was last painted with.
struct PooledBuffer {
    buffer: Buffer,
    last_painted: Option<PixelRect>,
}

/// How many SHM buffers may be in rotation at once. Two is the normal case;
/// a third absorbs a compositor that holds on to buffers a little longer.
const MAX_BUFFERS: usize = 3;

/// Extra margin, in pixels, added around the sprite's input region when global
/// input is unavailable, so the compositor still delivers nearby motion events.
const LOCAL_TRACKING_MARGIN: i32 = 300;

/// State shared between the Wayland callbacks, the input threads and the driver.
#[derive(Debug, Default)]
struct SharedState {
    /// Cursor position in screen coordinates, from evdev or from the pointer.
    cursor: Vec2,
    /// Monitor rectangles as announced by the compositor (FR-09).
    outputs: Vec<Rect>,
    /// Size the compositor gave the layer surface.
    surface_size: (u32, u32),
    /// Events waiting for the next `poll_events`.
    pending: Vec<InputEvent>,
    /// True between a left press and its release.
    pointer_down: bool,
    /// Whether evdev is feeding global cursor updates.
    evdev_active: bool,
}

/// The Linux platform driver.
pub struct WaylandDriver {
    connection: Connection,
    queue_handle: QueueHandle<DriverState>,
    event_queue: EventQueue<DriverState>,
    driver_state: DriverState,
    surface: Option<wl_surface::WlSurface>,
    layer_surface: Option<LayerSurface>,

    pool: SlotPool,
    /// The SHM buffers in rotation. Remembering what each one was last painted
    /// with is what allows a frame to clear a few hundred pixels instead of the
    /// whole surface.
    buffers: Vec<PooledBuffer>,
    buffer_size: (i32, i32),
    /// Sprite rectangle of the frame currently on screen. The compositor needs
    /// damage relative to what it is showing, which is not the same as what the
    /// buffer being reused last held.
    last_committed: Option<PixelRect>,

    shared: Arc<Mutex<SharedState>>,
    running: Arc<AtomicBool>,
    input_threads: Vec<JoinHandle<()>>,
    input_rx: crossbeam_channel::Receiver<InputEvent>,
    input_scope: InputScope,
}

impl std::fmt::Debug for WaylandDriver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WaylandDriver")
            .field("buffer_size", &self.buffer_size)
            .field("input_scope", &self.input_scope)
            .field("shared", &self.shared)
            .finish_non_exhaustive()
    }
}

/// The dispatch target sctk needs; holds everything the callbacks touch.
struct DriverState {
    registry_state: RegistryState,
    compositor_state: CompositorState,
    output_state: OutputState,
    shm_state: Shm,
    layer_shell: LayerShell,
    seat_state: SeatState,
    pointer: Option<wl_pointer::WlPointer>,
    layer_surface: Option<wl_surface::WlSurface>,
    shared: Arc<Mutex<SharedState>>,
    running: Arc<AtomicBool>,
}

impl WaylandDriver {
    /// Connect to the compositor, bind the globals and start input capture.
    pub fn new(config: &AppConfig) -> Result<Self> {
        let connection = Connection::connect_to_env().map_err(|error| {
            Error::Platform(format!(
                "cannot connect to a Wayland compositor ({error}); \
                 is WAYLAND_DISPLAY set?"
            ))
        })?;

        let (globals, event_queue) = registry_queue_init(&connection).map_err(|error| {
            Error::platform(format!("cannot read the Wayland registry: {error}"))
        })?;
        let queue_handle = event_queue.handle();

        let compositor_state = CompositorState::bind(&globals, &queue_handle)
            .map_err(|error| Error::platform(format!("no wl_compositor: {error}")))?;
        let shm_state = Shm::bind(&globals, &queue_handle)
            .map_err(|error| Error::platform(format!("no wl_shm: {error}")))?;
        let layer_shell = LayerShell::bind(&globals, &queue_handle).map_err(|error| {
            Error::Platform(format!(
                "this compositor does not support wlr-layer-shell ({error}), which the \
                 overlay needs. GNOME's Mutter is the usual culprit; KDE, Sway, Hyprland \
                 and COSMIC all support it."
            ))
        })?;

        let shared = Arc::new(Mutex::new(SharedState {
            cursor: Vec2::new(
                config.window_width as f32 / 2.0,
                config.window_height as f32 / 2.0,
            ),
            surface_size: (config.window_width, config.window_height),
            ..SharedState::default()
        }));
        let running = Arc::new(AtomicBool::new(true));

        let driver_state = DriverState {
            registry_state: RegistryState::new(&globals),
            compositor_state,
            output_state: OutputState::new(&globals, &queue_handle),
            shm_state,
            layer_shell,
            seat_state: SeatState::new(&globals, &queue_handle),
            pointer: None,
            layer_surface: None,
            shared: Arc::clone(&shared),
            running: Arc::clone(&running),
        };

        let pool_bytes = config.window_width as usize * config.window_height as usize * 4;
        let pool = SlotPool::new(pool_bytes, &driver_state.shm_state)
            .map_err(|error| Error::platform(format!("cannot create the SHM pool: {error}")))?;

        let (tx, rx) = crossbeam_channel::unbounded();
        let (input_threads, evdev_active) =
            spawn_input_threads(tx, Arc::clone(&shared), Arc::clone(&running));
        shared.lock().expect("shared state").evdev_active = evdev_active;

        let input_scope = if evdev_active {
            InputScope::Global
        } else {
            InputScope::WindowLocal
        };

        Ok(Self {
            connection,
            queue_handle,
            event_queue,
            driver_state,
            surface: None,
            layer_surface: None,
            pool,
            buffers: Vec::new(),
            buffer_size: (0, 0),
            last_committed: None,
            shared,
            running,
            input_threads,
            input_rx: rx,
            input_scope,
        })
    }

    /// Wayland gives pointer positions in surface-local coordinates. The overlay
    /// covers one output, so those coordinates need the output's origin added to
    /// become screen coordinates.
    fn surface_origin(&self) -> Vec2 {
        let shared = self.shared.lock().expect("shared state");
        shared
            .outputs
            .first()
            .map_or(Vec2::ZERO, |output| Vec2::new(output.left(), output.top()))
    }

    /// Restrict the region that receives pointer events, so clicks anywhere
    /// except on the monkey pass through to the application underneath.
    fn set_input_region(&mut self, frame: &Frame<'_>, width: i32, height: i32) {
        let Some(surface) = self.surface.as_ref() else {
            return;
        };

        let (pointer_down, evdev_active) = {
            let shared = self.shared.lock().expect("shared state");
            (shared.pointer_down, shared.evdev_active)
        };

        let region = self
            .driver_state
            .compositor_state
            .wl_compositor()
            .create_region(&self.queue_handle, ());

        if pointer_down {
            // Mid-drag the pointer routinely leaves the sprite; accepting input
            // everywhere for the duration is what keeps the drag smooth.
            region.add(0, 0, width, height);
        } else {
            let origin = self.surface_origin();
            let size = frame.size();
            let x = (frame.position.x - origin.x).round() as i32;
            let y = (frame.position.y - origin.y).round() as i32;
            let w = size.x.round() as i32;
            let h = size.y.round() as i32;
            if evdev_active {
                region.add(x, y, w, h);
            } else {
                // Without global input the only motion events we get are the
                // ones inside this region, so widen it enough for eye-follow.
                let rx = (x - LOCAL_TRACKING_MARGIN).max(0);
                let ry = (y - LOCAL_TRACKING_MARGIN).max(0);
                let rw = (w + LOCAL_TRACKING_MARGIN * 2).min(width - rx);
                let rh = (h + LOCAL_TRACKING_MARGIN * 2).min(height - ry);
                region.add(rx, ry, rw.max(0), rh.max(0));
            }
        }

        surface.set_input_region(Some(&region));
        region.destroy();
    }

    /// Blit one sprite cell into `canvas`, which is ARGB8888, premultiplied.
    fn blit(
        canvas: &mut [u8],
        stride: i32,
        height: i32,
        frame: &Frame<'_>,
        origin: Vec2,
    ) -> PixelRect {
        let width = stride / 4;
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
                let Some(slot) = canvas.get_mut(offset..offset + 4) else {
                    continue;
                };
                // wl_shm ARGB8888 is little-endian: [B, G, R, A], premultiplied.
                slot[0] = ((u32::from(pixel[2]) * alpha) / 255) as u8;
                slot[1] = ((u32::from(pixel[1]) * alpha) / 255) as u8;
                slot[2] = ((u32::from(pixel[0]) * alpha) / 255) as u8;
                slot[3] = alpha as u8;
            }
        }
        (dest_x, dest_y, dest_w, dest_h)
    }

    /// Zero a rectangle of the canvas back to fully transparent.
    fn clear_rect(canvas: &mut [u8], stride: i32, height: i32, rect: PixelRect) {
        let width = stride / 4;
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
            if let Some(slice) = canvas.get_mut(start..end) {
                slice.fill(0);
            }
        }
    }

    /// Drain everything the compositor has queued without blocking.
    fn dispatch_pending_events(&mut self) {
        let _ = self.connection.flush();
        let fd = self.connection.as_fd().as_raw_fd();
        while wait_readable(fd, 0) {
            if let Some(guard) = self.event_queue.prepare_read() {
                if guard.read().is_err() {
                    break;
                }
            }
            if self
                .event_queue
                .dispatch_pending(&mut self.driver_state)
                .is_err()
            {
                break;
            }
        }
        let _ = self.event_queue.dispatch_pending(&mut self.driver_state);
    }
}

impl Drop for WaylandDriver {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        for handle in self.input_threads.drain(..) {
            // Each reader wakes at least every INPUT_POLL_TIMEOUT_MS, so this
            // join is bounded.
            let _ = handle.join();
        }
    }
}

impl PlatformDriver for WaylandDriver {
    fn create_window(&mut self, width: u32, height: u32, title: &str) -> Result<()> {
        let surface = self
            .driver_state
            .compositor_state
            .create_surface(&self.queue_handle);
        let layer_surface = self.driver_state.layer_shell.create_layer_surface(
            &self.queue_handle,
            surface.clone(),
            Layer::Overlay,
            Some(title),
            None,
        );

        layer_surface.set_size(width, height);
        layer_surface.set_anchor(Anchor::TOP | Anchor::LEFT | Anchor::RIGHT | Anchor::BOTTOM);
        layer_surface.set_keyboard_interactivity(KeyboardInteractivity::None);
        // Nothing should be pushed aside to make room for a pet.
        layer_surface.set_exclusive_zone(-1);

        // An empty input region until the first frame, so the overlay never
        // swallows a click before the monkey has been drawn.
        let region = self
            .driver_state
            .compositor_state
            .wl_compositor()
            .create_region(&self.queue_handle, ());
        surface.set_input_region(Some(&region));
        region.destroy();

        surface.commit();
        self.connection
            .flush()
            .map_err(|error| Error::platform(format!("cannot flush the Wayland queue: {error}")))?;

        self.driver_state.layer_surface = Some(surface.clone());
        self.surface = Some(surface);
        self.layer_surface = Some(layer_surface);

        // One round trip gets the first configure and the seat capabilities.
        self.event_queue
            .roundtrip(&mut self.driver_state)
            .map_err(|error| Error::platform(format!("Wayland round trip failed: {error}")))?;
        Ok(())
    }

    fn poll_events(&mut self) -> Vec<InputEvent> {
        self.dispatch_pending_events();

        let mut events: Vec<InputEvent> = self.input_rx.try_iter().collect();
        let mut shared = self.shared.lock().expect("shared state");
        events.append(&mut shared.pending);
        events
    }

    fn render_frame(&mut self, frame: &Frame<'_>) -> Result<()> {
        let Some(surface) = self.surface.clone() else {
            return Ok(());
        };

        let (width, height) = {
            let shared = self.shared.lock().expect("shared state");
            (shared.surface_size.0 as i32, shared.surface_size.1 as i32)
        };
        if width <= 0 || height <= 0 {
            return Ok(());
        }
        let stride = width * 4;

        if self.buffer_size != (width, height) {
            self.buffers.clear();
            self.buffer_size = (width, height);
            self.last_committed = None;
        }

        let origin = {
            let shared = self.shared.lock().expect("shared state");
            shared
                .outputs
                .first()
                .map_or(Vec2::ZERO, |output| Vec2::new(output.left(), output.top()))
        };

        // Each buffer remembers the rectangle it was last painted with, so only
        // that rectangle has to be cleared before the next sprite goes down.
        // Splitting the borrow here lets the pool and the buffer list be used
        // together.
        let Self { buffers, pool, .. } = self;
        let free = buffers
            .iter()
            .position(|pooled| pooled.buffer.canvas(pool).is_some());

        let index = match free {
            Some(index) => index,
            None => {
                if buffers.len() >= MAX_BUFFERS {
                    // Every buffer is still on screen. Skipping the frame is
                    // better than stalling the loop; the next tick redraws.
                    log::trace!("all {MAX_BUFFERS} buffers are in use; skipping a frame");
                    return Ok(());
                }
                let (buffer, _) = pool
                    .create_buffer(width, height, stride, wl_shm::Format::Argb8888)
                    .map_err(|error| {
                        Error::platform(format!("cannot allocate a frame buffer: {error}"))
                    })?;
                buffers.push(PooledBuffer {
                    buffer,
                    last_painted: None,
                });
                buffers.len() - 1
            }
        };

        let pooled = &mut buffers[index];
        let previous = pooled.last_painted.take();
        let canvas = pooled
            .buffer
            .canvas(pool)
            .ok_or_else(|| Error::platform("the frame buffer is still held by the compositor"))?;

        match previous {
            // A buffer that has been drawn into before only needs its old
            // sprite rectangle cleared.
            Some(rect) => Self::clear_rect(canvas, stride, height, rect),
            // A fresh buffer has undefined contents.
            None => canvas.fill(0),
        }

        let drawn = Self::blit(canvas, stride, height, frame, origin);
        self.buffers[index].last_painted = Some(drawn);

        // Damage where the sprite was on screen and where it is now.
        let previously_shown = self.last_committed.replace(drawn);
        for rect in previously_shown.into_iter().chain(std::iter::once(drawn)) {
            let (x, y, w, h) = rect;
            if w > 0 && h > 0 {
                if surface.version() >= 4 {
                    surface.damage_buffer(x, y, w, h);
                } else {
                    surface.damage(x, y, w, h);
                }
            }
        }

        self.buffers[index]
            .buffer
            .attach_to(&surface)
            .map_err(|error| {
                Error::platform(format!("cannot attach the frame buffer: {error:?}"))
            })?;
        self.set_input_region(frame, width, height);
        surface.commit();
        self.connection
            .flush()
            .map_err(|error| Error::platform(format!("cannot flush the Wayland queue: {error}")))?;
        Ok(())
    }

    fn screen_layout(&self) -> ScreenLayout {
        ScreenLayout::new(self.shared.lock().expect("shared state").outputs.clone())
    }

    fn cursor_position(&self) -> Vec2 {
        self.shared.lock().expect("shared state").cursor
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    fn request_shutdown(&mut self) {
        self.running.store(false, Ordering::SeqCst);
    }

    fn input_scope(&self) -> InputScope {
        self.input_scope
    }
}

// ── evdev input ─────────────────────────────────────────────────────────────

/// Block until `fd` is readable or the timeout expires.
fn wait_readable(fd: RawFd, timeout_ms: i32) -> bool {
    let mut poll_fd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: `poll` is given one initialised pollfd and a matching length.
    let ready = unsafe { libc::poll(&mut poll_fd, 1, timeout_ms) };
    ready > 0 && (poll_fd.revents & libc::POLLIN) != 0
}

/// Open every readable input device and classify it.
fn open_input_devices() -> (Vec<Device>, Vec<Device>) {
    let mut keyboards = Vec::new();
    let mut mice = Vec::new();
    let mut permission_denied = false;

    let Ok(entries) = std::fs::read_dir("/dev/input") else {
        log::warn!("cannot list /dev/input; global input is unavailable");
        return (keyboards, mice);
    };

    let mut paths: Vec<_> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("event"))
        })
        .collect();
    paths.sort();

    for path in paths {
        match Device::open(&path) {
            Err(error) => {
                if error.kind() == std::io::ErrorKind::PermissionDenied {
                    permission_denied = true;
                }
                log::debug!("evdev: cannot open {}: {error}", path.display());
            }
            Ok(device) => {
                // A real keyboard has letter keys. Mice also report KEY events
                // for their buttons, so KEY alone is not enough to tell them
                // apart.
                let is_keyboard = device.supported_events().contains(EventType::KEY)
                    && device
                        .supported_keys()
                        .is_some_and(|keys| keys.contains(evdev::Key::KEY_A));
                let is_pointer = device.supported_relative_axes().is_some_and(|axes| {
                    axes.contains(RelativeAxisType::REL_X) && axes.contains(RelativeAxisType::REL_Y)
                });

                let name = device.name().unwrap_or("unnamed").to_string();
                if is_keyboard {
                    log::info!("evdev: keyboard {} ({name})", path.display());
                    keyboards.push(device);
                } else if is_pointer {
                    log::info!("evdev: pointer {} ({name})", path.display());
                    mice.push(device);
                }
            }
        }
    }

    if permission_denied && keyboards.is_empty() && mice.is_empty() {
        eprintln!(
            "warning: no readable devices in /dev/input. Global input capture is off; \
             add your user to the 'input' group (sudo usermod -aG input $USER) and log \
             back in to enable it."
        );
    }
    (keyboards, mice)
}

/// Start the reader threads. Returns the handles and whether global cursor
/// tracking is available.
fn spawn_input_threads(
    sender: crossbeam_channel::Sender<InputEvent>,
    shared: Arc<Mutex<SharedState>>,
    running: Arc<AtomicBool>,
) -> (Vec<JoinHandle<()>>, bool) {
    let (keyboards, mice) = open_input_devices();
    let has_pointer = !mice.is_empty();
    let mut handles = Vec::new();

    if keyboards.is_empty() {
        log::warn!("evdev: no keyboard found; keyboard scratching (FR-06) is disabled");
    }
    if mice.is_empty() {
        log::warn!("evdev: no pointer found; falling back to window-local cursor tracking");
    }

    // One keyboard is enough: the state machine only cares that *a* key moved.
    if let Some(mut device) = keyboards.into_iter().next() {
        let sender = sender.clone();
        let running = Arc::clone(&running);
        handles.push(thread::spawn(move || {
            let fd = device.as_raw_fd();
            while running.load(Ordering::SeqCst) {
                if !wait_readable(fd, INPUT_POLL_TIMEOUT_MS) {
                    continue;
                }
                let Ok(events) = device.fetch_events() else {
                    thread::sleep(Duration::from_millis(100));
                    continue;
                };
                for event in events {
                    if event.event_type() != EventType::KEY {
                        continue;
                    }
                    let key_code = u32::from(event.code());
                    let message = match event.value() {
                        1 => Some(InputEvent::KeyDown { key_code }),
                        0 => Some(InputEvent::KeyUp { key_code }),
                        _ => None, // 2 is auto-repeat; one press is enough.
                    };
                    if let Some(message) = message {
                        if sender.send(message).is_err() {
                            return;
                        }
                    }
                }
            }
        }));
    }

    // Every pointer gets its own thread: a laptop commonly has both a touchpad
    // and an external mouse, and either may be the one in use.
    for mut device in mice {
        let shared = Arc::clone(&shared);
        let running = Arc::clone(&running);
        handles.push(thread::spawn(move || {
            let fd = device.as_raw_fd();
            while running.load(Ordering::SeqCst) {
                if !wait_readable(fd, INPUT_POLL_TIMEOUT_MS) {
                    continue;
                }
                let Ok(events) = device.fetch_events() else {
                    thread::sleep(Duration::from_millis(100));
                    continue;
                };
                let (mut dx, mut dy) = (0i32, 0i32);
                for event in events {
                    if event.event_type() == EventType::RELATIVE {
                        match RelativeAxisType(event.code()) {
                            RelativeAxisType::REL_X => dx += event.value(),
                            RelativeAxisType::REL_Y => dy += event.value(),
                            _ => {}
                        }
                    }
                }
                if dx == 0 && dy == 0 {
                    continue;
                }

                let mut state = shared.lock().expect("shared state");
                let moved = Vec2::new(state.cursor.x + dx as f32, state.cursor.y + dy as f32);
                let layout = ScreenLayout::new(state.outputs.clone());
                state.cursor = match layout.bounding_box() {
                    Some(bounds) => Vec2::new(
                        moved.x.clamp(bounds.left(), bounds.right() - 1.0),
                        moved.y.clamp(bounds.top(), bounds.bottom() - 1.0),
                    ),
                    // Before any output is known, let the cursor roam rather
                    // than pinning it to a guessed rectangle.
                    None => moved,
                };
                let position = state.cursor;
                state.pending.push(InputEvent::MouseMove { position });
            }
        }));
    }

    (handles, has_pointer)
}

// ── sctk plumbing ───────────────────────────────────────────────────────────

impl ProvidesRegistryState for DriverState {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    sctk::registry_handlers![OutputState, SeatState];
}

impl CompositorHandler for DriverState {
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

impl OutputHandler for DriverState {
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

    fn update_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
        let outputs: Vec<Rect> = self
            .output_state
            .outputs()
            .filter_map(|output| {
                let info = self.output_state.info(&output)?;
                let (x, y) = info.logical_position?;
                let (width, height) = info.logical_size?;
                Some(Rect::new(x, y, width.max(0) as u32, height.max(0) as u32))
            })
            .collect();
        log::info!("output topology: {} monitor(s) {outputs:?}", outputs.len());
        self.shared.lock().expect("shared state").outputs = outputs;
    }

    fn output_destroyed(
        &mut self,
        conn: &Connection,
        qh: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        self.update_output(conn, qh, output);
    }
}

impl ShmHandler for DriverState {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm_state
    }
}

impl LayerShellHandler for DriverState {
    fn closed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _layer: &LayerSurface) {
        log::info!("the compositor closed the overlay");
        self.running.store(false, Ordering::SeqCst);
        self.shared
            .lock()
            .expect("shared state")
            .pending
            .push(InputEvent::CloseRequested);
    }

    fn configure(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        let (width, height) = configure.new_size;
        if width == 0 || height == 0 {
            return;
        }
        let mut shared = self.shared.lock().expect("shared state");
        if shared.surface_size != (width, height) {
            log::info!("overlay configured at {width}x{height}");
            shared.surface_size = (width, height);
            shared
                .pending
                .push(InputEvent::SurfaceResized { width, height });
        }
    }
}

impl SeatHandler for DriverState {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _seat: wl_seat::WlSeat) {}

    fn new_capability(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer && self.pointer.is_none() {
            match self.seat_state.get_pointer(qh, &seat) {
                Ok(pointer) => self.pointer = Some(pointer),
                Err(error) => log::warn!("cannot bind the pointer: {error}"),
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
            }
        }
    }

    fn remove_seat(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _seat: wl_seat::WlSeat) {
    }
}

impl PointerHandler for DriverState {
    fn pointer_frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _pointer: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        for event in events {
            if let Some(ours) = &self.layer_surface {
                if &event.surface != ours {
                    continue;
                }
            }

            let mut shared = self.shared.lock().expect("shared state");
            // Surface-local coordinates plus the output origin give screen
            // coordinates, because the overlay covers exactly one output.
            let origin = shared
                .outputs
                .first()
                .map_or(Vec2::ZERO, |output| Vec2::new(output.left(), output.top()));
            let position = Vec2::new(
                event.position.0 as f32 + origin.x,
                event.position.1 as f32 + origin.y,
            );

            match event.kind {
                PointerEventKind::Enter { .. } | PointerEventKind::Motion { .. } => {
                    // The compositor's position is authoritative whenever it is
                    // available; it also re-calibrates evdev's dead-reckoned
                    // position, which drifts under pointer acceleration.
                    shared.cursor = position;
                    shared.pending.push(InputEvent::MouseMove { position });
                }
                PointerEventKind::Leave { .. } => {}
                PointerEventKind::Press { button, .. } => {
                    let button = linux_button(button);
                    if button == MouseButton::Left {
                        shared.pointer_down = true;
                    }
                    shared.cursor = position;
                    shared
                        .pending
                        .push(InputEvent::MouseDown { position, button });
                }
                PointerEventKind::Release { button, .. } => {
                    let button = linux_button(button);
                    if button == MouseButton::Left {
                        shared.pointer_down = false;
                    }
                    shared.cursor = position;
                    shared
                        .pending
                        .push(InputEvent::MouseUp { position, button });
                }
                _ => {}
            }
        }
    }
}

/// Map a Linux `BTN_*` code to a portable button.
fn linux_button(button: u32) -> MouseButton {
    match button {
        0x110 => MouseButton::Left,
        0x111 => MouseButton::Right,
        0x112 => MouseButton::Middle,
        other => MouseButton::Other(other),
    }
}

/// `wl_region` has no events, but the compositor still needs a dispatch impl.
impl sctk::reexports::client::Dispatch<wl_region::WlRegion, ()> for DriverState {
    fn event(
        _state: &mut Self,
        _region: &wl_region::WlRegion,
        _event: wl_region::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

delegate_registry!(DriverState);
delegate_compositor!(DriverState);
delegate_output!(DriverState);
delegate_shm!(DriverState);
delegate_layer!(DriverState);
delegate_seat!(DriverState);
delegate_pointer!(DriverState);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_buttons_map_to_portable_ones() {
        assert_eq!(linux_button(0x110), MouseButton::Left);
        assert_eq!(linux_button(0x111), MouseButton::Right);
        assert_eq!(linux_button(0x112), MouseButton::Middle);
        assert_eq!(linux_button(0x113), MouseButton::Other(0x113));
    }

    #[test]
    fn clearing_a_rectangle_stays_inside_the_canvas() {
        let (width, height) = (8, 4);
        let mut canvas = vec![0xFFu8; (width * height * 4) as usize];
        WaylandDriver::clear_rect(&mut canvas, width * 4, height, (-4, -4, 6, 6));
        // The visible part of the rectangle is cleared...
        assert_eq!(&canvas[0..8], &[0, 0, 0, 0, 0, 0, 0, 0]);
        // ...and nothing outside it was touched.
        assert_eq!(canvas[(width * 4 - 4) as usize], 0xFF);
    }

    #[test]
    fn clearing_a_rectangle_entirely_off_canvas_is_a_no_op() {
        let mut canvas = vec![0xAAu8; 64];
        WaylandDriver::clear_rect(&mut canvas, 16, 1, (100, 100, 10, 10));
        assert!(canvas.iter().all(|byte| *byte == 0xAA));
    }
}
