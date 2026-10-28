use super::require_initial_admin;
use crate::{
    AppState,
    auth::{proof, security},
    error::ApiResult,
    passkeys::{
        self as keys, ADMIN, ADMIN_REAUTH_BINDING, Ceremony, FinishAuthentication,
        PendingAuthentication,
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
    require_initial_admin(&state, &headers).await?;
    let _permit = keys::permit(&state, &headers, peer).await?;
    let session = security::session_hash(&headers)?;
    let mut tx = state.pool.begin().await?;
    security::locked_admin(&mut tx, &session).await?;
    let response =
        keys::start_reauthentication(&state, &mut tx, ADMIN, ADMIN_REAUTH_BINDING, session).await?;
    tx.commit().await?;
    Ok(response)
}

pub(super) async fn finish(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(input): Json<FinishAuthentication>,
) -> ApiResult<Response> {
    require_initial_admin(&state, &headers).await?;
    let _permit = keys::permit(&state, &headers, peer).await?;
    let session = security::session_hash(&headers)?;
    let pending: Ceremony<PendingAuthentication> = keys::consume(
        &state,
        &headers,
        input.challenge_id,
        ADMIN,
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
    security::locked_admin(&mut tx, &session).await?;
    keys::authenticate(&mut tx, &pending, result).await?;
    let proof = proof::record_recent(&mut tx, &session).await?;
    tx.commit().await?;
    let mut response = keys::reply(proof);
    keys::set_cookie(&state, &mut response, ADMIN_REAUTH_BINDING, "", 0)?;
    Ok(response)
}
