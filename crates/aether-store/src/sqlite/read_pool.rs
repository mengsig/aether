//! Read-only connections to a file store, one per concurrent read.
//!
//! A single shared reader made every read wait for the one before it: a
//! long-lived agent's transcript took seconds to read, and `status` and
//! `get_tree`, a dozen short reads each, queued behind every such read (#333).

use super::KEEP_TEMP_IN_MEMORY;
use crate::Result;
use parking_lot::{Condvar, Mutex};
use rusqlite::{Connection, OpenFlags};
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Connections kept open between reads. More are opened while reads overlap.
const MAX_IDLE: usize = 8;

/// Connections open at once. Past it a read waits for one to come back: each
/// holds three file descriptors and its own page cache.
pub(super) const MAX_OPEN: usize = 32;

/// A wait for a connection this long is logged: it names what a slow request waited on.
pub(super) const SLOW_WAIT: Duration = Duration::from_secs(2);

struct Connections {
    idle: Vec<Connection>,
    /// Idle and leased alike.
    open: usize,
}

pub(super) struct ReadPool {
    path: PathBuf,
    connections: Mutex<Connections>,
    returned: Condvar,
}

impl ReadPool {
    /// A pool over the WAL database at `path`, with one connection opened now
    /// so a store that cannot be read fails to open.
    pub(super) fn open(path: &Path) -> Result<Self> {
        let first = open_reader(path)?;
        Ok(Self {
            path: path.to_path_buf(),
            connections: Mutex::new(Connections {
                idle: vec![first],
                open: 1,
            }),
            returned: Condvar::new(),
        })
    }

    /// A connection of the caller's own until the lease drops.
    pub(super) fn lease(&self) -> Result<Lease<'_>> {
        let started = Instant::now();
        let mut connections = self.connections.lock();
        loop {
            assert!(
                connections.idle.len() <= connections.open && connections.open <= MAX_OPEN,
                "the pool counts every connection it holds"
            );
            if let Some(conn) = connections.idle.pop() {
                drop(connections);
                return Ok(self.leased(conn, started));
            }
            if connections.open < MAX_OPEN {
                connections.open += 1;
                drop(connections);
                return match open_reader(&self.path) {
                    Ok(conn) => Ok(self.leased(conn, started)),
                    Err(error) => {
                        self.forget_one();
                        Err(error)
                    }
                };
            }
            self.returned.wait(&mut connections);
        }
    }

    fn leased(&self, conn: Connection, started: Instant) -> Lease<'_> {
        let waited = started.elapsed();
        if waited >= SLOW_WAIT {
            tracing::warn!(
                waited_ms = waited.as_millis() as u64,
                open = MAX_OPEN,
                "a store read waited for a free read connection: every one was busy"
            );
        }
        Lease {
            pool: self,
            conn: Some(conn),
        }
    }

    fn forget_one(&self) {
        let mut connections = self.connections.lock();
        assert!(connections.open > 0, "a forgotten connection was counted");
        connections.open -= 1;
        drop(connections);
        self.returned.notify_one();
    }

    /// Keep `conn` for the next read, unless enough are idle or it is still
    /// inside a transaction a read left open.
    fn give_back(&self, conn: Connection) {
        let reusable = conn.is_autocommit();
        debug_assert!(reusable, "a read left its transaction open");
        let mut connections = self.connections.lock();
        if reusable && connections.idle.len() < MAX_IDLE {
            connections.idle.push(conn);
            drop(connections);
            self.returned.notify_one();
            return;
        }
        drop(connections);
        drop(conn);
        self.forget_one();
    }
}

fn open_reader(path: &Path) -> Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.execute_batch(KEEP_TEMP_IN_MEMORY)?;
    Ok(conn)
}

/// A pooled connection, returned to the pool on drop.
pub(crate) struct Lease<'a> {
    pool: &'a ReadPool,
    conn: Option<Connection>,
}

impl Deref for Lease<'_> {
    type Target = Connection;

    fn deref(&self) -> &Connection {
        self.conn
            .as_ref()
            .unwrap_or_else(|| unreachable!("leased until dropped"))
    }
}

impl DerefMut for Lease<'_> {
    fn deref_mut(&mut self) -> &mut Connection {
        self.conn
            .as_mut()
            .unwrap_or_else(|| unreachable!("leased until dropped"))
    }
}

impl Drop for Lease<'_> {
    fn drop(&mut self) {
        if let Some(conn) = self.conn.take() {
            self.pool.give_back(conn);
        }
    }
}
