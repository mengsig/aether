//! Each agent's usage beside its row (`agent_usage`), and the totals the fleet
//! and each run read as one row (`usage_totals`), both kept by the V33 triggers.
//! The triggers read a written body through [`COUNTED_USAGE`], which decodes it
//! as the whole-agent reads do: an agent they would quarantine is not counted.

use std::collections::BTreeMap;

use aether_core::agent::Agent;
use aether_core::id::AgentId;
use aether_core::model::{CostBasis, CostGap, Usage};
use rusqlite::functions::FunctionFlags;
use rusqlite::types::ValueRef;
use rusqlite::{Connection, OptionalExtension, Row};
use serde::Serialize;

use crate::{Result, StoreError};

/// `aether_counted_usage(id, body)`: the agent's usage as an `agent_usage` row
/// in JSON, or NULL when `body` is not a readable agent whose id is `id`.
pub(crate) const COUNTED_USAGE: &str = "aether_counted_usage";

/// The `usage_totals` scope of the whole fleet; every other scope is a run id.
pub(crate) const FLEET: &str = "";

/// Registers [`COUNTED_USAGE`] on `conn`. Every connection that writes agents
/// needs it, a raw one opened outside the store included: without it a write
/// fails on the trigger, never silently.
pub fn register(conn: &Connection) -> rusqlite::Result<()> {
    let flags = FunctionFlags::SQLITE_UTF8
        | FunctionFlags::SQLITE_DETERMINISTIC
        | FunctionFlags::SQLITE_INNOCUOUS;
    conn.create_scalar_function(COUNTED_USAGE, 2, flags, |ctx| {
        assert_eq!(ctx.len(), 2, "registered with two arguments");
        let text = |index| match ctx.get_raw(index) {
            ValueRef::Text(bytes) => std::str::from_utf8(bytes).ok(),
            _ => None,
        };
        Ok(counted_usage(text(0), text(1)))
    })
}

/// What [`COUNTED_USAGE`] answers for one row.
fn counted_usage(id: Option<&str>, body: Option<&str>) -> Option<String> {
    let (id, body) = (id?, body?);
    let agent: Agent = serde_json::from_str(body).ok()?;
    if agent.id.to_string() != id {
        return None;
    }
    let row = LedgerUsage::of(&agent.stats.usage);
    let json = serde_json::to_string(&row).expect("a ledger row always serializes");
    debug_assert!(!json.is_empty(), "a counted agent has a row");
    Some(json)
}

/// One agent's usage as `agent_usage` holds it. Counters are clamped to
/// SQLite's integer range; the fold's two flags are kept by name.
#[derive(Debug, PartialEq, Serialize)]
struct LedgerUsage {
    input_tokens: i64,
    output_tokens: i64,
    cache_read_tokens: i64,
    cache_write_tokens: i64,
    cache_write_1h_tokens: i64,
    reasoning_tokens: Option<i64>,
    cost_usd: f64,
    estimated_cost_usd: Option<f64>,
    /// Absent exactly when every cost is accounted for.
    cost_gap: Option<&'static str>,
    cost_basis: &'static str,
    /// Whether `cost_basis` counts when totals combine: [`Usage::has_cost_figure`].
    figure: bool,
}

impl LedgerUsage {
    fn of(usage: &Usage) -> Self {
        let count = |n: u64| i64::try_from(n).unwrap_or(i64::MAX);
        let gap = (!usage.cost_complete).then(|| usage.cost_gap.unwrap_or(CostGap::Unknown));
        let row = Self {
            input_tokens: count(usage.input_tokens),
            output_tokens: count(usage.output_tokens),
            cache_read_tokens: count(usage.cache_read_tokens),
            cache_write_tokens: count(usage.cache_write_tokens),
            cache_write_1h_tokens: count(usage.cache_write_1h_tokens),
            reasoning_tokens: usage.reasoning_tokens.map(count),
            cost_usd: usage.cost_usd,
            estimated_cost_usd: usage.estimated_cost_usd,
            cost_gap: gap.map(gap_name),
            cost_basis: basis_name(usage.cost_basis),
            figure: usage.has_cost_figure(),
        };
        assert!(row.cost_usd.is_finite(), "a decoded cost is a number");
        assert!(row.input_tokens >= 0 && row.cache_write_1h_tokens >= 0);
        row
    }
}

const GAPS: [CostGap; 5] = [
    CostGap::Unreported,
    CostGap::Unpriced,
    CostGap::Interrupted,
    CostGap::Mixed,
    CostGap::Unknown,
];

