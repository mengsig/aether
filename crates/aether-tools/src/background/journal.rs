//! Per-store ownership records for Linux host jobs surviving a daemon crash.
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Path, PathBuf};

use std::collections::HashSet;

use aether_core::id::{AgentId, ProcessId};
use nix::sys::signal::{Signal, killpg};
use nix::unistd::{Pid, geteuid};
use serde::{Deserialize, Serialize};

pub(crate) const JOB_ENV: &str = "AETHER_BACKGROUND_JOB";
/// Carries a foreground call's id to everything it runs, so a group whose
/// leader is gone is still proven the call's.
pub(crate) const CALL_ENV: &str = "AETHER_FOREGROUND_CALL";

#[derive(Debug)]
pub(crate) struct Journal {
    directory: PathBuf,
    boot_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct Record {
    pgid: i32,
    start_time: u64,
    boot_id: String,
    uid: u32,
    #[serde(default)]
    job: Option<ProcessId>,
    /// A foreground call's id: recovery stops its group, not what it detached.
    #[serde(default)]
    call: Option<ProcessId>,
    /// The group's session: a group never leaves it, so once the leader is
    /// gone a member in another session is not the recorded group's.
    #[serde(default)]
    session: Option<i32>,
    /// The agent that ran a foreground call: it waits for inspection when
    /// recovery cannot prove what is left of the call its own.
    #[serde(default)]
    agent: Option<AgentId>,
}

/// What recovery found of a recorded group.
#[derive(Debug, PartialEq, Eq)]
enum Found {
    /// Its leader, or a member carrying its token, runs.
    Recorded,
    /// Leaderless members run in its session but none carries its token: the
    /// call's, its token cleared, or a later group that reused the id.
    Unproven,
    /// Nothing of it runs.
    Nothing,
}

impl Record {
    /// The environment entry every process of the recorded job or call carries.
    fn token(&self) -> Option<String> {
        match (self.job, self.call) {
            (Some(job), _) => Some(format!("{JOB_ENV}={job}")),
            (None, Some(call)) => Some(format!("{CALL_ENV}={call}")),
            (None, None) => None,
        }
    }
}

pub(crate) struct Registration(PathBuf);

pub(crate) struct Durability {
    file: std::fs::File,
    directory: std::fs::File,
}

impl Durability {
    pub fn sync(self) -> io::Result<()> {
        self.file.sync_all()?;
        self.directory.sync_all()
    }
}

impl Registration {
    pub fn clear(self) -> io::Result<()> {
        std::fs::remove_file(self.0)
    }
}

impl Journal {
    /// Stop what a crashed daemon's recorded groups left running. Returns the
    /// agents whose foreground call could not be proven stopped or spared.
    pub fn recover(directory: PathBuf) -> io::Result<(Self, HashSet<AgentId>)> {
        match std::fs::DirBuilder::new().mode(0o700).create(&directory) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
        let metadata = std::fs::symlink_metadata(&directory)?;
        if !metadata.is_dir()
            || metadata.uid() != geteuid().as_raw()
            || metadata.mode() & 0o077 != 0
        {
            return Err(io::Error::other(
                "background journal must be a private directory owned by this user",
            ));
        }
        let journal = Self {
            directory,
            boot_id: std::fs::read_to_string("/proc/sys/kernel/random/boot_id")?
                .trim()
                .to_owned(),
        };
        let mut unstopped = HashSet::new();
        for entry in std::fs::read_dir(&journal.directory)? {
            let path = entry?.path();
            if path
                .file_name()
                .is_some_and(|name| name.as_encoded_bytes().starts_with(b".tmp"))
            {
                std::fs::remove_file(path)?;
            } else if path.extension().is_some_and(|ext| ext == "json") {
                unstopped.extend(journal.recover_record(&path, Path::new("/proc"))?);
            }
        }
        Ok((journal, unstopped))
    }

