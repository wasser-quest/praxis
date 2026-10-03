use super::routes::DashboardState;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    path::{Path as FsPath, PathBuf},
    sync::Arc,
};

#[derive(Default, Deserialize)]
pub struct GraphQuery {
    pub workflow: Option<String>,
}

fn root() -> PathBuf {
    crate::gateway::state_ref()
        .map(|state| PathBuf::from(&state.config.root_dir))
        .or_else(|| std::env::var_os("ROOT_DIR").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn graph_in(
    db: &crate::db::Database,
    root: &FsPath,
    user: &str,
    workflow: Option<&str>,
) -> anyhow::Result<Value> {
    let mut ctx = db.load_context(user)?;
    let active_workflow = crate::gateway::prompt::workflow_name(&ctx).to_string();
    let name = workflow.unwrap_or(&active_workflow);
    let sm = crate::sm::load_file_in(&root.join("contexts"), name)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let preview = workflow
        .is_some_and(|w| w.trim_end_matches(".sm") != active_workflow.trim_end_matches(".sm"));
    if preview {
        ctx.settings.sm_file = Some(name.into());
        ctx.sm_file = Some(name.into());
        ctx.active_state = sm.entry_state().map(String::from);
        ctx.user_id.clear(); // Preview cannot borrow receipts from another workflow.
    }
    let mut graph = crate::gateway::workflow_graph::view(&sm, &ctx);
    graph["preview"] = json!(preview);
    if !preview && crate::gateway::task_control::cancellation(user).is_none() {
        if let Some(event) = db
            .execution_events(user, 100)?
            .into_iter()
            .rev()
            .find(|event| {
                event["kind"] == "state_transition"
                    && event["payload"]["workflow"].as_str() == Some(active_workflow.as_str())
                    && event["payload"]["to_state"] == graph["active_state"]
            })
        {
            graph["history"] = event["payload"]["graph"]["history"].clone();
            graph["history_source"] = json!("last_completed_task");
        }
    }
    Ok(graph)
}

pub async fn graph(
    State(state): State<Arc<DashboardState>>,
    Path(user): Path<String>,
    Query(query): Query<GraphQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    graph_in(&state.db, &root(), &user, query.workflow.as_deref())
        .map(Json)
        .map_err(|error| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"error":error.to_string()})),
            )
        })
}

pub async fn execution(
    State(state): State<Arc<DashboardState>>,
    Path(user): Path<String>,
) -> Result<Json<Value>, StatusCode> {
    let ctx = state
        .db
        .load_context(&user)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let events = state
        .db
        .execution_events(&user, 200)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let limits = crate::gateway::telemetry::limits(&state.db, &ctx)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(
        json!({"events":events,"limits":limits,"history_available":!events.is_empty()}),
    ))
}

pub async fn usage(
    State(state): State<Arc<DashboardState>>,
    Path(user): Path<String>,
) -> Result<Json<Value>, StatusCode> {
    let ctx = state
        .db
        .load_context(&user)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    crate::gateway::telemetry::limits(&state.db, &ctx)
        .map(Json)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn state_graph_dashboard_uses_parsed_workflow_and_cannot_write_context() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("contexts")).unwrap();
        std::fs::write(dir.path().join("contexts/branch.sm"), "@routing graph\n@start a\n[state a]\n[state b]\n[transitions]\na -> b\n[decision_ir b]\nK = agent_back").unwrap();
        let db = crate::db::Database::new(&dir.path().join("data")).unwrap();
        let ctx = db.load_context("graph-dashboard").unwrap();
        db.save_context(&ctx).unwrap();
        let view = graph_in(&db, dir.path(), "graph-dashboard", Some("branch")).unwrap();
        assert_eq!(view["nodes"].as_array().unwrap().len(), 2);
        assert_eq!(view["edges"][0]["index"], 0);
        assert_eq!(view["nodes"][1]["decision_ir"]["K"], "agent_back");
        assert_eq!(
            serde_json::to_value(db.load_context("graph-dashboard").unwrap()).unwrap(),
            serde_json::to_value(ctx).unwrap()
        );
        assert!(graph_in(&db, dir.path(), "graph-dashboard", Some("../outside")).is_err());
    }
}
