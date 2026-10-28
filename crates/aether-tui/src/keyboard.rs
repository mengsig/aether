//! Balance enhanced key reporting across suspension, panic, and normal exit.

use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};

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

/// Crossterm exposes OSC replies as Alt+] and DCS replies as Alt+Shift+P,
/// each followed by printable keys. Never let reply payloads reach the
/// composer or the approval hotkeys.
#[derive(Debug, Default)]
pub(crate) struct TerminalReplyFilter {
    in_reply: bool,
}

impl TerminalReplyFilter {
    pub fn consume(&mut self, key: crossterm::event::KeyEvent) -> bool {
        use crossterm::event::{KeyCode, KeyModifiers};
        if starts_reply(key) {
            self.in_reply = true;
            return true;
        }
        if !self.in_reply {
            return false;
        }
        if (key.code == KeyCode::Char('\\') && key.modifiers == KeyModifiers::ALT)
            || key.code == KeyCode::Char('\x07')
            || (key.code == KeyCode::Char('g') && key.modifiers == KeyModifiers::CONTROL)
            || key.code == KeyCode::Esc
        {
            self.in_reply = false;
        }
        true
    }
}

/// `ESC ]` opens an OSC reply and `ESC P` a DCS reply (DECRQSS, XTGETTCAP).
fn starts_reply(key: crossterm::event::KeyEvent) -> bool {
    use crossterm::event::{KeyCode, KeyModifiers};
    match key.code {
        KeyCode::Char(']') => key.modifiers == KeyModifiers::ALT,
        KeyCode::Char('P') => {
            key.modifiers == KeyModifiers::ALT | KeyModifiers::SHIFT
                || key.modifiers == KeyModifiers::ALT
        }
        _ => false,
    }
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

    fn reply_keys(bytes: &[u8]) -> Vec<crossterm::event::KeyEvent> {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut keys = Vec::new();
        let mut at = 0;
        while at < bytes.len() {
            // Mirrors crossterm: ESC + char is Alt+char, uppercase adds Shift.
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
            keys.push(KeyEvent::new(KeyCode::Char(c), modifiers));
            at += 1;
        }
        keys
    }

    #[test]
    fn late_dcs_capability_replies_are_swallowed_through_the_terminator() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        for reply in [
            &b"\x1bP1$r0;48;2;1;2;3m\x1b\\"[..],
            &b"\x1bP1+r524742=31\x1b\\"[..],
            &b"\x1bP0+r524742\x1b\\"[..],
            &b"\x1b]11;rgb:1c1c/1c1c/1c1c\x1b\\\x1bP1$r48:2::1:2:3m\x1b\\"[..],
        ] {
            let mut filter = TerminalReplyFilter::default();
            let keys = reply_keys(reply);
            assert!(keys.len() > 4, "{reply:?}");
            for key in keys {
                assert!(filter.consume(key), "{reply:?} leaked {key:?}");
            }
            assert!(!filter.in_reply, "{reply:?} left the filter open");
            let typed = KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE);
            assert!(!filter.consume(typed), "{reply:?} swallowed a typed key");
        }
    }

    #[test]
    fn typed_keys_outside_a_reply_pass_through() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut filter = TerminalReplyFilter::default();
        for key in [
            KeyEvent::new(KeyCode::Char('P'), KeyModifiers::SHIFT),
            KeyEvent::new(KeyCode::Char('p'), KeyModifiers::ALT),
            KeyEvent::new(KeyCode::Char('\\'), KeyModifiers::ALT),
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        ] {
            assert!(!filter.consume(key), "{key:?} was swallowed");
        }
        assert!(!filter.in_reply);
    }
}
