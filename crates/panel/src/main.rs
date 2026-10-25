#![forbid(unsafe_code)]

mod legacy_takeover;
mod source_migration;

use apsp_panel::{AppState, config::Config, maintenance::supervise, router};
use sqlx::{PgPool, postgres::PgPoolOptions};

async fn telemetry_history(pool: PgPool) {
    let mut timer = tokio::time::interval(std::time::Duration::from_secs(60));
    timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        timer.tick().await;
        let result = apsp_panel::telemetry::maintain(&pool).await;
        let status = if result.is_ok() { "healthy" } else { "failed" };
        if let Err(error) = result {
            tracing::warn!(%error, "telemetry history maintenance failed");
        }
        if let Err(error) = apsp_panel::control_center::system::heartbeat(
            &pool,
            "telemetry-history",
            status,
            serde_json::json!({"source":"history-maintenance-cycle"}),
        )
        .await
        {
            tracing::warn!(%error,"telemetry worker heartbeat failed");
        }
    }
}

async fn network_workbench(state: AppState) {
    let mut timer = tokio::time::interval(std::time::Duration::from_secs(5));
    timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        timer.tick().await;
        let result = apsp_panel::network_workbench::tick(&state).await;
        let status = if result.is_ok() { "healthy" } else { "failed" };
        if let Err(error) = result {
            tracing::warn!(%error,"network workbench dispatch failed");
        }
        if let Err(error) = apsp_panel::control_center::system::heartbeat(
            &state.pool,
            "network-workbench",
            status,
            serde_json::json!({"source":"dispatch-cycle"}),
        )
        .await
        {
            tracing::warn!(%error,"network workbench heartbeat failed");
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Maintenance commands run only when invoked; the service ignores other arguments.
    match args.first().map(String::as_str) {
        Some("source-migration") => std::process::exit(source_migration::run(&args[1..]).await?),
        Some("legacy-takeover") => std::process::exit(legacy_takeover::run(&args[1..]).await?),
        _ => {}
    }
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    apsp_panel::plugins::install();
    let config = Config::from_env()?;
    let listen = config.listen;
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(&config.database_url)
        .await?;
    apsp_panel::schema::prepare(&pool).await?;
    let state = AppState::new(pool, config).await?;
    let recovery_verified = match std::env::var("APSP_RECOVERY_VERIFY_CREDENTIALS") {
        Ok(value) if value == "true" => {
            let count =
                apsp_panel::control_center::credentials::verify_recovery_material(&state.pool)
                    .await?;
            tracing::info!(
                verified_credentials = count,
                "restored credential material authenticated"
            );
            true
        }
        Ok(value) if value == "false" => false,
        Err(std::env::VarError::NotPresent) => false,
        _ => anyhow::bail!("APSP_RECOVERY_VERIFY_CREDENTIALS must be true or false"),
    };
    let listener = tokio::net::TcpListener::bind(listen).await?;
    tracing::info!(address = %listener.local_addr()?, "panel started");
    // Background loops are supervised so a panic restarts them instead of
    // silently stopping alerts, renewals, publication or history maintenance.
    let maintenance = tokio::spawn(supervise("maintenance", {
        let state = state.clone();
        move || apsp_panel::maintenance::run(state.clone())
    }));
    let exchange = tokio::spawn(supervise("exchange-rates", {
        let pool = state.pool.clone();
        move || apsp_panel::exchange::run(pool.clone())
    }));
    let telemetry = tokio::spawn(supervise("telemetry-history", {
        let pool = state.pool.clone();
        move || telemetry_history(pool.clone())
    }));
    let plugins = tokio::spawn(apsp_panel::plugins::run(state.clone()));
    let control_center = tokio::spawn(supervise("control-center", {
        let state = state.clone();
        move || apsp_panel::control_center::run(state.clone())
    }));
    let operations = tokio::spawn(supervise("operations", {
        let state = state.clone();
        move || apsp_panel::operations::run(state.clone())
    }));
    let network = tokio::spawn(supervise("network-workbench", {
        let state = state.clone();
        move || network_workbench(state.clone())
    }));
    let certificates = tokio::spawn(supervise("network-certificates", {
        let state = state.clone();
        move || apsp_panel::network_configuration::run(state.clone())
    }));
    let mut application = router(state);
    if recovery_verified {
        application = application.layer(axum::middleware::from_fn(
            |request: axum::extract::Request, next: axum::middleware::Next| async move {
                let health_request = request.uri().path() == "/healthz";
                let mut response = next.run(request).await;
                if health_request && response.status().is_success() {
                    response.headers_mut().insert(
                        "x-apsp-recovery-material-verified",
                        axum::http::HeaderValue::from_static("true"),
                    );
                }
                response
            },
        ));
    }
    let result = axum::serve(
        listener,
        application.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await;
    maintenance.abort();
    exchange.abort();
    telemetry.abort();
    plugins.abort();
    control_center.abort();
    operations.abort();
    network.abort();
    certificates.abort();
    result?;
    Ok(())
}
