use crate::error::{ApiError, ApiResult};
use apsp_compiler::{AcmeChallenge, ProtocolConfig, SsMethod, TlsConfig};
use base64::{Engine, engine::general_purpose::STANDARD};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Default, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ProtocolInput {
    #[default]
    VlessReality,
    VlessTunnel,
    VlessTls {
        tls: TlsInput,
    },
    Trojan {
        tls: TlsInput,
    },
    Vmess {
        tls: TlsInput,
    },
    Http {
        tls: TlsInput,
    },
    Mixed {
        tls: TlsInput,
    },
    Socks,
    Shadowsocks {
        method: String,
    },
    Shadowtls {
        method: String,
    },
    Hysteria2 {
        tls: TlsInput,
    },
    Shadowsocks2022 {
        #[serde(default)]
        method: SsMethod,
    },
    Tuic {
        tls: TlsInput,
    },
    Anytls {
        tls: TlsInput,
    },
    Naive {
        tls: TlsInput,
    },
    SnellV6,
}

#[derive(Deserialize)]
#[serde(tag = "mode", rename_all = "kebab-case", deny_unknown_fields)]
pub enum TlsInput {
    Reuse {
        node_id: i64,
    },
    #[serde(skip)]
    Resolved {
        config: TlsConfig,
    },
    Manual {
        certificate: Option<String>,
        key: Option<String>,
    },
    Dns01 {
        email: String,
        api_token: Option<String>,
        domains: Vec<String>,
    },
    Acme {
        email: String,
        challenge: AcmeChallenge,
    },
}

impl ProtocolInput {
    pub async fn build_on(
        mut self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        server: i64,
        previous: Option<&ProtocolConfig>,
    ) -> ApiResult<ProtocolConfig> {
        let input = match &mut self {
            Self::VlessTls { tls }
            | Self::Trojan { tls }
            | Self::Vmess { tls }
            | Self::Http { tls }
            | Self::Mixed { tls }
            | Self::Hysteria2 { tls }
            | Self::Tuic { tls }
            | Self::Anytls { tls }
            | Self::Naive { tls } => Some(tls),
            _ => None,
        };
        if let Some(tls) = input
            && let TlsInput::Reuse { node_id } = tls
        {
            let value:Value=sqlx::query_scalar("SELECT protocol_config FROM nodes WHERE id=$1 AND server_id=$2 AND deleted_at IS NULL").bind(*node_id).bind(server).fetch_optional(&mut **tx).await?.ok_or_else(||ApiError::BadRequest("复用证书的节点必须属于同一服务器".into()))?;
            let protocol: ProtocolConfig =
                serde_json::from_value(value).map_err(anyhow::Error::from)?;
            *tls = TlsInput::Resolved {
                config: protocol
                    .tls()
                    .cloned()
                    .ok_or_else(|| ApiError::BadRequest("该节点没有可复用的证书".into()))?,
            };
        }
        self.build(previous)
    }
    pub fn build(self, previous: Option<&ProtocolConfig>) -> ApiResult<ProtocolConfig> {
        let previous_tls = previous.and_then(ProtocolConfig::tls);
        let config = match self {
            Self::VlessReality => ProtocolConfig::VlessReality,
            Self::VlessTunnel => ProtocolConfig::VlessTunnel,
            Self::VlessTls { tls } => ProtocolConfig::VlessTls {
                tls: tls.build(previous_tls)?,
            },
            Self::Trojan { tls } => ProtocolConfig::Trojan {
                tls: tls.build(previous_tls)?,
            },
            Self::Vmess { tls } => ProtocolConfig::Vmess {
                tls: tls.build(previous_tls)?,
            },
            Self::Http { tls } => ProtocolConfig::Http {
                tls: tls.build(previous_tls)?,
            },
            Self::Mixed { tls } => ProtocolConfig::Mixed {
                tls: tls.build(previous_tls)?,
            },
            Self::Socks => ProtocolConfig::Socks,
            Self::Shadowtls { method } => ProtocolConfig::Shadowtls { method },
            Self::Shadowsocks { method } => {
                if let Some(ProtocolConfig::Shadowsocks { method: old }) = previous
                    && &method != old
                {
                    return Err(ApiError::BadRequest("加密方法创建后不可更改".into()));
                }
                ProtocolConfig::Shadowsocks { method }
            }
            Self::Hysteria2 { tls } => ProtocolConfig::Hysteria2 {
                tls: tls.build(previous_tls)?,
            },
            Self::Tuic { tls } => ProtocolConfig::Tuic {
                tls: tls.build(previous_tls)?,
            },
            Self::Anytls { tls } => ProtocolConfig::Anytls {
                tls: tls.build(previous_tls)?,
            },
            Self::Naive { tls } => ProtocolConfig::Naive {
                tls: tls.build(previous_tls)?,
            },
            Self::Shadowsocks2022 { method } => {
                let password = if let Some(ProtocolConfig::Shadowsocks2022 {
                    method: old,
                    password,
                }) = previous
                {
                    if method != *old {
                        return Err(ApiError::BadRequest(
                            "加密方法创建后不可更改，请创建新节点".into(),
                        ));
                    }
                    password.clone()
                } else {
                    credential(method.key_size())
                };
                ProtocolConfig::Shadowsocks2022 { method, password }
            }
            Self::SnellV6 => ProtocolConfig::SnellV6 {
                psk: match previous {
                    Some(ProtocolConfig::SnellV6 { psk }) => psk.clone(),
                    _ => credential(32),
                },
            },
        };
        if previous.is_some_and(|old| old.kind() != config.kind()) {
            return Err(ApiError::BadRequest(
                "协议创建后不可更改，请创建新节点".into(),
            ));
        }
        Ok(config)
    }
}

