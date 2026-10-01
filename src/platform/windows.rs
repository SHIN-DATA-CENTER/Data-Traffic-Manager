//! Windows backend based on the IP Helper API.
//!
//! `GetIfTable2` returns 64-bit octet counters straight from NDIS for every
//! interface, including WireGuard (WireGuardNT) and Wintun adapters, which
//! is why it keeps working where performance-counter based tools fail.
//!
//! `GetAdaptersAddresses` is used only to find out which interfaces are
//! "real" adapters (the ones listed in Network Connections). The remaining
//! entries of the interface table are NDIS filter layers (e.g.
//! "Ethernet-WFP Native MAC Layer LightWeight Filter-0000"), WAN miniports
//! and similar pseudo interfaces whose traffic would otherwise be counted
//! twice.

use std::collections::HashSet;
use std::io;
use std::time::Duration;

use windows_sys::Win32::Foundation::{ERROR_BUFFER_OVERFLOW, NO_ERROR};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    FreeMibTable, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_FRIENDLY_NAME,
    GAA_FLAG_SKIP_MULTICAST, GAA_FLAG_SKIP_UNICAST, GetAdaptersAddresses, GetIfTable2, IF_TYPE_ETHERNET_CSMACD,
    IF_TYPE_IEEE80211, IF_TYPE_PPP, IF_TYPE_PROP_VIRTUAL, IF_TYPE_SOFTWARE_LOOPBACK, IF_TYPE_TUNNEL, IF_TYPE_WWANPP,
    IF_TYPE_WWANPP2, IP_ADAPTER_ADDRESSES_LH, MIB_IF_ROW2, MIB_IF_TABLE2,
};
use windows_sys::Win32::NetworkManagement::Ndis::{
    IfOperStatusUp, TUNNEL_TYPE, TUNNEL_TYPE_6TO4, TUNNEL_TYPE_IPHTTPS, TUNNEL_TYPE_ISATAP, TUNNEL_TYPE_TEREDO,
};
use windows_sys::Win32::Networking::WinSock::AF_UNSPEC;
use windows_sys::Win32::System::SystemInformation::GetTickCount64;

use super::{InterfaceKind, InterfaceSample, looks_like_vpn};

/// Bit of `MIB_IF_ROW2::InterfaceAndOperStatusFlags` marking NDIS filter
/// interfaces (`FilterInterface : 1` is the second bit of the bitfield).
const FILTER_INTERFACE_BIT: u8 = 0b0000_0010;

