//! Cooperative shutdown shared between the engine loop and signal handlers.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// A flag the engine loop checks once per tick.
#[derive(Debug, Clone, Default)]
pub struct ShutdownSignal {
    flag: Arc<AtomicBool>,
}

impl ShutdownSignal {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Ask the loop to stop at the end of the current tick.
    pub fn trigger(&self) {
        self.flag.store(true, Ordering::SeqCst);
    }

    #[must_use]
    pub fn is_triggered(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    /// Install handlers so Ctrl-C and a polite `kill` shut the overlay down
    /// instead of killing it mid-frame.
    ///
    /// On Unix this uses `signal(2)` with a handler that only touches an
    /// `AtomicBool`, which is async-signal-safe. On other platforms it is a
    /// no-op: the Win32 driver already exits on `WM_CLOSE`.
    pub fn install_handlers(&self) {
        #[cfg(target_os = "linux")]
        {
            // The handler cannot take arguments, so the flag lives in a static
            // that `trigger_global` sets and this signal shares.
            GLOBAL
                .set(self.clone())
                .unwrap_or_else(|_| log::debug!("signal handlers already installed"));

            // SAFETY: `handle_signal` only stores into an `AtomicBool`, which is
            // safe to call from a signal handler.
            unsafe {
                libc::signal(
                    libc::SIGINT,
                    handle_signal as *const () as libc::sighandler_t,
                );
                libc::signal(
                    libc::SIGTERM,
                    handle_signal as *const () as libc::sighandler_t,
                );
                // A dead pipe on stdout/stderr must not abort the process.
                libc::signal(libc::SIGPIPE, libc::SIG_IGN);
            }
        }
    }
}

#[cfg(target_os = "linux")]
static GLOBAL: std::sync::OnceLock<ShutdownSignal> = std::sync::OnceLock::new();

#[cfg(target_os = "linux")]
extern "C" fn handle_signal(_signal: libc::c_int) {
    if let Some(shutdown) = GLOBAL.get() {
        shutdown.trigger();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn triggering_is_visible_through_every_clone() {
        let signal = ShutdownSignal::new();
        let clone = signal.clone();
        assert!(!signal.is_triggered());
        clone.trigger();
        assert!(signal.is_triggered());
    }
}
