### Implementation Instructions

1. Isolate Linux platform code behind a `WaylandDriver` implementation, extracting inputs via `evdev`. Ensure compiling for target `windows` drops Linux-specific crates dynamically via `cfg` logic gates.
2. Do not import generalized game engine loops. All transformations, physics clamping, and canvas renders must remain lightweight to enforce a strict operating foot footprint below $45\text{MB}$ memory consumption.
3. If global OS input hooks drop due to permission boundaries under Wayland, log an explicit warning to `stderr` and fallback to localized tracking limits (capturing inputs only when interactions happen directly inside the application's immediate rendering container window boundary).Here's a comprehensive plan for developing your on-screen monkey companion, broken down into logical phases and addressing each functional requirement and system-level detail.

---

### Project Plan: On-Screen Monkey Companion

This plan outlines the development steps for the on-screen monkey companion, focusing on the specified system architecture, functional requirements, and implementation instructions.

---

### Phase 1: Core Infrastructure & Project Setup

**Objective:** Establish the foundational project structure, core engine loop, configuration parsing, and initial platform abstraction.

1. **Project Initialization:**
    
    - Create a new Rust project: `cargo new monkey_companion --bin`.
    - Add initial dependencies to `Cargo.toml`:
        - `toml`, `serde`, `serde_derive` for configuration parsing.
        - `image` or `png` for sprite sheet loading.
        - `egui` and `wgpu` (or a similar lightweight rendering backend) for cross-platform rendering.
        - `log`, `env_logger` for structured logging.
        - `rand` for random number generation.
    - Define the `PetState` enum in a central `state.rs` or `lib.rs` file.
2. **Configuration Management (`config.rs`):**
    
    - Define `AppConfig` and `SpriteConfig` structs using `#[derive(Deserialize)]`.
    - Implement a function to load and parse the `monkey_companion.toml` file at application startup.
    - Handle potential `IO` errors during config loading.
3. **Core Engine Loop (`main.rs` or `engine.rs`):**
    
    - Implement the main application loop, ensuring a fixed 60Hz tick rate. This can be achieved using `std::time::Instant` and `std::thread::sleep` to control frame timing.
    - Structure the loop to clearly separate:
        - Input processing (delegated to platform driver).
        - State updates (physics, animations, state transitions).
        - Rendering (delegated to platform driver).
4. **Sprite Engine Base (`sprite_renderer.rs`):**
    
    - Implement a `SpriteSheet` struct to hold the loaded texture, frame dimensions (`frame_width`, `frame_height`), and `render_scale`.
    - **01. Static Asset Validation:**
        - At initialization, load the image specified by `sheet_path` (`assets/sprites/monkey_brown.png`).
        - Validate the image's overall dimensions are multiples of `frame_width` and `frame_height`.
        - Implement a hard-fail mechanism (e.g., `panic!`) with an explicit `IO` trace if the asset sheet is corrupt or missing, or dimensions are incorrect.
        - Ensure the design prevents any runtime asset modifications or texture swaps.

---

### Phase 2: Platform Abstraction & Input Handling

**Objective:** Implement platform-specific windowing, rendering, and global input capture, adhering to conditional compilation and memory constraints.

1. **Platform Driver Trait (`platform.rs`):**
    
    - Define a `PlatformDriver` trait with methods for:
        - `new(config: &AppConfig) -> Self`
        - `create_window(width, height, title)`
        - `poll_events() -> Vec<InputEvent>` (e.g., mouse move, mouse down/up, key down/up)
        - `render_frame(sprite_data: &SpriteData)`
        - `get_screen_bounds() -> Vec<Rect>` (for all connected monitors)
        - `set_window_position(x, y)`
        - `set_window_size(width, height)`
        - `is_running() -> bool`
        - `log_warning_stderr(message: &str)`
        - `get_cursor_pos() -> (f32, f32)`
2. **Linux (COSMIC/Wayland) Driver (`platform/wayland.rs`):**
    
    - Implement `WaylandDriver` using `#[cfg(target_os = "linux")]`.
    - **Windowing:**
        - **Option A (Tauri v2):** Integrate Tauri v2 for `ext-layer-shell-v1` to create a transparent, click-through overlay. This might simplify some Wayland complexities.
        - **Option B (Native `egui`/`wgpu`):** Directly use `winit` (or similar) with `wayland-client` to create a Wayland surface, configure it for transparency, and set it as a layer shell surface. Ensure input passthrough is correctly configured.
    - **Global Input:**
        - Integrate the `evdev` crate to read raw input events from `/dev/input/event*`. This will require appropriate permissions.
        - Implement the fallback mechanism: If `evdev` fails (e.g., permission denied), log an explicit warning to `stderr` and switch to localized input tracking (only events within the application's window boundaries).
    - **Monitor Boundary Mapping (09):**
        - Query Wayland layer configuration outputs at launch to get active screen layouts.
        - Cache these rigid bounding rect coordinates.
        - Implement an event listener (via Wayland protocol) for display topology changes and force a recalculation of screen bounds.
3. **Windows (Win32) Driver (`platform/win32.rs`):**
    
    - Implement `Win32Driver` using `#[cfg(target_os = "windows")]`.
    - **Windowing:**
        - Instantiate a native Win32 window using `windows-sys` (or `windows` crate) with `WS_EX_LAYERED` and `WS_EX_TRANSPARENT` extended styles.
        - Set the window's `LWA_COLORKEY` to enable transparency.
    - **Global Input:**
        - Utilize `SetWindowsHookExW` with `WH_MOUSE_LL` and `WH_KEYBOARD_LL` for low-level global input capturing. This will require careful handling of `LLHOOKPROC` callbacks and message loops.
    - **Monitor Boundary Mapping (09):**
        - Use `EnumDisplayMonitors` at launch to enumerate and get bounding rects for all connected displays.
        - Cache these coordinates.
        - Implement an event listener for `WM_DISPLAYCHANGE` messages to detect display topology changes and force a recalculation.

---

### Phase 3: Behavioral Logic & State Transitions

**Objective:** Implement all monkey-specific behaviors and state transitions as defined in the functional requirements.

1. **Monkey State Management (`monkey.rs`):**
    
    - Create a `Monkey` struct to encapsulate the current `PetState`, position `(x, y)`, velocity `(vx, vy)`, current animation frame index, and other state-specific data.
    - Implement a `tick(&mut self, input_events: &[InputEvent], dt: f32)` method that updates the monkey's state based on inputs and elapsed time. This method will contain the core state machine logic.
2. **02. Eye Follow:**
    
    - In `Idle` and `Grooming` states, calculate the vector from the monkey's localized center `(C_x, C_y)` to the global screen mouse coordinates `(M_x, M_y)`.
    - Use `f32::atan2(M_y - C_y, M_x - C_x)` to get the angle $\theta$.
    - Map the continuous $\theta$ value into 8 discrete sectors (e.g., `0-45 deg`, `45-90 deg`, etc.) to select the corresponding row/column in the sprite sheet for eye/head direction.
3. **03. Mochi Drag:**
    
    - Detect mouse-down events where the cursor is within the monkey's bounding box.
    - Transition to `Dragged` state, storing the `anchor_offset` from the mouse click to the monkey's center.
    - While in `Dragged` state and mouse-down, capture the frame-to-frame vertical displacement delta `Δy` of the mouse.
    - Apply non-uniform scaling to the rendering context:
        - `scale_y = 1.0 + (Δy * k_stretch)`
        - `scale_x = 1.0 - (Δy * k_squish)`
    - Enforce `k_squish = 0.5 * k_stretch` to maintain volume.
    - On mouse-up, transition back to `Idle` (or `Dramatic` if conditions for 08 are met).
4. **04. Monkey Hunt:**
    
    - Implement a timer to track cursor inactivity. Reset this timer on any mouse movement.
    - If the cursor coordinate variance remains exactly 0 for 300 seconds, transition from `Idle` to `Hunting`.
    - In `Hunting` state, calculate the normalized direction vector from the monkey's current position to the target cursor coordinates.
    - Translate the monkey's `(X, Y)` coordinates along this vector at a constant velocity step of `3.0 pixels/frame`.
    - Halt translation and transition back to `Idle` when the distance to the target cursor is less than `10 pixels`.
5. **05. Grooming Mode:**
    
    - Monitor global cursor position and velocity (magnitude `|v|`).
    - If the cursor is inside the monkey's bounding box and `|v|` remains under `0.05` for 120 consecutive frames, transition to `Grooming`.
    - In `Grooming` state, play a localized scratch/groom animation loop.
    - Modify the core canvas scale using a low-amplitude sine wave function (`1.0 ± 0.02`) to generate a breathing animation pulse.
    - Transition out of `Grooming` if the cursor leaves the bounding box or `|v|` exceeds the threshold.
6. **06. Keyboard Scratching:**
    
    - The platform driver intercepts global system key-down interrupts.
    - Upon _any_ key intercept, instantly transition the state machine to `Scratching`.
    - Trigger an out-of-phase rapid arm movement animation loop.
    - Initialize/reset an internal decay clock at `750ms`.
    - If the clock reaches `0` without a new key event being caught, transition the state back to `Idle`.
7. **07. Micro-Wiggles:**
    
    - In the `Idle` state, maintain a frame counter.
    - Every 500 frames, use a pseudorandom number generator (e.g., `rand::thread_rng().gen_bool(probability)`) to decide if a micro-wiggle should occur.
    - If triggered, execute a transient tail/ear twitch micro-loop (frequency: `6Hz`, pixel translation amplitude: `±2 pixels`) for exactly `400ms`. This can be a temporary state or an overlay animation.
    - After `400ms`, return the animation frame pointer to the baseline static idle layouts.
8. **08. Dramatic Reaction:**
    
    - While the pet is in the `Dragged` state, calculate cursor acceleration `vec{a}` over a moving average filter window: `vec{a} = (vec{v}_current - vec{v}_previous) / Δt`.
    - If the acceleration magnitude `|vec{a}|` spikes beyond a configured limit (e.g., a rapid mouse flick), immediately drop the cursor coordinate lock.
    - Switch the state to `Dramatic` (using a tumbling frame row animation).
    - Evaluate downward gravity translation vectors (`g = 9.8 pixels/frame^2`, scaled by engine tick variables) until the monkey's lower bounds collide with the mapped monitor floor barriers (obtained from **09**).
9. **09. Monitor Boundary Mapping (Integration):**
    
    - The `PlatformDriver` will provide the cached rigid bounding rect coordinates for all active screens.
    - In the `Monkey::tick` method, continuously clamp the monkey sprite's screen position vectors to these bounds.
    - When a display topology change event is captured by the platform driver, it should trigger a recalculation of screen layouts and then snap the sprite position safely to the closest mapped physical floor boundary vector.

---

### Phase 4: Refinement, Optimization & Testing

**Objective:** Ensure the application is stable, performs well, and meets all requirements, including the strict memory footprint.

1. **Memory Footprint Optimization:**
    
    - Regularly profile memory usage using tools like `valgrind` (Linux) or `perfmon` (Windows) with Rust's `jemalloc` or `mimalloc` allocators.
    - Identify and optimize memory-intensive operations, especially image loading and input buffers.
    - Ensure the strict operating footprint below `45MB` is met. Avoid unnecessary allocations and prefer stack-allocated data where possible.
2. **Performance Tuning:**
    
    - Profile CPU usage to identify bottlenecks in the game loop, physics calculations, and rendering.
    - Optimize algorithms for efficiency, particularly in `atan2` calculations, vector math, and state transitions.
    - Ensure rendering calls are batched efficiently.
3. **Error Handling & Logging:**
    
    - Refine error handling for all `IO` operations, platform API calls, and input device access.
    - Ensure clear and informative logging, especially for `stderr` warnings (e.g., Wayland input fallback). Use the `log` crate for structured logging.
4. **Cross-Platform Testing:**
    
    - Thoroughly test on both Linux (Wayland) and Windows environments.
    - Verify all functional requirements work correctly and consistently on both platforms.
    - Test display topology changes, multi-monitor setups, and edge cases (e.g., dragging off-screen, rapid key presses).
5. **Animation & Visual Polish:**
    
    - Ensure smooth transitions between animation frames and states.
    - Verify visual effects (stretching, breathing, wiggles, tumbling) are appealing, physically plausible, and correctly implemented.