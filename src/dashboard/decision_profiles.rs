//! Administrative file editor and non-mutating playground probe. These routes
//! are registered only under the dashboard's authenticated API router.
use axum::{extract::Path, http::StatusCode, Json};
use serde::Deserialize;
use serde_json::{json,Value};
use crate::gateway::{decision_client,decision_profiles::{self as profiles,DecisionProfile}};

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
    let start=std::time::Instant::now();
    let results=decision_client::decide(&probe.profile,probe.contexts,&tokio_util::sync::CancellationToken::new()).await
        .map_err(|e|bad(format!("Decision error: {e}")))?;
    Ok(Json(json!({"results":results.iter().map(|d|json!({
        "label":d.label,"state":probe.profile.state_map.get(&d.label),"probability":d.probability,
        "confidence":d.confidence,"usage":d.usage,
        "meets_threshold":d.probability>=probe.profile.minimum_probability,
    })).collect::<Vec<_>>(),"elapsed_ms":start.elapsed().as_millis(),
        "notice":"Classification only: no context, state, history or permissions changed. Probabilities are not calibrated certainty."})))
}
