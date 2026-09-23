//! Human-readable sizes and ages, and parsers for their CLI forms.

use std::time::Duration;

const UNITS: [&str; 6] = ["B", "kB", "MB", "GB", "TB", "PB"];

/// Formats bytes with decimal (SI) units, matching Finder and most disk tools: `1.5 GB`.
pub fn bytes(n: u64) -> String {
    if n < 1000 {
        return format!("{n} B");
    }
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else if value >= 10.0 {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{value:.2} {}", UNITS[unit])
    }
}

/// Parses `500`, `100MB`, `1.5 GiB`, `2g`... Decimal units are powers of 1000, `*iB` of 1024.
pub fn parse_bytes(input: &str) -> Option<u64> {
    let input = input.trim();
    let split = input
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(input.len());
    let (number, unit) = input.split_at(split);
    let number: f64 = number.parse().ok()?;
    let multiplier: f64 = match unit.trim().to_ascii_lowercase().as_str() {
        "" | "b" => 1.0,
        "k" | "kb" => 1e3,
        "m" | "mb" => 1e6,
        "g" | "gb" => 1e9,
        "t" | "tb" => 1e12,
        "kib" => 1024.0,
        "mib" => 1024.0 * 1024.0,
        "gib" => 1024.0 * 1024.0 * 1024.0,
        "tib" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };
    let value = number * multiplier;
    (value.is_finite() && value >= 0.0).then_some(value as u64)
}

/// Compact age: `now`, `5m`, `3h`, `12d`, `8w`, `4mo`, `2y`.
pub fn age(duration: Duration) -> String {
    let secs = duration.as_secs();
    const MIN: u64 = 60;
    const HOUR: u64 = 60 * MIN;
    const DAY: u64 = 24 * HOUR;
    match secs {
        s if s < MIN => "now".into(),
        s if s < HOUR => format!("{}m", s / MIN),
        s if s < DAY => format!("{}h", s / HOUR),
        s if s < 14 * DAY => format!("{}d", s / DAY),
        s if s < 60 * DAY => format!("{}w", s / (7 * DAY)),
        s if s < 365 * DAY => format!("{}mo", s / (30 * DAY)),
        s => format!("{}y", s / (365 * DAY)),
    }
}

/// Parses `30d`, `2w`, `6mo`, `1y`, `12h`, `90m`.
pub fn parse_age(input: &str) -> Option<Duration> {
    let input = input.trim().to_ascii_lowercase();
    let split = input.find(|c: char| !c.is_ascii_digit())?;
    let (number, unit) = input.split_at(split);
    let number: u64 = number.parse().ok()?;
    let secs = match unit {
        "m" | "min" | "mins" => 60,
        "h" | "hr" | "hrs" | "hour" | "hours" => 3600,
        "d" | "day" | "days" => 86_400,
        "w" | "wk" | "week" | "weeks" => 7 * 86_400,
        "mo" | "month" | "months" => 30 * 86_400,
        "y" | "yr" | "year" | "years" => 365 * 86_400,
        _ => return None,
    };
    number.checked_mul(secs).map(Duration::from_secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_bytes() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(999), "999 B");
        assert_eq!(bytes(1_500), "1.50 kB");
        assert_eq!(bytes(52_300_000_000), "52.3 GB");
        assert_eq!(bytes(140_000_000_000), "140 GB");
    }

    #[test]
    fn parses_bytes() {
        assert_eq!(parse_bytes("500"), Some(500));
        assert_eq!(parse_bytes("100MB"), Some(100_000_000));
        assert_eq!(parse_bytes("1.5 gb"), Some(1_500_000_000));
        assert_eq!(parse_bytes("1GiB"), Some(1 << 30));
        assert_eq!(parse_bytes("2x"), None);
        assert_eq!(parse_bytes("abc"), None);
    }

    #[test]
    fn formats_and_parses_ages() {
        assert_eq!(age(Duration::from_secs(5)), "now");
        assert_eq!(age(Duration::from_secs(3 * 86_400)), "3d");
        assert_eq!(age(Duration::from_secs(21 * 86_400)), "3w");
        assert_eq!(age(Duration::from_secs(400 * 86_400)), "1y");
        assert_eq!(parse_age("30d"), Some(Duration::from_secs(30 * 86_400)));
        assert_eq!(parse_age("6mo"), Some(Duration::from_secs(180 * 86_400)));
        assert_eq!(parse_age("2W"), Some(Duration::from_secs(14 * 86_400)));
        assert_eq!(parse_age("soon"), None);
    }
}
