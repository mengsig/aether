//! The report as text: a digest for a terminal, a document for a file.
//!
//! Both read the same [`RunReport`], so a person and a file never disagree
//! about what a run did.

use std::fmt::Write as _;
use unicode_segmentation::UnicodeSegmentation;

use super::{Ended, OpenLoop, RunReport};

/// Dollars, with enough places to see a cheap run, said the way every
/// surface says it: a floor is marked, a cost nobody reported at all is not
/// a number, and an API-equivalent estimate is labelled as the estimate it
/// is rather than passed off as spend.
fn usd(usage: &crate::model::Usage) -> String {
    usage.cost_and_estimate_line()
}

/// Whether a subtree roll-up would only repeat the agent's own figure. Not
/// `==`: that compares an f64 too, so a child that added no dollars but an
/// unpriced model would print a redundant line. Equal to the fourth decimal
/// the line itself prints - and a roll-up that is only a floor, or one that
/// carries an estimate the own figure does not, is never a repeat of it.
fn adds_nothing(subtree: &crate::model::Usage, own: &crate::model::Usage) -> bool {
    let estimates_agree = match (subtree.api_equivalent_usd(), own.api_equivalent_usd()) {
        (None, None) => true,
        (Some(a), Some(b)) => (a - b).abs() < 0.000_05,
        _ => false,
    };
    subtree.total_tokens() == own.total_tokens()
        && subtree.cost_complete == own.cost_complete
        && subtree.cost_gap.unwrap_or(crate::model::CostGap::Unknown)
            == own.cost_gap.unwrap_or(crate::model::CostGap::Unknown)
        && (subtree.cost_usd - own.cost_usd).abs() < 0.000_05
        && estimates_agree
}

/// A count a person can read at a glance: 1234 -> 1.2k.
fn count(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => format!("{:.1}k", n as f64 / 1_000.0),
        _ => format!("{:.1}M", n as f64 / 1_000_000.0),
    }
}

/// Wall time, from milliseconds.
fn duration(ms: i64) -> String {
    let secs = ms.max(0) / 1000;
    match secs {
        0..=59 => format!("{secs}s"),
        60..=3599 => format!("{}m {}s", secs / 60, secs % 60),
        _ => format!("{}h {}m", secs / 3600, (secs % 3600) / 60),
    }
}

fn outcome_of(ended: &Ended) -> String {
    outcome_with_separator(ended, "·")
}

fn outcome_with_separator(ended: &Ended, separator: &str) -> String {
    match ended {
        Ended::Running => "running".into(),
        Ended::Completed { summary: Some(s) } => format!("completed {separator} {s}"),
        Ended::Completed { summary: None } => "completed".into(),
        Ended::Failed { error } => format!("failed {separator} {error}"),
        Ended::Cancelled { reason: Some(r) } => format!("cancelled {separator} {r}"),
        Ended::Cancelled { reason: None } => "cancelled".into(),
    }
}

/// One line, inert wherever the `.md` is read. Pipes and newlines would
/// break out of the table; `<` and `&` would make an imported objective into
/// active content in a viewer that renders HTML, and `.md` is the format
/// people paste into an issue or a wiki.
fn cell(text: &str) -> String {
    let flat: String = text
        .chars()
        .map(|c| match c {
            '|' => '/',
            '\n' | '\r' | '\t' => ' ',
            other => other,
        })
        .collect();
    let trimmed = flat.trim();
    let clipped: String = if trimmed.graphemes(true).count() > 70 {
        format!("{}…", trimmed.graphemes(true).take(69).collect::<String>())
    } else {
        trimmed.to_owned()
    };
    let mut out = String::with_capacity(clipped.len());
    for c in clipped.chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            // Markdown's own emphasis, link and code punctuation.
            '*' | '_' | '`' | '[' | ']' | '(' | ')' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            other => out.push(other),
        }
    }
    out
}

impl RunReport {
    /// A short answer to "what did this run do", for a terminal.
    pub fn digest(&self) -> String {
        self.digest_with_ascii(false)
    }

