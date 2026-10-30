//! Terminal events with an Alt chord split across two reads joined again.
//!
//! Terminals send Alt+key as ESC then the key. Crossterm reads a lone ESC at
//! the end of a read as the Esc key, so a chord split by load or a slow link
//! would close views or interrupt the agent and then type the bare key.

use std::io;
use std::time::Duration;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use futures::{Stream, StreamExt};
use tokio::time::Instant;

/// How long a lone Esc waits for the key it may prefix; Neovim's default too.
pub(crate) const ESC_JOIN: Duration = Duration::from_millis(50);

#[derive(Debug)]
pub(crate) struct TerminalInput<S> {
    events: S,
    /// A lone Esc was read: until then, the next key may be its Alt chord.
    escape_until: Option<Instant>,
    /// What followed a lone Esc without joining it, due after the Esc.
    queued: Option<io::Result<Event>>,
}

impl<S: Stream<Item = io::Result<Event>> + Unpin> TerminalInput<S> {
    pub(crate) fn new(events: S) -> Self {
        Self {
            events,
            escape_until: None,
            queued: None,
        }
    }

    /// The next event; `bound` says which chords the TUI answers. Cancel-safe:
    /// an Esc already read survives a dropped call.
    pub(crate) async fn next(
        &mut self,
        bound: impl Fn(KeyEvent) -> bool,
    ) -> Option<io::Result<Event>> {
        if let Some(event) = self.queued.take() {
            debug_assert!(self.escape_until.is_none(), "queued only after an Esc");
            return Some(event);
        }
        let until = match self.escape_until {
            Some(until) => until,
            None => {
                let event = self.events.next().await?;
                if !matches!(&event, Ok(Event::Key(key)) if lone_escape(*key)) {
                    return Some(event);
                }
                *self.escape_until.insert(Instant::now() + ESC_JOIN)
            }
        };
        let next = tokio::time::timeout_at(until, self.events.next()).await;
        self.escape_until = None;
        // `timeout_at` takes a ready key even past the window, as after a stall.
        let late = Instant::now() >= until;
        let escape = Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        Some(Ok(match next {
            Ok(Some(Ok(Event::Key(key)))) if !late && joins_escape(key, &bound) => {
                Event::Key(with_alt(key))
            }
            Ok(Some(event)) => {
                self.queued = Some(event);
                escape
            }
            Ok(None) | Err(_) => escape,
        }))
    }
}

fn lone_escape(key: KeyEvent) -> bool {
    key.code == KeyCode::Esc && key.modifiers.is_empty() && key.kind == KeyEventKind::Press
}

fn with_alt(key: KeyEvent) -> KeyEvent {
    KeyEvent::new_with_kind_and_state(
        key.code,
        key.modifiers | KeyModifiers::ALT,
        key.kind,
        key.state,
    )
}