    /// Recover the group recorded at `path`; the agent of a foreground call
    /// whose leftovers recovery spared unproven, which must not resume beside them.
    fn recover_record(&self, path: &Path, proc_root: &Path) -> io::Result<Option<AgentId>> {
        let bytes = std::fs::read(path)
            .map_err(|e| io::Error::new(e.kind(), format!("reading {}: {e}", path.display())))?;
        // Only a host crash between publish and the deferred fsync leaves this,
        // and that crash killed the group too.
        if bytes.is_empty() {
            tracing::warn!(record = %path.display(), "removing empty background record left by a host crash");
            std::fs::remove_file(path)?;
            return Ok(None);
        }
        let record: Record = serde_json::from_slice(&bytes).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid background record {}: {e}", path.display()),
            )
        })?;
        let stopped = self.matches(&record, proc_root).and_then(|(found, foreign)| {
            if found == Found::Recorded {
                tracing::warn!(pgid = record.pgid, record = %path.display(), "killing orphan host process group");
                kill_recorded_group(&record, proc_root)?;
            }
            Ok((found, foreign))
        });
        let (found, foreign) = match stopped {
            Ok(stopped) => stopped,
            Err(error) => return Self::unstoppable(&record, error),
        };
        if record.boot_id == self.boot_id
            && record.uid == geteuid().as_raw()
            && let Some(job) = record.job
            && let Err(error) = kill_job_at(job, proc_root)
        {
            tracing::warn!(%job, %error, "cannot kill escaped background descendants; retaining record");
            return Ok(None);
        }
        std::fs::remove_file(path)?;
        // Another user's member (a `sudo` child) outlives any kill of ours.
        let parked = record
            .agent
            .filter(|_| (found == Found::Unproven || foreign) && record.call.is_some());
        if let Some(agent) = parked {
            tracing::warn!(pgid = record.pgid, %agent, "its interrupted call may still run: the agent waits for inspection");
        }
        Ok(parked)
    }

    /// Refuse the start over a foreground call that could not be stopped; keep
    /// a job's record for the next start.
    fn unstoppable(record: &Record, error: io::Error) -> io::Result<Option<AgentId>> {
        assert!(record.pgid > 1, "not a recorded group: {}", record.pgid);
        assert!(
            record.job.is_none() || record.call.is_none(),
            "a record is a job's or a call's"
        );
        // An agent resumes only once its interrupted call is stopped: refuse the start.
        if record.call.is_some() {
            return Err(io::Error::new(
                error.kind(),
                format!(
                    "cannot stop the foreground call a crashed daemon left in process group {pgid}: {error}; \
                     an agent resumed beside it could race it. Stop that group (kill -KILL -- -{pgid}) and start again",
                    pgid = record.pgid
                ),
            ));
        }
        tracing::warn!(pgid = record.pgid, %error, "cannot stop orphan background group; retaining record");
        Ok(None)
    }

    pub fn register(&self, id: ProcessId, pgid: u32) -> io::Result<(Registration, Durability)> {
        let (registration, file) = self.publish(id, pgid, Some(id), None, None)?;
        let directory = std::fs::File::open(&self.directory)?;
        Ok((registration, Durability { file, directory }))
    }

    /// Record the group of `agent`'s foreground `call`, whose processes carry
    /// [`CALL_ENV`], so a start after a crash stops the call but not what it
    /// detached. Unsynced: a host crash ends the group too.
    pub fn register_foreground(
        &self,
        call: ProcessId,
        pgid: u32,
        agent: AgentId,
    ) -> io::Result<Registration> {
        let (registration, _file) = self.publish(call, pgid, None, Some(call), Some(agent))?;
        Ok(registration)
    }

    /// Publish the record of group `pgid` as `name`; recovery also stops what
    /// a `job` left outside the group, but only the group of a `call`.
    fn publish(
        &self,
        name: ProcessId,
        pgid: u32,
        job: Option<ProcessId>,
        call: Option<ProcessId>,
        agent: Option<AgentId>,
    ) -> io::Result<(Registration, std::fs::File)> {
        assert!(
            job.is_none() || call.is_none(),
            "a record is a job's or a call's"
        );
        let stat = std::fs::read(format!("/proc/{pgid}/stat"))?;
        let identity = identity(&stat)?;
        if identity.group != pgid as i32 || identity.group <= 1 {
            return Err(io::Error::other(
                "host command is not its process group leader",
            ));
        }
        let record = Record {
            pgid: identity.group,
            start_time: identity.start_time,
            boot_id: self.boot_id.clone(),
            uid: geteuid().as_raw(),
            job,
            call,
            session: Some(identity.session),
            agent,
        };
        let path = self.directory.join(format!("{name}.json"));
        let mut file = tempfile::NamedTempFile::new_in(&self.directory)?;
        file.write_all(&serde_json::to_vec(&record)?)?;
        let file = file.persist_noclobber(&path).map_err(|e| e.error)?;
        Ok((Registration(path), file))
    }

    /// What runs of `record`'s group, and whether a member of another user,
    /// which no kill of ours can stop, runs in it.
    fn matches(&self, record: &Record, proc_root: &Path) -> io::Result<(Found, bool)> {
        if record.pgid <= 1 || record.boot_id != self.boot_id || record.uid != geteuid().as_raw() {
            return Ok((Found::Nothing, false));
        }
        let Some(leader_missing) = leader_missing(record, proc_root)? else {
            return Ok((Found::Nothing, false));
        };
        let token = record.token();
        let mut found = false;
        let mut unproven = false;
        let mut foreign = false;
        for entry in std::fs::read_dir(proc_root)? {
            let entry = entry?;
            if entry.file_name().to_string_lossy().parse::<u32>().is_err() {
                continue;
            }
            let Some((member, owner)) = group_member(&entry.path(), record.pgid)? else {
                continue;
            };
            // Another user's member (a `sudo` child) cannot be signalled, but
            // the rest of the group is still ours to stop.
            if owner != record.uid {
                foreign |= member.running();
                continue;
            }
            // A vanished leader could belong to a reused group: only a member
            // carrying the token proves it ours.
            if leader_missing && member.running() {
                match proves_leaderless(record, &entry.path(), &member, token.as_deref())? {
                    Some(Found::Recorded) => {}
                    Some(Found::Unproven) => {
                        unproven = true;
                        continue;
                    }
                    Some(Found::Nothing) | None => continue,
                }
            }
            found |= member.running();
        }
        if unproven && !found {
            tracing::warn!(
                pgid = record.pgid,
                "sparing a leaderless group in the recorded session none of whose members carries its token"
            );
        }
        if foreign && !found {
            tracing::error!(
                pgid = record.pgid,
                "a member of a recorded group runs as another user and cannot be stopped"
            );
        }
        let found = match (found, unproven) {
            (true, _) => Found::Recorded,
            (false, true) => Found::Unproven,
            (false, false) => Found::Nothing,
        };
        Ok((found, foreign))
    }
}

