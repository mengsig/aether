//! Repository aliases: an operator names an old repository id and the id its
//! memory lives under now. The store moves the memory; this resolves the target.

use std::path::PathBuf;

use aether_core::event::Actor;
use aether_core::repository::{RepositoryAlias, RepositoryAliasReport};
use aether_core::time::now;
use aether_store::StoreError;

use crate::{Result, Runtime, RuntimeError};

impl Runtime {
    /// Alias `alias` to the repository at `root`, or to the id `repository`,
    /// moving every memory filed under the alias. A dry run moves nothing and
    /// reports what a real run would move now. Agents' rows are never rewritten.
    pub fn alias_repository(
        &self,
        alias: &str,
        root: Option<PathBuf>,
        repository: Option<String>,
        dry_run: bool,
        by: Actor,
    ) -> Result<RepositoryAliasReport> {
        if !matches!(by, Actor::User { .. }) {
            return Err(RuntimeError::Invalid(
                "only an operator can alias a repository".into(),
            ));
        }
        let canonical = alias_canonical(root, repository)?;
        let alias =
            RepositoryAlias::new(alias, &canonical, now()).map_err(RuntimeError::Invalid)?;
        debug_assert!(alias.is_valid());
        let moved = if dry_run {
            self.store.preview_repository_alias(&alias)
        } else {
            let hold = self.hold();
            let moved = self.store.alias_repository(&alias, by, &hold);
            for event in moved.iter().flat_map(|m| &m.events) {
                self.bus.publish_event(event.clone());
            }
            drop(hold);
            moved
        }
        .map_err(refused)?;
        debug_assert!(dry_run || moved.existing || moved.events.len() == 1);
        Ok(RepositoryAliasReport {
            alias: alias.alias,
            canonical: alias.canonical,
            dry_run,
            existing: moved.existing,
            counts: moved.counts,
        })
    }

    /// Every registered repository alias, ordered by alias.
    pub fn repository_aliases(&self) -> Result<Vec<RepositoryAlias>> {
        Ok(self.store.repository_aliases()?)
    }
}

/// The id an alias resolves to: the one a checkout at `root` derives, exactly
/// as `create_agent.repository_root` derives it, or an id named outright.
fn alias_canonical(root: Option<PathBuf>, repository: Option<String>) -> Result<String> {
    match (root, repository) {
        (Some(_), Some(_)) => Err(RuntimeError::Invalid(
            "give either root or repository, not both".into(),
        )),
        (None, None) => Err(RuntimeError::Invalid(
            "name what the alias resolves to: a checkout root or a repository id".into(),
        )),
        (Some(root), None) => crate::commands::checkout_repository_id(&root),
        (None, Some(named)) => aether_core::repository::normalize_id(&named).ok_or_else(|| {
            RuntimeError::Invalid(format!("repository `{named}` is not a valid repository id"))
        }),
    }
}

/// An alias the store refused is the caller's mistake, not the daemon's.
fn refused(error: StoreError) -> RuntimeError {
    match error {
        StoreError::AliasRefused(why) => RuntimeError::Invalid(why),
        other => RuntimeError::Store(other),
    }
}
