#![cfg(target_os = "linux")]

use crate::config::AppConfig;
use crate::platform::{InputEvent, MouseButton, PlatformDriver, Rect};
use crate::sprite_renderer::SpriteData;
use log::info;
use std::collections::HashSet;
use std::io;
use std::path::PathBuf;
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
        Connection, QueueHandle,
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

use evdev::{AbsoluteAxisType, Device, EventType, Key, PropType, RelativeAxisType};

/// How often the evdev watcher looks for input devices attached after launch
/// (Bluetooth keyboards, USB receivers, docks, …).
const EVDEV_RESCAN_INTERVAL: Duration = Duration::from_secs(2);
/// Touchpad finger travel → cursor pixels. Roughly libinput's default speed;
/// any drift is corrected whenever the pointer passes over the monkey.
const TOUCHPAD_PX_PER_MM: f32 = 12.0;
/// Sprite pixels fainter than this don't count as part of the monkey's hitbox.
const HITBOX_ALPHA_THRESHOLD: u8 = 32;
/// Opaque pixels are grouped into cells this size (px) so the input region
/// stays a handful of rectangles.
const HITBOX_CELL_PX: i32 = 8;

struct WaylandState {
    running: bool,
    /// Surface-local pointer position (when cursor is over our window).
    pointer_pos: (f32, f32),
    /// Global cursor position tracked via evdev deltas.
    global_cursor_pos: (f32, f32),
    outputs: Vec<Rect>,
    window_size: (u32, u32),
    pending_events: Vec<InputEvent>,
    is_pointer_down: bool,
    /// Number of evdev keyboards currently being read.
    evdev_keyboards: usize,
    /// Number of evdev mice/touchpads currently being read. While > 0 the
    /// cursor is tracked globally, whichever window it is over.
    evdev_pointers: usize,
    /// True while the monkey is being dragged. Wayland's implicit pointer grab
    /// keeps delivering motion to us until release, so evdev motion is ignored.
    is_dragging: bool,
}

impl WaylandState {
    /// Applies a relative cursor movement measured by evdev, clamped to the
    /// known monitor layout.
    fn move_global_cursor(&mut self, dx: f32, dy: f32) {
        // Default to a massive range so the cursor isn't trapped at 0,0
        // while waiting for Wayland output globals to bind.
        let (mut min_x, mut min_y, mut max_x, mut max_y) = (-10000.0f32, -10000.0f32, 10000.0f32, 10000.0f32);
        if !self.outputs.is_empty() {
            min_x = f32::INFINITY;
            min_y = f32::INFINITY;
            max_x = f32::NEG_INFINITY;
            max_y = f32::NEG_INFINITY;
            for r in &self.outputs {
                min_x = min_x.min(r.x as f32);
                min_y = min_y.min(r.y as f32);
                max_x = max_x.max((r.x + r.width as i32) as f32);
                max_y = max_y.max((r.y + r.height as i32) as f32);
            }
        }

        let (cx, cy) = self.global_cursor_pos;
        let x = (cx + dx).clamp(min_x, max_x - 1.0);
        let y = (cy + dy).clamp(min_y, max_y - 1.0);
        self.global_cursor_pos = (x, y);
        if !self.is_dragging {
            self.pending_events.push(InputEvent::MouseMove { x, y });
        }
    }
}

pub struct WaylandDriver {
    config: AppConfig,
    connection: Connection,
    queue_handle: QueueHandle<WaylandDriverState>,
    event_queue: sctk::reexports::client::EventQueue<WaylandDriverState>,
    layer_surface: Option<LayerSurface>,
    surface: Option<wl_surface::WlSurface>,
    wayland_state: Arc<Mutex<WaylandState>>,
    _evdev_watcher: thread::JoinHandle<()>,
    evdev_event_receiver: crossbeam_channel::Receiver<InputEvent>,
    driver_state: WaylandDriverState,
    slot_pool: sctk::shm::slot::SlotPool,
    /// What was drawn by the last committed frame; used to skip redundant
    /// redraws and to damage only the area the sprite moved through.
    last_frame: Option<FrameKey>,
}

