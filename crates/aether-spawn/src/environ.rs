//! Reads a process's environment from `/proc`, waiting out an `exec` under way.

use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

/// How long a process may stay inside `exec` before a read taken in it stands.
const EXEC_BUDGET: Duration = Duration::from_millis(50);
const EXEC_POLL: Duration = Duration::from_millis(1);
/// `PF_EXITING` and `PF_KTHREAD` in a stat's flags: neither will gain an environment.
const NO_ENVIRONMENT_COMING: u64 = 0x0000_0004 | 0x0020_0000;
/// The `start_code` a stat shows a reader it hides the image from, as for a non-dumpable process.
const HIDDEN_CODE: u64 = 1;

/// Where a process's environment lies, as its stat shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Image {
    /// The environment's start and end in a settled image.
    Settled(u64, u64),
    /// Inside `exec`: the new image is not set up yet.
    Exec,
    /// Gone, a zombie, exiting, a kernel thread, hidden, or a stat not to be read.
    Other,
}

/// The environment of the process whose `/proc` directory is `process`.
/// Inside `exec` a read comes back empty, torn or refused, so it is taken only
/// when the same settled image shows before and after it, and bash splits
/// each variable's name from its value in turn as it starts, so a split read
/// is taken again.
/// One still unsettled after 50 ms is [`io::ErrorKind::ResourceBusy`]: neither
/// a tag nor its absence is proven. A vfork child (`posix_spawn`) reads as its
/// parent until its exec swaps images.
pub fn read_environ(process: &Path) -> io::Result<Vec<u8>> {
    assert!(
        process
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.parse::<u32>().is_ok()),
        "not a process directory: {}",
        process.display()
    );
    let deadline = Instant::now() + EXEC_BUDGET;
    loop {
        let before = image(process);
        let read = std::fs::read(process.join("environ"));
        let after = image(process);
        let stable = match (before, after) {
            (Image::Settled(..), Image::Settled(..)) => before == after,
            (Image::Exec, _) | (_, Image::Exec) => false,
            _ => true,
        };
        if stable && !read.as_deref().is_ok_and(has_split_entry) {
            return read;
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::ResourceBusy,
                format!(
                    "{} did not settle within {EXEC_BUDGET:?}",
                    process.display()
                ),
            ));
        }
        std::thread::sleep(EXEC_POLL);
    }
}

/// Whether an entry is a bare variable name, as one bash is reading while it
/// starts is; a program that wrote its title over the region leaves other text.
fn has_split_entry(environment: &[u8]) -> bool {
    environment.split(|byte| *byte == 0).any(is_identifier)
}

fn is_identifier(entry: &[u8]) -> bool {
    let Some((first, rest)) = entry.split_first() else {
        return false;
    };
    (first.is_ascii_alphabetic() || *first == b'_')
        && rest
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
}

