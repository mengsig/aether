//! What `HEAD` names and its distance from upstream, via commands that read refs and commits only.
//! They run with the repository's config, so none may touch the index or the work tree.

use aether_protocol::ErrorReply;

use super::{ObjectFormat, Repo, failed, finish, object_name, unexpected};

const BRANCHES: &str = "refs/heads/";
/// One branch per line: its name, commit, upstream and distance from it.
const BRANCH_FORMAT: &str =
    "--format=%(refname)%00%(objectname)%00%(upstream)%00%(upstream:track,nobracket)";

/// The branch or bare commit `HEAD` is at.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Head {
    pub branch: Option<String>,
    /// The commit's full object name; `None` before the first commit.
    pub commit: Option<String>,
    pub detached: bool,
    pub ahead: Option<u32>,
    pub behind: Option<u32>,
}

impl Repo<'_> {
    pub(super) async fn read_head(&self, format: ObjectFormat) -> Result<Head, ErrorReply> {
        match self.head_ref().await? {
            Some(reference) => self.branch_head(&reference, format).await,
            None => self.detached_head(format).await,
        }
    }

    /// The ref `HEAD` points at; `None` when it is detached.
    async fn head_ref(&self) -> Result<Option<String>, ErrorReply> {
        let mut command = self.in_repository();
        command.args(["symbolic-ref", "-q", "HEAD"]);
        let run = finish(command).await?;
        if run.success {
            let reference = run.stdout.trim_end_matches('\n');
            // Anything else could be read as an option by the next command.
            if !reference.starts_with("refs/") || reference.contains('\n') {
                return Err(unexpected("symbolic-ref"));
            }
            return Ok(Some(reference.to_owned()));
        }
        // Exit 1 with nothing said is how `symbolic-ref -q` says detached.
        match run.stderr.trim().is_empty() {
            true => Ok(None),
            false => Err(failed("symbolic-ref", &run)),
        }
    }

    /// `HEAD` on the branch `reference`, which has no commit yet when no
    /// such ref exists.
    async fn branch_head(&self, reference: &str, format: ObjectFormat) -> Result<Head, ErrorReply> {
        let mut command = self.in_repository();
        command.args(["for-each-ref", BRANCH_FORMAT, reference]);
        let run = finish(command).await?;
        if !run.success {
            return Err(failed("for-each-ref", &run));
        }
        let name = reference.strip_prefix(BRANCHES).unwrap_or(reference);
        let mut head = Head {
            branch: Some(name.to_owned()),
            ..Head::default()
        };
        // The pattern also matches refs below `reference`; only it counts.
        let found = run.stdout.lines().find_map(|line| {
            let mut fields = line.split('\0');
            (fields.next() == Some(reference)).then_some(fields)
        });
        let Some(mut fields) = found else {
            return Ok(head);
        };
        let (Some(commit), Some(upstream), Some(track)) =
            (fields.next(), fields.next(), fields.next())
        else {
            return Err(unexpected("for-each-ref"));
        };
        head.commit = Some(object_name(commit, format, "for-each-ref")?);
        if !upstream.is_empty() {
            let distance = distance(track)?;
            (head.ahead, head.behind) = (distance.map(|d| d.0), distance.map(|d| d.1));
        }
        Ok(head)
    }

    async fn detached_head(&self, format: ObjectFormat) -> Result<Head, ErrorReply> {
        let mut command = self.in_repository();
        command.args(["rev-parse", "-q", "--verify", "HEAD^{commit}"]);
        let run = finish(command).await?;
        if !run.success {
            return Err(failed("rev-parse HEAD", &run));
        }
        let commit = object_name(run.stdout.trim_end(), format, "rev-parse HEAD")?;
        Ok(Head {
            commit: Some(commit),
            detached: true,
            ..Head::default()
        })
    }
}

/// Commits ahead of and behind an upstream, from `ahead 3, behind 1`, either
/// part alone, or nothing when level. `None` when the upstream is `gone`.
pub(super) fn distance(track: &str) -> Result<Option<(u32, u32)>, ErrorReply> {
    if track == "gone" {
        return Ok(None);
    }
    let (mut ahead, mut behind) = (0, 0);
    for part in track.split(", ").filter(|part| !part.is_empty()) {
        let counted = part
            .split_once(' ')
            .and_then(|(side, count)| Some((side, count.parse::<u32>().ok()?)));
        match counted {
            Some(("ahead", count)) => ahead = count,
            Some(("behind", count)) => behind = count,
            _ => return Err(unexpected("for-each-ref")),
        }
    }
    Ok(Some((ahead, behind)))
}