impl TlsInput {
    fn build(self, previous: Option<&TlsConfig>) -> ApiResult<TlsConfig> {
        Ok(match self {
            Self::Resolved { config } => config,
            Self::Reuse { .. } => {
                return Err(ApiError::BadRequest(
                    "证书引用需要在所属服务器上解析".into(),
                ));
            }
            Self::Dns01 {
                email,
                api_token,
                domains,
            } => TlsConfig::Dns01 {
                email,
                domains,
                api_token: api_token
                    .filter(|s| !s.is_empty())
                    .or_else(|| match previous {
                        Some(TlsConfig::Dns01 { api_token, .. }) => Some(api_token.clone()),
                        _ => None,
                    })
                    .ok_or_else(|| {
                        ApiError::BadRequest("请填写 Cloudflare DNS API Token".into())
                    })?,
            },
            Self::Acme { email, challenge } => TlsConfig::Acme { email, challenge },
            Self::Manual { certificate, key } => {
                let (certificate, key) = match (certificate, key, previous) {
                    (None, None, Some(TlsConfig::Manual { certificate, key })) => {
                        (certificate.clone(), key.clone())
                    }
                    (Some(certificate), Some(key), _) => (certificate, key),
                    _ => {
                        return Err(ApiError::BadRequest(
                            "请同时提供 PEM 证书链与私钥；不修改时可同时省略".into(),
                        ));
                    }
                };
                validate_pem(&certificate, &key)?;
                TlsConfig::Manual { certificate, key }
            }
        })
    }
}

pub(super) fn validate_pem(certificate: &str, key: &str) -> ApiResult<()> {
    let invalid = || ApiError::BadRequest("TLS 证书链或私钥无效，或两者不匹配".into());
    if certificate.len() > 65536 || key.len() > 16384 {
        return Err(ApiError::BadRequest(
            "证书链不能超过 64 KiB，私钥不能超过 16 KiB".into(),
        ));
    }
    let chain = CertificateDer::pem_slice_iter(certificate.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| invalid())?;
    if chain.is_empty() {
        return Err(invalid());
    }
    let key = PrivateKeyDer::from_pem_slice(key.as_bytes()).map_err(|_| invalid())?;
    rustls::sign::CertifiedKey::from_der(chain, key, &rustls::crypto::ring::default_provider())
        .map_err(|_| invalid())?;
    Ok(())
}

pub fn credential(size: usize) -> String {
    STANDARD.encode(apsp_panel_host::tokens::byte_vec(size))
}

/// Configuration returned by management endpoints must never include secrets.
pub(crate) fn view(config: &ProtocolConfig) -> Value {
    let mut result = json!({"type": config.kind()});
    if let ProtocolConfig::Shadowsocks { method } | ProtocolConfig::Shadowtls { method } = config {
        result["method"] = json!(method);
    }
    if let ProtocolConfig::Shadowsocks2022 { method, .. } = config {
        result["method"] = json!(method);
    }
    if let Some(tls) = config.tls() {
        result["tls"] = match tls {
            TlsConfig::Dns01 { email, domains, .. } => {
                json!({"mode":"dns01","email":email,"domains":domains,"configured":true})
            }
            TlsConfig::Manual { .. } => json!({"mode": "manual", "configured": true}),
            TlsConfig::Acme { email, challenge } => {
                json!({"mode": "acme", "email": email, "challenge": challenge})
            }
        };
    }
    result
}
