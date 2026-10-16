//! Preserve a complete SQLite snapshot before changing an existing schema.
use crate::{Result, StoreError, schema};
use rusqlite::Connection;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const ZSTD_LEVEL: i32 = 3;

pub(crate) fn before_migration(conn: &Connection, path: &Path) -> Result<Option<PathBuf>> {
    let Some(version) = schema::current_version(conn)? else {
        return Ok(None);
    };
    if version >= schema::SCHEMA_VERSION {
        return Ok(None);
    }
    let directory = path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("migration-backups");
    std::fs::create_dir_all(&directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
    }
    let destination = directory.join(format!(
        "schema-{version}-before-{}-{}.sqlite.zst",
        schema::SCHEMA_VERSION,
        ulid::Ulid::generate()
    ));
    let snapshot = tempfile::NamedTempFile::new_in(&directory)?;
    // SQLite's backup API includes committed WAL data and preserves row IDs,
    // which are referenced by the external-content memory search index.
    conn.backup(rusqlite::MAIN_DB, snapshot.path(), None)?;
    let length = check_snapshot(snapshot.path())?;
    let compressed = tempfile::NamedTempFile::new_in(&directory)?;
    let written = compress(&mut snapshot.reopen()?, compressed.as_file())?;
    compressed.as_file().sync_all()?;
    if written.0 != length || decompressed_digest(compressed.path())? != written {
        return Err(StoreError::Io(std::io::Error::other(
            "compressed migration backup does not decompress to the snapshot",
        )));
    }
    let file = compressed
        .persist_noclobber(&destination)
        .map_err(|error| StoreError::Io(error.error))?;
    file.sync_all()?;
    #[cfg(unix)]
    std::fs::File::open(&directory)?.sync_all()?;
    tracing::info!(from_version = version, to_version = schema::SCHEMA_VERSION, path = %destination.display(), bytes = file.metadata()?.len(), "saved compressed database before schema migration");
    Ok(Some(destination))
}

/// The snapshot's length, once its SQLite header accounts for every byte of
/// it. The header is read directly: opening a WAL-mode copy with SQLite
/// would leave `-wal` and `-shm` files beside it.
fn check_snapshot(snapshot: &Path) -> Result<u64> {
    let mut file = std::fs::File::open(snapshot)?;
    let mut header = [0; 100];
    file.read_exact(&mut header)?;
    let word = |at: usize| {
        u32::from_be_bytes([header[at], header[at + 1], header[at + 2], header[at + 3]])
    };
    let page_size = match u16::from_be_bytes([header[16], header[17]]) {
        1 => 65_536,
        size => u64::from(size),
    };
    let pages = u64::from(word(28));
    let length = file.metadata()?.len();
    let sized = &header[..16] == b"SQLite format 3\0" && word(24) == word(92);
    if !sized || pages == 0 || pages * page_size != length {
        return Err(StoreError::Io(std::io::Error::other(format!(
            "migration snapshot holds {length} bytes but its header records {pages} pages of {page_size}"
        ))));
    }
    Ok(length)
}

/// Compress `source` into `destination`, returning what was read from it.
fn compress(source: &mut impl Read, destination: &std::fs::File) -> Result<(u64, Vec<u8>)> {
    let mut encoder = zstd::stream::Encoder::new(destination, ZSTD_LEVEL)?;
    let read = hashed_copy(source, &mut encoder)?;
    encoder.finish()?.flush()?;
    Ok(read)
}

fn decompressed_digest(compressed: &Path) -> Result<(u64, Vec<u8>)> {
    let mut decoder = zstd::stream::Decoder::new(std::fs::File::open(compressed)?)?;
    hashed_copy(&mut decoder, &mut std::io::sink())
}

/// Copy `source` to `sink`, returning the length and SHA-256 of the bytes.
fn hashed_copy(source: &mut impl Read, sink: &mut impl Write) -> Result<(u64, Vec<u8>)> {
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; 1 << 20];
    let mut length = 0;
    loop {
        let read = match source.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error.into()),
        };
        hasher.update(&buffer[..read]);
        sink.write_all(&buffer[..read])?;
        length += read as u64;
    }
    Ok((length, hasher.finalize().to_vec()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_snapshot_shorter_or_longer_than_its_header_says_is_refused() {
        let directory = tempfile::tempdir().expect("directory");
        let path = directory.path().join("snapshot.db");
        let conn = Connection::open(&path).expect("database");
        conn.execute_batch("CREATE TABLE t(x); INSERT INTO t VALUES (randomblob(20000));")
            .expect("rows");
        drop(conn);
        let length = check_snapshot(&path).expect("intact snapshot");
        let file = std::fs::File::options()
            .write(true)
            .open(&path)
            .expect("open");
        file.set_len(length - 1).expect("truncate");
        assert!(check_snapshot(&path).is_err());
        file.set_len(length + 4096).expect("extend");
        assert!(check_snapshot(&path).is_err());
    }
}
