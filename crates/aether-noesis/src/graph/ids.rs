//! Content-derived ids. A proposition's id is a hash of its normalized
//! statement and a chain's of its premise set and conclusion, so the same
//! content always gets the same id, whatever the time or the order it came in.

use sha2::{Digest, Sha256};

/// Hex digits of the SHA-256 kept in an id: 96 bits, ample for one graph.
const ID_HEX: usize = 24;

/// SHA-256 of `bytes` as 64 lowercase hex digits.
pub fn fingerprint(bytes: &[u8]) -> String {
    let digest = hex::encode(Sha256::digest(bytes));
    debug_assert_eq!(digest.len(), 64);
    digest
}

/// The comparable form of a statement or name: lowercase words separated by
/// one space, without a trailing full stop.
pub fn normalize(text: &str) -> String {
    let words: Vec<String> = text.split_whitespace().map(str::to_lowercase).collect();
    let mut joined = words.join(" ");
    while joined.ends_with('.') {
        joined.pop();
    }
    joined.trim_end().to_string()
}

fn prefixed(prefix: &str, content: &str) -> String {
    let digest = fingerprint(content.as_bytes());
    format!("{prefix}{}", &digest[..ID_HEX])
}

/// `p_…` from the normalized statement.
pub fn proposition_id(statement: &str) -> String {
    let normalized = normalize(statement);
    assert!(
        !normalized.is_empty(),
        "a proposition needs a non-empty statement"
    );
    prefixed("p_", &normalized)
}

/// `c_…` from the premise set (in any order) and the conclusion.
pub fn chain_id<'a>(premises: impl IntoIterator<Item = &'a String>, conclusion: &str) -> String {
    let mut sorted: Vec<&str> = premises.into_iter().map(String::as_str).collect();
    sorted.sort_unstable();
    sorted.dedup();
    assert!(!sorted.is_empty(), "a chain needs at least one premise");
    assert!(
        conclusion.starts_with("p_"),
        "a chain concludes a proposition"
    );
    prefixed("c_", &format!("{}=>{conclusion}", sorted.join(",")))
}

/// `s_…` from the source's text, so importing the same text again finds it.
pub fn source_id(text: &str) -> String {
    prefixed("s_", text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_depend_on_normalized_content_only() {
        let a = proposition_id("Water boils at 100 C at sea level.");
        let b = proposition_id("  water   BOILS at 100 c at sea level ");
        assert_eq!(a, b);
        assert!(a.starts_with("p_") && a.len() == 2 + ID_HEX);
        assert_ne!(a, proposition_id("Water boils at 90 C at altitude."));
    }

    #[test]
    fn chain_ids_ignore_premise_order_and_repeats() {
        let (x, y) = ("p_x".to_string(), "p_y".to_string());
        let one = chain_id([&x, &y], "p_z");
        let two = chain_id([&y, &x, &y], "p_z");
        assert_eq!(one, two);
        assert!(one.starts_with("c_"));
        assert_ne!(one, chain_id([&x], "p_z"));
        assert_ne!(one, chain_id([&x, &y], "p_w"));
    }

    #[test]
    fn fingerprints_are_sha256_hex() {
        let print = fingerprint(b"abc");
        assert_eq!(
            print,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(source_id("abc"), format!("s_{}", &print[..ID_HEX]));
    }
}