    /// ASCII decoration only; objectives, names and other run content keep Unicode.
    pub fn digest_with_ascii(&self, ascii: bool) -> String {
        let separator = if ascii { "|" } else { "·" };
        // Only Usage's generated numeric labels and qualifiers reach this.
        let cost = |usage: &crate::model::Usage| {
            let text = usd(usage);
            if ascii {
                return text
                    .replace("≥ ", ">= ")
                    .replace(crate::model::Usage::APPROX, "~");
            }
            text
        };
        let b = &self.body;
        let mut out = String::new();
        let _ = writeln!(
            out,
            "run {} {separator} {} {separator} {} {separator} {} {separator} {} agents {separator} {} events",
            self.run,
            b.outcome.as_str(),
            duration(b.duration_ms),
            cost(&b.usage),
            b.agents.len(),
            b.events
        );
        if let Some(p) = &self.provenance {
            let _ = writeln!(
                out,
                "imported from {} of {}",
                p.source_run, p.source_instance
            );
        }
        if let Some(objective) = &b.objective {
            let _ = writeln!(out, "{objective}");
        }
        out.push('\n');
        for agent in &b.agents {
            let indent = "  ".repeat(agent.depth as usize);
            // Tokens are the agent's own, so the cost beside them must be too;
            // the roll-up is said separately, and only where it says something
            // the own figure does not.
            let subtree = if adds_nothing(&agent.subtree_usage, &agent.usage) {
                String::new()
            } else {
                format!("subtree {} {separator} ", cost(&agent.subtree_usage))
            };
            let _ = writeln!(
                out,
                "{indent}{} ({}) {separator} {} turns {separator} {} tools {separator} {} in / {} cached / {} out {separator} own {} {separator} {subtree}{}",
                agent.name,
                agent.role,
                agent.turns,
                agent.tool_calls,
                count(agent.usage.input_tokens),
                count(agent.usage.cache_read_tokens),
                count(agent.usage.output_tokens),
                cost(&agent.usage),
                outcome_with_separator(&agent.outcome, separator),
            );
        }
        if !b.failures.is_empty() {
            let _ = writeln!(out, "\n{} failure(s):", b.failures.len());
            for failure in &b.failures {
                let _ = writeln!(out, "  {} {}", failure.kind, failure.error);
            }
        }
        if !b.open_loops.is_empty() {
            let _ = writeln!(out, "\n{} open:", b.open_loops.len());
            for open in &b.open_loops {
                let _ = writeln!(out, "  {}", open.line());
            }
        }
        // Once, on the finished document, rather than at each of the dozen
        // places run content enters it: the renderer writes no control
        // character of its own, so nothing here is worth keeping.
        crate::text::strip_controls(&out).into_owned()
    }

