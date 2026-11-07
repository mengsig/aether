//! `noesis.db`: Noesis's own SQLite file in Aether's data directory. Aether
//! memory never reads it, and nothing here touches Aether's store.

use std::io;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};

mod schema;

pub use schema::SCHEMA_VERSION;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("noesis.db: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("noesis.db at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("noesis.db is corrupt: {0}")]
    Corrupt(String),
    #[error(
        "noesis.db has schema N{found}, newer than N{supported} this Aether reads; upgrade Aether"
    )]
    SchemaTooNew { found: u32, supported: u32 },
}

#[derive(Debug)]
pub struct NoesisStore {
    conn: Connection,
    path: PathBuf,
}

impl NoesisStore {
    /// Open `path`, creating it private to its owner if missing, and bring
    /// its schema up to [`SCHEMA_VERSION`].
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        make_private(path)?;
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let conn = Connection::open_with_flags(path, flags)?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;
             PRAGMA busy_timeout = 5000;
             PRAGMA temp_store = MEMORY;",
        )?;
        schema::migrate(&conn)?;
        let foreign_keys: bool = conn.query_row("PRAGMA foreign_keys", [], |row| row.get(0))?;
        if !foreign_keys {
            return Err(StoreError::Corrupt(
                "foreign keys could not be enabled".into(),
            ));
        }
        Ok(Self {
            conn,
            path: path.to_path_buf(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn schema_version(&self) -> Result<u32, StoreError> {
        let version = schema::current_version(&self.conn)?;
        version.ok_or_else(|| StoreError::Corrupt("schema_version is missing".into()))
    }

    /// The connection for the Noesis service's own queries.
    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    /// Close the file, reporting a failure to flush instead of dropping it.
    pub fn close(self) -> Result<(), StoreError> {
        self.conn
            .close()
            .map_err(|(_, error)| StoreError::Sqlite(error))
    }
}

/// SQLite would create the file with the umask's mode, and a copied or
/// restored store may carry any mode: create it 0600 and restrict what exists.
fn make_private(path: &Path) -> Result<(), StoreError> {
    let io_error = |path: &Path, source| StoreError::Io {
        path: path.to_path_buf(),
        source,
    };
    match aether_core::private_file::create_new(path) {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(io_error(path, error)),
    }
    let mut sidecars = [path.as_os_str().to_owned(), path.as_os_str().to_owned()];
    sidecars[0].push("-wal");
    sidecars[1].push("-shm");
    restrict_existing(path).map_err(|error| io_error(path, error))?;
    for sidecar in sidecars.iter().map(Path::new) {
        restrict_existing(sidecar).map_err(|error| io_error(sidecar, error))?;
    }
    Ok(())
}

/// Set an existing regular file to 0600 through its own handle, refusing
/// symlinks and anything that is not a regular file. A missing file is fine.
#[cfg(unix)]
fn restrict_existing(path: &Path) -> io::Result<()> {
    use rustix::fs::{FileType, Mode, OFlags};
    let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK;
    let fd = match rustix::fs::open(path, flags, Mode::empty()) {
        Ok(fd) => fd,
        Err(rustix::io::Errno::NOENT) => return Ok(()),
        Err(rustix::io::Errno::LOOP) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "refusing to follow a symlink",
            ));
        }
        Err(errno) => return Err(errno.into()),
    };
    let stat = rustix::fs::fstat(&fd)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }
    rustix::fs::fchmod(&fd, Mode::from_raw_mode(0o600))?;
    Ok(())
}

/// Windows has no mode to set; the file inherits the directory's ACL.
#[cfg(not(unix))]
fn restrict_existing(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests;
