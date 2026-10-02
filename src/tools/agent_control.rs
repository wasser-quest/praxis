use crate::db::Database;

#[derive(Debug, Clone)]
pub enum AgentControlSignal {
    Next,
    Complete,
    Path(String),
    Push(String),
    Pop,
    Feedback(String),
    SetMode(String),
    SetVariable(String, String),
}

pub async fn run(
    db: &Database,
    user_id: &str,
    signal: AgentControlSignal,
) -> Result<String, String> {
    let mut ctx = db
        .load_context(user_id)
        .map_err(|e| format!("Failed to load context: {}", e))?;

    let signal_name = format!("{:?}", signal);
    let before = ctx.clone();

    match signal {
        AgentControlSignal::Next => {
            // Advance to next template in queue
            if let Some(current) = ctx
                .settings
                .active_templates
                .iter()
                .position(|t| t == &ctx.settings.current_template)
            {
                if current + 1 < ctx.settings.active_templates.len() {
                    ctx.settings.current_template =
                        ctx.settings.active_templates[current + 1].clone();
                }
            }
            ctx.settings.llm_turn += 1;

            let path = crate::gateway::prompt::workflow_name(&ctx).to_string();
            let sm = crate::sm::load_file(&path).map_err(|e| format!("Failed to load SM workflow: {e}"))?;
            let mut value = serde_json::to_value(&ctx).map_err(|e| e.to_string())?;
            if let Some(next) = crate::sm::advance_workflow(&sm, &value) {
                if !crate::sm::transition_to(&sm, &mut value, &next) {
                    return Err(format!("SM target state does not exist: {next}"));
                }
                ctx = serde_json::from_value(value).map_err(|e| e.to_string())?;
                ctx.settings.active_state = ctx.active_state.clone();
            }
        }
        AgentControlSignal::Complete => {
            crate::gateway::action_contracts::require(user_id, "_complete").map_err(|e| e.to_string())?;
            ctx.settings.done = true;
        }
        AgentControlSignal::Path(path) => {
            ctx.settings.path = path;
        }
        AgentControlSignal::Push(template_name) => {
            ctx.settings.active_templates.push(template_name);
        }
        AgentControlSignal::Pop => {
            ctx.settings.active_templates.pop();
        }
        AgentControlSignal::Feedback(msg) => {
            // Sending feedback must not silently enable external delivery.
            // The gateway honors the user's feedback_enabled/rate-limit policy.
            tracing::info!(user_id = %user_id, msg = %msg, "Agent feedback");
            return Ok(format!("Feedback sent: {}", msg));
        }
        AgentControlSignal::SetMode(mode) => {
            if ctx.custom_data.is_null() {
                ctx.custom_data = serde_json::json!({});
            }
            if let Some(map) = ctx.custom_data.as_object_mut() {
                map.insert("mode".to_string(), serde_json::json!(mode));
            }
        }
        AgentControlSignal::SetVariable(key, value) => {
            if ctx.custom_data.is_null() {
                ctx.custom_data = serde_json::json!({});
            }
            if let Some(map) = ctx.custom_data.as_object_mut() {
                map.insert(key, serde_json::json!(value));
            }
        }
    }

    crate::gateway::action_contracts::validate_context(&before, &ctx).map_err(|e| e.to_string())?;
    db.save_context(&ctx)
        .map_err(|e| format!("Failed to save context: {}", e))?;

    Ok(format!("Agent signal processed: {}", signal_name))
}

#[cfg(test)]
mod tool_tests {
    use super::*;
    use tempfile::TempDir;

    fn test_setup() -> (Database, TempDir) {
        let dir = TempDir::new().unwrap();
        let db = Database::new(dir.path()).unwrap();
        (db, dir)
    }

    #[test]
    fn test_signal_variants() {
        let signals = vec![
            AgentControlSignal::Next,
            AgentControlSignal::Complete,
            AgentControlSignal::Path("/tmp".to_string()),
            AgentControlSignal::Push("template".to_string()),
            AgentControlSignal::Pop,
            AgentControlSignal::Feedback("msg".to_string()),
            AgentControlSignal::SetMode("code".to_string()),
            AgentControlSignal::SetVariable("key".to_string(), "val".to_string()),
        ];
        assert_eq!(signals.len(), 8);
    }

    #[tokio::test]
    async fn test_run_complete() {
        let (db, _dir) = test_setup();
        let result = run(&db, "user1", AgentControlSignal::Complete).await;
        assert!(result.is_ok());
        assert!(result.unwrap().contains("Complete"));

        let ctx = db.load_context("user1").unwrap();
        assert!(ctx.settings.done);
    }

    #[tokio::test]
    async fn test_run_path() {
        let (db, _dir) = test_setup();
        let result = run(
            &db,
            "user1",
            AgentControlSignal::Path("/home/user".to_string()),
        )
        .await;
        assert!(result.is_ok());

        let ctx = db.load_context("user1").unwrap();
        assert_eq!(ctx.settings.path, "/home/user");
    }

    #[tokio::test]
    async fn test_run_push_pop() {
        let (db, _dir) = test_setup();

        run(&db, "user1", AgentControlSignal::Push("t1".to_string()))
            .await
            .unwrap();
        run(&db, "user1", AgentControlSignal::Push("t2".to_string()))
            .await
            .unwrap();

        let ctx = db.load_context("user1").unwrap();
        assert_eq!(ctx.settings.active_templates, vec!["t1", "t2"]);

        run(&db, "user1", AgentControlSignal::Pop).await.unwrap();
        let ctx = db.load_context("user1").unwrap();
        assert_eq!(ctx.settings.active_templates, vec!["t1"]);
    }

    #[tokio::test]
    async fn test_run_next() {
        let (db, _dir) = test_setup();

        run(&db, "user1", AgentControlSignal::Push("t1".to_string()))
            .await
            .unwrap();
        run(&db, "user1", AgentControlSignal::Push("t2".to_string()))
            .await
            .unwrap();
        run(&db, "user1", AgentControlSignal::Next).await.unwrap();

        let ctx = db.load_context("user1").unwrap();
        assert_eq!(ctx.settings.llm_turn, 1);
    }

    #[tokio::test]
    async fn test_run_set_mode() {
        let (db, _dir) = test_setup();
        run(
            &db,
            "user1",
            AgentControlSignal::SetMode("code".to_string()),
        )
        .await
        .unwrap();

        let ctx = db.load_context("user1").unwrap();
        assert_eq!(ctx.custom_data["mode"], "code");
    }

    #[tokio::test]
    async fn test_run_set_variable() {
        let (db, _dir) = test_setup();
        run(
            &db,
            "user1",
            AgentControlSignal::SetVariable("language".to_string(), "rust".to_string()),
        )
        .await
        .unwrap();

        let ctx = db.load_context("user1").unwrap();
        assert_eq!(ctx.custom_data["language"], "rust");
    }

    #[tokio::test]
    async fn test_run_feedback() {
        let (db, _dir) = test_setup();
        let result = run(
            &db,
            "user1",
            AgentControlSignal::Feedback("progress update".to_string()),
        )
        .await;
        assert!(result.is_ok());
        assert!(result.unwrap().contains("Feedback sent"));
    }
}
