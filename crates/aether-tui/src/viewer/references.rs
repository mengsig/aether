//! Local transcript references; filesystem access happens only during resolution.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};

const MAX_REFERENCES: usize = 256;
const MAX_REFERENCE_BYTES: usize = 16 * 1024;
const MAX_PARSE_BYTES: usize = 256 * 1024;
const MAX_LOOKAHEAD_BYTES: usize = 256 * 1024;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Reference {
    pub target: String,
    pub label: String,
}

/// Extract local links without probing the filesystem, in transcript order.
pub fn parse(text: &str) -> Vec<Reference> {
    let mut budget = MAX_LOOKAHEAD_BYTES;
    parse_with_budget(text, &mut budget)
}

fn parse_with_budget(text: &str, budget: &mut usize) -> Vec<Reference> {
    let text = prefix(text, MAX_PARSE_BYTES);
    let mut references = Vec::new();
    let mut seen = HashSet::new();
    let mut offset = 0;
    while offset < text.len() && references.len() < MAX_REFERENCES && *budget > 0 {
        debug_assert!(text.is_char_boundary(offset));
        let rest = &text[offset..];
        let (consumed, reference) = osc_link(rest, budget)
            .or_else(|| markdown_link(rest, budget))
            .unwrap_or_else(|| bare_reference(rest));
        debug_assert!(consumed > 0 && consumed <= rest.len());
        offset += consumed;
        if let Some(reference) = reference
            && seen.insert(reference.clone())
        {
            references.push(reference);
        }
    }
    debug_assert!(references.len() <= MAX_REFERENCES);
    debug_assert_eq!(references.len(), seen.len());
    references
}

fn reference(target: &str, label: &str) -> Option<Reference> {
    if target.len() > MAX_REFERENCE_BYTES
        || label.is_empty()
        || label.len() > MAX_REFERENCE_BYTES
        || label.chars().any(char::is_control)
        || local_path(target).is_err()
    {
        return None;
    }
    debug_assert!(!target.is_empty());
    debug_assert!(!label.contains('\0'));
    Some(Reference {
        target: target.to_owned(),
        label: label.to_owned(),
    })
}

fn markdown_link(text: &str, budget: &mut usize) -> Option<(usize, Option<Reference>)> {
    let text = text.strip_prefix('[')?;
    let (label_end, _) = scan(text, budget).find(|(_, c)| matches!(*c, '[' | ']' | '\n'))?;
    if !text[label_end..].starts_with("](") {
        return None;
    }
    let target_start = label_end + 2;
    let tail = &text[target_start..];
    let mut depth = 1_usize;
    let mut escaped = false;
    for (offset, character) in scan(tail, budget) {
        if escaped {
            escaped = false;
            continue;
        }
        match character {
            '\\' => escaped = true,
            '(' => depth += 1,
            ')' => depth -= 1,
            _ => {}
        }
        if depth != 0 {
            continue;
        }
        let target = markdown_target(&tail[..offset]);
        let consumed = 1 + target_start + offset + 1;
        debug_assert!(text.is_char_boundary(target_start + offset));
        debug_assert!(consumed <= text.len() + 1);
        return Some((consumed, reference(target, &text[..label_end])));
    }
    None
}

fn markdown_target(text: &str) -> &str {
    let text = text.trim();
    if let Some(inner) = text.strip_prefix('<')
        && let Some((target, _title)) = inner.split_once('>')
    {
        return target;
    }
    for (separator, quote) in [(" \"", '"'), (" '", '\'')] {
        if let Some((target, title)) = text.rsplit_once(separator)
            && title.ends_with(quote)
        {
            return target;
        }
    }
    text
}

fn prefix(text: &str, limit: usize) -> &str {
    let mut end = text.len().min(limit);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    debug_assert!(end <= limit);
    debug_assert!(text.is_char_boundary(end));
    &text[..end]
}

fn scan<'a>(text: &'a str, budget: &'a mut usize) -> impl Iterator<Item = (usize, char)> + 'a {
    let text = prefix(text, (*budget).min(MAX_REFERENCE_BYTES));
    text.char_indices().inspect(|(_, character)| {
        debug_assert!(*budget >= character.len_utf8());
        *budget -= character.len_utf8();
    })
}