/// Everything that determines the pixels of a frame.
#[derive(Clone, Copy, PartialEq, Eq)]
struct FrameKey {
    window: (i32, i32),
    uv_rect: (u32, u32, u32, u32),
    /// Sprite destination rectangle on the surface: (x, y, w, h).
    dest: (i32, i32, i32, i32),
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

// ── evdev: global keyboard / mouse / touchpad input ─────────────────────────
//
// Wayland deliberately never tells a client about input aimed at other
// windows, so the only way for the monkey to react to typing and cursor
// movement everywhere is to read the kernel input devices directly. This is
// read-only: devices are never grabbed, so every event still reaches the
// focused application untouched.
//
// **Permissions**: requires read access to /dev/input/event* — normally by
// being in the `input` group (`sudo usermod -aG input $USER`, then log out
// and back in).

/// What an input device can tell the monkey about.
#[derive(Clone, Copy)]
struct DeviceCaps {
    /// Has letter keys. Mice often report BTN_LEFT etc. as KEY events but
    /// never KEY_A.
    keyboard: bool,
    /// Relative X/Y motion (mice, trackballs, trackpoints).
    mouse: bool,
    /// Absolute-position touchpad, with its units-per-mm on each axis.
    touchpad: Option<(f32, f32)>,
}

impl DeviceCaps {
    fn of(dev: &Device) -> Self {
        let has_key = |k: Key| dev.supported_keys().map_or(false, |keys| keys.contains(k));
        let keyboard = has_key(Key::KEY_A);
        let mouse = dev.supported_relative_axes().map_or(false, |axes| {
            axes.contains(RelativeAxisType::REL_X) && axes.contains(RelativeAxisType::REL_Y)
        });

        // Touchpads report absolute finger positions plus BTN_TOOL_FINGER.
        // Touchscreens and drawing tablets (INPUT_PROP_DIRECT) map straight
        // to the screen instead, so they're left out.
        let has_abs_xy = dev.supported_absolute_axes().map_or(false, |axes| {
            axes.contains(AbsoluteAxisType::ABS_X) && axes.contains(AbsoluteAxisType::ABS_Y)
        });
        let touchpad = if has_abs_xy
            && has_key(Key::BTN_TOOL_FINGER)
            && !dev.properties().contains(PropType::DIRECT)
        {
            dev.get_abs_state().ok().map(|abs| {
                let units_per_mm = |axis: AbsoluteAxisType, assumed_mm: f32| {
                    let info = abs[axis.0 as usize];
                    if info.resolution > 0 {
                        info.resolution as f32
                    } else {
                        // No resolution reported: assume a typical laptop pad size.
                        ((info.maximum - info.minimum) as f32 / assumed_mm).max(1.0)
                    }
                };
                (units_per_mm(AbsoluteAxisType::ABS_X, 100.0), units_per_mm(AbsoluteAxisType::ABS_Y, 65.0))
            })
        } else {
            None
        };

        Self { keyboard, mouse, touchpad }
    }

    fn is_pointer(&self) -> bool {
        self.mouse || self.touchpad.is_some()
    }

    fn is_useful(&self) -> bool {
        self.keyboard || self.is_pointer()
    }

