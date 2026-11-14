//! Addresses, names and the connections the panel opens itself, shared by every domain:
//! which addresses are public, what a host name and a URL must look like, checking what a
//! destination resolved to before connecting there, TLS to such a destination, and TCP
//! dials.
//!
//! A test below keeps the classification of addresses in this module.

use apsp_protocol::names::hostname;
use reqwest::Url;
use rustls::pki_types::{InvalidDnsNameError, ServerName};
use std::{
    io,
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::Duration,
};
use tokio::net::{TcpStream, ToSocketAddrs};
use tokio_rustls::{TlsConnector, client::TlsStream};

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
    let valid = host.contains('.') && host.parse::<IpAddr>().is_err() && hostname(host);
    valid.then_some(value)
}

/// What a host field accepts besides a name in RFC 1123 syntax
/// ([`apsp_protocol::names::hostname`]). The default is that name alone.
#[derive(Clone, Copy, Debug, Default)]
pub struct HostRules {
    /// An IP literal is judged by this function instead of as a name.
    pub ip: Option<fn(IpAddr) -> bool>,
    /// How many trailing dots may follow the name; the whole value stays within 253 bytes.
    pub trailing_dots: usize,
    /// At least two labels.
    pub dotted: bool,
    /// Letters only in lowercase.
    pub lowercase: bool,
}

/// Whether `value` is a host the field accepts under `rules`.
pub fn host(value: &str, rules: HostRules) -> bool {
    if let Some(accept) = rules.ip
        && let Ok(ip) = value.parse::<IpAddr>()
    {
        return accept(ip);
    }
    let name = value.trim_end_matches('.');
    value.len() <= 253
        && value.len() - name.len() <= rules.trailing_dots
        && hostname(name)
        && (!rules.dotted || name.contains('.'))
        && (!rules.lowercase || !name.bytes().any(|byte| byte.is_ascii_uppercase()))
}

/// Whether a URL names a host under one of `schemes` and carries no credentials or
/// fragment: the part every URL check of the panel shares. Lengths, ports, host rules and
/// which hosts are allowed stay with each check.
pub fn plain_url(url: &Url, schemes: &[&str]) -> bool {
    schemes.contains(&url.scheme())
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.fragment().is_none()
}

/// The host of a URL as address parsing, name checks and resolution take it: an IPv6
/// literal without the brackets a URL writes around it, anything else as the URL holds it.
/// `None` when the URL has no host.
pub fn url_host(url: &Url) -> Option<&str> {
    let host = url.host_str()?;
    Some(
        host.strip_prefix('[')
            .and_then(|literal| literal.strip_suffix(']'))
            .unwrap_or(host),
    )
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

/// Whether the panel may connect to an address on someone else's behalf (a URL an
/// administrator or a remote party supplied): an ordinary server address and, on IPv6, also
/// outside every range that tunnels or embeds IPv4 or is assigned to protocols (2001::/23,
/// 6to4 in 2002::/16, all of 3fff::/16), so no IPv6 form reaches what the IPv4 rules refuse.
pub fn egress_ip(ip: IpAddr) -> bool {
    server_ip(ip)
        && match ip {
            IpAddr::V4(_) => true,
            IpAddr::V6(ip) => {
                let [a, b, ..] = ip.segments();
                !(a == 0x2001 && b < 0x0200) && a != 0x2002 && a != 0x3fff
            }
        }
}

/// Whether an address is in a documentation block (192.0.2.0/24, 198.51.100.0/24,
/// 203.0.113.0/24, 2001:db8::/32), which [`public_ip`] refuses.
pub fn documentation_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            matches!((a, b, c), (192, 0, 2) | (198, 51, 100) | (203, 0, 113))
        }
        IpAddr::V6(ip) => ip.segments()[0] == 0x2001 && ip.segments()[1] == 0x0db8,
    }
}

/// Why the panel refuses to connect to what a destination resolved to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// The name resolved to no address.
    Empty,
    /// It resolved to more addresses than the caller takes.
    TooMany,
    /// An address is one the panel must not connect to.
    NotPublic,
}

