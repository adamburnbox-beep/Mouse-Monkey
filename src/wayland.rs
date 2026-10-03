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

use crate::touchpad::Touchpad;
use evdev::{AbsoluteAxisType, Device, EventType, PropType, RelativeAxisType};

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

// ── Global input via evdev ──────────────────────────────────────────────────
//
// Wayland never tells a client about input aimed at other windows, so the only
// way to react to typing and mouse activity everywhere is to read the kernel's
// input devices (/dev/input/event*) directly. That needs read access to them,
// normally by being in the `input` group.

/// Input devices the monkey can read.
struct InputDevices {
    keyboards: Vec<Device>,
    /// Mice and touchpads; touchpads carry a tracker that turns absolute
    /// finger positions into relative motion and two-finger scrolling.
    pointers: Vec<(Device, Option<Touchpad>)>,
    permission_denied: usize,
    /// One human-readable line per device, for `--check-input`.
    report: Vec<String>,
}

fn is_touchpad(dev: &Device) -> bool {
    let has_xy = dev.supported_absolute_axes().map_or(false, |axes| {
        axes.contains(AbsoluteAxisType::ABS_X) && axes.contains(AbsoluteAxisType::ABS_Y)
    });
    let has_finger = dev
        .supported_keys()
        .map_or(false, |keys| keys.contains(evdev::Key::BTN_TOOL_FINGER));
    // Touchscreens and tablets map directly to the screen; skip those.
    has_xy && has_finger && !dev.properties().contains(PropType::DIRECT)
}

fn touchpad_tracker(dev: &Device) -> Touchpad {
    match dev.get_abs_state() {
        Ok(abs) => {
            let x = abs[AbsoluteAxisType::ABS_X.0 as usize];
            let y = abs[AbsoluteAxisType::ABS_Y.0 as usize];
            Touchpad::new((x.resolution, y.resolution), (x.maximum - x.minimum, y.maximum - y.minimum))
        }
        Err(_) => Touchpad::new((0, 0), (0, 0)),
    }
}

fn discover_input_devices() -> InputDevices {
    let mut found = InputDevices {
        keyboards: Vec::new(),
        pointers: Vec::new(),
        permission_denied: 0,
        report: Vec::new(),
    };
    let mut paths: Vec<_> = match std::fs::read_dir("/dev/input") {
        Ok(entries) => entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.file_name().and_then(|n| n.to_str()).map_or(false, |s| s.starts_with("event")))
            .collect(),
        Err(e) => {
            found.report.push(format!("cannot list /dev/input: {}", e));
            return found;
        }
    };
    paths.sort_by_key(|p| {
        p.to_string_lossy().trim_start_matches("/dev/input/event").parse::<u32>().unwrap_or(u32::MAX)
    });

    for path in &paths {
        let dev = match Device::open(path) {
            Ok(dev) => dev,
            Err(e) => {
                if e.kind() == io::ErrorKind::PermissionDenied {
                    found.permission_denied += 1;
                }
                found.report.push(format!("{}: cannot open ({})", path.display(), e));
                continue;
            }
        };
        let name = dev.name().unwrap_or("?").to_string();
        let is_keyboard = dev.supported_events().contains(EventType::KEY)
            && dev.supported_keys().map_or(false, |keys| keys.contains(evdev::Key::KEY_A));
        let is_mouse = dev.supported_relative_axes().map_or(false, |axes| {
            axes.contains(RelativeAxisType::REL_X) && axes.contains(RelativeAxisType::REL_Y)
        });
        let touchpad = is_touchpad(&dev);
        let roles: Vec<&str> = [(is_keyboard, "keyboard"), (is_mouse, "mouse"), (touchpad, "touchpad")]
            .iter()
            .filter(|(yes, _)| *yes)
            .map(|(_, role)| *role)
            .collect();
        found.report.push(format!(
            "{}: {} ({})",
            path.display(),
            name,
            if roles.is_empty() { "not used".to_string() } else { roles.join(" + ") }
        ));

        let pointer = is_mouse || touchpad;
        let tracker = |d: &Device| if touchpad { Some(touchpad_tracker(d)) } else { None };
        if is_keyboard {
            found.keyboards.push(dev);
            // A combo device gets a second handle for its pointer role.
            if pointer {
                if let Ok(second) = Device::open(path) {
                    let t = tracker(&second);
                    found.pointers.push((second, t));
                }
            }
        } else if pointer {
            let t = tracker(&dev);
            found.pointers.push((dev, t));
        }
    }
    found
}

fn key_kind(code: u16) -> KeyKind {
    match evdev::Key(code) {
        evdev::Key::KEY_SPACE | evdev::Key::KEY_ENTER | evdev::Key::KEY_KPENTER => KeyKind::Thump,
        _ => KeyKind::Other,
    }
}