    fn describe(&self) -> String {
        let mut kinds = Vec::new();
        if self.keyboard {
            kinds.push("keyboard");
        }
        if self.mouse {
            kinds.push("mouse");
        }
        if self.touchpad.is_some() {
            kinds.push("touchpad");
        }
        kinds.join(" + ")
    }
}

/// True for keyboard keys, false for mouse/joystick/touch buttons (BTN_*),
/// which share the KEY event type.
fn is_keyboard_key(code: u16) -> bool {
    (1..0x100).contains(&code) || (0x160..0x2c0).contains(&code)
}

/// Turns absolute touchpad finger positions into relative cursor movement,
/// the way libinput does: only single-finger contact moves the cursor, so
/// two-finger scrolling and multi-finger gestures are ignored.
#[derive(Default)]
struct TouchpadTracker {
    touching: bool,
    /// Bitmask of active BTN_TOOL_{FINGER,DOUBLETAP,...}; bit 0 = one finger.
    tools: u8,
    pos: (Option<i32>, Option<i32>),
    last: Option<(i32, i32)>,
}

impl TouchpadTracker {
    fn on_key(&mut self, code: u16, value: i32) {
        let bit = match Key(code) {
            Key::BTN_TOUCH => {
                self.touching = value != 0;
                return;
            }
            Key::BTN_TOOL_FINGER => 1,
            Key::BTN_TOOL_DOUBLETAP => 2,
            Key::BTN_TOOL_TRIPLETAP => 4,
            Key::BTN_TOOL_QUADTAP => 8,
            Key::BTN_TOOL_QUINTTAP => 16,
            _ => return,
        };
        if value != 0 {
            self.tools |= bit;
        } else {
            self.tools &= !bit;
        }
        // Finger count changed: the reported position may jump to another
        // finger, so restart delta tracking.
        self.last = None;
    }

    fn on_abs(&mut self, code: u16, value: i32) {
        match AbsoluteAxisType(code) {
            AbsoluteAxisType::ABS_X => self.pos.0 = Some(value),
            AbsoluteAxisType::ABS_Y => self.pos.1 = Some(value),
            _ => {}
        }
    }

    /// Called at the end of each event frame; returns the movement in device units.
    fn on_sync(&mut self) -> (i32, i32) {
        let (Some(x), Some(y)) = self.pos else { return (0, 0) };
        if !self.touching || self.tools != 1 {
            self.last = None;
            return (0, 0);
        }
        let delta = self.last.map_or((0, 0), |(lx, ly)| (x - lx, y - ly));
        self.last = Some((x, y));
        delta
    }
}

/// Finds input devices and spawns one reader thread per useful device.
struct EvdevWatcher {
    key_sender: crossbeam_channel::Sender<InputEvent>,
    wayland_state: Arc<Mutex<WaylandState>>,
    /// Devices with a live reader thread (removed by the thread when it exits).
    tracked: Arc<Mutex<HashSet<PathBuf>>>,
    /// Devices that are neither keyboards nor pointers.
    ignored: HashSet<PathBuf>,
    /// Whether the "can't read input devices" diagnosis was already printed.
    reported_denied: bool,
}

impl EvdevWatcher {
    fn new(
        key_sender: crossbeam_channel::Sender<InputEvent>,
        wayland_state: Arc<Mutex<WaylandState>>,
    ) -> Self {
        Self {
            key_sender,
            wayland_state,
            tracked: Arc::new(Mutex::new(HashSet::new())),
            ignored: HashSet::new(),
            reported_denied: false,
        }
    }

    /// Opens any /dev/input/event* not yet being read. Returns how many
    /// devices could not be opened for lack of permission.
    fn scan(&mut self) -> usize {
        let mut paths: Vec<PathBuf> = match std::fs::read_dir("/dev/input") {
            Ok(entries) => entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .map_or(false, |s| s.starts_with("event"))
                })
                .collect(),
            Err(e) => {
                log::debug!("evdev: cannot list /dev/input: {}", e);
                return 0;
            }
        };
        paths.sort();
        // Forget ignored devices that were unplugged; their node may be reused.
        self.ignored.retain(|p| paths.contains(p));

