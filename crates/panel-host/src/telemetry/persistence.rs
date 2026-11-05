use super::{aggregate::HistoryPoint, *};
use futures_util::TryStreamExt;
use sqlx::{PgPool, Postgres, QueryBuilder, Transaction};
use std::collections::BTreeMap;

pub(super) const DAY_MS: i64 = 86_400_000;
pub(super) const RAW_MS: i64 = 2 * 3_600_000;
pub(super) const REPLAY_MS: i64 = 7 * DAY_MS;

pub(super) async fn initialized(tx: &mut Transaction<'_, Postgres>, server: i64) -> ApiResult<()> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM telemetry_history_initialized WHERE server_id=$1)",
    )
    .bind(server)
    .fetch_one(&mut **tx)
    .await?;
    if exists {
        return Ok(());
    }
    let mut points = BTreeMap::<i64, HistoryPoint>::new();
    let mut minutes = HashSet::new();
    {
        let mut rows=sqlx::query("SELECT id,sampled_at,metrics FROM telemetry_samples WHERE server_id=$1 ORDER BY sampled_at,id").bind(server).fetch(&mut **tx);
        while let Some(row) = rows.try_next().await? {
            let sample = row_sample(&row)?;
            let mut point = HistoryPoint::sample(&sample, 60_000);
            point.partial = true;
            minutes.insert(point.bucket_at);
            if let Some(old) = points.get_mut(&point.bucket_at) {
                old.merge(point);
            } else {
                points.insert(point.bucket_at, point);
            }
        }
    }
    // The legacy table holds one real observation per minute, not an average.
    {
        let mut rows = sqlx::query(
            "SELECT sampled_at,metrics FROM metrics_minutely WHERE server_id=$1 ORDER BY bucket",
        )
        .bind(server)
        .fetch(&mut **tx);
        while let Some(row) = rows.try_next().await? {
            let at: i64 = row.get("sampled_at");
            if at <= 0 || minutes.contains(&(at.div_euclid(60_000) * 60_000)) {
                continue;
            }
            let sample = TelemetrySample {
                id: uuid::Uuid::nil(),
                sampled_at: at,
                metrics: serde_json::from_value(row.get("metrics")).map_err(anyhow::Error::from)?,
            };
            let mut point = HistoryPoint::sample(&sample, 60_000);
            point.partial = true;
            points.insert(point.bucket_at, point);
        }
    }
    let points: Vec<_> = points.into_values().collect();
    for chunk in points.chunks(128) {
        let mut query = QueryBuilder::<Postgres>::new(
            "INSERT INTO telemetry_history(server_id,resolution_secs,bucket_at,summary) ",
        );
        let encoded = chunk
            .iter()
            .map(|point| serde_json::to_value(point).map(|value| (point.bucket_at, value)))
            .collect::<Result<Vec<_>, _>>()
            .map_err(anyhow::Error::from)?;
        query.push_values(&encoded, |mut row, (at, value)| {
            row.push_bind(server)
                .push_bind(60)
                .push_bind(at)
                .push_bind(value);
        });
        query
            .push(" ON CONFLICT DO NOTHING")
            .build()
            .execute(&mut **tx)
            .await?;
    }
    sqlx::query(
        "INSERT INTO telemetry_history_initialized(server_id) VALUES($1) ON CONFLICT DO NOTHING",
    )
    .bind(server)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub(super) fn row_sample(row: &sqlx::postgres::PgRow) -> ApiResult<TelemetrySample> {
    Ok(TelemetrySample {
        id: row.get("id"),
        sampled_at: row.get("sampled_at"),
        metrics: serde_json::from_value(row.get("metrics")).map_err(anyhow::Error::from)?,
    })
}

