//! Compact human formatting for numbers the UI shows constantly.

use std::fmt::Display;

use aether_core::time::Timestamp;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// `123`, `12.3k`, `1.2M`, `1.2B`.
pub fn tokens(n: u64) -> String {
    if n < 1_000 {
        n.to_string()
    } else if n < 999_950 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else if n < 999_950_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else {
        format!("{:.1}B", n as f64 / 1_000_000_000.0)
    }
}

/// A payload's size the way a person says it: `812 B`, `84 KB`, `2.1 MB`.
/// Distinct from [`tokens`], which counts things rather than measuring them.
pub fn bytes(n: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    match n {
        0..KB => format!("{n} B"),
        // Rounded first: `{:.0}` alone prints 1_048_575 as `1024 KB`, a
        // number this band cannot reach.
        KB..MB if (n as f64 / KB as f64).round() as u64 >= 1024 => {
            format!("{:.1} MB", n as f64 / MB as f64)
        }
        KB..MB => format!("{:.0} KB", n as f64 / KB as f64),
        MB..GB if (n as f64 / MB as f64 * 10.0).round() >= 10_240.0 => {
            format!("{:.1} GB", n as f64 / GB as f64)
        }
        MB..GB => format!("{:.1} MB", n as f64 / MB as f64),
        _ => format!("{:.1} GB", n as f64 / GB as f64),
    }
}

/// The daemon's disk hold, shared by the persistent warning and plain output.
pub fn low_disk(disk: &aether_core::disk::LowDisk) -> String {
    const GIB: f64 = (1u64 << 30) as f64;
    format!(
        "Disk low: new turns held - {:.1} GiB free / {:.1} GiB floor",
        disk.available_bytes as f64 / GIB,
        disk.floor_bytes as f64 / GIB,
    )
}

/// A usage's cost, in the phrasing every surface shares: a complete total is
/// a number, a total that folded in an unpriced invocation is a floor and
/// says so, and one with nothing priced at all is not a number.
pub fn cost(usage: &aether_core::model::Usage) -> String {
    let said = usage.cost_said(amount);
    if usage.cost_complete || usage.cost_usd > 0.0 {
        cost_with_basis(said, usage.cost_basis)
    } else {
        said
    }
}

pub fn cost_with_basis(amount: String, basis: aether_core::model::CostBasis) -> String {
    match basis.label() {
        Some(label) => format!("{amount} {label}"),
        None => amount,
    }
}

/// The API-equivalent estimate beside that cost, where the engine billed
/// nothing but the registry knows what the API would have charged. `None`
/// where there is no estimate to say.
pub fn cost_estimate(usage: &aether_core::model::Usage) -> Option<String> {
    usage
        .estimate_said(amount)
        .map(|amount| cost_with_basis(amount, usage.cost_basis))
}

/// Two decimals down to a cent (§9.2's `$0.41`), four below that, and a bare
/// `$0` at zero — four zeroes after the point read as a placeholder, not a
/// number. Spend too small to write in four is said to be small, not rounded
/// away to nothing.
pub(crate) fn amount(usd: f64) -> String {
    if usd <= 0.0 {
        return "$0".into();
    }
    if usd >= 0.01 {
        return format!("${usd:.2}");
    }
    if usd >= 0.000_05 {
        return format!("${usd:.4}");
    }
    "<$0.0001".into()
}

/// `340ms`, `1.2s`, `2m05s`, `1h03m`.
pub fn duration_ms(ms: u64) -> String {
    if ms < 1_000 {
        format!("{ms}ms")
    } else if ms < 59_950 {
        format!("{:.1}s", ms as f64 / 1_000.0)
    } else if ms < 60_000 {
        "1m00s".into()
    } else if ms < 3_600_000 {
        let m = ms / 60_000;
        let s = (ms % 60_000) / 1_000;
        format!("{m}m{s:02}s")
    } else {
        let h = ms / 3_600_000;
        let m = (ms % 3_600_000) / 60_000;
        format!("{h}h{m:02}m")
    }
}

/// Board and status-row elapsed (SPEC §2.1): `41s`, `2m 40s`, `1h 01m`.
pub fn elapsed_compact(ms: u64) -> String {
    let seconds = ms / 1_000;
    match seconds {
        0..60 => format!("{seconds}s"),
        60..3_600 => format!("{}m {:02}s", seconds / 60, seconds % 60),
        _ => format!("{}h {:02}m", seconds / 3_600, seconds % 3_600 / 60),
    }
}