pub fn collect() -> io::Result<Vec<InterfaceSample>> {
    // A failure here only affects the "visible" heuristic, not the counters.
    let adapters = adapter_luids().ok();

    let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
    // SAFETY: GetIfTable2 allocates the table and stores the pointer in
    // `table`; it is released with FreeMibTable below.
    let status = unsafe { GetIfTable2(&mut table) };
    if status != NO_ERROR {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    let _guard = MibTableGuard(table.cast());

    // SAFETY: on success `table` points to a valid MIB_IF_TABLE2 whose
    // `Table` array holds `NumEntries` rows.
    let rows: &[MIB_IF_ROW2] = unsafe {
        let count = (*table).NumEntries as usize;
        let first = std::ptr::addr_of!((*table).Table).cast::<MIB_IF_ROW2>();
        std::slice::from_raw_parts(first, count)
    };

    Ok(rows.iter().map(|row| sample_from_row(row, adapters.as_ref())).collect())
}

pub fn uptime() -> Option<Duration> {
    // SAFETY: trivial FFI call without arguments.
    Some(Duration::from_millis(unsafe { GetTickCount64() }))
}

struct MibTableGuard(*const core::ffi::c_void);

impl Drop for MibTableGuard {
    fn drop(&mut self) {
        // SAFETY: the pointer was returned by GetIfTable2.
        unsafe { FreeMibTable(self.0) };
    }
}

fn sample_from_row(row: &MIB_IF_ROW2, adapters: Option<&HashSet<u64>>) -> InterfaceSample {
    let alias = wide_to_string(&row.Alias);
    let description = wide_to_string(&row.Description);
    // SAFETY: NET_LUID_LH is a plain u64 union.
    let luid = unsafe { row.InterfaceLuid.Value };
    let is_filter = row.InterfaceAndOperStatusFlags._bitfield & FILTER_INTERFACE_BIT != 0;
    let kind = classify(row.Type, &description);

    let is_adapter = adapters.is_none_or(|set| set.contains(&luid));
    let visible =
        is_adapter && !is_filter && kind != InterfaceKind::Loopback && !is_transition_tunnel(row.Type, row.TunnelType);

    let link_speed = row.ReceiveLinkSpeed.max(row.TransmitLinkSpeed);

    InterfaceSample {
        key: if alias.is_empty() { format!("LUID {luid:016x}") } else { alias.clone() },
        name: if alias.is_empty() { description.clone() } else { alias },
        description,
        kind,
        is_up: row.OperStatus == IfOperStatusUp,
        visible,
        rx_bytes: row.InOctets,
        tx_bytes: row.OutOctets,
        // u64::MAX is reported when the speed is unknown.
        link_speed_bps: (link_speed != 0 && link_speed != u64::MAX).then_some(link_speed),
    }
}

fn classify(if_type: u32, description: &str) -> InterfaceKind {
    match if_type {
        IF_TYPE_SOFTWARE_LOOPBACK => InterfaceKind::Loopback,
        IF_TYPE_IEEE80211 => InterfaceKind::Wireless,
        IF_TYPE_WWANPP | IF_TYPE_WWANPP2 => InterfaceKind::Cellular,
        IF_TYPE_TUNNEL | IF_TYPE_PPP | IF_TYPE_PROP_VIRTUAL => InterfaceKind::Vpn,
        _ if looks_like_vpn(description) => InterfaceKind::Vpn,
        IF_TYPE_ETHERNET_CSMACD => InterfaceKind::Ethernet,
        _ => InterfaceKind::Other,
    }
}

/// Teredo, 6to4, ISATAP and IP-HTTPS are IPv6 transition pseudo interfaces
/// that only re-encapsulate traffic of the physical adapter.
fn is_transition_tunnel(if_type: u32, tunnel_type: TUNNEL_TYPE) -> bool {
    if_type == IF_TYPE_TUNNEL
        && matches!(tunnel_type, TUNNEL_TYPE_TEREDO | TUNNEL_TYPE_6TO4 | TUNNEL_TYPE_ISATAP | TUNNEL_TYPE_IPHTTPS)
}

/// LUIDs of the adapters bound to TCP/IP, i.e. the ones a user knows about.
fn adapter_luids() -> io::Result<HashSet<u64>> {
    let flags = GAA_FLAG_SKIP_UNICAST
        | GAA_FLAG_SKIP_ANYCAST
        | GAA_FLAG_SKIP_MULTICAST
        | GAA_FLAG_SKIP_DNS_SERVER
        | GAA_FLAG_SKIP_FRIENDLY_NAME;

    // u64 elements keep the buffer suitably aligned for the structures.
    let mut buffer: Vec<u64> = vec![0; 2048];
    for _ in 0..4 {
        let mut size = (buffer.len() * size_of::<u64>()) as u32;
        let first = buffer.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
        // SAFETY: `first` points to a writable buffer of `size` bytes.
        let status = unsafe { GetAdaptersAddresses(AF_UNSPEC as u32, flags, std::ptr::null(), first, &mut size) };
        match status {
            NO_ERROR => {
                let mut luids = HashSet::new();
                let mut node = first.cast_const();
                while !node.is_null() {
                    // SAFETY: the API built a linked list inside `buffer`.
                    unsafe {
                        luids.insert((*node).Luid.Value);
                        node = (*node).Next;
                    }
                }
                return Ok(luids);
            }
            ERROR_BUFFER_OVERFLOW => {
                buffer.resize((size as usize).div_ceil(size_of::<u64>()) + 64, 0);
            }
            other => return Err(io::Error::from_raw_os_error(other as i32)),
        }
    }
    Err(io::Error::other("GetAdaptersAddresses: buffer kept growing"))
}

fn wide_to_string(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len]).trim().to_string()
}
