#[cfg(test)]
mod tests {
    use super::super::provider::*;

    #[test]
    fn execution_anthropic_keeps_graph_instructions_before_prompt_change_notice() {
        let messages: Vec<_> = [
            ("system", "Graph contract and active IR"),
            ("user", "Implement"),
            ("system", "Prompt changed"),
            ("system", ""),
        ]
        .into_iter()
        .map(|(role, content)| ChatMessage {
            role: role.into(),
            content: Some(content.into()),
            reasoning_content: None,
            content_parts: None,
            tool_calls: None,
            tool_call_id: None,
            tool_name: None,
        })
        .collect();
        let (system, ordinary) = super::super::anthropic::build_anthropic_messages(&messages);
        assert_eq!(system, "Graph contract and active IR\n\nPrompt changed");
        assert_eq!(ordinary.len(), 1);
        assert_eq!(ordinary[0]["role"], "user");
    }

    // ── OpenAI tool call parsing tests ────────────────────────────────────────

    #[test]
    fn test_openai_parse_tool_calls() {
        let data: serde_json::Value = serde_json::json!({
            "choices": [{
                "message": {
                    "content": null,
                    "tool_calls": [{
                        "id": "call_abc123",
                        "type": "function",
                        "function": {
                            "name": "execute_terminal",
                            "arguments": "{\"command\": \"ls -la\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": { "prompt_tokens": 100, "completion_tokens": 50, "total_tokens": 150 }
        });

        let message = &data["choices"][0]["message"];
        let content = message["content"].as_str().map(|s| s.to_string());
        let tool_calls: Option<Vec<ToolCall>> = message["tool_calls"].as_array().map(|calls| {
            calls
                .iter()
                .map(|tc| ToolCall {
                    id: tc["id"].as_str().unwrap_or_default().to_string(),
                    function: FunctionCall {
                        name: tc["function"]["name"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string(),
                        arguments: tc["function"]["arguments"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string(),
                    },
                })
                .collect()
        });

        assert!(content.is_none());
        let calls = tool_calls.unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_abc123");
        assert_eq!(calls[0].function.name, "execute_terminal");
        assert_eq!(calls[0].function.arguments, "{\"command\": \"ls -la\"}");
    }

    #[test]
    fn test_openai_parse_no_tool_calls() {
        let data: serde_json::Value = serde_json::json!({
            "choices": [{
                "message": { "content": "Hello, world!", "tool_calls": null },
                "finish_reason": "stop"
            }],
            "usage": { "prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15 }
        });

        let message = &data["choices"][0]["message"];
        let content = message["content"].as_str().map(|s| s.to_string());
        let tool_calls: Option<Vec<ToolCall>> = message["tool_calls"].as_array().map(|calls| {
            calls
                .iter()
                .map(|tc| ToolCall {
                    id: tc["id"].as_str().unwrap_or_default().to_string(),
                    function: FunctionCall {
                        name: tc["function"]["name"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string(),
                        arguments: tc["function"]["arguments"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string(),
                    },
                })
                .collect()
        });

        assert_eq!(content.unwrap(), "Hello, world!");
        assert!(tool_calls.is_none());
    }

    #[test]
    fn test_openai_parse_multiple_tool_calls() {
        let data: serde_json::Value = serde_json::json!({
            "choices": [{
                "message": {
                    "content": null,
                    "tool_calls": [
                        { "id": "call_1", "type": "function", "function": { "name": "read_file", "arguments": "{\"path\": \"/tmp/a.txt\"}" } },
                        { "id": "call_2", "type": "function", "function": { "name": "read_file", "arguments": "{\"path\": \"/tmp/b.txt\"}" } }
                    ]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": { "prompt_tokens": 50, "completion_tokens": 30, "total_tokens": 80 }
        });

        let tool_calls: Vec<ToolCall> = data["choices"][0]["message"]["tool_calls"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tc| ToolCall {
                id: tc["id"].as_str().unwrap_or_default().to_string(),
                function: FunctionCall {
                    name: tc["function"]["name"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                    arguments: tc["function"]["arguments"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                },
            })
            .collect();

        assert_eq!(tool_calls.len(), 2);
        assert_eq!(tool_calls[0].function.name, "read_file");
        assert_eq!(tool_calls[1].function.name, "read_file");
    }

    // ── Anthropic tool call parsing tests ─────────────────────────────────────

    #[test]
    fn test_anthropic_parse_tool_use() {
        let data: serde_json::Value = serde_json::json!({
            "content": [
                { "type": "text", "text": "I'll run that command." },
                { "type": "tool_use", "id": "toolu_abc123", "name": "execute_terminal", "input": { "command": "echo hello" } }
            ],
            "stop_reason": "tool_use",
            "usage": { "input_tokens": 100, "output_tokens": 50 }
        });

        let mut text_content = String::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();

        for block in data["content"].as_array().unwrap() {
            match block["type"].as_str() {
                Some("text") => {
                    text_content.push_str(block["text"].as_str().unwrap_or(""));
                }
                Some("tool_use") => {
                    tool_calls.push(ToolCall {
                        id: block["id"].as_str().unwrap_or_default().to_string(),
                        function: FunctionCall {
                            name: block["name"].as_str().unwrap_or_default().to_string(),
                            arguments: block["input"].to_string(),
                        },
                    });
                }
                _ => {}
            }
        }

        assert_eq!(text_content, "I'll run that command.");
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].id, "toolu_abc123");
        assert_eq!(tool_calls[0].function.name, "execute_terminal");
        let args: serde_json::Value =
            serde_json::from_str(&tool_calls[0].function.arguments).unwrap();
        assert_eq!(args["command"], "echo hello");
    }

    #[test]
    fn test_anthropic_parse_no_tool_use() {
        let data: serde_json::Value = serde_json::json!({
            "content": [{ "type": "text", "text": "Just a regular response." }],
            "stop_reason": "end_turn",
            "usage": { "input_tokens": 10, "output_tokens": 5 }
        });

        let tool_calls: Vec<ToolCall> = data["content"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|b| b["type"].as_str() == Some("tool_use"))
            .map(|b| ToolCall {
                id: b["id"].as_str().unwrap_or_default().to_string(),
                function: FunctionCall {
                    name: b["name"].as_str().unwrap_or_default().to_string(),
                    arguments: b["input"].to_string(),
                },
            })
            .collect();

        assert!(tool_calls.is_empty());
    }

    #[test]
    fn test_anthropic_parse_multiple_tools() {
        let data: serde_json::Value = serde_json::json!({
            "content": [
                { "type": "text", "text": "Reading both files." },
                { "type": "tool_use", "id": "t1", "name": "read_file", "input": { "path": "/a.txt" } },
                { "type": "tool_use", "id": "t2", "name": "read_file", "input": { "path": "/b.txt" } }
            ],
            "stop_reason": "tool_use",
            "usage": { "input_tokens": 50, "output_tokens": 30 }
        });

        let tool_calls: Vec<ToolCall> = data["content"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|b| b["type"].as_str() == Some("tool_use"))
            .map(|b| ToolCall {
                id: b["id"].as_str().unwrap_or_default().to_string(),
                function: FunctionCall {
                    name: b["name"].as_str().unwrap_or_default().to_string(),
                    arguments: b["input"].to_string(),
                },
            })
            .collect();

        assert_eq!(tool_calls.len(), 2);
        assert_eq!(tool_calls[0].id, "t1");
        assert_eq!(tool_calls[1].id, "t2");
    }

    // ── Ollama tool call parsing tests ────────────────────────────────────────

    #[test]
    fn test_ollama_parse_tool_calls() {
        let data: serde_json::Value = serde_json::json!({
            "message": {
                "role": "assistant",
                "content": "",
                "tool_calls": [{ "function": { "name": "execute_terminal", "arguments": { "command": "pwd" } } }]
            }
        });

        let message = &data["message"];
        let content = message["content"].as_str().map(|s| s.to_string());
        let tool_calls: Option<Vec<ToolCall>> = message["tool_calls"].as_array().map(|calls| {
            calls
                .iter()
                .filter_map(|tc| {
                    let func = tc.get("function")?;
                    let name = func.get("name")?.as_str()?.to_string();
                    let args = func.get("arguments")?.clone();
                    Some(ToolCall {
                        id: format!("call_{}", uuid::Uuid::new_v4()),
                        function: FunctionCall {
                            name,
                            arguments: args.to_string(),
                        },
                    })
                })
                .collect()
        });

        assert!(content.is_none() || content.unwrap().is_empty());
        let calls = tool_calls.unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "execute_terminal");
    }

    #[test]
    fn test_ollama_parse_no_tool_calls() {
        let data: serde_json::Value = serde_json::json!({
            "message": { "role": "assistant", "content": "Hello!", "tool_calls": null }
        });

        let message = &data["message"];
        let content = message["content"].as_str().map(|s| s.to_string());
        let tool_calls: Option<Vec<ToolCall>> = message["tool_calls"].as_array().map(|_| vec![]);

        assert_eq!(content.unwrap(), "Hello!");
        assert!(tool_calls.is_none());
    }

    // ── MiniMax / MiMo (OpenAI-compatible) tool call parsing tests ────────────

    #[test]
    fn test_openai_compatible_parse_tool_calls() {
        let data: serde_json::Value = serde_json::json!({
            "choices": [{
                "message": {
                    "content": null,
                    "tool_calls": [{
                        "id": "call_mm1",
                        "type": "function",
                        "function": { "name": "execute_terminal", "arguments": "{\"command\": \"ls\"}" }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": { "prompt_tokens": 80, "completion_tokens": 40, "total_tokens": 120 }
        });

        let message = &data["choices"][0]["message"];
        let tool_calls: Vec<ToolCall> = message["tool_calls"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|tc| {
                Some(ToolCall {
                    id: tc["id"].as_str().unwrap_or("call_0").to_string(),
                    function: FunctionCall {
                        name: tc["function"]["name"].as_str()?.to_string(),
                        arguments: tc["function"]["arguments"]
                            .as_str()
                            .unwrap_or("{}")
                            .to_string(),
                    },
                })
            })
            .collect();

        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].id, "call_mm1");
        assert_eq!(tool_calls[0].function.name, "execute_terminal");
    }

    // ── Tool definition format conversion tests ───────────────────────────────

    #[test]
    fn test_tool_definition_to_openai_format() {
        let tools = vec![ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "execute_terminal".to_string(),
                description: "Execute a shell command".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": { "command": { "type": "string" } },
                    "required": ["command"]
                }),
            },
        }];

        let body = serde_json::json!({ "tools": tools });
        let serialized = serde_json::to_string(&body).unwrap();
        assert!(serialized.contains("execute_terminal"));
        assert!(serialized.contains("Execute a shell command"));
    }

    #[test]
    fn test_tool_definition_to_anthropic_format() {
        let tools = vec![ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "read_file".to_string(),
                description: "Read a file".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": { "path": { "type": "string" } },
                    "required": ["path"]
                }),
            },
        }];

        let anthropic_tools: Vec<serde_json::Value> = tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "name": t.function.name,
                    "description": t.function.description,
                    "input_schema": t.function.parameters
                })
            })
            .collect();

        assert_eq!(anthropic_tools.len(), 1);
        assert_eq!(anthropic_tools[0]["name"], "read_file");
        assert!(anthropic_tools[0]["input_schema"].is_object());
    }

    // ── Chat message serialization tests ──────────────────────────────────────

    #[test]
    fn test_chat_message_with_tool_calls_serialize() {
        let msg = ChatMessage {
     reasoning_content: None,
            role: "assistant".to_string(),
            content: Some("Let me check.".to_string()),
            content_parts: None,
            tool_calls: Some(vec![ToolCall {
                id: "call_123".to_string(),
                function: FunctionCall {
                    name: "read_file".to_string(),
                    arguments: "{\"path\": \"/tmp/test.txt\"}".to_string(),
                },
            }]),
            tool_call_id: None,
            tool_name: None,
        };

        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["role"], "assistant");
        assert_eq!(json["content"], "Let me check.");
        assert!(json["tool_calls"].is_array());
        assert_eq!(json["tool_calls"][0]["id"], "call_123");
    }

    #[test]
    fn test_chat_message_tool_result_serialize() {
        let msg = ChatMessage {
     reasoning_content: None,
            role: "tool".to_string(),
            content: Some("file contents".to_string()),
            content_parts: None,
            tool_calls: None,
            tool_call_id: Some("call_123".to_string()),
            tool_name: None,
        };

        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["role"], "tool");
        assert_eq!(json["tool_call_id"], "call_123");
        assert!(json.get("tool_calls").is_none());
    }

    // ── API mode enum tests ───────────────────────────────────────────────────

    #[derive(Debug, Clone, PartialEq)]
    enum ApiMode {
        OpenAI,
        Anthropic,
    }

    #[test]
    fn test_api_mode_selection() {
        let mode = ApiMode::OpenAI;
        assert_eq!(mode, ApiMode::OpenAI);
        let mode2 = ApiMode::Anthropic;
        assert_eq!(mode2, ApiMode::Anthropic);
    }
}
