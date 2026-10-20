//! Graphics capability queries, completed before the terminal event reader starts.

use std::fs::OpenOptions;
use std::io::{self, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    #[default]
    HalfBlocks,
    Kitty,
    Sixel,
    Iterm2,
}

pub const PROBE_TIMEOUT: Duration = Duration::from_millis(200);
const MAX_REPLY_BYTES: usize = 4096;
const QUERY: &[u8] =
    b"\x1b_Gi=44671,a=q,t=d,f=24,s=1,v=1;AAAA\x1b\\\x1b]1337;ReportCellSize\x1b\\\x1b[16t\x1b[c";

/// Call in raw mode, before starting any other terminal reader.
/// Input arriving during the query is consumed along with the replies.
pub fn detect() -> Result<Protocol> {
    Ok(detect_with_metrics()?.protocol)
}

#[derive(Debug)]
pub struct Capabilities {
    pub protocol: Protocol,
    pub cell_pixels: Option<(u16, u16)>,
}

pub fn detect_with_metrics() -> Result<Capabilities> {
    ensure!(
        crossterm::terminal::is_raw_mode_enabled()?,
        "graphics detection requires raw terminal mode"
    );
    let until = Instant::now() + PROBE_TIMEOUT;
    let mut tty = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(nix::libc::O_NONBLOCK)
        .open("/dev/tty")
        .context("open terminal for graphics detection")?;
    tty.write_all(QUERY).context("write graphics queries")?;
    let reply = read_reply(&mut tty, until).context("read graphics replies")?;
    debug_assert!(reply.len() <= MAX_REPLY_BYTES);
    Ok(Capabilities {
        protocol: detect_from_reply(&reply),
        cell_pixels: cell_size_from_reply(&reply),
    })
}

