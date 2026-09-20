//! Binary entry point: parse arguments, build the app, pick a driver, run.

use std::process::ExitCode;
use std::time::Duration;

use monkey_companion::app::App;
use monkey_companion::cli::{Cli, Command};
use monkey_companion::config::AppConfig;
use monkey_companion::error::Result;
use monkey_companion::platform::headless::HeadlessDriver;
use monkey_companion::platform::PlatformDriver;
use monkey_companion::shutdown::ShutdownSignal;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            // The logger may not be up yet, and a startup failure is something
            // the user needs to see even with logging turned off.
            eprintln!("monkey_companion: {error}");
            let mut source = std::error::Error::source(&error);
            while let Some(cause) = source {
                eprintln!("  caused by: {cause}");
                source = cause.source();
            }
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let cli = Cli::from_env()?;

    if let Command::Print(text) = &cli.command {
        print!("{text}");
        return Ok(());
    }

    init_logging(cli.verbose);

    let config = match &cli.config_path {
        Some(path) => AppConfig::load(path)?,
        None => AppConfig::load_default()?,
    };
    log::debug!("effective configuration: {config:?}");

    if cli.command == Command::PrintConfig {
        match toml::to_string_pretty(&config) {
            Ok(text) => print!("{text}"),
            Err(error) => log::error!("cannot serialise config: {error}"),
        }
        return Ok(());
    }

    let mut app = App::new(config)?;

    match cli.command {
        Command::Check => {
            let config = app.config();
            println!("configuration: {}", describe_source(config));
            println!("sprite sheet:  {}", config.resolved_sheet_path().display());
            println!("tick rate:     {} Hz", config.tick_rate_hz);
            println!("everything loads.");
            Ok(())
        }
        Command::SelfTest => {
            let mut driver = HeadlessDriver::new(1920, 1080);
            app.run_fixed(&mut driver, 600, Duration::from_micros(16_667))?;
            println!(
                "self-test: {} frame(s) rendered, final state {}, position {:?}",
                app.frames(),
                app.monkey().state(),
                app.monkey().position()
            );
            Ok(())
        }
        _ => {
            let shutdown = ShutdownSignal::new();
            shutdown.install_handlers();
            let mut driver = platform_driver(app.config())?;
            app.run(driver.as_mut(), &shutdown)
        }
    }
}

/// Build the driver for this target.
#[allow(unused_variables)]
fn platform_driver(config: &AppConfig) -> Result<Box<dyn PlatformDriver>> {
    #[cfg(target_os = "linux")]
    {
        Ok(Box::new(
            monkey_companion::platform::wayland::WaylandDriver::new(config)?,
        ))
    }
    #[cfg(target_os = "windows")]
    {
        Ok(Box::new(
            monkey_companion::platform::win32::Win32Driver::new(config)?,
        ))
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        Err(monkey_companion::error::Error::Platform(format!(
            "{} is not supported; only Linux (Wayland) and Windows have drivers. \
             Use --self-test to exercise the engine on this platform.",
            std::env::consts::OS
        )))
    }
}

fn describe_source(config: &AppConfig) -> String {
    config.source.as_ref().map_or_else(
        || "built-in defaults".to_string(),
        |path| path.display().to_string(),
    )
}

/// Set up logging. `RUST_LOG` always wins, so an operator can turn the noise up
/// or down without restarting with different flags.
fn init_logging(verbose: bool) {
    let default = if verbose { "debug" } else { "info" };
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(default)).init();
}
