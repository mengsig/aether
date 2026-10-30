//! Balance enhanced key reporting across suspension, panic, and normal exit,
//! and keep late terminal replies out of the keys.

use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};

use aether_core::time::Timestamp;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Default)]
pub(crate) struct KeyboardMode {
    active: AtomicBool,
}

impl KeyboardMode {
    pub fn enable(&self, out: &mut impl Write) -> io::Result<()> {
        // Windows reports modifiers through its native input API; crossterm's
        // enhancement command is unsupported there.
        #[cfg(unix)]
        if !self.active.load(Ordering::Relaxed) {
            use crossterm::event::{KeyboardEnhancementFlags, PushKeyboardEnhancementFlags};
            crossterm::execute!(
                out,
                PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
            )?;
            self.active.store(true, Ordering::Relaxed);
        }
        #[cfg(not(unix))]
        let _ = out;
        Ok(())
    }

    pub fn disable(&self, out: &mut impl Write) -> io::Result<()> {
        if self.active.swap(false, Ordering::Relaxed) {
            crossterm::execute!(out, crossterm::event::PopKeyboardEnhancementFlags)?;
        }
        Ok(())
    }
}

/// How long a held key waits for the rest of a reply before it is typed.
const REPLY_GAP_MS: i64 = 250;
/// Longer than any reply the startup probes ask for.
const MAX_REPLY_KEYS: usize = 512;

/// Crossterm exposes OSC replies as Alt+], DCS replies as Alt+Shift+P and APC
/// replies as Alt+_, each followed by printable keys. Keys that may open a reply
/// are held while what follows keeps a reply's shape, and dropped only once its
/// terminator arrives; anything else types every held key, so none is lost.
#[derive(Debug, Default)]
pub(crate) struct TerminalReplyFilter {
    held: Vec<KeyEvent>,
    expect: Option<Expect>,
    last_held_at: Option<Timestamp>,
}

/// The next part of a reply's shape: `ESC ] digits ; payload ST`,
/// `ESC P [01] [$+] r payload ST` or `ESC _ G payload ST`, ST being `ESC \` or BEL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    OscCode {
        digits: bool,
    },
    DcsStatus,
    DcsKind,
    DcsFinal,
    ApcGraphics,
    Payload,
    /// A lone Esc inside the payload: ST split across two reads.
    Backslash,
}

enum Step {
    Next(Expect),
    Terminated,
    NotAReply,
}

impl TerminalReplyFilter {
    /// The keys to dispatch now, in order: none while `key` may still be part
    /// of a reply, else every held key, then `key` unless it opens a reply.
    pub fn feed(&mut self, key: KeyEvent, now: Timestamp) -> Vec<KeyEvent> {
        let mut typed = self.expire(now);
        let step = match self.expect {
            Some(expect) if self.held.len() < MAX_REPLY_KEYS => step(expect, key),
            Some(_) => Step::NotAReply,
            None => opens(key).map_or(Step::NotAReply, Step::Next),
        };
        match step {
            Step::Next(expect) => self.hold(key, expect, now),
            Step::Terminated => {
                self.held.clear();
                self.expect = None;
                self.last_held_at = None;
            }
            Step::NotAReply => {
                typed.append(&mut self.release());
                match opens(key) {
                    Some(expect) => self.hold(key, expect, now),
                    None => typed.push(key),
                }
            }
        }
        debug_assert_eq!(self.held.is_empty(), self.expect.is_none());
        typed
    }

    /// The held keys once no more of a reply came for [`REPLY_GAP_MS`].
    pub fn expire(&mut self, now: Timestamp) -> Vec<KeyEvent> {
        match self.deadline() {
            Some(deadline) if now >= deadline => self.release(),
            _ => Vec::new(),
        }
    }

    /// When the held keys are typed if nothing else arrives.
    pub fn deadline(&self) -> Option<Timestamp> {
        debug_assert_eq!(self.last_held_at.is_some(), self.expect.is_some());
        self.last_held_at
            .map(|at| at + chrono::Duration::milliseconds(REPLY_GAP_MS))
    }

    fn hold(&mut self, key: KeyEvent, expect: Expect, now: Timestamp) {
        debug_assert!(self.held.len() < MAX_REPLY_KEYS);
        self.held.push(key);
        self.expect = Some(expect);
        self.last_held_at = Some(now);
    }

    fn release(&mut self) -> Vec<KeyEvent> {
        self.expect = None;
        self.last_held_at = None;
        std::mem::take(&mut self.held)
    }
}

