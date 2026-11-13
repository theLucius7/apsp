//! Host name syntax, shared by the panel and the configuration compiler. Only a check: no
//! message of the protocol carries this type.

/// Whether `value` is a host name in RFC 1123 syntax: at most 253 bytes of dot-separated
/// labels, each 1 to 63 ASCII letters, digits or hyphens that neither starts nor ends with a
/// hyphen. A trailing dot (an empty last label) is not part of the name, a single label is
/// one, and so are the digits of an IPv4 address; callers that treat those differently
/// check them first.
pub fn hostname(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 253
        && value.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
}

#[cfg(test)]
mod tests {
    use super::hostname;

    #[test]
    fn host_names_follow_rfc_1123() {
        let longest = [
            "a".repeat(63),
            "b".repeat(63),
            "c".repeat(63),
            "d".repeat(61),
        ]
        .join(".");
        let too_long = format!("{longest}d");
        let long_label = "a".repeat(64);
        assert_eq!(longest.len(), 253);
        for value in [
            "example.com",
            "a",
            "xn--fiqs8s.example",
            "Node-1.Example.COM",
            "192.0.2.1",
            longest.as_str(),
        ] {
            assert!(hostname(value), "{value}");
        }
        for value in [
            "",
            ".",
            "example.com.",
            "a..b",
            "-a.example",
            "a-.example",
            "a_b.example",
            "*.example.com",
            "[2001:db8::1]",
            "exa mple.com",
            long_label.as_str(),
            too_long.as_str(),
        ] {
            assert!(!hostname(value), "{value}");
        }
    }
}
