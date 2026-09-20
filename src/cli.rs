//! A small hand-rolled argument parser.
//!
//! The companion takes six flags; pulling in an argument-parsing framework for
//! that would cost more binary size and dependency surface than it saves.

use std::path::PathBuf;

use crate::error::{Error, Result};

/// Usage text shown by `--help`.
pub const HELP: &str = concat!(
    "monkey_companion ",
    env!("CARGO_PKG_VERSION"),
    "\n",
    env!("CARGO_PKG_DESCRIPTION"),
    r#"

USAGE:
    monkey_companion [OPTIONS]

OPTIONS:
    -c, --config <PATH>   Use this config file instead of searching the standard locations
        --check           Load the config and sprite sheet, report what was found, then exit
        --self-test       Run the engine headlessly for a few seconds and report the result
        --print-config    Print the effective configuration as TOML and exit
    -v, --verbose         Log at debug level (same as RUST_LOG=debug)
    -h, --help            Show this help and exit
    -V, --version         Show the version and exit

ENVIRONMENT:
    RUST_LOG              Log filter, e.g. RUST_LOG=monkey_companion=debug
"#
);

/// What the process was asked to do.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Command {
    /// Run the companion (the default).
    #[default]
    Run,
    /// Validate the configuration and assets, then exit.
    Check,
    /// Run the engine against the headless driver, then exit.
    SelfTest,
    /// Print the effective configuration, then exit.
    PrintConfig,
    /// Print help or the version, then exit.
    Print(String),
}

/// Parsed command line.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Cli {
    pub command: Command,
    pub config_path: Option<PathBuf>,
    pub verbose: bool,
}

impl Cli {
    /// Parse the process arguments.
    pub fn from_env() -> Result<Self> {
        Self::parse(std::env::args_os().skip(1))
    }

    /// Parse an argument list (without the program name).
    pub fn parse<I, S>(args: I) -> Result<Self>
    where
        I: IntoIterator<Item = S>,
        S: Into<std::ffi::OsString>,
    {
        let mut cli = Self::default();
        let mut args = args.into_iter().map(Into::into);

        while let Some(raw) = args.next() {
            let arg = raw.to_string_lossy().into_owned();
            match arg.as_str() {
                "-h" | "--help" => {
                    cli.command = Command::Print(HELP.to_string());
                    return Ok(cli);
                }
                "-V" | "--version" => {
                    cli.command =
                        Command::Print(format!("monkey_companion {}\n", env!("CARGO_PKG_VERSION")));
                    return Ok(cli);
                }
                "-v" | "--verbose" => cli.verbose = true,
                "--check" => cli.command = Command::Check,
                "--self-test" => cli.command = Command::SelfTest,
                "--print-config" => cli.command = Command::PrintConfig,
                "-c" | "--config" => {
                    let value = args.next().ok_or_else(|| {
                        Error::Cli(format!(
                            "{arg} needs a path; try --config ./monkey_companion.toml"
                        ))
                    })?;
                    cli.config_path = Some(PathBuf::from(value));
                }
                other if other.starts_with("--config=") => {
                    let value = other.trim_start_matches("--config=");
                    if value.is_empty() {
                        return Err(Error::Cli("--config= needs a path".to_string()));
                    }
                    cli.config_path = Some(PathBuf::from(value));
                }
                other => {
                    return Err(Error::Cli(format!(
                        "unrecognised argument '{other}'\n\nTry 'monkey_companion --help'."
                    )));
                }
            }
        }
        Ok(cli)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_arguments_means_run() {
        let cli = Cli::parse(Vec::<String>::new()).expect("parse");
        assert_eq!(cli.command, Command::Run);
        assert_eq!(cli.config_path, None);
        assert!(!cli.verbose);
    }

    #[test]
    fn config_takes_a_path_in_either_form() {
        let split = Cli::parse(["--config", "a.toml"]).expect("parse");
        assert_eq!(split.config_path, Some(PathBuf::from("a.toml")));
        let joined = Cli::parse(["--config=b.toml"]).expect("parse");
        assert_eq!(joined.config_path, Some(PathBuf::from("b.toml")));
        let short = Cli::parse(["-c", "c.toml"]).expect("parse");
        assert_eq!(short.config_path, Some(PathBuf::from("c.toml")));
    }

    #[test]
    fn a_config_flag_with_no_value_is_an_error_that_explains_itself() {
        let error = Cli::parse(["--config"]).unwrap_err();
        assert!(error.to_string().contains("needs a path"), "{error}");
    }

    #[test]
    fn unknown_flags_are_rejected_rather_than_ignored() {
        let error = Cli::parse(["--wiggle-harder"]).unwrap_err();
        assert!(error.to_string().contains("--wiggle-harder"), "{error}");
    }

    #[test]
    fn help_and_version_short_circuit() {
        assert!(matches!(
            Cli::parse(["--help", "--nonsense"]).expect("parse").command,
            Command::Print(_)
        ));
        assert!(matches!(
            Cli::parse(["-V"]).expect("parse").command,
            Command::Print(_)
        ));
    }

    #[test]
    fn modes_are_recognised() {
        assert_eq!(
            Cli::parse(["--check"]).expect("parse").command,
            Command::Check
        );
        assert_eq!(
            Cli::parse(["--self-test"]).expect("parse").command,
            Command::SelfTest
        );
        assert_eq!(
            Cli::parse(["--print-config"]).expect("parse").command,
            Command::PrintConfig
        );
        assert!(Cli::parse(["-v"]).expect("parse").verbose);
    }
}
