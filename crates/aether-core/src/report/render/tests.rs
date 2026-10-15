use super::*;
use crate::event::Event;

fn fixture() -> RunReport {
    let events: Vec<Event> =
        serde_json::from_str(include_str!("../../../tests/fixtures/run_log.json"))
            .expect("fixture log");
    let run = events.iter().find_map(|e| e.run).expect("run");
    let at = "2026-02-01T12:00:00Z".parse().expect("timestamp");
    crate::report::build(run, &events, at)
}

#[test]
fn digest_unicode_default_is_byte_for_byte_unchanged() {
    let expected = "run run_01M1ZA1VZHX7VXSKQ9NZKNHBF1 · completed · 25s · $0.0360 · 2 agents · 26 events\nsummarize the repository\n\nroot (assistant) · 1 turns · 1 tools · 2.1k in / 0 cached / 100 out · own $0.0320 · subtree $0.0360 · completed · done\n  reader (worker) · 1 turns · 1 tools · 400 in / 0 cached / 20 out · own $0.0040 · completed · reported the missing manifest\n\n1 failure(s):\n  tool.failed no such file\n";
    assert_eq!(fixture().digest(), expected);
    assert_eq!(fixture().digest_with_ascii(false), expected);
}

#[test]
fn digest_ascii_changes_only_authored_decoration_and_keeps_cost_truth() {
    let mut report = fixture();
    let original = "界·→≥▎";
    report.body.objective = Some(original.into());
    report.body.agents[0].name = original.into();
    report.body.failures[0].error = original.into();
    for ended in [
        Ended::Completed {
            summary: Some(original.into()),
        },
        Ended::Failed {
            error: original.into(),
        },
        Ended::Cancelled {
            reason: Some(original.into()),
        },
    ] {
        report.body.agents[0].outcome = ended;
        for usage in [
            crate::model::Usage::default(),
            crate::model::Usage::cost_only(Some(0.4)),
            crate::model::Usage {
                cost_usd: 0.4,
                cost_complete: false,
                ..Default::default()
            },
        ] {
            report.body.usage = usage;
            report.body.agents[0].usage = usage;
            let text = report.digest_with_ascii(true);
            assert!(text.matches(original).count() >= 4, "{text}");
            assert!(text.replace(original, "DATA").is_ascii(), "{text}");
            if !usage.cost_complete && usage.cost_usd > 0.0 {
                assert!(text.contains(">= $0.4000 (some cost unknown)"), "{text}");
            } else if !usage.cost_complete {
                assert!(text.contains(crate::model::Usage::COST_UNKNOWN), "{text}");
            }
        }
    }
}

/// A cache read is a tenth of the price of fresh input, so folding the two
/// together makes a cache-dominated agent read as fourteen times the work
/// it did - the misreading the separate columns exist to end.
#[test]
fn cache_reads_are_said_apart_from_fresh_input() {
    let mut report = fixture();
    report.body.agents[0].usage = crate::model::Usage {
        input_tokens: 39_500,
        output_tokens: 61_000,
        cache_read_tokens: 3_210_000,
        cost_usd: 0.0,
        cost_complete: false,
        estimated_cost_usd: Some(1.36),
        ..Default::default()
    };
    report.body.agents[0].subtree_usage = report.body.agents[0].usage;
    let line = report
        .digest()
        .lines()
        .find(|l| l.contains("root (assistant)"))
        .expect("the parent's line")
        .to_owned();
    assert!(
        line.contains("39.5k in / 3.2M cached / 61.0k out"),
        "the split is visible: {line}"
    );
    assert!(
        !line.contains("3.3M in"),
        "and the cache is never folded into input: {line}"
    );
    let md = report.to_markdown();
    assert!(md.contains("| 39.5k | 3.2M | 61.0k |"), "{md}");
    assert!(md.contains("cache read"), "the totals split too: {md}");
}

/// An estimate is shown, and can never be mistaken for billed dollars.
#[test]
fn an_api_equivalent_estimate_is_labelled_wherever_it_shows() {
    let mut report = fixture();
    let estimated = crate::model::Usage {
        input_tokens: 39_500,
        cost_usd: 0.0,
        cost_complete: false,
        estimated_cost_usd: Some(1.36),
        ..Default::default()
    };
    report.body.usage = estimated;
    report.body.agents[0].usage = estimated;
    report.body.agents[0].subtree_usage = estimated;
    let text = report.digest();
    assert!(
        text.contains("not reported (≈ $1.3600 API-equivalent)"),
        "{text}"
    );
    assert!(
        !text.contains("$1.3600 ·"),
        "never a bare number in the cost's place: {text}"
    );
    assert!(
        report.to_markdown().contains("≈ $1.3600 API-equivalent"),
        "the document says it too"
    );

    // ASCII only trades the sign, never the label.
    let ascii = report.digest_with_ascii(true);
    assert!(ascii.is_ascii(), "{ascii}");
    assert!(
        ascii.contains("not reported (~ $1.3600 API-equivalent)"),
        "{ascii}"
    );
}

#[test]
fn the_digest_says_the_outcome_the_cost_and_every_agent() {
    let text = fixture().digest();
    assert!(text.contains("completed"), "{text}");
    assert!(text.contains("$0.0360"), "{text}");
    assert!(text.contains("root (assistant)"), "{text}");
    assert!(text.contains("  reader (worker)"), "indented child: {text}");
    assert!(text.contains("no such file"), "{text}");
}

