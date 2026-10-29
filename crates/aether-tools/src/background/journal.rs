//! Per-store ownership records for Linux host jobs surviving a daemon crash.
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Path, PathBuf};

use aether_core::id::ProcessId;
use nix::sys::signal::{Signal, killpg};
use nix::unistd::{Pid, geteuid};
use serde::{Deserialize, Serialize};

pub(crate) const JOB_ENV: &str = "AETHER_BACKGROUND_JOB";

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
    pub fn recover(directory: PathBuf) -> io::Result<Self> {
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
        for entry in std::fs::read_dir(&journal.directory)? {
            let path = entry?.path();
            if path
                .file_name()
                .is_some_and(|name| name.as_encoded_bytes().starts_with(b".tmp"))
            {
                std::fs::remove_file(path)?;
            } else if path.extension().is_some_and(|ext| ext == "json") {
                journal.recover_record(&path, Path::new("/proc"))?;
            }
        }
        Ok(journal)
    }

    fn recover_record(&self, path: &Path, proc_root: &Path) -> io::Result<()> {
        let bytes = std::fs::read(path)
            .map_err(|e| io::Error::new(e.kind(), format!("reading {}: {e}", path.display())))?;
        // Only a host crash between publish and the deferred fsync leaves this,
        // and that crash killed the group too.
        if bytes.is_empty() {
            tracing::warn!(record = %path.display(), "removing empty background record left by a host crash");
            return std::fs::remove_file(path);
        }
        let record: Record = serde_json::from_slice(&bytes).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid background record {}: {e}", path.display()),
            )
        })?;
        match self.matches(&record, proc_root) {
            Ok(true) => {
                tracing::warn!(pgid = record.pgid, record = %path.display(), "killing orphan background process group");
                match killpg(Pid::from_raw(record.pgid), Signal::SIGKILL) {
                    Ok(()) | Err(nix::errno::Errno::ESRCH) => {}
                    Err(error) => {
                        tracing::warn!(pgid = record.pgid, %error, "cannot kill orphan group; retaining record");
                        return Ok(());
                    }
                }
            }
            Ok(false) => {}
            Err(error) => {
                tracing::warn!(pgid = record.pgid, %error, "cannot prove background group ownership; retaining record");
                return Ok(());
            }
        }
        if record.boot_id == self.boot_id
            && record.uid == geteuid().as_raw()
            && let Some(job) = record.job
            && let Err(error) = kill_job_at(job, proc_root)
        {
            tracing::warn!(%job, %error, "cannot kill escaped background descendants; retaining record");
            return Ok(());
        }
        std::fs::remove_file(path)
    }

    pub fn register(&self, id: ProcessId, pgid: u32) -> io::Result<(Registration, Durability)> {
        let stat = std::fs::read(format!("/proc/{pgid}/stat"))?;
        let identity = identity(&stat)?;
        if identity.group != pgid as i32 || identity.group <= 1 {
            return Err(io::Error::other(
                "background child is not its process group leader",
            ));
        }
        let record = Record {
            pgid: identity.group,
            start_time: identity.start_time,
            boot_id: self.boot_id.clone(),
            uid: geteuid().as_raw(),
            job: Some(id),
        };
        let path = self.directory.join(format!("{id}.json"));
        let mut file = tempfile::NamedTempFile::new_in(&self.directory)?;
        file.write_all(&serde_json::to_vec(&record)?)?;
        let file = file.persist_noclobber(&path).map_err(|e| e.error)?;
        let directory = std::fs::File::open(&self.directory)?;
        Ok((Registration(path), Durability { file, directory }))
    }

    fn matches(&self, record: &Record, proc_root: &Path) -> io::Result<bool> {
        if record.pgid <= 1 || record.boot_id != self.boot_id || record.uid != geteuid().as_raw() {
            return Ok(false);
        }
        let leader_missing =
            match std::fs::read(proc_root.join(record.pgid.to_string()).join("stat")) {
                Ok(stat) => {
                    let identity = identity(&stat)?;
                    if identity.group != record.pgid || identity.start_time != record.start_time {
                        return Ok(false);
                    }
                    false
                }
                Err(e) if disappeared(&e) => true,
                Err(e) => return Err(e),
            };
        let token = record.job.map(|job| format!("{JOB_ENV}={job}"));
        let mut found = false;
        for entry in std::fs::read_dir(proc_root)? {
            let entry = entry?;
            if entry.file_name().to_string_lossy().parse::<u32>().is_err() {
                continue;
            }
            let member = match std::fs::read(entry.path().join("stat")) {
                Ok(stat) => identity(&stat)?,
                Err(e) if disappeared(&e) => continue,
                Err(e) => return Err(e),
            };
            if member.group != record.pgid {
                continue;
            }
            let metadata = match entry.metadata() {
                Ok(metadata) => metadata,
                Err(e) if disappeared(&e) => continue,
                Err(e) => return Err(e),
            };
            if metadata.uid() != record.uid {
                return Ok(false);
            }
            // A vanished leader could belong to a reused group. Every live
            // member must still carry this job's token before we signal it.
            if leader_missing && member.running() {
                let environment = match aether_spawn::read_environ(&entry.path()) {
                    Ok(environment) => environment,
                    Err(e) if disappeared(&e) => continue,
                    Err(e) => return Err(e),
                };
                if token.as_ref().is_none_or(|token| {
                    !environment
                        .split(|b| *b == 0)
                        .any(|entry| entry == token.as_bytes())
                }) {
                    tracing::warn!(pgid = record.pgid, member = %entry.path().display(), "skipping group: background job token missing");
                    return Ok(false);
                }
            }
            found |= member.running();
        }
        Ok(found)
    }
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
    for entry in std::fs::read_dir(proc_root)? {
        let entry = entry?;
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<i32>() else {
            continue;
        };
        if pid <= 1 || pid == std::process::id() as i32 {
            continue;
        }
        let metadata = match entry.metadata() {
            Ok(metadata) => metadata,
            Err(error) if disappeared(&error) => continue,
            Err(error) => return Err(error),
        };
        if metadata.uid() != geteuid().as_raw() {
            continue;
        }
        if readable_environ(&entry.path())?.is_some_and(|environment| swept(&environment, token)) {
            members.push(pid);
        }
    }
    Ok(members)
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
    for entry in std::fs::read_dir(proc_root)? {
        let entry = entry?;
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<i32>() else {
            continue;
        };
        if pid <= 1 || pid == std::process::id() as i32 {
            continue;
        }
        if signalled_in_group(&entry.path(), pgid)? {
            members.push(pid);
        }
    }
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
    let start_time = fields.get(19).and_then(|s| s.parse().ok());
    let state = fields.first().and_then(|s| s.chars().next());
    match (group, start_time, state) {
        (Some(group), Some(start_time), Some(state)) => Ok(Identity {
            group,
            start_time,
            state,
        }),
        _ => Err(io::Error::other("invalid process group or start time")),
    }
}

#[cfg(test)]
mod tests;
