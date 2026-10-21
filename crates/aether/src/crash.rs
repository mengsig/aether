//! Installed only by the foreground daemon entry point, never by the library.

use std::path::{Path, PathBuf};
use std::sync::Once;
use std::sync::atomic::{AtomicBool, Ordering};

use aether_core::crash::{self, CrashKind};

static INSTALLED: Once = Once::new();
static PANIC_RECORDED: AtomicBool = AtomicBool::new(false);

pub fn install_hook(store: PathBuf) {
    INSTALLED.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let message = info
                .payload()
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| info.payload().downcast_ref::<&str>().copied())
                .unwrap_or("panic with a non-string payload");
            let location = info.location().map(|location| location.to_string());
            let recorded = crash::record(&store, CrashKind::Panic, message, location.as_deref());
            PANIC_RECORDED.store(recorded.is_ok(), Ordering::Relaxed);
            if let Err(error) = recorded {
                // Preserve the original panic even if its evidence cannot be persisted.
                use std::io::Write;
                if let Err(stderr_error) =
                    writeln!(std::io::stderr(), "Cannot persist daemon panic: {error}; run `aether doctor` and `aether diagnose`")
                {
                    tracing::error!(%error, %stderr_error, "daemon panic evidence was not saved");
                }
            }
            previous(info);
        }));
    });
}

#[track_caller]
pub fn record_result(store: &Path, result: anyhow::Result<()>) -> anyhow::Result<()> {
    let Err(error) = result else { return Ok(()) };
    if error.is::<aether_daemon::LockBusy>() {
        return Err(error);
    }
    if PANIC_RECORDED.load(Ordering::Relaxed)
        && error
            .downcast_ref::<tokio::task::JoinError>()
            .is_some_and(tokio::task::JoinError::is_panic)
    {
        return Err(error);
    }
    let location = std::panic::Location::caller().to_string();
    match crash::record(
        store,
        CrashKind::FatalError,
        &format!("{error:#}"),
        Some(&location),
    ) {
        Ok(()) => Err(error),
        Err(recording) => {
            Err(error.context(format!("cannot persist daemon fatal error: {recording}")))
        }
    }
}

#[cfg(test)]
mod tests;
