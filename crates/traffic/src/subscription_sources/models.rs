use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::FromRow;
use std::collections::BTreeMap;
use uuid::Uuid;

pub const MAX_CONTENT_BYTES: usize = 2 * 1024 * 1024;
pub const DEFAULT_REFRESH_SECS: i64 = 86_400;
pub const MIN_REFRESH_SECS: i64 = 300;
pub const MAX_REFRESH_SECS: i64 = 2_592_000;

fn deserialize_auth_headers<'de, D>(deserializer: D) -> Result<BTreeMap<String, String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct HeaderText(String);
    impl<'de> Deserialize<'de> for HeaderText {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: serde::Deserializer<'de>,
        {
            struct TextVisitor;
            impl serde::de::Visitor<'_> for TextVisitor {
                type Value = HeaderText;
                fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    formatter.write_str("a bounded authentication-header string")
                }
                fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
                where
                    E: serde::de::Error,
                {
                    if value.len() > 8192 {
                        return Err(E::custom("authentication headers exceed their limit"));
                    }
                    Ok(HeaderText(value.to_owned()))
                }
                fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
                where
                    E: serde::de::Error,
                {
                    if value.len() > 8192 {
                        return Err(E::custom("authentication headers exceed their limit"));
                    }
                    Ok(HeaderText(value))
                }
            }
            deserializer.deserialize_str(TextVisitor)
        }
    }
    struct HeadersVisitor;
    impl<'de> serde::de::Visitor<'de> for HeadersVisitor {
        type Value = BTreeMap<String, String>;
        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("at most three distinct bounded authentication headers")
        }
        fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
        where
            A: serde::de::MapAccess<'de>,
        {
            let mut result = BTreeMap::new();
            let mut bytes = 0usize;
            while let Some(HeaderText(name)) = map.next_key::<HeaderText>()? {
                let name = name.to_ascii_lowercase();
                if result.len() >= 3 || result.contains_key(&name) {
                    return Err(serde::de::Error::custom(
                        "authentication headers are duplicated or exceed their limit",
                    ));
                }
                let HeaderText(value) = map.next_value::<HeaderText>()?;
                bytes = bytes.saturating_add(name.len()).saturating_add(value.len());
                if bytes > 8192 {
                    return Err(serde::de::Error::custom(
                        "authentication headers exceed their limit",
                    ));
                }
                result.insert(name, value);
            }
            Ok(result)
        }
    }
    deserializer.deserialize_map(HeadersVisitor)
}

/// Axum's default JSON rejection can echo secret field values. Only fixed errors
/// leave the protected source-writing boundary, including oversized bodies.
pub struct ProtectedJson<T>(pub T);

impl<S, T> axum::extract::FromRequest<S> for ProtectedJson<T>
where
    S: Send + Sync,
    T: serde::de::DeserializeOwned + Send,
{
    type Rejection = axum::response::Response;

    async fn from_request(
        request: axum::extract::Request,
        state: &S,
    ) -> Result<Self, Self::Rejection> {
        use axum::response::IntoResponse;
        match <axum::Json<T> as axum::extract::FromRequest<S>>::from_request(request, state).await {
            Ok(axum::Json(value)) => Ok(Self(value)),
            Err(error) => {
                let oversized = error.status() == axum::http::StatusCode::PAYLOAD_TOO_LARGE;
                let status = if oversized {
                    axum::http::StatusCode::PAYLOAD_TOO_LARGE
                } else {
                    axum::http::StatusCode::BAD_REQUEST
                };
                let message = if oversized {
                    "来源请求正文超过大小限制"
                } else {
                    "来源请求格式无效，请检查字段及类型"
                };
                Err((status, axum::Json(serde_json::json!({"error":message}))).into_response())
            }
        }
    }
}

/// Public failures contain fixed descriptions, never upstream error text or input.
#[derive(Clone, Deserialize, Serialize)]
pub struct SourceFailure {
    pub stage: String,
    pub kind: String,
    pub message: String,
    pub http_status: Option<u16>,
}