pub(super) async fn merge(
    tx: &mut Transaction<'_, Postgres>,
    server: i64,
    resolution: i32,
    mut point: HistoryPoint,
) -> ApiResult<()> {
    point.bucket_at =
        point.bucket_at.div_euclid(i64::from(resolution) * 1000) * i64::from(resolution) * 1000;
    let previous: Option<serde_json::Value> = sqlx::query_scalar("SELECT summary FROM telemetry_history WHERE server_id=$1 AND resolution_secs=$2 AND bucket_at=$3").bind(server).bind(resolution).bind(point.bucket_at).fetch_optional(&mut **tx).await?;
    if let Some(value) = previous {
        let mut old: HistoryPoint = serde_json::from_value(value).map_err(anyhow::Error::from)?;
        old.merge(point);
        point = old;
    }
    sqlx::query("INSERT INTO telemetry_history(server_id,resolution_secs,bucket_at,summary) VALUES($1,$2,$3,$4) ON CONFLICT(server_id,resolution_secs,bucket_at) DO UPDATE SET summary=EXCLUDED.summary").bind(server).bind(resolution).bind(point.bucket_at).bind(serde_json::to_value(&point).map_err(anyhow::Error::from)?).execute(&mut **tx).await?;
    Ok(())
}

pub(super) async fn save(
    pool: &PgPool,
    server: i64,
    batch: TelemetryBatch,
    now: i64,
) -> ApiResult<TelemetryAck> {
    let retention = storage::read_policy(pool).await?.history_retention_days;
    let mut tx = pool.begin().await?;
    let row = sqlx::query("SELECT asset_settings,latest_metrics,metrics_sampled_at FROM servers WHERE id=$1 AND deleted_at IS NULL FOR UPDATE")
        .bind(server)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(ApiError::NotFound)?;
    initialized(&mut tx, server).await?;
    let asset: crate::server_assets::AssetSettings =
        serde_json::from_value(row.get("asset_settings")).map_err(anyhow::Error::from)?;
    // The newest metrics so far: each sample is compared with the one before
    // it, exactly as when every sample re-read and replaced the stored value.
    let mut latest: serde_json::Value = row.get("latest_metrics");
    let mut latest_at: i64 = row.get("metrics_sampled_at");
    let mut latest_changed = false;
    let mut entries = Vec::with_capacity(batch.samples.len());
    // Digest and time of each identity's first occurrence in this batch.
    let mut first = BTreeMap::<uuid::Uuid, (String, i64)>::new();
    for sample in batch.samples {
        let value = serde_json::to_value(&sample.metrics).map_err(anyhow::Error::from)?;
        let digest = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&sample).map_err(anyhow::Error::from)?)
        );
        // A repeated identity within the batch behaves like a replay of the first.
        let repeat = match first.get(&sample.id) {
            Some((earlier, _)) if *earlier != digest => {
                return Err(ApiError::Conflict("同一遥测标识的内容发生变化".into()));
            }
            Some(_) => true,
            None => {
                first.insert(sample.id, (digest.clone(), sample.sampled_at));
                false
            }
        };
        entries.push((sample, value, digest, repeat));
    }
    // One statement records every new receipt; the outer query still sees the
    // digests stored before it, which decide replays and conflicts.
    let ids: Vec<uuid::Uuid> = first.keys().copied().collect();
    let times: Vec<i64> = first.values().map(|(_, at)| *at).collect();
    let digests: Vec<&str> = first.values().map(|(digest, _)| digest.as_str()).collect();
    let receipts = sqlx::query("WITH input AS (SELECT * FROM UNNEST($2::uuid[],$3::bigint[],$4::text[]) AS t(id,sampled_at,digest)), inserted AS (INSERT INTO telemetry_receipts(server_id,id,sampled_at,digest) SELECT $1,id,sampled_at,digest FROM input ON CONFLICT DO NOTHING RETURNING id) SELECT i.id,i.id IN (SELECT id FROM inserted) AS added,r.digest AS stored FROM input i LEFT JOIN telemetry_receipts r ON r.server_id=$1 AND r.id=i.id")
        .bind(server).bind(&ids).bind(&times).bind(&digests).fetch_all(&mut *tx).await?;
    let mut added = std::collections::HashSet::new();
    for receipt in receipts {
        let id: uuid::Uuid = receipt.get("id");
        if receipt.get::<bool, _>("added") {
            added.insert(id);
        } else if receipt.get::<Option<String>, _>("stored").as_deref()
            != first.get(&id).map(|(digest, _)| digest.as_str())
        {
            return Err(ApiError::Conflict("同一遥测标识的内容发生变化".into()));
        }
    }
    let raw_from = (now - RAW_MS).div_euclid(60_000) * 60_000;
    let history_from = now - i64::from(retention) * DAY_MS;
    let mut ack = Vec::new();
    let mut samples = Vec::new();
    let mut raw = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    // Newest sample per minute bucket; the first of equal timestamps wins.
    let mut minutes = BTreeMap::<i64, (i64, serde_json::Value)>::new();
    let mut buckets = BTreeMap::<i64, HistoryPoint>::new();
    for (sample, value, digest, repeat) in entries {
        ack.push(sample.id);
        if repeat || !added.contains(&sample.id) {
            continue;
        }
        if sample.sampled_at >= raw_from {
            raw.0.push(sample.id);
            raw.1.push(sample.sampled_at);
            raw.2.push(digest);
            raw.3.push(value.clone());
        }
        if sample.sampled_at > latest_at {
            let events = crate::fleet::monitoring::changes(&latest, &sample);
            crate::fleet::monitoring::record(&mut tx, server, sample.sampled_at, events).await?;
            latest = value.clone();
            latest_at = sample.sampled_at;
            latest_changed = true;
        }
        let minute = sample.sampled_at.div_euclid(60_000) * 60;
        if minutes
            .get(&minute)
            .is_none_or(|(at, _)| *at < sample.sampled_at)
        {
            minutes.insert(minute, (sample.sampled_at, value));
        }
        if sample.sampled_at >= history_from {
            let point = HistoryPoint::sample_with_scope(&sample, 60_000, &asset);
            if let Some(old) = buckets.get_mut(&point.bucket_at) {
                old.merge(point);
            } else {
                buckets.insert(point.bucket_at, point);
            }
        }
        samples.push(sample);
    }
    if !raw.0.is_empty() {
        sqlx::query("INSERT INTO telemetry_samples(server_id,id,sampled_at,digest,metrics) SELECT $1,id,sampled_at,digest,metrics FROM UNNEST($2::uuid[],$3::bigint[],$4::text[],$5::jsonb[]) AS t(id,sampled_at,digest,metrics) ON CONFLICT DO NOTHING")
            .bind(server).bind(&raw.0).bind(&raw.1).bind(&raw.2).bind(&raw.3).execute(&mut *tx).await?;
    }
    if latest_changed {
        sqlx::query("UPDATE servers SET latest_metrics=$2,metrics_sampled_at=$3 WHERE id=$1 AND metrics_sampled_at<$3").bind(server).bind(&latest).bind(latest_at).execute(&mut *tx).await?;
    }
    if !minutes.is_empty() {
        // Preserve the old last-observation projection for old consumers.
        let (minute_keys, rest): (Vec<i64>, Vec<(i64, serde_json::Value)>) =
            minutes.into_iter().unzip();
        let (minute_times, minute_values): (Vec<i64>, Vec<serde_json::Value>) =
            rest.into_iter().unzip();
        sqlx::query("INSERT INTO metrics_minutely(server_id,bucket,metrics,sampled_at) SELECT $1,bucket,metrics,sampled_at FROM UNNEST($2::bigint[],$3::jsonb[],$4::bigint[]) AS t(bucket,metrics,sampled_at) ON CONFLICT(server_id,bucket) DO UPDATE SET metrics=EXCLUDED.metrics,sampled_at=EXCLUDED.sampled_at WHERE metrics_minutely.sampled_at<EXCLUDED.sampled_at")
            .bind(server).bind(&minute_keys).bind(&minute_values).bind(&minute_times).execute(&mut *tx).await?;
    }
    for point in buckets.into_values() {
        merge(&mut tx, server, 60, point).await?;
    }
    // Exact device counters, replay identity and historical aggregates commit
    // atomically. Aggregated rates are never used to debit network usage.
    crate::server_traffic::ingest(&mut tx, server, &mut samples).await?;
    tx.commit().await?;
    Ok(TelemetryAck { ids: ack })
}
