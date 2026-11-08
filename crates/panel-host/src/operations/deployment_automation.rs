//! Deployment steps that operations jobs run through the traffic runtime.
//!
//! Operations decides when a step runs and records the attempt; whether the
//! deployment is valid and how it is applied stays with the runtime's owner,
//! which answers this interface. The application connects it.

use apsp_protocol::RuntimeCheckpointRequest;
use serde_json::Value;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::{
    AppState,
    error::{ApiError, ApiResult},
    ports::{BoxFuture, Port},
};

/// Runtime version and capability a runtime step requires.
#[derive(Clone, Copy)]
pub struct RuntimeStepPolicy {
    pub version: &'static str,
    pub capability: &'static str,
}

/// The immutable deployment candidate and the actor bound to one task.
pub struct DeploymentIntent<'a> {
    pub server: i64,
    pub job: Uuid,
    pub actor: i64,
    pub candidate: &'a Value,
    pub expires: i64,
}

pub trait DeploymentAutomation: Send + Sync {
    fn runtime_step_policy(&self, kind: &str) -> Option<RuntimeStepPolicy>;
    fn candidate<'a, 't>(
        &'a self,
        state: &'a AppState,
        tx: &'a mut Transaction<'t, Postgres>,
        server: i64,
    ) -> BoxFuture<'a, ApiResult<Value>>;
    fn enqueue_tx<'a, 't>(
        &'a self,
        state: &'a AppState,
        tx: &'a mut Transaction<'t, Postgres>,
        intent: DeploymentIntent<'a>,
    ) -> BoxFuture<'a, ApiResult<Uuid>>;
    fn cancel_tx<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        request: Uuid,
    ) -> BoxFuture<'a, ApiResult<()>>;
    fn dispatch_matches_tx<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        server: i64,
        request: Uuid,
    ) -> BoxFuture<'a, ApiResult<bool>>;
    fn receipt<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        request: Uuid,
    ) -> BoxFuture<'a, ApiResult<Value>>;
    fn request_checkpoint_tx<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        original: Uuid,
    ) -> BoxFuture<'a, ApiResult<RuntimeCheckpointRequest>>;
    fn reconcile_tx<'a, 't>(
        &'a self,
        tx: &'a mut Transaction<'t, Postgres>,
        original: Uuid,
        checkpoint: Uuid,
        evidence: &'a str,
    ) -> BoxFuture<'a, ApiResult<Value>>;
}

static AUTOMATION: Port<dyn DeploymentAutomation> = Port::new();

pub fn install(automation: &'static dyn DeploymentAutomation) {
    AUTOMATION.install(automation);
}

fn required() -> ApiResult<&'static dyn DeploymentAutomation> {
    AUTOMATION
        .get()
        .ok_or_else(|| ApiError::Conflict("执行器提供方未登记，无法确认能力或安全执行".into()))
}

pub fn runtime_step_policy(kind: &str) -> Option<RuntimeStepPolicy> {
    AUTOMATION
        .get()
        .and_then(|automation| automation.runtime_step_policy(kind))
}

pub async fn automation_candidate(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    server: i64,
) -> ApiResult<Value> {
    required()?.candidate(state, tx, server).await
}

pub async fn enqueue_automation_deployment_tx(
    state: &AppState,
    tx: &mut Transaction<'_, Postgres>,
    server: i64,
    job: Uuid,
    actor: i64,
    candidate: &Value,
    expires: i64,
) -> ApiResult<Uuid> {
    required()?
        .enqueue_tx(
            state,
            tx,
            DeploymentIntent {
                server,
                job,
                actor,
                candidate,
                expires,
            },
        )
        .await
}

pub async fn cancel_automation_deployment_tx(
    tx: &mut Transaction<'_, Postgres>,
    request: Uuid,
) -> ApiResult<()> {
    required()?.cancel_tx(tx, request).await
}

pub async fn automation_dispatch_matches_tx(
    tx: &mut Transaction<'_, Postgres>,
    server: i64,
    request: Uuid,
) -> ApiResult<bool> {
    required()?.dispatch_matches_tx(tx, server, request).await
}

pub async fn automation_deployment_receipt(
    tx: &mut Transaction<'_, Postgres>,
    request: Uuid,
) -> ApiResult<Value> {
    required()?.receipt(tx, request).await
}

pub async fn request_automation_deployment_checkpoint_tx(
    tx: &mut Transaction<'_, Postgres>,
    original: Uuid,
) -> ApiResult<RuntimeCheckpointRequest> {
    required()?.request_checkpoint_tx(tx, original).await
}

pub async fn reconcile_automation_deployment_tx(
    tx: &mut Transaction<'_, Postgres>,
    original: Uuid,
    checkpoint: Uuid,
    evidence: &str,
) -> ApiResult<Value> {
    required()?
        .reconcile_tx(tx, original, checkpoint, evidence)
        .await
}