impl SourceFailure {
    pub fn new(stage: &str, kind: &str, message: &str) -> Self {
        Self {
            stage: stage.into(),
            kind: kind.into(),
            message: message.into(),
            http_status: None,
        }
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SourceInput {
    Url {
        url: String,
        #[serde(default, deserialize_with = "deserialize_auth_headers")]
        auth_headers: BTreeMap<String, String>,
    },
    Inline {
        content: String,
    },
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CreateSource {
    pub request_id: Uuid,
    pub name: String,
    pub input: SourceInput,
    pub refresh_interval_secs: Option<i64>,
    /// Absent means no User-Agent header, as before this field existed.
    pub user_agent: Option<String>,
    pub auto_refresh: Option<bool>,
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum HeaderUpdate {
    Replace {
        #[serde(deserialize_with = "deserialize_auth_headers")]
        value: BTreeMap<String, String>,
    },
    Clear,
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum UserAgentUpdate {
    Replace { value: String },
    Clear,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityAction {
    Update,
    Replace,
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum InputUpdate {
    Url {
        url: Option<String>,
        auth_headers: Option<HeaderUpdate>,
    },
    Inline {
        content: String,
        identity_action: IdentityAction,
    },
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateSource {
    pub request_id: Uuid,
    pub settings_revision: i64,
    pub name: Option<String>,
    pub refresh_interval_secs: Option<i64>,
    pub archived: Option<bool>,
    pub input: Option<InputUpdate>,
    pub user_agent: Option<UserAgentUpdate>,
    pub auto_refresh: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceRevisionInput {
    pub settings_revision: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdoptNode {
    pub adopted: bool,
    pub settings_revision: i64,
    pub identity_epoch: i64,
    pub node_version_id: Uuid,
    /// Catalog metadata revision of the node; 0 when it has none yet.
    pub metadata_revision: i64,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct MutationReceipt {
    pub source_id: i64,
    pub settings_revision: i64,
    pub identity_epoch: i64,
    pub job_id: Option<Uuid>,
}

#[derive(Clone, Default, Deserialize, Serialize)]
pub struct NodeCounts {
    pub supported: i64,
    pub unsupported: i64,
    pub ambiguous: i64,
    pub missing: i64,
}

#[derive(Clone, Deserialize, Serialize, FromRow)]
pub struct RevisionView {
    pub id: Uuid,
    pub source_id: i64,
    pub settings_revision: i64,
    pub identity_epoch: i64,
    pub parser_version: String,
    pub format: String,
    pub parsed_at: i64,
    #[sqlx(json)]
    pub counts: NodeCounts,
}

#[derive(Clone, Deserialize, Serialize, FromRow)]
pub struct JobView {
    pub id: Uuid,
    pub source_id: i64,
    pub settings_revision: i64,
    pub identity_epoch: i64,
    pub parser_version: String,
    pub status: String,
    pub stage: String,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub source_revision_id: Option<Uuid>,
    #[sqlx(json(nullable))]
    pub error: Option<SourceFailure>,
}

#[derive(Serialize)]
pub struct SourceView {
    pub id: i64,
    pub name: String,
    pub kind: String,
    pub host: Option<String>,
    pub configured: bool,
    pub auth_configured: bool,
    pub settings_revision: i64,
    pub identity_epoch: i64,
    pub archived: bool,
    pub refresh_interval_secs: i64,
    pub user_agent: Option<String>,
    pub auto_refresh: bool,
    pub traffic: Value,
    pub changes: Value,
    pub last_attempt_at: Option<i64>,
    pub last_success_at: Option<i64>,
    pub latest_success: Option<RevisionView>,
    pub active_job: Option<JobView>,
    pub last_error: Option<SourceFailure>,
    pub stale_reason: Option<String>,
    pub counts: NodeCounts,
    pub dependencies: Vec<Value>,
}

#[derive(Clone, Serialize)]
pub struct NodeView {
    pub id: Uuid,
    pub public_id: i64,
    pub adopted: bool,
    /// Catalog metadata revision shared with the node catalog; 0 when none.
    pub metadata_revision: i64,
    pub source_id: i64,
    pub identity_epoch: i64,
    pub version_id: Uuid,
    pub source_revision_id: Uuid,
    pub present_in_latest: bool,
    pub identity_state: String,
    pub supported: bool,
    pub selectable: bool,
    pub reasons: Vec<String>,
    pub capabilities: Value,
    #[serde(flatten)]
    pub preview: Value,
}

#[derive(Serialize)]
pub struct NodePage {
    pub source_id: i64,
    pub current_settings_revision: i64,
    pub current_identity_epoch: i64,
    pub success_revision: Option<RevisionView>,
    pub nodes: Vec<NodeView>,
}

#[derive(Serialize)]
pub struct HistoryPage {
    pub source_id: i64,
    pub revisions: Vec<RevisionView>,
}

#[derive(FromRow)]
pub(super) struct SourceRow {
    pub id: i64,
    pub name: String,
    pub kind: String,
    pub host: Option<String>,
    pub input_config: Value,
    pub settings_revision: i64,
    pub identity_epoch: i64,
    pub archived: bool,
    pub deleted_at: Option<i64>,
    pub refresh_interval_secs: i64,
    pub next_refresh_at: Option<i64>,
    pub last_attempt_at: Option<i64>,
    pub last_success_at: Option<i64>,
    pub current_success_revision: Option<Uuid>,
    pub last_error: Option<Value>,
    pub conditional_etag: Option<String>,
    pub conditional_last_modified: Option<String>,
    pub conditional_settings_revision: Option<i64>,
    pub conditional_identity_epoch: Option<i64>,
    pub user_agent: Option<String>,
    pub auto_refresh: bool,
    pub traffic: Value,
    pub changes: Value,
}

pub(super) const SOURCE_COLUMNS: &str = "id,name,kind,host,input_config,settings_revision,identity_epoch,archived,deleted_at,refresh_interval_secs,next_refresh_at,last_attempt_at,last_success_at,current_success_revision,last_error,conditional_etag,conditional_last_modified,conditional_settings_revision,conditional_identity_epoch,user_agent,auto_refresh,traffic,changes";
pub(super) const REVISION_COLUMNS: &str =
    "id,source_id,settings_revision,identity_epoch,parser_version,format,parsed_at,counts";
pub(super) const JOB_COLUMNS: &str = "id,source_id,settings_revision,identity_epoch,parser_version,status,stage,created_at,started_at,finished_at,source_revision_id,error";
