//! `apsp-panel source-migration`: the manual move of numbered subscription
//! sources into ordered sources (ADR 0079 phase 3, step S1b). Nothing runs
//! unless an operator invokes it. Results are JSON on standard output; logs go
//! to standard error. The exit code is 0 only when the step succeeded, the
//! precheck found no blocker, or two records are equal.

use anyhow::{Context, Result, bail};
use apsp_panel::{AppState, config::Config, plugins::singbox::source_migration};
use serde_json::{Value, json};
use sqlx::postgres::PgPoolOptions;

const USAGE: &str = "usage: apsp-panel source-migration precheck | apply | rollback | snapshot [--at <unix-seconds>] | compare <before.json> <after.json>";

fn read(path: &str) -> Result<Value> {
    let bytes = std::fs::read(path).with_context(|| format!("cannot read {path}"))?;
    serde_json::from_slice(&bytes).with_context(|| format!("{path} is not a snapshot"))
}

pub async fn run(args: &[String]) -> Result<i32> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .init();
    let command = args.first().map(String::as_str).unwrap_or_default();
    if command == "compare" {
        let [_, before, after] = args else {
            bail!(USAGE)
        };
        let differences = source_migration::compare(&read(before)?, &read(after)?);
        println!(
            "{}",
            serde_json::to_string_pretty(
                &json!({"equal": differences.is_empty(), "differences": differences})
            )?
        );
        return Ok(i32::from(!differences.is_empty()));
    }
    let at = match args {
        [_] => None,
        [name, flag, value] if name == "snapshot" && flag == "--at" => {
            Some(value.parse::<i64>().context("--at takes Unix seconds")?)
        }
        _ => bail!(USAGE),
    };
    if !matches!(command, "precheck" | "apply" | "rollback" | "snapshot") {
        bail!(USAGE);
    }
    let config = Config::from_env()?;
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&config.database_url)
        .await?;
    let state = AppState::new(pool, config).await?;
    apsp_panel::plugins::install();
    let (value, succeeded) = match command {
        "precheck" => {
            let report = source_migration::precheck(&state.pool).await?;
            (json!(report), report.ready())
        }
        "apply" => {
            let report = source_migration::apply(&state.pool).await?;
            (json!(report), report.migrated && report.ready())
        }
        "rollback" => {
            let outcome = source_migration::rollback(&state.pool).await?;
            let succeeded = outcome.rolled_back;
            (json!(outcome), succeeded)
        }
        _ => {
            let at = at.unwrap_or_else(apsp_protocol::now_timestamp);
            (source_migration::snapshot(&state, at).await?, true)
        }
    };
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(i32::from(!succeeded))
}
