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
        create_private_if_missing(path)?;
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

/// SQLite would create the file with the umask's mode; create it 0600 first.
/// Its `-wal` and `-shm` files take the same mode from it.
fn create_private_if_missing(path: &Path) -> Result<(), StoreError> {
    let io_error = |source| StoreError::Io {
        path: path.to_path_buf(),
        source,
    };
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => return Ok(()),
        Ok(_) => {
            let source = io::Error::new(io::ErrorKind::InvalidInput, "not a regular file");
            return Err(io_error(source));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(io_error(error)),
    }
    match aether_core::private_file::create_new(path) {
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(io_error(error)),
    }
}

#[cfg(test)]
mod tests;
