//! Naming a repository, without carrying its credentials.
//!
//! A repository id is a memory scope key. It reaches the system prompt, the
//! event log, every memory row and every listing, and none of those can be
//! retracted afterwards. Remotes routinely embed secrets -
//! `https://gitlab-ci-token:<job token>@gitlab.example/org/repo.git` is what
//! GitLab CI writes - so the raw remote URL is never the id: it is normalized
//! to host and path first, and anything that still looks like it carries
//! userinfo is refused rather than guessed at.

use serde::{Deserialize, Serialize};

use crate::event::{Actor, EventDraft, EventKind};
use crate::time::Timestamp;

/// The `custom` event name registering a repository alias is recorded under.
pub const ALIASED_EVENT: &str = "repository.aliased";

/// Host plus path for `remote`, with userinfo, port, query, fragment and a
/// `.git` suffix removed and the host lowercased, so every clone of one
/// repository - ssh, https, with or without a token - names one scope.
///
/// `None` when nothing credential-free can be derived: the caller falls back
/// to the canonical path rather than storing a value it cannot vouch for.
pub fn normalize_remote(remote: &str) -> Option<String> {
    let remote = remote.trim();
    if remote.is_empty() {
        return None;
    }
    let (host, path) = split_host_and_path(remote)?;
    let host = host.trim().to_lowercase();
    let path = path.trim();
    // A host-relative path has no use for its separators; a bare local path
    // keeps its leading slash, because that is what makes it absolute.
    let path = match host.is_empty() {
        true => path.trim_end_matches('/'),
        false => path.trim_matches('/'),
    };
    let path = path.strip_suffix(".git").unwrap_or(path);
    let id = match (host.is_empty(), path.is_empty()) {
        (true, true) => return None,
        (true, false) => path.to_owned(),
        (false, true) => host,
        (false, false) => format!("{host}/{path}"),
    };
    // Defence in depth: normalization strips userinfo, so a survivor means
    // the input was a shape this does not understand.
    (!id.contains('@') && !id.contains(char::is_whitespace)).then_some(id)
}

/// A repository id fit to store, from a remote URL, a plain path, or a name a
/// person chose. Anything URL-shaped goes through [`normalize_remote`];
/// anything else is kept as it is, minus surrounding whitespace, so
/// `my-project` stays `my-project` and a filesystem path stays a path.
///
/// `None` for an empty id, one carrying control characters, or a URL-shaped
/// one that cannot be made credential-free.
pub fn normalize_id(id: &str) -> Option<String> {
    let id = id.trim();
    if id.is_empty() {
        return None;
    }
    // This id is rendered into the system prompt and into every memory row,
    // so it holds the same shape [`normalize_remote`] demands: one line, no
    // control characters steering whatever displays it.
    if id.contains(char::is_control) {
        return None;
    }
    // An absolute filesystem path is a path whatever it holds: `/home/u@corp`
    // and `node_modules/@scope/pkg` are directories, not credentials, and
    // refusing them leaves the agent with no repository scope at all.
    if std::path::Path::new(id).is_absolute() {
        return Some(id.to_owned());
    }
    // `@` is the only thing that can hide a credential, and `://` is the only
    // thing that can hide a host; everything else is already a plain name.
    match id.contains("://") || id.contains('@') {
        true => normalize_remote(id),
        false => Some(id.to_owned()),
    }
}

/// An old repository name memory used to be filed under, and the id it now
/// resolves to. Registered by an operator; never guessed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RepositoryAlias {
    pub alias: String,
    pub canonical: String,
    pub created_at: Timestamp,
}

impl RepositoryAlias {
    /// Both ids normalized and distinct, or why not. Chains are the store's
    /// to refuse: only it knows which ids are already aliases.
    pub fn new(alias: &str, canonical: &str, created_at: Timestamp) -> Result<Self, String> {
        let alias = normalize_id(alias)
            .ok_or_else(|| format!("alias `{alias}` is not a valid repository id"))?;
        let canonical = normalize_id(canonical)
            .ok_or_else(|| format!("repository `{canonical}` is not a valid repository id"))?;
        if alias == canonical {
            return Err(format!("`{alias}` cannot be an alias of itself"));
        }
        Ok(Self {
            alias,
            canonical,
            created_at,
        })
    }

    /// The record of registering this alias. A `custom` event, so a client
    /// of every served protocol version still decodes the log.
    pub fn event(&self, moved: &RepositoryAliasCounts, by: Actor) -> EventDraft {
        debug_assert!(self.is_valid(), "an alias event for an invalid alias");
        EventDraft::new(
            by,
            EventKind::Custom {
                name: ALIASED_EVENT.to_owned(),
                payload: serde_json::json!({
                    "alias": self.alias,
                    "canonical": self.canonical,
                    "moved": moved,
                }),
            },
        )
    }

