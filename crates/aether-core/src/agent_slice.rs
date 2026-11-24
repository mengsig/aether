//! The limits of `aether-agents.slice`, the systemd slice every engine scope
//! joins on Linux, so agents' builds yield to the person's own programs.

/// The slice engine scopes join. systemd reads the dash as nesting, so it
/// lives in [`AGENT_SLICE_PARENT`].
pub const AGENT_SLICE: &str = "aether-agents.slice";
/// Weights only compare siblings: the parent carries them against `app.slice`.
pub const AGENT_SLICE_PARENT: &str = "aether.slice";

pub const DEFAULT_CPU_WEIGHT: u64 = 20;
pub const DEFAULT_IO_WEIGHT: u64 = 20;
pub const DEFAULT_MEMORY_HIGH: &str = "8G";
/// systemd's range for `CPUWeight=` and `IOWeight=`.
pub const WEIGHTS: std::ops::RangeInclusive<u64> = 1..=10_000;

/// What the slice carries; `None` leaves that limit off. MemoryHigh throttles
/// and reclaims, and never kills, so no limit here can end an agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSliceLimits {
    pub cpu_weight: Option<u64>,
    pub io_weight: Option<u64>,
    /// As systemd takes it: bytes, with an optional K, M, G, T or % suffix.
    pub memory_high: Option<String>,
}

impl Default for AgentSliceLimits {
    fn default() -> Self {
        Self {
            cpu_weight: Some(DEFAULT_CPU_WEIGHT),
            io_weight: Some(DEFAULT_IO_WEIGHT),
            memory_high: Some(DEFAULT_MEMORY_HIGH.to_owned()),
        }
    }
}

impl AgentSliceLimits {
    /// Every limit off: engines keep today's placement, outside the slice.
    pub fn is_off(&self) -> bool {
        self.cpu_weight.is_none() && self.io_weight.is_none() && self.memory_high.is_none()
    }

    /// `systemctl set-property` assignments for [`AGENT_SLICE`]. An off limit
    /// is reset, so a value an earlier configuration left is dropped.
    pub fn slice_properties(&self) -> Vec<String> {
        let mut properties = self.weight_properties();
        properties.push(format!(
            "MemoryHigh={}",
            self.memory_high.as_deref().unwrap_or("infinity")
        ));
        properties
    }

    /// The assignments for [`AGENT_SLICE_PARENT`]: the weights alone.
    pub fn weight_properties(&self) -> Vec<String> {
        let weight = |value: Option<u64>| value.map(|v| v.to_string()).unwrap_or_default();
        vec![
            format!("CPUWeight={}", weight(self.cpu_weight)),
            format!("IOWeight={}", weight(self.io_weight)),
        ]
    }
}

impl std::fmt::Display for AgentSliceLimits {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.is_off() {
            return f.write_str("off");
        }
        let shown = |value: Option<String>| value.unwrap_or_else(|| "off".to_owned());
        write!(
            f,
            "CPUWeight={} IOWeight={} MemoryHigh={}",
            shown(self.cpu_weight.map(|v| v.to_string())),
            shown(self.io_weight.map(|v| v.to_string())),
            shown(self.memory_high.clone())
        )
    }
}

/// A weight as configured: within [`WEIGHTS`].
pub fn parse_weight(what: &str, value: u64) -> Result<u64, String> {
    if WEIGHTS.contains(&value) {
        return Ok(value);
    }
    Err(format!(
        "{what} {value} is outside {}..={}; use \"off\" to leave it unset",
        WEIGHTS.start(),
        WEIGHTS.end()
    ))
}

/// A `MemoryHigh=` value systemd accepts and that limits something: a
/// positive number of bytes, with K, M, G or T, or a percentage up to 100.
pub fn parse_memory_high(what: &str, text: &str) -> Result<String, String> {
    let invalid = || {
        format!("{what} `{text}` must be a size such as \"8G\", \"512M\" or \"50%\", or \"off\"")
    };
    let (digits, suffix) = match text.char_indices().last() {
        Some((at, 'K' | 'M' | 'G' | 'T' | '%')) => (&text[..at], &text[at..]),
        _ => (text, ""),
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalid());
    }
    let amount: u64 = digits.parse().map_err(|_| invalid())?;
    if amount == 0 || (suffix == "%" && amount > 100) {
        return Err(invalid());
    }
    Ok(text.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_weight_the_slice_and_throttle_its_memory() {
        let limits = AgentSliceLimits::default();
        assert!(!limits.is_off());
        assert_eq!(
            limits.slice_properties(),
            ["CPUWeight=20", "IOWeight=20", "MemoryHigh=8G"]
        );
        assert_eq!(limits.weight_properties(), ["CPUWeight=20", "IOWeight=20"]);
        assert_eq!(limits.to_string(), "CPUWeight=20 IOWeight=20 MemoryHigh=8G");
    }

    #[test]
    fn an_off_limit_is_reset_rather_than_left_as_it_was() {
        let limits = AgentSliceLimits {
            cpu_weight: None,
            io_weight: Some(50),
            memory_high: None,
        };
        assert_eq!(
            limits.slice_properties(),
            ["CPUWeight=", "IOWeight=50", "MemoryHigh=infinity"]
        );
        assert_eq!(
            limits.to_string(),
            "CPUWeight=off IOWeight=50 MemoryHigh=off"
        );
        let off = AgentSliceLimits {
            io_weight: None,
            ..limits
        };
        assert!(off.is_off());
        assert_eq!(off.to_string(), "off");
    }

    #[test]
    fn accepts_only_values_systemd_takes_as_limits() {
        for good in ["8G", "512M", "1024", "50%", "100%", "1T", "64K"] {
            assert_eq!(parse_memory_high("m", good).as_deref(), Ok(good));
        }
        for bad in [
            "", "G", "0", "0G", "8X", "8g", "-1G", "101%", "8 G", "infinity", "1.5G",
        ] {
            assert!(parse_memory_high("m", bad).is_err(), "accepted `{bad}`");
        }
        assert_eq!(parse_weight("w", 1), Ok(1));
        assert_eq!(parse_weight("w", 10_000), Ok(10_000));
        assert!(parse_weight("w", 0).is_err());
        assert!(parse_weight("w", 10_001).is_err());
    }
}