/// Sums one batch of pointer events into (dx, dy, scroll notches; positive = down).
fn read_pointer_batch(
    events: impl Iterator<Item = evdev::InputEvent>,
    touchpad: &mut Option<Touchpad>,
) -> (f32, f32, f32) {
    let (mut dx, mut dy, mut scroll) = (0.0f32, 0.0f32, 0.0f32);
    for ev in events {
        match ev.event_type() {
            EventType::RELATIVE => match RelativeAxisType(ev.code()) {
                RelativeAxisType::REL_X => dx += ev.value() as f32,
                RelativeAxisType::REL_Y => dy += ev.value() as f32,
                // evdev reports wheel-up as positive
                RelativeAxisType::REL_WHEEL => scroll -= ev.value() as f32,
                _ => {}
            },
            EventType::KEY => {
                if let Some(t) = touchpad.as_mut() {
                    t.key(ev.code(), ev.value());
                }
            }
            EventType::ABSOLUTE => {
                if let Some(t) = touchpad.as_mut() {
                    t.abs(ev.code(), ev.value());
                }
            }
            EventType::SYNCHRONIZATION => {
                if let Some(t) = touchpad.as_mut() {
                    let r = t.sync();
                    dx += r.dx;
                    dy += r.dy;
                    scroll += r.scroll;
                }
            }
            _ => {}
        }
    }
    (dx, dy, scroll)
}

#[derive(Debug, PartialEq)]
enum InputGroup {
    Active,
    NeedsRelogin,
    NotMember,
    Unknown,
}

fn input_group_status() -> InputGroup {
    let Ok(group_file) = std::fs::read_to_string("/etc/group") else {
        return InputGroup::Unknown;
    };
    let mut ids = vec![0 as libc::gid_t; 256];
    let n = unsafe { libc::getgroups(ids.len() as i32, ids.as_mut_ptr()) };
    ids.truncate(n.max(0) as usize);
    ids.push(unsafe { libc::getegid() });
    group_status_from(&group_file, &std::env::var("USER").unwrap_or_default(), &ids)
}

/// `process_gids` are the groups this process actually runs with; /etc/group
/// says who *will* have them at their next login.
fn group_status_from(group_file: &str, user: &str, process_gids: &[u32]) -> InputGroup {
    let Some(line) = group_file.lines().find(|l| l.starts_with("input:")) else {
        return InputGroup::Unknown;
    };
    let fields: Vec<&str> = line.split(':').collect();
    let Some(gid) = fields.get(2).and_then(|g| g.parse::<u32>().ok()) else {
        return InputGroup::Unknown;
    };
    if process_gids.contains(&gid) {
        return InputGroup::Active;
    }
    let members = fields.get(3).copied().unwrap_or("");
    if !user.is_empty() && members.split(',').any(|m| m == user) {
        InputGroup::NeedsRelogin
    } else {
        InputGroup::NotMember
    }
}

fn access_fix() -> String {
    let start_now = "sg input -c \"cargo run --release\"";
    match input_group_status() {
        InputGroup::NotMember => format!(
            "To fix, give your user access to input devices:\n\
             \x20     sudo usermod -aG input $USER\n\
             \x20 then log out and back in. To try it right away without logging out,\n\
             \x20 start the monkey with:  {}",
            start_now
        ),
        InputGroup::NeedsRelogin => format!(
            "You're in the `input` group, but this login session started before you were\n\
             \x20 added. Log out and back in, or start the monkey with:  {}",
            start_now
        ),
        InputGroup::Active => "This process is in the `input` group but access was still refused;\n\
             \x20 check `ls -l /dev/input/event*` (they should be readable by group `input`)."
            .to_string(),
        InputGroup::Unknown => {
            "To fix: `sudo usermod -aG input $USER`, then log out and back in.".to_string()
        }
    }
}

/// Printed on every start so missing access is impossible to miss.
fn print_input_summary(devices: &InputDevices) {
    let count = |n: usize, what: &str| format!("OK ({} {})", n, what);
    let touchpads = devices.pointers.iter().filter(|(_, t)| t.is_some()).count();
    eprintln!("Mouse Monkey input access:");
    eprintln!(
        "  keyboard        {}",
        if devices.keyboards.is_empty() {
            "NOT AVAILABLE - he won't react to typing".to_string()
        } else {
            count(devices.keyboards.len(), "device(s)")
        }
    );
    eprintln!(
        "  mouse/touchpad  {}",
        if devices.pointers.is_empty() {
            "NOT AVAILABLE - he only notices the cursor while it's over him, and won't react to scrolling"
                .to_string()
        } else {
            count(devices.pointers.len(), &format!("device(s), {} touchpad(s)", touchpads))
        }
    );
    if devices.permission_denied > 0 && (devices.keyboards.is_empty() || devices.pointers.is_empty()) {
        eprintln!("  {}", access_fix());
    }
    if devices.keyboards.is_empty() || devices.pointers.is_empty() {
        eprintln!("  Test your devices with:  cargo run --release -- --check-input");
    }
}

