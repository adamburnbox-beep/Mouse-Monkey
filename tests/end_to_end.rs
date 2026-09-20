//! End-to-end tests that drive the whole engine through its public API using
//! the headless driver — the same path `--self-test` takes.

use std::time::Duration;

use monkey_companion::app::App;
use monkey_companion::config::AppConfig;
use monkey_companion::geometry::{Rect, ScreenLayout, Vec2};
use monkey_companion::platform::headless::HeadlessDriver;
use monkey_companion::platform::{InputEvent, MouseButton, PlatformDriver};
use monkey_companion::state::PetState;

const TICK: Duration = Duration::from_micros(16_667);
const DT: f32 = 1.0 / 60.0;

fn app() -> App {
    App::new(AppConfig::default()).expect("the shipped defaults must build an app")
}

#[test]
fn the_shipped_configuration_runs_a_full_second_without_incident() {
    let mut app = app();
    let mut driver = HeadlessDriver::new(1920, 1080);
    app.run_fixed(&mut driver, 60, TICK)
        .expect("a second of ticks");
    assert_eq!(driver.rendered_frames(), 60);
    assert!(app.monkey().position().x.is_finite());
}

#[test]
fn a_full_interaction_session_ends_somewhere_sensible() {
    let mut app = app();
    let mut driver = HeadlessDriver::new(1920, 1080);
    app.start(&mut driver).expect("start");

    // Type a little: the monkey scratches.
    driver.push_event(InputEvent::KeyDown { key_code: 30 });
    app.tick(&mut driver, DT).expect("tick");
    assert_eq!(app.monkey().state(), PetState::Scratching);

    // Stop typing: it settles back down.
    for _ in 0..90 {
        app.tick(&mut driver, DT).expect("tick");
    }
    assert_ne!(app.monkey().state(), PetState::Scratching);

    // Pick it up and drop it gently somewhere else.
    let grab = app.monkey().center();
    driver.push_event(InputEvent::MouseDown {
        position: grab,
        button: MouseButton::Left,
    });
    driver.move_cursor(grab);
    app.tick(&mut driver, DT).expect("tick");
    assert_eq!(app.monkey().state(), PetState::Dragged);

    let mut cursor = grab;
    for _ in 0..60 {
        cursor = cursor + Vec2::new(4.0, 2.0);
        driver.move_cursor(cursor);
        app.tick(&mut driver, DT).expect("tick");
    }
    driver.push_event(InputEvent::MouseUp {
        position: cursor,
        button: MouseButton::Left,
    });
    app.tick(&mut driver, DT).expect("tick");
    assert_ne!(app.monkey().state(), PetState::Dragged);

    // Wherever it ended up, it is on the screen.
    let position = app.monkey().position();
    assert!((0.0..=1920.0 - 128.0).contains(&position.x), "{position:?}");
    assert!((0.0..=1080.0 - 128.0).contains(&position.y), "{position:?}");
}

#[test]
fn the_monkey_stays_on_screen_across_a_multi_monitor_layout() {
    let mut app = app();
    let layout = ScreenLayout::new(vec![
        Rect::new(0, 0, 1920, 1080),
        Rect::new(1920, 100, 1280, 1024),
    ]);
    let mut driver = HeadlessDriver::with_layout(layout);
    app.start(&mut driver).expect("start");

    // Walk the cursor across both monitors while the companion reacts.
    for step in 0..600 {
        let x = step as f32 * 5.0;
        driver.move_cursor(Vec2::new(x, 500.0));
        app.tick(&mut driver, DT).expect("tick");

        let position = app.monkey().position();
        let on_a_monitor = driver
            .screen_layout()
            .monitors()
            .iter()
            .any(|monitor| monitor.contains(position));
        assert!(
            on_a_monitor,
            "monkey left the desktop at step {step}: {position:?}"
        );
    }
}

#[test]
fn a_config_with_an_unreadable_sheet_fails_to_start_with_a_clear_message() {
    let config = AppConfig {
        sprite: monkey_companion::config::SpriteConfig {
            sheet_path: "assets/sprites/does_not_exist.png".into(),
            ..Default::default()
        },
        ..AppConfig::default()
    };
    let error = App::new(config).unwrap_err();
    let message = error.to_string();
    assert!(message.contains("does_not_exist.png"), "{message}");
}

#[test]
fn shutdown_stops_the_loop() {
    let mut app = app();
    let mut driver = HeadlessDriver::new(1920, 1080);
    let shutdown = monkey_companion::shutdown::ShutdownSignal::new();
    shutdown.trigger();
    app.run(&mut driver, &shutdown).expect("run");
    // The loop checks the signal before the first tick, so nothing is drawn.
    assert_eq!(driver.rendered_frames(), 0);
}
