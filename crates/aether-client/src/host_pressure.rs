//! What a client on the daemon's own machine can see of why the daemon is
//! slow: CPU load, memory pressure and disk stalls, read from `/proc`.
//! Elsewhere, or where a file is missing, that signal is simply not named.

/// Runnable tasks per core past which the host is overloaded.
const LOAD_PER_CORE: f64 = 1.5;
/// Swap in use, as a share of RAM, past which memory is short.
const SWAP_SHARE: f64 = 0.25;
/// Available memory, as a share of RAM, below which memory is short.
const AVAILABLE_SHARE: f64 = 0.10;
/// Share of the last 10 s some task stalled on disk I/O past which the
/// disk is the bottleneck (Linux pressure stall information).
const IO_STALL_PERCENT: f64 = 20.0;

const KIB_PER_GIB: f64 = (1u64 << 20) as f64;

/// What the host looks like now, as far as it can be read.
#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct Readings {
    /// One-minute load average.
    pub(crate) load: Option<f64>,
    pub(crate) cores: usize,
    pub(crate) memory: Option<Memory>,
    /// `some avg10` of `/proc/pressure/io`, in percent.
    pub(crate) io_stall: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Memory {
    pub(crate) total_kib: u64,
    pub(crate) available_kib: u64,
    pub(crate) swap_used_kib: u64,
}

/// The likely cause of a slow daemon this host shows right now, if any.
pub(crate) fn observe() -> Option<String> {
    let read = |path: &str| std::fs::read_to_string(path).ok();
    let readings = Readings {
        load: read("/proc/loadavg").as_deref().and_then(parse_load),
        cores: std::thread::available_parallelism().map_or(1, usize::from),
        memory: read("/proc/meminfo").as_deref().and_then(parse_memory),
        io_stall: read("/proc/pressure/io").as_deref().and_then(parse_stall),
    };
    describe(&readings)
}

/// The signals past their thresholds, in one sentence; `None` when none is.
pub(crate) fn describe(readings: &Readings) -> Option<String> {
    assert!(readings.cores > 0, "a host has at least one core");
    let mut signs = Vec::new();
    if let Some(load) = readings
        .load
        .filter(|load| *load >= LOAD_PER_CORE * readings.cores as f64)
    {
        signs.push(format!("load {load:.1} on {} cores", readings.cores));
    }
    if let Some(memory) = readings.memory.filter(memory_is_short) {
        signs.push(format!(
            "{:.1} GiB of swap in use and {:.1} GiB of memory available",
            memory.swap_used_kib as f64 / KIB_PER_GIB,
            memory.available_kib as f64 / KIB_PER_GIB,
        ));
    }
    if let Some(stall) = readings.io_stall.filter(|s| *s >= IO_STALL_PERCENT) {
        signs.push(format!(
            "tasks waited on the disk {stall:.0}% of the last 10 s"
        ));
    }
    if signs.is_empty() {
        return None;
    }
    Some(format!("this host is overloaded: {}", signs.join(", ")))
}

fn memory_is_short(memory: &Memory) -> bool {
    assert!(
        memory.available_kib <= memory.total_kib,
        "available memory fits in RAM"
    );
    let total = memory.total_kib as f64;
    memory.swap_used_kib as f64 >= SWAP_SHARE * total
        || (memory.available_kib as f64) < AVAILABLE_SHARE * total
}

/// The one-minute load from `/proc/loadavg`.
pub(crate) fn parse_load(text: &str) -> Option<f64> {
    let load: f64 = text.split_whitespace().next()?.parse().ok()?;
    (load.is_finite() && load >= 0.0).then_some(load)
}

/// RAM, available memory and swap in use from `/proc/meminfo`.
pub(crate) fn parse_memory(text: &str) -> Option<Memory> {
    let field = |name: &str| -> Option<u64> {
        let line = text.lines().find(|line| line.starts_with(name))?;
        line[name.len()..].split_whitespace().next()?.parse().ok()
    };
    let total_kib = field("MemTotal:")?;
    let available_kib = field("MemAvailable:")?;
    let swap_total = field("SwapTotal:")?;
    let swap_free = field("SwapFree:")?;
    if total_kib == 0 || available_kib > total_kib || swap_free > swap_total {
        return None;
    }
    Some(Memory {
        total_kib,
        available_kib,
        swap_used_kib: swap_total - swap_free,
    })
}

/// `some avg10` from a `/proc/pressure/*` file, in percent.
pub(crate) fn parse_stall(text: &str) -> Option<f64> {
    let some = text.lines().find(|line| line.starts_with("some "))?;
    let avg10 = some
        .split_whitespace()
        .find_map(|pair| pair.strip_prefix("avg10="))?;
    let percent: f64 = avg10.parse().ok()?;
    (0.0..=100.0).contains(&percent).then_some(percent)
}

#[cfg(test)]
#[path = "host_pressure_tests.rs"]
mod tests;
