//! What other owners contribute to administration: route permissions, audit
//! redaction and snapshots, search categories, worker names and signed tool
//! packages. Declared here, answered by the owners of those routes and objects,
//! and connected by the application.

use axum::http::Method;
use serde_json::Value;

use crate::{
    AppState,
    artifacts::ArtifactEntry,
    error::ApiResult,
    ports::{BoxFuture, Port},
};

/// Authorization facts supplied by the owner of a route.
#[derive(Clone, Copy, Default)]
pub struct RoutePolicy {
    pub independent_identity: bool,
    pub feature: Option<&'static str>,
    pub high_risk: bool,
    pub read: bool,
    pub scoped_objects: bool,
}

/// One search category whose SQL belongs to its owner; visibility is bound first.
pub struct SearchSource {
    pub category: &'static str,
    pub capability: &'static str,
    pub global_only: bool,
    pub extra_ctes: &'static str,
    pub select: &'static str,
}

pub trait Contributions: Send + Sync {
    fn route_policy(&self, path: &str, method: &Method) -> Option<RoutePolicy>;
    fn sensitive_key(&self, key: &str) -> bool;
    fn search_sources(&self) -> Vec<SearchSource>;
    fn worker_names(&self) -> &'static [&'static str];
    fn signed_tool_packages(&self, entries: &[ArtifactEntry]) -> Vec<Value>;
    fn audit_snapshot<'a>(
        &'a self,
        state: &'a AppState,
        path: &'a str,
    ) -> BoxFuture<'a, ApiResult<Option<Value>>>;
}

static CONTRIBUTIONS: Port<dyn Contributions> = Port::new();

pub fn install(contributions: &'static dyn Contributions) {
    CONTRIBUTIONS.install(contributions);
}

pub fn route_policy(path: &str, method: &Method) -> Option<RoutePolicy> {
    CONTRIBUTIONS
        .get()
        .and_then(|contributions| contributions.route_policy(path, method))
}

pub fn sensitive_key(key: &str) -> bool {
    CONTRIBUTIONS
        .get()
        .is_some_and(|contributions| contributions.sensitive_key(key))
}

pub fn search_sources() -> Vec<SearchSource> {
    CONTRIBUTIONS
        .get()
        .map(|contributions| contributions.search_sources())
        .unwrap_or_default()
}

pub fn worker_names() -> &'static [&'static str] {
    CONTRIBUTIONS
        .get()
        .map(|contributions| contributions.worker_names())
        .unwrap_or_default()
}

pub fn signed_tool_packages(entries: &[ArtifactEntry]) -> Vec<Value> {
    CONTRIBUTIONS
        .get()
        .map(|contributions| contributions.signed_tool_packages(entries))
        .unwrap_or_default()
}

pub async fn audit_snapshot(state: &AppState, path: &str) -> ApiResult<Option<Value>> {
    match CONTRIBUTIONS.get() {
        Some(contributions) => contributions.audit_snapshot(state, path).await,
        None => Ok(None),
    }
}