/// Checks what a destination resolved to before the panel connects there itself: at least
/// one address, at most `limit`, and every one accepted by `accept` ([`public_ip`],
/// [`egress_ip`]). Loopback, private, link-local and metadata addresses are reachable from
/// the panel host alone, so a destination that resolves to one could reach them through the
/// panel. The connection that follows goes only to the checked addresses: pinned
/// ([`crate::http::Outbound::pin`]) or dialled one by one.
pub fn check_resolved(
    addresses: &[SocketAddr],
    limit: usize,
    accept: fn(IpAddr) -> bool,
) -> Result<(), Refused> {
    if addresses.is_empty() {
        return Err(Refused::Empty);
    }
    if addresses.len() > limit {
        return Err(Refused::TooMany);
    }
    if addresses.iter().any(|address| !accept(address.ip())) {
        return Err(Refused::NotPublic);
    }
    Ok(())
}

/// Why a TCP connection did not open.
#[derive(Debug)]
pub enum DialFailure {
    /// Nothing answered within the limit.
    TimedOut,
    /// The connection was refused or failed.
    Failed(io::Error),
}

/// Opens a TCP connection within `limit`: the one way the panel checks that a port answers.
pub async fn dial(address: impl ToSocketAddrs, limit: Duration) -> Result<TcpStream, DialFailure> {
    match tokio::time::timeout(limit, TcpStream::connect(address)).await {
        Ok(Ok(stream)) => Ok(stream),
        Ok(Err(error)) => Err(DialFailure::Failed(error)),
        Err(_) => Err(DialFailure::TimedOut),
    }
}