/// Keys an Alt chord can end in. A Ctrl key joins only a bound Ctrl+Alt chord,
/// so a quick Esc then Ctrl+C stays both keys rather than one nothing answers.
fn joins_escape(key: KeyEvent, bound: impl Fn(KeyEvent) -> bool) -> bool {
    if key.kind != KeyEventKind::Press
        || key.code == KeyCode::Esc
        || key.modifiers.contains(KeyModifiers::ALT)
    {
        return false;
    }
    !key.modifiers.contains(KeyModifiers::CONTROL) || bound(with_alt(key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::FutureExt;
    use futures::channel::mpsc;

    type Sender = mpsc::UnboundedSender<io::Result<Event>>;
    type Receiver = mpsc::UnboundedReceiver<io::Result<Event>>;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> Event {
        Event::Key(KeyEvent::new(code, modifiers))
    }

    fn input() -> (Sender, TerminalInput<Receiver>) {
        let (tx, rx) = mpsc::unbounded();
        (tx, TerminalInput::new(rx))
    }

    fn unbound(_: KeyEvent) -> bool {
        false
    }

    async fn next(input: &mut TerminalInput<Receiver>) -> Event {
        match input.next(unbound).await {
            Some(Ok(event)) => event,
            other => panic!("expected an event, got {other:?}"),
        }
    }

    /// #351 (e): ESC and the key in two reads are still the Alt chord.
    #[tokio::test(start_paused = true)]
    async fn an_escape_split_from_its_key_joins_it_as_alt() {
        let (tx, mut input) = input();
        for code in [
            KeyCode::Char('o'),
            KeyCode::Enter,
            KeyCode::Backspace,
            KeyCode::Up,
        ] {
            tx.unbounded_send(Ok(key(KeyCode::Esc, KeyModifiers::NONE)))
                .unwrap();
            let sender = tx.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(5)).await;
                sender
                    .unbounded_send(Ok(key(code, KeyModifiers::NONE)))
                    .unwrap();
            });
            assert_eq!(next(&mut input).await, key(code, KeyModifiers::ALT));
        }
        tx.unbounded_send(Ok(key(KeyCode::Esc, KeyModifiers::NONE)))
            .unwrap();
        tx.unbounded_send(Ok(key(KeyCode::Char('V'), KeyModifiers::SHIFT)))
            .unwrap();
        assert_eq!(
            next(&mut input).await,
            key(KeyCode::Char('V'), KeyModifiers::SHIFT | KeyModifiers::ALT)
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_lone_escape_stays_escape_and_keeps_what_follows() {
        let (tx, mut input) = input();
        let esc = key(KeyCode::Esc, KeyModifiers::NONE);
        tx.unbounded_send(Ok(esc.clone())).unwrap();
        let started = Instant::now();
        assert_eq!(next(&mut input).await, esc);
        assert_eq!(started.elapsed(), ESC_JOIN, "waits exactly the join window");
        // A second Esc, an Alt or Ctrl chord or a paste is not joined, and still arrives.
        for follower in [
            esc.clone(),
            key(KeyCode::Char('o'), KeyModifiers::ALT),
            key(KeyCode::Char('c'), KeyModifiers::CONTROL),
            Event::Paste("text".into()),
        ] {
            tx.unbounded_send(Ok(esc.clone())).unwrap();
            tx.unbounded_send(Ok(follower.clone())).unwrap();
            assert_eq!(next(&mut input).await, esc);
            assert_eq!(next(&mut input).await, follower);
        }
        // A bound Ctrl+Alt chord split after its Esc still joins.
        let ctrl_u = key(KeyCode::Char('u'), KeyModifiers::CONTROL);
        let ctrl_alt_u = key(
            KeyCode::Char('u'),
            KeyModifiers::CONTROL | KeyModifiers::ALT,
        );
        tx.unbounded_send(Ok(esc.clone())).unwrap();
        tx.unbounded_send(Ok(ctrl_u)).unwrap();
        let bound = |key: KeyEvent| Event::Key(key) == ctrl_alt_u;
        assert_eq!(input.next(bound).await.unwrap().unwrap(), ctrl_alt_u);
        let plain = key(KeyCode::Char('o'), KeyModifiers::NONE);
        tx.unbounded_send(Ok(plain.clone())).unwrap();
        assert_eq!(next(&mut input).await, plain);
    }

    #[tokio::test(start_paused = true)]
    async fn a_cancelled_read_keeps_the_escape_it_already_took() {
        let (tx, mut input) = input();
        tx.unbounded_send(Ok(key(KeyCode::Esc, KeyModifiers::NONE)))
            .unwrap();
        let cancelled = tokio::time::timeout(Duration::from_millis(10), input.next(unbound)).await;
        assert!(cancelled.is_err(), "still waiting for the join window");
        tx.unbounded_send(Ok(key(KeyCode::Char('n'), KeyModifiers::NONE)))
            .unwrap();
        assert_eq!(
            next(&mut input).await,
            key(KeyCode::Char('n'), KeyModifiers::ALT)
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_read_resumed_after_the_window_keeps_the_escape_alone() {
        let (tx, mut input) = input();
        let esc = key(KeyCode::Esc, KeyModifiers::NONE);
        tx.unbounded_send(Ok(esc.clone())).unwrap();
        let cancelled = tokio::time::timeout(Duration::from_millis(10), input.next(unbound)).await;
        assert!(cancelled.is_err(), "still waiting for the join window");
        tokio::time::sleep(ESC_JOIN * 2).await;
        let n = key(KeyCode::Char('n'), KeyModifiers::NONE);
        tx.unbounded_send(Ok(n.clone())).unwrap();
        assert_eq!(next(&mut input).await, esc, "the window closed before n");
        assert_eq!(next(&mut input).await, n);
    }

    /// A read left pending past the window, as a stalled loop leaves it.
    #[tokio::test(start_paused = true)]
    async fn a_read_polled_again_after_the_window_keeps_the_escape_alone() {
        let (tx, mut input) = input();
        let esc = key(KeyCode::Esc, KeyModifiers::NONE);
        tx.unbounded_send(Ok(esc.clone())).unwrap();
        let mut pending = Box::pin(input.next(unbound));
        assert!((&mut pending).now_or_never().is_none(), "waiting to join");
        tokio::time::sleep(ESC_JOIN * 2).await;
        let n = key(KeyCode::Char('n'), KeyModifiers::NONE);
        tx.unbounded_send(Ok(n.clone())).unwrap();
        assert_eq!(
            pending.await.unwrap().unwrap(),
            esc,
            "n came after the window"
        );
        assert_eq!(next(&mut input).await, n);
    }
}
