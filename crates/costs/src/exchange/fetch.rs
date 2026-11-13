use apsp_panel_host::http::{self, BodyError, Outbound};
use reqwest::Client;
use serde_json::Value;
use std::{collections::BTreeMap, time::Duration};

const PRIMARY: &str = "https://api.frankfurter.dev/v2/rates?base=CNY";
const FALLBACK: &str = "https://api.frankfurter.dev/v1/latest?base=CNY";
const MAX_BODY: usize = 256 * 1024;
const DAY: i64 = 86_400;

pub(super) struct FetchedRates {
    pub rates: BTreeMap<String, f64>,
    pub dates: BTreeMap<String, String>,
    pub source: &'static str,
}

pub(super) async fn latest() -> Result<FetchedRates, &'static str> {
    let client = Outbound {
        connect_timeout: Some(Duration::from_secs(3)),
        timeout: Some(Duration::from_secs(8)),
        https_only: true,
        product: Some("Apsp-exchange-rates/1"),
        ..Outbound::default()
    }
    .build()
    .map_err(|_| "download_failed")?;
    let now = apsp_protocol::now_timestamp();
    if let Ok(body) = download(&client, PRIMARY).await
        && let Ok(value) = parse(&body, now, true)
    {
        return Ok(value);
    }
    let body = download(&client, FALLBACK).await?;
    parse(&body, now, false)
}

async fn download(client: &Client, url: &str) -> Result<Value, &'static str> {
    let response = client
        .get(url)
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .map_err(|_| "download_failed")?;
    if !response.status().is_success() {
        return Err("source_http_failed");
    }
    http::json(response, MAX_BODY)
        .await
        .map_err(|error| match error {
            BodyError::TooLarge => "response_too_large",
            BodyError::Read(_) => "download_failed",
            BodyError::Invalid => "invalid_response",
        })
}

pub(super) fn date_day(value: &str) -> Option<i64> {
    let bytes = value.as_bytes();
    if bytes.len() != 10
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes
            .iter()
            .enumerate()
            .any(|(i, b)| i != 4 && i != 7 && !b.is_ascii_digit())
    {
        return None;
    }
    let year: i64 = value[..4].parse().ok()?;
    let month: i64 = value[5..7].parse().ok()?;
    let day: i64 = value[8..].parse().ok()?;
    if !(1970..=9999).contains(&year) {
        return None;
    }
    apsp_panel_host::time::day_number(year, month, day)
}

fn insert(
    result: &mut FetchedRates,
    currency: &str,
    value: &Value,
    date: &str,
    now: i64,
) -> Result<(), &'static str> {
    let day = date_day(date).ok_or("invalid_response")?;
    let rate = value.as_f64().ok_or("invalid_response")?;
    if !apsp_panel_host::money::currency(currency)
        || !rate.is_finite()
        || !(1e-12..=1e12).contains(&rate)
        || day > now.div_euclid(DAY)
        || now.div_euclid(DAY).saturating_sub(day) > 31
        || (currency == "CNY" && rate != 1.0)
        || result.dates.contains_key(currency)
    {
        return Err("invalid_response");
    }
    result.rates.insert(currency.into(), rate);
    result.dates.insert(currency.into(), date.into());
    Ok(())
}

pub(super) fn parse(value: &Value, now: i64, v2: bool) -> Result<FetchedRates, &'static str> {
    let mut result = FetchedRates {
        rates: BTreeMap::from([("CNY".into(), 1.0)]),
        dates: BTreeMap::new(),
        source: if v2 { "frankfurter" } else { "frankfurter-ecb" },
    };
    if v2 {
        let values = value.as_array().ok_or("invalid_response")?;
        if values.len() > 256 {
            return Err("invalid_response");
        }
        for value in values {
            if value["base"] != "CNY" {
                return Err("invalid_response");
            }
            insert(
                &mut result,
                value["quote"].as_str().ok_or("invalid_response")?,
                &value["rate"],
                value["date"].as_str().ok_or("invalid_response")?,
                now,
            )?;
        }
    } else {
        if value["base"] != "CNY" || value["amount"].as_f64() != Some(1.0) {
            return Err("invalid_response");
        }
        let date = value["date"].as_str().ok_or("invalid_response")?;
        let rates = value["rates"].as_object().ok_or("invalid_response")?;
        if rates.len() > 256 {
            return Err("invalid_response");
        }
        for (currency, rate) in rates {
            insert(&mut result, currency, rate, date, now)?;
        }
    }
    if !["USD", "EUR"]
        .iter()
        .all(|code| result.rates.contains_key(*code))
    {
        return Err("invalid_response");
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Router,
        body::Body,
        http::{Response, StatusCode},
        routing::get,
    };
    use std::{
        convert::Infallible,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
    };

    #[tokio::test]
    async fn downloader_bounds_streamed_bodies_and_never_follows_a_redirect() -> anyhow::Result<()>
    {
        let hits = Arc::new(AtomicUsize::new(0));
        let counter = hits.clone();
        let app = Router::new()
            .route(
                "/ok",
                get(move || {
                    let counter = counter.clone();
                    async move {
                        counter.fetch_add(1, Ordering::SeqCst);
                        axum::Json(serde_json::json!({"valid":true}))
                    }
                }),
            )
            .route(
                "/redirect",
                get(|| async {
                    Response::builder()
                        .status(StatusCode::TEMPORARY_REDIRECT)
                        .header("Location", "/ok")
                        .body(Body::empty())
                        .unwrap()
                }),
            )
            .route(
                "/large",
                get(|| async {
                    Body::from_stream(futures_util::stream::iter([
                        Ok::<_, Infallible>(vec![b' '; MAX_BODY]),
                        Ok(vec![b' '; 1024]),
                    ]))
                }),
            )
            .route(
                "/failed",
                get(|| async { (StatusCode::BAD_GATEWAY, "untrusted provider diagnostic") }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = Outbound {
            timeout: Some(Duration::from_secs(2)),
            ..Outbound::default()
        }
        .build()?;
        assert_eq!(
            download(&client, &format!("http://{address}/redirect")).await,
            Err("source_http_failed")
        );
        assert_eq!(hits.load(Ordering::SeqCst), 0);
        assert_eq!(
            download(&client, &format!("http://{address}/large")).await,
            Err("response_too_large")
        );
        assert_eq!(
            download(&client, &format!("http://{address}/failed")).await,
            Err("source_http_failed")
        );
        assert_eq!(
            download(&client, &format!("http://{address}/ok"))
                .await
                .map_err(anyhow::Error::msg)?,
            serde_json::json!({"valid":true})
        );
        task.abort();
        Ok(())
    }
}
