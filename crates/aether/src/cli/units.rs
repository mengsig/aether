//! Scaled units for human output. `--json` keeps the exact counts.

use std::time::Duration;

/// A duration such as `30s`, `5m`, `2h` or `1d`: a positive integer and a unit.
pub(crate) fn parse_duration(value: &str) -> Result<Duration, String> {
    let split = value
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(value.len());
    let (number, unit) = value.split_at(split);
    let scale = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86400,
        _ => return Err("use an integer duration with s, m, h or d (for example 2h)".into()),
    };
    let seconds = number
        .parse::<u64>()
        .ok()
        .and_then(|n| n.checked_mul(scale));
    match seconds {
        Some(seconds) if seconds > 0 => Ok(Duration::from_secs(seconds)),
        _ => Err("duration must be positive and within range".into()),
    }
}

/// An interval in seconds: a bare integer, or a duration such as `5m`.
pub(crate) fn parse_every(value: &str) -> Result<std::num::NonZeroU64, String> {
    let seconds = match value.parse::<u64>() {
        Ok(seconds) => seconds,
        Err(_) => parse_duration(value)
            .map_err(|e| format!("{e}; a bare number counts seconds"))?
            .as_secs(),
    };
    std::num::NonZeroU64::new(seconds).ok_or_else(|| "interval must be positive".to_owned())
}

/// A count of tokens to three significant figures: `812`, `12.3k`, `5.98B`.
pub fn tokens(n: u64) -> String {
    scaled(n, 1000, &["", "k", "M", "B", "T"], "")
}

/// A size in binary units to three significant figures: `812 B`, `99.5 GiB`.
pub fn bytes(n: u64) -> String {
    scaled(
        n,
        1024,
        &["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"],
        " ",
    )
}

fn scaled(n: u64, base: u64, suffixes: &[&str], gap: &str) -> String {
    debug_assert!(base > 1 && !suffixes.is_empty());
    if n < base {
        return format!("{n}{gap}{}", suffixes[0]);
    }
    let base = base as f64;
    let mut value = n as f64;
    let mut step = 0;
    while step + 1 < suffixes.len() && value >= base {
        value /= base;
        step += 1;
    }
    // Rounding can reach the next unit: 999_999 reads `1.00M`, never `1000k`.
    if step + 1 < suffixes.len() && round3(value) >= base {
        value /= base;
        step += 1;
    }
    debug_assert!(step > 0, "a value of at least one base is scaled");
    let value = round3(value);
    let decimals = if value >= 100.0 {
        0
    } else if value >= 10.0 {
        1
    } else {
        2
    };
    format!("{value:.decimals$}{gap}{}", suffixes[step])
}

/// `value` rounded to three significant figures.
fn round3(value: f64) -> f64 {
    if value <= 0.0 {
        return 0.0;
    }
    let scale = 10f64.powi(2 - value.log10().floor() as i32);
    (value * scale).round() / scale
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_scale_to_three_figures() {
        assert_eq!(tokens(0), "0");
        assert_eq!(tokens(999), "999");
        assert_eq!(tokens(1_000), "1.00k");
        assert_eq!(tokens(12_345), "12.3k");
        assert_eq!(tokens(999_999), "1.00M");
        assert_eq!(tokens(5_981_893_206), "5.98B");
        assert_eq!(tokens(1_500_000_000_000), "1.50T");
    }

    #[test]
    fn bytes_scale_in_binary_units() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(1023), "1023 B");
        assert_eq!(bytes(1024), "1.00 KiB");
        assert_eq!(bytes(1_048_575), "1020 KiB");
        assert_eq!(bytes(1_048_576), "1.00 MiB");
        assert_eq!(bytes(106_801_321_380), "99.5 GiB");
        assert_eq!(bytes(u64::MAX), "16.0 EiB");
    }

    #[test]
    fn intervals_take_seconds_or_a_unit() {
        assert_eq!(parse_every("90").unwrap().get(), 90);
        assert_eq!(parse_every("30s").unwrap().get(), 30);
        assert_eq!(parse_every("5m").unwrap().get(), 300);
        assert_eq!(parse_every("1h").unwrap().get(), 3600);
        assert!(parse_every("0").is_err());
        assert!(parse_every("1x").is_err());
        assert!(parse_every("").is_err());
    }
}
