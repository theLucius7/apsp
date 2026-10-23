use apsp_compiler::external::ExternalOutbound;
use base64::{
    Engine,
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD},
};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    sync::{Arc, OnceLock},
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, TryAcquireError};

pub const MAX_BODY: usize = 2 * 1024 * 1024;
pub const MAX_NODES: usize = 5000;
pub const MAX_DEPTH: usize = 64;
pub const MAX_SCALAR: usize = 64 * 1024;
pub const PARSER_VERSION: &str = "apsp-subscriptions-2";

const PARSER_CONCURRENCY: usize = 4;
static PARSER_PERMITS: OnceLock<Arc<Semaphore>> = OnceLock::new();

fn new_admission() -> Arc<Semaphore> {
    Arc::new(Semaphore::new(PARSER_CONCURRENCY))
}

pub(super) fn try_admit() -> Result<OwnedSemaphorePermit, TryAcquireError> {
    PARSER_PERMITS
        .get_or_init(new_admission)
        .clone()
        .try_acquire_owned()
}

async fn blocking_with_permit<T: Send + 'static>(
    permit: OwnedSemaphorePermit,
    operation: impl FnOnce() -> Result<T, ImportError> + Send + 'static,
) -> Result<(OwnedSemaphorePermit, T), ImportError> {
    tokio::task::spawn_blocking(move || {
        // A cancelled waiter cannot stop an already-running blocking task.
        // Its real work therefore owns the admission slot until completion.
        let result = operation()?;
        // Successful parsed results remain admitted while their caller waits
        // for persistence; otherwise large batches could accumulate unbounded.
        Ok((permit, result))
    })
    .await
    .map_err(|_| ImportError("parser_interrupted"))?
}

pub(super) async fn admitted_parse(
    body: Vec<u8>,
    permit: OwnedSemaphorePermit,
) -> Result<(Vec<u8>, String, ParsedBatch, OwnedSemaphorePermit), ImportError> {
    let (permit, (body, body_sha256, batch)) = blocking_with_permit(permit, move || {
        let batch = parse(&body)?;
        let body_sha256 = digest(&body);
        Ok((body, body_sha256, batch))
    })
    .await?;
    Ok((body, body_sha256, batch, permit))
}

#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct ImportError(pub &'static str);

#[derive(Clone)]
pub struct ParsedNode {
    pub name: String,
    pub identity_key: String,
    pub outbound: ExternalOutbound,
}

#[derive(Clone, Serialize)]
pub struct RejectedNode {
    pub index: usize,
    pub name: String,
    pub reason: String,
}

pub struct ParsedBatch {
    pub format: &'static str,
    pub nodes: Vec<ParsedNode>,
    pub rejected: Vec<RejectedNode>,
    pub ambiguous_keys: Vec<String>,
}

pub fn digest(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}

pub fn parse(bytes: &[u8]) -> Result<ParsedBatch, ImportError> {
    if bytes.len() > MAX_BODY {
        return Err(ImportError("body_limit"));
    }
    let text = std::str::from_utf8(bytes)
        .map_err(|_| ImportError("invalid_utf8"))?
        .trim_start_matches('\u{feff}')
        .trim();
    if text.is_empty() {
        return Err(ImportError("empty_configuration"));
    }
    if text.starts_with('<') {
        return Err(ImportError("html_response"));
    }
    if text.starts_with('{') || text.starts_with('[') {
        return from_object(super::structured::json(text)?, "sing_box_json");
    }
    if text
        .lines()
        .map(str::trim)
        .find(|v| !v.is_empty() && !v.starts_with('#'))
        .is_some_and(|v| {
            v.split_once("://").is_some_and(|(scheme, _)| {
                !scheme.is_empty()
                    && scheme
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'+' | b'-' | b'.'))
            })
        })
    {
        return from_uris(text, "uri");
    }
    if !text.contains(':') {
        let decoded = decode_base64(text)?;
        let decoded = std::str::from_utf8(&decoded)
            .map_err(|_| ImportError("invalid_base64_configuration"))?;
        if !decoded.contains("://") {
            return Err(ImportError("invalid_base64_configuration"));
        }
        return from_uris(decoded, "base64_uri");
    }
    from_object(super::structured::yaml(text)?, "clash_yaml")
}

