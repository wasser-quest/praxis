use super::*;
use crate::{db::Database, gateway::task_control};
use serde_json::json;

#[test]
fn discovery_is_bounded_task_local_and_respects_disabled_tools() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::new(dir.path()).unwrap();
    crate::db::tools::init_default_tools(&db).unwrap();
    let plugins = crate::plugins::load_all_plugins(std::path::Path::new("plugins"));
    let user = format!("discovery-{}", uuid::Uuid::new_v4());
    let initial = definitions(&db, &plugins, &user).unwrap();
    assert!(initial.len() <= 14); // existing core plus graph back navigation
    assert!(initial.iter().any(|t| t.function.name == "agent_back"));
    assert!(initial.iter().any(|t| t.function.name == "read_tool_result"));
    assert!(initial.iter().any(|t| t.function.name == "search_tools"));
    assert!(!initial.iter().any(|t| t.function.name == "brave_web_search"));
    assert!(search(&db, &plugins, &user, &json!({"query":"brave"})).is_err(), "no activation outside an owned task");
    {
        let _task = task_control::begin(&user).unwrap();
        let result: serde_json::Value = serde_json::from_str(&search(&db, &plugins, &user, &json!({"query":"brave", "limit":1})).unwrap()).unwrap();
        assert_eq!(result["tools"][0]["name"], "brave_web_search");
        assert!(result["tools"][0].get("parameters").is_none(), "Schemas belong in the next request's tools, not twice in history");
        assert!(definitions(&db, &plugins, &user).unwrap().iter().any(|t| t.function.name == "brave_web_search"));
        assert!(!definitions(&db, &plugins, "other-user").unwrap().iter().any(|t| t.function.name == "brave_web_search"));
        search(&db, &plugins, &user, &json!({"query":"memory_set"})).unwrap();
        crate::db::tools::disable(&db, "memory_set").unwrap();
        assert!(!definitions(&db, &plugins, &user).unwrap().iter().any(|t| t.function.name == "memory_set"));
        let result = search(&db, &plugins, &user, &json!({"query":"memory_set"})).unwrap();
        assert!(!result.contains("\"name\":\"memory_set\""));
        for args in [json!({"query":""}), json!({"query":"brave", "limit":0}), json!({"query":"brave", "limit":100})] {
            assert!(search(&db, &plugins, &user, &args).is_err());
        }
    }
    let _next = task_control::begin(&user).unwrap();
    assert!(!definitions(&db, &plugins, &user).unwrap().iter().any(|t| t.function.name == "brave_web_search"));
}

#[test]
fn discovery_selection_has_a_hard_bound_and_can_be_replaced() {
    let user = format!("selection-{}", uuid::Uuid::new_v4());
    let _task = task_control::begin(&user).unwrap();
    task_control::select_tools(&user, (0..24).map(|i| format!("t{i}")).collect(), false).unwrap();
    assert!(task_control::select_tools(&user, vec!["overflow".into()], false).is_err());
    task_control::select_tools(&user, vec!["replacement".into()], true).unwrap();
    assert_eq!(task_control::selected_tools(&user).len(), 1);
}