        let mut denied = 0;
        for path in paths {
            if self.ignored.contains(&path) || self.tracked.lock().unwrap().contains(&path) {
                continue;
            }
            let dev = match Device::open(&path) {
                Ok(dev) => dev,
                Err(e) => {
                    if e.kind() == io::ErrorKind::PermissionDenied {
                        denied += 1;
                    } else {
                        log::debug!("evdev: cannot open {:?}: {}", path, e);
                    }
                    continue;
                }
            };
            let caps = DeviceCaps::of(&dev);
            if !caps.is_useful() {
                self.ignored.insert(path);
                continue;
            }

            info!(
                "evdev: reading {:?} ({}) as {}",
                path,
                dev.name().unwrap_or("?"),
                caps.describe()
            );
            {
                let mut s = self.wayland_state.lock().unwrap();
                if caps.keyboard {
                    s.evdev_keyboards += 1;
                }
                if caps.is_pointer() {
                    s.evdev_pointers += 1;
                }
            }
            self.tracked.lock().unwrap().insert(path.clone());

            let key_sender = self.key_sender.clone();
            let ws = Arc::clone(&self.wayland_state);
            let tracked = Arc::clone(&self.tracked);
            thread::spawn(move || {
                read_device(dev, caps, &key_sender, &ws);
                tracked.lock().unwrap().remove(&path);
                let mut s = ws.lock().unwrap();
                if caps.keyboard {
                    s.evdev_keyboards -= 1;
                }
                if caps.is_pointer() {
                    s.evdev_pointers -= 1;
                }
                info!("evdev: stopped reading {:?} (device removed)", path);
            });
        }
        denied
    }

    /// Warns (once) when the monkey can't see keyboard or cursor input, and
    /// explains how to fix it.
    fn report_status(&mut self, denied: usize) {
        let (keyboards, pointers) = {
            let s = self.wayland_state.lock().unwrap();
            (s.evdev_keyboards, s.evdev_pointers)
        };
        if keyboards > 0 && pointers > 0 {
            info!("evdev: global input active ({} keyboard(s), {} pointer device(s)).", keyboards, pointers);
            return;
        }

        let mut missing = Vec::new();
        if keyboards == 0 {
            missing.push("react to typing");
        }
        if pointers == 0 {
            missing.push("follow the cursor outside its own body");
        }
        let impact = format!("the monkey can't {}.", missing.join(" or "));

        let msg = if denied > 0 {
            if self.reported_denied {
                return;
            }
            self.reported_denied = true;
            format!(
                "evdev: permission denied reading {} input device(s) in /dev/input, so {}\n{}",
                denied,
                impact,
                input_permission_hint()
            )
        } else {
            format!("evdev: no matching input devices found, so {} Still watching for new devices.", impact)
        };
        log::warn!("{}", msg);
        eprintln!("Warning (WaylandDriver): {}", msg);
    }
}

/// Reads one device until it disappears, forwarding key presses and cursor
/// movement to the monkey.
fn read_device(
    mut dev: Device,
    caps: DeviceCaps,
    key_sender: &crossbeam_channel::Sender<InputEvent>,
    wayland_state: &Mutex<WaylandState>,
) {
    let mut touchpad = TouchpadTracker::default();
    loop {
        let events = match dev.fetch_events() {
            Ok(events) => events,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => {
                log::debug!("evdev read error: {}", e);
                return;
            }
        };

        let (mut dx, mut dy) = (0.0f32, 0.0f32);
        for ev in events {
            match ev.event_type() {
                EventType::KEY => {
                    if caps.keyboard && is_keyboard_key(ev.code()) {
                        let key_code = ev.code() as u32;
                        // value 2 = autorepeat, which isn't a new key press.
                        match ev.value() {
                            1 => {
                                let _ = key_sender.send(InputEvent::KeyDown { key_code });
                            }
                            0 => {
                                let _ = key_sender.send(InputEvent::KeyUp { key_code });
                            }
                            _ => {}
                        }
                    }
                    if caps.touchpad.is_some() {
                        touchpad.on_key(ev.code(), ev.value());
                    }
                }
                EventType::RELATIVE if caps.mouse => match RelativeAxisType(ev.code()) {
                    RelativeAxisType::REL_X => dx += ev.value() as f32,
                    RelativeAxisType::REL_Y => dy += ev.value() as f32,
                    _ => {}
                },
                EventType::ABSOLUTE if caps.touchpad.is_some() => touchpad.on_abs(ev.code(), ev.value()),
                EventType::SYNCHRONIZATION => {
                    if let Some((units_x, units_y)) = caps.touchpad {
                        let (ux, uy) = touchpad.on_sync();
                        dx += ux as f32 / units_x * TOUCHPAD_PX_PER_MM;
                        dy += uy as f32 / units_y * TOUCHPAD_PX_PER_MM;
                    }
                }
                _ => {}
            }
        }

        if dx != 0.0 || dy != 0.0 {
            wayland_state.lock().unwrap().move_global_cursor(dx, dy);
        }
    }
}

