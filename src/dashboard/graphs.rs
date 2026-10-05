//! Dashboard HTTP adapter over `crate::services::graphs`.
use super::routes::DashboardState;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

#[derive(Default, Deserialize)]
pub struct GraphQuery {
    pub workflow: Option<String>,
}

pub async fn graph(
    State(state): State<Arc<DashboardState>>,
    Path(user): Path<String>,
    Query(query): Query<GraphQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    crate::services::graphs::graph(&state.db, &user, query.workflow.as_deref())
        .await
        .map(Json)
        .map_err(|error| (StatusCode::BAD_REQUEST, Json(json!({"error":error.to_string()}))))
}

pub async fn execution(
    State(state): State<Arc<DashboardState>>,
    Path(user): Path<String>,
) -> Result<Json<Value>, StatusCode> {
    crate::services::graphs::execution(&state.db, &user)
        .map(Json)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

pub async fn usage(
    State(state): State<Arc<DashboardState>>,
    Path(user): Path<String>,
) -> Result<Json<Value>, StatusCode> {
    crate::services::graphs::usage(&state.db, &user)
        .map(Json)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}