const BASES: [CostBasis; 4] = [
    CostBasis::Unknown,
    CostBasis::ListPrice,
    CostBasis::ProviderReported,
    CostBasis::Mixed,
];

/// The names the V33 SQL compares against, which are the wire names.
fn gap_name(gap: CostGap) -> &'static str {
    match gap {
        CostGap::Unreported => "unreported",
        CostGap::Unpriced => "unpriced",
        CostGap::Interrupted => "interrupted",
        CostGap::Mixed => "mixed",
        CostGap::Unknown => "unknown",
    }
}

fn basis_name(basis: CostBasis) -> &'static str {
    match basis {
        CostBasis::Unknown => "unknown",
        CostBasis::ListPrice => "list_price",
        CostBasis::ProviderReported => "provider_reported",
        CostBasis::Mixed => "mixed",
    }
}

/// A count or sum kept as an INTEGER, read back as a token count. SQLite turns
/// an overflowing sum into a REAL; that saturates, as `Usage`'s own sums do.
fn tokens(row: &Row<'_>, index: usize) -> rusqlite::Result<u64> {
    Ok(match row.get_ref(index)? {
        ValueRef::Integer(n) => u64::try_from(n).unwrap_or(0),
        ValueRef::Real(x) if x >= u64::MAX as f64 => u64::MAX,
        ValueRef::Real(x) => x.max(0.0) as u64,
        other => {
            return Err(rusqlite::Error::InvalidColumnType(
                index,
                "usage count".into(),
                other.data_type(),
            ));
        }
    })
}

/// A dollar sum: never below zero, as the costs summed never are.
fn dollars(row: &Row<'_>, index: usize) -> rusqlite::Result<f64> {
    Ok(row.get::<_, f64>(index)?.max(0.0))
}

/// The columns [`totals_from`] reads, in its order.
pub(super) const TOTALS_COLUMNS: &str = "members, input_tokens, output_tokens, cache_read_tokens,
    cache_write_tokens, cache_write_1h_tokens, reasoning_members, reasoning_tokens, cost_usd,
    estimated_members, estimated_cost_usd, gap_unreported, gap_unpriced, gap_interrupted,
    gap_mixed, gap_unknown, basis_unknown, basis_list_price, basis_provider_reported, basis_mixed";

/// One `usage_totals` row as the usage the fold of its members gives: sums
/// for the counters, and the flags from how many members carried each value.
fn totals_from(row: &Row<'_>) -> rusqlite::Result<(u64, Usage)> {
    let members = tokens(row, 0)?;
    let reasoning_members = tokens(row, 6)?;
    let estimated_members = tokens(row, 9)?;
    if reasoning_members > members || estimated_members > members {
        return Err(rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Integer,
            format!("usage totals of {members} members count more of them with a figure").into(),
        ));
    }
    let mut gaps = Vec::new();
    for (offset, gap) in GAPS.into_iter().enumerate() {
        if tokens(row, 11 + offset)? > 0 {
            gaps.push(gap);
        }
    }
    let mut bases = Vec::new();
    for (offset, basis) in BASES.into_iter().enumerate() {
        if tokens(row, 16 + offset)? > 0 {
            bases.push(basis);
        }
    }
    let usage = Usage {
        input_tokens: tokens(row, 1)?,
        output_tokens: tokens(row, 2)?,
        cache_read_tokens: tokens(row, 3)?,
        cache_write_tokens: tokens(row, 4)?,
        cache_write_1h_tokens: tokens(row, 5)?,
        reasoning_tokens: (reasoning_members > 0)
            .then(|| tokens(row, 7))
            .transpose()?,
        cost_usd: dollars(row, 8)?,
        cost_complete: gaps.is_empty(),
        cost_gap: combined_gap(&gaps),
        cost_basis: combined_basis(&bases),
        estimated_cost_usd: (estimated_members > 0)
            .then(|| dollars(row, 10))
            .transpose()?,
    };
    debug_assert!(usage.cost_usd >= 0.0, "dollars are clamped at zero");
    Ok((members, usage))
}

/// The gap `Usage`'s fold leaves from these distinct gaps: one, or mixed.
fn combined_gap(gaps: &[CostGap]) -> Option<CostGap> {
    match gaps {
        [] => None,
        [only] => Some(*only),
        _ => Some(CostGap::Mixed),
    }
}

