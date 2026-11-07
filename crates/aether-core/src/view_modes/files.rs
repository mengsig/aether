use super::{Catalog, Definition, Diagnostic, Mode, RESERVED_NAMES, valid_name};
use std::io::Read;
use std::path::Path;

const MAX_BYTES: u64 = 64 * 1024;
const MAX_FILES: usize = 128;

pub fn discover(config: &Path) -> Catalog {
    let mut catalog = Catalog::default();
    let dir = config.join("modes");
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return catalog,
        Err(e) => {
            catalog.diagnostics.push(Diagnostic {
                source: dir.display().to_string(),
                reason: e.to_string(),
            });
            return catalog;
        }
    };
    let mut paths = Vec::new();
    for entry in entries {
        match entry {
            Ok(entry) if entry.path().extension().is_some_and(|ext| ext == "toml") => {
                paths.push(entry.path())
            }
            Ok(_) => {}
            Err(e) => catalog.diagnostics.push(Diagnostic {
                source: dir.display().to_string(),
                reason: e.to_string(),
            }),
        }
        if paths.len() > MAX_FILES {
            catalog.diagnostics.push(Diagnostic {
                source: dir.display().to_string(),
                reason: format!("more than {MAX_FILES} mode files; external catalog rejected"),
            });
            return catalog;
        }
    }
    paths.sort();
    for path in paths {
        let source = path.display().to_string();
        match load(&path) {
            Ok(mode) => {
                catalog.modes.insert(mode.name.clone(), mode);
            }
            Err(reason) => catalog.diagnostics.push(Diagnostic { source, reason }),
        }
    }
    catalog
}

fn load(path: &Path) -> Result<Mode, String> {
    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or("filename must be UTF-8")?;
    if !valid_name(name) {
        return Err("name must match [a-z][a-z0-9_-]{0,47}".into());
    }
    if RESERVED_NAMES.contains(&name) {
        return Err("name is reserved by a built-in mode or command".into());
    }
    if !std::fs::symlink_metadata(path)
        .map_err(|e| e.to_string())?
        .is_file()
    {
        return Err("mode must be a regular file, not a symlink".into());
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(
            (rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32,
        );
    }
    let file = options.open(path).map_err(|e| e.to_string())?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("mode must be a regular file".into());
    }
    let mut text = String::new();
    file.take(MAX_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|e| e.to_string())?;
    if text.len() as u64 > MAX_BYTES {
        return Err("mode exceeds 64 KiB".into());
    }
    let definition = Definition::parse(&text)?;
    Ok(Mode {
        name: name.into(),
        source: path.display().to_string(),
        definition,
    })
}
