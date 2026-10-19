use clap::{Subcommand, ValueEnum};

/// The role a device is paired with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum DeviceRoleArg {
    /// Everything an operator at the host can do, minus the host-only set.
    Operator,
    /// Read-only.
    Observer,
}

impl From<DeviceRoleArg> for aether_protocol::remote::DeviceRole {
    fn from(role: DeviceRoleArg) -> Self {
        match role {
            DeviceRoleArg::Operator => Self::Operator,
            DeviceRoleArg::Observer => Self::Observer,
        }
    }
}

#[derive(Debug, Subcommand)]
pub(crate) enum RemoteCmd {
    /// Guided setup: check Tailscale, enable the listener, pair a device.
    Setup {
        /// Accept every default and never wait for you; stops at anything
        /// only you can do. Pairing is left for `aether remote pair`.
        #[arg(long, short = 'y')]
        yes: bool,
        /// Print what is and is not set up as one object. Changes nothing
        /// unless --yes is also given.
        #[arg(long)]
        json: bool,
        /// An https:// address that already reaches the listener, for your
        /// own reverse proxy or another overlay. Skips the Tailscale steps.
        #[arg(long, value_name = "URL")]
        proxy_url: Option<String>,
        /// Publish on the public internet with Tailscale Funnel. Anyone can
        /// then reach the sign-in page; read the warning before using it.
        #[arg(long, conflicts_with = "proxy_url")]
        funnel: bool,
    },
    /// Show the listener, its public origins, paired devices and any open pairing.
    Status {
        /// Print the result as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Open the remote listener. Takes effect at once; no restart.
    Enable {
        /// An origin browsers reach this daemon by, e.g.
        /// https://box.tail1234.ts.net. Repeat for several; replaces the list.
        #[arg(long = "origin", value_name = "URL")]
        origins: Vec<String>,
        /// Leave pairing closed: paired devices connect, no new one enrols.
        #[arg(long)]
        no_pairing: bool,
        /// Send push notifications to devices that subscribed.
        #[arg(long, overrides_with = "no_push")]
        push: bool,
        /// Send no push notifications: the opposite of `--push`.
        #[arg(long = "no-push", overrides_with = "push")]
        no_push: bool,
        /// Print the result as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Close the remote listener and every device connection.
    Disable {
        /// Print the result as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Pair a device: prints a code and a QR, then asks you to confirm it.
    Pair {
        /// What the device may do once paired.
        #[arg(long, value_enum, default_value = "operator")]
        role: DeviceRoleArg,
    },
    /// List paired devices.
    Devices {
        /// Include revoked devices.
        #[arg(long)]
        all: bool,
        /// Print the result as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Revoke a device: its connections close at once and it never
    /// authenticates again.
    Revoke {
        /// A device id or an unambiguous name.
        device: String,
        /// Why it is revoked.
        #[arg(long)]
        reason: Option<String>,
        /// Print the result as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Rename a device.
    Rename {
        /// A device id or an unambiguous name.
        device: String,
        name: String,
        /// Print the result as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Remember another machine's daemon, reached over SSH.
    Add {
        /// 1 to 32 characters of a-z, 0-9, - and _.
        name: String,
        /// The ssh destination, e.g. me@build-box or a Host alias.
        #[arg(long, value_name = "TARGET")]
        ssh: String,
        /// Executable path on the host (default aether).
        #[arg(long)]
        executable: Option<String>,
        /// Unix socket path on the host.
        #[arg(long = "remote-socket")]
        remote_socket: Option<String>,
        /// Print the result as JSON.
        #[arg(long)]
        json: bool,
    },
    /// List the remembered SSH remotes.
    List {
        /// Print the result as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Forget an SSH remote.
    Remove {
        name: String,
        /// Print the result as JSON.
        #[arg(long)]
        json: bool,
    },
}
