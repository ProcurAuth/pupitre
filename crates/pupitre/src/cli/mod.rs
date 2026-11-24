// SPDX-License-Identifier: MIT OR Apache-2.0
//! The command line (6.24).
//!
//! The parser is written by hand. A subcommand is one variant of `Command`, one case of
//! `parse` and one case of `run_command`.
//!
//! Exit codes: 0 success, 1 configuration error or failure, 2 command line error.
//!
//! The store commands are in `store_cmds` and the command `import` is in `import`.

mod import;
mod store_cmds;

use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};

use pupitre_core::ids::EnvId;

use crate::config::{self, CapturedEnvironment, LoadedConfig, Profile, ReadMode, ReferenceFormat};

/// Exit code of a success.
pub const EXIT_OK: u8 = 0;
/// Exit code of a configuration error or of a failure.
pub const EXIT_FAILURE: u8 = 1;
/// Exit code of a command line error.
pub const EXIT_USAGE: u8 = 2;

const USAGE: &str = "usage: pupitre <command> [options]\n\
commands:\n\
\x20 serve --config <path>\n\
\x20 check-config --config <path>\n\
\x20 print-config --config <path>\n\
\x20 print-reference --format hcl --profile local|distributed\n\
\x20 print-reference --format markdown\n\
\x20 migrate --config <path>\n\
\x20 import --config <path> --inventory-json <path> --environment <id> [--write]\n\
\x20 export --config <path> --to <folder>\n\
\x20 copy-store --from <path> --to <path>\n\
\x20 version                           (also --version and -V)\n\
\x20 help                              (also --help and -h)\n\
each path is absolute; the short form of --config is -c\n\
exit codes:\n\
\x20 0  success\n\
\x20 1  a configuration error, or the command failed\n\
\x20 2  the command line is not valid";

/// A parsed command line.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Command {
    Help,
    Version,
    Serve { config: PathBuf },
    CheckConfig { config: PathBuf },
    PrintConfig { config: PathBuf },
    PrintReference { format: ReferenceFormat, profile: Option<Profile> },
    Migrate { config: PathBuf },
    Import { config: PathBuf, inventory_json: PathBuf, environment: EnvId, write: bool },
    Export { config: PathBuf, to: PathBuf },
    CopyStore { from: PathBuf, to: PathBuf },
}

/// A command line that is not valid. The text is the message for the standard error.
#[derive(Debug, Clone, PartialEq, Eq)]
struct UsageError(String);

fn usage<T>(message: impl Into<String>) -> Result<T, UsageError> {
    Err(UsageError(message.into()))
}

/// The options of one command line, each at most one time.
#[derive(Debug, Default)]
struct Options {
    values: Vec<(&'static str, String)>,
    write: bool,
}

impl Options {
    fn take(&mut self, name: &str) -> Option<String> {
        let i = self.values.iter().position(|(n, _)| *n == name)?;
        Some(self.values.remove(i).1)
    }
}

/// Read the options of a command. `accepts` lists the options with a value. `--write` is a
/// flag that only `import` accepts.
fn parse_options(
    command: &str,
    args: &[OsString],
    accepts: &[&'static str],
    flag_write: bool,
) -> Result<Options, UsageError> {
    let mut options = Options::default();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let Some(text) = arg.to_str() else {
            return usage(format!("{command}: an argument is not valid UTF-8"));
        };
        if text == "--write" && flag_write {
            if options.write {
                return usage(format!("{command}: the option --write is given two times"));
            }
            options.write = true;
            continue;
        }
        let (flag, inline) = match text.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f, Some(v.to_string())),
            _ => (text, None),
        };
        let flag = if flag == "-c" { "--config" } else { flag };
        let Some(name) = accepts.iter().find(|a| **a == flag) else {
            if text.starts_with('-') {
                return usage(format!("{command}: unknown option {flag}"));
            }
            return usage(format!("{command}: unexpected argument {text}"));
        };
        let value = match inline {
            Some(v) => v,
            None => match iter.next().map(|v| v.to_str()) {
                Some(Some(v)) => v.to_string(),
                Some(None) => return usage(format!("{command}: the value of {name} is not valid UTF-8")),
                None => return usage(format!("{command}: the option {name} needs a value")),
            },
        };
        if options.values.iter().any(|(n, _)| n == name) {
            return usage(format!("{command}: the option {name} is given two times"));
        }
        options.values.push((name, value));
    }
    Ok(options)
}

