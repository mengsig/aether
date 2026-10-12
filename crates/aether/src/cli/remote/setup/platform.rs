//! Which operating system the wizard is on, so it can say how to install and
//! start Tailscale there.

use std::fmt;

/// The Linux family a package manager command depends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Distro {
    Debian,
    Fedora,
    Arch,
    Alpine,
    Nix,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Platform {
    Linux(Distro),
    MacOs,
    /// Windows Subsystem for Linux, where Tailscale usually lives in Windows.
    Wsl,
    Other(String),
}

impl Platform {
    /// Reads the three facts that identify the platform: `std::env::consts::OS`,
    /// `/proc/version` and `/etc/os-release`. Absent files are `None`.
    pub(super) fn detect(os: &str, proc_version: Option<&str>, os_release: Option<&str>) -> Self {
        match os {
            "macos" => Self::MacOs,
            "linux" if proc_version.is_some_and(is_wsl_kernel) => Self::Wsl,
            "linux" => Self::Linux(os_release.map_or(Distro::Other, distro_of)),
            other => Self::Other(other.to_owned()),
        }
    }

    /// The names the Tailscale command line may have here, best first.
    pub(super) fn tailscale_programs(&self) -> &'static [&'static str] {
        match self {
            Self::MacOs => &[
                "tailscale",
                "/Applications/Tailscale.app/Contents/MacOS/Tailscale",
            ],
            Self::Wsl => &["tailscale", "tailscale.exe"],
            Self::Linux(_) | Self::Other(_) => &["tailscale"],
        }
    }

    /// The label reports use: `linux-arch`, `macos`, `wsl`.
    pub(super) fn label(&self) -> String {
        match self {
            Self::Linux(distro) => format!("linux-{}", distro.label()),
            Self::MacOs => "macos".to_owned(),
            Self::Wsl => "wsl".to_owned(),
            Self::Other(os) => os.clone(),
        }
    }
}

impl fmt::Display for Platform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.label())
    }
}

impl Distro {
    fn label(self) -> &'static str {
        match self {
            Self::Debian => "debian",
            Self::Fedora => "fedora",
            Self::Arch => "arch",
            Self::Alpine => "alpine",
            Self::Nix => "nix",
            Self::Other => "other",
        }
    }
}

fn is_wsl_kernel(proc_version: &str) -> bool {
    let lower = proc_version.to_ascii_lowercase();
    lower.contains("microsoft") || lower.contains("wsl")
}

/// The distribution family from an os-release file's `ID` and `ID_LIKE`.
fn distro_of(os_release: &str) -> Distro {
    let mut names: Vec<String> = Vec::new();
    for line in os_release.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key == "ID" || key == "ID_LIKE" {
            let value = value.trim().trim_matches('"').to_ascii_lowercase();
            names.extend(value.split_whitespace().map(str::to_owned));
        }
    }
    let has = |wanted: &[&str]| names.iter().any(|n| wanted.contains(&n.as_str()));
    if has(&["debian", "ubuntu", "raspbian"]) {
        Distro::Debian
    } else if has(&["fedora", "rhel", "centos"]) {
        Distro::Fedora
    } else if has(&["arch", "manjaro"]) {
        Distro::Arch
    } else if has(&["alpine"]) {
        Distro::Alpine
    } else if has(&["nixos"]) {
        Distro::Nix
    } else {
        Distro::Other
    }
}
