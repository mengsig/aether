use std::sync::Arc;
use std::time::Duration;

use aether_core::agent::{
    Agent, AgentConfig, AgentStats, AgentStatus, BlockReason, Role, Scope, WaitCondition,
};
use aether_core::id::TaskId;
use aether_core::model::ModelRef;
use aether_store::{Commit, Mutation};

use super::*;

const REPOSITORY: &str = "github.com/example-org/example-repo";
const KIB: u64 = 1 << 10;
const DAY: Duration = Duration::from_secs(86_400);

/// A runtime with its own cache root and a budget of `max_bytes`.
struct Fixture {
    runtime: Arc<Runtime>,
    root: PathBuf,
    _config: tempfile::TempDir,
    _cache: tempfile::TempDir,
}

impl Fixture {
    async fn new(max_bytes: u64) -> Self {
        let config = tempfile::tempdir().unwrap();
        fs::write(
            config.path().join("config.toml"),
            format!("[storage]\nworkspaces_days = 0\n[build_cache]\nmax_bytes = {max_bytes}\n"),
        )
        .unwrap();
        let cache = tempfile::tempdir().unwrap();
        let root = cache.path().join("aether.build-cache");
        let configured = root.clone();
        let (runtime, config) = crate::test_runtime_in(config, |config| {
            config.build_cache_root = Some(configured);
        })
        .await;
        runtime.shutdown.cancel();
        tokio::task::yield_now().await;
        Self {
            runtime,
            root,
            _config: config,
            _cache: cache,
        }
    }

    /// Store an agent of a new run in a repository workspace, last active
    /// `days_ago`, whose partition got `kib` of build output then.
    fn agent(&self, status: AgentStatus, days_ago: u32, kib: u64) -> (Agent, PathBuf) {
        let then = SystemTime::now() - DAY * days_ago;
        let agent = Agent {
            id: AgentId::new(),
            parent: None,
            role: Role::new("worker"),
            name: "builder".into(),
            scope: Scope {
                repository: Some(REPOSITORY.into()),
                workspace_root: Some("/srv/workspace".into()),
                ..Scope::default()
            },
            model: ModelRef::new("mock", "test"),
            fallbacks: vec![],
            status,
            paused: false,
            paused_via: None,
            current_task: None,
            archived: false,
            inbox_cursor: 0,
            checkpoint: None,
            engine_session: None,
            engine_usage: None,
            engine_sent: None,
            status_since: Some(then.into()),
            resume_wait: None,
            restarted_from: None,
            restarted_as: None,
            config: AgentConfig::default(),
            stats: AgentStats::default(),
            run: RunId::new(),
            created_at: then.into(),
            updated_at: then.into(),
        };
        self.runtime
            .apply(Commit::new().mutate(Mutation::UpsertAgent(agent.clone())))
            .unwrap();
        crate::build_cache::claim(&self.runtime, &agent).unwrap();
        let partition = crate::build_cache::aether_partition(&self.root, &agent).unwrap();
        assert!(is_marked(&partition, PARTITION_MARKER), "claimed");
        build(&partition, kib, then);
        (agent, partition)
    }

    fn gc(&self, dry_run: bool) -> BuildCacheReport {
        self.runtime.gc_build_cache(&self.root, dry_run).unwrap()
    }
}

/// Write `kib` of output into `partition`'s cargo target, every level of it
/// last changed at `then`.
fn build(partition: &Path, kib: u64, then: SystemTime) {
    let target = partition.join("cargo-target");
    let debug = target.join("debug");
    fs::create_dir_all(&debug).unwrap();
    let artifact = debug.join("artifact");
    fs::write(&artifact, vec![7u8; usize::try_from(kib * KIB).unwrap()]).unwrap();
    let marker = partition.join(PARTITION_MARKER);
    for path in [&marker, &artifact, &debug, &target, partition] {
        if path.exists() {
            fs::File::open(path).unwrap().set_modified(then).unwrap();
        }
    }
}

/// Build in `partition` as an Aether partition: marked as `claim` marks one.
fn build_marked(partition: &Path, then: SystemTime) {
    fs::create_dir_all(partition).unwrap();
    fs::File::create_new(partition.join(PARTITION_MARKER)).unwrap();
    build(partition, 8, then);
}