    /// Whether this record holds what [`Self::new`] would have built.
    pub fn is_valid(&self) -> bool {
        normalize_id(&self.alias).as_deref() == Some(self.alias.as_str())
            && normalize_id(&self.canonical).as_deref() == Some(self.canonical.as_str())
            && self.alias != self.canonical
    }
}

/// What registering an alias moves, or moved: every memory under the alias,
/// in any state, and the agents whose stored scope still names it.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
pub struct RepositoryAliasCounts {
    /// Memories in the alias's repository scope.
    pub repository: usize,
    /// Memories in the alias's repository-role scopes.
    pub repository_role: usize,
    /// Current memories whose text a current memory of the scope they move
    /// into already holds. Both are kept: nothing is merged or superseded.
    pub duplicates: usize,
    /// Agents, archived and finished ones included, whose stored
    /// `scope.repository` is the alias. Their rows are not rewritten.
    pub agents: usize,
}

impl RepositoryAliasCounts {
    pub fn memories(&self) -> usize {
        self.repository + self.repository_role
    }
}

/// The answer to registering an alias, or to a dry run of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RepositoryAliasReport {
    pub alias: String,
    pub canonical: String,
    /// Nothing was written; the counts are what a real run would move now.
    pub dry_run: bool,
    /// The alias was already registered to this canonical id. A real run
    /// still moves anything filed under the alias since.
    pub existing: bool,
    pub counts: RepositoryAliasCounts,
}

/// The authority and path of a remote, for every shape git accepts.
/// `(host, path)`; the host is empty for a local path or a `file:` URL.
fn split_host_and_path(remote: &str) -> Option<(&str, &str)> {
    if let Some((scheme, rest)) = remote.split_once("://") {
        let rest = rest.split(['?', '#']).next().unwrap_or(rest);
        // `file://` has no authority worth keeping: the rest is a path, and
        // its leading slash is part of it.
        if scheme.eq_ignore_ascii_case("file") {
            return Some(("", rest.strip_prefix("localhost").unwrap_or(rest)));
        }
        let (authority, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i + 1..]),
            None => (rest, ""),
        };
        // Everything before the last `@` is userinfo, and a password may
        // itself contain `@`. A port distinguishes nothing about which
        // repository this is.
        let host = strip_port(authority.rsplit('@').next().unwrap_or(authority));
        return Some((host, path));
    }
    // scp-like: `[user@]host:path`, but not a Windows drive or a bare path.
    if let Some((authority, path)) = remote.split_once(':')
        && !path.starts_with('/')
        && !path.starts_with('\\')
    {
        let host = authority.rsplit('@').next().unwrap_or(authority);
        if !host.is_empty() && !host.contains('/') {
            // A token can ride in a query here too: this shape is not a URL,
            // but git accepts one and the id must not carry it either way.
            return Some((host, path.split(['?', '#']).next().unwrap_or(path)));
        }
    }
    // A local path. One carrying `@` is a shape this does not understand.
    match remote.contains('@') {
        true => None,
        false => Some(("", remote)),
    }
}