/// An absolute path option. A relative path stops the command (6.24).
fn need_path(command: &str, options: &mut Options, name: &str) -> Result<PathBuf, UsageError> {
    match options.take(name) {
        Some(v) if v.starts_with('/') => Ok(PathBuf::from(v)),
        Some(v) if v.is_empty() => usage(format!("{command}: the value of {name} is empty")),
        Some(_) => usage(format!("{command}: the value of {name} is not an absolute path")),
        None => usage(format!("{command}: the option {name} <path> is necessary")),
    }
}

fn parse(args: &[OsString]) -> Result<Command, UsageError> {
    let Some(first) = args.first() else {
        return usage("no command");
    };
    let rest = &args[1..];
    let Some(name) = first.to_str() else {
        return usage("the command is not valid UTF-8");
    };
    let config_only = |command: &str| -> Result<PathBuf, UsageError> {
        let mut o = parse_options(command, rest, &["--config"], false)?;
        need_path(command, &mut o, "--config")
    };
    match name {
        "help" | "--help" | "-h" => {
            parse_options(name, rest, &[], false)?;
            Ok(Command::Help)
        }
        "version" | "--version" | "-V" => {
            parse_options(name, rest, &[], false)?;
            Ok(Command::Version)
        }
        "serve" => Ok(Command::Serve { config: config_only("serve")? }),
        "check-config" => Ok(Command::CheckConfig { config: config_only("check-config")? }),
        "print-config" => Ok(Command::PrintConfig { config: config_only("print-config")? }),
        "migrate" => Ok(Command::Migrate { config: config_only("migrate")? }),
        "print-reference" => {
            let mut o = parse_options(name, rest, &["--format", "--profile"], false)?;
            let format = match o.take("--format").as_deref() {
                Some("hcl") => ReferenceFormat::Hcl,
                Some("markdown") => ReferenceFormat::Markdown,
                Some(other) => return usage(format!("print-reference: unknown format {other:?}; use hcl or markdown")),
                None => return usage("print-reference: the option --format is necessary"),
            };
            let profile = match o.take("--profile").as_deref() {
                Some("local") => Some(Profile::Local),
                Some("distributed") => Some(Profile::Distributed),
                Some(other) => {
                    return usage(format!("print-reference: unknown profile {other:?}; use local or distributed"));
                }
                None => None,
            };
            match (format, profile) {
                (ReferenceFormat::Hcl, None) => {
                    usage("print-reference: the option --profile is necessary with --format hcl")
                }
                (ReferenceFormat::Markdown, Some(_)) => {
                    usage("print-reference: the option --profile does not apply to --format markdown")
                }
                _ => Ok(Command::PrintReference { format, profile }),
            }
        }
        "import" => {
            let mut o = parse_options(name, rest, &["--config", "--inventory-json", "--environment"], true)?;
            let config = need_path(name, &mut o, "--config")?;
            let inventory_json = need_path(name, &mut o, "--inventory-json")?;
            let environment = match o.take("--environment") {
                Some(e) => EnvId::parse(&e).or_else(|e| usage(format!("import: --environment: {e}")))?,
                None => return usage("import: the option --environment <id> is necessary"),
            };
            Ok(Command::Import { config, inventory_json, environment, write: o.write })
        }
        "export" => {
            let mut o = parse_options(name, rest, &["--config", "--to"], false)?;
            Ok(Command::Export { config: need_path(name, &mut o, "--config")?, to: need_path(name, &mut o, "--to")? })
        }
        "copy-store" => {
            let mut o = parse_options(name, rest, &["--from", "--to"], false)?;
            Ok(Command::CopyStore { from: need_path(name, &mut o, "--from")?, to: need_path(name, &mut o, "--to")? })
        }
        other => usage(format!("unknown command {other:?}")),
    }
}

fn write_errors(errors: &[config::ConfigError]) {
    let mut err = std::io::stderr().lock();
    for e in errors {
        let _ = writeln!(err, "{e}");
    }
}

fn load(path: &Path, env: &CapturedEnvironment, mode: ReadMode) -> Result<LoadedConfig, u8> {
    config::load(path, env, mode).map_err(|errors| {
        write_errors(&errors);
        EXIT_FAILURE
    })
}

fn failure(message: impl std::fmt::Display) -> u8 {
    let _ = writeln!(std::io::stderr().lock(), "{message}");
    EXIT_FAILURE
}

