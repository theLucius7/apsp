use crate::ServerId;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const RETIREMENT_CAPABILITY: &str = "server:retire-v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetirementRequest {
    pub request_id: Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetirementReceipt {
    pub server_id: ServerId,
    pub request_id: Uuid,
    pub signature: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetirementResult {
    pub request_id: Uuid,
    pub success: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt: Option<RetirementReceipt>,
}

/// Domain-separated completion proof; it cannot authenticate a device session.
pub fn retirement_receipt_message(server_id: ServerId, request_id: Uuid) -> Vec<u8> {
    let mut message = b"apsp-retirement-v1\0".to_vec();
    message.extend_from_slice(&server_id.to_be_bytes());
    message.extend_from_slice(request_id.as_bytes());
    message
}