/// Drop `:port` from a host, leaving a bracketed IPv6 literal intact.
fn strip_port(host: &str) -> &str {
    if host.starts_with('[') {
        return match host.find(']') {
            Some(end) => &host[..=end],
            None => host,
        };
    }
    host.split_once(':').map_or(host, |(h, _)| h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_clone_of_one_repository_names_one_scope() {
        for remote in [
            "https://github.com/mengsig/aether.git",
            "https://github.com/mengsig/aether",
            "https://GitHub.com/mengsig/aether.git/",
            "git@github.com:mengsig/aether.git",
            "ssh://git@github.com:22/mengsig/aether.git",
            "git://github.com/mengsig/aether.git",
            "https://github.com/mengsig/aether.git?depth=1#frag",
        ] {
            assert_eq!(
                normalize_remote(remote).as_deref(),
                Some("github.com/mengsig/aether"),
                "{remote}"
            );
        }
    }

    #[test]
    fn a_credential_in_the_remote_never_reaches_the_id() {
        for remote in [
            "https://gitlab-ci-token:glcbt-64hexchars@gitlab.example.com/org/repo.git",
            "https://ghp_0123456789abcdef@github.com/org/repo.git",
            "https://user:p@ss@word@github.com/org/repo.git",
            "ssh://deploy-key:secret@git.example.com:2222/org/repo.git",
        ] {
            let id = normalize_remote(remote).expect("normalized");
            assert!(!id.contains('@'), "{remote} -> {id}");
            for secret in ["glcbt-64hexchars", "ghp_0123456789abcdef", "p@ss", "secret"] {
                assert!(!id.contains(secret), "{remote} -> {id} leaks {secret}");
            }
        }
        assert_eq!(
            normalize_remote("https://gitlab-ci-token:tok@gitlab.example.com/org/repo.git")
                .as_deref(),
            Some("gitlab.example.com/org/repo")
        );
        assert_eq!(
            normalize_remote("ssh://deploy-key:secret@git.example.com:2222/org/repo.git")
                .as_deref(),
            Some("git.example.com/org/repo")
        );
    }

    #[test]
    fn a_local_remote_keeps_the_path_that_identifies_it() {
        assert_eq!(
            normalize_remote("file:///srv/git/repo.git").as_deref(),
            Some("/srv/git/repo")
        );
        assert_eq!(
            normalize_remote("/srv/git/repo.git").as_deref(),
            Some("/srv/git/repo")
        );
        assert_eq!(normalize_remote("   ").as_deref(), None);
    }

    #[test]
    fn a_name_a_person_chose_survives_and_a_url_is_normalized() {
        assert_eq!(normalize_id("my-project").as_deref(), Some("my-project"));
        assert_eq!(
            normalize_id("/home/me/work").as_deref(),
            Some("/home/me/work")
        );
        assert_eq!(
            normalize_id("https://tok@github.com/org/repo.git").as_deref(),
            Some("github.com/org/repo")
        );
        assert_eq!(normalize_id("  ").as_deref(), None);
    }

    /// A workspace path is a path. Routing one through the URL rules because
    /// it happens to contain `@` refused it, and the agent was then created
    /// with no repository scope at all - reading no repository notes and
    /// writing memory no sibling could see.
    #[test]
    fn a_workspace_path_carrying_an_at_sign_is_still_a_path() {
        for path in [
            "/home/user@corp/project",
            "/home/u/node_modules/@babel/core",
            "/srv/git/repo@2",
        ] {
            assert_eq!(normalize_id(path).as_deref(), Some(path), "{path}");
        }
    }

    /// The id reaches the system prompt and every memory row, so it holds one
    /// shape: `normalize_remote` refuses a newline and so does this.
    #[test]
    fn an_id_carrying_control_characters_is_refused() {
        for id in ["my\nproject", "repo\u{7}", "a\tb"] {
            assert_eq!(normalize_id(id).as_deref(), None, "{id:?}");
        }
    }

    /// `?` and `#` were trimmed on the `://` branch only, so a token in a
    /// query survived into an id this module says has them removed.
    #[test]
    fn a_query_never_survives_the_scp_like_shape_either() {
        assert_eq!(
            normalize_remote("git@github.com:org/repo.git?token=abc").as_deref(),
            Some("github.com/org/repo")
        );
        assert_eq!(
            normalize_remote("git@github.com:org/repo#frag").as_deref(),
            Some("github.com/org/repo")
        );
    }

    #[test]
    fn an_alias_holds_two_normalized_distinct_ids() {
        let at = crate::time::now();
        let alias =
            RepositoryAlias::new(" archon ", "https://github.com/mengsig/archon.git", at).unwrap();
        assert_eq!(alias.alias, "archon");
        assert_eq!(alias.canonical, "github.com/mengsig/archon");
        assert!(alias.is_valid());

        for (old, new) in [("archon", " archon"), ("", "x"), ("x", "a\nb")] {
            assert!(
                RepositoryAlias::new(old, new, at).is_err(),
                "{old:?} -> {new:?}"
            );
        }
        let forged = RepositoryAlias {
            canonical: "archon".into(),
            ..alias.clone()
        };
        assert!(!forged.is_valid(), "an alias of itself read back from disk");
    }

    #[test]
    fn the_alias_event_names_both_ids_and_what_moved() {
        let alias = RepositoryAlias::new("archon", "github.com/mengsig/archon", crate::time::now())
            .unwrap();
        let counts = RepositoryAliasCounts {
            repository: 2,
            repository_role: 1,
            duplicates: 1,
            agents: 3,
        };
        let draft = alias.event(
            &counts,
            Actor::User {
                name: None,
                origin: None,
            },
        );
        let EventKind::Custom { name, payload } = draft.kind else {
            panic!("an alias is recorded as a custom event");
        };
        assert_eq!(name, ALIASED_EVENT);
        assert_eq!(payload["alias"], "archon");
        assert_eq!(payload["canonical"], "github.com/mengsig/archon");
        assert_eq!(payload["moved"]["repository_role"], 1);
        assert_eq!(counts.memories(), 3);
    }
}
