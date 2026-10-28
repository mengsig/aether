//! Whether a stream chunk ends partway into a known secret that spans lines,
//! found in time linear in the chunk whatever the secret's shape, or into a
//! built-in rule whose whitespace may cross a line end.

/// A known secret that spans lines, with the tables of a Knuth-Morris-Pratt
/// matcher over its bytes.
pub(super) struct MultilineSecret {
    text: Box<[u8]>,
    /// `border[j]`: the longest proper prefix of `text[..j]` that also ends it.
    border: Box<[usize]>,
    /// `open[j]`: the shortest prefix in `j`'s border chain that ends a line
    /// and is not the whole secret, or 0. A chunk ending with it is cut there.
    open: Box<[usize]>,
}

impl MultilineSecret {
    pub(super) fn new(secret: &str) -> Self {
        debug_assert!(secret.contains('\n'), "only a multiline secret needs this");
        let text: Box<[u8]> = secret.as_bytes().into();
        let len = text.len();
        let mut border = vec![0; len + 1];
        for j in 2..=len {
            let mut k = border[j - 1];
            while k > 0 && text[k] != text[j - 1] {
                k = border[k];
            }
            border[j] = if text[k] == text[j - 1] { k + 1 } else { 0 };
        }
        let mut open = vec![0; len + 1];
        for j in 1..=len {
            let shorter = open[border[j]];
            let ends_line = j < len && text[j - 1] == b'\n';
            open[j] = if shorter != 0 || !ends_line {
                shorter
            } else {
                j
            };
        }
        debug_assert!(border.iter().enumerate().all(|(j, b)| *b < j.max(1)));
        Self {
            text,
            border: border.into(),
            open: open.into(),
        }
    }

    /// The matcher state after `bytes`, from `state`: the longest prefix of
    /// the secret that ends what has been read.
    fn advance(&self, mut state: usize, bytes: &[u8]) -> usize {
        for &byte in bytes {
            if state == self.text.len() {
                state = self.border[state];
            }
            while state > 0 && self.text[state] != byte {
                state = self.border[state];
            }
            if self.text[state] == byte {
                state += 1;
            }
        }
        debug_assert!(state <= self.text.len());
        state
    }
}

/// The matcher state of every multiline secret over one growing chunk.
pub(super) struct BoundaryScan {
    states: Vec<usize>,
    scanned: usize,
}

impl BoundaryScan {
    pub(super) fn new(secrets: &[MultilineSecret]) -> Self {
        Self {
            states: vec![0; secrets.len()],
            scanned: 0,
        }
    }

    /// The longest prefix of any of the secrets that ends the chunk so far.
    pub(super) fn partial(&self) -> usize {
        self.states.iter().copied().max().unwrap_or(0)
    }

    /// How many more bytes a secret needs when `chunk` ends with its first
    /// lines, else 0. Only the bytes added since the last call are read.
    pub(super) fn unfinished(&mut self, secrets: &[MultilineSecret], chunk: &[u8]) -> usize {
        assert_eq!(self.states.len(), secrets.len(), "one state per secret");
        assert!(chunk.len() >= self.scanned, "a chunk only grows");
        let added = &chunk[self.scanned..];
        self.scanned = chunk.len();
        let mut need = 0;
        for (secret, state) in secrets.iter().zip(self.states.iter_mut()) {
            *state = secret.advance(*state, added);
            let open = secret.open[*state];
            if open != 0 {
                need = need.max(secret.text.len() - open);
            }
        }
        need
    }
}

/// Where to cut a chunk stopped at its size cap, holding the rest for the
/// next chunk: at the start of the line where the `partial` secret prefix
/// ending the chunk begins, or past a complete `known` match across that
/// point, so the stream masks what masking the whole input would. A line
/// that fills the chunk is cut where the prefix begins.
pub(super) fn cut_at_cap(
    chunk: &[u8],
    partial: usize,
    known: Option<&regex::bytes::Regex>,
) -> usize {
    assert!(partial <= chunk.len(), "the prefix lies inside the chunk");
    let begins = chunk.len() - partial;
    let line = chunk[..begins]
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |at| at + 1);
    let mut cut = if line > 0 { line } else { begins };
    if let Some(known) = known
        && let Some(across) = known
            .find_iter(chunk)
            .take_while(|found| found.start() < cut)
            .find(|found| found.end() > cut)
    {
        cut = across.end();
    }
    debug_assert!(cut <= chunk.len());
    if cut == 0 { chunk.len() } else { cut }
}