fn built(partition: &Path) -> bool {
    partition.join("cargo-target/debug/artifact").is_file()
}

fn entry<'a>(report: &'a BuildCacheReport, path: &Path) -> &'a PartitionEntry {
    report
        .entries
        .iter()
        .find(|entry| entry.path == path)
        .unwrap_or_else(|| panic!("{} is not reported: {report:?}", path.display()))
}

#[tokio::test]
async fn over_budget_the_least_recently_used_unleased_partition_goes_first() {
    let f = Fixture::new(600 * KIB).await;
    let (holder, leased) = f.agent(AgentStatus::Running, 30, 256);
    let (_, oldest) = f.agent(AgentStatus::Completed { summary: None }, 20, 256);
    let (_, newer) = f.agent(AgentStatus::Completed { summary: None }, 10, 256);

    let preview = f.gc(true);
    assert_eq!(preview.max_bytes, Some(600 * KIB));
    assert!(preview.bytes > 600 * KIB, "{preview:?}");
    assert_eq!(entry(&preview, &oldest).action, "would_evict");
    assert_eq!(entry(&preview, &newer).action, "kept", "the rest fit");
    let leased_entry = entry(&preview, &leased);
    assert_eq!(leased_entry.action, "kept", "older, but leased");
    assert_eq!(leased_entry.leased_by, [holder.id]);
    assert!(
        leased_entry.last_used < entry(&preview, &newer).last_used,
        "{preview:?}"
    );
    assert!(built(&oldest), "a preview deletes nothing");

    let swept = f.gc(false);
    assert_eq!(entry(&swept, &oldest).action, "evicted");
    assert!(swept.errors.is_empty(), "{:?}", swept.errors);
    assert!(swept.bytes_removed >= 256 * KIB, "{swept:?}");
    assert!(!oldest.exists());
    assert!(built(&newer) && built(&leased));
    let trash: Vec<_> = fs::read_dir(f.root.join(TRASH))
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(
        trash,
        [TRASH_MARKER],
        "evicted output is deleted, not parked"
    );

    let again = f.gc(false);
    assert!(
        again.entries.iter().all(|entry| entry.action == "kept"),
        "within budget now: {again:?}"
    );
}

#[tokio::test]
async fn every_live_status_leases_its_partition_even_with_no_budget() {
    let f = Fixture::new(0).await;
    let live = [
        AgentStatus::Created,
        AgentStatus::Ready,
        AgentStatus::Running,
        AgentStatus::Waiting {
            condition: WaitCondition::Task { id: TaskId::new() },
        },
        AgentStatus::Blocked {
            reason: BlockReason::Dependency { on: TaskId::new() },
        },
    ];
    let live: Vec<_> = live.map(|status| f.agent(status, 90, 8)).into();
    let finished = [
        AgentStatus::Completed { summary: None },
        AgentStatus::Failed {
            error: "boom".into(),
        },
        AgentStatus::Cancelled { reason: None },
    ];
    let finished: Vec<_> = finished.map(|status| f.agent(status, 1, 8)).into();

    let swept = f.gc(false);
    for (agent, partition) in &live {
        let kept = entry(&swept, partition);
        assert_eq!(kept.action, "kept", "{:?}", agent.status);
        assert_eq!(kept.leased_by, [agent.id]);
        assert!(built(partition), "{:?}", agent.status);
    }
    for (agent, partition) in &finished {
        assert_eq!(
            entry(&swept, partition).action,
            "evicted",
            "{:?}",
            agent.status
        );
        assert!(!partition.exists(), "{:?}", agent.status);
    }
}

#[tokio::test]
async fn a_finished_agent_with_work_in_flight_still_leases_its_partition() {
    let f = Fixture::new(0).await;
    let (agent, partition) = f.agent(AgentStatus::Completed { summary: None }, 5, 8);
    f.runtime.sched.lock().engine_retakers.insert(agent.id);
    let swept = f.gc(false);
    assert_eq!(entry(&swept, &partition).leased_by, [agent.id]);
    assert!(built(&partition));

    f.runtime.sched.lock().engine_retakers.remove(&agent.id);
    assert_eq!(entry(&f.gc(false), &partition).action, "evicted");
    assert!(!partition.exists());
}

