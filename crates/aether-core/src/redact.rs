//! Masking recognizable secrets before text is persisted or shown.
//!
//! One rule set, so every path that writes provider or tool text to a durable
//! record - events, invocation rows, artifacts, logs - masks the same shapes.

use std::borrow::Cow;
use std::sync::{Arc, LazyLock};

use regex::Regex;

mod boundary;
mod span;

pub use span::{CUT_CONTEXT_BYTES, PAGE_LOOKBACK_BYTES};

/// Replaces recognizable secrets before output is persisted or shown.
#[derive(Clone)]
pub struct Redactor {
    patterns: Vec<(Regex, &'static str)>,
    /// Known secret values that span lines, which a stream must not split.
    multiline_known: Arc<[boundary::MultilineSecret]>,
    /// The known-secret rule over bytes, kept while a secret spans lines: a
    /// stream held at its size cap must not cut through one of its matches.
    known_bytes: Option<regex::bytes::Regex>,
    /// Fewest bytes any rule matches; a shorter text run needs no masking.
    shortest_match: usize,
    /// Bytes in the longest spelling of a known secret, 0 with none.
    longest_known: usize,
}

/// Shortest known secret value masked verbatim: a shorter one would also
/// mask ordinary words that happen to equal it.
pub const MIN_KNOWN_SECRET_CHARS: usize = 8;

/// Not derived: a known-secret rule's pattern is the secret itself.
impl std::fmt::Debug for Redactor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Redactor")
            .field("rules", &self.patterns.len())
            .finish()
    }
}

const RULE_COUNT: usize = 19;

/// What every private key block rule leaves in place of the block.
const PRIVATE_KEY_MASK: &str = "[REDACTED:private_key]";

static DEFAULT_PATTERNS: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    // One rule per key shape the distribution can hold: every credentialed
    // provider it ships, plus the tool-side shapes.
    let rules: [(&str, &str); RULE_COUNT] = [
        (r"sk-ant-[A-Za-z0-9_\-]{8,}", "[REDACTED:anthropic_key]"),
        (r"sk-[A-Za-z0-9_\-]{16,}", "[REDACTED:api_key]"),
        (r"\bgsk_[A-Za-z0-9]{20,}", "[REDACTED:groq_key]"),
        (r"\bxai-[A-Za-z0-9]{20,}", "[REDACTED:xai_key]"),
        (r"\bAIza[A-Za-z0-9_\-]{30,}", "[REDACTED:google_key]"),
        (
            r"\b(?:ghp|gho|ghs|ghu|ghr)_[A-Za-z0-9]{20,}",
            "[REDACTED:github_token]",
        ),
        (r"\bgithub_pat_[A-Za-z0-9_]{20,}", "[REDACTED:github_token]"),
        (r"\bAKIA[0-9A-Z]{16}\b", "[REDACTED:aws_access_key]"),
        (
            r"(?i)\bbearer\s+[A-Za-z0-9\-._~+/]{8,}=*",
            "Bearer [REDACTED]",
        ),
        (
            r"\beyJ[A-Za-z0-9_\-]{8,}\.[A-Za-z0-9_\-]{8,}\.[A-Za-z0-9_\-]{8,}",
            "[REDACTED:jwt]",
        ),
        (
            r#"(?i)\baws_?secret_?access_?key\b[\s"']*[=:][\s"']*[A-Za-z0-9/+=]{40}"#,
            "[REDACTED:aws_secret_key]",
        ),
        (
            r"\bxox[abposr]-[A-Za-z0-9\-]{10,}",
            "[REDACTED:slack_token]",
        ),
        (r"\bglpat-[A-Za-z0-9_\-]{20,}", "[REDACTED:gitlab_token]"),
        (
            r"\b[sr]k_(?:live|test)_[A-Za-z0-9]{16,}",
            "[REDACTED:stripe_key]",
        ),
        (r"\bnpm_[A-Za-z0-9]{30,}", "[REDACTED:npm_token]"),
        (r"\bhf_[A-Za-z0-9]{30,}", "[REDACTED:huggingface_token]"),
        (
            r"-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*?-----END [A-Z ]*PRIVATE KEY-----",
            PRIVATE_KEY_MASK,
        ),
        // Truncated captures: a tail that starts inside a key, or a key whose end was cut.
        (
            r"\A[A-Za-z0-9+/=\r\n ]*-----END [A-Z ]*PRIVATE KEY-----",
            PRIVATE_KEY_MASK,
        ),
        (
            r"-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*\z",
            PRIVATE_KEY_MASK,
        ),
    ];
    rules
        .iter()
        .map(|(re, rep)| {
            // A rule that cannot compile must stop the process, not vanish.
            #[allow(clippy::expect_used)]
            let compiled = Regex::new(re).expect("a built-in redaction rule compiles");
            (compiled, *rep)
        })
        .collect()
});

