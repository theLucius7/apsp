use serde::{Deserialize, Serialize};

mod apply;
mod transport;
mod validate;
pub(crate) use apply::apply;
pub use transport::NodeTransport;
pub(crate) use validate::validate;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NodeSettings {
    pub listen: String,
    pub public_port: Option<u16>,
    pub public_addresses: Vec<PublicAddress>,
    pub realm: Option<HysteriaRealm>,
    pub tcp_fast_open: bool,
    pub disable_tcp_keep_alive: bool,
    pub tcp_keep_alive_seconds: Option<u16>,
    pub tcp_keep_alive_interval_seconds: Option<u16>,
    pub tls_alpn: Vec<String>,
    pub tls_min_version: Option<TlsVersion>,
    pub tls_max_version: Option<TlsVersion>,
    pub tls_handshake_timeout_seconds: Option<u16>,
    pub transport: NodeTransport,
    pub reality: RealitySettings,
    pub hysteria2: Hysteria2Settings,
    pub tuic: TuicSettings,
    pub anytls: AnyTlsSettings,
    pub snell: SnellSettings,
    pub shadowsocks: ShadowsocksSettings,
}

impl Default for NodeSettings {
    fn default() -> Self {
        Self {
            listen: "::".into(),
            public_port: None,
            public_addresses: Vec::new(),
            realm: None,
            tcp_fast_open: false,
            disable_tcp_keep_alive: false,
            tcp_keep_alive_seconds: None,
            tcp_keep_alive_interval_seconds: None,
            tls_alpn: vec![],
            tls_min_version: None,
            tls_max_version: None,
            tls_handshake_timeout_seconds: None,
            transport: NodeTransport::default(),
            reality: RealitySettings::default(),
            hysteria2: Hysteria2Settings::default(),
            tuic: TuicSettings::default(),
            anytls: AnyTlsSettings::default(),
            snell: SnellSettings::default(),
            shadowsocks: ShadowsocksSettings::default(),
        }
    }
}

impl NodeSettings {
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RealitySettings {
    pub handshake_server: Option<String>,
    pub handshake_port: u16,
    pub fingerprint: Fingerprint,
    pub max_time_difference_seconds: Option<u16>,
    pub flow: RealityFlow,
}

impl Default for RealitySettings {
    fn default() -> Self {
        Self {
            handshake_server: None,
            handshake_port: 443,
            fingerprint: Fingerprint::default(),
            max_time_difference_seconds: None,
            flow: RealityFlow::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Fingerprint {
    #[default]
    Chrome,
    Firefox,
    Safari,
    Edge,
    Ios,
    Android,
    Randomized,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Hysteria2Settings {
    pub up_mbps: Option<u32>,
    pub down_mbps: Option<u32>,
    pub ignore_client_bandwidth: bool,
    pub obfs_password: Option<String>,
    pub bbr_profile: BbrProfile,
    pub masquerade: Option<Hysteria2Masquerade>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CongestionControl {
    #[default]
    Cubic,
    NewReno,
    Bbr,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TuicSettings {
    pub congestion_control: CongestionControl,
    pub auth_timeout_seconds: Option<u16>,
    pub heartbeat_seconds: Option<u16>,
    pub zero_rtt_handshake: bool,
    pub udp_relay_mode: TuicUdpRelayMode,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AnyTlsSettings {
    pub idle_session_check_seconds: Option<u16>,
    pub idle_session_timeout_seconds: Option<u16>,
    pub min_idle_session: Option<u16>,
    pub padding_scheme: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum TlsVersion {
    #[serde(rename = "1.2")]
    V12,
    #[serde(rename = "1.3")]
    V13,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RealityFlow {
    #[default]
    Vision,
    None,
}

impl RealityFlow {
    pub fn native(self) -> &'static str {
        match self {
            Self::Vision => "xtls-rprx-vision",
            Self::None => "",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BbrProfile {
    #[default]
    Standard,
    Conservative,
    Aggressive,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hysteria2Masquerade {
    pub status_code: u16,
    pub content_type: String,
    pub content: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TuicUdpRelayMode {
    #[default]
    Native,
    QuicStream,
    UdpOverStream,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SnellSettings {
    pub mode: SnellMode,
    pub reuse: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SnellMode {
    #[default]
    Default,
    Unshaped,
    UnsafeRaw,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ShadowsocksSettings {
    pub udp_over_tcp: bool,
    pub multiplex: MultiplexSettings,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MultiplexSettings {
    pub enabled: bool,
    pub padding: bool,
    pub protocol: MultiplexProtocol,
    pub max_connections: Option<u16>,
    pub min_streams: Option<u16>,
    pub max_streams: Option<u16>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MultiplexProtocol {
    #[default]
    H2mux,
    Smux,
    Yamux,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicAddress {
    pub name: String,
    pub server: String,
    pub port: u16,
    #[serde(default)]
    pub sni: Option<String>,
    #[serde(default)]
    pub host: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HysteriaRealm {
    pub server_url: String,
    pub realm_id: String,
    pub server_token: String,
    pub client_token: String,
    pub stun_servers: Vec<String>,
    #[serde(default)]
    pub ip_version: u8,
}
