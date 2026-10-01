//! Human readable formatting of data amounts and transfer rates.

use serde::{Deserialize, Serialize};

/// Bytes in one "GB" as displayed by the application (binary, like Explorer).
pub const GIB: u64 = 1024 * 1024 * 1024;

/// How transfer rates are displayed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RateUnit {
    /// KB/s, MB/s, ... (1 KB = 1024 bytes)
    #[default]
    Bytes,
    /// kbps, Mbps, ... (1 kbit = 1000 bits), as used by ISPs and Task Manager.
    Bits,
}

/// Formats an amount of data, e.g. `1.23 GB`.
pub fn bytes(value: u64) -> String {
    scaled(value as f64, 1024.0, &["B", "KB", "MB", "GB", "TB", "PB"])
}

/// Formats a transfer rate given in bytes per second.
pub fn rate(bytes_per_sec: f64, unit: RateUnit) -> String {
    match unit {
        RateUnit::Bytes => scaled(bytes_per_sec, 1024.0, &["B/s", "KB/s", "MB/s", "GB/s", "TB/s"]),
        RateUnit::Bits => scaled(bytes_per_sec * 8.0, 1000.0, &["bps", "kbps", "Mbps", "Gbps", "Tbps"]),
    }
}

/// Formats a link speed given in bits per second, e.g. `1 Gbps`.
pub fn link_speed(bits_per_sec: u64) -> String {
    scaled(bits_per_sec as f64, 1000.0, &["bps", "kbps", "Mbps", "Gbps", "Tbps"])
}

/// Scales `value` to the largest unit that keeps it below 1000 and keeps
/// three significant digits (`1.23`, `12.3`, `123`).
fn scaled(value: f64, step: f64, units: &[&str]) -> String {
    let mut value = if value.is_finite() { value.max(0.0) } else { 0.0 };
    let mut index = 0;
    while value >= 999.5 && index + 1 < units.len() {
        value /= step;
        index += 1;
    }
    let unit = units[index];
    if index == 0 {
        return format!("{} {unit}", value.round() as u64);
    }
    if value < 9.995 {
        format!("{value:.2} {unit}")
    } else if value < 99.95 {
        format!("{value:.1} {unit}")
    } else {
        format!("{value:.0} {unit}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_bytes() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(999), "999 B");
        assert_eq!(bytes(1000), "0.98 KB");
        assert_eq!(bytes(1536), "1.50 KB");
        assert_eq!(bytes(10 * 1024 * 1024 + 300 * 1024), "10.3 MB");
        assert_eq!(bytes(123 * 1024 * 1024), "123 MB");
        assert_eq!(bytes(5 * GIB), "5.00 GB");
        assert_eq!(bytes(u64::MAX), "16384 PB");
    }

    #[test]
    fn format_rates() {
        assert_eq!(rate(0.0, RateUnit::Bytes), "0 B/s");
        assert_eq!(rate(2048.0, RateUnit::Bytes), "2.00 KB/s");
        assert_eq!(rate(125_000.0, RateUnit::Bits), "1.00 Mbps");
        assert_eq!(rate(f64::NAN, RateUnit::Bits), "0 bps");
        assert_eq!(rate(-5.0, RateUnit::Bytes), "0 B/s");
        assert_eq!(link_speed(1_000_000_000), "1.00 Gbps");
        assert_eq!(link_speed(100_000_000), "100 Mbps");
    }
}
