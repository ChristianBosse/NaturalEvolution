//! Which addresses count as the local network (D-005: NEVO never talks to the internet).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Whether `ip` is a local network address: loopback, private (RFC 1918), link-local, shared
/// address space (RFC 6598, used by some community meshes) or IPv6 unique-local.
pub fn is_local(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_local_v4(v4),
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => is_local_v4(v4),
            None => is_local_v6(v6),
        },
    }
}

fn is_local_v4(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    ip.is_loopback() || ip.is_private() || ip.is_link_local() || (a == 100 && (64..128).contains(&b))
}

fn is_local_v6(ip: Ipv6Addr) -> bool {
    let first = ip.segments()[0];
    ip.is_loopback()
        // fc00::/7 unique local
        || (first & 0xfe00) == 0xfc00
        // fe80::/10 link-local
        || (first & 0xffc0) == 0xfe80
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_and_internet_addresses() {
        for local in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.20",
            "169.254.3.4",
            "100.64.0.1",
            "::1",
            "fd12::1",
            "fe80::1",
            "::ffff:192.168.0.1",
        ] {
            assert!(is_local(local.parse().unwrap()), "{local}");
        }
        for internet in [
            "8.8.8.8",
            "172.32.0.1",
            "100.128.0.1",
            "2001:db8::1",
            "::ffff:1.1.1.1",
            "0.0.0.0",
        ] {
            assert!(!is_local(internet.parse().unwrap()), "{internet}");
        }
    }
}