/// Why TLS could not be prepared for a host.
#[derive(Debug, thiserror::Error)]
pub enum TlsSetup {
    #[error("TLS configuration failed: {0}")]
    Configuration(#[from] rustls::Error),
    #[error("not a TLS server name: {0}")]
    Name(#[from] InvalidDnsNameError),
}

/// TLS for one host on a connection the panel opened itself, verifying the host's
/// certificate against the public web roots.
pub struct Tls {
    connector: TlsConnector,
    name: ServerName<'static>,
}

impl Tls {
    pub fn new(host: &str) -> Result<Self, TlsSetup> {
        let roots =
            rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let config = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()?
        .with_root_certificates(roots)
        .with_no_client_auth();
        Ok(Self {
            connector: TlsConnector::from(Arc::new(config)),
            name: ServerName::try_from(host.to_owned())?,
        })
    }

    /// Runs the handshake; it fails when the certificate does not verify for the host.
    pub async fn handshake(self, stream: TcpStream) -> io::Result<TlsStream<TcpStream>> {
        self.connector.connect(self.name, stream).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn egress_addresses_refuse_every_ipv4_embedding_and_reserved_range() {
        for value in [
            "0.0.0.0",
            "0.0.0.1",
            "10.0.0.1",
            "127.0.0.1",
            "169.254.169.254",
            "172.16.0.1",
            "192.168.0.1",
            "100.64.0.1",
            "192.0.0.9",
            "192.0.2.1",
            "192.88.99.1",
            "198.18.0.1",
            "198.51.100.1",
            "203.0.113.1",
            "224.0.0.1",
            "240.0.0.1",
            "255.255.255.255",
            "::",
            "::1",
            "::7f00:1",
            "::ffff:8.8.8.8",
            "::ffff:127.0.0.1",
            "::ffff:0:7f00:1",
            "64:ff9b::808:808",
            "64:ff9b:1::1",
            "fc00::1",
            "fd00::1",
            "fe80::1",
            "2001::1",
            "2001:20::1",
            "2001:db8::1",
            "2002:808:808::1",
            "3fff::1",
            // The whole 3fff::/16 stays refused, not only the documentation /20.
            "3fff:1000::1",
            "5f00::1",
        ] {
            assert!(!egress_ip(value.parse().unwrap()), "{value}");
        }
        for value in [
            "8.8.8.8",
            "1.1.1.1",
            "2003::1",
            "2600::1",
            "2a00::1",
            "2606:4700::1111",
            "2001:4860:4860::8888",
        ] {
            assert!(egress_ip(value.parse().unwrap()), "{value}");
        }
        for value in ["192.0.2.1", "198.51.100.7", "203.0.113.255", "2001:db8::1"] {
            assert!(documentation_ip(value.parse().unwrap()), "{value}");
        }
        for value in ["192.0.3.1", "8.8.8.8", "2001:db9::1"] {
            assert!(!documentation_ip(value.parse().unwrap()), "{value}");
        }
    }

    #[test]
    fn resolved_destinations_are_checked_before_any_connection() {
        let public: SocketAddr = "8.8.8.8:443".parse().unwrap();
        let private: SocketAddr = "10.0.0.1:443".parse().unwrap();
        assert_eq!(check_resolved(&[], 4, public_ip), Err(Refused::Empty));
        assert_eq!(
            check_resolved(&[public, public], 1, public_ip),
            Err(Refused::TooMany)
        );
        assert_eq!(
            check_resolved(&[public, private], 4, public_ip),
            Err(Refused::NotPublic)
        );
        assert_eq!(check_resolved(&[public, public], 2, public_ip), Ok(()));
        assert_eq!(check_resolved(&[private], 1, |_| true), Ok(()));
    }

    #[tokio::test]
    async fn dials_connect_or_say_why_not() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let stream = dial(address, Duration::from_secs(3)).await.unwrap();
        assert_eq!(stream.peer_addr().unwrap(), address);
        drop(listener);
        assert!(matches!(
            dial(address, Duration::from_secs(3)).await,
            Err(DialFailure::Failed(_))
        ));
        assert!(matches!(
            dial(address, Duration::ZERO).await,
            Err(DialFailure::TimedOut) | Err(DialFailure::Failed(_))
        ));
    }

    #[test]
    fn tls_needs_a_server_name() {
        assert!(Tls::new("example.com").is_ok());
        assert!(Tls::new("192.0.2.1").is_ok());
        assert!(matches!(Tls::new("bad name"), Err(TlsSetup::Name(_))));
    }

    /// The Rust sources below `crates/` of the panel, without build output, the Agent, its
    /// adapters and its probe (another program with its own network policy).
    fn panel_sources(directory: &std::path::Path, found: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(directory).expect("readable source directory") {
            let entry = entry.expect("directory entry");
            let name = entry.file_name().to_string_lossy().into_owned();
            let kind = entry.file_type().expect("file type");
            let path = entry.path();
            if kind.is_dir()
                && name != "target"
                && !matches!(name.as_str(), "agent" | "agent-core" | "tcp-probe")
                && !name.starts_with("adapter-")
            {
                panel_sources(&path, found);
            } else if kind.is_file() && name.ends_with(".rs") {
                found.push(path);
            }
        }
    }

    /// Address classification lives here, so that every check of "may the panel connect to
    /// this address" answers the same way. Standard-library classifiers and IPv6 prefix masks
    /// elsewhere would start another one; the files below classify addresses for another
    /// purpose.
    #[test]
    fn addresses_are_classified_only_here() {
        const CLASSIFIERS: [&str; 15] = [
            ".is_private()",
            ".is_link_local()",
            ".is_documentation()",
            ".is_global()",
            ".is_unique_local()",
            ".is_unicast_link_local()",
            ".is_unicast_global()",
            ".is_broadcast()",
            ".is_benchmarking()",
            ".is_shared()",
            ".is_reserved()",
            "0xfe80",
            "0xfc00",
            "0xffc0",
            "0xe000",
        ];
        const OTHER_PURPOSES: [(&str, &str); 2] = [
            (
                "resources/src/cloud/addresses.rs",
                "collects the IPv6 addresses a cloud provider lists for a resource and drops \
                 only the ones that cannot be the resource's own; the panel never connects to \
                 them",
            ),
            (
                "observability/src/diagnostic_tools/tcpquality/mod.rs",
                "a server probes a target, not the panel: any unicast address is a valid \
                 target, so only unspecified, multicast and broadcast ones are refused",
            ),
        ];
        let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("crates directory");
        let mut files = Vec::new();
        panel_sources(crates, &mut files);
        assert!(files.len() > 100, "the workspace sources were not found");
        let mut misplaced = Vec::new();
        let mut used = std::collections::BTreeSet::new();
        for path in &files {
            let relative = path
                .strip_prefix(crates)
                .expect("path under crates/")
                .to_string_lossy()
                .replace('\\', "/");
            if relative == "panel-host/src/net.rs" {
                continue;
            }
            let source = std::fs::read_to_string(path).expect("readable source");
            for (index, line) in source.lines().enumerate() {
                if !CLASSIFIERS.iter().any(|pattern| line.contains(pattern)) {
                    continue;
                }
                if OTHER_PURPOSES.iter().any(|(file, _)| *file == relative) {
                    used.insert(relative.clone());
                } else {
                    misplaced.push(format!("{relative}:{}: {}", index + 1, line.trim()));
                }
            }
        }
        assert!(
            misplaced.is_empty(),
            "classify addresses with apsp_panel_host::net (public_ip, server_ip, egress_ip):\n{}",
            misplaced.join("\n")
        );
        let listed: std::collections::BTreeSet<_> = OTHER_PURPOSES
            .iter()
            .map(|(file, _)| (*file).to_owned())
            .collect();
        assert_eq!(used, listed, "remove exceptions that no longer classify");
    }

    #[test]
    fn host_rules_keep_each_field_to_what_it_accepts() {
        let names = HostRules::default();
        assert!(host("node-1.example.com", names));
        assert!(host("localhost", names));
        assert!(host("192.0.2.1", names));
        assert!(!host("node.example.com.", names));
        assert!(!host("2001:db8::1", names));
        let literals = HostRules {
            ip: Some(|ip| !ip.is_unspecified()),
            ..names
        };
        assert!(host("2001:db8::1", literals));
        assert!(!host("0.0.0.0", literals));
        let qualified = HostRules {
            trailing_dots: 1,
            ..names
        };
        assert!(host("node.example.com.", qualified));
        assert!(!host("node.example.com..", qualified));
        assert!(!host(".", qualified));
        let longest = [
            "a".repeat(63),
            "b".repeat(63),
            "c".repeat(63),
            "d".repeat(61),
        ]
        .join(".");
        assert!(host(&longest, qualified));
        assert!(!host(&format!("{longest}."), qualified));
        let lowercase = HostRules {
            dotted: true,
            lowercase: true,
            ..names
        };
        assert!(host("apsp.example.com", lowercase));
        assert!(!host("apsp", lowercase));
        assert!(!host("Apsp.example.com", lowercase));
    }

    #[test]
    fn plain_urls_have_a_host_and_no_credentials_or_fragment() {
        for (value, plain) in [
            ("https://example.com/path?query", true),
            ("https://[2001:db8::1]:8443/", true),
            ("http://example.com/", false),
            ("https://name@example.com/", false),
            ("https://:secret@example.com/", false),
            ("https://example.com/#fragment", false),
            ("https://example.com/#", false),
            ("mailto:someone@example.com", false),
        ] {
            assert_eq!(
                plain_url(&Url::parse(value).unwrap(), &["https"]),
                plain,
                "{value}"
            );
        }
        assert!(plain_url(
            &Url::parse("http://example.com/").unwrap(),
            &["http", "https"]
        ));
    }

    #[test]
    fn url_hosts_drop_only_the_brackets_of_ipv6_literals() {
        for (value, host) in [
            ("https://[2001:db8::1]:8443/x", Some("2001:db8::1")),
            ("wss://[::1]/", Some("::1")),
            ("http://192.0.2.1:8080/", Some("192.0.2.1")),
            (
                "https://Node-1.Example.COM/path",
                Some("node-1.example.com"),
            ),
            ("mailto:someone@example.com", None),
        ] {
            assert_eq!(url_host(&Url::parse(value).unwrap()), host, "{value}");
        }
    }
}
