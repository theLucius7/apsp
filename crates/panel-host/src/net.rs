//! Address classification shared by every domain.

use std::net::IpAddr;

/// Whether an address is globally routable. Private, shared, loopback, link-local,
/// multicast, benchmarking and documentation ranges are not.
pub fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            !matches!(a, 0 | 10 | 127 | 224..=255)
                && !(a == 100 && (64..=127).contains(&b))
                && !(a == 169 && b == 254)
                && !(a == 172 && (16..=31).contains(&b))
                && !(a == 192 && ((b == 0 && matches!(c, 0 | 2)) || b == 168))
                && !(a == 198 && (matches!(b, 18 | 19) || (b == 51 && c == 100)))
                && !(a == 203 && b == 0 && c == 113)
        }
        IpAddr::V6(ip) => {
            let segments = ip.segments();
            segments[0] & 0xe000 == 0x2000 && !(segments[0] == 0x2001 && segments[1] == 0x0db8)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::public_ip;

    #[test]
    fn only_routable_addresses_are_public() {
        for value in [
            "127.0.0.1",
            "10.0.0.1",
            "100.64.0.1",
            "192.0.2.1",
            "198.18.0.1",
            "203.0.113.1",
            "224.0.0.1",
            "::1",
            "fe80::1",
            "fc00::1",
            "2001:db8::1",
            "::ffff:127.0.0.1",
        ] {
            assert!(!public_ip(value.parse().unwrap()), "{value}");
        }
        for value in ["8.8.8.8", "2606:4700::1111"] {
            assert!(public_ip(value.parse().unwrap()), "{value}");
        }
    }
}