fn from_object(value: Value, format: &'static str) -> Result<ParsedBatch, ImportError> {
    let root = value
        .as_object()
        .ok_or(ImportError("configuration_must_be_object"))?;
    let (nodes, is_clash) = if let Some(outbounds) = root.get("outbounds") {
        if root.contains_key("proxies") || root.contains_key("payload") {
            return Err(ImportError("ambiguous_configuration_format"));
        }
        (outbounds, false)
    } else if let Some(proxies) = root.get("proxies").or_else(|| root.get("payload")) {
        if root.contains_key("proxies") && root.contains_key("payload") {
            return Err(ImportError("ambiguous_configuration_format"));
        }
        (proxies, true)
    } else {
        return Err(ImportError(if root.contains_key("proxy-providers") {
            "provider_url_without_nodes"
        } else {
            "missing_proxy_nodes"
        }));
    };
    let nodes = nodes.as_array().ok_or(ImportError("nodes_must_be_array"))?;
    if nodes.is_empty() || nodes.len() > MAX_NODES {
        return Err(ImportError("node_count_limit"));
    }
    let mut batch = ParsedBatch {
        format: if is_clash { "clash_yaml" } else { format },
        nodes: Vec::new(),
        rejected: Vec::new(),
        ambiguous_keys: Vec::new(),
    };
    for (index, value) in nodes.iter().enumerate() {
        let name = public_name(
            value
                .get(if is_clash { "name" } else { "tag" })
                .and_then(Value::as_str),
            index,
        );
        let result = if is_clash {
            super::mihomo::convert(value.clone())
        } else {
            sing_box(value.clone())
        };
        append(&mut batch, index, name, result);
    }
    identify(batch)
}

fn sing_box(mut value: Value) -> Result<(ExternalOutbound, Option<String>), ImportError> {
    let object = value
        .as_object_mut()
        .ok_or(ImportError("invalid_node_object"))?;
    let provider = provider_id(object.remove("provider_id"))?;
    object.remove("tag");
    // Dialer dependencies cannot be silently removed without changing node semantics.
    let outbound = ExternalOutbound(value);
    outbound
        .validate()
        .map_err(|_| ImportError("unsupported_or_invalid_proxy_parameter"))?;
    Ok((outbound, provider))
}

fn from_uris(text: &str, format: &'static str) -> Result<ParsedBatch, ImportError> {
    let mut batch = ParsedBatch {
        format,
        nodes: Vec::new(),
        rejected: Vec::new(),
        ambiguous_keys: Vec::new(),
    };
    for line in text
        .lines()
        .map(str::trim)
        .filter(|v| !v.is_empty() && !v.starts_with('#'))
    {
        let index = batch.nodes.len() + batch.rejected.len();
        if index >= MAX_NODES {
            return Err(ImportError("node_count_limit"));
        }
        if line.len() > MAX_SCALAR {
            return Err(ImportError("scalar_limit"));
        }
        let result = super::uri::parse(line);
        let name = result
            .as_ref()
            .map(|v| public_name(Some(&v.1), index))
            .unwrap_or_else(|_| public_name(None, index));
        append(&mut batch, index, name, result.map(|v| (v.0, None)));
    }
    if batch.nodes.is_empty() && batch.rejected.is_empty() {
        return Err(ImportError("empty_configuration"));
    }
    identify(batch)
}

fn append(
    batch: &mut ParsedBatch,
    index: usize,
    name: String,
    result: Result<(ExternalOutbound, Option<String>), ImportError>,
) {
    match result {
        Ok((mut outbound, provider)) => {
            let server = outbound
                .server()
                .parse::<std::net::IpAddr>()
                .map(|v| v.to_string())
                .unwrap_or_else(|_| outbound.server().to_ascii_lowercase());
            outbound.0["server"] = Value::String(server);
            if let Some(Value::String(sni)) = outbound.0.pointer_mut("/tls/server_name") {
                *sni = sni.to_ascii_lowercase();
            }
            let identity_key = match provider {
                Some(value) => format!("provider:{}", digest(value.as_bytes())),
                None => format!(
                    "endpoint:{}",
                    digest(&serde_json::to_vec(&outbound.identity_value()).expect("JSON value"))
                ),
            };
            batch.nodes.push(ParsedNode {
                name,
                identity_key,
                outbound,
            });
        }
        Err(error) => batch.rejected.push(RejectedNode {
            index,
            name,
            reason: error.0.into(),
        }),
    }
}

fn identify(mut batch: ParsedBatch) -> Result<ParsedBatch, ImportError> {
    let mut counts = BTreeMap::<String, usize>::new();
    for node in &batch.nodes {
        *counts.entry(node.identity_key.clone()).or_default() += 1;
    }
    batch.ambiguous_keys = counts
        .into_iter()
        .filter_map(|(key, count)| (count > 1).then_some(key))
        .collect();
    let mut nodes = Vec::new();
    for (index, node) in batch.nodes.into_iter().enumerate() {
        if batch
            .ambiguous_keys
            .binary_search(&node.identity_key)
            .is_ok()
        {
            batch.rejected.push(RejectedNode {
                index,
                name: node.name,
                reason: "ambiguous_node_identity".into(),
            });
        } else {
            nodes.push(node);
        }
    }
    batch.nodes = nodes;
    Ok(batch)
}

pub(super) fn provider_id(value: Option<Value>) -> Result<Option<String>, ImportError> {
    match value {
        None => Ok(None),
        Some(Value::String(value))
            if !value.trim().is_empty()
                && value.len() <= 256
                && !value.chars().any(char::is_control) =>
        {
            Ok(Some(value))
        }
        _ => Err(ImportError("invalid_provider_identity")),
    }
}

pub(super) fn public_name(name: Option<&str>, index: usize) -> String {
    let name = name.unwrap_or_default().trim();
    if name.is_empty() || name.contains("://") || name.chars().any(char::is_control) {
        format!("节点 {}", index + 1)
    } else {
        name.chars().take(160).collect()
    }
}