/// Explains why /dev/input/event* can't be opened and what to do about it.
fn input_permission_hint() -> String {
    let user = std::env::var("USER").unwrap_or_else(|_| "$USER".to_string());
    // (gid, user listed as member) of the `input` group, if it exists.
    let input_group = std::fs::read_to_string("/etc/group").ok().and_then(|groups| {
        groups.lines().find_map(|line| {
            let fields: Vec<&str> = line.split(':').collect();
            if fields.len() >= 4 && fields[0] == "input" {
                let gid = fields[2].parse::<libc::gid_t>().ok()?;
                Some((gid, fields[3].split(',').any(|m| m == user)))
            } else {
                None
            }
        })
    });

    let try_now = "To try it right away in this terminal without logging out: sg input -c 'cargo run'";
    match input_group {
        Some((gid, _)) if process_has_group(gid) => {
            "This process is in the `input` group but was still refused; check the device permissions with `ls -l /dev/input`.".to_string()
        }
        Some((_, true)) => format!(
            "You are in the `input` group, but this login session started before you were added. Log out and back in (or reboot).\n{}",
            try_now
        ),
        Some((_, false)) => format!(
            "Fix: sudo usermod -aG input {} — then log out and back in.\n{}",
            user, try_now
        ),
        None => "Grant your user read access to /dev/input/event* (on most distros: sudo usermod -aG input $USER, then log out and back in).".to_string(),
    }
}