#[tokio::test]
async fn directories_aether_does_not_create_are_never_evicted() {
    let f = Fixture::new(0).await;
    let then = SystemTime::now() - DAY;
    let repository = f.root.join("github-com-org-repo-0123456789abcdef");
    let operators = f.root.join("my-target/debug");
    let not_a_run = repository.join("scratch");
    let foreign_repository = f.root.join(format!("project/{}", RunId::new()));
    let unmarked = repository.join(RunId::new().to_string());
    for dir in [&operators, &not_a_run, &foreign_repository, &unmarked] {
        build(dir, 8, then);
    }
    let orphan = repository.join(RunId::new().to_string());
    build_marked(&orphan, then);

    let swept = f.gc(false);
    for kept in [&operators, &not_a_run, &foreign_repository, &unmarked] {
        let seen = entry(&swept, kept);
        assert_eq!(
            (seen.action, seen.reason),
            ("kept", "not a partition Aether creates"),
            "{}",
            kept.display()
        );
        assert!(built(kept), "{}", kept.display());
    }
    assert_eq!(
        entry(&swept, &orphan).action,
        "evicted",
        "a marked partition whose agents left the store is still Aether's"
    );
}

#[tokio::test]
async fn a_partition_holding_a_client_named_cache_is_never_evicted() {
    let f = Fixture::new(0).await;
    let (_, named) = f.agent(AgentStatus::Completed { summary: None }, 5, 8);
    let (_, holds_one) = f.agent(AgentStatus::Completed { summary: None }, 5, 8);
    for (agent_dir, client_dir) in [
        (named.clone(), named.clone()),
        (holds_one.clone(), holds_one.join("cargo-target/items/one")),
    ] {
        let (mut client, _) = f.agent(AgentStatus::Running, 1, 0);
        client.config.build_cache = Some(client_dir);
        f.runtime
            .apply(Commit::new().mutate(Mutation::UpsertAgent(client)))
            .unwrap();
        assert!(agent_dir.exists());
    }

    let swept = f.gc(false);
    for kept in [&named, &holds_one] {
        let seen = entry(&swept, kept);
        assert_eq!(
            (seen.action, seen.reason),
            ("kept", "holds a client's build cache")
        );
        assert!(built(kept), "{}", kept.display());
    }
}

#[tokio::test]
async fn a_client_cache_named_through_a_link_still_protects_its_partition() {
    let f = Fixture::new(0).await;
    let (_, linked) = f.agent(AgentStatus::Completed { summary: None }, 5, 8);
    let (_, through_parent) = f.agent(AgentStatus::Completed { summary: None }, 5, 8);
    let links = f._cache.path().join("links");
    fs::create_dir(&links).unwrap();
    std::os::unix::fs::symlink(&linked, links.join("client-cache")).unwrap();
    std::os::unix::fs::symlink(&through_parent, links.join("parent")).unwrap();
    for client_dir in [
        links.join("client-cache"),
        links.join("parent/cargo-target/not-yet-made"),
    ] {
        let (mut client, _) = f.agent(AgentStatus::Running, 1, 0);
        client.config.build_cache = Some(client_dir);
        f.runtime
            .apply(Commit::new().mutate(Mutation::UpsertAgent(client)))
            .unwrap();
    }

    let swept = f.gc(false);
    for kept in [&linked, &through_parent] {
        assert_eq!(entry(&swept, kept).reason, "holds a client's build cache");
        assert!(built(kept), "{}", kept.display());
    }
}

#[test]
fn resolving_keeps_the_part_that_does_not_exist_yet() {
    let dir = tempfile::tempdir().unwrap();
    let real = fs::canonicalize(dir.path()).unwrap();
    std::os::unix::fs::symlink(&real, real.join("alias")).unwrap();
    assert_eq!(resolve(&real.join("alias/a/b")), real.join("a/b"));
    assert_eq!(resolve(&real.join("alias")), real);
    assert_eq!(resolve(Path::new("/")), Path::new("/"));
}