/// `--check-input`: lists input devices and listens briefly to prove which ones
/// deliver keys, motion and scrolling.
pub fn check_input() {
    let devices = discover_input_devices();
    println!("Input devices under /dev/input:");
    for line in &devices.report {
        println!("  {}", line);
    }
    println!();
    print_input_summary(&devices);
    if devices.keyboards.is_empty() && devices.pointers.is_empty() {
        return;
    }

    const LISTEN_S: u64 = 10;
    println!();
    println!("Listening for {} seconds: type a few keys, move the mouse or touchpad, and scroll...", LISTEN_S);
    let (tx, rx) = crossbeam_channel::unbounded::<(String, &'static str)>();
    for mut dev in devices.keyboards {
        let tx = tx.clone();
        thread::spawn(move || {
            let name = dev.name().unwrap_or("?").to_string();
            while let Ok(events) = dev.fetch_events() {
                for ev in events {
                    if ev.event_type() == EventType::KEY && ev.value() == 1 {
                        let _ = tx.send((name.clone(), "key presses"));
                    }
                }
            }
        });
    }
    for (mut dev, mut touchpad) in devices.pointers {
        let tx = tx.clone();
        thread::spawn(move || {
            let name = dev.name().unwrap_or("?").to_string();
            while let Ok(events) = dev.fetch_events() {
                let (dx, dy, scroll) = read_pointer_batch(events, &mut touchpad);
                if dx != 0.0 || dy != 0.0 {
                    let _ = tx.send((name.clone(), "pointer moves"));
                }
                if scroll != 0.0 {
                    let _ = tx.send((name.clone(), "scroll notches"));
                }
            }
        });
    }
    drop(tx);

    let deadline = std::time::Instant::now() + Duration::from_secs(LISTEN_S);
    let mut counts: Vec<((String, &str), usize)> = Vec::new();
    while let Ok(item) = rx.recv_deadline(deadline) {
        match counts.iter_mut().find(|(k, _)| *k == item) {
            Some((_, n)) => *n += 1,
            None => counts.push((item, 1)),
        }
    }
    println!();
    if counts.is_empty() {
        println!("Nothing was received. If you did type and move the mouse, please report this");
        println!("together with the device list above.");
    }
    for ((name, kind), n) in &counts {
        println!("  {}: {} {}", name, n, kind);
    }
    let got = |kind: &str| counts.iter().any(|((_, k), _)| *k == kind);
    println!();
    println!("Typing:        {}", if got("key presses") { "works" } else { "no key presses received" });
    println!("Cursor:        {}", if got("pointer moves") { "works" } else { "no movement received" });
    println!("Scrolling:     {}", if got("scroll notches") { "works" } else { "no scrolling received" });
    // Reader threads are blocked in fetch_events; exiting ends them.
    std::process::exit(0);
}

impl WaylandDriver {
    /// Spawns a reader thread per keyboard and per mouse/touchpad.
    /// Returns the thread handles and whether any pointer device is being read.
    fn setup_evdev_input(
        event_sender: crossbeam_channel::Sender<InputEvent>,
        wayland_state: Arc<Mutex<WaylandState>>,
    ) -> (Vec<thread::JoinHandle<()>>, bool) {
        let devices = discover_input_devices();
        for line in &devices.report {
            log::info!("evdev: {}", line);
        }
        print_input_summary(&devices);

        let mut handles = Vec::new();

        // ── Keyboard threads (one per device) ──────────────────────────────────
        // Some keyboards report the same press on two interfaces; drop a repeat
        // of the same key from another device within a few milliseconds.
        let last_press: Arc<Mutex<Option<(u16, std::time::Instant)>>> = Arc::new(Mutex::new(None));
        for mut dev in devices.keyboards {
            let sender = event_sender.clone();
            let ws = Arc::clone(&wayland_state);
            let last_press = Arc::clone(&last_press);
            handles.push(thread::spawn(move || {
                let name = dev.name().unwrap_or("?").to_string();
                let mut announced = false;
                loop {
                    if !ws.lock().unwrap().running {
                        break;
                    }
                    match dev.fetch_events() {
                        Err(e) => {
                            log::warn!("evdev: keyboard {} read error: {}", name, e);
                            thread::sleep(Duration::from_millis(500));
                        }
                        Ok(events) => {
                            for ev in events {
                                if ev.event_type() != EventType::KEY {
                                    continue;
                                }
                                let key_code = ev.code() as u32;
                                if ev.value() == 1 {
                                    {
                                        let mut last = last_press.lock().unwrap();
                                        let now = std::time::Instant::now();
                                        if let Some((code, at)) = *last {
                                            if code == ev.code() && now.duration_since(at) < Duration::from_millis(15) {
                                                continue;
                                            }
                                        }
                                        *last = Some((ev.code(), now));
                                    }
                                    if !announced {
                                        log::info!("evdev: receiving key presses from {}", name);
                                        announced = true;
                                    }
                                    let kind = key_kind(ev.code());
                                    let _ = sender.send(InputEvent::KeyDown { key_code, kind });
                                } else if ev.value() == 0 {
                                    let _ = sender.send(InputEvent::KeyUp { key_code });
                                }
                            }
                        }
                    }
                    thread::sleep(Duration::from_millis(5));
                }
            }));
        }

        // ── Pointer threads (one per mouse/touchpad) ───────────────────────────
        // Each accumulates motion into the shared cursor position. Pointer
        // events go into pending_events only (not the channel) to avoid
        // double-counting in poll_events(), which drains both.
        let has_pointer = !devices.pointers.is_empty();
        for (mut dev, mut touchpad) in devices.pointers {
            let ws = Arc::clone(&wayland_state);
            handles.push(thread::spawn(move || {
                log::info!("evdev: pointer thread started for {}", dev.name().unwrap_or("?"));
                loop {
                    if !ws.lock().unwrap().running {
                        break;
                    }
                    match dev.fetch_events() {
                        Err(e) => {
                            log::warn!("evdev: pointer read error: {}", e);
                            thread::sleep(Duration::from_millis(100));
                        }
                        Ok(events) => {
                            let (dx, dy, scroll) = read_pointer_batch(events, &mut touchpad);
                            let mut s = ws.lock().unwrap();
                            if scroll != 0.0 {
                                s.pending_events.push(InputEvent::Scroll { delta: scroll });
                            }
                            if dx != 0.0 || dy != 0.0 {
                                // Cursor coordinates are surface-local (they are
                                // recalibrated from Wayland pointer events), so
                                // keep them within our overlay surface.
                                let max_x = s.window_size.0 as f32 - 1.0;
                                let max_y = s.window_size.1 as f32 - 1.0;
                                let (cx, cy) = s.global_cursor_pos;
                                let pos = ((cx + dx).clamp(0.0, max_x), (cy + dy).clamp(0.0, max_y));
                                s.global_cursor_pos = pos;
                                s.pending_events.push(InputEvent::MouseMove { x: pos.0, y: pos.1 });
                            }
                        }
                    }
                    thread::sleep(Duration::from_millis(5));
                }
            }));
        }

        (handles, has_pointer)
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

        // Input region: only the monkey's own body takes clicks and scrolls;
        // everything else passes through to the windows underneath. (Global
        // reactions come from evdev, never from widening this region.) During
        // a drag the whole surface takes input so pointer motion isn't lost.
        let is_dragging = self.wayland_state.lock().unwrap().is_dragging;
        let region = self.driver_state.compositor_state.wl_compositor().create_region(&self.queue_handle, ());
        if is_dragging {
            region.add(0, 0, width, height);
        } else {
            let (pos_x, pos_y) = sprite_data.position;
            let (scale_x, scale_y) = sprite_data.scale;
            let monkey_w = (sprite_data.sprite_sheet.frame_width as f32 * scale_x).round() as i32;
            let monkey_h = (sprite_data.sprite_sheet.frame_height as f32 * scale_y).round() as i32;
            region.add(pos_x.round() as i32, pos_y.round() as i32, monkey_w, monkey_h);
        }
        surface.set_input_region(Some(&region));
        region.destroy();

        surface.commit();
        self.connection
            .flush()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

        Ok(())
    }

    /// The monkey lives in our layer surface's coordinate space: the surface
    /// covers exactly one output and every pointer position we receive is
    /// surface-local. Using the compositor's global output layout here would
    /// put the floor and walls somewhere the surface can't draw (e.g. on a
    /// second monitor, or anywhere before output info arrives).
    fn get_screen_bounds(&self) -> Vec<Rect> {
        let (width, height) = self.wayland_state.lock().unwrap().window_size;
        vec![Rect { x: 0, y: 0, width, height }]
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
#[cfg(test)]
mod tests {
    use super::*;

    const GROUPS: &str = "root:x:0:\ninput:x:104:arosa,bob\nusers:x:100:\n";

    #[test]
    fn group_membership_is_diagnosed() {
        assert_eq!(group_status_from(GROUPS, "arosa", &[1000, 104]), InputGroup::Active);
        assert_eq!(group_status_from(GROUPS, "arosa", &[1000, 27]), InputGroup::NeedsRelogin);
        assert_eq!(group_status_from(GROUPS, "carol", &[1000]), InputGroup::NotMember);
        assert_eq!(group_status_from("root:x:0:\n", "arosa", &[1000]), InputGroup::Unknown);
    }
}