/// A finished span, `<1s` when too short to count, else [`elapsed_compact`].
/// Live clocks keep `0s` so they do not jump from `<1s` to `1s`.
pub fn took(ms: u64) -> String {
    if ms < 1_000 {
        return "<1s".into();
    }
    elapsed_compact(ms)
}

/// Continuous state age, not model runtime. Unknown history stays unknown.
pub fn status_age(since: Option<Timestamp>, now: Timestamp) -> String {
    let Some(since) = since else {
        return "unknown".into();
    };
    let seconds = (now - since).num_seconds().max(0) as u64;
    match seconds {
        0..60 => format!("{seconds}s"),
        60..3_600 => format!("{}m{:02}s", seconds / 60, seconds % 60),
        3_600..86_400 => format!("{}h{:02}m", seconds / 3_600, seconds % 3_600 / 60),
        _ => format!("{}d{:02}h", seconds / 86_400, seconds % 86_400 / 3_600),
    }
}

/// `now`, `3s`, `2m`, `1h`, `3d`. Short by design; the timeline is dense.
pub fn relative(at: Timestamp, now: Timestamp) -> String {
    let secs = (now - at).num_seconds();
    if secs < 2 {
        "now".into()
    } else if secs < 60 {
        format!("{secs}s")
    } else if secs < 3_600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        format!("{}h", secs / 3_600)
    } else {
        format!("{}d", secs / 86_400)
    }
}

/// A path as people write it: `~` for home, and only the last two parts
/// when it gets long.
pub fn short_path(path: &std::path::Path) -> String {
    short_path_with_marker(path, '…')
}

pub(crate) fn short_path_with_marker(path: &std::path::Path, marker: char) -> String {
    let text =
        match dirs::home_dir().and_then(|h| path.strip_prefix(&h).ok().map(|p| p.to_path_buf())) {
            Some(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
            Some(rest) => format!("~/{}", rest.display()),
            None => path.display().to_string(),
        };
    if text.chars().count() <= 32 {
        return text;
    }
    let parts: Vec<&str> = text.split('/').filter(|p| !p.is_empty()).collect();
    match parts.len() {
        0 | 1 => text,
        n => format!("{marker}/{}/{}", parts[n - 2], parts[n - 1]),
    }
}

/// Wall-clock time of day, local.
pub fn clock(at: Timestamp) -> String {
    at.with_timezone(&chrono::Local)
        .format("%H:%M:%S")
        .to_string()
}

/// The distinctive tail of a typed id, e.g. `agt_…7K9Q`.
pub fn short_id(id: &impl Display) -> String {
    short_id_with_marker(id, '…')
}

pub(crate) fn short_id_with_marker(id: &impl Display, marker: char) -> String {
    let s = id.to_string();
    match s.split_once('_') {
        Some((prefix, rest)) if rest.len() > 4 => {
            format!("{prefix}_{marker}{}", &rest[rest.len() - 4..])
        }
        _ => s,
    }
}

/// Cut to `max` display columns with an ellipsis, on a grapheme boundary. Columns
/// rather than characters: a wide glyph costs two cells, and a column that
/// counts them as one runs into its neighbour.
pub fn truncate(s: &str, max: usize) -> String {
    truncate_with_marker(s, max, '…')
}

pub(crate) fn truncate_with_marker(s: &str, max: usize, marker: char) -> String {
    if s.width() <= max {
        return s.to_owned();
    }
    let room = max.saturating_sub(1);
    let mut head = String::new();
    let mut used = 0usize;
    for g in s.graphemes(true) {
        let w = g.width();
        if used + w > room {
            break;
        }
        head.push_str(g);
        used += w;
    }
    if max > 0 {
        head.push(marker);
    }
    head
}

/// Cut to `max` columns by dropping the middle, so distinguishing ends such as
/// `-5-5` or `-07` survive: `claude-code/cl…sonnet-5-5`.
pub(crate) fn truncate_middle_with_marker(s: &str, max: usize, marker: char) -> String {
    if s.width() <= max {
        return s.to_owned();
    }
    let tail_room = max.saturating_sub(1) / 2;
    let mut tail = Vec::new();
    let mut used = 0usize;
    for g in s.graphemes(true).rev() {
        let w = g.width();
        if used + w > tail_room {
            break;
        }
        tail.push(g);
        used += w;
    }
    let mut out = truncate_with_marker(s, max - used, marker);
    out.extend(tail.into_iter().rev());
    out
}

/// First line only, truncated.
pub fn one_line(s: &str, max: usize) -> String {
    one_line_with_marker(s, max, '…')
}

pub(crate) fn one_line_with_marker(s: &str, max: usize, marker: char) -> String {
    truncate_with_marker(s.lines().next().unwrap_or(""), max, marker)
}

/// Does `haystack` contain `needle`, ignoring case? `needle` must already be
/// lowercase. Nothing is allocated: search runs this over every item of the
/// transcript and every event on the ring, on every keystroke.
pub fn contains_fold(haystack: &str, needle: &str) -> bool {
    find_fold(haystack, needle).is_some()
}

/// Byte offset of the first case-insensitive occurrence of `needle`, which
/// must already be lowercase.
pub fn find_fold(haystack: &str, needle: &str) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    let first = needle.chars().next()?;
    for (at, c) in haystack.char_indices() {
        if !c.to_lowercase().any(|l| l == first) {
            continue;
        }
        let mut folded = haystack[at..].chars().flat_map(char::to_lowercase);
        if needle.chars().all(|n| folded.next() == Some(n)) {
            return Some(at);
        }
    }
    None
}

