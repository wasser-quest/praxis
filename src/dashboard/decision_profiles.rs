//! Administrative file editor and non-mutating playground probe. These routes
//! are registered only under the dashboard's authenticated API router.
use axum::{extract::Path, http::StatusCode, Json};
use serde::Deserialize;
use serde_json::{json,Value};
use crate::gateway::decision_profiles::{self as profiles,DecisionProfile};

type Error=(StatusCode,Json<Value>);
fn bad(error:impl std::fmt::Display)->Error {(StatusCode::BAD_REQUEST,Json(json!({"error":error.to_string()})))}

pub async fn list()->Result<Json<Value>,Error> {
    Ok(Json(json!({"profiles":profiles::list(std::path::Path::new("decisions")).map_err(bad)?})))
}
pub async fn get(Path(name):Path<String>)->Result<Json<Value>,Error> {
    let path=profiles::resolve(std::path::Path::new("decisions"),&name).map_err(|_|(StatusCode::NOT_FOUND,Json(json!({"error":"Decision profile not found"}))))?;
    Ok(Json(json!({"name":name,"content":std::fs::read_to_string(path).map_err(bad)?})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileUpdate {content:String}
pub async fn save(Path(name):Path<String>,Json(update):Json<FileUpdate>)->Result<Json<Value>,Error> {
    profiles::save(std::path::Path::new("decisions"),&name,&update.content).map_err(bad)?;
    Ok(Json(json!({"success":true})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Probe {profile:DecisionProfile,contexts:Vec<String>}
pub async fn probe(Json(probe):Json<Probe>)->Result<Json<Value>,Error> {
    match crate::services::admin::decision_probe(&probe.profile,probe.contexts).await {
        Ok(value)=>Ok(Json(value)),
        Err(crate::services::admin::Failure::BadRequest(message))=>Err(bad(message)),
        Err(other)=>Err((StatusCode::INTERNAL_SERVER_ERROR,Json(json!({"error":format!("{other:?}")})))),
    }
}
