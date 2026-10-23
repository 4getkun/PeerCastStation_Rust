use std::path::PathBuf;

use crate::config::AppConfig;

/// Command-line arguments for PeerCastStation (REQ-067).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AppCliArgs {
    /// Listening port override (`--port <PORT>` or `-p <PORT>`).
    pub port: Option<u16>,
    /// Bind address override (`--bind <ADDR>` or `-b <ADDR>`).
    pub bind_address: Option<String>,
    /// Explicit configuration file path (`--config <PATH>`, `-c <PATH>`, or `--settings <PATH>`).
    pub config_path: Option<PathBuf>,
    /// Run in headless mode without desktop integration (`--headless`).
    pub headless: bool,
    /// Send termination signal to running instance and exit (`--kill` or `-kill`).
    pub kill: bool,
    /// Allow multiple instances concurrently (`--multi` or `-multi`).
    pub multi: bool,
    /// Display help information (`--help` or `-h`).
    pub help: bool,
    /// Display version information (`--version` or `-v`).
    pub version: bool,
}

impl AppCliArgs {
    /// Parses CLI arguments from string slices.
    pub fn parse_from<I, T>(args: I) -> Result<Self, String>
    where
        I: IntoIterator<Item = T>,
        T: AsRef<str>,
    {
        let mut cli = Self::default();
        let mut iter = args.into_iter();

        // Skip binary name if present
        let _ = iter.next();

        while let Some(arg) = iter.next() {
            let s = arg.as_ref();
            match s {
                "-p" | "--port" => {
                    let val = iter
                        .next()
                        .ok_or_else(|| "--port requires a port number argument".to_string())?;
                    let port = val
                        .as_ref()
                        .parse::<u16>()
                        .map_err(|_| format!("Invalid port number: '{}'", val.as_ref()))?;
                    cli.port = Some(port);
                }
                "-b" | "--bind" => {
                    let val = iter
                        .next()
                        .ok_or_else(|| "--bind requires an IP address argument".to_string())?;
                    cli.bind_address = Some(val.as_ref().to_string());
                }
                "-c" | "--config" | "-s" | "--settings" => {
                    let val = iter
                        .next()
                        .ok_or_else(|| "--config requires a file path argument".to_string())?;
                    cli.config_path = Some(PathBuf::from(val.as_ref()));
                }
                "--headless" => {
                    cli.headless = true;
                }
                "--kill" | "-kill" => {
                    cli.kill = true;
                }
                "--multi" | "-multi" => {
                    cli.multi = true;
                }
                "-h" | "--help" => {
                    cli.help = true;
                }
                "-v" | "--version" => {
                    cli.version = true;
                }
                unknown if unknown.starts_with('-') => {
                    return Err(format!("Unrecognized option: '{}'", unknown));
                }
                pos => {
                    return Err(format!("Unexpected positional argument: '{}'", pos));
                }
            }
        }

        Ok(cli)
    }

    /// Applies CLI flag overrides to an existing `AppConfig` (CLI takes precedence over config file).
    pub fn apply_to_config(&self, config: &mut AppConfig) {
        if let Some(port) = self.port {
            config.server.port = port;
        }
        if let Some(ref bind) = self.bind_address {
            config.server.bind_address = bind.clone();
        }
        if self.headless {
            config.lifecycle.headless = true;
        }
    }

    /// Formats help text.
    pub fn help_text() -> &'static str {
        r#"PeerCastStation (Rust Edition)
Usage: peercast-station [OPTIONS]

Options:
  -p, --port <PORT>        Override listening port (default: 7144)
  -b, --bind <ADDR>        Override bind IP address (default: 127.0.0.1)
  -c, --config <PATH>      Path to configuration file
      --headless           Run in headless mode without desktop integration
      --kill, -kill        Signal running instance to terminate and exit
      --multi, -multi      Allow multiple instances to run concurrently
  -h, --help               Print help information
  -v, --version            Print version information
"#
    }
}