/// A window of `text` around the first occurrence of `needle`, on one line,
/// so a search result shows the match rather than wherever the text started.
pub fn snippet(text: &str, needle: &str, max: usize) -> String {
    snippet_marked(text, &needle.to_lowercase(), max).0
}

/// The snippet plus where the match sits in it, as a character range, so the
/// overlay can show which words it matched. `needle` must be lowercase.
pub fn snippet_marked(text: &str, needle: &str, max: usize) -> (String, Option<(usize, usize)>) {
    snippet_marked_with_marker(text, needle, max, '…')
}

pub(crate) fn snippet_marked_with_marker(
    text: &str,
    needle: &str,
    max: usize,
    marker: char,
) -> (String, Option<(usize, usize)>) {
    let out = snippet_text(text, needle, max, marker);
    let at = find_fold(&out, needle).map(|byte| {
        let start = out[..byte].chars().count();
        (start, start + needle.chars().count())
    });
    (out, at)
}

fn snippet_text(text: &str, needle: &str, max: usize, marker: char) -> String {
    let Some(at) = find_fold(text, needle) else {
        return one_line_with_marker(text, max, marker);
    };
    // The line the match falls on, so surrounding paragraphs stay out.
    let start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let end = text[at..].find('\n').map_or(text.len(), |i| at + i);
    let line = text[start..end].trim();
    if line.chars().count() <= max {
        return line.to_owned();
    }
    // Keep a little context before the match, then fill to the budget.
    let in_line = text[start..at].chars().count();
    let lead = in_line.saturating_sub(max / 4);
    let mut out = String::new();
    if lead > 0 {
        out.push(marker);
    }
    out.extend(line.chars().skip(lead).take(max.saturating_sub(2)));
    out.push(marker);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncation_never_splits_a_grapheme_cluster() {
        let coder = "👨\u{200d}💻";
        assert_eq!(
            truncate_middle_with_marker(&format!("reviewer-{coder}"), 5, '…'),
            format!("re…{coder}")
        );
        assert_eq!(truncate(&format!("{coder}abc"), 3), format!("{coder}…"));
    }

    #[test]
    fn middle_truncation_keeps_both_ends_within_the_width() {
        let id = "claude-code/claude-sonnet-5-5";
        assert_eq!(truncate_middle_with_marker(id, 40, '…'), id);
        assert_eq!(
            truncate_middle_with_marker(id, 17, '…'),
            "claude-c…nnet-5-5"
        );
        assert_eq!(truncate_middle_with_marker(id, 17, '…').width(), 17);
        assert_eq!(
            truncate_middle_with_marker("日本語のモデル名", 9, '~'),
            "日本~ル名"
        );
        assert_eq!(truncate_middle_with_marker(id, 1, '…'), "…");
        assert_eq!(truncate_middle_with_marker(id, 0, '…'), "");
    }
    use chrono::Duration;

    #[test]
    fn elapsed_compact_matches_the_board_format() {
        assert_eq!(elapsed_compact(41_900), "41s");
        assert_eq!(elapsed_compact(160_000), "2m 40s");
        assert_eq!(elapsed_compact(3_660_000), "1h 01m");
    }

    #[test]
    fn a_finished_span_under_a_second_reads_as_less_than_one() {
        assert_eq!(took(0), "<1s");
        assert_eq!(took(999), "<1s");
        assert_eq!(took(1_000), "1s");
        assert_eq!(took(160_000), "2m 40s");
    }

    #[test]
    fn status_age_is_explicit_and_clamps_future_clock_skew() {
        let at: Timestamp = "2026-09-14T10:00:00Z".parse().unwrap();
        assert_eq!(status_age(None, at), "unknown");
        assert_eq!(status_age(Some(at), at), "0s");
        assert_eq!(status_age(Some(at + chrono::Duration::days(1)), at), "0s");
        assert_eq!(
            status_age(Some(at), at + chrono::Duration::seconds(4)),
            "4s"
        );
        assert_eq!(
            status_age(Some(at), at + chrono::Duration::minutes(4)),
            "4m00s"
        );
        assert_eq!(
            status_age(Some(at), at + chrono::Duration::hours(2)),
            "2h00m"
        );
        assert_eq!(
            status_age(Some(at), at + chrono::Duration::days(3)),
            "3d00h"
        );
    }

    #[test]
    fn a_snippet_shows_the_match_not_the_start() {
        let text = "## Plan\n\nFirst we look at the workspace and then we build it.";
        let out = snippet(text, "workspace", 40);
        assert!(
            out.contains("workspace"),
            "the match must be visible: {out}"
        );
        assert!(!out.contains("## Plan"), "unrelated lines stay out: {out}");
        assert!(out.chars().count() <= 41, "{out}");
        // No match: the head, as before.
        assert_eq!(snippet("hello there", "zzz", 40), "hello there");
        // Short line: kept whole.
        assert_eq!(snippet("run cargo build", "cargo", 40), "run cargo build");
    }

    #[test]
    fn token_and_cost_formats() {
        assert_eq!(tokens(999), "999");
        assert_eq!(tokens(12_345), "12.3k");
        assert_eq!(tokens(1_234_567), "1.2M");
        assert_eq!(cost(&priced(0.0)), "$0");
        assert_eq!(cost(&priced(0.000_003)), "<$0.0001");
        assert_eq!(cost(&priced(0.004_213)), "$0.0042");
        assert_eq!(cost(&priced(0.4213)), "$0.42");
        assert_eq!(cost(&priced(12.3456)), "$12.35");
    }

    #[test]
    fn rounded_values_promote_to_the_next_unit() {
        assert_eq!(tokens(999_949), "999.9k");
        assert_eq!(tokens(999_950), "1.0M");
        assert_eq!(tokens(999_949_999), "999.9M");
        assert_eq!(tokens(999_950_000), "1.0B");
        assert_eq!(duration_ms(59_949), "59.9s");
        assert_eq!(duration_ms(59_950), "1m00s");
        assert_eq!(duration_ms(60_000), "1m00s");
        assert_eq!(bytes(1_073_741_823), "1.0 GB");
        assert_eq!(bytes(1_073_689_395), "1023.9 MB");
        assert_eq!(bytes(1_073_689_396), "1.0 GB");
    }

    /// The dense form still says which of the three things it means.
    #[test]
    fn an_incomplete_cost_reads_as_a_floor() {
        let mut floor = priced(0.4213);
        floor.cost_complete = false;
        assert_eq!(cost(&floor), "≥ $0.42 (some cost unknown)");

        let mut nothing = priced(0.0);
        nothing.cost_complete = false;
        assert_eq!(cost(&nothing), aether_core::model::Usage::COST_UNKNOWN);
    }

    fn priced(usd: f64) -> aether_core::model::Usage {
        aether_core::model::Usage {
            cost_usd: usd,
            ..Default::default()
        }
    }

    #[test]
    fn durations() {
        assert_eq!(duration_ms(340), "340ms");
        assert_eq!(duration_ms(1_234), "1.2s");
        assert_eq!(duration_ms(125_000), "2m05s");
        assert_eq!(duration_ms(3_780_000), "1h03m");
    }

    #[test]
    fn relative_times() {
        let now = aether_core::time::now();
        assert_eq!(relative(now, now), "now");
        assert_eq!(relative(now - Duration::seconds(30), now), "30s");
        assert_eq!(relative(now - Duration::minutes(5), now), "5m");
        assert_eq!(relative(now - Duration::hours(2), now), "2h");
        assert_eq!(relative(now - Duration::days(3), now), "3d");
    }

    #[test]
    fn ids_and_truncation() {
        assert_eq!(short_id(&"agt_01ARZ3NDEKTSV4RRFFQ69G5FAV"), "agt_…5FAV");
        assert_eq!(truncate("hello world", 6), "hello…");
        assert_eq!(truncate("hi", 6), "hi");
        assert_eq!(one_line("first\nsecond", 10), "first");
    }
}

