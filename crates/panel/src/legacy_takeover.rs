//! `apsp-panel legacy-takeover`: the manual takeover of legacy two-hop
//! chains as ordered chains (ADR 0079 phase 3, step S1c). Nothing runs unless
//! an operator invokes it. Results are JSON on standard output; logs go to
//! standard error. The exit code is 0 only when every selected chain passed.

use anyhow::{Context, Result, bail};
use apsp_panel::{AppState, config::Config, plugins::singbox::legacy_takeover};
use sqlx::postgres::PgPoolOptions;

const USAGE: &str =
    "usage: apsp-panel legacy-takeover precheck | apply | rollback [--chain <id>]...";

fn chains(args: &[String]) -> Result<Option<Vec<i64>>> {
    let mut chains = Vec::new();
    let mut rest = args.iter();
    while let Some(flag) = rest.next() {
        if flag != "--chain" {
            bail!(USAGE);
        }
        let value = rest.next().context(USAGE)?;
        chains.push(value.parse::<i64>().context("--chain takes a chain id")?);
    }
    Ok((!chains.is_empty()).then_some(chains))
}

pub async fn run(args: &[String]) -> Result<i32> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .init();
    let command = args.first().map(String::as_str).unwrap_or_default();
    if !matches!(command, "precheck" | "apply" | "rollback") {
        bail!(USAGE);
    }
    let selected = chains(&args[1..])?;
    let config = Config::from_env()?;
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&config.database_url)
        .await?;
    let state = AppState::new(pool, config).await?;
    apsp_panel::plugins::install();
    let report = match command {
        "precheck" => legacy_takeover::precheck(&state, selected).await?,
        "apply" => legacy_takeover::apply(&state, selected).await?,
        _ => legacy_takeover::rollback(&state, selected).await?,
    };
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(i32::from(!report.succeeded()))
}
