//! What the wizard tells a person to do, in the words of their platform.

use super::platform::{Distro, Platform};
use super::tailscale::TailscaleState;

pub(super) const ADMIN_DNS: &str = "https://login.tailscale.com/admin/dns";
pub(super) const ADMIN_MACHINES: &str = "https://login.tailscale.com/admin/machines";
pub(super) const SIGN_UP: &str = "https://login.tailscale.com/start";
pub(super) const IOS_STORE: &str = "https://apps.apple.com/app/tailscale/id1470499037";
pub(super) const ANDROID_STORE: &str =
    "https://play.google.com/store/apps/details?id=com.tailscale.ipn";

/// One thing in the way, said as a headline and the steps that clear it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Guidance {
    pub(super) headline: String,
    pub(super) steps: Vec<String>,
}

impl Guidance {
    fn new(headline: &str, steps: Vec<String>) -> Self {
        Self {
            headline: headline.to_owned(),
            steps,
        }
    }
}

/// What stands in the way of `state` being ready, and how to clear it.
pub(super) fn for_state(state: &TailscaleState, platform: &Platform) -> Guidance {
    match state {
        TailscaleState::Ready(_) => Guidance::new("Tailscale is ready.", Vec::new()),
        TailscaleState::NotInstalled => not_installed(platform),
        TailscaleState::NotRunning { detail } => not_running(platform, detail),
        TailscaleState::Stopped => stopped(platform),
        TailscaleState::LoggedOut { login_url } => logged_out(platform, login_url.as_deref()),
        TailscaleState::NeedsApproval => Guidance::new(
            "Tailscale is waiting for an administrator to approve this computer.",
            vec![format!(
                "Open {ADMIN_MACHINES}, find this computer and approve it."
            )],
        ),
        TailscaleState::Connecting => Guidance::new(
            "Tailscale is still connecting.",
            vec!["Wait a few seconds; if this keeps showing, restart Tailscale.".to_owned()],
        ),
        TailscaleState::NoMagicDns => Guidance::new(
            "MagicDNS is off for your tailnet, so this computer has no name your phone can use.",
            vec![
                format!("Open {ADMIN_DNS}"),
                "Find \"MagicDNS\" and turn it on.".to_owned(),
            ],
        ),
        TailscaleState::NoHttpsCerts => no_https(),
    }
}

fn no_https() -> Guidance {
    Guidance::new(
        "HTTPS certificates are off for your tailnet. Phones only run Aether over HTTPS.",
        vec![
            format!("Open {ADMIN_DNS}"),
            "Scroll to \"HTTPS Certificates\" and choose \"Enable HTTPS\".".to_owned(),
            "Tailscale lists the names of your machines in a public certificate log. \
             They are names like box.tail1234.ts.net, not traffic."
                .to_owned(),
        ],
    )
}

fn not_installed(platform: &Platform) -> Guidance {
    let headline = "Tailscale is not installed on this computer.";
    let steps = match platform {
        Platform::Linux(Distro::Arch) => vec![
            "Install it:  sudo pacman -S tailscale".to_owned(),
            "Start it:    sudo systemctl enable --now tailscaled".to_owned(),
        ],
        Platform::Linux(Distro::Alpine) => vec![
            "Install it:  sudo apk add tailscale".to_owned(),
            "Start it:    sudo rc-update add tailscale && sudo rc-service tailscale start"
                .to_owned(),
        ],
        Platform::Linux(Distro::Nix) => vec![
            "Add  services.tailscale.enable = true;  to your NixOS configuration.".to_owned(),
            "Apply it:    sudo nixos-rebuild switch".to_owned(),
        ],
        Platform::Linux(_) => vec![
            "Install it with Tailscale's own installer (read it first at \
             https://tailscale.com/install.sh if you like):"
                .to_owned(),
            "    curl -fsSL https://tailscale.com/install.sh | sh".to_owned(),
            "Other ways to install: https://tailscale.com/download/linux".to_owned(),
        ],
        Platform::MacOs => vec![
            "Install it from the Mac App Store (search \"Tailscale\"), or:".to_owned(),
            "    brew install --cask tailscale".to_owned(),
            "Then open the Tailscale app once so it can start.".to_owned(),
        ],
        Platform::Wsl => vec![
            "Install Tailscale for Windows: https://tailscale.com/download/windows".to_owned(),
            "Sign in to it, then run this command again from this terminal.".to_owned(),
        ],
        Platform::Other(_) => vec!["Download it from https://tailscale.com/download".to_owned()],
    };
    Guidance::new(headline, steps)
}