/// Where a typed id starts inside free text: `agt_` / `tsk_` and the 26
/// Crockford base32 characters a ULID prints as.
const ID_LEN: usize = 26;

fn is_crockford(c: char) -> bool {
    c.is_ascii_digit() || (c.is_ascii_uppercase() && !matches!(c, 'I' | 'L' | 'O' | 'U'))
}

/// Replace the ids the daemon wrote into free text with the names a person
/// uses. A daemon message says `Task tsk_01J8… completed` because that is
/// what it has; the client knows the objective, so it says that instead.
/// `name_of` returns `None` for an id it cannot place, and the id stays -
/// worse to invent a name than to show the id it really was.
///
/// Borrowed back unchanged when there is nothing to replace, which is the
/// common case: this runs over every system note and tool output on screen.
pub fn name_ids<'a>(
    text: &'a str,
    name_of: impl Fn(&str) -> Option<String>,
) -> std::borrow::Cow<'a, str> {
    let Some(first) = find_id(text, 0) else {
        return std::borrow::Cow::Borrowed(text);
    };
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    let mut next = Some(first);
    while let Some((start, end)) = next {
        out.push_str(&text[at..start]);
        match name_of(&text[start..end]) {
            Some(name) => out.push_str(&name),
            None => out.push_str(&text[start..end]),
        }
        at = end;
        next = find_id(text, at);
    }
    out.push_str(&text[at..]);
    std::borrow::Cow::Owned(out)
}

