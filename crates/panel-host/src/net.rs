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

/// A complete ASCII DNS name, lowercased and without its trailing dot, or `None` when the
/// value is not one (an address, a single label, a label that is empty, longer than 63
/// bytes or not letters, digits and inner hyphens). With `wildcard`, a leading `*.` is
/// accepted and kept. Internationalized names are given in punycode.
pub fn dns_name(value: &str, wildcard: bool) -> Option<String> {
    let value = value.trim().trim_end_matches('.').to_ascii_lowercase();
    let host = if wildcard {
        value.strip_prefix("*.").unwrap_or(&value)
    } else {
        &value
    };
    let valid = host.len() <= 253
        && host.contains('.')
        && host.parse::<IpAddr>().is_err()
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        });
    valid.then_some(value)
}

/// Whether an address can be an ordinary server's public address: globally routable and
/// outside the special-use blocks the general classifier still lets through (the
/// deprecated 6to4 relay anycast, the benchmarking block and the newer documentation
/// block).
pub fn server_ip(ip: IpAddr) -> bool {
    if !public_ip(ip) {
        return false;
    }
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            !(a == 192 && b == 88 && c == 99)
        }
        IpAddr::V6(ip) => {
            let [a, b, c, ..] = ip.segments();
            !(a == 0x2001 && b == 2 && c == 0) && !(a == 0x3fff && b < 0x1000)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{dns_name, public_ip, server_ip};

    #[test]
    fn dns_names_are_normalized_and_incomplete_ones_refused() {
        assert_eq!(
            dns_name(" Proxy.Example.COM. ", false).as_deref(),
            Some("proxy.example.com")
        );
        assert_eq!(
            dns_name("*.example.com", true).as_deref(),
            Some("*.example.com")
        );
        for value in [
            "*.example.com",
            "localhost",
            "192.0.2.1",
            "a..example.com",
            "-a.example.com",
            "a_b.example.com",
        ] {
            assert_eq!(dns_name(value, false), None, "{value}");
        }
    }

    #[test]
    fn server_addresses_exclude_special_use_blocks() {
        for value in ["192.88.99.1", "2001:2::1", "3fff:0fff::1", "10.0.0.1"] {
            assert!(!server_ip(value.parse().unwrap()), "{value}");
        }
        for value in ["8.8.8.8", "2606:4700::1111", "3fff:1000::1"] {
            assert!(server_ip(value.parse().unwrap()), "{value}");
        }
    }

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
