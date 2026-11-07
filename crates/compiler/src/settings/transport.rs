use crate::{CompileError, Node, RealityFlow, invalid_node, valid_public_host};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
pub enum NodeTransport {
    Tcp {},
    Ws {
        #[serde(default = "default_path")]
        path: String,
        #[serde(default)]
        host: Option<String>,
        #[serde(default)]
        max_early_data: u16,
        #[serde(default)]
        early_data_header_name: String,
    },
    Httpupgrade {
        #[serde(default = "default_path")]
        path: String,
        #[serde(default)]
        host: Option<String>,
    },
    Grpc {
        #[serde(default)]
        service_name: String,
    },
}

fn default_path() -> String {
    "/".into()
}

impl Default for NodeTransport {
    fn default() -> Self {
        Self::Tcp {}
    }
}

impl NodeTransport {
    pub fn is_tcp(&self) -> bool {
        matches!(self, Self::Tcp {})
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Self::Tcp {} => "tcp",
            Self::Ws { .. } => "ws",
            Self::Httpupgrade { .. } => "httpupgrade",
            Self::Grpc { .. } => "grpc",
        }
    }

    pub(crate) fn validate(&self, node: &Node) -> Result<(), CompileError> {
        let fail = |reason| invalid_node(node, reason);
        if self.is_tcp() {
            return Ok(());
        }
        if !node.protocol_config.is_vless()
            && !matches!(
                node.protocol_config,
                crate::ProtocolConfig::Trojan { .. } | crate::ProtocolConfig::Vmess { .. }
            )
        {
            return Err(fail("当前协议不支持额外传输设置"));
        }
        // Vision flow belongs to Reality; a tunnel node keeps Reality settings at their defaults.
        if node.protocol_config.is_reality() && node.settings.reality.flow != RealityFlow::None {
            return Err(fail(
                "WebSocket、HTTPUpgrade 和 gRPC 传输必须关闭 Vision 流控",
            ));
        }
        match self {
            Self::Ws {
                path,
                host,
                max_early_data,
                early_data_header_name,
            } => {
                validate_http(node, path, host)?;
                if (*max_early_data == 0 && !early_data_header_name.is_empty())
                    || (!early_data_header_name.is_empty()
                        && (early_data_header_name.len() > 128
                            || !early_data_header_name.bytes().all(header_token)
                            || [
                                "host",
                                "connection",
                                "upgrade",
                                "content-length",
                                "transfer-encoding",
                                "sec-websocket-key",
                                "sec-websocket-version",
                            ]
                            .contains(&early_data_header_name.to_ascii_lowercase().as_str())))
                {
                    return Err(fail(
                        "WebSocket 提前数据头最多 128 字节，需为合法且非保留的 HTTP 字段，并启用提前数据",
                    ));
                }
            }
            Self::Httpupgrade { path, host } => validate_http(node, path, host)?,
            Self::Grpc { service_name } => {
                if service_name.len() > 128
                    || !service_name
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
                {
                    return Err(fail(
                        "gRPC 服务名最多 128 字节，只允许英文字母、数字、点、下划线和连字符",
                    ));
                }
            }
            Self::Tcp {} => {}
        }
        Ok(())
    }

    pub(crate) fn native(&self, client: bool) -> Option<Value> {
        Some(match self {
            Self::Tcp {} => return None,
            Self::Ws {
                path,
                host,
                max_early_data,
                early_data_header_name,
            } => {
                let mut value = json!({"type":"ws","path":path});
                // WebSocket inbound headers are response headers; Host belongs only to requests.
                if client && let Some(host) = host {
                    value["headers"] = json!({"Host":host});
                }
                if *max_early_data > 0 {
                    value["max_early_data"] = json!(max_early_data);
                    if !early_data_header_name.is_empty() {
                        value["early_data_header_name"] = json!(early_data_header_name);
                    }
                }
                value
            }
            Self::Httpupgrade { path, host } => {
                let mut value = json!({"type":"httpupgrade","path":path});
                if let Some(host) = host {
                    value["host"] = json!(host);
                }
                value
            }
            Self::Grpc { service_name } => json!({"type":"grpc","service_name":service_name}),
        })
    }
}

fn header_token(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte)
}

fn validate_http(node: &Node, path: &str, host: &Option<String>) -> Result<(), CompileError> {
    if path.len() > 2048
        || !path.starts_with('/')
        || path.starts_with("//")
        || path
            .bytes()
            .any(|b| !b.is_ascii_graphic() || b"?#".contains(&b))
        || host.as_ref().is_some_and(|h| !valid_public_host(h))
    {
        return Err(invalid_node(
            node,
            "传输路径需以 / 开头、不含查询/片段且最多 2048 字节；Host 需为不含端口的域名或 IP",
        ));
    }
    Ok(())
}