static KEY_BEGIN: LazyLock<Regex> =
    LazyLock::new(|| compile_rule(r"-----BEGIN [A-Z ]*PRIVATE KEY-----", "").0);
static KEY_END: LazyLock<Regex> =
    LazyLock::new(|| compile_rule(r"-----END [A-Z ]*PRIVATE KEY-----", "").0);

impl Default for Redactor {
    fn default() -> Self {
        Self::with_patterns(DEFAULT_PATTERNS.clone())
    }
}

impl Redactor {
    /// Every built-in rule and known secret matches at least
    /// [`MIN_KNOWN_SECRET_CHARS`] bytes.
    fn with_patterns(patterns: Vec<(Regex, &'static str)>) -> Self {
        Self {
            patterns,
            multiline_known: Arc::from([]),
            known_bytes: None,
            shortest_match: MIN_KNOWN_SECRET_CHARS,
            longest_known: 0,
        }
    }

    /// A redactor with the default rules plus extra `(regex, replacement)` pairs.
    pub fn with_extra(extra: impl IntoIterator<Item = (Regex, &'static str)>) -> Self {
        let mut r = Self::default();
        let before = r.patterns.len();
        r.patterns.extend(extra);
        // An extra rule may match a single byte.
        if r.patterns.len() > before {
            r.shortest_match = 1;
        }
        r
    }

    /// The default rules plus one that masks each known secret value wherever
    /// it appears verbatim. Values shorter than [`MIN_KNOWN_SECRET_CHARS`]
    /// after trimming are skipped. Fails only when the values exceed the
    /// regex size limit; the error never quotes them.
    pub fn with_known_secrets<'v>(
        values: impl IntoIterator<Item = &'v str>,
    ) -> Result<Self, KnownSecretsError> {
        let mut known: Vec<&str> = values
            .into_iter()
            .map(str::trim)
            .filter(|value| value.chars().count() >= MIN_KNOWN_SECRET_CHARS)
            .collect();
        // Longest first, so a value that contains another is masked whole.
        known.sort_unstable_by(|a, b| b.len().cmp(&a.len()).then(a.cmp(b)));
        known.dedup();
        let mut redactor = Self::default();
        if known.is_empty() {
            return Ok(redactor);
        }
        // A tool that prints JSON shows a value with a newline, quote or
        // backslash escaped, so that spelling is masked as well.
        let escaped: Vec<String> = known.iter().map(|value| json_escaped(value)).collect();
        let mut spellings: Vec<&str> = known
            .iter()
            .copied()
            .chain(escaped.iter().map(String::as_str))
            .collect();
        spellings.sort_unstable_by(|a, b| b.len().cmp(&a.len()).then(a.cmp(b)));
        spellings.dedup();
        redactor.longest_known = spellings[0].len();
        debug_assert!(
            spellings
                .iter()
                .all(|value| value.len() <= redactor.longest_known)
        );
        let alternation = spellings
            .iter()
            .map(|value| regex::escape(value))
            .collect::<Vec<_>>()
            .join("|");
        let rule = Regex::new(&alternation).map_err(|_| KnownSecretsError {
            values: known.len(),
        })?;
        if known.iter().any(|value| value.contains('\n')) {
            let bytes = regex::bytes::Regex::new(&alternation).map_err(|_| KnownSecretsError {
                values: known.len(),
            })?;
            redactor.known_bytes = Some(bytes);
        }
        // First, before a shape rule rewrites part of a value and hides the rest from it.
        redactor
            .patterns
            .insert(0, (rule, "[REDACTED:known_secret]"));
        redactor.multiline_known = known
            .iter()
            .filter(|value| value.contains('\n'))
            .map(|value| boundary::MultilineSecret::new(value))
            .collect();
        debug_assert_eq!(redactor.patterns.len(), RULE_COUNT + 1);
        Ok(redactor)
    }

    pub fn redact<'a>(&self, text: &'a str) -> Cow<'a, str> {
        let mut out = Cow::Borrowed(text);
        for (re, rep) in &self.patterns {
            if re.is_match(&out) {
                out = Cow::Owned(re.replace_all(&out, *rep).into_owned());
            }
        }
        out
    }

    /// Masks the UTF-8 text runs of `bytes` and keeps every other byte as
    /// it is, so a log with a stray invalid byte is still cleaned.
    pub fn redact_bytes<'a>(&self, bytes: &'a [u8]) -> Cow<'a, [u8]> {
        self.redact_piece(bytes, &mut false)
    }

    /// [`Self::redact_bytes`] for one piece of a stream: `in_key` says whether
    /// the piece starts inside a private key block, and is left saying whether
    /// it ends inside one.
    fn redact_piece<'a>(&self, bytes: &'a [u8], in_key: &mut bool) -> Cow<'a, [u8]> {
        if let Ok(text) = std::str::from_utf8(bytes) {
            return match self.redact_run(text, in_key) {
                Cow::Borrowed(_) => Cow::Borrowed(bytes),
                Cow::Owned(clean) => Cow::Owned(clean.into_bytes()),
            };
        }
        let mut out = Vec::with_capacity(bytes.len());
        let mut changed = false;
        for chunk in bytes.utf8_chunks() {
            match self.redact_run(chunk.valid(), in_key) {
                Cow::Borrowed(text) => out.extend_from_slice(text.as_bytes()),
                Cow::Owned(clean) => {
                    changed = true;
                    out.extend_from_slice(clean.as_bytes());
                }
            }
            // Inside a key block every byte is the key's, valid text or not.
            if *in_key {
                changed |= !chunk.invalid().is_empty();
            } else {
                out.extend_from_slice(chunk.invalid());
            }
        }
        if !changed {
            debug_assert_eq!(out, bytes, "an unchanged copy holds the same bytes");
            return Cow::Borrowed(bytes);
        }
        Cow::Owned(out)
    }

    /// Masks one UTF-8 run. A run that starts inside a key block loses
    /// everything up to the block's end, whose start was masked already.
    fn redact_run<'a>(&self, text: &'a str, in_key: &mut bool) -> Cow<'a, str> {
        let rest = if *in_key {
            let Some(end) = KEY_END.find(text) else {
                if text.is_empty() {
                    return Cow::Borrowed(text);
                }
                return Cow::Owned(String::new());
            };
            &text[end.end()..]
        } else {
            text
        };
        *in_key = ends_inside_key(rest);
        debug_assert!(!*in_key || rest.len() >= self.shortest_match);
        let masked = if rest.len() < self.shortest_match {
            Cow::Borrowed(rest)
        } else {
            self.redact(rest)
        };
        if rest.len() == text.len() {
            return masked;
        }
        Cow::Owned(masked.into_owned())
    }

    /// Copies `input` to `output` with every chunk masked, holding about one
    /// chunk in memory. Chunks end at a line break, and never partway through
    /// a known secret, bearer token or AWS key assignment that spans lines; a
    /// key block cut between chunks is masked from its start to its end, or
    /// to the end of the input when it has none, as [`Self::redact`] does. Only a line longer than
    /// [`STREAM_LINE_LIMIT_BYTES`] is cut mid-line. Returns the bytes written.
    pub fn redact_stream(
        &self,
        mut input: impl std::io::BufRead,
        mut output: impl std::io::Write,
    ) -> std::io::Result<u64> {
        let mut chunk = Vec::with_capacity(STREAM_CHUNK_BYTES);
        let mut in_key = false;
        let mut written = 0u64;
        loop {
            let ready = self.read_chunk(&mut input, &mut chunk)?;
            if chunk.is_empty() {
                break;
            }
            assert!(
                ready > 0 && ready <= chunk.len(),
                "every chunk moves the stream on"
            );
            let clean = self.redact_piece(&chunk[..ready], &mut in_key);
            output.write_all(&clean)?;
            written += clean.len() as u64;
            drop(clean);
            // What is held back starts the next chunk.
            chunk.drain(..ready);
        }
        output.flush()?;
        Ok(written)
    }

    /// Reads about [`STREAM_CHUNK_BYTES`] more on to a line end, and on again
    /// while the chunk ends partway through a known secret or a built-in rule
    /// whose whitespace crosses lines. Returns how much
    /// of the chunk is ready to mask; the rest is held back for the next one.
    fn read_chunk(
        &self,
        input: &mut impl std::io::BufRead,
        chunk: &mut Vec<u8>,
    ) -> std::io::Result<usize> {
        use std::io::{BufRead as _, Read as _};
        const MAX: usize = STREAM_CHUNK_BYTES + STREAM_LINE_LIMIT_BYTES;
        debug_assert!(chunk.len() < MAX, "a held-back tail leaves room to read");
        let mut want = STREAM_CHUNK_BYTES;
        let mut scan = boundary::BoundaryScan::new(&self.multiline_known);
        let mut ended = false;
        while want > 0 && chunk.len() < MAX {
            let before = chunk.len();
            let room = |chunk: &Vec<u8>| (MAX - chunk.len()) as u64;
            input
                .by_ref()
                .take((want as u64).min(room(chunk)))
                .read_to_end(chunk)?;
            if chunk.last() != Some(&b'\n') {
                input.by_ref().take(room(chunk)).read_until(b'\n', chunk)?;
            }
            if chunk.len() == before {
                ended = true;
                break;
            }
            want = scan.unfinished(&self.multiline_known, chunk);
            if want == 0 && boundary::open_shape(chunk) > 0 {
                // Read a whole chunk on, so a run of blank lines is scanned once.
                want = STREAM_CHUNK_BYTES;
            }
        }
        debug_assert!(chunk.len() <= MAX);
        if ended || chunk.len() < MAX {
            return Ok(chunk.len());
        }
        let partial = scan.partial().max(boundary::open_shape(chunk));
        let cut = boundary::cut_at_cap(chunk, partial, self.known_bytes.as_ref());
        // A line that fills the chunk is cut mid-line: hold back enough to see
        // any secret across the cut whole, and keep the cut off it.
        let latest = chunk.len().saturating_sub(self.cut_context());
        let (_, clear) = self.clear_span(chunk, 0, cut.min(latest));
        debug_assert!(clear <= chunk.len());
        Ok(if clear == 0 { chunk.len() } else { clear })
    }
}