/// The next `<prefix>_<ulid>` at or after `from`, as a byte range.
fn find_id(text: &str, from: usize) -> Option<(usize, usize)> {
    let mut at = from;
    while at < text.len() {
        let rest = &text[at..];
        let hit = rest
            .find("agt_")
            .into_iter()
            .chain(rest.find("tsk_"))
            .min()?;
        let start = at + hit;
        let body = start + 4;
        let taken: usize = text[body..]
            .chars()
            .take(ID_LEN)
            .take_while(|c| is_crockford(*c))
            .count();
        // A run longer than an id is something else entirely, not an id with
        // a suffix, so it is left alone.
        let ends_clean = text[body + taken..]
            .chars()
            .next()
            .is_none_or(|c| !is_crockford(c));
        if taken == ID_LEN && ends_clean {
            return Some((start, body + ID_LEN));
        }
        at = body;
    }
    None
}

#[cfg(test)]
mod id_name_tests {
    use super::name_ids;

    fn known(id: &str) -> Option<String> {
        match id {
            "agt_01J8Z3F5KDQW9V2MRXB7NCTHYE" => Some("steward:api".into()),
            "tsk_01J8Z3F5KDQW9V2MRXB7NCTHYF" => Some("split the migration".into()),
            _ => None,
        }
    }

    #[test]
    fn ids_become_names_and_unknown_ids_stay_as_they_are() {
        let cases = [
            (
                "Delivered message to agt_01J8Z3F5KDQW9V2MRXB7NCTHYE.",
                "Delivered message to steward:api.",
            ),
            (
                "Task tsk_01J8Z3F5KDQW9V2MRXB7NCTHYF completed.",
                "Task split the migration completed.",
            ),
            // Two in one line, and one nobody can place.
            (
                "agt_01J8Z3F5KDQW9V2MRXB7NCTHYE waits for tsk_0000000000000000000000000Z",
                "steward:api waits for tsk_0000000000000000000000000Z",
            ),
            // Nothing to do: borrowed back untouched.
            ("no ids here at all", "no ids here at all"),
            // Not an id: too short, and a longer run is something else.
            ("agt_01J8Z3F5", "agt_01J8Z3F5"),
            (
                "agt_01J8Z3F5KDQW9V2MRXB7NCTHYEEXTRA",
                "agt_01J8Z3F5KDQW9V2MRXB7NCTHYEEXTRA",
            ),
        ];
        for (input, want) in cases {
            assert_eq!(name_ids(input, known), want, "input: {input}");
        }
        assert!(
            matches!(
                name_ids("no ids here at all", known),
                std::borrow::Cow::Borrowed(_)
            ),
            "text with no id must not be rebuilt"
        );
    }
}