/// Whether the current process has `gid` as its primary or a supplementary group.
fn process_has_group(gid: libc::gid_t) -> bool {
    unsafe {
        if libc::getegid() == gid {
            return true;
        }
        let count = libc::getgroups(0, std::ptr::null_mut());
        if count <= 0 {
            return false;
        }
        let mut groups = vec![0 as libc::gid_t; count as usize];
        let count = libc::getgroups(count, groups.as_mut_ptr());
        count > 0 && groups[..count as usize].contains(&gid)
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
            evdev_keyboards: 0,
            evdev_pointers: 0,
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

        // Scan once now so startup logs say exactly what input the monkey can
        // see, then keep watching for devices plugged in later.
        let (evdev_tx, evdev_rx) = crossbeam_channel::unbounded();
        let mut watcher = EvdevWatcher::new(evdev_tx, Arc::clone(&wayland_state));
        let denied = watcher.scan();
        watcher.report_status(denied);
        let watcher_state = Arc::clone(&wayland_state);
        let evdev_watcher = thread::spawn(move || loop {
            thread::sleep(EVDEV_RESCAN_INTERVAL);
            if !watcher_state.lock().unwrap().running {
                break;
            }
            let (had_keyboard, had_pointer) = {
                let s = watcher_state.lock().unwrap();
                (s.evdev_keyboards > 0, s.evdev_pointers > 0)
            };
            let denied = watcher.scan();
            let (has_keyboard, has_pointer) = {
                let s = watcher_state.lock().unwrap();
                (s.evdev_keyboards > 0, s.evdev_pointers > 0)
            };
            if (had_keyboard, had_pointer) != (has_keyboard, has_pointer) || denied > 0 {
                watcher.report_status(denied);
            }
        });

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
            _evdev_watcher: evdev_watcher,
            evdev_event_receiver: evdev_rx,
            driver_state,
            slot_pool,
            last_frame: None,
        })
    }

    fn create_window(&mut self, _width: u32, _height: u32, _title: &str) -> io::Result<()> {
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

        // Size 0 + all four anchors = stretch over the whole output, and an
        // exclusive zone of -1 ignores panels, so surface coordinates are
        // screen coordinates (which evdev cursor tracking relies on). The
        // configured window size is only a placeholder until the first configure.
        layer_surface.set_size(0, 0);
        layer_surface.set_exclusive_zone(-1);
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

        let src_tex = &sprite_data.sprite_sheet.texture;
        let (src_w, src_h) = src_tex.dimensions();
        let (uv_x, uv_y, uv_w, uv_h) = sprite_data.uv_rect;
        let origin_x = sprite_data.position.0 as i32;
        let origin_y = sprite_data.position.1 as i32;
        let scale_x = sprite_data.scale.0 * sprite_data.sprite_sheet.render_scale;
        let scale_y = sprite_data.scale.1 * sprite_data.sprite_sheet.render_scale;
        let dest_w = (uv_w as f32 * scale_x) as i32;
        let dest_h = (uv_h as f32 * scale_y) as i32;

        // Nothing changed since the last commit: don't make the compositor
        // recomposite the screen for an identical frame.
        let frame = FrameKey {
            window: (width, height),
            uv_rect: sprite_data.uv_rect,
            dest: (origin_x, origin_y, dest_w, dest_h),
        };
        if self.last_frame == Some(frame) {
            return Ok(());
        }

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

        // Hitbox cells (relative to the sprite origin) that contain visible
        // pixels; they become the surface's input region.
        let cell_cols = ((dest_w.max(0) + HITBOX_CELL_PX - 1) / HITBOX_CELL_PX) as usize;
        let cell_rows = ((dest_h.max(0) + HITBOX_CELL_PX - 1) / HITBOX_CELL_PX) as usize;
        let mut hitbox = vec![false; cell_cols * cell_rows];

        // Blit the sprite tile onto the canvas.
        if dest_w > 0 && dest_h > 0 {
            for dy in 0..dest_h {
                let target_y = origin_y + dy;
                if target_y < 0 || target_y >= height {
                    continue;
                }
                let src_rel_y = (dy as f32 / scale_y) as u32;
                let src_y = uv_y + src_rel_y.min(uv_h - 1);
                if src_y >= src_h {
                    continue;
                }

                for dx in 0..dest_w {
                    let target_x = origin_x + dx;
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
                        if alpha >= HITBOX_ALPHA_THRESHOLD {
                            let cell = (dy / HITBOX_CELL_PX) as usize * cell_cols
                                + (dx / HITBOX_CELL_PX) as usize;
                            hitbox[cell] = true;
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

        // Damage only where the sprite was and where it is now.
        match self.last_frame {
            Some(last) if last.window == frame.window => {
                for (x, y, w, h) in [last.dest, frame.dest] {
                    surface.damage(x, y, w, h);
                }
            }
            _ => surface.damage(0, 0, width, height),
        }

        // Input region: only the monkey's visible body, so clicks and scrolls
        // everywhere else — including right next to the monkey — go straight
        // to the window underneath. Dragging still works because Wayland's
        // implicit grab keeps sending us pointer events until the button is
        // released, even once the pointer leaves this region. With
        // `click_through` the monkey takes no pointer input at all.
        let region = self.driver_state.compositor_state.wl_compositor().create_region(&self.queue_handle, ());
        if !self.config.click_through {
            for row in 0..cell_rows {
                let mut col = 0;
                while col < cell_cols {
                    if !hitbox[row * cell_cols + col] {
                        col += 1;
                        continue;
                    }
                    let run_start = col;
                    while col < cell_cols && hitbox[row * cell_cols + col] {
                        col += 1;
                    }
                    region.add(
                        origin_x + run_start as i32 * HITBOX_CELL_PX,
                        origin_y + row as i32 * HITBOX_CELL_PX,
                        (col - run_start) as i32 * HITBOX_CELL_PX,
                        HITBOX_CELL_PX,
                    );
                }
            }
        }
        surface.set_input_region(Some(&region));
        region.destroy();

        surface.commit();
        self.connection
            .flush()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
        self.last_frame = Some(frame);

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
                    if s.evdev_pointers == 0 || s.is_dragging {
                        s.pending_events
                            .push(InputEvent::MouseMove { x, y });
                    }
                }
                PointerEventKind::Leave { .. } => {
                    // Cursor left our input region. evdev (if active) continues
                    // tracking independently. Leaving mid-drag means the
                    // compositor ended the implicit grab without a release
                    // (e.g. a workspace switch): drop the monkey rather than
                    // leave it stuck to the cursor.
                    if s.is_pointer_down {
                        s.is_pointer_down = false;
                        s.is_dragging = false;
                        let (px, py) = s.pointer_pos;
                        s.pending_events.push(InputEvent::MouseUp {
                            x: px,
                            y: py,
                            button: MouseButton::Left,
                        });
                    }
                }
                PointerEventKind::Motion { .. } => {
                    s.pointer_pos = (x, y);
                    // Recalibrate evdev from Wayland's authoritative position.
                    s.global_cursor_pos = (x, y);
                    if s.evdev_pointers == 0 || s.is_dragging {
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
                _ => {}
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
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyboard_keys_exclude_mouse_and_touch_buttons() {
        assert!(is_keyboard_key(Key::KEY_A.code()));
        assert!(is_keyboard_key(Key::KEY_SPACE.code()));
        assert!(is_keyboard_key(Key::KEY_VOLUMEUP.code()));
        assert!(!is_keyboard_key(Key::BTN_LEFT.code()));
        assert!(!is_keyboard_key(Key::BTN_TOUCH.code()));
        assert!(!is_keyboard_key(Key::BTN_TOOL_FINGER.code()));
    }

    /// Feeds one evdev frame (touch state + position) and returns the delta.
    fn frame(tp: &mut TouchpadTracker, x: i32, y: i32) -> (i32, i32) {
        tp.on_abs(AbsoluteAxisType::ABS_X.0, x);
        tp.on_abs(AbsoluteAxisType::ABS_Y.0, y);
        tp.on_sync()
    }

    #[test]
    fn touchpad_single_finger_moves_cursor() {
        let mut tp = TouchpadTracker::default();
        tp.on_key(Key::BTN_TOUCH.code(), 1);
        tp.on_key(Key::BTN_TOOL_FINGER.code(), 1);
        assert_eq!(frame(&mut tp, 100, 100), (0, 0), "first contact must not jump");
        assert_eq!(frame(&mut tp, 130, 90), (30, -10));
        assert_eq!(frame(&mut tp, 140, 90), (10, 0));
    }

    #[test]
    fn touchpad_ignores_two_finger_scroll_and_lift() {
        let mut tp = TouchpadTracker::default();
        tp.on_key(Key::BTN_TOUCH.code(), 1);
        tp.on_key(Key::BTN_TOOL_FINGER.code(), 1);
        frame(&mut tp, 100, 100);

        // Second finger lands: two-finger scroll must not move the cursor.
        tp.on_key(Key::BTN_TOOL_FINGER.code(), 0);
        tp.on_key(Key::BTN_TOOL_DOUBLETAP.code(), 1);
        assert_eq!(frame(&mut tp, 100, 300), (0, 0));
        assert_eq!(frame(&mut tp, 100, 500), (0, 0));

        // Back to one finger: resumes from the new position without a jump.
        tp.on_key(Key::BTN_TOOL_DOUBLETAP.code(), 0);
        tp.on_key(Key::BTN_TOOL_FINGER.code(), 1);
        assert_eq!(frame(&mut tp, 400, 500), (0, 0));
        assert_eq!(frame(&mut tp, 405, 500), (5, 0));

        // Lift and touch down elsewhere: no jump either.
        tp.on_key(Key::BTN_TOUCH.code(), 0);
        tp.on_key(Key::BTN_TOOL_FINGER.code(), 0);
        assert_eq!(frame(&mut tp, 405, 500), (0, 0));
        tp.on_key(Key::BTN_TOUCH.code(), 1);
        tp.on_key(Key::BTN_TOOL_FINGER.code(), 1);
        assert_eq!(frame(&mut tp, 900, 100), (0, 0));
        assert_eq!(frame(&mut tp, 901, 102), (1, 2));
    }
}
