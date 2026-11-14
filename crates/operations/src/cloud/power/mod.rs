//! ECS power automation: the power policy and its form (`api`, `policy`, `cron`), manual and
//! automatic power jobs (`jobs`, `scheduler`), the CDT guard (`guard`) and the cloud calls
//! (`state`). The stored shapes are in `model`; operations keeps them in its own table
//! (`crate::cloud::automation`).

mod api;
mod cron;
mod guard;
mod jobs;
pub mod model;
mod policy;
mod scheduler;
mod state;
#[cfg(test)]
mod tests;

pub use api::routes;
pub use jobs::{cancel_pending, idle, recent, unresolved};

use apsp_panel_host::maintenance::{self, Cadence, Pass};
use model::State;
use serde::Serialize;
use sqlx::{FromRow, types::Json};
use std::time::Duration;
use uuid::Uuid;

pub async fn run(pool: sqlx::PgPool) {
    maintenance::periodic(
        "alicloud-power",
        pool.clone(),
        Cadence::every(Duration::from_secs(5)),
        pool,
        |pool| Box::pin(reconcile(pool)),
    )
    .await
}

async fn reconcile(pool: &sqlx::PgPool) -> Pass {
    let result = match apsp_resources::cloud::client::Cloud::new(pool) {
        Ok(cloud) => jobs::tick(pool, &cloud).await,
        Err(error) => Err(apsp_resources::cloud::failure(error)),
    };
    match result {
        Ok(()) => Ok(
            serde_json::json!({"scope":"power_scheduler_and_reconciliation_cycle","completed":true,"remote_resource_health_confirmed":false}),
        ),
        Err(error) => Err(maintenance::Failure::new(
            &error,
            serde_json::json!({"scope":"power_scheduler_and_reconciliation_cycle","completed":false,"error":error.to_string(),"remote_resource_health_confirmed":false}),
        )),
    }
}

#[derive(Serialize, FromRow)]
pub struct Job {
    pub id: Uuid,
    pub resource_id: Uuid,
    pub account_revision: i64,
    pub resource_revision: i64,
    pub action: String,
    pub stop_mode: String,
    pub source: String,
    pub dedup_key: Option<String>,
    pub before_state: Json<State>,
    pub status: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub updated_at: i64,
    pub next_check_at: i64,
    pub request_id: Option<String>,
    pub error_code: Option<String>,
}
