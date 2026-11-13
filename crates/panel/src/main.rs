#![forbid(unsafe_code)]

use apsp_panel::router;
use apsp_panel_host::config::Config;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::str::FromStr;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    apsp_panel::domains::install();
    let config = Config::from_env()?;
    let listen = config.listen;
    // No JIT: the panel's statements are short, but the planner's estimates for
    // the set-returning functions in the entitlement path run into billions, so
    // PostgreSQL compiled, inlined and optimized them with LLVM on every
    // execution (about 0.8 s of compilation for 0.3 s of work).
    let options = PgConnectOptions::from_str(&config.database_url)?.options([("jit", "off")]);
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect_with(options)
        .await?;
    let state = apsp_panel::state(pool, config).await?;
    let recovery_verified = match std::env::var("APSP_RECOVERY_VERIFY_CREDENTIALS") {
        Ok(value) if value == "true" => {
            let count =
                apsp_governance::control_center::credentials::verify_recovery_material(&state.pool)
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
    let workers = tokio::spawn(apsp_panel::domains::run(state.clone()));
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
    workers.abort();
    result?;
    Ok(())
}
