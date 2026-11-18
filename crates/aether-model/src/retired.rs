//! Retired built-in model ids and aliases, with explicit replacement guidance.

const CLAUDE: &[&str] = &["anthropic", "claude-code"];
const GPT: &[&str] = &["codex", "openai-codex", "openai", "openai-responses"];
const GPT_API: &[&str] = &["openai", "openai-responses"];

struct RetiredModel {
    providers: &'static [&'static str],
    ids: &'static [&'static str],
    successor: &'static str,
}

const RETIRED: &[RetiredModel] = &[
    RetiredModel {
        providers: CLAUDE,
        ids: &["claude-opus-5", "opus-5"],
        successor: "claude-opus-5-5",
    },
    RetiredModel {
        providers: &["anthropic"],
        ids: &["claude-opus-4-8", "claude-opus-4-7", "claude-opus-4-6"],
        successor: "claude-opus-5-5",
    },
    RetiredModel {
        providers: CLAUDE,
        ids: &["claude-sonnet-5", "sonnet-5"],
        successor: "claude-sonnet-5-5",
    },
    RetiredModel {
        providers: &["anthropic"],
        ids: &["claude-sonnet-4-6"],
        successor: "claude-sonnet-5-5",
    },
    RetiredModel {
        providers: CLAUDE,
        ids: &["claude-haiku-4-5", "haiku-4-5", "claude-haiku-4-5-20251001"],
        successor: "claude-haiku-5-5",
    },
    RetiredModel {
        providers: &["anthropic"],
        ids: &["claude-fable-5"],
        successor: "claude-fable-5-1",
    },
    RetiredModel {
        providers: GPT,
        ids: &["gpt-6-sol", "gpt-5.6-sol", "gpt-5.6", "gpt-5.5"],
        successor: "gpt-6.2-sol",
    },
    RetiredModel {
        providers: GPT_API,
        ids: &["gpt-5.4"],
        successor: "gpt-6.2-sol",
    },
    RetiredModel {
        providers: GPT,
        ids: &["gpt-5.6-luna", "gpt-5.3-codex-spark", "spark"],
        successor: "gpt-6-luna",
    },
    RetiredModel {
        providers: GPT_API,
        ids: &["gpt-5.4-mini"],
        successor: "gpt-6-luna",
    },
];

pub(crate) fn entries() -> impl Iterator<Item = (&'static str, &'static str, &'static str)> {
    RETIRED.iter().flat_map(|entry| {
        entry.providers.iter().flat_map(move |provider| {
            entry
                .ids
                .iter()
                .map(move |id| (*provider, *id, entry.successor))
        })
    })
}

/// A superseded model still resolves by id but no longer represents a tier.
pub(crate) fn superseded_by(provider: &str, id: &str) -> Option<&'static str> {
    match id {
        "claude-fable-5-1" if CLAUDE.contains(&provider) => Some("claude-fable-5-2"),
        "gpt-6-astra" if GPT.contains(&provider) => Some("gpt-6.2-astra"),
        "gpt-6.1-sol" if GPT.contains(&provider) => Some("gpt-6.2-sol"),
        _ => None,
    }
}

/// Replacement guidance for a retired id on its built-in provider.
pub fn successor(provider: &str, id: &str) -> Option<&'static str> {
    RETIRED
        .iter()
        .find(|entry| entry.providers.contains(&provider) && entry.ids.contains(&id))
        .map(|entry| entry.successor)
}

#[cfg(test)]
mod tests;