fn run_command(command: Command, env: &CapturedEnvironment) -> u8 {
    let mut out = std::io::stdout().lock();
    match command {
        Command::Help => {
            let _ = writeln!(out, "{USAGE}");
            EXIT_OK
        }
        Command::Version => {
            let _ = writeln!(out, "pupitre {}\nlicence: MIT OR Apache-2.0", env!("CARGO_PKG_VERSION"));
            EXIT_OK
        }
        Command::PrintReference { format, profile } => {
            let _ = write!(out, "{}", config::render_reference(format, profile.unwrap_or(Profile::Local)));
            EXIT_OK
        }
        Command::CheckConfig { config } => match load(&config, env, ReadMode::All) {
            Ok(loaded) => {
                for w in &loaded.warnings {
                    let _ = writeln!(std::io::stderr().lock(), "warning: {}: {}", w.event, w.message);
                }
                let _ = writeln!(out, "the configuration is valid");
                EXIT_OK
            }
            Err(code) => code,
        },
        Command::PrintConfig { config } => match load(&config, env, ReadMode::All) {
            Ok(loaded) => {
                let _ = write!(out, "{}", config::render_print_config(&loaded));
                EXIT_OK
            }
            Err(code) => code,
        },
        Command::Serve { config } => {
            let loaded = match load(&config, env, ReadMode::All) {
                Ok(l) => l,
                Err(code) => return code,
            };
            match runtime() {
                Ok(rt) => match rt.block_on(crate::server::serve(loaded)) {
                    Ok(()) => EXIT_OK,
                    Err(e) => failure(e),
                },
                Err(e) => failure(e),
            }
        }
        Command::Migrate { config } => match load(&config, env, ReadMode::Storage) {
            Ok(loaded) => store_cmds::migrate(&loaded),
            Err(code) => code,
        },
        Command::Export { config, to } => match load(&config, env, ReadMode::Storage) {
            Ok(loaded) => store_cmds::export(&loaded, &to),
            Err(code) => code,
        },
        Command::Import { config, inventory_json, environment, write } => match load(&config, env, ReadMode::Storage) {
            Ok(loaded) => import::import(&loaded, &inventory_json, &environment, write),
            Err(code) => code,
        },
        Command::CopyStore { from, to } => {
            let from = match load(&from, env, ReadMode::Storage) {
                Ok(l) => l,
                Err(code) => return code,
            };
            match load(&to, env, ReadMode::Storage) {
                Ok(to) => store_cmds::copy_store(&from, &to),
                Err(code) => code,
            }
        }
    }
}

fn runtime() -> std::io::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread().enable_all().build()
}

/// Run the command line. The answer is the exit code.
pub fn run<I: IntoIterator<Item = OsString>>(args: I, env: &CapturedEnvironment) -> u8 {
    let args: Vec<OsString> = args.into_iter().collect();
    match parse(&args) {
        Ok(command) => run_command(command, env),
        Err(UsageError(message)) => {
            let _ = writeln!(std::io::stderr().lock(), "{message}\n{USAGE}");
            EXIT_USAGE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(line: &str) -> Result<Command, UsageError> {
        parse(&line.split_whitespace().map(OsString::from).collect::<Vec<_>>())
    }

    #[test]
    fn the_commands_of_6_24() {
        assert_eq!(p("serve --config /etc/pupitre.hcl"), Ok(Command::Serve { config: "/etc/pupitre.hcl".into() }));
        assert_eq!(
            p("check-config -c /etc/pupitre.hcl"),
            Ok(Command::CheckConfig { config: "/etc/pupitre.hcl".into() })
        );
        assert_eq!(
            p("print-config --config=/etc/pupitre.hcl"),
            Ok(Command::PrintConfig { config: "/etc/pupitre.hcl".into() })
        );
        assert_eq!(p("--version"), Ok(Command::Version));
        assert_eq!(p("-h"), Ok(Command::Help));
        assert_eq!(
            p("print-reference --format hcl --profile distributed"),
            Ok(Command::PrintReference { format: ReferenceFormat::Hcl, profile: Some(Profile::Distributed) })
        );
        assert_eq!(
            p("import --config /c.hcl --inventory-json /i.json --environment production --write"),
            Ok(Command::Import {
                config: "/c.hcl".into(),
                inventory_json: "/i.json".into(),
                environment: EnvId::parse("production").unwrap(),
                write: true
            })
        );
        assert_eq!(
            p("copy-store --from /a.hcl --to /b.hcl"),
            Ok(Command::CopyStore { from: "/a.hcl".into(), to: "/b.hcl".into() })
        );
    }

    #[test]
    fn refusals() {
        for bad in [
            "",
            "serve",
            "serve --config pupitre.hcl",
            "serve --config /a --config /b",
            "serve --config /a --write",
            "serve --other /a",
            "print-reference --format hcl",
            "print-reference --format markdown --profile local",
            "import --config /c --inventory-json /i --environment Bad",
            "export --config /c --to relative",
            "unknown",
        ] {
            assert!(p(bad).is_err(), "{bad}");
        }
    }
}
