//! Content digests: the hex SHA-256 of a value's JSON serialization.

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::error::ApiResult;

/// The digest of `value` as serde writes it: a `serde_json::Value` with its keys in
/// order, a struct in field order. Callers compare digests of the same type only.
pub fn json(value: &impl Serialize) -> ApiResult<String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(value).map_err(anyhow::Error::from)?)
    ))
}
