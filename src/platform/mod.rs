//! OS-specific collection of network interface byte counters.
//!
//! Every backend returns a list of [`InterfaceSample`]s holding the
//! *cumulative* receive/transmit byte counters as reported by the OS.
//! Rates and usage are derived from the deltas between samples by
//! [`crate::monitor::Monitor`].

use std::io;
use std::time::Duration;

use serde::{Deserialize, Serialize};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

/// Rough classification of an interface, used for icons/badges and sorting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InterfaceKind {
    Ethernet,
    Wireless,
    /// WireGuard, Wintun, TAP, PPP and other tunnel adapters.
    Vpn,
    Cellular,
    Loopback,
    Other,
}

impl InterfaceKind {
    /// Short machine readable identifier, also used by the UI.
    pub fn id(self) -> &'static str {
        match self {
            InterfaceKind::Ethernet => "ethernet",
            InterfaceKind::Wireless => "wifi",
            InterfaceKind::Vpn => "vpn",
            InterfaceKind::Cellular => "cellular",
            InterfaceKind::Loopback => "loopback",
            InterfaceKind::Other => "other",
        }
    }

    /// Human readable (Japanese) label.
    pub fn label(self) -> &'static str {
        match self {
            InterfaceKind::Ethernet => "有線 LAN",
            InterfaceKind::Wireless => "Wi-Fi",
            InterfaceKind::Vpn => "VPN",
            InterfaceKind::Cellular => "モバイル",
            InterfaceKind::Loopback => "ループバック",
            InterfaceKind::Other => "その他",
        }
    }
}

/// A single reading of an interface's counters.
#[derive(Debug, Clone, PartialEq)]
pub struct InterfaceSample {
    /// Stable identifier used to persist usage across restarts.
    ///
    /// This is the interface alias (the name shown in "Network Connections"
    /// on Windows, e.g. the WireGuard tunnel name), so usage survives the
    /// adapter being recreated when a tunnel is reconnected.
    pub key: String,
    /// Display name (usually identical to `key`).
    pub name: String,
    /// Driver / adapter description, e.g. "WireGuard Tunnel".
    pub description: String,
    pub kind: InterfaceKind,
    pub is_up: bool,
    /// Whether the interface is shown when "show all interfaces" is off.
    /// Hidden interfaces are loopback, NDIS filter and pseudo interfaces.
    pub visible: bool,
    /// Cumulative received bytes.
    pub rx_bytes: u64,
    /// Cumulative transmitted bytes.
    pub tx_bytes: u64,
    /// Link speed in bits per second, if known.
    pub link_speed_bps: Option<u64>,
}

/// Collects the current counters of all network interfaces.
pub fn collect() -> io::Result<Vec<InterfaceSample>> {
    let mut samples = collect_impl()?;
    dedup_keys(&mut samples);
    Ok(samples)
}

/// Time elapsed since the operating system booted (including sleep).
///
/// Used to detect whether interface counters were reset by a reboot while
/// the application was not running.
pub fn uptime() -> Option<Duration> {
    uptime_impl()
}

#[cfg(windows)]
use self::windows::{collect as collect_impl, uptime as uptime_impl};

#[cfg(target_os = "linux")]
use self::linux::{collect as collect_impl, uptime as uptime_impl};

#[cfg(not(any(windows, target_os = "linux")))]
fn collect_impl() -> io::Result<Vec<InterfaceSample>> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "このOSはまだサポートされていません"))
}

#[cfg(not(any(windows, target_os = "linux")))]
fn uptime_impl() -> Option<Duration> {
    None
}

/// Makes sure every sample has a unique key, so persisted usage of two
/// interfaces sharing a name is never merged.
fn dedup_keys(samples: &mut [InterfaceSample]) {
    let mut seen = std::collections::HashSet::new();
    for sample in samples.iter_mut() {
        if sample.key.is_empty() {
            sample.key = sample.description.clone();
        }
        if !seen.insert(sample.key.clone()) {
            let base = sample.key.clone();
            let mut n = 2;
            while !seen.insert(format!("{base} #{n}")) {
                n += 1;
            }
            sample.key = format!("{base} #{n}");
        }
    }
}

/// Heuristic used by the backends to spot VPN adapters by their driver name.
pub(crate) fn looks_like_vpn(text: &str) -> bool {
    const MARKERS: &[&str] = &[
        "wireguard",
        "wintun",
        "tap-windows",
        "tap-win32",
        "openvpn",
        "tailscale",
        "zerotier",
        "softether",
        "fortinet",
        "anyconnect",
        "globalprotect",
        "vpn",
    ];
    let lower = text.to_ascii_lowercase();
    MARKERS.iter().any(|m| lower.contains(m))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(key: &str) -> InterfaceSample {
        InterfaceSample {
            key: key.into(),
            name: key.into(),
            description: "desc".into(),
            kind: InterfaceKind::Other,
            is_up: true,
            visible: true,
            rx_bytes: 0,
            tx_bytes: 0,
            link_speed_bps: None,
        }
    }

    #[test]
    fn duplicate_keys_get_suffix() {
        let mut list = vec![sample("a"), sample("a"), sample("b"), sample("a"), sample("")];
        dedup_keys(&mut list);
        let keys: Vec<_> = list.iter().map(|s| s.key.as_str()).collect();
        assert_eq!(keys, ["a", "a #2", "b", "a #3", "desc"]);
    }

    #[test]
    fn vpn_detection() {
        assert!(looks_like_vpn("WireGuard Tunnel"));
        assert!(looks_like_vpn("Wintun Userspace Tunnel"));
        assert!(looks_like_vpn("TAP-Windows Adapter V9"));
        assert!(!looks_like_vpn("Intel(R) Ethernet Connection I219-V"));
    }

    #[test]
    fn collect_smoke() {
        // The CI / developer machine always has at least one interface.
        let samples = collect().expect("collect interfaces");
        assert!(!samples.is_empty());
        let mut keys: Vec<_> = samples.iter().map(|s| &s.key).collect();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), samples.len(), "keys must be unique");
        assert!(uptime().is_some());
    }
}
