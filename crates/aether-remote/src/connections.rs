//! The carrier connections open right now: where each came from, who it
//! authenticated as, and the token that ends it.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use aether_protocol::remote::DeviceId;
use tokio_util::sync::CancellationToken;

#[derive(Debug)]
struct Open {
    /// The allowlisted `Origin` the connection arrived with.
    origin: String,
    stop: CancellationToken,
    /// Set once its hello is accepted: who it is, and the token that
    /// fires when that session is revoked.
    session: Option<(DeviceId, CancellationToken)>,
}

impl Open {
    /// The device this connection is serving, unless its session was ended.
    fn serving(&self) -> Option<DeviceId> {
        let (device, closed) = self.session.as_ref()?;
        (!closed.is_cancelled()).then_some(*device)
    }
}

/// Every WebSocket the listener is carrying, greeted or not.
#[derive(Debug, Default)]
pub struct Connections {
    next: AtomicU64,
    open: parking_lot::Mutex<HashMap<u64, Open>>,
}

impl Connections {
    /// Track a connection until the returned guard drops.
    pub(crate) fn register(self: &Arc<Self>, origin: &str, stop: CancellationToken) -> Registered {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let open = Open {
            origin: origin.to_owned(),
            stop,
            session: None,
        };
        let replaced = self.open.lock().insert(id, open);
        debug_assert!(replaced.is_none(), "connection ids are never reused");
        Registered {
            id,
            connections: Arc::clone(self),
        }
    }

    /// Connections whose hello was accepted and whose session still stands.
    pub fn authenticated(&self) -> u32 {
        let open = self.open.lock();
        let count = open.values().filter(|c| c.serving().is_some()).count();
        u32::try_from(count).unwrap_or(u32::MAX)
    }

    /// Connections serving `device`. One whose session was just ended does
    /// not count, though its socket may still be saying goodbye.
    pub fn of(&self, device: DeviceId) -> u32 {
        let open = self.open.lock();
        let serving = open.values().filter(|c| c.serving() == Some(device));
        let count = serving.count();
        u32::try_from(count).unwrap_or(u32::MAX)
    }

    /// End every connection whose origin `allowed` no longer accepts.
    /// Returns how many were told to stop.
    pub(crate) fn close_unless(&self, allowed: impl Fn(&str) -> bool) -> usize {
        let open = self.open.lock();
        let mut stopped = 0;
        for connection in open.values().filter(|c| !allowed(&c.origin)) {
            connection.stop.cancel();
            stopped += 1;
        }
        stopped
    }
}

/// One tracked connection; dropping it forgets the connection.
#[derive(Debug)]
pub(crate) struct Registered {
    id: u64,
    connections: Arc<Connections>,
}

impl Registered {
    /// Record who the connection authenticated as; `closed` ends the session.
    pub(crate) fn identify(&self, device: DeviceId, closed: CancellationToken) {
        let mut open = self.connections.open.lock();
        let Some(connection) = open.get_mut(&self.id) else {
            debug_assert!(false, "a registered connection is tracked until it drops");
            return;
        };
        debug_assert!(connection.session.is_none(), "one hello per connection");
        connection.session = Some((device, closed));
    }
}

impl Drop for Registered {
    fn drop(&mut self) {
        self.connections.open.lock().remove(&self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_connection_counts_for_its_device_only_once_greeted_and_until_dropped() {
        let connections = Arc::new(Connections::default());
        let device = DeviceId::new();
        let first = connections.register("https://a.example", CancellationToken::new());
        let second = connections.register("https://a.example", CancellationToken::new());
        assert_eq!(connections.authenticated(), 0, "nobody greeted yet");
        let revoked = CancellationToken::new();
        first.identify(device, CancellationToken::new());
        second.identify(device, revoked.clone());
        assert_eq!(connections.authenticated(), 2);
        assert_eq!(connections.of(device), 2);
        assert_eq!(connections.of(DeviceId::new()), 0);
        revoked.cancel();
        assert_eq!(connections.of(device), 1, "an ended session is not served");
        drop(second);
        assert_eq!(connections.of(device), 1);
        drop(first);
        assert_eq!(connections.authenticated(), 0);
    }

    #[test]
    fn removing_an_origin_stops_only_the_connections_that_came_by_it() {
        let connections = Arc::new(Connections::default());
        let (kept, gone) = (CancellationToken::new(), CancellationToken::new());
        let _kept = connections.register("https://kept.example", kept.clone());
        let _gone = connections.register("https://gone.example", gone.clone());
        let stopped = connections.close_unless(|origin| origin == "https://kept.example");
        assert_eq!(stopped, 1);
        assert!(gone.is_cancelled());
        assert!(!kept.is_cancelled());
    }
}
