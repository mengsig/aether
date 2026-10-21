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
