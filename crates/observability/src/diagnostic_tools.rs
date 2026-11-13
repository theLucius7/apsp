//! Diagnostic tools that the shared diagnostic job service runs: NodeQuality, TCP quality,
//! IP quality and network checks. Each tool converts parameters, runs and parses its
//! report; the service owns the job lifecycle (ADR 0028).

use crate::diagnostics::service::DiagnosticTool;
pub mod ipquality;
pub mod nodequality;
pub mod tcpquality;
static NODEQUALITY: nodequality::NodeQualityTool = nodequality::NodeQualityTool;
static NODE_IPQUALITY: nodequality::node_queries::NodeIpQualityTool =
    nodequality::node_queries::NodeIpQualityTool;
static TCPQUALITY: tcpquality::TcpQualityTool = tcpquality::TcpQualityTool;
static IPQUALITY: ipquality::IpQualityTool = ipquality::IpQualityTool;
static NETWORK_WORKBENCH: crate::network_workbench::NetworkWorkbenchTool =
    crate::network_workbench::NetworkWorkbenchTool;
static REGISTERED: [&dyn DiagnosticTool; 4] =
    [&NODEQUALITY, &TCPQUALITY, &IPQUALITY, &NETWORK_WORKBENCH];
pub fn all() -> &'static [&'static dyn DiagnosticTool] {
    &REGISTERED
}
pub fn find(id: &str) -> Option<&'static dyn DiagnosticTool> {
    all().iter().copied().find(|plugin| plugin.id() == id)
}

/// Jobs created before plugin registration belong to the original diagnostic plugin.
pub fn for_job(job: &serde_json::Value) -> Option<&'static dyn DiagnosticTool> {
    if job["plugin"].as_str() == Some("nodequality")
        && job["version"].as_str() == Some(nodequality::node_queries::NODE_QUERY_VERSION)
        && job["options"]["mode"].as_str() == Some("ip")
    {
        return Some(&NODE_IPQUALITY);
    }
    find(job["plugin"].as_str().unwrap_or("nodequality"))
}