/// `value` as it reads inside a JSON string, without the quotes.
fn json_escaped(value: &str) -> String {
    let quoted = serde_json::Value::from(value).to_string();
    debug_assert!(quoted.len() >= 2 && quoted.starts_with('"') && quoted.ends_with('"'));
    quoted[1..quoted.len() - 1].to_owned()
}

/// Whether `text` opens a private key block it does not close.
fn ends_inside_key(text: &str) -> bool {
    if !text.contains("-----BEGIN ") {
        return false;
    }
    let Some(begin) = KEY_BEGIN.find_iter(text).last() else {
        return false;
    };
    KEY_END.find_at(text, begin.end()).is_none()
}

/// How much [`Redactor::redact_stream`] reads before it looks for a line end.
pub const STREAM_CHUNK_BYTES: usize = 1 << 20;

/// How far past a chunk [`Redactor::redact_stream`] reads to finish its line.
pub const STREAM_LINE_LIMIT_BYTES: usize = 4 << 20;

/// The known secret values could not be compiled into one rule.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{values} known secret values exceed the redaction rule size limit")]
pub struct KnownSecretsError {
    pub values: usize,
}

const LOCATOR_SECRET_NAME: &str = r"(?:token|secret|passw(?:or)?d|api[_\-]?key|auth|credential)";