fn osc_link(text: &str, budget: &mut usize) -> Option<(usize, Option<Reference>)> {
    let header = text.strip_prefix("\x1b]8;")?;
    let (header_end, terminator_length) = osc_terminator(header, budget)?;
    let target = header[..header_end].split_once(';')?.1;
    let label_start = 4 + header_end + terminator_length;
    let label_tail = &text[label_start..];
    let (label_end, _) = scan(label_tail, budget).find(|(index, character)| {
        *character == '\x1b' && label_tail[*index..].starts_with("\x1b]8;;")
    })?;
    let closing = &label_tail[label_end + 5..];
    let closing_length = if closing.starts_with('\x07') {
        1
    } else if closing.starts_with("\x1b\\") {
        2
    } else {
        return None;
    };
    let consumed = label_start + label_end + 5 + closing_length;
    debug_assert!(consumed <= text.len());
    debug_assert!(text.is_char_boundary(consumed));
    Some((consumed, reference(target, &label_tail[..label_end])))
}

fn osc_terminator(text: &str, budget: &mut usize) -> Option<(usize, usize)> {
    scan(text, budget).find_map(|(index, character)| {
        if character == '\x07' {
            Some((index, 1))
        } else if text[index..].starts_with("\x1b\\") {
            Some((index, 2))
        } else {
            None
        }
    })
}

fn bare_reference(text: &str) -> (usize, Option<Reference>) {
    let end = text.find(is_separator).unwrap_or(text.len());
    if end == 0 {
        return (text.chars().next().map_or(0, char::len_utf8), None);
    }
    let token = text[..end]
        .trim_start_matches('(')
        .trim_end_matches([')', '.', ';', '!', ':']);
    let candidate = token.contains('/') || known_extension(token);
    debug_assert!(end > 0 && text.is_char_boundary(end));
    debug_assert!(token.len() <= end);
    (end, candidate.then(|| reference(token, token)).flatten())
}

fn is_separator(character: char) -> bool {
    character.is_whitespace()
        || character.is_control()
        || matches!(character, '[' | ']' | '<' | '>' | '"' | '\'' | '`' | ',')
}

fn known_extension(target: &str) -> bool {
    const EXTENSIONS: &[&str] = &[
        "png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "ico", "avif", "tif", "tiff", "txt",
        "md", "markdown", "rst", "log", "csv", "tsv", "json", "jsonl", "yaml", "yml", "toml",
        "xml", "ini", "conf", "rs", "py", "js", "jsx", "ts", "tsx", "c", "h", "cpp", "hpp", "cc",
        "go", "rb", "java", "kt", "swift", "sh", "bash", "zsh", "fish", "sql", "css", "scss",
        "sass", "vue", "svelte", "r", "lua", "ex", "exs", "erl", "hs", "zig", "pdf", "html", "htm",
    ];
    Path::new(target)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            EXTENSIONS
                .iter()
                .any(|known| extension.eq_ignore_ascii_case(known))
        })
}

/// Resolve symlinks and require a regular file; the caller approves external paths.
pub fn resolve(target: &str, workspace: &Path) -> Result<PathBuf> {
    let path = local_path(target)?;
    let candidate = if path.is_absolute() {
        path
    } else {
        workspace.join(path)
    };
    let canonical = candidate
        .canonicalize()
        .with_context(|| format!("resolve local file {}", candidate.display()))?;
    let metadata = canonical
        .metadata()
        .with_context(|| format!("inspect local file {}", canonical.display()))?;
    ensure!(metadata.is_file(), "reference is not a regular file");
    debug_assert!(canonical.is_absolute());
    debug_assert!(!canonical.as_os_str().is_empty());
    Ok(canonical)
}

fn local_path(target: &str) -> Result<PathBuf> {
    ensure!(!target.is_empty(), "empty file reference");
    ensure!(
        !target.chars().any(char::is_control),
        "control character in file reference"
    );
    ensure!(
        !target.contains(['?', '#']),
        "queries and fragments are not file references"
    );
    let path = if let Some((scheme, rest)) = target.split_once(':') {
        ensure!(scheme.eq_ignore_ascii_case("file"), "nonlocal URI scheme");
        let rest = rest.strip_prefix("//").context("expected file:// URI")?;
        let slash = rest.find('/').context("file URI has no absolute path")?;
        let authority = &rest[..slash];
        ensure!(
            authority.is_empty() || authority.eq_ignore_ascii_case("localhost"),
            "remote file URI authority"
        );
        percent_decode(&rest[slash..])?
    } else {
        target.to_owned()
    };
    ensure!(!path.starts_with("//"), "network paths are not local files");
    ensure!(
        !path.chars().any(char::is_control),
        "decoded control character in file reference"
    );
    debug_assert!(!path.is_empty());
    debug_assert!(!path.contains('\0'));
    Ok(PathBuf::from(path))
}

