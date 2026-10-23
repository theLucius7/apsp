use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub panel_url: String,
    /// Additional certificate authorities for this panel origin only.
    pub panel_ca_file: Option<PathBuf>,
    pub identity_dir: PathBuf,
    pub state_db: PathBuf,
    pub runtime_root: PathBuf,
    pub install_root: PathBuf,
    pub agent_root: PathBuf,
    pub status_socket: PathBuf,
    pub operation_timeout_secs: u64,
    pub public_ips: Vec<String>,
    /// Only the node operator can permit arbitrary commands from the panel.
    pub allow_remote_commands: bool,
    pub settings: apsp_protocol::AgentSettings,
}

impl Default for Config {
    fn default() -> Self {
        let mut config = Self {
            panel_url: String::new(),
            panel_ca_file: None,
            identity_dir: "/etc/apsp/identity".into(),
            state_db: "/var/lib/apsp/core/state.db".into(),
            runtime_root: "/var/lib/apsp/plugins".into(),
            install_root: "/opt/apsp/plugins".into(),
            agent_root: "/opt/apsp/core".into(),
            status_socket: "/run/apsp/agent.sock".into(),
            operation_timeout_secs: 30,
            public_ips: Vec::new(),
            allow_remote_commands: false,
            settings: apsp_protocol::AgentSettings::default(),
        };
        if cfg!(windows) {
            let root = std::env::var_os("ProgramData")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("C:\\ProgramData"))
                .join("Apsp");
            config.identity_dir = root.join("identity");
            config.state_db = root.join("state/core/state.db");
            config.runtime_root = root.join("state/plugins");
            config.install_root = root.join("plugins");
            config.agent_root = root.join("core");
            config.status_socket = root.join("state/core/status.json");
            config.operation_timeout_secs = 120;
        } else if cfg!(target_os = "macos") {
            config.identity_dir = "/private/etc/apsp/identity".into();
            config.state_db = "/private/var/lib/apsp/core/state.db".into();
            config.runtime_root = "/private/var/lib/apsp/plugins".into();
            config.status_socket = "/private/var/run/apsp/agent.sock".into();
        } else if cfg!(target_os = "freebsd") {
            config.status_socket = "/var/run/apsp/agent.sock".into();
        }
        config
    }
}

pub fn default_path() -> PathBuf {
    if cfg!(windows) {
        Config::default()
            .identity_dir
            .parent()
            .expect("default identity parent")
            .join("agent.toml")
    } else if cfg!(target_os = "macos") {
        "/private/etc/apsp/agent.toml".into()
    } else {
        "/etc/apsp/agent.toml".into()
    }
}

impl Config {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let mut config: Self = toml::from_str(
            &std::fs::read_to_string(path)
                .with_context(|| format!("read configuration {}", path.display()))?,
        )?;
        if cfg!(target_os = "macos") {
            config.use_macos_system_paths();
        }
        config.validate()?;
        Ok(config)
    }

    fn use_macos_system_paths(&mut self) {
        // These are fixed macOS system aliases, not arbitrary symlinks.
        // Also migrate previously serialized defaults without moving any data.
        // Keep installation roots verbatim: persisted artifact paths and absolute
        // Agent version links use their original lexical identities.
        for path in [
            &mut self.identity_dir,
            &mut self.state_db,
            &mut self.runtime_root,
            &mut self.status_socket,
        ] {
            for (alias, real) in [("/etc", "/private/etc"), ("/var", "/private/var")] {
                if let Ok(relative) = path.strip_prefix(alias) {
                    *path = Path::new(real).join(relative);
                    break;
                }
            }
        }
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        validate_panel_url(&self.panel_url)?;
        crate::panel_tls::certificates(self.panel_ca_file.as_deref())?;
        if !self.settings.valid() {
            bail!(
                "telemetry intervals must be within 1..=60 seconds and upload cannot precede sampling"
            );
        }
        for path in [
            &self.identity_dir,
            &self.state_db,
            &self.runtime_root,
            &self.install_root,
            &self.agent_root,
            &self.status_socket,
        ] {
            if !path.is_absolute() {
                bail!("configured paths must be absolute: {}", path.display());
            }
        }
        if self.operation_timeout_secs == 0 || self.operation_timeout_secs > 3600 {
            bail!("operation_timeout_secs must be between 1 and 3600");
        }
        if self.public_ips.len() > 32 {
            bail!("public_ips may contain at most 32 addresses");
        }
        for value in &self.public_ips {
            let address: std::net::IpAddr = value.parse().context("invalid public IP address")?;
            let address = match address {
                std::net::IpAddr::V6(address) => address
                    .to_ipv4_mapped()
                    .map(std::net::IpAddr::V4)
                    .unwrap_or(std::net::IpAddr::V6(address)),
                address => address,
            };
            if address.is_unspecified() || address.is_multicast() || address.is_loopback() {
                bail!("public_ips cannot contain unspecified, multicast, or loopback addresses");
            }
        }
        Ok(())
    }
}

