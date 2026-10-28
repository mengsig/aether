use std::path::Path;

pub(crate) fn report_previous(store: &Path) {
    match aether_core::crash::acknowledge_startup(store) {
        Ok(Some(record)) => tracing::warn!("{}", record.summary()),
        Ok(None) => {}
        Err(error) => tracing::warn!(
            %error, path = %aether_core::crash::path_for(store).display(),
            "cannot acknowledge daemon crash evidence; run `aether doctor` and `aether diagnose`"
        ),
    }
}

/// Mark the store as run by this process; warn when the run before it never
/// stopped cleanly, which `aether doctor` reports until the next clean stop.
pub(crate) fn begin_run(store: &Path) {
    match aether_core::crash::shutdown::begin(store, std::process::id()) {
        Ok(Some(found)) => tracing::warn!("{}", found.summary()),
        Ok(None) => {}
        Err(error) => tracing::warn!(
            %error,
            "cannot mark the daemon run; `aether doctor` cannot tell whether it stops cleanly"
        ),
    }
}

/// The daemon stopped cleanly. A mark left behind only makes the next start
/// report an unclean stop, so failing to clear it does not fail the stop.
pub(crate) fn end_run(store: &Path) {
    if let Err(error) = aether_core::crash::shutdown::end(store) {
        tracing::warn!(%error, "cannot clear the daemon run mark; the next start reports an unclean stop");
    }
}