/// Where the environment of the process at `process` lies.
fn image(process: &Path) -> Image {
    let Ok(stat) = std::fs::read(process.join("stat")) else {
        return Image::Other;
    };
    // The command name may hold spaces and parentheses; fields resume after the last ')'.
    let Some(end) = stat.iter().rposition(|byte| *byte == b')') else {
        return Image::Other;
    };
    let Ok(tail) = std::str::from_utf8(&stat[end + 1..]) else {
        return Image::Other;
    };
    // proc(5) numbers the state 3, the flags 9, `start_code` 26, `env_start` 50 and `env_end` 51.
    let fields: Vec<&str> = tail.split_whitespace().collect();
    let number = |field: usize| {
        fields
            .get(field - 3)
            .and_then(|value| value.parse::<u64>().ok())
    };
    let state = fields.first().copied();
    let code = number(26);
    match (state, number(9), number(50), number(51)) {
        (Some("Z" | "X") | None, ..) => Image::Other,
        (_, Some(flags), ..) if flags & NO_ENVIRONMENT_COMING != 0 => Image::Other,
        _ if code == Some(HIDDEN_CODE) => Image::Other,
        // `exec` sets the environment's bounds while it fills it in, and the code's after that.
        (_, Some(_), Some(_), Some(end)) if end == 0 || code == Some(0) => Image::Exec,
        (_, Some(_), Some(start), Some(end)) => Image::Settled(start, end),
        _ => Image::Other,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// A stat line of the given state, flags, code start and environment
    /// bounds, the rest zero.
    fn stat_line(state: &str, flags: u64, start_code: u64, env: (u64, u64)) -> String {
        let mut fields = vec!["0".to_string(); 50];
        fields[0] = state.into();
        fields[6] = flags.to_string();
        fields[23] = start_code.to_string();
        fields[47] = env.0.to_string();
        fields[48] = env.1.to_string();
        format!("7 (a (b) c) {}\n", fields.join(" "))
    }

    fn process(root: &Path, stat: &str, environ: &[u8]) -> std::path::PathBuf {
        let dir = root.join("7");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("stat"), stat).unwrap();
        std::fs::write(dir.join("environ"), environ).unwrap();
        dir
    }

    #[test]
    fn only_a_live_user_process_without_an_environment_is_inside_exec() {
        let root = tempfile::tempdir().unwrap();
        let image_of = |stat: &str| image(&process(root.path(), stat, b""));
        assert_eq!(image_of(&stat_line("R", 0, 0, (0, 0))), Image::Exec);
        let filling = stat_line("R", 0, 0, (4096, 4096));
        assert_eq!(image_of(&filling), Image::Exec, "bounds set, code not yet");
        let settled = stat_line("S", 0, 1024, (4096, 4096));
        assert_eq!(image_of(&settled), Image::Settled(4096, 4096), "env -i");
        assert_eq!(image_of(&stat_line("Z", 0, 0, (0, 0))), Image::Other);
        assert_eq!(
            image_of(&stat_line("S", 0x0020_0000, 0, (0, 0))),
            Image::Other
        );
        assert_eq!(
            image_of(&stat_line("R", 0x0000_0004, 0, (0, 0))),
            Image::Other
        );
        assert_eq!(image_of("7 (short) R 1"), Image::Other);
        let no_bounds = format!("7 (short) R 1{}", " 0".repeat(30));
        assert_eq!(
            image_of(&no_bounds),
            Image::Other,
            "a stat too short to show bounds"
        );
        let non_dumpable = stat_line("S", 0, HIDDEN_CODE, (0, 0));
        assert_eq!(
            image_of(&non_dumpable),
            Image::Other,
            "hidden, not inside exec"
        );
    }

    #[test]
    fn an_entry_without_its_equals_sign_is_split() {
        assert!(has_split_entry(b"A\x001\0B=2\0"));
        assert!(has_split_entry(b"A=1\0_LONE2"));
        assert!(!has_split_entry(b"A=1\0B=\0\0C=x=y\0"));
        assert!(!has_split_entry(b""));
        let title = b"-appDir /usr/lib/firefox 2 socket\0\0tPid 12940\0";
        assert!(!has_split_entry(title), "a title written over the region");
        assert!(!has_split_entry(b"1A\0"), "not a name bash would read");
    }

    #[test]
    fn a_process_that_never_settles_is_unsettled_once_the_budget_is_spent() {
        let root = tempfile::tempdir().unwrap();
        let settled = process(root.path(), &stat_line("S", 0, 1024, (8, 12)), b"A=1\0");
        assert_eq!(read_environ(&settled).unwrap(), b"A=1\0");
        let stuck = process(root.path(), &stat_line("D", 0, 0, (0, 0)), b"");
        let started = Instant::now();
        let error = read_environ(&stuck).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::ResourceBusy);
        assert!(started.elapsed() >= EXEC_BUDGET, "gave up early");
        let split = process(root.path(), &stat_line("S", 0, 1024, (8, 12)), b"A\x001\0");
        let error = read_environ(&split).unwrap_err();
        assert_eq!(
            error.kind(),
            io::ErrorKind::ResourceBusy,
            "a split entry stays split"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_running_process_shows_where_its_environment_lies() {
        let own = image(&Path::new("/proc").join(std::process::id().to_string()));
        assert!(
            matches!(own, Image::Settled(start, end) if 0 < start && start < end),
            "this test's own environment bounds: {own:?}"
        );
    }

    /// Every settled read of a process that execs over and over, as a sweep
    /// may catch one, shows its environment; a busy one proves nothing and is
    /// taken again. It is spawned as Aether spawns tagged children, through
    /// `pre_exec`, so not by vfork.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_process_read_as_it_execs_shows_its_environment() {
        const REEXEC: &str =
            r#"if [ "$1" -lt 300 ]; then exec sh -c "$0" "$0" $(($1 + 1)); fi; exec sleep 5"#;
        let mut command = std::process::Command::new("sh");
        command
            .args(["-c", REEXEC, REEXEC, "0"])
            .env("AETHER_ENVIRON_PROBE", "1");
        crate::reset_signals(&mut command);
        let mut child = command.spawn().unwrap();
        let dir = Path::new("/proc").join(child.id().to_string());
        let deadline = Instant::now() + Duration::from_secs(20);
        let (mut settled, mut busy) = (0, 0);
        let missed = loop {
            match read_environ(&dir) {
                Err(error) if error.kind() == io::ErrorKind::ResourceBusy => busy += 1,
                read => {
                    let environment = read.unwrap();
                    settled += 1;
                    let tagged = environment
                        .split(|byte| *byte == 0)
                        .any(|entry| entry == b"AETHER_ENVIRON_PROBE=1");
                    if !tagged {
                        break Some(String::from_utf8_lossy(&environment).into_owned());
                    }
                }
            }
            let done =
                std::fs::read_to_string(dir.join("comm")).is_ok_and(|comm| comm == "sleep\n");
            if done || Instant::now() >= deadline {
                break None;
            }
        };
        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(
            missed, None,
            "the tag was missed mid-exec on settled read {settled} ({busy} busy)"
        );
        assert!(
            settled > 1,
            "only {settled} read(s) settled ({busy} busy) before the child stopped exec'ing"
        );
    }
}
