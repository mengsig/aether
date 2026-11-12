use std::path::{Path, PathBuf};

use aether_core::view_modes::{Diagnostic, discover};
use aether_protocol::{ErrorReply, Reply, ViewMode, ViewModes};

pub(super) async fn list(config: PathBuf) -> Result<Reply, ErrorReply> {
    super::blocking(move || read(&config)).await?
}

fn read(config: &Path) -> Result<Reply, ErrorReply> {
    let catalog = discover(config);
    let modes = catalog.ordered().map(ViewMode::from).collect();
    let diagnostics = catalog
        .diagnostics
        .into_iter()
        .map(|diagnostic| {
            let source = Path::new(&diagnostic.source)
                .file_name()
                .ok_or_else(|| ErrorReply::internal("mode diagnostic source has no filename"))?
                .to_string_lossy()
                .into_owned();
            debug_assert!(
                !source.is_empty(),
                "discovery always names a file or the modes directory"
            );
            debug_assert_eq!(Path::new(&source).components().count(), 1);
            Ok(Diagnostic {
                source,
                reason: diagnostic.reason,
            })
        })
        .collect::<Result<_, ErrorReply>>()?;
    Ok(Reply::ViewModes(ViewModes { modes, diagnostics }))
}
