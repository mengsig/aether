//! Grouping one scope's memories into near-duplicate clusters, and choosing
//! the one each cluster keeps.
//!
//! Deterministic by construction: records are put in an order read from
//! their content, links are taken strongest first with ties broken by that
//! order, and nothing depends on the order the store returned them in.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use aether_core::event::Actor;
use aether_core::memory::MemoryRecord;
use aether_core::memory_admission::{MemoryAdmission, MemoryState};

use super::similarity::{self, Features, LINK, RELATED};

/// Memories one word may be in and still propose the pairs that share it.
const MAX_POSTING: usize = 256;

/// A record with the admission it stands at.
#[derive(Debug, Clone)]
pub(crate) struct Standing {
    pub record: MemoryRecord,
    pub admission: MemoryAdmission,
}

impl Standing {
    /// Pinned, or stated by a person: a decision that consolidation may keep
    /// but never retire, so a cluster holds at most one and keeps that one.
    pub(crate) fn fixed(&self) -> bool {
        self.record.pinned
            || self
                .record
                .provenance
                .actor
                .as_ref()
                .is_some_and(|a| matches!(a.original(), Actor::User { .. }))
    }
}

/// `records`, all of one scope, grouped into clusters of two or more. Each
/// cluster lists its survivor first, then the rest strongest first; clusters
/// come in the content order of their survivors.
pub(crate) fn clusters(mut records: Vec<Standing>) -> Vec<Vec<Standing>> {
    debug_assert!(
        records
            .windows(2)
            .all(|w| w[0].record.scope == w[1].record.scope)
    );
    records.sort_by(content_order);
    let features: Vec<Features> = records
        .iter()
        .map(|s| Features::of(&s.record.text))
        .collect();
    let compatible = compatible_pairs(&records, &features);
    let groups = link(records.len(), &compatible);
    let mut slots: Vec<Option<Standing>> = records.into_iter().map(Some).collect();
    let mut out: Vec<Vec<Standing>> = groups
        .into_iter()
        .map(|group| {
            let mut members: Vec<Standing> =
                group.iter().filter_map(|&i| slots[i].take()).collect();
            debug_assert_eq!(members.len(), group.len(), "a record sits in one cluster");
            members.sort_by(survivor_order);
            members
        })
        .collect();
    out.sort_by(|a, b| content_order(&a[0], &b[0]));
    debug_assert!(out.iter().all(|c| c.len() >= 2));
    debug_assert!(out.iter().all(|c| c.iter().skip(1).all(|s| !s.fixed())));
    out
}

/// Every pair that may share a cluster, with its similarity. Only pairs that
/// share a stemmed word are compared: the rest score zero by definition. A
/// word in more than [`MAX_POSTING`] memories proposes no pairs, so a scope
/// full of one common word costs no more than one without it.
fn compatible_pairs(records: &[Standing], features: &[Features]) -> BTreeMap<(usize, usize), f32> {
    let mut postings: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (i, f) in features.iter().enumerate() {
        f.stems()
            .for_each(|stem| postings.entry(stem).or_default().push(i));
    }
    postings.retain(|_, ids| ids.len() <= MAX_POSTING);
    let mut compatible = BTreeMap::new();
    for (i, f) in features.iter().enumerate() {
        let later: BTreeSet<usize> = f
            .stems()
            .filter_map(|stem| postings.get(stem))
            .flat_map(|ids| ids.iter().copied().filter(|&j| j > i))
            .collect();
        for j in later {
            if records[i].fixed() && records[j].fixed() {
                continue;
            }
            let score = similarity::similarity(f, &features[j]);
            if score >= RELATED && similarity::conflict(f, &features[j]).is_none() {
                compatible.insert((i, j), score);
            }
        }
    }
    debug_assert!(compatible.keys().all(|(i, j)| i < j && *j < records.len()));
    compatible
}

/// Join clusters along links, strongest first, only while every pair across
/// the two is compatible: no chain of links can join a conflicting pair.
fn link(n: usize, compatible: &BTreeMap<(usize, usize), f32>) -> Vec<Vec<usize>> {
    let mut links: Vec<((usize, usize), f32)> = compatible
        .iter()
        .filter(|(_, score)| **score >= LINK)
        .map(|(pair, score)| (*pair, *score))
        .collect();
    links.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    let mut cluster_of: Vec<usize> = (0..n).collect();
    let mut members: Vec<Vec<usize>> = (0..n).map(|i| vec![i]).collect();
    for ((i, j), _) in links {
        let (a, b) = (cluster_of[i], cluster_of[j]);
        if a == b {
            continue;
        }
        let joinable = members[a].iter().all(|&x| {
            members[b]
                .iter()
                .all(|&y| compatible.contains_key(&(x.min(y), x.max(y))))
        });
        if !joinable {
            continue;
        }
        let (keep, gone) = (a.min(b), a.max(b));
        let moved = std::mem::take(&mut members[gone]);
        moved.iter().for_each(|&x| cluster_of[x] = keep);
        members[keep].extend(moved);
    }
    let groups: Vec<Vec<usize>> = members.into_iter().filter(|m| m.len() >= 2).collect();
    debug_assert!(
        groups.iter().flatten().collect::<BTreeSet<_>>().len()
            == groups.iter().map(Vec::len).sum::<usize>()
    );
    groups
}

/// An order read from content alone, so the same memories cluster the same
/// way whatever order they were written in.
fn content_order(a: &Standing, b: &Standing) -> Ordering {
    a.record
        .text
        .cmp(&b.record.text)
        .then(a.record.importance.total_cmp(&b.record.importance))
        .then(a.record.confidence.total_cmp(&b.record.confidence))
        .then(a.record.id.cmp(&b.record.id))
}

/// The survivor first: a fixed record, then an active one, then the best
/// verified, best supported, most important and most confident.
fn survivor_order(a: &Standing, b: &Standing) -> Ordering {
    let active = |s: &Standing| s.admission.state == MemoryState::Active;
    b.fixed()
        .cmp(&a.fixed())
        .then(active(b).cmp(&active(a)))
        .then(b.admission.verification.cmp(&a.admission.verification))
        .then(b.admission.independent().cmp(&a.admission.independent()))
        .then(b.admission.tasks().cmp(&a.admission.tasks()))
        .then(b.record.importance.total_cmp(&a.record.importance))
        .then(b.record.confidence.total_cmp(&a.record.confidence))
        .then_with(|| content_order(a, b))
}