#[tokio::test]
async fn a_claim_marks_only_a_partition_it_creates() {
    let f = Fixture::new(0).await;
    let (agent, partition) = f.agent(AgentStatus::Running, 3, 8);
    let marker = partition.join(PARTITION_MARKER);
    let before = fs::metadata(&marker).unwrap().modified().unwrap();
    crate::build_cache::claim(&f.runtime, &agent).unwrap();
    let after = fs::metadata(&marker).unwrap().modified().unwrap();
    assert!(after > before, "a claim is the partition's last use");

    let mut other = agent.clone();
    other.run = RunId::new();
    let existing = crate::build_cache::aether_partition(&f.root, &other).unwrap();
    fs::create_dir_all(&existing).unwrap();
    crate::build_cache::claim(&f.runtime, &other).unwrap();
    assert!(
        !is_marked(&existing, PARTITION_MARKER),
        "a directory already there is not adopted"
    );
}

#[tokio::test]
async fn only_marked_partitions_in_aethers_own_trash_are_removed_later() {
    let f = Fixture::new(0).await;
    let trash = f.root.join(TRASH);
    let left = trash.join("01LEFTOVER");
    build_marked(&left, SystemTime::now());
    let unknown = trash.join("operator-files");
    build(&unknown, 8, SystemTime::now());

    let unmarked = f.gc(false);
    assert!(built(&left), "an unmarked trash is not Aether's");
    assert_eq!(unmarked.errors.len(), 1, "{:?}", unmarked.errors);

    fs::File::create_new(trash.join(TRASH_MARKER)).unwrap();
    let preview = f.gc(true);
    assert!(built(&left), "a preview deletes nothing");
    assert!(preview.entries.is_empty(), "the trash is not a partition");
    let swept = f.gc(false);
    assert!(!left.exists(), "an evicted partition left behind goes");
    assert!(built(&unknown), "anything else in the trash stays");
    assert_eq!(swept.errors.len(), 1, "{:?}", swept.errors);
    assert!(swept.errors[0].contains("not an evicted partition"));
}

#[tokio::test]
async fn a_leftover_holding_a_client_named_cache_stays_in_the_trash() {
    let f = Fixture::new(0).await;
    let trash = f.root.join(TRASH);
    let left = trash.join("01LEFTOVER");
    build_marked(&left, SystemTime::now());
    fs::File::create_new(trash.join(TRASH_MARKER)).unwrap();
    let links = f._cache.path().join("links");
    fs::create_dir(&links).unwrap();
    std::os::unix::fs::symlink(&left, links.join("client")).unwrap();
    let (mut client, _) = f.agent(AgentStatus::Completed { summary: None }, 1, 0);
    client.config.build_cache = Some(links.join("client/cargo-target"));
    f.runtime
        .apply(Commit::new().mutate(Mutation::UpsertAgent(client)))
        .unwrap();

    let swept = f.gc(false);
    assert!(built(&left), "a client's directory is never removed");
    assert!(
        swept.errors.iter().any(|e| e.contains("left in place")),
        "{:?}",
        swept.errors
    );
}

#[tokio::test]
async fn a_partly_failed_deletion_stays_marked_and_finishes_later() {
    use std::os::unix::fs::PermissionsExt as _;
    let f = Fixture::new(0).await;
    let (_, partition) = f.agent(AgentStatus::Completed { summary: None }, 5, 8);
    let sealed = partition.join("cargo-target/debug");
    fs::set_permissions(&sealed, fs::Permissions::from_mode(0o500)).unwrap();

    let failed = f.gc(false);
    assert_eq!(entry(&failed, &partition).action, "evicted");
    assert_eq!(failed.errors.len(), 1, "{:?}", failed.errors);
    let [left] = <[_; 1]>::try_from(
        fs::read_dir(f.root.join(TRASH))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| !path.ends_with(TRASH_MARKER))
            .collect::<Vec<_>>(),
    )
    .unwrap();
    assert!(is_marked(&left, PARTITION_MARKER), "the marker goes last");

    let sealed = left.join("cargo-target/debug");
    fs::set_permissions(&sealed, fs::Permissions::from_mode(0o700)).unwrap();
    let finished = f.gc(false);
    assert!(finished.errors.is_empty(), "{:?}", finished.errors);
    assert!(!left.exists());
}

