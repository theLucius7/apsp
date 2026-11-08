//! Costs whose code still lives in this crate: exchange rates.

use axum::{
    Router,
    routing::{get, post},
};

use crate::{AppState, exchange};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/exchange-rates", get(exchange::get))
        .route("/api/exchange-rates/refresh", post(exchange::refresh))
}