/// The digest's per-agent cost is the agent's *own* (it sits beside that
/// agent's own token counts); the roll-up is said separately, and only
/// where it says something the own figure does not.
#[test]
fn the_digest_tells_an_agents_own_cost_from_its_subtrees() {
    let mut report = fixture();
    {
        let agents = &mut report.body.agents;
        assert!(agents.len() >= 2, "the fixture has a parent and a child");
        agents[0].usage.cost_usd = 0.0100;
        agents[0].subtree_usage.cost_usd = 0.0500;
        // A leaf: its roll-up is itself, so it says nothing extra.
        let own = agents[1].usage;
        agents[1].subtree_usage = own;
    }
    let text = report.digest();
    let lines: Vec<&str> = text.lines().collect();
    let parent = lines
        .iter()
        .find(|l| l.contains("root (assistant)"))
        .expect("the parent's line");
    assert!(
        parent.contains("own $0.0100 · subtree $0.0500"),
        "own before subtree, both named: {parent}"
    );
    let child = lines
        .iter()
        .find(|l| l.contains("reader (worker)"))
        .expect("the child's line");
    assert!(
        !child.contains("subtree"),
        "a roll-up that only repeats the own figure is not said: {child}"
    );
}

/// The roll-up is compared on what the line prints - tokens, dollars, and
/// whether the figure is exact - not on `Usage`'s derived equality, which
/// an f64 would flip. A roll-up that is only a floor says so even when its
/// numbers match the agent's own exact figure: suppressing it told the
/// operator the total was exact when it was a lower bound.
#[test]
fn a_subtree_floor_is_printed_even_when_it_matches_the_own_figure() {
    let priced_alike = |complete: bool| {
        let mut report = fixture();
        {
            let agent = &mut report.body.agents[0];
            agent.usage.cost_usd = 0.0100;
            agent.usage.cost_complete = true;
            agent.subtree_usage = agent.usage;
            agent.subtree_usage.cost_complete = complete;
        }
        report
            .digest()
            .lines()
            .find(|l| l.contains("root (assistant)"))
            .expect("the parent's line")
            .to_owned()
    };
    let floor = priced_alike(false);
    assert!(
        floor.contains("subtree") && floor.contains('\u{2265}'),
        "a floor is not the same figure: {floor}"
    );
    let exact = priced_alike(true);
    assert!(
        !exact.contains("subtree"),
        "an exact roll-up that repeats the own figure adds nothing: {exact}"
    );
}

#[test]
fn the_document_has_a_section_per_thing_the_run_produced() {
    let md = fixture().to_markdown();
    for heading in [
        "## Agents",
        "## Tasks",
        "## Tools",
        "## Approvals",
        "## Failures",
        "## Artifacts",
        "## Timeline",
    ] {
        assert!(md.contains(heading), "{heading} missing from\n{md}");
    }
    assert!(!md.contains("## Open loops"), "nothing was left open");
    assert!(md.contains("summarize the repository"));
}

/// Since import, a report's objectives, errors and summaries can come
/// out of a file someone was sent. Neither surface may hand an escape
/// sequence in one of them straight to a terminal.
#[test]
fn nothing_in_a_report_can_repaint_the_screen_it_is_read_on() {
    let mut report = fixture();
    let hostile = "clean\u{1b}[2J\u{1b}]0;owned\u{7}\u{9b}2Jend";
    report.body.objective = Some(hostile.into());
    if let Some(failure) = report.body.failures.first_mut() {
        failure.error = hostile.into();
    }
    if let Some(agent) = report.body.agents.first_mut() {
        agent.name = hostile.into();
    }
    for text in [report.digest(), report.to_markdown()] {
        assert!(!text.contains('\u{1b}'), "an escape survived: {text:?}");
        assert!(!text.contains('\u{7}'), "a bell survived: {text:?}");
        assert!(!text.contains('\u{9b}'), "a C1 CSI survived: {text:?}");
        assert!(text.contains("clean"), "the text itself was lost: {text}");
        assert!(text.contains("end"), "the tail was lost: {text}");
    }
}

#[test]
fn a_cell_never_breaks_the_table_it_is_in() {
    let md = fixture().to_markdown();
    for line in md.lines().filter(|l| l.starts_with('|')) {
        assert!(
            !line.trim_end().ends_with("| |"),
            "empty trailing cell: {line}"
        );
    }
    assert_eq!(cell("a | b\nc"), "a / b c");
    assert_eq!(cell(&"x".repeat(200)).chars().count(), 70);
}

/// A `.md` report is what a person pastes into an issue or a wiki, and
/// an imported run's objective, agent name or artifact summary is a
/// string someone else wrote.
#[test]
fn a_cell_cannot_become_active_content_where_the_md_is_read() {
    let hostile = cell("<img src=x onerror=alert(1)> [x](javascript:1) **bold** a & b");
    for raw in ["<img", "](", "**"] {
        assert!(!hostile.contains(raw), "{raw} survived: {hostile}");
    }
    assert!(hostile.contains("&lt;img"), "{hostile}");
    assert!(hostile.contains("&amp;"), "{hostile}");
    // The words themselves still read.
    assert!(
        hostile.contains("bold") && hostile.contains("onerror"),
        "{hostile}"
    );
}

#[test]
fn markdown_cells_clip_on_grapheme_boundaries() {
    for cluster in ["👩‍💻", "❤️", "e\u{301}"] {
        let prefix = "a".repeat(68);
        assert_eq!(
            cell(&format!("{prefix}{cluster}zz")),
            format!("{prefix}{cluster}…")
        );
        assert_eq!(
            cell(&format!("{}{cluster}", "a".repeat(69))),
            format!("{}{cluster}", "a".repeat(69))
        );
    }
}