/// The length of the tail of `chunk` that starts a built-in rule whose
/// whitespace may cross a line end (a bearer token, an AWS secret key
/// assignment) and still waits for its value, else 0.
pub(super) fn open_shape(chunk: &[u8]) -> usize {
    // Any non-ASCII byte may belong to a Unicode space; holding more is safe.
    let spaced = |byte: u8| byte.is_ascii_whitespace() || byte == 0x0b || !byte.is_ascii();
    let quoted = |byte: u8| spaced(byte) || matches!(byte, b'"' | b'\'');
    let bearer_end = trim_end(chunk, spaced);
    if bearer_end < chunk.len()
        && let Some(start) = word_start(chunk, bearer_end, &["bearer"])
    {
        return chunk.len() - start;
    }
    let mut name_end = trim_end(chunk, quoted);
    if name_end > 0 && matches!(chunk[name_end - 1], b'=' | b':') {
        name_end = trim_end(&chunk[..name_end - 1], quoted);
    }
    if name_end == chunk.len() {
        return 0;
    }
    let start = word_start(chunk, name_end, &["key", "access", "secret", "aws"]);
    let open = start.map_or(0, |start| chunk.len() - start);
    debug_assert!(open <= chunk.len());
    open
}

/// Where the run of trailing bytes `strip` accepts begins.
fn trim_end(bytes: &[u8], strip: impl Fn(u8) -> bool) -> usize {
    bytes
        .iter()
        .rposition(|byte| !strip(*byte))
        .map_or(0, |at| at + 1)
}

/// Where a word ending at `end` begins when it spells `parts`, read from its
/// end, case-insensitively with an optional `_` between parts.
fn word_start(text: &[u8], end: usize, parts: &[&str]) -> Option<usize> {
    assert!(
        end <= text.len() && !parts.is_empty(),
        "a word inside the text"
    );
    let mut start = end;
    for (at, part) in parts.iter().enumerate() {
        if at > 0 && start > 0 && text[start - 1] == b'_' {
            start -= 1;
        }
        start = start.checked_sub(part.len())?;
        if !text[start..start + part.len()].eq_ignore_ascii_case(part.as_bytes()) {
            return None;
        }
    }
    let word = |byte: &u8| byte.is_ascii_alphanumeric() || *byte == b'_';
    let bounded = start == 0 || !text.get(start - 1).is_some_and(word);
    bounded.then_some(start)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The definition the tables replace: every line-ending proper prefix
    /// of the secret that ends the chunk, tried one by one.
    fn naive(secret: &str, chunk: &[u8]) -> usize {
        secret
            .match_indices('\n')
            .map(|(at, _)| &secret.as_bytes()[..=at])
            .filter(|head| head.len() < secret.len() && chunk.ends_with(head))
            .map(|head| secret.len() - head.len())
            .max()
            .unwrap_or(0)
    }

    #[test]
    fn the_need_matches_the_prefix_definition() {
        let secrets = ["ab\nab\nab\nZ", "a\na\na\n", "x\ny\nx\ny\nz", "\n\nq\n\n"];
        let chunks = [
            "",
            "ab\n",
            "zab\nab\n",
            "a\na\n",
            "x\ny\nx\ny\n",
            "\n\n",
            "q\n\n",
            "pp\n",
        ];
        for secret in secrets {
            let table = [MultilineSecret::new(secret)];
            for chunk in chunks {
                let mut scan = BoundaryScan::new(&table);
                // Fed in two parts, as `read_chunk` grows a chunk.
                let (first, _) = chunk.as_bytes().split_at(chunk.len() / 2);
                scan.unfinished(&table, first);
                let need = scan.unfinished(&table, chunk.as_bytes());
                assert_eq!(
                    need,
                    naive(secret, chunk.as_bytes()),
                    "{secret:?} after {chunk:?}"
                );
            }
        }
    }

    #[test]
    fn a_long_repetitive_secret_is_scanned_in_linear_time() {
        let secret = format!("{}Z", "a\n".repeat(64 * 1024));
        let table = [MultilineSecret::new(&secret)];
        let chunk = "a\n".repeat(512 * 1024);
        let mut scan = BoundaryScan::new(&table);
        let started = std::time::Instant::now();
        for end in (0..=chunk.len()).step_by(2) {
            scan.unfinished(&table, &chunk.as_bytes()[..end]);
        }
        assert_eq!(scan.unfinished(&table, chunk.as_bytes()), secret.len() - 2);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "took {:?}",
            started.elapsed()
        );
    }
}
