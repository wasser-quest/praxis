use super::{provider::*, LLMRouter};
use std::sync::atomic::{AtomicUsize, Ordering};
use crate::gateway::llm::error::ProviderError;

struct ToolLoopProvider {
    turn: AtomicUsize,
    calls: Vec<ToolCall>,
}

#[async_trait::async_trait]
impl LLMProvider for ToolLoopProvider {
    fn name(&self) -> &str {
        "offline-skill-test"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn chat(&self, _: ChatRequest) -> Result<ChatResponse, ProviderError> {
        let first = self.turn.fetch_add(1, Ordering::SeqCst) == 0;
        Ok(ChatResponse {
    reasoning_content: None,
            content: Some(
                if first {
                    "Loading instructions"
                } else {
                    "Mock reply"
                }
                .into(),
            ),
            tool_calls: first.then(|| self.calls.clone()),
            finish_reason: None,
            usage: None,
        })
    }
}

async fn run_calls(calls: Vec<ToolCall>) -> super::ChatWithToolsResult {
    let dir = tempfile::tempdir().unwrap();
    let db = crate::db::Database::new(dir.path()).unwrap();
    crate::db::tools::init_default_tools(&db).unwrap();
    let tools = crate::db::tools::to_tool_definitions(&db).unwrap();
    let router = LLMRouter::with_providers(
        vec![Box::new(ToolLoopProvider {
            turn: AtomicUsize::new(0),
            calls,
        })],
        "offline-skill-test".into(), vec![], super::resilience::ResilienceConfig::default(),
    );
    router
        .chat_with_tools(&db, "test", vec![], tools, Some(2), None, None, None)
        .await
        .unwrap()
}

#[tokio::test]
async fn skill_tool_loop_dispatches_shared_handlers() {
    let calls = vec![
        ToolCall {
            id: "1".into(),
            function: FunctionCall {
                name: "use_skill".into(),
                arguments: r#"{"name":"debug","parameters":{}}"#.into(),
            },
        },
        ToolCall {
            id: "2".into(),
            function: FunctionCall {
                name: "update_template".into(),
                arguments: r#"{"name":"../escape","content":"<poml><p>Test</p></poml>"}"#.into(),
            },
        },
    ];
    let result = run_calls(calls).await;
    assert_eq!(result.tool_calls.len(), 2);
    assert!(result.tool_calls[0]
        .result
        .contains("non-empty string parameter"));
    assert!(result.tool_calls[1].result.contains("Template name must"));
}

#[tokio::test]
#[ignore = "Requires Node and POML_CLI pointing to Microsoft's JavaScript CLI"]
async fn skill_tool_loop_real_poml() {
    assert!(std::env::var("POML_CLI").is_ok());
    let result = run_calls(vec![ToolCall {
        id: "1".into(),
        function: FunctionCall {
            name: "use_skill".into(),
            arguments: r#"{"name":"debug","parameters":{"error":"Sentinel error"}}"#.into(),
        },
    }])
    .await;
    assert!(result.tool_calls[0].result.contains("Sentinel error"));
    assert!(result.tool_calls[0].result.contains("\n\n# Role\n"));
    assert_eq!(result.response, "Mock reply");
}