/// Whether `record`'s leader is gone; None when its pid now names another
/// process, so the group is not the recorded one.
fn leader_missing(record: &Record, proc_root: &Path) -> io::Result<Option<bool>> {
    assert!(record.pgid > 1, "not a recorded group: {}", record.pgid);
    let leader = match std::fs::read(proc_root.join(record.pgid.to_string()).join("stat")) {
        Ok(stat) => identity(&stat)?,
        Err(e) if disappeared(&e) => return Ok(Some(true)),
        Err(e) => return Err(e),
    };
    let recorded = leader.group == record.pgid && leader.start_time == record.start_time;
    Ok(recorded.then_some(false))
}

/// `process`'s identity and owner when it is in group `pgid`; None otherwise
/// or once it is gone. The owner is followed, so a test's proc root can name
/// another user's process.
fn group_member(process: &Path, pgid: i32) -> io::Result<Option<(Identity, u32)>> {
    assert!(pgid > 1, "not a recorded group: {pgid}");
    let member = match std::fs::read(process.join("stat")) {
        Ok(stat) => identity(&stat)?,
        Err(e) if disappeared(&e) => return Ok(None),
        Err(e) => return Err(e),
    };
    if member.group != pgid {
        return Ok(None);
    }
    match std::fs::metadata(process) {
        Ok(metadata) => Ok(Some((member, metadata.uid()))),
        Err(e) if disappeared(&e) => Ok(None),
        Err(e) => Err(e),
    }
}