pub(super) fn decode_base64(value: &str) -> Result<Vec<u8>, ImportError> {
    let value: String = value.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    if value.len() > MAX_BODY {
        return Err(ImportError("body_limit"));
    }
    for engine in [&STANDARD, &STANDARD_NO_PAD, &URL_SAFE, &URL_SAFE_NO_PAD] {
        if let Ok(bytes) = engine.decode(&value)
            && bytes.len() <= MAX_BODY
        {
            return Ok(bytes);
        }
    }
    Err(ImportError("invalid_base64_configuration"))
}

#[cfg(test)]
mod admission_tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn cancellation_keeps_real_blocking_work_in_the_shared_budget() {
        let permits = new_admission();
        let occupied: Vec<_> = (0..PARSER_CONCURRENCY - 1)
            .map(|_| permits.clone().try_acquire_owned().expect("available slot"))
            .collect();
        let permit = permits.clone().try_acquire_owned().expect("last slot");
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let waiter = tokio::spawn(blocking_with_permit(permit, move || {
            let _ = started_tx.send(());
            release_rx
                .recv_timeout(Duration::from_secs(2))
                .map_err(|_| ImportError("fixture_timeout"))?;
            Ok(())
        }));
        tokio::time::timeout(Duration::from_secs(2), started_rx)
            .await
            .expect("bounded task start")
            .expect("started");
        waiter.abort();
        assert!(waiter.await.expect_err("cancelled waiter").is_cancelled());
        assert_eq!(permits.available_permits(), 0);
        assert!(permits.clone().try_acquire_owned().is_err());
        release_tx.send(()).expect("release real blocking task");
        let returned =
            tokio::time::timeout(Duration::from_secs(2), permits.clone().acquire_owned())
                .await
                .expect("slot returned after completion")
                .expect("open admission");
        assert_eq!(permits.available_permits(), 0);
        drop(returned);
        drop(occupied);
        assert_eq!(permits.available_permits(), PARSER_CONCURRENCY);
    }

    #[tokio::test]
    async fn parser_success_rejection_and_interruption_return_only_their_owned_slot() {
        let permits = new_admission();
        let input = b"http://proxy.example.com:443#TEST_ONLY-node".to_vec();
        let permit = permits.clone().try_acquire_owned().expect("available slot");
        let (body, body_sha256, batch, pending) = admitted_parse(input.clone(), permit)
            .await
            .expect("parsed fixture");
        assert_eq!(body, input);
        assert_eq!(body_sha256, digest(&input));
        assert_eq!(batch.nodes.len(), 1);
        assert_eq!(permits.available_permits(), PARSER_CONCURRENCY - 1);
        drop(pending);
        assert_eq!(permits.available_permits(), PARSER_CONCURRENCY);
        let permit = permits.clone().try_acquire_owned().expect("available slot");
        assert!(matches!(
            admitted_parse(vec![b'x'; MAX_BODY + 1], permit).await,
            Err(ImportError("body_limit"))
        ));
        assert_eq!(permits.available_permits(), PARSER_CONCURRENCY);
        let permit = permits.clone().try_acquire_owned().expect("available slot");
        let result: Result<(OwnedSemaphorePermit, ()), ImportError> =
            blocking_with_permit(permit, || panic!("TEST_ONLY parser interruption")).await;
        assert!(matches!(result, Err(ImportError("parser_interrupted"))));
        assert_eq!(permits.available_permits(), PARSER_CONCURRENCY);
    }

    #[tokio::test]
    async fn parsed_pending_results_keep_the_slot_until_persist_or_cancellation() {
        let permits = new_admission();
        let occupied: Vec<_> = (0..PARSER_CONCURRENCY - 1)
            .map(|_| permits.clone().try_acquire_owned().expect("available slot"))
            .collect();
        let permit = permits.clone().try_acquire_owned().expect("last slot");
        let (parsed_tx, parsed_rx) = tokio::sync::oneshot::channel();
        let (finish_tx, finish_rx) = tokio::sync::oneshot::channel();
        let pending = tokio::spawn(async move {
            let (_body, _body_sha256, _batch, _permit) = admitted_parse(
                b"http://proxy.example.com:443#TEST_ONLY-pending".to_vec(),
                permit,
            )
            .await
            .expect("parsed fixture");
            let _ = parsed_tx.send(());
            let _ = finish_rx.await;
        });
        tokio::time::timeout(Duration::from_secs(2), parsed_rx)
            .await
            .expect("bounded parser completion")
            .expect("pending persistence");
        assert_eq!(permits.available_permits(), 0);
        assert!(permits.clone().try_acquire_owned().is_err());
        pending.abort();
        assert!(
            pending
                .await
                .expect_err("cancelled pending result")
                .is_cancelled()
        );
        assert_eq!(permits.available_permits(), 1);
        assert!(finish_tx.send(()).is_err());
        drop(occupied);
        assert_eq!(permits.available_permits(), PARSER_CONCURRENCY);
    }
}