/// `ESC ]` opens an OSC reply, `ESC P` a DCS reply (DECRQSS, XTGETTCAP) and
/// `ESC _` an APC reply (kitty graphics).
fn opens(key: KeyEvent) -> Option<Expect> {
    let alt = key.modifiers == KeyModifiers::ALT;
    match key.code {
        KeyCode::Char(']') if alt => Some(Expect::OscCode { digits: false }),
        KeyCode::Char('_') if alt => Some(Expect::ApcGraphics),
        KeyCode::Char('P') if alt || key.modifiers == KeyModifiers::ALT | KeyModifiers::SHIFT => {
            Some(Expect::DcsStatus)
        }
        _ => None,
    }
}

fn step(expect: Expect, key: KeyEvent) -> Step {
    let terminator = (key.code == KeyCode::Char('\\') && key.modifiers == KeyModifiers::ALT)
        || (key.code == KeyCode::Char('g') && key.modifiers == KeyModifiers::CONTROL);
    if expect == Expect::Payload && terminator {
        return Step::Terminated;
    }
    if expect == Expect::Payload && key.code == KeyCode::Esc && key.modifiers.is_empty() {
        return Step::Next(Expect::Backslash);
    }
    let Some(c) = typed_char(key) else {
        return Step::NotAReply;
    };
    match (expect, c) {
        (Expect::OscCode { .. }, '0'..='9') => Step::Next(Expect::OscCode { digits: true }),
        (Expect::OscCode { digits: true }, ';')
        | (Expect::DcsFinal, 'r')
        | (Expect::ApcGraphics, 'G')
        | (Expect::Payload, ' '..='~') => Step::Next(Expect::Payload),
        (Expect::DcsStatus, '0' | '1') => Step::Next(Expect::DcsKind),
        (Expect::DcsKind, '$' | '+') => Step::Next(Expect::DcsFinal),
        (Expect::Backslash, '\\') => Step::Terminated,
        _ => Step::NotAReply,
    }
}

