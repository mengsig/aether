//! Masking recognizable secrets before text is persisted or shown.
//!
//! One rule set, so every path that writes provider or tool text to a durable
//! record - events, invocation rows, artifacts, logs - masks the same shapes.

use std::borrow::Cow;
use std::sync::LazyLock;

use regex::Regex;

/// Replaces recognizable secrets before output is persisted or shown.
#[derive(Debug, Clone)]
pub struct Redactor {
    patterns: Vec<(Regex, &'static str)>,
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn clean_text_is_borrowed() {
        let r = Redactor::default();
        assert!(matches!(r.redact("nothing secret here"), Cow::Borrowed(_)));
    }
}
