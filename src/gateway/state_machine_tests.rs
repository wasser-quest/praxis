//! Real-state workflow regressions; no live services or user data.
use crate::{db::{contexts::Context, Database}, gateway::prompt, plugins::PluginRegistry};
use serde_json::json;
use std::path::Path;

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

#[tokio::test]
async fn the_exit_is_part_of_the_graph_and_not_a_missing_state() {
    let sm = crate::sm::parse("[state review]\n[transitions]\nreview -> _complete : when settings.done == true\n").unwrap();
    let ctx = Context { user_id: "exit-graph".into(), active_state: Some("review".into()), ..Default::default() };
    let graph = crate::gateway::workflow_graph::view(&sm, &ctx).await;
    let edge = graph["edges"].as_array().unwrap().iter().find(|e| e["to"] == "_complete").expect("declared exit edge");
    assert_ne!(edge["blocked_reason"].as_str().unwrap_or(""), "Destination state is undefined", "the exit is a declared destination, not a typo");
    assert!(graph["nodes"].as_array().unwrap().iter().any(|node| node["id"] == "_complete"), "the graph shows how a run is allowed to end");
    assert_eq!(graph["exit"]["title"].as_str(), Some("Complete"));
}