/// Masks what an endpoint or command line can carry: URL passwords, and
/// secret-looking `name=value` pairs or `--flag value` pairs. Scheme, host,
/// path and the other arguments stay readable, and the default rules still apply.
pub fn redact_locator(text: &str) -> Cow<'_, str> {
    static LOCATOR: LazyLock<Redactor> = LazyLock::new(|| {
        let assignment =
            format!(r"(?i)\b([A-Za-z0-9_\-]*{LOCATOR_SECRET_NAME}[A-Za-z0-9_\-]*)=[^\s&]+");
        let flag_value =
            format!(r"(?i)(--?[A-Za-z0-9_\-]*{LOCATOR_SECRET_NAME}[A-Za-z0-9_\-]*)(\s+)[^\s-]\S*");
        // Locator rules run first, so a URL password is masked whole rather than
        // after a token rule has already rewritten part of it.
        let mut patterns = vec![
            compile_rule(
                r"([A-Za-z][A-Za-z0-9+.\-]*://[^\s/:@]*):[^\s/]*@",
                "$1:[REDACTED]@",
            ),
            compile_rule(r"([A-Za-z][A-Za-z0-9+.\-]*://)[^\s/:@]+@", "$1[REDACTED]@"),
            compile_rule(&assignment, "$1=[REDACTED]"),
            compile_rule(&flag_value, "$1$2[REDACTED]"),
        ];
        patterns.extend(DEFAULT_PATTERNS.iter().cloned());
        Redactor::with_patterns(patterns)
    });
    LOCATOR.redact(text)
}

/// Masks the secret values of a command line one argument at a time, so a
/// value is hidden whole even when it holds `&`, spaces or a leading `-`.
pub fn redact_argv(args: &[String]) -> Vec<String> {
    static SECRET_FLAG: LazyLock<Regex> = LazyLock::new(|| {
        let pattern = format!(r"(?i)^--?[A-Za-z0-9_\-]*{LOCATOR_SECRET_NAME}[A-Za-z0-9_\-]*$");
        compile_rule(&pattern, "").0
    });
    let mut out = Vec::with_capacity(args.len());
    let mut hide_next = false;
    for arg in args {
        // Which argument is a value cannot be known, so the one after a secret
        // flag is always hidden. A hidden secret-looking flag hides its successor too.
        if std::mem::take(&mut hide_next) {
            hide_next =
                SECRET_FLAG.is_match(arg.split_once('=').map_or(arg.as_str(), |(name, _)| name));
            out.push("[REDACTED]".to_owned());
            continue;
        }
        match arg.split_once('=') {
            Some((name, _)) if SECRET_FLAG.is_match(name) => {
                out.push(format!("{name}=[REDACTED]"));
            }
            _ if SECRET_FLAG.is_match(arg) => {
                hide_next = true;
                out.push(arg.clone());
            }
            _ => out.push(redact_locator(arg).into_owned()),
        }
    }
    out
}

