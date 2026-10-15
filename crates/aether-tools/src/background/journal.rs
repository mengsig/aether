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
                let environment = match std::fs::read(entry.path().join("environ")) {
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

fn kill_job_at(job: ProcessId, proc_root: &Path) -> io::Result<()> {
    use rustix::process::{Pid as ProcessPid, PidfdFlags, pidfd_open, pidfd_send_signal};
    let token = format!("{JOB_ENV}={job}");
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
        let Some(environment) = readable_environ(&entry.path())? else {
            continue;
        };
        if carries(&environment, &token) {
            // A pidfd prevents signalling an unrelated process after PID reuse.
            let Some(pid) = ProcessPid::from_raw(pid) else {
                continue;
            };
            let process = match pidfd_open(pid, PidfdFlags::empty()) {
                Ok(process) => process,
                Err(rustix::io::Errno::SRCH) => continue,
                Err(error) => return Err(error.into()),
            };
            let Some(environment) = readable_environ(&entry.path())? else {
                continue;
            };
            if carries(&environment, &token) {
                match pidfd_send_signal(&process, rustix::process::Signal::KILL) {
                    Ok(()) | Err(rustix::io::Errno::SRCH) => {}
                    Err(error) => return Err(error.into()),
                }
            }
        }
    }
    Ok(())
}

/// A scanned process's environment, or None once it is gone or unreadable.
fn readable_environ(process: &Path) -> io::Result<Option<Vec<u8>>> {
    match std::fs::read(process.join("environ")) {
        Ok(environment) => Ok(Some(environment)),
        Err(error) if disappeared(&error) => Ok(None),
        // Same-uid non-dumpable processes are unreadable and aren't proven ours. Before
        // Linux 7.2 an exiting process's environ is root-owned too, so this also means gone.
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => Ok(None),
        Err(error) => Err(error),
    }
}

fn carries(environment: &[u8], token: &str) -> bool {
    environment
        .split(|byte| *byte == 0)
        .any(|entry| entry == token.as_bytes())
}

pub(crate) fn group_is_running(pgid: Pid) -> io::Result<bool> {
    for entry in std::fs::read_dir("/proc")? {
        let entry = entry?;
        if entry.file_name().to_string_lossy().parse::<u32>().is_err() {
            continue;
        }
        let member = match std::fs::read(entry.path().join("stat")) {
            Ok(stat) => identity(&stat)?,
            Err(e) if disappeared(&e) => continue,
            Err(e) => return Err(e),
        };
        if member.group == pgid.as_raw() && member.running() {
            return Ok(true);
        }
    }
    Ok(false)
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