/// What a running member of a group whose leader is gone proves of it: one
/// carrying the token, `record`'s group; one in another session, a later group
/// that reused the id; one in its session without the token, neither. None
/// once it is gone.
fn proves_leaderless(
    record: &Record,
    process: &Path,
    member: &Identity,
    token: Option<&str>,
) -> io::Result<Option<Found>> {
    assert!(member.running(), "only a running member proves a group");
    assert_eq!(
        member.group, record.pgid,
        "not a member of the recorded group"
    );
    if record
        .session
        .is_some_and(|session| member.session != session)
    {
        return Ok(Some(Found::Nothing));
    }
    Ok(carries_token(process, token)?.map(|carries| {
        if carries {
            Found::Recorded
        } else {
            Found::Unproven
        }
    }))
}

/// Whether `process` carries `token`; None once it is gone.
fn carries_token(process: &Path, token: Option<&str>) -> io::Result<Option<bool>> {
    let environment = match aether_spawn::read_environ(process) {
        Ok(environment) => environment,
        Err(e) if disappeared(&e) => return Ok(None),
        Err(e) => return Err(e),
    };
    Ok(Some(
        token.is_some_and(|token| carries(&environment, token)),
    ))
}

/// SIGKILL the group of a `record` recovery proved ours. Through its leader,
/// stopped first so the group id stays its own until the signal lands, or,
/// once that leader is gone, member by member: each pinned and checked to be
/// this user's and, when recorded, in the group's session.
fn kill_recorded_group(record: &Record, proc_root: &Path) -> io::Result<()> {
    assert!(record.pgid > 1, "not a recorded group: {}", record.pgid);
    if kill_through_leader(record, proc_root)? {
        return Ok(());
    }
    let pgid = Pid::from_raw(record.pgid);
    for _ in 0..KILL_ROUNDS {
        let mut found = false;
        for_each_candidate(proc_root, |pid, _| {
            found |= signal_pinned(proc_root, pid, rustix::process::Signal::KILL, |process| {
                running_member(process, record)
            })?;
            Ok(())
        })?;
        if !found {
            return Ok(());
        }
        std::thread::sleep(JOB_POLL);
    }
    Err(io::Error::other(format!(
        "group {pgid} kept running through {KILL_ROUNDS} kill rounds"
    )))
}