fn percent_decode(text: &str) -> Result<String> {
    let mut decoded = Vec::with_capacity(text.len());
    let mut bytes = text.as_bytes();
    while let Some((&byte, tail)) = bytes.split_first() {
        if byte != b'%' {
            decoded.push(byte);
            bytes = tail;
            continue;
        }
        let digits = tail.get(..2).context("incomplete file URI escape")?;
        ensure!(
            digits.iter().all(u8::is_ascii_hexdigit),
            "invalid file URI escape"
        );
        let digits = std::str::from_utf8(digits).context("invalid file URI escape")?;
        decoded.push(u8::from_str_radix(digits, 16).context("invalid file URI escape")?);
        bytes = &tail[2..];
    }
    debug_assert!(decoded.len() <= text.len());
    debug_assert!(bytes.is_empty());
    String::from_utf8(decoded).context("file URI path is not UTF-8")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_markdown_spaces_unicode_and_balanced_parentheses() {
        let references =
            parse("[résumé](<docs/my résumé.pdf>) [source](src/a(b).rs) [notes](notes today.txt)");
        assert_eq!(references.len(), 3);
        assert_eq!(references[0].target, "docs/my résumé.pdf");
        assert_eq!(references[0].label, "résumé");
        assert_eq!(references[1].target, "src/a(b).rs");
        assert_eq!(references[2].target, "notes today.txt");
    }

    #[test]
    fn markdown_titles_are_not_part_of_the_target() {
        let references = parse("[source](src/a.rs \"title\") [notes](<my notes.txt> 'title')");
        assert_eq!(references.len(), 2);
        assert_eq!(references[0].target, "src/a.rs");
        assert_eq!(references[1].target, "my notes.txt");
    }

    #[test]
    fn extracts_bare_paths_and_known_filenames_without_filesystem_access() {
        let references = parse("See `src/new.rs`, (report.PDF). ../图.png /tmp/output README.md");
        let targets: Vec<_> = references
            .iter()
            .map(|reference| reference.target.as_str())
            .collect();
        assert_eq!(
            targets,
            [
                "src/new.rs",
                "report.PDF",
                "../图.png",
                "/tmp/output",
                "README.md"
            ]
        );
        assert!(
            references
                .iter()
                .all(|reference| reference.label == reference.target)
        );
        assert!(parse("ordinary words example.unknown").is_empty());
    }

    #[test]
    fn remote_links_and_their_labels_never_become_local_references() {
        let text = "https://example.com/a.rs http://example.com/x.png [fake.rs](https://example.com/a) \
                    [remote](file://server/tmp/a.rs) //server/share/a.rs ftp://example.com/a.rs";
        assert!(parse(text).is_empty());
        assert!(
            parse("[bad](file:///tmp/a.rs?download=1) [bad](file:///tmp/a.rs#page)").is_empty()
        );
    }

    #[test]
    fn deduplicates_target_label_pairs_and_caps_output() {
        assert_eq!(parse("a.rs a.rs [a.rs](a.rs)").len(), 1);
        assert_eq!(parse("[first](a.rs) [second](a.rs)").len(), 2);
        let text = (0..300)
            .map(|index| format!("{index}.rs "))
            .collect::<String>();
        let references = parse(&text);
        assert_eq!(references.len(), MAX_REFERENCES);
        assert_eq!(references.last().unwrap().target, "255.rs");
    }

    #[test]
    fn extracts_osc8_links_with_both_terminal_delimiters() {
        for end in ["\x07", "\x1b\\"] {
            let text = format!("\x1b]8;id=1;file:///tmp/a%20b.rs{end}source\x1b]8;;{end}");
            let references = parse(&text);
            assert_eq!(references.len(), 1);
            assert_eq!(references[0].target, "file:///tmp/a%20b.rs");
            assert_eq!(references[0].label, "source");
            let remote = format!("\x1b]8;;https://example.com/a.rs{end}fake.rs\x1b]8;;{end}");
            assert!(parse(&remote).is_empty());
        }
    }

    #[test]
    fn malformed_and_oversized_references_are_safe() {
        for input in [
            "",
            "[",
            "[label](",
            "é[",
            "\x1b]8;",
            "[bad](file:///tmp/%GG.rs)",
        ] {
            assert!(parse(input).is_empty(), "{input:?}");
        }
        let long = format!("{}.rs", "a".repeat(MAX_REFERENCE_BYTES));
        assert!(parse(&long).is_empty());
        assert!(parse("[bad](file:///tmp/%FF.rs)").is_empty());
    }

    #[test]
    fn malformed_link_lookahead_has_one_shared_budget() {
        for fragment in ["[a](", "\x1b]8;"] {
            let text = fragment.repeat(MAX_PARSE_BYTES / fragment.len());
            let mut budget = MAX_LOOKAHEAD_BYTES;
            assert!(parse_with_budget(&text, &mut budget).is_empty());
            assert_eq!(budget, 0, "unterminated links must exhaust shared credit");
        }
        let text = "[a](".repeat(1024);
        let mut budget = 64;
        assert!(parse_with_budget(&text, &mut budget).is_empty());
        assert_eq!(budget, 0);
    }

    #[test]
    fn parse_input_limit_is_utf8_safe_and_keeps_ordinary_link_capacity() {
        let text = (0..MAX_REFERENCES)
            .map(|index| format!("[file {index}](file-{index}.txt) "))
            .collect::<String>();
        assert_eq!(parse(&text).len(), MAX_REFERENCES);
        let text = format!("{}é hidden.txt", " ".repeat(MAX_PARSE_BYTES - 1));
        assert!(parse(&text).is_empty());
        assert_eq!(prefix(&text, MAX_PARSE_BYTES).len(), MAX_PARSE_BYTES - 1);
    }

    #[test]
    fn resolves_regular_files_relative_absolute_and_file_uris() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("résumé + space.txt");
        std::fs::write(&file, "text").unwrap();
        let expected = file.canonicalize().unwrap();
        assert_eq!(
            resolve("résumé + space.txt", directory.path()).unwrap(),
            expected
        );
        assert_eq!(
            resolve(file.to_str().unwrap(), directory.path()).unwrap(),
            expected
        );
        let escaped = file
            .to_str()
            .unwrap()
            .replace(' ', "%20")
            .replace('é', "%C3%A9");
        for authority in ["", "localhost", "LOCALHOST"] {
            let uri = format!("file://{authority}{escaped}");
            assert_eq!(resolve(&uri, directory.path()).unwrap(), expected);
        }
    }

    #[test]
    fn rejects_invalid_uris_missing_files_and_directories() {
        let directory = tempfile::tempdir().unwrap();
        for target in [
            "",
            "https://example.com/a.rs",
            "http://example.com/a.rs",
            "file://server/a.rs",
            "file://user@localhost/a.rs",
            "file://localhost:80/a.rs",
            "file:relative.rs",
            "file://",
            "//server/a.rs",
            "file:////server/a.rs",
            "file:///tmp/%",
            "file:///tmp/%2",
            "file:///tmp/%GG",
            "file:///tmp/%+1",
            "file:///tmp/%FF",
            "file:///tmp/%00",
            "file:///tmp/%0A",
            "file:///tmp/a?query",
            "file:///tmp/a#fragment",
            "a\0.rs",
        ] {
            assert!(resolve(target, directory.path()).is_err(), "{target:?}");
        }
        assert!(resolve("missing.rs", directory.path()).is_err());
        assert!(resolve(".", directory.path()).is_err());
    }

    #[test]
    fn traversal_resolves_outside_workspace_for_caller_approval() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = directory.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let file = directory.path().join("outside.txt");
        std::fs::write(&file, "outside").unwrap();
        let resolved = resolve("../outside.txt", &workspace).unwrap();
        assert_eq!(resolved, file.canonicalize().unwrap());
        assert!(!resolved.starts_with(&workspace));
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_canonicalized_before_caller_approval() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = directory.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let file = directory.path().join("outside.txt");
        std::fs::write(&file, "outside").unwrap();
        std::os::unix::fs::symlink(&file, workspace.join("linked.txt")).unwrap();
        let resolved = resolve("linked.txt", &workspace).unwrap();
        assert_eq!(resolved, file.canonicalize().unwrap());
        assert!(!resolved.starts_with(&workspace));
        std::os::unix::fs::symlink("absent.txt", workspace.join("broken.txt")).unwrap();
        assert!(resolve("broken.txt", &workspace).is_err());
    }
}
