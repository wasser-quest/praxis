//! Real-state workflow regressions; no live services or user data.
use crate::{db::{contexts::Context, Database}, gateway::prompt, plugins::PluginRegistry};
use serde_json::json;
use std::path::Path;

#[tokio::test]
async fn twenty_tasks_declares_real_states_without_deterministic_task_routing() {
    let sm=crate::sm::load_file("20-tasks").expect("dedicated workflow");
    assert!(sm.auto_rules.is_empty());
    assert!(sm.overrides.is_empty());
    assert_eq!(sm.steps,vec!["standard"]); // initialization only, no phase queue
    let plugins=PluginRegistry::new();
    for name in ["standard","code_architect","senior_dev","expert_programmer","debugger","code","review","teach","research"] {
        let mut ctx=Context {user_id:"synthetic-state-test".into(),active_state:Some(name.into()),..Default::default()};
        ctx.settings.sm_file=Some("20-tasks".into());
        ctx.sm_data=json!({"role":"stale-persona"});
        prompt::route_context(Path::new("."),&mut ctx,"An ordinary task with no transition request",&plugins,None).await.unwrap();
        assert_eq!(ctx.active_state.as_deref(),Some(name));
        assert_eq!(ctx.settings.active_state,ctx.active_state);
        assert_eq!(ctx.sm_data["role"],name,"role must be a projection of the REAL state");
        assert_eq!(ctx.settings.system_template.as_deref(),Some("20-tasks"));
        assert!(crate::sm::advance_workflow(&sm,&serde_json::to_value(&ctx).unwrap()).await.is_none());
    }
}

#[test]
fn twenty_tasks_invalid_model_state_never_persists_or_silently_resets() {
    let dir=tempfile::tempdir().unwrap();
    let db=Database::new(dir.path()).unwrap();
    let mut ctx=Context {user_id:"state-validation".into(),active_state:Some("review".into()),..Default::default()};
    ctx.settings.sm_file=Some("20-tasks".into());
    ctx.settings.active_state=ctx.active_state.clone();
    db.save_context(&ctx).unwrap();
    for key in ["active_state","settings.active_state"] {
        for target in ["invented","","_default"] {
            assert!(db.merge_context_from_agent(&ctx.user_id,json!({key:target})).is_err(),"accepted {key}={target}");
            assert_eq!(db.load_context(&ctx.user_id).unwrap().active_state.as_deref(),Some("review"));
        }
        let actual=db.merge_context_from_agent(&ctx.user_id,json!({key:"debugger"})).unwrap();
        assert_eq!(actual.active_state.as_deref(),Some("debugger"));
        assert_eq!(actual.settings.active_state,actual.active_state);
        db.save_context(&ctx).unwrap();
    }
    assert!(db.merge_context_from_agent(&ctx.user_id,json!({"settings":{"active_state":"invented"}})).is_err());
}

#[test]
fn twenty_tasks_manifest_contains_twenty_natural_tasks_and_hidden_rubrics() {
    let raw=std::fs::read_to_string("tests/fixtures/20-tasks.json").unwrap();
    let tasks:serde_json::Value=serde_json::from_str(&raw).unwrap();
    let tasks=tasks.as_array().unwrap();
    assert_eq!(tasks.len(),20);
    let mut ids=std::collections::HashSet::new();
    let forbidden=regex::Regex::new(r"(?i)active_state|sm_data|set_context|zustandswechsel|state.?wechsel|rollenwechsel|wechsle.*rolle").unwrap();
    for task in tasks {
        assert!(ids.insert(task["id"].as_str().unwrap()));
        assert!(!forbidden.is_match(task["prompt"].as_str().unwrap()));
        assert!(!forbidden.is_match(task["fixture"].as_str().unwrap_or("")));
        assert!(!task["acceptable_states"].as_array().unwrap().is_empty());
        assert!(!task["checks"].as_array().unwrap().is_empty());
    }
}