    /// The whole document, as markdown. This is what `--md` writes and what
    /// a person keeps beside a run.
    pub fn to_markdown(&self) -> String {
        let b = &self.body;
        let mut out = String::new();
        let _ = writeln!(out, "# Run {}\n", self.run);
        if let Some(p) = &self.provenance {
            let _ = writeln!(
                out,
                "> Imported from run `{}` of instance `{}`, exported {}.\n",
                p.source_run,
                p.source_instance,
                p.exported_at.to_rfc3339()
            );
        }
        if let Some(objective) = &b.objective {
            let _ = writeln!(out, "**{}**\n", cell(objective));
        }
        let _ = writeln!(
            out,
            "- outcome: **{}**\n- started: {}\n- duration: {}\n- cost: {}\n- tokens: {} in · {} out · {} cache read\n- events: {} (seq {}..{})\n",
            b.outcome.as_str(),
            b.started_at.to_rfc3339(),
            duration(b.duration_ms),
            usd(&b.usage),
            count(b.usage.input_tokens),
            count(b.usage.output_tokens),
            count(b.usage.cache_read_tokens),
            b.events,
            self.from_seq,
            self.to_seq,
        );

        let _ = writeln!(out, "## Agents\n");
        let _ = writeln!(
            out,
            "| agent | role | model | turns | tools | in | cached | out | cost | subtree | outcome |"
        );
        let _ = writeln!(out, "|---|---|---|---:|---:|---:|---:|---:|---:|---:|---|");
        for a in &b.agents {
            let name = format!("{}{}", "· ".repeat(a.depth as usize), a.name);
            let _ = writeln!(
                out,
                "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
                cell(&name),
                cell(a.role.as_str()),
                cell(&a.model.to_string()),
                a.turns,
                a.tool_calls,
                count(a.usage.input_tokens),
                count(a.usage.cache_read_tokens),
                count(a.usage.output_tokens),
                usd(&a.usage),
                usd(&a.subtree_usage),
                cell(&outcome_of(&a.outcome)),
            );
        }

        if !b.tasks.is_empty() {
            let _ = writeln!(out, "\n## Tasks\n");
            let _ = writeln!(out, "| task | objective | priority | outcome |");
            let _ = writeln!(out, "|---|---|---|---|");
            for t in &b.tasks {
                let _ = writeln!(
                    out,
                    "| `{}` | {} | {} | {} |",
                    t.id,
                    cell(&t.objective),
                    t.priority.as_str(),
                    cell(&outcome_of(&t.outcome)),
                );
            }
        }

        if !b.tools.is_empty() {
            let _ = writeln!(out, "\n## Tools\n");
            let _ = writeln!(
                out,
                "| tool | requested | ok | failed | denied | cancelled | replayed | retries | time |"
            );
            let _ = writeln!(out, "|---|---:|---:|---:|---:|---:|---:|---:|---:|");
            for t in &b.tools {
                let y = &t.tally;
                let _ = writeln!(
                    out,
                    "| {} | {} | {} | {} | {} | {} | {} | {} | {} |",
                    cell(&t.tool),
                    y.requested,
                    y.succeeded,
                    y.failed,
                    y.denied,
                    y.cancelled,
                    y.replayed,
                    y.retries,
                    duration(t.duration_ms as i64),
                );
            }
        }

        if !b.approvals.is_empty() {
            let _ = writeln!(out, "\n## Approvals\n");
            let _ = writeln!(out, "| tool | what | decision | by |");
            let _ = writeln!(out, "|---|---|---|---|");
            for a in &b.approvals {
                let _ = writeln!(
                    out,
                    "| {} | {} | {} | {} |",
                    cell(&a.tool),
                    cell(&a.description),
                    a.decision.map(|d| d.as_str()).unwrap_or("unanswered"),
                    cell(
                        &a.decided_by
                            .as_ref()
                            .map(super::Actor::label)
                            .unwrap_or_else(|| "-".into()),
                    ),
                );
            }
        }

        if !b.failures.is_empty() {
            let _ = writeln!(out, "\n## Failures\n");
            for f in &b.failures {
                let _ = writeln!(
                    out,
                    "- `{}` {} — {}",
                    f.kind,
                    f.at.to_rfc3339(),
                    cell(&f.error)
                );
            }
        }

        if !b.artifacts.is_empty() {
            let _ = writeln!(out, "\n## Artifacts\n");
            let _ = writeln!(out, "| name | kind | bytes | summary |");
            let _ = writeln!(out, "|---|---|---:|---|");
            for a in &b.artifacts {
                let dropped = if a.evicted { " (bytes dropped)" } else { "" };
                let _ = writeln!(
                    out,
                    "| {} | {} | {} | {}{} |",
                    cell(&a.name),
                    cell(&a.kind),
                    a.size_bytes,
                    cell(&a.summary),
                    dropped,
                );
            }
        }

        if !b.open_loops.is_empty() {
            let _ = writeln!(out, "\n## Open loops\n");
            for open in &b.open_loops {
                let id = match open {
                    OpenLoop::Agent { id, .. } => id.to_string(),
                    OpenLoop::Task { id, .. } => id.to_string(),
                    OpenLoop::Approval { id, .. } => id.to_string(),
                };
                let _ = writeln!(out, "- {} (`{id}`)", cell(&open.line()));
            }
        }

        if !b.timeline.is_empty() {
            let _ = writeln!(out, "\n## Timeline\n");
            for m in &b.timeline {
                let _ = writeln!(
                    out,
                    "- {} `{}` {}",
                    m.at.to_rfc3339(),
                    m.kind,
                    cell(&m.detail)
                );
            }
        }
        crate::text::strip_controls(&out).into_owned()
    }
}

#[cfg(test)]
mod tests;
