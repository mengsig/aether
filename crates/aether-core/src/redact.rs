//! Masking recognizable secrets before text is persisted or shown.
//!
//! One rule set, so every path that writes provider or tool text to a durable
//! record - events, invocation rows, artifacts, logs - masks the same shapes.

use std::borrow::Cow;
use std::sync::LazyLock;

use regex::Regex;

/// Replaces recognizable secrets before output is persisted or shown.
#[derive(Clone)]
pub struct Redactor {
    patterns: Vec<(Regex, &'static str)>,
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
            "[REDACTED:private_key]",
        ),
        // Truncated captures: a tail that starts inside a key, or a key whose end was cut.
        (
            r"\A[A-Za-z0-9+/=\r\n ]*-----END [A-Z ]*PRIVATE KEY-----",
            "[REDACTED:private_key]",
        ),
        (
            r"-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*\z",
            "[REDACTED:private_key]",
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

impl Default for Redactor {
    fn default() -> Self {
        Self {
            patterns: DEFAULT_PATTERNS.clone(),
        }
    }
}

impl Redactor {
    /// A redactor with the default rules plus extra `(regex, replacement)` pairs.
    pub fn with_extra(extra: impl IntoIterator<Item = (Regex, &'static str)>) -> Self {
        let mut r = Self::default();
        r.patterns.extend(extra);
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
        let alternation = known
            .iter()
            .map(|value| regex::escape(value))
            .collect::<Vec<_>>()
            .join("|");
        let rule = Regex::new(&alternation).map_err(|_| KnownSecretsError {
            values: known.len(),
        })?;
        // First, before a shape rule rewrites part of a value and hides the rest from it.
        redactor
            .patterns
            .insert(0, (rule, "[REDACTED:known_secret]"));
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
}

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
        Redactor { patterns }
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
}