#[tokio::test]
async fn a_partition_swapped_for_a_link_after_the_scan_is_never_followed() {
    let f = Fixture::new(0).await;
    let (_, partition) = f.agent(AgentStatus::Completed { summary: None }, 5, 8);
    let mut report = scan(&f.root).unwrap();
    let index = report
        .entries
        .iter()
        .position(|entry| entry.path == partition)
        .unwrap();
    let operator = f.root.parent().unwrap().join("operator-target");
    fs::rename(&partition, &operator).unwrap();
    fs::remove_file(operator.join(PARTITION_MARKER)).unwrap();
    std::os::unix::fs::symlink(&operator, &partition).unwrap();

    let moved = move_to_trash(&mut report, &[index], &f.root.join(TRASH));
    assert!(moved.is_empty(), "{moved:?}");
    assert_eq!(report.entries[index].action, "kept");
    assert_eq!(report.errors.len(), 1, "{:?}", report.errors);
    assert!(remove_partition(&partition).is_err(), "a link is refused");
    assert!(built(&operator), "the operator's files stay");
}

#[test]
fn a_trash_that_is_a_link_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("real");
    fs::create_dir(&real).unwrap();
    fs::File::create_new(real.join(TRASH_MARKER)).unwrap();
    let trash = dir.path().join(TRASH);
    std::os::unix::fs::symlink(&real, &trash).unwrap();
    assert!(open_trash(&trash, true).is_err());
    assert!(open_trash(&real, false).unwrap());
    assert!(!open_trash(&dir.path().join("missing"), false).unwrap());
}

#[test]
fn partitions_are_counted_and_links_never_followed() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("cache");
    let missing = scan(&root).unwrap();
    assert_eq!((missing.bytes, missing.partitions), (0, 0));

    let outside = dir.path().join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("big"), vec![1u8; 1 << 20]).unwrap();
    let run = root.join("repo-0123/run_one/cargo-target/debug");
    fs::create_dir_all(&run).unwrap();
    fs::write(run.join("artifact"), vec![1u8; 64 << 10]).unwrap();
    fs::create_dir_all(root.join("repo-0123/run_two")).unwrap();
    fs::create_dir_all(root.join("repo-4567")).unwrap();
    std::os::unix::fs::symlink(&outside, run.join("link")).unwrap();
    std::os::unix::fs::symlink(&outside, root.join("repo-0123/run_link")).unwrap();
    fs::write(root.join("stray"), b"not a repository").unwrap();

    let seen = scan(&root).unwrap();
    assert_eq!(seen.repositories, 2);
    assert_eq!(seen.partitions, 2, "a link is not a partition");
    assert!(seen.bytes >= 64 << 10, "{seen:?}");
    assert!(
        seen.bytes < 1 << 20,
        "the linked file is not counted: {seen:?}"
    );
    assert_eq!((seen.unreadable, seen.truncated), (0, false));
}

#[test]
fn the_budget_bounds_every_level_and_marks_the_report() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("cache");
    let partition = root.join("repo-0123/run_one");
    fs::create_dir_all(&partition).unwrap();
    for i in 0..10 {
        fs::write(partition.join(i.to_string()), b"x").unwrap();
    }
    let mut seen = scan(&root).unwrap();
    assert_eq!(
        (seen.budget, seen.truncated),
        (MAX_CACHE_ENTRIES - 12, false)
    );

    seen.budget = 5;
    seen.truncated = false;
    seen.add_tree(&partition);
    assert_eq!((seen.budget, seen.truncated), (0, true), "{seen:?}");
    assert!(
        seen.real_dirs(&root).is_empty(),
        "nothing is read past the bound"
    );
}

#[test]
fn a_root_that_is_not_a_directory_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("cache");
    fs::write(&root, b"file").unwrap();
    assert!(scan(&root).is_err());
}