/// The character a reply byte decodes to: crossterm adds Shift to capitals.
fn typed_char(key: KeyEvent) -> Option<char> {
    let KeyCode::Char(c) = key.code else {
        return None;
    };
    let shift_only = key.modifiers == KeyModifiers::SHIFT && c.is_ascii_uppercase();
    (key.modifiers.is_empty() || shift_only).then_some(c)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn reporting_is_enabled_and_restored_once_per_terminal_session() -> io::Result<()> {
        let mode = KeyboardMode::default();
        let mut bytes = Vec::new();
        mode.enable(&mut bytes)?;
        mode.enable(&mut bytes)?;
        assert_eq!(bytes, b"\x1b[>1u");
        mode.disable(&mut bytes)?;
        mode.disable(&mut bytes)?;
        assert_eq!(bytes, b"\x1b[>1u\x1b[<1u");
        mode.enable(&mut bytes)?;
        mode.disable(&mut bytes)?;
        assert_eq!(bytes, b"\x1b[>1u\x1b[<1u\x1b[>1u\x1b[<1u");
        Ok(())
    }

    fn reply_keys(bytes: &[u8]) -> Vec<KeyEvent> {
        let mut keys = Vec::new();
        let mut at = 0;
        while at < bytes.len() {
            // Mirrors crossterm: ESC + char is Alt+char, uppercase adds Shift, BEL is Ctrl+G.
            let (alt, c) = if bytes[at] == 0x1b {
                at += 1;
                (true, bytes[at] as char)
            } else {
                (false, bytes[at] as char)
            };
            let mut modifiers = KeyModifiers::NONE;
            if alt {
                modifiers |= KeyModifiers::ALT;
            }
            if c.is_ascii_uppercase() {
                modifiers |= KeyModifiers::SHIFT;
            }
            keys.push(if c == '\x07' {
                KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL)
            } else {
                KeyEvent::new(KeyCode::Char(c), modifiers)
            });
            at += 1;
        }
        keys
    }

    fn at(ms: i64) -> Timestamp {
        Timestamp::UNIX_EPOCH + chrono::Duration::milliseconds(ms)
    }

    fn feed_all(filter: &mut TerminalReplyFilter, keys: &[KeyEvent]) -> Vec<KeyEvent> {
        keys.iter()
            .flat_map(|key| filter.feed(*key, at(0)))
            .collect()
    }

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn late_replies_are_swallowed_through_the_terminator() {
        for reply in [
            &b"\x1bP1$r0;48;2;1;2;3m\x1b\\"[..],
            &b"\x1bP1+r524742=31\x1b\\"[..],
            &b"\x1bP0+r524742\x1b\\"[..],
            &b"\x1bP0$r\x1b\\"[..],
            &b"\x1b]11;rgb:1c1c/1c1c/1c1c\x1b\\\x1bP1$r48:2::1:2:3m\x1b\\"[..],
            &b"\x1b]10;rgb:ffff/ffff/ffff\x07"[..],
            &b"\x1b]1337;ReportCellSize=17.0;8.0\x1b\\"[..],
            &b"\x1b_Gi=44671;OK\x1b\\"[..],
        ] {
            let mut filter = TerminalReplyFilter::default();
            let keys = reply_keys(reply);
            assert!(keys.len() > 4, "{reply:?}");
            assert_eq!(feed_all(&mut filter, &keys), [], "{reply:?} leaked");
            assert_eq!(filter.deadline(), None, "{reply:?} left the filter open");
            let typed = key(KeyCode::Char('y'), KeyModifiers::NONE);
            assert_eq!(
                filter.feed(typed, at(0)),
                [typed],
                "{reply:?} swallowed a typed key"
            );
        }
    }

    #[test]
    fn a_terminator_split_across_two_reads_still_ends_the_reply() {
        let mut filter = TerminalReplyFilter::default();
        let mut keys = reply_keys(b"\x1b]11;rgb:0/0/0");
        keys.push(key(KeyCode::Esc, KeyModifiers::NONE));
        keys.push(key(KeyCode::Char('\\'), KeyModifiers::NONE));
        assert_eq!(feed_all(&mut filter, &keys), []);
        assert_eq!(filter.deadline(), None);
    }

    /// #351: Alt+] or Alt+Shift+P used to swallow every key up to the next Esc.
    #[test]
    fn keys_after_a_reply_opener_are_typed_in_order() {
        let alt = KeyModifiers::ALT;
        let typed = [
            key(KeyCode::Char('h'), KeyModifiers::NONE),
            key(KeyCode::Char('o'), alt),
            key(KeyCode::Esc, KeyModifiers::NONE),
        ];
        for opener in [
            key(KeyCode::Char(']'), alt),
            key(KeyCode::Char('P'), alt | KeyModifiers::SHIFT),
            key(KeyCode::Char('P'), alt),
            key(KeyCode::Char('_'), alt),
        ] {
            let mut filter = TerminalReplyFilter::default();
            let mut keys = vec![opener];
            keys.extend(typed);
            assert_eq!(feed_all(&mut filter, &keys), keys, "after {opener:?}");
            assert_eq!(filter.deadline(), None);
        }
        // A reply's shape broken late still types every key, the opener first.
        let mut filter = TerminalReplyFilter::default();
        let mut keys = reply_keys(b"\x1b]11;rgb");
        keys.push(key(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(feed_all(&mut filter, &keys), keys);
        // Two openers in a row: the first is typed, the second waits.
        let mut filter = TerminalReplyFilter::default();
        let opener = key(KeyCode::Char(']'), alt);
        assert_eq!(feed_all(&mut filter, &[opener, opener]), [opener]);
        assert!(filter.deadline().is_some());
    }

    #[test]
    fn a_lone_opener_is_typed_once_no_reply_follows() {
        let opener = key(KeyCode::Char('P'), KeyModifiers::ALT | KeyModifiers::SHIFT);
        let mut filter = TerminalReplyFilter::default();
        assert_eq!(filter.feed(opener, at(1_000)), []);
        assert_eq!(filter.deadline(), Some(at(1_000 + REPLY_GAP_MS)));
        assert_eq!(filter.expire(at(1_000 + REPLY_GAP_MS - 1)), []);
        assert_eq!(filter.expire(at(1_000 + REPLY_GAP_MS)), [opener]);
        assert_eq!(filter.deadline(), None);
        // A key arriving after the gap types the stale opener before itself.
        let typed = key(KeyCode::Char('1'), KeyModifiers::NONE);
        assert_eq!(filter.feed(opener, at(0)), []);
        assert_eq!(filter.feed(typed, at(REPLY_GAP_MS)), [opener, typed]);
    }

    #[test]
    fn typed_keys_outside_a_reply_pass_through() {
        let mut filter = TerminalReplyFilter::default();
        for key in [
            key(KeyCode::Char('P'), KeyModifiers::SHIFT),
            key(KeyCode::Char('p'), KeyModifiers::ALT),
            key(KeyCode::Char('p'), KeyModifiers::ALT | KeyModifiers::SHIFT),
            key(KeyCode::Char('\\'), KeyModifiers::ALT),
            key(KeyCode::Char('g'), KeyModifiers::CONTROL),
            key(KeyCode::Esc, KeyModifiers::NONE),
        ] {
            assert_eq!(filter.feed(key, at(0)), [key], "{key:?} was held");
        }
        assert_eq!(filter.deadline(), None);
    }
}
