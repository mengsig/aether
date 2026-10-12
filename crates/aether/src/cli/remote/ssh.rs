//! SSH remotes: the names in `remotes.toml`, and which daemon a command
//! talks to once `--remote` and `--local` are taken into account.

use std::path::PathBuf;

use aether_client::Endpoint;

/// Refuse `--remote` typed beside `--socket` or `--local`. Not clap's
/// `conflicts_with`: that would also fire on a value from the environment.
pub(crate) fn check_selectors(matches: &clap::ArgMatches) -> anyhow::Result<()> {
    use clap::parser::ValueSource;
    let typed = |id: &str| matches.value_source(id) == Some(ValueSource::CommandLine);
    if typed("remote") && typed("socket") {
        anyhow::bail!("--remote and --socket name two different daemons; pass one");
    }
    if typed("remote") && typed("local") {
        anyhow::bail!("--remote and --local name two different daemons; pass one");
    }
    Ok(())
}

/// The daemon this invocation addresses. `--local` wins over a remote
/// named by the environment.
pub(crate) fn endpoint(
    socket: PathBuf,
    remote: Option<&str>,
    local: bool,
) -> anyhow::Result<Endpoint> {
    match remote {
        Some(name) if !local => {
            let profiles = load()?;
            let profile = profiles.remotes.get(name).cloned().ok_or_else(|| {
                anyhow::anyhow!(
                    "unknown remote `{name}`; add it with `aether remote add {name} --ssh TARGET`"
                )
            })?;
            Ok(Endpoint::Ssh(
                profile.validated(name).map_err(anyhow::Error::msg)?,
            ))
        }
        _ => Ok(Endpoint::Socket(socket)),
    }
}

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Profiles {
    #[serde(default)]
    remotes: std::collections::BTreeMap<String, aether_client::SshRemote>,
}

fn load() -> anyhow::Result<Profiles> {
    use anyhow::Context as _;
    let path = aether_protocol::paths::config_dir().join("remotes.toml");
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Profiles::default()),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    parse(&text)
}

fn parse(text: &str) -> anyhow::Result<Profiles> {
    use anyhow::Context as _;
    let mut profiles: Profiles = toml::from_str(text).context("invalid remotes.toml")?;
    for (name, remote) in &mut profiles.remotes {
        *remote = remote.clone().validated(name).map_err(anyhow::Error::msg)?;
    }
    Ok(profiles)
}

fn profile_lock() -> anyhow::Result<std::fs::File> {
    let dir = aether_protocol::paths::config_dir();
    aether_model::config::create_config_dir(&dir)?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("remotes.lock"))?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        match lock.try_lock() {
            Ok(()) => break,
            Err(std::fs::TryLockError::WouldBlock) => {
                anyhow::ensure!(
                    std::time::Instant::now() < deadline,
                    "remote profiles are busy in another process; retry after it exits"
                );
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            Err(std::fs::TryLockError::Error(error)) => return Err(error.into()),
        }
    }
    Ok(lock)
}

fn save(profiles: &Profiles) -> anyhow::Result<()> {
    use std::io::Write as _;
    let dir = aether_protocol::paths::config_dir();
    aether_model::config::create_config_dir(&dir)?;
    let mut temporary = tempfile::NamedTempFile::new_in(&dir)?;
    temporary.write_all(toml::to_string(profiles)?.as_bytes())?;
    temporary.as_file().sync_all()?;
    temporary.persist(dir.join("remotes.toml"))?;
    std::fs::File::open(dir)?.sync_all()?;
    Ok(())
}

pub(super) async fn add(
    name: String,
    target: String,
    executable: Option<String>,
    socket: Option<String>,
    json: bool,
) -> anyhow::Result<()> {
    let remote = aether_client::SshRemote::configured(
        &name,
        &target,
        executable.as_deref().unwrap_or("aether"),
        socket.as_deref(),
    )
    .map_err(anyhow::Error::msg)?;
    let mut profiles = load()?;
    anyhow::ensure!(
        !profiles.remotes.contains_key(&name),
        "remote `{name}` already exists; remove it before replacing it"
    );
    let client = aether_client::Client::connect_endpoint(&Endpoint::Ssh(remote.clone())).await?;
    let _lock = profile_lock()?;
    profiles = load()?;
    anyhow::ensure!(
        !profiles.remotes.contains_key(&name),
        "remote `{name}` was added by another client"
    );
    let welcome = client
        .welcome()
        .ok_or_else(|| anyhow::anyhow!("remote handshake returned no welcome"))?;
    profiles.remotes.insert(name.clone(), remote);
    save(&profiles)?;
    super::super::acted(
        json,
        &format!(
            "{name} · connected · {target} · protocol {}",
            welcome.protocol_version
        ),
        serde_json::json!({"name": name, "ssh": target, "protocol_version": welcome.protocol_version, "instance_id": welcome.instance_id}),
    )
}

pub(super) fn list(json: bool) -> anyhow::Result<()> {
    let profiles = load()?;
    if json {
        println!("{}", serde_json::to_string_pretty(&profiles)?);
    } else {
        for (name, remote) in profiles.remotes {
            println!("{name}\t{}", remote.target());
        }
    }
    Ok(())
}

pub(super) fn remove(name: String, json: bool) -> anyhow::Result<()> {
    let _lock = profile_lock()?;
    let mut profiles = load()?;
    anyhow::ensure!(
        profiles.remotes.remove(&name).is_some(),
        "unknown remote `{name}`"
    );
    save(&profiles)?;
    super::super::acted(
        json,
        &format!("removed remote {name}"),
        serde_json::json!({"removed": name}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_wins_over_a_remote_named_by_the_environment() {
        let socket = PathBuf::from("/run/aether/aetherd.sock");
        let here = endpoint(socket.clone(), Some("build"), true).expect("local");
        assert_eq!(here, Endpoint::Socket(socket.clone()));
        assert_eq!(
            endpoint(socket.clone(), None, false).expect("default"),
            Endpoint::Socket(socket.clone())
        );
        let refused = endpoint(socket, Some("build"), false).expect_err("not yet");
        assert!(refused.to_string().contains("unknown remote"), "{refused}");
    }

    #[test]
    fn a_typed_remote_conflicts_with_a_typed_socket_or_local_only() {
        use clap::CommandFactory as _;
        let parse = |line: &[&str]| {
            crate::args::Args::command()
                .try_get_matches_from(line)
                .expect("parses")
        };
        for line in [
            ["aether", "--remote", "build", "--local", "status"],
            ["aether", "--remote", "build", "--socket=/tmp/s", "status"],
            ["aether", "status", "--local", "--remote", "build"],
        ] {
            let refused = check_selectors(&parse(&line)).expect_err("two daemons named");
            assert!(refused.to_string().contains("--remote"), "{refused}");
        }
        check_selectors(&parse(&["aether", "--remote", "build", "status"])).expect("one daemon");
        check_selectors(&parse(&["aether", "--local", "status"])).expect("one daemon");
    }

    #[test]
    fn profiles_validate_paths_names_and_reject_secrets() {
        let profiles = parse(
            "[remotes.box]\nssh = 'me@box'\nexecutable = '/opt/aether'\nsocket = '/run/box.sock'\n",
        )
        .expect("profile");
        assert_eq!(profiles.remotes["box"].name(), "box");
        assert!(parse("[remotes.box]\nssh = 'host'\ntoken = 'secret'\n").is_err());
        assert!(parse("[remotes.Box]\nssh = 'host'\n").is_err());
    }
}