fn not_running(platform: &Platform, detail: &str) -> Guidance {
    let mut steps = match platform {
        Platform::MacOs => vec![
            "Open the Tailscale app (Spotlight: Tailscale) and wait for its icon in the menu bar."
                .to_owned(),
        ],
        Platform::Wsl => vec!["Start Tailscale from the Windows Start menu.".to_owned()],
        _ => vec!["Start it:  sudo systemctl enable --now tailscaled".to_owned()],
    };
    steps.push(format!("What was reported: {detail}"));
    Guidance::new("Tailscale is installed but not running.", steps)
}

fn stopped(platform: &Platform) -> Guidance {
    let step = match platform {
        Platform::MacOs | Platform::Wsl => {
            "Click the Tailscale icon and choose \"Connect\".".to_owned()
        }
        _ => "Connect it:  sudo tailscale up".to_owned(),
    };
    Guidance::new("Tailscale is switched off.", vec![step])
}

fn logged_out(platform: &Platform, login_url: Option<&str>) -> Guidance {
    let mut steps = match platform {
        Platform::MacOs | Platform::Wsl => {
            vec!["Click the Tailscale icon and choose \"Log in\".".to_owned()]
        }
        _ => vec!["Sign in:  sudo tailscale up".to_owned()],
    };
    if let Some(url) = login_url {
        steps.push(format!("If no browser opens, go to {url}"));
    }
    steps.push(format!(
        "No account yet? Sign in with Google, Microsoft, Apple or GitHub at {SIGN_UP}. \
         Personal use is free."
    ));
    Guidance::new("Tailscale is not signed in.", steps)
}

/// Why the first Tailscale commands matter, said once to a person who has
/// never heard of it.
pub(super) fn tailscale_intro() -> &'static str {
    "Tailscale is a free service that gives your computer and your phone a small private\n\
     network of their own. Aether uses it so that only your own devices can reach it:\n\
     nothing is opened to the public internet."
}

/// The steps that finish setup on the phone once it is paired.
pub(super) fn home_screen_steps() -> Vec<String> {
    vec![
        "iPhone or iPad (use Safari):".to_owned(),
        "  1. Paired inside the Aether home-screen app? Go to step 3.".to_owned(),
        "  2. Paired in Safari? Tap the Share button, then \"Add to Home Screen\", open Aether from \
         the new icon and pair it too with `aether remote pair`: the app keeps its own pairing."
            .to_owned(),
        "  3. In Aether's settings turn on notifications and tap Allow. iPhones can only \
         notify home-screen apps."
            .to_owned(),
        "Android (use Chrome):".to_owned(),
        "  1. Open the Aether address, tap the ⋮ menu, then \"Install app\" or \"Add to Home screen\"."
            .to_owned(),
        "  2. Open Aether from the new icon.".to_owned(),
        "  3. In Aether's settings turn on notifications and tap Allow.".to_owned(),
    ]
}

/// What to run to manage devices or undo everything.
pub(super) fn management_commands() -> Vec<(&'static str, &'static str)> {
    vec![
        ("aether remote status", "what is on and who is paired"),
        ("aether remote devices", "list the paired devices"),
        ("aether remote pair", "add another phone or computer"),
        ("aether remote revoke NAME", "cut a device off at once"),
        (
            "aether remote disable",
            "close remote access and every connection",
        ),
        ("tailscale serve reset", "stop publishing on Tailscale"),
    ]
}