/// The basis `Usage`'s fold leaves from the distinct bases of the members
/// with a figure: unknown absorbs, one stays, several mix.
fn combined_basis(bases: &[CostBasis]) -> CostBasis {
    if bases.contains(&CostBasis::Unknown) {
        return CostBasis::Unknown;
    }
    match bases {
        [] => CostBasis::Unknown,
        [only] => *only,
        _ => CostBasis::Mixed,
    }
}

/// `scope`'s members and their usage, from its one `usage_totals` row.
pub(crate) fn read_totals(conn: &Connection, scope: &str) -> Result<(u64, Usage)> {
    let mut statement = conn.prepare_cached(&format!(
        "SELECT {TOTALS_COLUMNS} FROM usage_totals WHERE scope = ?1"
    ))?;
    let totals = statement
        .query_row([scope], totals_from)
        .optional()
        .map_err(|error| match error {
            rusqlite::Error::FromSqlConversionFailure(..) => {
                StoreError::Corrupt(format!("usage totals of scope `{scope}`: {error}"))
            }
            error => error.into(),
        })?;
    Ok(totals.unwrap_or((0, Usage::default())))
}

/// When `run`'s first counted agent was created, from `agent_usage_run`.
pub(super) fn read_run_start(conn: &Connection, run: &str) -> Result<Option<String>> {
    let mut statement =
        conn.prepare_cached("SELECT MIN(created_at) FROM agent_usage WHERE run = ?1")?;
    Ok(statement.query_row([run], |row| row.get(0))?)
}

/// The columns [`usage_from`] reads, in its order.
pub(super) const USAGE_COLUMNS: &str = "u.input_tokens, u.output_tokens, u.cache_read_tokens,
    u.cache_write_tokens, u.cache_write_1h_tokens, u.reasoning_tokens, u.cost_usd,
    u.estimated_cost_usd, u.cost_gap, u.cost_basis";

/// One agent's usage from its `agent_usage` row, starting at column `at`.
pub(super) fn usage_from(row: &Row<'_>, at: usize) -> rusqlite::Result<Usage> {
    let named = |index: usize| -> rusqlite::Result<Option<String>> { row.get(at + index) };
    let gap = match named(8)? {
        None => None,
        Some(name) => Some(
            GAPS.into_iter()
                .find(|gap| gap_name(*gap) == name)
                .ok_or_else(|| unknown_name(at + 8, &name))?,
        ),
    };
    let basis_name_read = named(9)?.unwrap_or_default();
    let basis = BASES
        .into_iter()
        .find(|basis| basis_name(*basis) == basis_name_read)
        .ok_or_else(|| unknown_name(at + 9, &basis_name_read))?;
    Ok(Usage {
        input_tokens: tokens(row, at)?,
        output_tokens: tokens(row, at + 1)?,
        cache_read_tokens: tokens(row, at + 2)?,
        cache_write_tokens: tokens(row, at + 3)?,
        cache_write_1h_tokens: tokens(row, at + 4)?,
        reasoning_tokens: row.get::<_, Option<i64>>(at + 5)?.map(|n| n.max(0) as u64),
        cost_usd: dollars(row, at + 6)?,
        estimated_cost_usd: row.get::<_, Option<f64>>(at + 7)?.map(|x| x.max(0.0)),
        cost_complete: gap.is_none(),
        cost_gap: gap,
        cost_basis: basis,
    })
}

fn unknown_name(index: usize, name: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        index,
        rusqlite::types::Type::Text,
        format!("`{name}` is no name agent_usage writes").into(),
    )
}

/// The usage of each of `agents` that has an `agent_usage` row, by primary key.
pub(super) fn read_agent_usages(
    conn: &Connection,
    agents: &[AgentId],
) -> Result<BTreeMap<AgentId, Usage>> {
    let mut statement = conn.prepare_cached(&format!(
        "SELECT u.agent_id, {USAGE_COLUMNS} FROM agent_usage u WHERE u.agent_id = ?1"
    ))?;
    let mut usages = BTreeMap::new();
    for agent in agents {
        let read = statement
            .query_row([agent.to_string()], |row| {
                Ok((row.get::<_, String>(0)?, usage_from(row, 1)?))
            })
            .optional()?;
        if let Some((id, usage)) = read {
            if id != agent.to_string() {
                return Err(StoreError::Corrupt(format!(
                    "agent_usage row {id} answered for agent {agent}"
                )));
            }
            usages.insert(*agent, usage);
        }
    }
    assert!(
        usages.len() <= agents.len(),
        "one usage per agent asked for"
    );
    Ok(usages)
}

#[cfg(test)]
#[path = "usage_totals_tests.rs"]
mod tests;
