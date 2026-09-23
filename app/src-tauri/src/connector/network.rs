//! Network policy: eligible local IPv4 addresses, interface classes and listener rechecks.
//!
//! Split out of the one-file `connector/mod.rs` on 2026-09-23; the contract
//! stays in `connector/mod.rs`.

use super::*;

pub(super) fn interface_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 32 && name.bytes().all(|b| b.is_ascii_graphic())
}

pub(super) fn local_ipv4_addresses() -> Vec<Ipv4Addr> {
    let mut out = local_ipv4_interfaces()
        .into_iter()
        .map(|x| x.ip)
        .collect::<Vec<_>>();
    out.sort();
    out.dedup();
    out
}

/// One Connector-eligible IPv4 address with the interface and netmask that
/// carry it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct LocalAddress {
    pub(super) ip: Ipv4Addr,
    pub(super) interface: String,
    pub(super) netmask: Ipv4Addr,
}

/// Connector-eligible IPv4 addresses (`getifaddrs`, no process spawn).
pub(super) fn local_ipv4_interfaces() -> Vec<LocalAddress> {
    unsafe {
        let mut head = std::ptr::null_mut();
        if libc::getifaddrs(&mut head) != 0 {
            return vec![];
        }
        let mut out = vec![];
        let mut p = head;
        while !p.is_null() {
            let a = &*p;
            if !a.ifa_addr.is_null()
                && !a.ifa_name.is_null()
                && (*a.ifa_addr).sa_family as i32 == libc::AF_INET
            {
                let sin = &*(a.ifa_addr as *const libc::sockaddr_in);
                let ip = Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr));
                let netmask = if a.ifa_netmask.is_null() {
                    Ipv4Addr::UNSPECIFIED
                } else {
                    let mask = &*(a.ifa_netmask as *const libc::sockaddr_in);
                    Ipv4Addr::from(u32::from_be(mask.sin_addr.s_addr))
                };
                let name = std::ffi::CStr::from_ptr(a.ifa_name)
                    .to_string_lossy()
                    .into_owned();
                if interface_name(&name) && connector_network_address(ip, &name) {
                    out.push(LocalAddress {
                        ip,
                        interface: name,
                        netmask,
                    });
                }
            }
            p = a.ifa_next;
        }
        libc::freeifaddrs(head);
        out
    }
}

pub(super) fn interface_of(ip: Ipv4Addr, interfaces: &[LocalAddress]) -> Option<String> {
    interfaces
        .iter()
        .find(|candidate| candidate.ip == ip)
        .map(|candidate| candidate.interface.clone())
}

pub(super) fn validate_connector_listener(
    address: &str,
    port: u16,
    local: &[Ipv4Addr],
) -> Result<Ipv4Addr, DeckError> {
    let ip = address
        .parse::<Ipv4Addr>()
        .map_err(|_| DeckError::new(ErrorKind::Invalid, "invalid connector address"))?;
    if port < 1024
        || ip.is_unspecified()
        || ip.is_loopback()
        || !connector_network_range(ip)
        || !local.contains(&ip)
    {
        return Err(DeckError::new(
            ErrorKind::Invalid,
            "connector address or port is invalid",
        ));
    }
    Ok(ip)
}

/// A listener runs only where the user enabled it: the saved address must be
/// on an eligible interface and, once recorded, on the same interface. A
/// different network that happens to hand out the same private address is a
/// changed context, not a place to listen. Returns the entry carrying the
/// address; the running listener keeps it and stops as soon as a later check
/// (`server.rs`, every `NETWORK_RECHECK`) finds a different one — the address
/// gone, moved, or its netmask changed.
pub(super) fn listener_network_ok(
    cfg: &Config,
    interfaces: &[LocalAddress],
) -> Result<LocalAddress, DeckError> {
    let ip = cfg
        .address
        .parse::<Ipv4Addr>()
        .map_err(|_| DeckError::new(ErrorKind::Invalid, "invalid connector address"))?;
    let mut carrying = interfaces.iter().filter(|candidate| candidate.ip == ip);
    let Some(first) = carrying.clone().next() else {
        return Err(DeckError::new(
            ErrorKind::Invalid,
            "connector address is not an available private-network address",
        ));
    };
    match &cfg.interface {
        None => Ok(first.clone()),
        Some(recorded) => carrying
            .find(|candidate| &candidate.interface == recorded)
            .cloned()
            .ok_or_else(|| DeckError::new(ErrorKind::ContextChanged, "connector network changed")),
    }
}

/// Whether the running listener's network is still the one it started on.
pub(super) fn listener_network_unchanged(cfg: &Config, started: &LocalAddress) -> bool {
    listener_network_ok(cfg, &local_ipv4_interfaces()).is_ok_and(|now| &now == started)
}

/// The address ranges Connector may use at all.
pub(super) fn connector_network_range(ip: Ipv4Addr) -> bool {
    let octets = ip.octets();
    ip.is_private() || ip.is_link_local() || (octets[0] == 100 && (64..=127).contains(&octets[1]))
}

/// Addresses on which Connector may listen, each range only on the interface
/// kind it exists for. RFC1918 covers ordinary LANs on any interface. RFC6598
/// 100.64/10 is accepted only on a `utun` tunnel (Tailscale and other direct
/// VPNs); a carrier-grade NAT address on Wi-Fi or Ethernet is shared with
/// strangers. 169.254/16 link-local is accepted only on a `bridge` (a direct
/// Thunderbolt/USB cable network); a self-assigned address on Wi-Fi or
/// Ethernet is what every client of a DHCP-less public network gets too.
pub(super) fn connector_network_address(ip: Ipv4Addr, interface: &str) -> bool {
    if !connector_network_range(ip) {
        return false;
    }
    if ip.is_link_local() {
        return interface.starts_with("bridge");
    }
    if !ip.is_private() {
        return interface.starts_with("utun");
    }
    true
}
pub(super) fn host_name() -> String {
    let mut b = [0i8; 256];
    unsafe {
        if libc::gethostname(b.as_mut_ptr(), b.len()) == 0 {
            let n = b.iter().position(|x| *x == 0).unwrap_or(b.len());
            return String::from_utf8_lossy(std::slice::from_raw_parts(b.as_ptr() as *const u8, n))
                .chars()
                .filter(|c| !c.is_control())
                .take(80)
                .collect();
        }
    }
    "Mac".into()
}