/// SIGKILL group `record.pgid` while its recorded leader is held stopped; false
/// when that leader is gone or is no longer the process recorded.
fn kill_through_leader(record: &Record, proc_root: &Path) -> io::Result<bool> {
    use rustix::process::{Pid as ProcessPid, PidfdFlags, Signal as LeaderSignal};
    use rustix::process::{pidfd_open, pidfd_send_signal};

    let Some(raw) = ProcessPid::from_raw(record.pgid) else {
        return Ok(false);
    };
    let leader = match pidfd_open(raw, PidfdFlags::empty()) {
        Ok(leader) => leader,
        Err(rustix::io::Errno::SRCH) => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    // The pidfd pins the process checked here; the stop keeps it from exiting.
    if !is_recorded_leader(record, proc_root, false)? {
        return Ok(false);
    }
    match pidfd_send_signal(&leader, LeaderSignal::STOP) {
        Ok(()) => {}
        Err(rustix::io::Errno::SRCH) => return Ok(false),
        Err(error) => return Err(error.into()),
    }
    let stopped = wait_recorded_leader_stopped(record, proc_root);
    if !matches!(stopped, Ok(true)) {
        // It may still be ours: resume it and keep the record for the next start.
        let resumed = pidfd_send_signal(&leader, LeaderSignal::CONT);
        let error = stopped.err().unwrap_or_else(|| {
            io::Error::other(format!("group {} leader did not stop", record.pgid))
        });
        return match resumed {
            Ok(()) | Err(rustix::io::Errno::SRCH) => Err(error),
            Err(errno) => Err(io::Error::other(format!(
                "{error}; resuming the stopped leader {} failed: {errno}",
                record.pgid
            ))),
        };
    }
    let killed = killpg(Pid::from_raw(record.pgid), Signal::SIGKILL);
    // The group's SIGKILL took the leader already; this covers a failed killpg.
    match pidfd_send_signal(&leader, LeaderSignal::KILL) {
        Ok(()) | Err(rustix::io::Errno::SRCH) => {}
        Err(error) => return Err(error.into()),
    }
    killed.map_err(io::Error::from)?;
    Ok(true)
}

/// Whether `record.pgid` is still the recorded leader, and when `stopped`,
/// held stopped.
fn is_recorded_leader(record: &Record, proc_root: &Path, stopped: bool) -> io::Result<bool> {
    let stat = match std::fs::read(proc_root.join(record.pgid.to_string()).join("stat")) {
        Ok(stat) => stat,
        Err(e) if disappeared(&e) => return Ok(false),
        Err(e) => return Err(e),
    };
    let leader = identity(&stat)?;
    let recorded = leader.group == record.pgid && leader.start_time == record.start_time;
    let state_ok = if stopped {
        leader.state == 'T'
    } else {
        leader.running()
    };
    Ok(recorded && state_ok)
}

/// Poll up to a second until the recorded leader shows as stopped.
fn wait_recorded_leader_stopped(record: &Record, proc_root: &Path) -> io::Result<bool> {
    for _ in 0..100 {
        if is_recorded_leader(record, proc_root, true)? {
            return Ok(true);
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    Ok(false)
}

/// Whether `process` is a live member of `record`'s group this user can stop,
/// in the recorded session when there is one.
fn running_member(process: &Path, record: &Record) -> io::Result<bool> {
    let Some((member, owner)) = group_member(process, record.pgid)? else {
        return Ok(false);
    };
    let in_session = record
        .session
        .is_none_or(|session| member.session == session);
    Ok(member.running() && owner == record.uid && in_session)
}

pub(crate) fn kill_job(job: ProcessId) -> io::Result<()> {
    kill_job_at(job, Path::new("/proc"))
}

/// How many SIGKILL rounds catch job processes forked while the last landed.
const KILL_ROUNDS: usize = 5;
const JOB_POLL: std::time::Duration = std::time::Duration::from_millis(50);

/// SIGTERM what `job` left running outside its process group, including what
/// it forks meanwhile, then SIGKILL whatever of it outlives `grace`.
pub(crate) async fn stop_job(job: ProcessId, grace: std::time::Duration) -> io::Result<()> {
    use rustix::process::Signal as JobSignal;
    let proc_root = Path::new("/proc");
    let token = format!("{JOB_ENV}={job}");
    let deadline = tokio::time::Instant::now() + grace;
    let mut terminated: Vec<i32> = Vec::new();
    loop {
        let members = job_members(proc_root, &token)?;
        if members.is_empty() {
            return Ok(());
        }
        for pid in members {
            if !terminated.contains(&pid) && signal_member(proc_root, pid, &token, JobSignal::TERM)?
            {
                terminated.push(pid);
            }
        }
        if tokio::time::Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(JOB_POLL).await;
    }
    for _ in 0..KILL_ROUNDS {
        if !signal_job_at(proc_root, &token, JobSignal::KILL)? {
            return Ok(());
        }
        tokio::time::sleep(JOB_POLL).await;
    }
    let survivors = job_members(proc_root, &token)?;
    if survivors.is_empty() {
        return Ok(());
    }
    Err(io::Error::other(format!(
        "background job {job} processes survived SIGKILL: {survivors:?}"
    )))
}

fn kill_job_at(job: ProcessId, proc_root: &Path) -> io::Result<()> {
    let token = format!("{JOB_ENV}={job}");
    signal_job_at(proc_root, &token, rustix::process::Signal::KILL).map(drop)
}

/// Send `signal` to each live process of this user carrying `token`; true
/// when there was one.
fn signal_job_at(
    proc_root: &Path,
    token: &str,
    signal: rustix::process::Signal,
) -> io::Result<bool> {
    let mut found = false;
    for pid in job_members(proc_root, token)? {
        found |= signal_member(proc_root, pid, token, signal)?;
    }
    Ok(found)
}

/// The live processes of this user under `proc_root` carrying `token`,
/// without init and this daemon.
fn job_members(proc_root: &Path, token: &str) -> io::Result<Vec<i32>> {
    assert!(token.starts_with(JOB_ENV), "not a job token: {token}");
    let mut members = Vec::new();
    for_each_candidate(proc_root, |pid, process| {
        let metadata = match std::fs::symlink_metadata(process) {
            Ok(metadata) => metadata,
            Err(error) if disappeared(&error) => return Ok(()),
            Err(error) => return Err(error),
        };
        if metadata.uid() == geteuid().as_raw()
            && readable_environ(process)?.is_some_and(|environment| swept(&environment, token))
        {
            members.push(pid);
        }
        Ok(())
    })?;
    Ok(members)
}

/// Visit each process under `proc_root` with its /proc entry, but init and
/// this daemon, as it is listed: one forked meanwhile is still visited.
fn for_each_candidate(
    proc_root: &Path,
    mut visit: impl FnMut(i32, &Path) -> io::Result<()>,
) -> io::Result<()> {
    let own = std::process::id();
    aether_spawn::visit_processes(proc_root, |pid| {
        if pid <= 1 || pid == own {
            return Ok(());
        }
        let pid =
            i32::try_from(pid).map_err(|_| io::Error::other(format!("pid {pid} out of range")))?;
        visit(pid, &proc_root.join(pid.to_string()))
    })
}

/// Signal `pid` through a pidfd opened before its `token` is checked again,
/// so a pid reused since the scan is never hit; false once it is gone.
fn signal_member(
    proc_root: &Path,
    pid: i32,
    token: &str,
    signal: rustix::process::Signal,
) -> io::Result<bool> {
    signal_pinned(proc_root, pid, signal, |process| {
        Ok(readable_environ(process)?.is_some_and(|environment| swept(&environment, token)))
    })
}

/// Signal `pid` through a pidfd opened before `still` checks its /proc entry
/// again, so a pid reused since the scan is never hit; false once it is gone.
fn signal_pinned(
    proc_root: &Path,
    pid: i32,
    signal: rustix::process::Signal,
    still: impl Fn(&Path) -> io::Result<bool>,
) -> io::Result<bool> {
    use rustix::process::{Pid as ProcessPid, PidfdFlags, pidfd_open, pidfd_send_signal};
    assert!(pid > 1, "not a job process: {pid}");
    assert_ne!(
        pid,
        std::process::id() as i32,
        "the daemon never signals itself"
    );
    let Some(raw) = ProcessPid::from_raw(pid) else {
        return Ok(false);
    };
    let process = match pidfd_open(raw, PidfdFlags::empty()) {
        Ok(process) => process,
        Err(rustix::io::Errno::SRCH) => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    if !still(&proc_root.join(pid.to_string()))? {
        return Ok(false);
    }
    match pidfd_send_signal(&process, signal) {
        Ok(()) => Ok(true),
        Err(rustix::io::Errno::SRCH) => Ok(false),
        Err(error) => Err(error.into()),
    }
}

/// A scanned process's environment, or None once it is gone or unreadable.
fn readable_environ(process: &Path) -> io::Result<Option<Vec<u8>>> {
    match aether_spawn::read_environ(process) {
        Ok(environment) => Ok(Some(environment)),
        Err(error) if disappeared(&error) => Ok(None),
        // Same-uid non-dumpable processes are unreadable and aren't proven ours. Before
        // Linux 7.2 an exiting process's environ is root-owned too, so this also means gone.
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => Ok(None),
        // Still inside `exec`: neither the token nor the keep mark is proven.
        Err(error) if error.kind() == io::ErrorKind::ResourceBusy => Ok(None),
        Err(error) => Err(error),
    }
}

/// Whether a process with `environment` is one of the job's that a sweep
/// signals: it carries `token` and was not kept past its owner.
fn swept(environment: &[u8], token: &str) -> bool {
    carries(environment, token) && !carries(environment, &keep_entry())
}

/// The environment entry that keeps a process past its owner.
fn keep_entry() -> String {
    format!("{}=1", aether_core::agent::PROCESS_KEEP_ENV)
}

fn carries(environment: &[u8], token: &str) -> bool {
    environment
        .split(|byte| *byte == 0)
        .any(|entry| entry == token.as_bytes())
}

/// SIGTERM the members of a job's group `pgid`, then SIGKILL what outlives
/// `grace`. Its shell is unreaped, so the group id is not reused meanwhile;
/// each member is signalled on its own, so one kept is spared.
pub(crate) async fn stop_pinned_group(pgid: Pid, grace: std::time::Duration) -> io::Result<()> {
    use rustix::process::Signal as JobSignal;
    assert!(pgid.as_raw() > 1, "not a job's group: {pgid}");
    let proc_root = Path::new("/proc");
    let deadline = tokio::time::Instant::now() + grace;
    let mut terminated = std::collections::HashSet::new();
    // Each round sends SIGTERM to members new since the last, as one forked
    // during the grace is owed the same chance; the first round always runs.
    loop {
        let members = pinned_group_members(proc_root, pgid)?;
        if members.is_empty() {
            return Ok(());
        }
        let fresh: Vec<i32> = members
            .into_iter()
            .filter(|pid| !terminated.contains(pid))
            .collect();
        signal_members(proc_root, pgid, &fresh, JobSignal::TERM)?;
        terminated.extend(fresh);
        if tokio::time::Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(JOB_POLL).await;
    }
    for _ in 0..KILL_ROUNDS {
        let members = pinned_group_members(proc_root, pgid)?;
        if !signal_members(proc_root, pgid, &members, JobSignal::KILL)? {
            return Ok(());
        }
        tokio::time::sleep(JOB_POLL).await;
    }
    let survivors = pinned_group_members(proc_root, pgid)?;
    if survivors.is_empty() {
        return Ok(());
    }
    Err(io::Error::other(format!(
        "group {pgid} processes survived SIGKILL: {survivors:?}"
    )))
}

/// Send `signal` to each of `pids` still a member of `pgid` that may be
/// signalled; true when there was one.
fn signal_members(
    proc_root: &Path,
    pgid: Pid,
    pids: &[i32],
    signal: rustix::process::Signal,
) -> io::Result<bool> {
    let mut found = false;
    for pid in pids {
        found |= signal_pinned(proc_root, *pid, signal, |process| {
            signalled_in_group(process, pgid)
        })?;
    }
    Ok(found)
}

/// The live members of `pgid` that may be signalled, without init and this daemon.
fn pinned_group_members(proc_root: &Path, pgid: Pid) -> io::Result<Vec<i32>> {
    let mut members = Vec::new();
    for_each_candidate(proc_root, |pid, process| {
        if signalled_in_group(process, pgid)? {
            members.push(pid);
        }
        Ok(())
    })?;
    Ok(members)
}

/// Whether `process` is a live member of `pgid` whose environment is
/// readable and not marked to be kept; one that cannot be read may be kept.
fn signalled_in_group(process: &Path, pgid: Pid) -> io::Result<bool> {
    let member = match std::fs::read(process.join("stat")) {
        Ok(stat) => identity(&stat)?,
        Err(e) if disappeared(&e) => return Ok(false),
        Err(e) => return Err(e),
    };
    if member.group != pgid.as_raw() || !member.running() {
        return Ok(false);
    }
    // The leader is the job's own shell, or what it exec'd: stopping the job
    // stops it, kept or not, and it is reaped only after the stop.
    if process.file_name() == Some(pgid.as_raw().to_string().as_ref()) {
        return Ok(true);
    }
    Ok(readable_environ(process)?.is_some_and(|environment| !carries(&environment, &keep_entry())))
}

fn disappeared(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::NotFound || error.raw_os_error() == Some(nix::libc::ESRCH)
}

struct Identity {
    group: i32,
    session: i32,
    start_time: u64,
    state: char,
}

impl Identity {
    fn running(&self) -> bool {
        !matches!(self.state, 'Z' | 'X')
    }
}

fn identity(stat: &[u8]) -> io::Result<Identity> {
    let end = stat
        .iter()
        .rposition(|byte| *byte == b')')
        .ok_or_else(|| io::Error::other("invalid process stat"))?;
    let tail = std::str::from_utf8(&stat[end + 1..])
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let fields: Vec<_> = tail.split_whitespace().collect();
    let group = fields.get(2).and_then(|s| s.parse().ok());
    let session = fields.get(3).and_then(|s| s.parse().ok());
    let start_time = fields.get(19).and_then(|s| s.parse().ok());
    let state = fields.first().and_then(|s| s.chars().next());
    match (group, session, start_time, state) {
        (Some(group), Some(session), Some(start_time), Some(state)) => Ok(Identity {
            group,
            session,
            start_time,
            state,
        }),
        _ => Err(io::Error::other("invalid process group or start time")),
    }
}

#[cfg(test)]
mod tests;