fn read_reply(reader: &mut impl Read, until: Instant) -> io::Result<Vec<u8>> {
    let mut reply = Vec::new();
    let mut chunk = [0; 256];
    while Instant::now() < until && reply.len() < MAX_REPLY_BYTES {
        let available = chunk.len().min(MAX_REPLY_BYTES - reply.len());
        match reader.read(&mut chunk[..available]) {
            Ok(0) => break,
            Ok(count) => {
                debug_assert!(count <= available);
                reply.extend_from_slice(&chunk[..count]);
                if has_da_reply(&reply) {
                    break;
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                let remaining = until.saturating_duration_since(Instant::now());
                std::thread::sleep(remaining.min(Duration::from_millis(2)));
            }
            Err(error) => return Err(error),
        }
    }
    debug_assert!(reply.len() <= MAX_REPLY_BYTES);
    Ok(reply)
}

/// Recognize only complete replies to our queries; terminal names are unused.
pub fn detect_from_reply(reply: &[u8]) -> Protocol {
    if reply.len() > MAX_REPLY_BYTES {
        return Protocol::HalfBlocks;
    }
    if framed_payloads(reply, b"\x1b_G").any(|payload| {
        let Some((header, answer)) = payload.split_once(';') else {
            return false;
        };
        header.split(',').any(|field| field == "i=44671") && answer == "OK"
    }) {
        return Protocol::Kitty;
    }
    if framed_payloads(reply, b"\x1b]1337;ReportCellSize=").any(valid_cell_size) {
        return Protocol::Iterm2;
    }
    if device_attributes(reply).any(|attributes| attributes.split(';').skip(1).any(|p| p == "4")) {
        return Protocol::Sixel;
    }
    Protocol::HalfBlocks
}

pub fn cell_size_from_reply(reply: &[u8]) -> Option<(u16, u16)> {
    if reply.len() > MAX_REPLY_BYTES {
        return None;
    }
    let csi = reply.windows(4).enumerate().find_map(|(at, part)| {
        if part != b"\x1b[6;" {
            return None;
        }
        let tail = &reply[at + 4..];
        let end = tail.iter().position(|b| *b == b't')?;
        let (height, width) = std::str::from_utf8(&tail[..end]).ok()?.split_once(';')?;
        valid_pixels(width.parse().ok()?, height.parse().ok()?)
    });
    csi.or_else(|| {
        framed_payloads(reply, b"\x1b]1337;ReportCellSize=").find_map(|payload| {
            if !valid_cell_size(payload) {
                return None;
            }
            let values: Vec<f32> = payload.split(';').filter_map(|v| v.parse().ok()).collect();
            let scale = values.get(2).copied().unwrap_or(1.0);
            valid_pixels(values[1] * scale, values[0] * scale)
        })
    })
}

fn valid_pixels(width: f32, height: f32) -> Option<(u16, u16)> {
    if !width.is_finite()
        || !height.is_finite()
        || !(1.0..=1024.0).contains(&width)
        || !(1.0..=1024.0).contains(&height)
    {
        return None;
    }
    debug_assert!(width > 0.0 && height > 0.0);
    debug_assert!(width <= 1024.0 && height <= 1024.0);
    Some((width.round() as u16, height.round() as u16))
}

fn framed_payloads<'a>(reply: &'a [u8], prefix: &'a [u8]) -> impl Iterator<Item = &'a str> {
    debug_assert!(!prefix.is_empty());
    debug_assert!(prefix.starts_with(b"\x1b"));
    reply
        .windows(prefix.len())
        .enumerate()
        .filter_map(move |(at, bytes)| {
            if bytes != prefix {
                return None;
            }
            let rest = &reply[at + prefix.len()..];
            let end = rest.iter().enumerate().find_map(|(index, byte)| {
                ((*byte == 7 && prefix.starts_with(b"\x1b]"))
                    || (*byte == 27 && rest.get(index + 1) == Some(&b'\\')))
                .then_some(index)
            })?;
            std::str::from_utf8(&rest[..end]).ok()
        })
}

fn valid_cell_size(payload: &str) -> bool {
    let values: Vec<_> = payload.split(';').collect();
    (2..=3).contains(&values.len())
        && values.iter().all(|value| {
            value
                .parse::<f32>()
                .is_ok_and(|number| number.is_finite() && number > 0.0 && number <= 1024.0)
        })
}

fn device_attributes(reply: &[u8]) -> impl Iterator<Item = &str> {
    reply.windows(3).enumerate().filter_map(|(at, bytes)| {
        if bytes != b"\x1b[?" {
            return None;
        }
        let rest = &reply[at + 3..];
        let end = rest
            .iter()
            .position(|byte| !byte.is_ascii_digit() && *byte != b';')?;
        if end == 0 || rest[end] != b'c' {
            return None;
        }
        let attributes = std::str::from_utf8(&rest[..end]).ok()?;
        attributes
            .split(';')
            .all(|value| !value.is_empty())
            .then_some(attributes)
    })
}

fn has_da_reply(reply: &[u8]) -> bool {
    device_attributes(reply).next().is_some()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn measured_cells_are_bounded_and_iterm_scale_is_applied() {
        assert_eq!(cell_size_from_reply(b"\x1b[6;18;9t"), Some((9, 18)));
        assert_eq!(
            cell_size_from_reply(b"\x1b]1337;ReportCellSize=17.5;8;2\x07"),
            Some((16, 35))
        );
        for reply in [
            b"\x1b[6;0;8t".as_slice(),
            b"\x1b[6;18;9",
            b"\x1b[6;65535;8t",
            b"\x1b]1337;ReportCellSize=NaN;8\x07",
        ] {
            assert_eq!(cell_size_from_reply(reply), None);
        }
    }

    #[test]
    fn complete_capability_replies_select_protocol() {
        assert_eq!(
            detect_from_reply(b"\x1b_Gi=44671;OK\x1b\\"),
            Protocol::Kitty
        );
        assert_eq!(detect_from_reply(b"\x1b[?62;4;22c"), Protocol::Sixel);
        assert_eq!(
            detect_from_reply(b"\x1b]1337;ReportCellSize=17.5;8;2\x07"),
            Protocol::Iterm2
        );
        assert_eq!(
            detect_from_reply(b"\x1b]1337;ReportCellSize=17;8\x1b\\"),
            Protocol::Iterm2
        );
        assert_eq!(
            detect_from_reply(b"\x1b[?62;4c\x1b_Gi=44671;OK\x1b\\"),
            Protocol::Kitty
        );
    }

    #[test]
    fn names_malformed_partial_and_unrelated_replies_are_not_capabilities() {
        for reply in [
            b"xterm-kitty iTerm2 sixel".as_slice(),
            b"\x1b_Gi=44671;OK",
            b"\x1b_Gi=44671;OK\x07",
            b"\x1b_Gi=2;OK\x1b\\",
            b"\x1b_Gi=44671;ENOENT\x1b\\",
            b"\x1b[?4c",
            b"\x1b[?62;44c",
            b"\x1b[?62;4",
            b"\x1b[?62;;4c",
            b"\x1b]1337;ReportCellSize=NaN;8\x07",
            b"\x1b]1337;ReportCellSize=-1;8\x07",
        ] {
            assert_eq!(detect_from_reply(reply), Protocol::HalfBlocks, "{reply:?}");
        }
        assert_eq!(
            detect_from_reply(&vec![0; MAX_REPLY_BYTES + 1]),
            Protocol::HalfBlocks
        );
    }

    #[test]
    fn reply_reader_accepts_fragments_and_stops_at_da() {
        let mut reader = io::Cursor::new(b"\x1b[?62;4c");
        let reply = read_reply(&mut reader, Instant::now() + PROBE_TIMEOUT).unwrap();
        assert_eq!(reply, b"\x1b[?62;4c");
        assert_eq!(detect_from_reply(&reply), Protocol::Sixel);
    }

    #[test]
    fn nonblocking_reader_obeys_deadline_and_byte_limit() {
        let (mut reader, _writer) = std::os::unix::net::UnixStream::pair().unwrap();
        reader.set_nonblocking(true).unwrap();
        let start = Instant::now();
        assert!(
            read_reply(&mut reader, start + Duration::from_millis(10))
                .unwrap()
                .is_empty()
        );
        assert!(start.elapsed() < PROBE_TIMEOUT);
        let mut reader = io::repeat(b'x');
        assert_eq!(
            read_reply(&mut reader, Instant::now() + PROBE_TIMEOUT)
                .unwrap()
                .len(),
            MAX_REPLY_BYTES
        );
    }

    #[test]
    fn read_failures_remain_visible() {
        struct Broken;
        impl Read for Broken {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::other("read failed"))
            }
        }
        let result = read_reply(&mut Broken, Instant::now() + PROBE_TIMEOUT);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().to_string(), "read failed");
    }
}
