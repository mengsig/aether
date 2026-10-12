# Installing, updating and uninstalling

Aether is one binary, `aether`. Each release carries it prebuilt for:

| Platform | Release target |
| --- | --- |
| Linux, x86_64 | `x86_64-unknown-linux-musl` |
| Linux, aarch64 | `aarch64-unknown-linux-musl` |
| macOS, Apple silicon | `aarch64-apple-darwin` |
| macOS, Intel | `x86_64-apple-darwin` |

The Linux binaries are linked statically against musl, so they need no
particular glibc. Windows is not supported natively: install inside WSL 2. Any
other platform [builds from source](#building-from-source).

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/mengsig/aether/main/install.sh | sh
```

The script downloads the archive for your platform from the latest release,
checks its SHA-256 against the published one, and installs `aether` into
`~/.local/bin`. It needs no `sudo`, edits no shell profile, and never touches
Aether's configuration or data. It ends by printing the installed version,
whether the directory is on your `PATH` (and the one line to add if it is not),
and the next command to run.

It needs `curl` or `wget`, `tar`, and `sha256sum` or `shasum`. Without a tool to
check the SHA-256 it refuses to install. A proxy named by `https_proxy` is used.

To read the script before running it:

```sh
curl -fsSL https://raw.githubusercontent.com/mengsig/aether/main/install.sh -o install.sh
less install.sh
sh install.sh
```

### Settings

| Variable | Meaning | Default |
| --- | --- | --- |
| `AETHER_VERSION` | Release tag to install, such as `v1.0.0` | the latest release |
| `AETHER_INSTALL_DIR` | Directory the binary goes into (an absolute path) | `$HOME/.local/bin` |
| `AETHER_INSTALL_BASE_URL` | Download base, for a mirror | `https://github.com/mengsig/aether/releases` |

A variable goes on the `sh` side of the pipe:

```sh
curl -fsSL https://raw.githubusercontent.com/mengsig/aether/main/install.sh | AETHER_VERSION=v1.0.0 sh
curl -fsSL https://raw.githubusercontent.com/mengsig/aether/main/install.sh | AETHER_INSTALL_DIR="$HOME/bin" sh
```

Running the installer again replaces the binary in place, by a rename, so an
`aether` that is running keeps working until it exits.

A mirror serves the same layout as GitHub: `<base>/latest/download/<asset>` and
`<base>/download/<tag>/<asset>`. It must be `https://`; plain `http://` is
accepted only for `127.0.0.1`, `localhost` and `[::1]`. `aether update` reads
the same variable.

### What is installed

Two files, both in the install directory:

- `aether`, the binary;
- `.aether-receipt`, a two-line text file that records that `install.sh` put the
  binary there. `aether update` replaces only a binary that has it.

## Verifying a download by hand

Every release has, for each target, `aether-<target>.tar.gz` and
`aether-<target>.tar.gz.sha256`, plus one `SHA256SUMS` covering all four
archives and `VERSION`, which holds the tag. An archive holds the binary,
`aether`, with Aether's `LICENSE` and `THIRD-PARTY-NOTICES`, the licences of
the code compiled into it. The installer and `aether update` unpack the binary
only.

```sh
target=x86_64-unknown-linux-musl
base=https://github.com/mengsig/aether/releases/latest/download
curl -fsSLO "$base/aether-$target.tar.gz"
curl -fsSLO "$base/aether-$target.tar.gz.sha256"
sha256sum -c "aether-$target.tar.gz.sha256"   # macOS: shasum -a 256 -c
tar -xzf "aether-$target.tar.gz" aether
install -m 755 aether "$HOME/.local/bin/aether"
```

A binary installed this way has no receipt, so `aether update` leaves it alone;
repeat these steps to update it, or run the installer over it once.

Releases built while the repository is public also carry a build provenance
attestation, which ties each archive to the workflow run that built it:

```sh
gh attestation verify "aether-$target.tar.gz" --repo mengsig/aether
```

## Updating

```sh
aether update                    # install the latest release, if it is newer
aether update --check            # only report; changes nothing
aether update --version v1.2.3   # install exactly this release
```

`aether update` downloads the same assets as the installer and applies the same
SHA-256 check. It then runs the downloaded binary once and requires it to report
the expected version before renaming it over the installed one. A failed
download, a checksum mismatch or a binary that does not run leaves the installed
binary as it was.

`--version` with an older release is a downgrade, and the command says so. An
older build may refuse a store that a newer one has already migrated; the
snapshot taken before that migration is described in
[Migration backups](migration-backups.md).

| Exit code | Meaning |
| --- | --- |
| 0 | Updated, or nothing to do. With `--check`: up to date. An update whose staging directory could not be removed afterwards also exits 0, with a warning on stderr that names the directory. |
| 1 | An error. The installed binary is unchanged. |
| 2 | Invalid command-line arguments, such as a `--version` that is not a release tag. |
| 10 | Only with `--check`: a newer release exists. |

With `--json` the outcome is one object on stdout and no progress lines; the
exit codes are the same. `--check --json` prints `current`, `latest`,
`update_available` and `update_command` (the command that installs the newer
release, or `null` when there is none). An update prints `current`, `version` and `action` (`none`,
`upgraded` or `downgraded`), and after replacing the binary also `binary`,
`daemon_running` and `restart`, the commands that restart a running daemon.

The network is used only while this command runs: one request for the latest
release's `VERSION`, then the checksum and the archive of that release. Nothing
checks for updates in the background and nothing is reported anywhere. Every
request has a deadline.

### Binaries it will not replace

`aether update` replaces the running binary only when `install.sh` installed it
(the receipt is beside it) and you can write to its directory. Otherwise it
changes nothing and prints the command to use instead, and `aether update
--check` names that command too when a newer release exists:

- a binary Cargo installed (`~/.cargo/bin`, listed in Cargo's `.crates.toml`):
  `cargo install --git https://github.com/mengsig/aether --locked aether`.
  Cargo's record counts even when the installer's receipt is in the same
  directory;
- a binary from a package manager or copied by hand: update it the way it was
  installed, or run the installer;
- a directory you cannot write to: run the installer, which installs into a
  directory you own.

### Restarting the daemon

Replacing the binary does not replace a daemon that is already running: it stays
the old build until it is stopped. After an update, `aether update` says whether
one is running and prints the commands:

```sh
aether stop
aether
```

Agents resume when the daemon starts again.

A client and a daemon from different builds talk only when their protocol
versions allow it: a daemon serves its own protocol version and a few before
it, never a newer one, and refuses any other client ([Protocol](protocol.md)).
So when a release changes the protocol version, the new `aether` cannot ask
the old daemon to shut down. `aether stop` then signals that daemon instead,
which needs Linux 6.5 or newer.

On macOS and on older Linux kernels there is no such signal, so `aether update`
looks at the daemon's socket before it downloads anything. If a daemon is
running, it installs nothing, exits with code 1 and prints the two commands to
run, while the installed build is still the one that daemon accepts:

```sh
aether stop
aether update
```

The same happens on any system when the socket is there but cannot be reached or
does not answer. With nothing to install, a running daemon is no obstacle.

The installer does not make this check. If a binary was replaced another way
while its daemon ran, and `aether stop` reports that it cannot signal that
daemon, end the daemon's process yourself: its process ID is kept beside the
socket, in `aetherd.pid`.

## Uninstalling

Stop the daemon if one is running, then remove the two installed files:

```sh
aether stop
rm "$HOME/.local/bin/aether" "$HOME/.local/bin/.aether-receipt"
```

Use your own directory if you set `AETHER_INSTALL_DIR`, and remove the `PATH`
line if you added one for it.

That leaves your configuration and data, which the installer and `aether update`
never touch:

| | Linux | macOS | Override |
| --- | --- | --- | --- |
| Configuration and credentials | `~/.config/aether` | `~/Library/Application Support/aether` | `AETHER_CONFIG_DIR` |
| Store, artifacts and logs | `~/.local/share/aether` | `~/Library/Application Support/aether` | `AETHER_DATA_DIR` |
| Daemon socket and PID file | `$XDG_RUNTIME_DIR/aether`, or the data directory when that is unset | the data directory | `AETHER_SOCKET` |

On Linux, `XDG_CONFIG_HOME` and `XDG_DATA_HOME` move the first two. Removing
these directories deletes agents, history, artifacts and stored credentials for
good; keep them to pick up where you left off after reinstalling.

## Building from source

Building needs Rust and a C compiler; SQLite and the TLS library are compiled
from source. The repository pins its toolchain in `rust-toolchain.toml`
(currently 1.98.1), and [rustup](https://rustup.rs) installs that version by
itself the first time `cargo` runs in the checkout. The oldest Rust that builds
Aether is 1.90.

```sh
git clone https://github.com/mengsig/aether.git
cd aether
cargo install --path crates/aether --locked   # or: make install
```

Cargo installs into `~/.cargo/bin`. Update such a build by pulling and
installing again; `aether update` leaves it alone.

If Cargo stops at once with

```text
`resolver` setting `3` is not valid, valid options are "1" or "2"
```

then the `cargo` on your `PATH` is older than the workspace supports, usually
one packaged by a Linux distribution. `cargo --version` shows it. Install
rustup, make sure `~/.cargo/bin` comes first on your `PATH`, and run the command
again: rustup then uses the pinned toolchain.
