//! Scaled units for human output. `--json` keeps the exact counts.

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
}
