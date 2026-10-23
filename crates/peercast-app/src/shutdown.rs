/// Platform-specific OS shutdown signal handling (REQ-067).
///
/// On Windows: handles `Ctrl+C`, `Ctrl+Break`, and console `Close` events.
/// On Unix: handles `SIGINT` and `SIGTERM`.
pub async fn wait_for_os_shutdown_signal() -> &'static str {
    #[cfg(windows)]
    {
        use tokio::signal::windows::{ctrl_break, ctrl_c, ctrl_close};

        let mut c = ctrl_c().expect("failed to install Windows ctrl_c handler");
        let mut b = ctrl_break().expect("failed to install Windows ctrl_break handler");
        let mut cl = ctrl_close().expect("failed to install Windows ctrl_close handler");

        tokio::select! {
            _ = c.recv() => "Windows Ctrl+C",
            _ = b.recv() => "Windows Ctrl+Break",
            _ = cl.recv() => "Windows Close",
        }
    }

    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};

        let mut sigint = signal(SignalKind::interrupt()).expect("failed to install SIGINT handler");
        let mut sigterm = signal(SignalKind::terminate()).expect("failed to install SIGTERM handler");

        tokio::select! {
            _ = sigint.recv() => "POSIX SIGINT",
            _ = sigterm.recv() => "POSIX SIGTERM",
        }
    }

    #[cfg(not(any(windows, unix)))]
    {
        tokio::signal::ctrl_c().await.expect("failed to install ctrl_c handler");
        "Generic Ctrl+C"
    }
}

/// Verification metadata describing platform-specific signal capabilities.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformSignalSupport {
    pub os: &'static str,
    pub supported_signals: &'static [&'static str],
}

impl PlatformSignalSupport {
    pub fn current() -> Self {
        #[cfg(windows)]
        {
            Self {
                os: "windows",
                supported_signals: &["Ctrl+C", "Ctrl+Break", "Close"],
            }
        }
        #[cfg(unix)]
        {
            Self {
                os: "unix",
                supported_signals: &["SIGINT", "SIGTERM"],
            }
        }
        #[cfg(not(any(windows, unix)))]
        {
            Self {
                os: "other",
                supported_signals: &["Ctrl+C"],
            }
        }
    }
}
