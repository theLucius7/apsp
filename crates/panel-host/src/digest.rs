//! Content digests: the hex SHA-256 of a value's JSON serialization.

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::error::ApiResult;

pub fn json(value: &Value) -> ApiResult<String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(value).map_err(anyhow::Error::from)?)
    ))
}
