//! Linux backend reading `/sys/class/net/<iface>/statistics`.
//!
//! The application targets Windows, but a Linux backend makes development
//! and testing possible on other machines.

use std::fs;
use std::io;
use std::path::Path;
use std::time::Duration;

use super::{InterfaceKind, InterfaceSample, looks_like_vpn};

const SYS_NET: &str = "/sys/class/net";
const ARPHRD_LOOPBACK: u32 = 772;
const ARPHRD_NONE: u32 = 65534;
const ARPHRD_PPP: u32 = 512;
const IFF_UP: u32 = 0x1;

pub fn collect() -> io::Result<Vec<InterfaceSample>> {
    let mut samples = Vec::new();
    for entry in fs::read_dir(SYS_NET)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        // Interfaces can disappear while we are iterating; skip those.
        if let Some(sample) = read_interface(&entry.path(), name) {
            samples.push(sample);
        }
    }
    samples.sort_by(|a, b| a.key.cmp(&b.key));
    Ok(samples)
}

pub fn uptime() -> Option<Duration> {
    let text = fs::read_to_string("/proc/uptime").ok()?;
    let secs: f64 = text.split_whitespace().next()?.parse().ok()?;
    Some(Duration::from_secs_f64(secs))
}

fn read_interface(dir: &Path, name: String) -> Option<InterfaceSample> {
    let rx_bytes = read_number(&dir.join("statistics/rx_bytes"))?;
    let tx_bytes = read_number(&dir.join("statistics/tx_bytes"))?;

    let arp_type = read_number(&dir.join("type")).unwrap_or(0) as u32;
    let flags = read_hex(&dir.join("flags")).unwrap_or(0);
    let operstate = read_string(&dir.join("operstate")).unwrap_or_default();
    let devtype = read_uevent(dir, "DEVTYPE");
    let driver = fs::read_link(dir.join("device/driver"))
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()));

    let kind = classify(dir, &name, arp_type, devtype.as_deref());
    // Tunnels such as WireGuard report "unknown" while they are up.
    let is_up = operstate == "up" || (operstate == "unknown" && flags & IFF_UP != 0);
    // `speed` is in Mbit/s and reads as -1 or fails when unknown.
    let link_speed_bps = read_string(&dir.join("speed"))
        .and_then(|s| s.parse::<i64>().ok())
        .filter(|&mbps| mbps > 0)
        .map(|mbps| mbps as u64 * 1_000_000);

    let description = match (&devtype, &driver) {
        (Some(t), _) if t == "wireguard" => "WireGuard".to_string(),
        (_, Some(d)) => d.clone(),
        (Some(t), None) => t.clone(),
        (None, None) => kind.label().to_string(),
    };

    let visible = kind != InterfaceKind::Loopback && !name.starts_with("veth");

    Some(InterfaceSample {
        key: name.clone(),
        name,
        description,
        kind,
        is_up,
        visible,
        rx_bytes,
        tx_bytes,
        link_speed_bps,
    })
}

fn classify(dir: &Path, name: &str, arp_type: u32, devtype: Option<&str>) -> InterfaceKind {
    if arp_type == ARPHRD_LOOPBACK {
        return InterfaceKind::Loopback;
    }
    if dir.join("wireless").exists() || dir.join("phy80211").exists() || devtype == Some("wlan") {
        return InterfaceKind::Wireless;
    }
    if devtype == Some("wwan") {
        return InterfaceKind::Cellular;
    }
    let tunnel_name = ["wg", "tun", "tap", "ppp", "tailscale", "zt"].iter().any(|p| name.starts_with(p));
    if devtype == Some("wireguard")
        || arp_type == ARPHRD_NONE
        || arp_type == ARPHRD_PPP
        || tunnel_name
        || looks_like_vpn(name)
    {
        return InterfaceKind::Vpn;
    }
    if arp_type == 1 {
        return InterfaceKind::Ethernet;
    }
    InterfaceKind::Other
}

fn read_string(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

fn read_number(path: &Path) -> Option<u64> {
    read_string(path)?.parse().ok()
}

fn read_hex(path: &Path) -> Option<u32> {
    let text = read_string(path)?;
    u32::from_str_radix(text.trim_start_matches("0x"), 16).ok()
}

fn read_uevent(dir: &Path, key: &str) -> Option<String> {
    let text = fs::read_to_string(dir.join("uevent")).ok()?;
    text.lines().find_map(|line| line.strip_prefix(key)?.strip_prefix('=')).map(str::to_string)
}
