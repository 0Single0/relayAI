use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

use crate::gateway::{channels::workbuddy::WorkbuddyChannel, state::SharedGateway};

pub(super) async fn get(State(state): State<SharedGateway>, Path(key): Path<String>) -> Response {
    let (channel, uid) = super::split_key(&key);
    let Some(account) = state.account(&channel, &uid) else {
        return (StatusCode::NOT_FOUND, Json(json!({"error":"账号不存在"}))).into_response();
    };
    if account.platform != "workbuddy" {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":"该实时扣费查询仅适用于 WorkBuddy 账号"})),
        )
            .into_response();
    }
    let client = match state.client_for(&account) {
        Ok(client) => client,
        Err(error) => return super::internal_error(error),
    };
    let adapter = WorkbuddyChannel::from_options("workbuddy", &json!({}));
    match adapter.live_models(&client, &account.snapshot()).await {
        Ok(mut catalog) => {
            catalog.models.retain(|model| {
                account.model_allow.is_empty() || account.model_allow.contains(&model.id)
            });
            ([("cache-control", "no-store")], Json(catalog)).into_response()
        }
        Err(error) => super::internal_error(error),
    }
}