pub fn validate_panel_url(value: &str) -> anyhow::Result<reqwest::Url> {
    let url = reqwest::Url::parse(value).context("invalid panel URL")?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
        || value.chars().any(char::is_control)
    {
        bail!("panel URL must be an HTTP(S) origin without credentials, path, query, or fragment");
    }
    let host = url.host_str().unwrap_or_default();
    let literal = host
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(host);
    let loopback = host == "localhost"
        || literal
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback());
    if url.scheme() == "http" && !loopback {
        bail!(
            "panel URL must use HTTPS; HTTP is restricted to literal loopback addresses or localhost"
        );
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macos_system_alias_migration_is_fixed_and_preserves_custom_paths() {
        let mut config = Config {
            identity_dir: "/etc/apsp/identity".into(),
            state_db: "/var/lib/apsp/core/state.db".into(),
            runtime_root: "/var/lib/apsp/plugins".into(),
            status_socket: "/var/run/apsp/agent.sock".into(),
            install_root: "/opt/custom-artifacts".into(),
            agent_root: "/various/custom-agent".into(),
            ..Config::default()
        };
        config.use_macos_system_paths();
        assert_eq!(config.identity_dir, Path::new("/private/etc/apsp/identity"));
        assert_eq!(
            config.state_db,
            Path::new("/private/var/lib/apsp/core/state.db")
        );
        assert_eq!(
            config.runtime_root,
            Path::new("/private/var/lib/apsp/plugins")
        );
        assert_eq!(
            config.status_socket,
            Path::new("/private/var/run/apsp/agent.sock")
        );
        assert_eq!(config.install_root, Path::new("/opt/custom-artifacts"));
        assert_eq!(config.agent_root, Path::new("/various/custom-agent"));
    }

    #[test]
    fn macos_alias_migration_preserves_existing_var_installation_references() {
        let mut config = Config {
            install_root: "/var/apsp-plugins".into(),
            agent_root: "/var/apsp-core".into(),
            ..Config::default()
        };
        let saved_runtime = config.install_root.join("demo/1.0.0/runtime");
        let existing_agent_link = config.agent_root.join("0.3.0");
        config.use_macos_system_paths();
        assert_eq!(config.install_root, Path::new("/var/apsp-plugins"));
        assert_eq!(config.agent_root, Path::new("/var/apsp-core"));
        assert_eq!(
            saved_runtime,
            config.install_root.join("demo/1.0.0/runtime")
        );
        assert_eq!(
            existing_agent_link.parent(),
            Some(config.agent_root.as_path())
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_defaults_and_loaded_legacy_paths_avoid_system_symlink_ancestors() -> anyhow::Result<()>
    {
        let root = std::env::temp_dir().join(format!("apsp-macos-config-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root)?;
        let path = root.join("agent.toml");
        let outcome = (|| {
            std::fs::write(
                &path,
                "panel_url = 'http://127.0.0.1:9'\nidentity_dir = '/etc/apsp/identity'\nstate_db = '/var/lib/apsp/core/state.db'\nruntime_root = '/var/lib/apsp/plugins'\nstatus_socket = '/var/run/apsp/agent.sock'\n",
            )?;
            for config in [Config::default(), Config::load(&path)?] {
                for path in [
                    &config.identity_dir,
                    &config.state_db,
                    &config.runtime_root,
                    &config.status_socket,
                ] {
                    for ancestor in path.ancestors() {
                        if let Ok(metadata) = std::fs::symlink_metadata(ancestor) {
                            anyhow::ensure!(
                                !metadata.file_type().is_symlink(),
                                "unexpected alias: {}",
                                ancestor.display()
                            );
                        }
                    }
                }
            }
            assert_eq!(default_path(), Path::new("/private/etc/apsp/agent.toml"));
            Ok(())
        })();
        std::fs::remove_dir_all(root)?;
        outcome
    }

    #[test]
    fn panel_transport_requires_https_outside_literal_loopback() {
        for origin in [
            "https://panel.example.test",
            "https://192.0.2.1:8443",
            "https://[2001:db8::1]",
            "http://127.0.0.1:8080",
            "http://127.12.34.56",
            "http://[::1]:8080",
            "http://localhost:8080",
        ] {
            assert!(
                validate_panel_url(origin).is_ok(),
                "valid origin rejected: {origin}"
            );
        }
        for origin in [
            "http://panel.example.test",
            "http://localhost.example.test",
            "http://192.0.2.1",
            "http://10.0.0.1",
            "http://[2001:db8::1]",
            "http://[::]",
            "ftp://localhost",
            "https://account:password@panel.example.test",
            "https://panel.example.test/path",
            "https://panel.example.test?token=test",
            "https://panel.example.test#fragment",
        ] {
            assert!(
                validate_panel_url(origin).is_err(),
                "invalid origin accepted: {origin}"
            );
        }
    }

    #[test]
    fn remote_commands_require_local_opt_in_outside_panel_settings() {
        let config: Config = toml::from_str("panel_url = 'https://panel.example.test'").unwrap();
        assert!(!config.allow_remote_commands);
        assert!(toml::from_str::<Config>(
            "panel_url = 'https://panel.example.test'\n[settings]\nallow_remote_commands = true",
        ).is_err());
        let config: Config = toml::from_str(
            "panel_url = 'https://panel.example.test'\nallow_remote_commands = true",
        )
        .unwrap();
        assert!(config.allow_remote_commands);
    }

    #[test]
    fn public_ip_override_is_optional_and_validated() {
        let mut config: Config = toml::from_str("panel_url = 'http://127.0.0.1:8080'").unwrap();
        assert!(config.public_ips.is_empty());
        config.public_ips = vec!["192.0.2.1".into(), "2001:db8::1".into()];
        assert!(config.validate().is_ok());
        for value in [
            "example.com",
            "192.0.2.1:443",
            "::",
            "127.0.0.1",
            "::ffff:127.0.0.1",
            "224.0.0.1",
        ] {
            config.public_ips = vec![value.into()];
            assert!(
                config.validate().is_err(),
                "invalid override accepted: {value}"
            );
        }
    }
}
