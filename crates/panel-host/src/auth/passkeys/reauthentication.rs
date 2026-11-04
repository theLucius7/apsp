use super::{account_of, no_keys};
use crate::{
    AppState,
    auth::{proof, require_admin, security},
    error::ApiResult,
    passkeys::{
        self as keys, ADMIN_REAUTH_BINDING, Ceremony, FinishAuthentication, PendingAuthentication,
    },
};
use axum::{
    Json,
    extract::{ConnectInfo, State},
    http::HeaderMap,
    response::Response,
};
use std::net::SocketAddr;

pub(super) async fn start(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let admin = require_admin(&state, &headers).await?;
    let _permit = keys::permit(&state, &headers, peer).await?;
    let session = security::session_hash(&headers)?;
    let mut tx = state.pool.begin().await?;
    security::locked_admin(&mut tx, &session).await?;
    let account = account_of(&mut *tx, admin).await?.ok_or_else(no_keys)?;
    let response =
        keys::start_reauthentication(&state, &mut tx, account, ADMIN_REAUTH_BINDING, session)
            .await?;
    tx.commit().await?;
    Ok(response)
}

pub(super) async fn finish(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(input): Json<FinishAuthentication>,
) -> ApiResult<Response> {
    let admin = require_admin(&state, &headers).await?;
    let _permit = keys::permit(&state, &headers, peer).await?;
    let session = security::session_hash(&headers)?;
    let account = account_of(&state.pool, admin)
        .await?
        .ok_or_else(keys::invalid)?;
    let pending: Ceremony<PendingAuthentication> = keys::consume(
        &state,
        &headers,
        input.challenge_id,
        account,
        "reauthenticate",
        ADMIN_REAUTH_BINDING,
    )
    .await?;
    if pending.authorization != session {
        return Err(keys::invalid());
    }
    let result = state
        .passkeys
        .authentication(input.credential, &pending.state)
        .await?;
    let mut tx = state.pool.begin().await?;
    // Recheck session revocation and credential generation after signature verification.
    let row = security::locked_admin(&mut tx, &session).await?;
    if sqlx::Row::try_get::<i64, _>(&row, "id")? != admin {
        return Err(keys::invalid());
    }
    keys::authenticate(&mut tx, &pending, result).await?;
    let proof = proof::record_recent(&mut tx, &session).await?;
    tx.commit().await?;
    let mut response = keys::reply(proof);
    keys::set_cookie(&state, &mut response, ADMIN_REAUTH_BINDING, "", 0)?;
    Ok(response)
}
