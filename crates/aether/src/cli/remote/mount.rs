//! `aether remote mount`: the listener prefixes forwarded to daemons on this machine.
//! Host-only, like the rest of `aether remote`; the daemon checks Funnel before adding one.

use std::path::PathBuf;

use aether_protocol::remote::{MountKind, RemoteMount, RemoteSettings, validate_mounts};
use anyhow::bail;

use super::requests;
use crate::args::MountCmd;
use crate::cli::{acted, connect, sane};

pub(super) async fn run(socket: PathBuf, command: MountCmd) -> anyhow::Result<()> {
    match command {
        MountCmd::Add {
            prefix,
            target,
            kind,
            json,
        } => {
            let mount = RemoteMount {
                prefix,
                kind: kind.into(),
                target,
            };
            add(socket, mount, json).await
        }
        MountCmd::Remove { prefix, json } => remove(socket, &prefix, json).await,
        MountCmd::List { json } => list(socket, json).await,
    }
}

async fn add(socket: PathBuf, mount: RemoteMount, json: bool) -> anyhow::Result<()> {
    let client = connect(&socket).await?;
    let current = requests::status(&client).await?;
    let mounts = with(saved(&current.settings), mount.clone())?;
    let settings = RemoteSettings {
        mounts: Some(mounts),
        ..current.settings
    };
    let status = requests::configure(&client, settings, current.revision).await?;
    let applied = saved(&status.settings);
    anyhow::ensure!(
        applied.contains(&mount),
        "the daemon answered without the mount"
    );
    let line = format!("mounted {} on {}", sane(&mount.prefix), sane(&mount.target));
    acted(json, &line, serde_json::to_value(&status)?)
}

async fn remove(socket: PathBuf, prefix: &str, json: bool) -> anyhow::Result<()> {
    let client = connect(&socket).await?;
    let current = requests::status(&client).await?;
    let mounts = without(saved(&current.settings), prefix)?;
    let settings = RemoteSettings {
        mounts: Some(mounts),
        ..current.settings
    };
    let status = requests::configure(&client, settings, current.revision).await?;
    let applied = saved(&status.settings);
    anyhow::ensure!(
        applied.iter().all(|mount| mount.prefix != prefix),
        "the daemon answered with {prefix} still mounted"
    );
    acted(
        json,
        &format!("unmounted {}", sane(prefix)),
        serde_json::to_value(&status)?,
    )
}

async fn list(socket: PathBuf, json: bool) -> anyhow::Result<()> {
    let client = connect(&socket).await?;
    let mounts = saved(&requests::status(&client).await?.settings);
    if json {
        println!("{}", serde_json::to_string_pretty(&mounts)?);
        return Ok(());
    }
    if mounts.is_empty() {
        println!("no mounts (add one with `aether remote mount add /archon --target <URL>`)");
        return Ok(());
    }
    print!("{}", table(&mounts));
    Ok(())
}

fn saved(settings: &RemoteSettings) -> Vec<RemoteMount> {
    settings.mounts.clone().unwrap_or_default()
}

/// `mounts` with `mount` added; a prefix already mounted is refused, not replaced.
fn with(mut mounts: Vec<RemoteMount>, mount: RemoteMount) -> anyhow::Result<Vec<RemoteMount>> {
    if let Some(taken) = mounts.iter().find(|saved| saved.prefix == mount.prefix) {
        bail!(
            "{} is already mounted on {}; remove it first",
            sane(&taken.prefix),
            sane(&taken.target)
        );
    }
    mounts.push(mount);
    validate_mounts(&mounts).map_err(anyhow::Error::msg)?;
    Ok(mounts)
}

fn without(mut mounts: Vec<RemoteMount>, prefix: &str) -> anyhow::Result<Vec<RemoteMount>> {
    let before = mounts.len();
    mounts.retain(|mount| mount.prefix != prefix);
    if mounts.len() == before {
        bail!(
            "{} is not mounted; `aether remote mount list` shows the mounts",
            sane(prefix)
        );
    }
    debug_assert_eq!(mounts.len() + 1, before, "prefixes are unique");
    Ok(mounts)
}

fn kind_label(kind: MountKind) -> &'static str {
    match kind {
        MountKind::Archon => "archon",
    }
}

fn table(mounts: &[RemoteMount]) -> String {
    let mut out = format!("{:<34}  {:<8}  target\n", "prefix", "kind");
    for mount in mounts {
        let prefix = sane(&mount.prefix);
        let kind = kind_label(mount.kind);
        out.push_str(&format!(
            "{prefix:<34}  {kind:<8}  {}\n",
            sane(&mount.target)
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mount(prefix: &str, port: u16) -> RemoteMount {
        RemoteMount {
            prefix: prefix.into(),
            kind: MountKind::Archon,
            target: format!("http://127.0.0.1:{port}"),
        }
    }

    #[test]
    fn a_mount_is_added_once_and_checked_before_it_is_sent() {
        let one = with(vec![], mount("/archon", 8378)).expect("added");
        assert_eq!(one, [mount("/archon", 8378)]);
        let again = with(one.clone(), mount("/archon", 9000))
            .unwrap_err()
            .to_string();
        assert!(
            again.contains("already mounted on http://127.0.0.1:8378"),
            "{again}"
        );
        let outside = RemoteMount {
            target: "http://192.0.2.1:80".into(),
            ..mount("/other", 1)
        };
        assert!(with(one.clone(), outside).is_err());
        let four = ["/a", "/b", "/c"].iter().fold(one, |all, prefix| {
            with(all, mount(prefix, 1)).expect("four fit")
        });
        assert!(
            with(four, mount("/e", 1))
                .unwrap_err()
                .to_string()
                .contains("at most 4")
        );
    }

    #[test]
    fn removing_names_a_mounted_prefix() {
        let both = vec![mount("/archon", 8378), mount("/b", 1)];
        assert_eq!(
            without(both.clone(), "/b").expect("removed"),
            [mount("/archon", 8378)]
        );
        let missing = without(both, "/nope").unwrap_err().to_string();
        assert!(missing.contains("is not mounted"), "{missing}");
    }

    #[test]
    fn the_table_shows_prefix_kind_and_target() {
        let table = table(&[mount("/archon", 8378)]);
        let rows: Vec<&str> = table.lines().collect();
        assert_eq!(rows.len(), 2);
        assert!(rows[1].starts_with("/archon"), "{table}");
        assert!(
            rows[1].ends_with("archon    http://127.0.0.1:8378"),
            "{table}"
        );
    }
}
