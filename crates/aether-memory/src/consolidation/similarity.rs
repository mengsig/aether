//! When two memories restate one fact, and when they only look alike.
//!
//! Similarity is lexical and deterministic: half the cosine of the texts'
//! character trigrams, half the Jaccard of their stemmed words. Alone it
//! cannot tell a restatement from a correction, so a pair that shows a sign
//! of one (an edit of the same sentence, a negation only one side has,
//! different numbers, a change being reported) is never put in one cluster.

use std::collections::{BTreeMap, BTreeSet};

use crate::index::carries_meaning;

/// Similarity a pair must reach to link two memories into one cluster.
pub(crate) const LINK: f32 = 0.40;
/// Similarity every pair in a cluster must reach: below it two memories are
/// about different things, however many links join them.
pub(crate) const RELATED: f32 = 0.25;
/// Share of whitespace tokens past which two texts are one sentence edited.
/// Admission keeps such edits apart on purpose: a changed word may be a
/// changed requirement.
const EDITED: f32 = 0.5;

const NEGATIONS: [&str; 9] = [
    "not", "never", "no", "cannot", "nor", "without", "none", "nothing", "nobody",
];

/// Words saying a fact changed. A memory that reports a change replaces
/// another; that is supersession, which admission decides, not consolidation.
const CHANGE_MARKERS: [&str; 16] = [
    "removed",
    "dropped",
    "replaced",
    "deprecated",
    "switched",
    "migrated",
    "retired",
    "obsolete",
    "renamed",
    "anymore",
    "no longer",
    "used to",
    "moved to",
    "instead of",
    "was changed",
    "now uses",
];

/// What one memory's text contributes to every comparison, computed once.
#[derive(Debug, Clone)]
pub(crate) struct Features {
    trigrams: BTreeMap<[char; 3], f32>,
    stems: BTreeSet<String>,
    tokens: BTreeSet<String>,
    negated: bool,
    reports_change: bool,
    numbers: Vec<String>,
}

impl Features {
    pub(crate) fn of(text: &str) -> Self {
        let lower = text.to_lowercase();
        let words: Vec<&str> = lower
            .split(|c: char| !(c.is_alphanumeric() || c == '\''))
            .filter(|w| !w.is_empty())
            .collect();
        let meaningful: Vec<&str> = words
            .iter()
            .copied()
            .filter(|w| carries_meaning(w))
            .collect();
        let joined = format!(" {} ", words.join(" "));
        let features = Self {
            trigrams: trigrams(&meaningful),
            stems: meaningful.iter().map(|w| stem(w)).collect(),
            tokens: text.split_whitespace().map(str::to_owned).collect(),
            negated: words
                .iter()
                .any(|w| NEGATIONS.contains(w) || w.ends_with("n't")),
            reports_change: CHANGE_MARKERS
                .iter()
                .any(|m| joined.contains(&format!(" {m} "))),
            numbers: digit_runs(text),
        };
        debug_assert!(features.stems.len() <= meaningful.len());
        debug_assert!(features.trigrams.values().all(|v| v.is_finite()));
        features
    }

    pub(crate) fn stems(&self) -> impl Iterator<Item = &str> {
        self.stems.iter().map(String::as_str)
    }
}

/// Similarity in 0..=1. Two texts that share no meaningful word score 0:
/// trigrams alone link "reindex" to "index", not a fact to its restatement.
pub(crate) fn similarity(a: &Features, b: &Features) -> f32 {
    let shared = a.stems.intersection(&b.stems).count();
    if shared == 0 {
        return 0.0;
    }
    let union = a.stems.union(&b.stems).count();
    let jaccard = shared as f32 / union as f32;
    let score = 0.5 * cosine(&a.trigrams, &b.trigrams) + 0.5 * jaccard;
    debug_assert!((0.0..=1.0 + 1e-5).contains(&score), "similarity {score}");
    score.clamp(0.0, 1.0)
}

/// Why `a` and `b` must not be merged however alike they read, if they must not.
pub(crate) fn conflict(a: &Features, b: &Features) -> Option<&'static str> {
    let shared = a.tokens.intersection(&b.tokens).count();
    let union = a.tokens.union(&b.tokens).count().max(1);
    if shared as f32 / union as f32 >= EDITED {
        return Some("one sentence edited: the change may be a changed requirement");
    }
    if a.negated != b.negated {
        return Some("only one of them is negated");
    }
    if !a.numbers.is_empty() && !b.numbers.is_empty() && a.numbers != b.numbers {
        return Some("they state different numbers");
    }
    if a.reports_change || b.reports_change {
        return Some("one reports a change, so it replaces rather than restates");
    }
    None
}

/// L2-normalized counts of each word's padded trigrams.
fn trigrams(words: &[&str]) -> BTreeMap<[char; 3], f32> {
    let mut counts: BTreeMap<[char; 3], f32> = BTreeMap::new();
    for word in words {
        let padded: Vec<char> = format!(" {word} ").chars().collect();
        for window in padded.windows(3) {
            *counts.entry([window[0], window[1], window[2]]).or_default() += 1.0;
        }
    }
    let norm = counts.values().map(|v| v * v).sum::<f32>().sqrt();
    if norm > 0.0 {
        counts.values_mut().for_each(|v| *v /= norm);
    }
    counts
}

fn cosine(a: &BTreeMap<[char; 3], f32>, b: &BTreeMap<[char; 3], f32>) -> f32 {
    let (small, large) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    small
        .iter()
        .filter_map(|(gram, x)| large.get(gram).map(|y| x * y))
        .sum::<f32>()
        .clamp(0.0, 1.0)
}

/// A crude suffix strip, so "push" meets "pushing" and "test" meets "tests".
fn stem(word: &str) -> String {
    for suffix in ["ing", "ed", "es", "s"] {
        if let Some(root) = word.strip_suffix(suffix)
            && root.chars().count() >= 3
        {
            return root.to_owned();
        }
    }
    word.to_owned()
}

fn digit_runs(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_ascii_digit())
        .filter(|run| !run.is_empty())
        .map(str::to_owned)
        .collect()
}