/// Same rule as the built-in set: a rule that cannot compile stops the process.
fn compile_rule(pattern: &str, replacement: &'static str) -> (Regex, &'static str) {
    #[allow(clippy::expect_used)]
    let compiled = Regex::new(pattern).expect("a built-in redaction rule compiles");
    (compiled, replacement)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locator_keeps_the_readable_parts_and_masks_secrets() {
        let out = redact_locator("http://user:SECRETURLPW@127.0.0.1:9/mcp");
        assert_eq!(out, "http://user:[REDACTED]@127.0.0.1:9/mcp");
        let out = redact_locator("stdio: npx server --token=SECRETARG /srv/data");
        assert_eq!(out, "stdio: npx server --token=[REDACTED] /srv/data");
        let out = redact_locator("stdio: srv --api-key SECRETARG --verbose");
        assert_eq!(out, "stdio: srv --api-key [REDACTED] --verbose");
        let out = redact_locator("https://h.example/mcp?access_token=SECRETQ&page=2");
        assert_eq!(out, "https://h.example/mcp?access_token=[REDACTED]&page=2");
    }

    #[test]
    fn locator_masks_a_userinfo_without_password_and_token_shapes() {
        let out = redact_locator("https://ghp_abcdefghijklmnopqrstuvwxyz1234@h.example/mcp");
        assert_eq!(out, "https://[REDACTED]@h.example/mcp");
        let out = redact_locator("stdio: srv ghp_abcdefghijklmnopqrstuvwxyz1234");
        assert!(!out.contains("ghp_abc"), "{out}");
    }

    #[test]
    fn locator_leaves_plain_text_borrowed() {
        assert!(matches!(
            redact_locator("http://127.0.0.1:9/mcp"),
            Cow::Borrowed(_)
        ));
        assert!(matches!(
            redact_locator("stdio: npx -y srv"),
            Cow::Borrowed(_)
        ));
    }

    #[test]
    fn argv_masks_each_secret_value_whole() {
        let argv =
            |items: &[&str]| -> Vec<String> { items.iter().map(|s| s.to_string()).collect() };
        let out = redact_argv(&argv(&["--token=abc&SECRETTAIL", "/srv"]));
        assert_eq!(out, ["--token=[REDACTED]", "/srv"]);
        let out = redact_argv(&argv(&["--password=hello SECRETTAIL", "x"]));
        assert_eq!(out, ["--password=[REDACTED]", "x"]);
        let out = redact_argv(&argv(&["--token", "-SECRETARG", "--verbose"]));
        assert_eq!(out, ["--token", "[REDACTED]", "[REDACTED]"]);
        let out = redact_argv(&argv(&["--auth", "--token", "SECRET"]));
        assert_eq!(out, ["--auth", "[REDACTED]", "[REDACTED]"]);
        let out = redact_argv(&argv(&["--auth", "-token", "SECRET"]));
        assert_eq!(out, ["--auth", "[REDACTED]", "[REDACTED]"]);
        let out = redact_argv(&argv(&["--token", "--SECRETARG"]));
        assert_eq!(out, ["--token", "[REDACTED]"]);
    }

    /// One planted key per credentialed provider the distribution ships, so
    /// the §10 guarantee is not strongest for only the two whose 401 bodies
    /// were studied. Local providers (ollama, vllm, lmstudio, llama.cpp) hold
    /// no credential and have no shape to mask.
    #[test]
    fn masks_every_shipped_providers_key_shape() {
        let r = Redactor::default();
        let cases = [
            (
                "anthropic",
                "sk-ant-api03-NOTAREALKEYbutkeyshaped0123456789",
            ),
            ("openai", "sk-proj-NOTAREALKEYbutkeyshaped0123456789"),
            ("openrouter", "sk-or-v1-NOTAREALKEYbutkeyshaped0123456789"),
            ("groq", "gsk_NOTAREALKEYbutkeyshaped0123456789"),
            ("xai", "xai-NOTAREALKEYbutkeyshaped0123456789"),
            ("google", "AIzaNOTAREALKEYbutkeyshaped0123456789xyz"),
        ];
        for (provider, key) in cases {
            let text = format!("Incorrect API key provided: {key}. See the docs.");
            let out = r.redact(&text);
            assert!(
                !out.contains(key),
                "{provider}: the key survived redaction: {out}"
            );
            assert!(out.contains("[REDACTED"), "{provider}: {out}");
        }
    }

    #[test]
    fn redacts_known_secret_shapes() {
        let r = Redactor::default();
        let input = "key sk-ant-api03-abcdefghijklmnop and sk-proj-abcdefghijklmnopqrstuvwxyz \
                     ghp_abcdefghijklmnopqrstuvwxyz1234 AKIAIOSFODNN7EXAMPLE Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.abc \
                     -----BEGIN RSA PRIVATE KEY-----\nMIIE\n-----END RSA PRIVATE KEY-----";
        let out = r.redact(input);
        assert!(!out.contains("sk-ant-api03"));
        assert!(!out.contains("sk-proj-"));
        assert!(!out.contains("ghp_abc"));
        assert!(!out.contains("AKIAIOSFODNN7EXAMPLE"));
        assert!(!out.contains("eyJhbGci"));
        assert!(!out.contains("MIIE"));
        assert!(out.contains("[REDACTED:anthropic_key]"));
        assert!(out.contains("Bearer [REDACTED]"));
        assert!(out.contains("[REDACTED:private_key]"));
    }

    #[test]
    fn every_rule_compiles() {
        assert_eq!(Redactor::default().patterns.len(), RULE_COUNT);
    }

    #[test]
    fn masks_widened_shapes_and_leaves_controls() {
        let r = Redactor::default();
        let secrets = [
            "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dBjftJeZ4CVPmB92K27uhbUJU1p1r",
            "aws_secret_access_key=wJalrXUtnFEMIK7MDENGbPxRfiCYEXAMPLEKEY12",
            "xoxb-123456789012-abcdefghijkl",
            "glpat-abcdefghij0123456789",
            "sk_live_abcdefghijklmnop0123",
            "npm_abcdefghijklmnopqrstuvwxyz0123456789",
            "hf_abcdefghijklmnopqrstuvwxyz0123456789",
        ];
        for secret in secrets {
            let text = format!("value: {secret} end");
            let out = r.redact(&text);
            assert!(out.contains("[REDACTED"), "{secret} survived: {out}");
            assert!(!out.contains(&secret[secret.len() - 12..]), "{out}");
        }
        for control in ["see hf_model docs", "task_live_run ok", "eyJ short", "xox"] {
            assert_eq!(r.redact(control).as_ref(), control);
        }
    }

    #[test]
    fn masks_private_keys_cut_by_truncation() {
        let r = Redactor::default();
        let headless = "Zm9vYmFy\nYmF6cXV4\n-----END RSA PRIVATE KEY-----\nnext line";
        let out = r.redact(headless);
        assert!(
            !out.contains("Zm9vYmFy") && out.contains("next line"),
            "{out}"
        );
        let endless = "log\n-----BEGIN PRIVATE KEY-----\nZm9vYmFy\nYmF6cXV4";
        let out = r.redact(endless);
        assert!(!out.contains("Zm9vYmFy") && out.starts_with("log"), "{out}");
    }

    #[test]
    fn known_secrets_are_masked_verbatim_and_short_ones_skipped() {
        let token = "fleet-token-0123456789abcdef";
        let inner = "sk-ant-api03-INNERKEYbutkeyshaped0123456789";
        let wrapped = format!("pre{inner}post");
        let r = Redactor::with_known_secrets([token, " short7 ", wrapped.as_str(), token]).unwrap();
        let text = format!("env: TOKEN={token} K={wrapped} word short7");
        let out = r.redact(&text);
        assert!(!out.contains(token) && !out.contains("post"), "{out}");
        assert_eq!(out.matches("[REDACTED:known_secret]").count(), 2, "{out}");
        assert!(
            out.ends_with("word short7"),
            "a short value is left alone: {out}"
        );
        let dotted = Redactor::with_known_secrets(["a.b.c.d.e.f.g.h"]).unwrap();
        assert_eq!(dotted.redact("aXbXcXdXeXfXgXh").as_ref(), "aXbXcXdXeXfXgXh");
    }

    #[test]
    fn known_secrets_never_appear_in_debug_output() {
        let token = "fleet-token-0123456789abcdef";
        let r = Redactor::with_known_secrets([token]).unwrap();
        assert!(!format!("{r:?}").contains("fleet-token"), "{r:?}");
        assert_eq!(
            Redactor::with_known_secrets(["tiny"])
                .unwrap()
                .patterns
                .len(),
            RULE_COUNT
        );
    }

    #[test]
    fn clean_text_is_borrowed() {
        let r = Redactor::default();
        assert!(matches!(r.redact("nothing secret here"), Cow::Borrowed(_)));
    }

    const KNOWN: &str = "fleet-token-0123456789abcdef";

    #[test]
    fn bytes_are_masked_around_invalid_utf8_and_otherwise_kept() {
        let r = Redactor::with_known_secrets([KNOWN]).unwrap();
        let mut log = b"\xff\xfeheader ".to_vec();
        log.extend_from_slice(KNOWN.as_bytes());
        log.extend_from_slice(b" tail\x80\x00");
        let out = r.redact_bytes(&log);
        let text = String::from_utf8_lossy(&out);
        assert!(!text.contains(KNOWN), "{text}");
        assert!(out.starts_with(b"\xff\xfeheader [REDACTED:known_secret] tail"));
        assert!(
            out.ends_with(b"\x80\x00"),
            "invalid bytes are kept verbatim"
        );
        let binary = [0u8, 159, 146, 150, 255, 7];
        assert!(matches!(r.redact_bytes(&binary), Cow::Borrowed(_)));
    }

    #[test]
    fn stream_masks_secrets_across_chunks_and_lines() {
        let r = Redactor::with_known_secrets([KNOWN]).unwrap();
        // A secret straddling the first chunk's end, and one per later line.
        let mut input = "x".repeat(STREAM_CHUNK_BYTES - 5);
        input.push_str(KNOWN);
        input.push('\n');
        for i in 0..20_000 {
            input.push_str(&format!("line {i} {KNOWN}\n"));
        }
        input.push_str("-----BEGIN PRIVATE KEY-----\nZm9vYmFy\n-----END PRIVATE KEY-----");
        let mut out = Vec::new();
        let written = r.redact_stream(input.as_bytes(), &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert_eq!(written, text.len() as u64);
        assert!(!text.contains(KNOWN) && !text.contains("Zm9vYmFy"));
        assert_eq!(text.matches("[REDACTED:known_secret]").count(), 20_001);
        assert!(
            text.ends_with("[REDACTED:private_key]"),
            "{}",
            &text[text.len() - 64..]
        );
    }

    /// A run of a secret's own first lines grows a chunk to its cap; the
    /// tail is held for the next chunk, so the stream masks what masking the
    /// whole input would.
    #[test]
    fn a_stream_held_at_its_cap_masks_as_the_whole_input_would() {
        let lines = (STREAM_CHUNK_BYTES + STREAM_LINE_LIMIT_BYTES) / 9 + 1;
        let token = format!("ghp_{}secret01\n", "A".repeat(28));
        let cases = [
            // The secret itself completes after the cap.
            (
                vec!["alpha123\nalpha123\nZ"],
                "alpha123\n".repeat(lines) + "Z\n",
            ),
            // A shorter secret on the short line the cap falls in.
            (
                vec!["alpha123\nalpha123\nZ", "alpha123"],
                "alpha123\n".repeat(lines) + "Z\n",
            ),
            // The secret starts inside a token, which must stay whole.
            (
                vec!["secret01\nghp_AAAAAAAAAAAAAAAAAAAAAAAAAAAAsecret01\nZ"],
                token.repeat(lines / 4) + "Z\n",
            ),
            // Back-to-back matches: one of them straddles the held-back tail,
            // whichever line the cap falls in.
            (vec!["aaaaaaaa\naaaaaaaa\n"], "aaaaaaaa\n".repeat(lines)),
            (vec!["aaaaaaaa\naaaaaaaa\n"], "aaaaaaaa\n".repeat(lines + 1)),
        ];
        for (known, input) in cases {
            let redactor = Redactor::with_known_secrets(known.clone()).expect("compiles");
            let whole = redactor.redact_bytes(input.as_bytes());
            assert!(
                String::from_utf8_lossy(&whole).contains("[REDACTED:known_secret]"),
                "{known:?}: the case masks something"
            );
            let mut out = Vec::new();
            redactor
                .redact_stream(input.as_bytes(), &mut out)
                .expect("streams");
            assert!(
                out == whole.as_ref(),
                "{known:?}: the stream differs from masking the whole input"
            );
        }
    }

    #[test]
    fn a_line_cut_at_the_stream_cap_never_splits_a_secret() {
        const CAP: usize = STREAM_CHUNK_BYTES + STREAM_LINE_LIMIT_BYTES;
        let known = "fleet-token-0123456789abcdef";
        let key = "sk-ant-api03-AbCdEfGhIjKlMnOpQrStUvWxYz012345";
        let r = Redactor::with_known_secrets([known]).unwrap();
        for (secret, label) in [(known, "known_secret"), (key, "anthropic_key")] {
            for before_cap in [1, 8, secret.len() - 1] {
                // One line with no break, so the cap cuts it mid-line.
                let input = format!("{} {secret} tail\n", ".".repeat(CAP - before_cap - 1));
                let mut out = Vec::new();
                r.redact_stream(input.as_bytes(), &mut out).unwrap();
                let out = String::from_utf8(out).unwrap();
                assert!(
                    out.contains(&format!("[REDACTED:{label}]")),
                    "{label} {before_cap}: masked whole"
                );
                assert!(
                    !out.contains(&secret[secret.len() - 8..]),
                    "{label} {before_cap}: no part of it shows"
                );
                assert!(out.ends_with(" tail\n"), "the rest of the line is kept");
            }
        }
    }

    #[test]
    fn stream_never_splits_a_known_secret_that_spans_lines() {
        let secret = "alpha123\nbeta4567";
        let r = Redactor::with_known_secrets([secret]).unwrap();
        // The first chunk ends at the newline inside the secret.
        let mut input = "x".repeat(STREAM_CHUNK_BYTES - 9);
        input.push_str(secret);
        input.push_str("\nafter\n");
        let mut out = Vec::new();
        r.redact_stream(input.as_bytes(), &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(!text.contains("alpha123") && !text.contains("beta4567"));
        assert!(
            text.ends_with("x[REDACTED:known_secret]\nafter\n"),
            "{}",
            &text[text.len() - 64..]
        );
    }

    /// A built-in rule whose whitespace crosses a line end is never split
    /// where a chunk ends, however many blank lines it spans.
    #[test]
    fn stream_never_splits_a_built_in_shape_at_a_line_end() {
        let token = "opaque-token-0123456789";
        let aws = "wJalrXUtnFEMIK7MDENGbPxRfiCYEXAMPLEKEY01";
        let cases = [
            ("Bearer\n", token.to_owned()),
            ("bearer \n\n\n", format!("\n  {token}")),
            ("AWS_SECRET_ACCESS_KEY =\n", aws.to_owned()),
            ("aws_secret_access_key\n", format!("= '\n{aws}'")),
            ("awsSecretAccessKey: \"\n", format!("{aws}\"")),
        ];
        let r = Redactor::default();
        for (head, rest) in cases {
            // The first chunk ends with `head`; the value follows in the next.
            let mut input = "x".repeat(STREAM_CHUNK_BYTES - head.len() - 1);
            input.push('\n');
            input.push_str(head);
            input.push_str(&rest);
            input.push('\n');
            let whole = r.redact_bytes(input.as_bytes());
            let masked = String::from_utf8_lossy(&whole);
            assert!(
                !masked.contains(token) && !masked.contains(aws),
                "{head:?}: masking the whole input hides the value"
            );
            let mut out = Vec::new();
            r.redact_stream(input.as_bytes(), &mut out)
                .expect("streams");
            assert!(
                out == whole.as_ref(),
                "{head:?}: the stream differs from masking the whole input"
            );
        }
    }

    #[test]
    fn the_open_shape_is_the_tail_from_its_rule_name() {
        assert_eq!(boundary::open_shape(b"x Bearer \n"), 8);
        assert_eq!(boundary::open_shape(b"AWS_secret_ACCESS_key=\"\n"), 24);
        assert_eq!(boundary::open_shape(b"a Bearer abc\n"), 0);
        assert_eq!(boundary::open_shape(b"unbearer\n"), 0);
        assert_eq!(boundary::open_shape(b"my_aws_secret_access_key\n"), 0);
        assert_eq!(boundary::open_shape(b"key = \n"), 0);
    }

    #[test]
    fn stream_masks_a_key_block_spanning_two_or_more_chunks() {
        let r = Redactor::default();
        let body = format!("{}\n", "QUJD".repeat(16));
        // About 1.3 MiB and 2.6 MiB of short lines: two and three chunks.
        for lines in [20_000, 40_000] {
            let input = format!(
                "before\n-----BEGIN PRIVATE KEY-----\n{}-----END PRIVATE KEY-----\nafter\n",
                body.repeat(lines)
            );
            let mut out = Vec::new();
            r.redact_stream(input.as_bytes(), &mut out).unwrap();
            let text = String::from_utf8(out).unwrap();
            assert_eq!(text, "before\n[REDACTED:private_key]\nafter\n", "{lines}");
        }
    }

    #[test]
    fn a_key_block_broken_by_invalid_bytes_is_masked_whole() {
        let r = Redactor::default();
        let log =
            b"-----BEGIN PRIVATE KEY-----\nAAAA\xffBBBB\xffCCCC\n-----END PRIVATE KEY-----\nok";
        let out = r.redact_bytes(log);
        assert_eq!(out.as_ref(), b"[REDACTED:private_key]\nok");
    }

    #[test]
    fn a_custom_rule_shorter_than_a_known_secret_applies_to_bytes() {
        let r = Redactor::with_extra([(Regex::new("SECRET").unwrap(), "[MASK]")]);
        assert_eq!(
            r.redact_bytes(b"\xffSECRET\xff").as_ref(),
            b"\xff[MASK]\xff"
        );
        let plain = Redactor::default();
        assert!(matches!(
            plain.redact_bytes(b"\xffSECRET\xff"),
            Cow::Borrowed(_)
        ));
    }

    #[test]
    fn a_known_secret_escaped_inside_json_is_masked() {
        let secrets = [
            "alpha123\nbeta4567",
            "q-sec-77\"q-tail-88",
            "b-sec-55\\b-tail-66",
        ];
        let r = Redactor::with_known_secrets(secrets).unwrap();
        for secret in secrets {
            let rendered = serde_json::json!({ "text": format!("token {secret} end") }).to_string();
            let out = r.redact(&rendered);
            for fragment in [
                "alpha123",
                "beta4567",
                "q-sec-77",
                "q-tail-88",
                "b-sec-55",
                "b-tail-66",
            ] {
                assert!(!out.contains(fragment), "{out}");
            }
            let decoded: serde_json::Value = serde_json::from_str(&out).unwrap();
            assert_eq!(decoded["text"], "token [REDACTED:known_secret] end");
        }
    }
}
