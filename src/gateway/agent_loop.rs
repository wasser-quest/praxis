use crate::sm;
use crate::gateway::llm::provider::{ChatMessage, ChatRequest, ToolCall};
use crate::gateway::GatewayState;
use crate::tools::registry::build_tool_definitions_for_user;
use crate::tags::{self, TagExecution};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Global registry of active agent loops and their user input senders
static ACTIVE_LOOPS: once_cell::sync::Lazy<Arc<RwLock<HashMap<String, tokio::sync::mpsc::UnboundedSender<String>>>>> =
    once_cell::sync::Lazy::new(|| Arc::new(RwLock::new(HashMap::new())));

/// Get a sender for injecting user input into an active agent loop
pub async fn get_user_input_sender(user_id: &str) -> Option<tokio::sync::mpsc::UnboundedSender<String>> {
    let loops = ACTIVE_LOOPS.read().await;
    loops.get(user_id).cloned()
}

/// Stop an active agent loop
pub async fn stop_agent_loop(user_id: &str) {
    crate::gateway::task_control::cancel(user_id);
    let mut loops = ACTIVE_LOOPS.write().await;
    loops.remove(user_id);
}

/// Check if a stop signal has been sent
async fn should_stop(user_id: &str) -> bool {
    crate::gateway::task_control::cancellation(user_id).is_some_and(|token| token.is_cancelled())
}

/// Register an active agent loop sender
async fn register_active_loop(user_id: &str, tx: tokio::sync::mpsc::UnboundedSender<String>) {
    let mut loops = ACTIVE_LOOPS.write().await;
    loops.insert(user_id.to_string(), tx);
}

/// Unregister an active agent loop
async fn unregister_active_loop(user_id: &str) {
    let mut loops = ACTIVE_LOOPS.write().await;
    loops.remove(user_id);
}

#[derive(Debug)]
pub struct AgentLoopResult {
    pub response: String,
    pub response_message_id: Option<i64>,
    pub turns_used: i32,
    pub tag_execution: Option<TagExecution>,
    pub completed: bool,
    pub advanced: bool,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
    pub generation_ms: u64,
}

pub struct AgentLoopConfig {
    pub max_turns: i32,
    pub max_tool_calls: i32,
    pub tags_enabled: bool,
    pub sm_file: Option<String>,
    pub feedback_enabled: bool,
    pub message_on_toolcalling: bool,
    pub tool_history_limit: usize,
}

impl Default for AgentLoopConfig {
    fn default() -> Self {
        Self {
            max_turns: 10,
            max_tool_calls: 5,
            tags_enabled: true,
            sm_file: None,
            feedback_enabled: false,
            message_on_toolcalling: false,
            tool_history_limit: 50,
        }
    }
}

/// Channel for injecting user messages into a running agent loop
pub struct UserInputChannel {
    rx: tokio::sync::mpsc::UnboundedReceiver<String>,
    tx: tokio::sync::mpsc::UnboundedSender<String>,
}

impl UserInputChannel {
    pub fn new() -> Self {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        Self { rx, tx }
    }

    pub fn sender(&self) -> tokio::sync::mpsc::UnboundedSender<String> {
        self.tx.clone()
    }

    pub fn try_recv(&mut self) -> Option<String> {
        self.rx.try_recv().ok()
    }

    pub fn drain_all(&mut self) -> Vec<String> {
        let mut msgs = Vec::new();
        if let Some(first) = self.try_recv() {
            msgs.push(first);
            while let Some(next) = self.try_recv() {
                msgs.push(next);
            }
        }
        msgs
    }
}

pub async fn run_agent_loop(
    state: &GatewayState,
    user_id: &str,
    user_message_input: &str,
    config: AgentLoopConfig,
    feedback_tx: Option<tokio::sync::mpsc::UnboundedSender<String>>,
) -> anyhow::Result<AgentLoopResult> {
    let _task = crate::gateway::task_control::begin(user_id)?;
    crate::gateway::prompt::reset_task_completion(&state.db, user_id)?;
    run_agent_loop_in_task(state, user_id, user_message_input, config, feedback_tx).await
}

/// The message handler already owns the per-user task guard.
pub(crate) async fn run_agent_loop_in_task(
    state: &GatewayState,
    user_id: &str,
    user_message_input: &str,
    config: AgentLoopConfig,
    feedback_tx: Option<tokio::sync::mpsc::UnboundedSender<String>>,
) -> anyhow::Result<AgentLoopResult> {
    let result = run_agent_loop_inner(state, user_id, user_message_input, config, feedback_tx).await;
    // Strict rendering/DB errors must not leave a ghost active loop behind.
    unregister_active_loop(user_id).await;
    if let Ok(reply) = &result {
        if let Some(id) = reply.response_message_id {
            crate::dashboard::stream::assistant_saved(user_id, id, &reply.response);
            // Emit usage event for TUI/dashboard token display.
            if reply.total_tokens > 0 {
                let usage_data = serde_json::json!({
                    "prompt_tokens": reply.prompt_tokens,
                    "completion_tokens": reply.completion_tokens,
                    "total_tokens": reply.total_tokens,
                    "generation_ms": reply.generation_ms,
                    "tokens_per_sec": if reply.generation_ms > 0 {
                        (reply.completion_tokens as f64 / (reply.generation_ms as f64 / 1000.0)).round()
                    } else { 0.0 },
                });
                crate::dashboard::stream::send(user_id, "usage", &usage_data.to_string());
            }
        }
    }
    crate::dashboard::stream::send(user_id, "agent_stop", "{}");
    result
}
async fn run_agent_loop_inner(
    state: &GatewayState,
    user_id: &str,
    user_message_input: &str,
    config: AgentLoopConfig,
    feedback_tx: Option<tokio::sync::mpsc::UnboundedSender<String>>,
) -> anyhow::Result<AgentLoopResult> {
    let mut turn = 0;
    let mut completed = false;
    let mut advanced = false;
    let mut last_tag_execution = None;
    let mut user_message = user_message_input.to_string();
    let mut final_response: Option<String> = None;
    let mut final_message_id = None;
    let mut turn_limit_reached = false;
    let mut current_tool_ids = std::collections::HashSet::new();

    // Create user input channel and register sender globally
    let mut user_input = UserInputChannel::new();
    register_active_loop(user_id, user_input.sender()).await;

    // Notify the dashboard chat that the agent loop has started, so the
    // frontend can render a visually distinct "loop running" banner.
    crate::dashboard::stream::send(user_id, "agent_start", "{}");

    let mut ctx = state.db.load_context(user_id)?;
    // Immutable per-task ceilings: tool-written settings and injected input
    // cannot refill a running task. Changes apply to the next task.
    let turn_limit = ctx.settings.max_llm_turns.unwrap_or(config.max_turns).clamp(1, 128);
    let tool_limit = ctx.settings.max_tool_calls.unwrap_or(config.max_tool_calls).clamp(0, 128) as usize;
    let mut tool_calls_used = 0usize;
    // Accumulate token usage and generation timing across all LLM turns.
    let mut cumulative_prompt_tokens: u32 = 0;
    let mut cumulative_completion_tokens: u32 = 0;
    let mut cumulative_total_tokens: u32 = 0;
    let mut cumulative_generation_ms: u64 = 0;
    // Config supplies only an initial default. Subsequent tool changes/clears
    // are loaded from DB, never shadowed by a frozen loop configuration.
    if ctx.settings.sm_file.is_none() && ctx.sm_file.is_none() {
        ctx.sm_file = config.sm_file.clone();
    }
    let workspace = state.config.workspace_root()?;
    crate::gateway::prompt::route_context_with_workspace(std::path::Path::new(&state.config.root_dir), &workspace, &mut ctx, &user_message, &state.plugins, None)?;
    state.db.save_context(&ctx)?;
    // Store the RAW user input; render_user runs per request (raw-storing policy).
    state.db.add_message(
        user_id,
        &crate::db::messages::Message::user(user_message.clone()),
    )?;

    loop {
        // Check for stop signal
        if should_stop(user_id).await {
            tracing::info!(user_id = %user_id, "Stop signal received, ending agent loop");
            break;
        }

        // Drain injected user messages that arrived during previous turn
        let injected = user_input.drain_all();
        if !injected.is_empty() {
            tracing::info!(user_id = %user_id, injected_count = injected.len(), "Injecting {} user messages into conversation", injected.len());
            for msg in &injected {
                crate::gateway::prompt::append_injected_message(state, user_id, msg).await?;
            }
            // New input updates the task, but does not refill its finite budget.
            user_message = injected.last().cloned().unwrap_or(user_message);
            continue;
        }

        if turn >= turn_limit {
            tracing::warn!(user_id = %user_id, turn = turn, "Max turns reached");
            turn_limit_reached = true;
            break;
        }

        turn += 1;
        ctx.turn = turn;

        // Compact BEFORE sending a request, never after it already overflowed.
        crate::gateway::compaction::before_request(state, user_id).await?;
        // Refresh after tools and injected inputs; route BEFORE rendering.
        ctx = crate::gateway::decision_routing::prepare(state, user_id, &user_message, Some(turn), None).await?;
        let system_prompt = crate::gateway::prompt::render_system(state, &ctx, &user_message).await?;
        let mut messages = vec![ChatMessage {
            reasoning_content: None,
role: "system".to_string(),
            content: Some(system_prompt.clone()),
            content_parts: None,
            tool_calls: None,
            tool_call_id: None,
            tool_name: None,
        }];

        // Announce system-prompt changes so the model re-reads its instructions
        // (template updates via SM files or manual switches would otherwise go
        // unnoticed until the user points them out). Text-based so the notice
        // includes a diff of WHICH lines changed.
        if let Some(notice) =
            crate::gateway::prompt_change::take_prompt_change_notice_text(user_id, &system_prompt)
        {
            tracing::info!(user_id = %user_id, "System prompt changed; injecting change notice");
            messages.push(ChatMessage {
    reasoning_content: None,
                role: "system".to_string(),
                content: Some(notice),
                content_parts: None,
                tool_calls: None,
                tool_call_id: None,
                tool_name: None,
            });
        }

        // Inject latest VM screenshot so the LLM always sees the current state
        if let Some(ss_path) = ctx
            .custom_data
            .get("vm_last_screenshot")
            .and_then(|v| v.as_str())
        {
            if let Some(data_url) = crate::tools::vm_tools::screenshot_to_data_url(ss_path) {
                messages.push(ChatMessage {
    reasoning_content: None,
                    role: "user".to_string(),
                    content: Some(
                        "[Current VM screenshot — this is what is on screen right now]".to_string(),
                    ),
                    content_parts: Some(vec![
                        crate::gateway::llm::provider::ContentPart::ImageUrl {
                            image_url: crate::gateway::llm::provider::ImageUrlDetail {
                                url: data_url,
                                detail: Some("high".to_string()),
                            },
                        },
                    ]),
                    tool_calls: None,
                    tool_call_id: None,
                    tool_name: None,
                });
            }
        }

        let token_budget = ctx.settings.history_token_limit.unwrap_or(crate::db::messages::DEFAULT_HISTORY_TOKENS);
        let (history, _history_tokens) = state
            .db
            .get_messages_with_token_budget(user_id, token_budget)?;
        let current_user_idx = history.iter().rposition(|m| m.role == "user");
        // Render only the CURRENT user turn with fresh context; older turns stay raw.
        let rendered_current_user =
            crate::gateway::prompt::render_user(state, &ctx, &user_message).await?;

        let image_msg_indices = crate::gateway::prompt::history_image_indices(
            &history, &current_tool_ids, state.llm.get().history_image_messages(),
        );
        let omitted_history_images = history.iter()
            .filter(|m| m.content_parts.as_ref().is_some_and(|p| !p.is_empty()))
            .count().saturating_sub(image_msg_indices.len());
        if omitted_history_images > 0 {
            messages.push(ChatMessage {
    reasoning_content: None,
                role: "system".into(),
                content: Some(crate::gateway::prompt::OMITTED_HISTORY_IMAGES_NOTICE.into()),
                content_parts: None, tool_calls: None, tool_call_id: None, tool_name: None,
            });
        }

        for (msg_idx, msg) in history.iter().enumerate() {
            // The history preference may hide OLD tool conversations, but not
            // this task's pending tool chain (including loaded skill bodies).
            let current_tool_message = msg.tool_call_id.as_ref().is_some_and(|id| current_tool_ids.contains(id))
                || msg.tool_calls.as_ref().is_some_and(|calls| calls.iter().any(|call| current_tool_ids.contains(&call.id)));
            if !ctx.settings.history_with_toolcalls
                && !current_tool_message
                && (msg.role == "tool" || msg.tool_calls.is_some())
            {
                continue;
            }
            let tool_calls = msg.tool_calls.as_ref().map(|tcs| {
                tcs.iter()
                    .map(|tc| ToolCall {
                        id: tc.id.clone(),
                        function: crate::gateway::llm::provider::FunctionCall {
                            name: tc.function.name.clone(),
                            arguments: tc.function.arguments.clone(),
                        },
                    })
                    .collect()
            });
            messages.push(ChatMessage {
    reasoning_content: None,
                role: msg.role.clone(),
                content: if Some(msg_idx) == current_user_idx {
                    Some(rendered_current_user.clone())
                } else if msg.content.is_empty() {
                    None
                } else {
                    Some(msg.content.clone())
                },
                content_parts: {
                    // Only include image data for the last 2 messages with content_parts
                    if image_msg_indices.contains(&msg_idx) {
                        let cp = msg.content_parts.as_ref().map(|parts| {
                            let converted: Vec<crate::gateway::llm::provider::ContentPart> = parts
                                .iter()
                                .filter_map(|v| serde_json::from_value(v.clone()).ok())
                                .collect();
                            converted
                        });
                        cp
                    } else {
                        None
                    }
                },
                tool_calls,
                tool_call_id: msg.tool_call_id.clone(),
                tool_name: msg.tool_name.clone(),
            });
        }

        // Only bootstrap tools and schemas discovered during this owned task.
        let tools = build_tool_definitions_for_user(&ctx.settings, Some(&state.plugins.tool_definitions()), Some(&state.db), user_id);

        // Clone tool names and definitions for validation before moving tools into request
        let tool_names: Vec<String> = tools.iter().map(|t| t.function.name.clone()).collect();
        let tools_for_validation = tools.clone();

        if tool_calls_used >= tool_limit {
            messages.push(ChatMessage { role:"system".into(), content:Some("The task's tool budget is exhausted. No further actions may run. Give an honest final answer using existing results; state unfinished work explicitly.".into()), reasoning_content:None, content_parts:None, tool_calls:None, tool_call_id:None, tool_name:None });
        }
        let request = ChatRequest {
            messages: messages.clone(),
            tools: if tools.is_empty() || tool_calls_used >= tool_limit { None } else { Some(tools) },
            temperature: Some(0.7),
            max_tokens: Some(state.llm.get().task_output_tokens()),
            model: ctx.settings.model.clone(),
            vision_provider: ctx.settings.vision_provider.clone().or_else(|| state.config.vision_provider.clone()),
            vision_model: ctx.settings.vision_model.clone().or_else(|| state.config.vision_model.clone()),
            thinking: crate::gateway::llm::provider::ThinkingMode::from_setting(&ctx.settings.thinking_mode),
        };

        // Log LLM request details
        let msg_count = request.messages.len();
        let tool_count = request.tools.as_ref().map(|t| t.len()).unwrap_or(0);
        tracing::info!(
            user_id = %user_id,
            turn = ctx.turn,
            message_count = msg_count,
            tool_count = tool_count,
            history_images_included = image_msg_indices.len(),
            history_images_omitted = omitted_history_images,
            model = ?request.model,
            provider = ?ctx.settings.provider,
            ">>> LLM REQUEST >>>"
        );

        // Log the LLM call itself into activity log so dashboard shows what the LLM is doing
        {
            let conn = state.db.conn();
            let vm_name = ctx.custom_data.get("active_vm").and_then(|v| v.as_str()).unwrap_or("host");
            let _ = conn.execute(
                "INSERT INTO vm_activity_log (vm_id, action, input) VALUES (?1, ?2, ?3)",
                rusqlite::params![
                    vm_name,
                    "llm_request",
                    &format!("turn={} provider={:?} model={:?} messages={} tools={}", ctx.turn, ctx.settings.provider, request.model, msg_count, tool_count)
                ],
            );
        }

        // Select provider: free_router when use_freerouter is enabled
        let provider = if ctx.settings.use_freerouter {
            Some("free_router")
        } else {
            ctx.settings.provider.as_deref()
        };

        // Call LLM
        let llm_start = std::time::Instant::now();
        let response = match state
            .llm.get()
            .streaming_chat(request, provider, user_id)
            .await
        {
            Ok(r) => {
                let elapsed = llm_start.elapsed();
                cumulative_generation_ms += elapsed.as_millis() as u64;
                if let Some(ref usage) = r.usage {
                    cumulative_prompt_tokens = cumulative_prompt_tokens.saturating_add(usage.prompt_tokens);
                    cumulative_completion_tokens = cumulative_completion_tokens.saturating_add(usage.completion_tokens);
                    cumulative_total_tokens = cumulative_total_tokens.saturating_add(usage.total_tokens);
                }
                r
            },
            Err(e) => {
                tracing::error!(user_id = %user_id, error = %e, "<<< LLM ERROR <<<");
                {
                    let conn = state.db.conn();
                    let vm_name = ctx.custom_data.get("active_vm").and_then(|v| v.as_str()).unwrap_or("host");
                    let _ = conn.execute(
                        "INSERT INTO vm_activity_log (vm_id, action, input) VALUES (?1, ?2, ?3)",
                        rusqlite::params![vm_name, "llm_error", &e.to_string()],
                    );
                }
                if let Some(ref tx) = feedback_tx {
                    let _ = tx.send(format!("LLM error: {}", e));
                }
                return Err(e);
            }
        };

        // Log LLM response details
        let has_tool_calls = response.tool_calls.is_some();
        let tool_calls_count = response.tool_calls.as_ref().map(|tc| tc.len()).unwrap_or(0);
        let response_preview = response.content.as_deref()
            .map(|c| crate::util::truncate_chars_ascii(c, 300))
            .unwrap_or_else(|| "(no content)".to_string());
        tracing::info!(
            user_id = %user_id,
            has_tool_calls = has_tool_calls,
            tool_calls_count = tool_calls_count,
            response_preview = %response_preview,
            agent_name = %ctx.settings.agent_name,
            "<<< LLM RESPONSE <<<"
        );

        // Log LLM response into activity log
        {
            let conn = state.db.conn();
            let vm_name = ctx.custom_data.get("active_vm").and_then(|v| v.as_str()).unwrap_or("host");
            let _ = conn.execute(
                "INSERT INTO vm_activity_log (vm_id, action, input, output) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![
                    vm_name,
                    if has_tool_calls { "llm_tool_calls" } else { "llm_response" },
                    &format!("tool_calls={}", tool_calls_count),
                    &response_preview,
                ],
            );
        }

        // Handle tool calls
        if let Some(tool_calls) = &response.tool_calls {
            current_tool_ids.extend(tool_calls.iter().map(|call| call.id.clone()));
            // Persist the assistant message with tool_calls
            let db_tool_calls: Vec<crate::db::messages::ToolCallData> = tool_calls
                .iter()
                .map(|tc| crate::db::messages::ToolCallData {
                    id: tc.id.clone(),
                    function: crate::db::messages::FunctionCallData {
                        name: tc.function.name.clone(),
                        arguments: tc.function.arguments.clone(),
                    },
                })
                .collect();
            state.db.add_message(
                user_id,
                &crate::db::messages::Message::assistant_with_tool_calls(
                    response.content.clone().unwrap_or_default(),
                    db_tool_calls,
                ),
            )?;

            if let Some(ref content) = response.content {
                if !content.trim().is_empty() {
                    let send_first = ctx
                        .custom_data
                        .get("send_first_response")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(true);
                    if send_first {
                        if let Some(ref tx) = feedback_tx {
                            let _ = tx.send(content.clone());
                        }
                    }
                }
            }

            for tc in tool_calls {
                let exhausted = tool_calls_used >= tool_limit;
                tool_calls_used = tool_calls_used.saturating_add(1);
                let args: serde_json::Value = serde_json::from_str(&tc.function.arguments).unwrap_or_default();
                let skipped = if should_stop(user_id).await {
                    Some("Task cancelled; tool not executed".to_string())
                } else if exhausted {
                    Some("Maximum tool calls reached; tool not executed".to_string())
                } else if !tool_names.contains(&tc.function.name)
                    || !crate::tools::discovery::enabled(&state.db, &state.plugins, &tc.function.name)
                {
                    Some(format!("Unknown tool '{}'; tool not executed", tc.function.name))
                } else {
                    validate_tool_params(&tc.function.name, &args, &tools_for_validation).err().map(|e| e.to_string())
                };
                if let Some(reason) = skipped {
                    // Persist every tool result, including skipped calls. The
                    // next turn rebuilds history from DB, not this local Vec.
                    let mut message = crate::db::messages::Message::tool(format!("Error: {reason}"), tc.id.clone());
                    message.tool_name = Some(tc.function.name.clone());
                    state.db.add_message(user_id, &message)?;
                    continue;
                }

                if let Some(ref tx) = feedback_tx {
                    if config.feedback_enabled && config.message_on_toolcalling {
                        let _ = tx.send(format!("Calling tool: {}", tc.function.name));
                    }
                }

                // Keep invocation diagnostics without logging speech/image prompts.
                tracing::info!(
                    user_id = %user_id,
                    tool = %tc.function.name,
                    call_id = %tc.id,
                    args_bytes = tc.function.arguments.len(),
                    "=== TOOL CALL START ==="
                );

                // Dashboard chat visibility: announce the tool call so the user
                // sees what the agent is doing right now.
                {
                    let args_preview: serde_json::Value =
                        serde_json::from_str(&tc.function.arguments).unwrap_or_default();
                    let preview = match tc.function.name.as_str() {
                        "write_file" | "edit_file" | "modify_source" => serde_json::json!({
                            "path": args_preview.get("path").cloned().unwrap_or_default(),
                            "content_bytes": tc.function.arguments.len(),
                        }),
                        "execute_decision" => serde_json::json!({
                            "ir_bytes": args_preview.get("ir").and_then(serde_json::Value::as_str).map(str::len),
                        }),
                        "read_file" | "get_context" | "rag_search" => serde_json::json!({
                            "path": args_preview.get("path").cloned().unwrap_or_default(),
                            "query": args_preview.get("query").cloned().unwrap_or_default(),
                        }),
                        _ => serde_json::json!({
                            "args": crate::util::truncate_chars(
                                &tc.function.arguments, 200),
                        }),
                    };
                    crate::dashboard::stream::send(
                        user_id,
                        "tool_call",
                        &serde_json::json!({
                            "tool": tc.function.name,
                            "call_id": tc.id,
                            "args_preview": preview,
                        })
                        .to_string(),
                    );
                }

                let tool_start_time = chrono::Local::now();
                let result = execute_tool_call(&state.db, user_id, tc, &state.plugins).await;
                // The tool may have changed settings, state, or custom_data.
                // Start history/screenshot updates from that fresh snapshot.
                ctx = state.db.load_context(user_id)?;
                let tool_duration = chrono::Local::now().signed_duration_since(tool_start_time);
                let mut final_result = result.clone();
                let mut image_content_parts: Option<Vec<serde_json::Value>> = None;

                // Record tool call in custom_data.tool_history and expose used_tools at root
                {
                    if ctx.custom_data.is_null() {
                        ctx.custom_data = serde_json::json!({});
                    }
                    if let Some(obj) = ctx.custom_data.as_object_mut() {
                        if !obj.contains_key("tool_history") {
                            obj.insert("tool_history".to_string(), serde_json::json!([]));
                        }
                        let args: serde_json::Value = serde_json::from_str(&tc.function.arguments).unwrap_or_default();
                        let result_preview = crate::util::truncate_chars_ascii(&final_result, 500);

                        if let Some(history) = obj.get_mut("tool_history").and_then(|v| v.as_array_mut()) {
                            history.push(serde_json::json!({
                                "iteration": turn,
                                "tool": tc.function.name,
                                "args": args,
                                "result": result_preview,
                                "timestamp": tool_start_time.format("%Y-%m-%d %H:%M:%S").to_string(),
                                "duration_ms": tool_duration.num_milliseconds()
                            }));
                            // Respect context-tool changes made during this loop.
                            let limit = ctx.settings.tool_history_limit;
                            if history.len() > limit {
                                let drain_count = history.len() - limit;
                                history.drain(0..drain_count);
                            }
                        }

                        // Build used_tools object at root of custom_data
                        let history_arr = obj.get("tool_history").and_then(|v| v.as_array()).cloned().unwrap_or_default();
                        obj.insert("used_tools".to_string(), serde_json::json!({
                            "last_call": tc.function.name,
                            "last_result": crate::util::truncate_chars(&final_result, 1000),
                            "last_args": args,
                            "count": turn,
                            "history": history_arr
                        }));
                    }
                }

                // Tool text can contain private data; diagnostics record metadata only.
                tracing::info!(
                    user_id = %user_id,
                    tool = %tc.function.name,
                    call_id = %tc.id,
                    result_bytes = result.len(),
                    "=== TOOL CALL END ==="
                );

                // Dashboard chat visibility: report the tool result so the user
                // can follow what happened.
                {
                    let result_for_stream = if tc.function.name == "understand_image" {
                        // Keep the chat payload small: text summary only, the
                        // image itself is streamed separately as chat_image.
                        serde_json::from_str::<serde_json::Value>(&result)
                            .ok()
                            .and_then(|v| v.get("text").and_then(|t| t.as_str()).map(|s| s.to_string()))
                            .unwrap_or_else(|| crate::util::truncate_chars(&result, 400))
                    } else {
                        crate::util::truncate_chars(&result, 400)
                    };
                    crate::dashboard::stream::send(
                        user_id,
                        "tool_result",
                        &serde_json::json!({
                            "tool": tc.function.name,
                            "call_id": tc.id,
                            "duration_ms": tool_duration.num_milliseconds(),
                            "result": result_for_stream,
                            "success": crate::gateway::tool_results::display_success(&result),
                        })
                        .to_string(),
                    );
                }

                // Check if understand_image returned image data
                if tc.function.name == "understand_image" {
                    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&result) {
                        if let Some(parts) = parsed.get("content_parts").and_then(|v| v.as_array())
                        {
                            if !parts.is_empty() {
                                final_result = parsed
                                    .get("text")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(&result)
                                    .to_string();
                                image_content_parts = Some(parts.clone());
                                tracing::info!(
                                    "understand_image: got {} content_parts, text: {}",
                                    parts.len(),
                                    final_result
                                );
                                // Show the image directly in the dashboard chat so
                                // the user sees what the agent is looking at.
                                for part in parts {
                                    if let Some(url) = part
                                        .get("image_url")
                                        .and_then(|iu| iu.get("url"))
                                        .and_then(|u| u.as_str())
                                    {
                                        crate::dashboard::stream::send(
                                            user_id,
                                            "chat_image",
                                            &serde_json::json!({
                                                "url": url,
                                                "caption": final_result,
                                            })
                                            .to_string(),
                                        );
                                    }
                                }
                            }
                        }
                    }
                }

                // Log ALL tool calls to vm_activity_log for dashboard visibility
                {
                    let conn = state.db.conn();
                    let vm_name = if tc.function.name.starts_with("vm_") {
                        // Try to extract real vm name from tool arguments
                        let args_parsed: serde_json::Value = serde_json::from_str(&tc.function.arguments).unwrap_or(serde_json::json!({}));
                        args_parsed.get("name").and_then(|v| v.as_str()).or_else(|| args_parsed.get("vm_name").and_then(|v| v.as_str())).unwrap_or("praxis-vm").to_string()
                    } else {
                        // For non-vm tools in VM mode, tag with the active VM name if available
                        ctx.custom_data.get("active_vm").and_then(|v| v.as_str()).unwrap_or("host").to_string()
                    };
                    let _ = conn.execute(
                        "INSERT INTO vm_activity_log (vm_id, action, input, output) VALUES (?1, ?2, ?3, ?4)",
                        rusqlite::params![
                            vm_name,
                            &tc.function.name,
                            &tc.function.arguments,
                            crate::util::truncate_chars(&result, 2000)
                        ],
                    );
                }

                // Auto-screenshot after VM tool calls
                if tc.function.name.starts_with("vm_") && tc.function.name != "vm_screenshot" {
                    let auto_screenshot = ctx.settings.vm_screenshot_enabled;
                    if auto_screenshot {
                        let data_dir =
                            std::env::var("DATA_DIR").unwrap_or_else(|_| "./data".to_string());
                        let args_parsed: serde_json::Value = serde_json::from_str(&tc.function.arguments).unwrap_or(serde_json::json!({}));
                        let vm_name = args_parsed.get("name").and_then(|v| v.as_str()).or_else(|| args_parsed.get("vm_name").and_then(|v| v.as_str())).unwrap_or("praxis-vm");
                        if let Some(path) =
                            crate::tools::vm_tools::save_screenshot_to_disk(vm_name, &data_dir)
                                .await
                        {
                            final_result = format!("{}\n\nScreenshot saved to: {}", result, path);
                            if ctx.custom_data.is_object() {
                                ctx.custom_data["vm_last_screenshot"] = serde_json::json!(path);
                            } else {
                                ctx.custom_data = serde_json::json!({ "vm_last_screenshot": path });
                            }
                        }
                    }
                }

                let stored_result = crate::gateway::tool_results::prepare_for_call(&state.db, user_id, tc, &final_result)?;
                let msg = if let Some(parts) = image_content_parts {
                    crate::db::messages::Message::tool_with_image(
                        stored_result,
                        tc.id.clone(),
                        parts,
                    )
                } else {
                    crate::db::messages::Message::tool(stored_result, tc.id.clone())
                };
                // Set tool_name for Ollama compatibility
                let mut msg = msg;
                msg.tool_name = Some(tc.function.name.clone());
                state.db.add_message(user_id, &msg)?;
                state.db.save_context(&ctx)?;
                // The attempt was charged before validation/execution.
            }
            ctx = state.db.load_context(user_id)?;
            if ctx.settings.done {
                if let Err(error) = super::action_contracts::require(user_id, "_complete") {
                    ctx.settings.done = false;
                    state.db.save_context(&ctx)?;
                    let mut message = crate::db::messages::Message::assistant(format!("Execution guard rejected completion: {error}"));
                    message.role = "system".into();
                    state.db.add_message(user_id, &message)?;
                    continue;
                }
                tracing::info!(user_id, turn, "Current task explicitly marked complete");
                completed = true;
                break;
            }
            continue;
        }

        // Preserve context/skill commands received while the provider was busy.
        ctx = state.db.load_context(user_id)?;
        // No tool calls — process response (final answer)
        let raw_response = response.content.unwrap_or_default();
        tracing::info!(
            user_id = %user_id,
            turn = ctx.turn,
            response_len = raw_response.len(),
            "<<< LLM FINAL RESPONSE (no tool calls) <<<"
        );

        // Strip think tags
        let response_text = crate::gateway::poml::convert_think_tags(&raw_response);

        // Extract agent signals first
        let (agent_signals, response_text) =
            crate::gateway::poml::extract_agent_signals(&response_text);

        // Process agent signals
        for signal in &agent_signals {
            match signal {
                crate::gateway::poml::AgentSignal::Complete => {
                    completed = true;
                }
                crate::gateway::poml::AgentSignal::Next => {
                    advanced = true;
                }
                crate::gateway::poml::AgentSignal::Feedback(msg) => {
                    if let Some(ref tx) = feedback_tx {
                        let _ = tx.send(msg.clone());
                    }
                }
                crate::gateway::poml::AgentSignal::Push(template) => {
                    ctx.settings.active_templates.push(template.clone());
                }
                crate::gateway::poml::AgentSignal::Pop => {
                    ctx.settings.active_templates.pop();
                }
                crate::gateway::poml::AgentSignal::Path(path) => {
                    ctx.settings.path = path.clone();
                }
                crate::gateway::poml::AgentSignal::Set(key, value) => {
                    if ctx.custom_data.is_null() {
                        ctx.custom_data = serde_json::json!({});
                    }
                    if let Some(map) = ctx.custom_data.as_object_mut() {
                        map.insert(key.clone(), serde_json::json!(value));
                    }
                }
                _ => {}
            }
        }

        // Parse and execute tags if enabled
        if config.tags_enabled {
            let mut tag_result = tags::parse_tags_with_prefix(&response_text, &ctx.settings.tag_prefix)?;
            let mut tag_exec = tags::execute_tags(&tag_result, &mut ctx);

            // Send feedback messages
            for msg in &tag_exec.feedback_messages {
                if let Some(ref tx) = feedback_tx {
                    let _ = tx.send(msg.clone());
                }
            }

            if tag_exec.should_complete {
                completed = true;
            }
            if tag_exec.should_advance {
                advanced = true;
            }

            // Save learned data
            if !tag_exec.learned_facts.is_empty()
                || !tag_exec.learned_preferences.is_empty()
                || !tag_exec.learned_topics.is_empty()
            {
                let saved = crate::db::memory_profiles::update_for_context(&state.db, &ctx, None, |memory| {
                    for fact in &tag_exec.learned_facts {
                        crate::db::memory::add_learned_fact(memory, fact);
                    }
                    for (key, val) in &tag_exec.learned_preferences {
                        crate::db::memory::update_preference(memory, key, val);
                    }
                    for topic in &tag_exec.learned_topics {
                        crate::db::memory::add_topic(memory, topic);
                    }
                    Ok(())
                });
                if saved.is_err() {
                    // Missing categories must not dump into standard or discard
                    // the answer. Make failed persistence explicit, not claimed.
                    let notice = "Memory was NOT saved. Inspect/create/load the relevant profile before an explicit retry; no other profile was changed.";
                    tag_result.cleaned_response.push_str(&format!("\n\n{notice}"));
                    tag_exec.learned_facts.clear();
                    tag_exec.learned_preferences.clear();
                    tag_exec.learned_topics.clear();
                    tag_exec.feedback_messages.push(notice.to_owned());
                    if let Some(ref tx) = feedback_tx { let _ = tx.send(notice.to_owned()); }
                    tracing::warn!("Profile learning write failed; no fallback or automatic retry");
                }
            }

            last_tag_execution = Some(tag_exec);

            if !tag_result.cleaned_response.trim().is_empty() {
                final_response = Some(tag_result.cleaned_response.clone());
                let mut msg = crate::db::messages::Message::assistant(tag_result.cleaned_response.clone());
                if cumulative_total_tokens > 0 {
                    msg.prompt_tokens = Some(cumulative_prompt_tokens);
                    msg.completion_tokens = Some(cumulative_completion_tokens);
                    msg.total_tokens = Some(cumulative_total_tokens);
                    msg.generation_ms = Some(cumulative_generation_ms);
                }
                final_message_id = Some(state.db.add_message(user_id, &msg)?);
            }
        } else if !response_text.trim().is_empty() {
            final_response = Some(response_text.clone());
            let mut msg = crate::db::messages::Message::assistant(response_text.clone());
            if cumulative_total_tokens > 0 {
                msg.prompt_tokens = Some(cumulative_prompt_tokens);
                msg.completion_tokens = Some(cumulative_completion_tokens);
                msg.total_tokens = Some(cumulative_total_tokens);
                msg.generation_ms = Some(cumulative_generation_ms);
            }
            final_message_id = Some(state.db.add_message(user_id, &msg)?);
        }

        let mut guard_error = None;
        if completed {
            if let Err(error) = super::action_contracts::require(user_id, "_complete") {
                completed = false;
                guard_error = Some(error.to_string());
                if let Some(exec) = &mut last_tag_execution { exec.should_complete = false; }
            }
        }
        if advanced {
            let sm = sm::load_file(crate::gateway::prompt::workflow_name(&ctx)).map_err(|e| anyhow::anyhow!("{e}"))?;
            let mut value = serde_json::to_value(&ctx)?;
            if let Some(next) = sm::advance_workflow(&sm, &value) {
                match super::action_contracts::require_for_workflow(user_id, &sm, &next) {
                    Ok(()) => {
                        anyhow::ensure!(sm::transition_to(&sm, &mut value, &next), "SM target state does not exist: {next}");
                        ctx = serde_json::from_value(value)?;
                        ctx.settings.active_state = ctx.active_state.clone();
                    }
                    Err(error) => guard_error = Some(error.to_string()),
                }
            }
            advanced = false;
        }

        state.db.save_context(&ctx)?;

        if completed {
            break;
        }

        // A plain final answer cannot bypass the completion contract either.
        if guard_error.is_none() {
            guard_error = super::action_contracts::require(user_id, "_complete").err().map(|e| e.to_string());
        }
        if let Some(error) = guard_error {
            let mut message = crate::db::messages::Message::assistant(format!("Execution guard rejected completion/transition: {error}"));
            message.role = "system".into();
            state.db.add_message(user_id, &message)?;
            continue;
        }

        // Drain all injected user messages and add them for the next turn
        let injected = user_input.drain_all();
        if !injected.is_empty() {
            tracing::info!(user_id = %user_id, injected_count = injected.len(), "Injecting {} user messages into conversation", injected.len());
            for msg in &injected {
                crate::gateway::prompt::append_injected_message(state, user_id, msg).await?;
            }
            // New input cannot refill this task's immutable turn budget.
            user_message = injected.last().cloned().unwrap_or(user_message);
            continue;
        }

        // If no tool calls and not completed, we're done with this message
        break;
    }

    // Never substitute an older task's answer after a tool-only turn, limit or
    // cancellation. Only a text response from THIS invocation is a final reply.
    if should_stop(user_id).await {
        return Err(crate::gateway::llm::error::ProviderError::new(
            crate::gateway::llm::error::ErrorKind::Cancelled,
        ).into());
    }
    if turn_limit_reached {
        anyhow::bail!("Agent turn limit reached after {turn} LLM turns before a final answer. Tool results are saved; increase settings.max_llm_turns to allow a longer task.");
    }
    let final_response = match final_response {
        Some(text) => text,
        None if completed => "The agent marked the task complete without a text reply.".to_string(),
        None => anyhow::bail!("Agent stopped without a final text reply. Tool results are saved."),
    };

    Ok(AgentLoopResult {
        response: final_response,
        response_message_id: final_message_id,
        turns_used: turn,
        tag_execution: last_tag_execution,
        completed,
        advanced,
        prompt_tokens: cumulative_prompt_tokens,
        completion_tokens: cumulative_completion_tokens,
        total_tokens: cumulative_total_tokens,
        generation_ms: cumulative_generation_ms,
    })
}

/// Generate a compaction summary of the conversation using the LLM.
/// This condenses the conversation into a short summary for context preservation.
pub async fn generate_compaction_summary(
    state: &GatewayState,
    user_id: &str,
    compaction_template: Option<&str>,
) -> anyhow::Result<String> {
    let (messages, _) = state.db.get_messages_with_token_budget(user_id, usize::MAX)?;
    let ctx = state.db.load_context(user_id)?;
    crate::gateway::compaction::summarize(state, user_id, &messages,
        &ctx.settings.compaction_summary, compaction_template).await
}

async fn execute_tool_call(
    db: &crate::db::Database,
    user_id: &str,
    tc: &ToolCall,
    plugins: &crate::plugins::PluginRegistry,
) -> String {
    tracing::info!(tool = %tc.function.name, args_bytes = tc.function.arguments.len(), "execute_tool_call: dispatching");

    let resolved;
    let tc = if tc.function.name == "execute_decision" {
        resolved = match super::decision_ir::resolve(db, user_id, tc, plugins) {
            Ok(call) => call,
            Err(error) => return format!("Error: {error}; decision not executed"),
        };
        &resolved
    } else { tc };

    let args: serde_json::Value = match serde_json::from_str(&tc.function.arguments) {
        Ok(v) => v,
        Err(e) => return format!("Error parsing arguments: {}", e),
    };

    let args = match crate::tools::tool_output::execution_args(&args) {
        Ok(args) => args,
        Err(error) => return format!("Error: {error}; tool not executed"),
    };
    if !plugins.manages_contract(&tc.function.name) {
        if let Err(error) = super::action_contracts::before_tool(user_id, &tc.function.name) {
            return format!("Error: {error}; tool not executed");
        }
    }
    let ctx_data = db
        .load_context(user_id)
        .ok()
        .map(|ctx| ctx.custom_data)
        .filter(|v| !v.is_null());

    let all_secrets = crate::db::secrets::get_secrets();
    let plugin_secrets = plugins.secrets_for_tool(&tc.function.name, &all_secrets);

    match tc.function.name.as_str() {
        "apply_patch" => crate::tools::apply_patch::run(user_id, &tc.id, &args).await
            .unwrap_or_else(|e| format!("Error: {e}")),
        "inspect_file" => crate::tools::apply_patch::inspect(user_id, &args).await
            .unwrap_or_else(|e| format!("Error: {e}")),
        "run_check" => super::action_contracts::run(user_id, &tc.id, &args).await
            .unwrap_or_else(|e| format!("Error: {e}")),
        "read_tool_result" => crate::tools::tool_output::run(db, user_id, &args)
            .unwrap_or_else(|e| format!("Error: {e}")),
        "search_tools" => crate::tools::discovery::search(db, plugins, user_id, &args)
            .unwrap_or_else(|e| format!("Error: {e}")),
        "memory_profile_create" => crate::tools::memory::profile_create(db, user_id, &args)
            .unwrap_or_else(|e| format!("Error: {e}")),
        "memory_profile_load" => crate::tools::memory::profile_load(db, user_id, &args)
            .unwrap_or_else(|e| format!("Error: {e}")),
        "memory_profile_list" => crate::tools::memory::profile_list(db, user_id)
            .unwrap_or_else(|e| format!("Error: {e}")),
        "memory_get" => crate::tools::memory::get(db, user_id, &args)
            .unwrap_or_else(|e| format!("Error: {e}")),
        "memory_set" => crate::tools::memory::set(db, user_id, &args)
            .unwrap_or_else(|e| format!("Error: {e}")),
        "search_skills" => crate::tools::search_skills::run(db, &args).await
            .unwrap_or_else(|e| format!("Error: {e}")),
        "use_skill" => crate::tools::use_skill::run(db, &args).await
            .unwrap_or_else(|e| format!("Error: {}", e)),
        "execute_terminal" => {
            // If VM_MODE=vm, redirect to VM
            let vm_mode = std::env::var("VM_MODE").unwrap_or_else(|_| "shared".to_string());
            let vm_enabled = std::env::var("VM_ENABLED")
                .map(|v| v == "true" || v == "1")
                .unwrap_or(false);
            if vm_enabled && vm_mode == "vm" {
                let command = args["command"].as_str().unwrap_or("");
                match crate::tools::vm_tools::dispatch_vm_tool(
                    "vm_shell",
                    &serde_json::json!({"command": command}),
                )
                .await
                {
                    Some(result) => result,
                    None => "Error: VM not running. Start a VM first with vm_start.".to_string(),
                }
            } else {
                let command = args["command"].as_str().unwrap_or("");
                match crate::tools::execute_terminal::execute_terminal(command, None).await {
                    Ok(result) => result.render(),
                    Err(e) => format!("Error: {}", e),
                }
            } // end else (shared mode)
        }
        "run_background" => {
            let command = args["command"].as_str().unwrap_or("");
            let cwd = args["cwd"].as_str();
            match crate::tools::execute_terminal::start_background(command, cwd, Some(user_id)).await {
                Ok(job_id) => format!(
                    "Background job started: {} (command: {}). You can keep working; it will announce completion automatically. Check with background_status job_id={}.",
                    job_id, command, job_id
                ),
                Err(e) => format!("Error: {}", e),
            }
        }
        "background_status" => {
            match args["job_id"].as_str() {
                Some(id) => match crate::tools::execute_terminal::job_status(id) {
                    Some(job) => serde_json::to_string_pretty(&job).unwrap_or_else(|_| format!("{}", id)),
                    None => format!("Unknown job id: {}. Use no job_id to list all jobs.", id),
                },
                None => {
                    let jobs = crate::tools::execute_terminal::list_jobs();
                    if jobs.is_empty() {
                        "No background jobs.".to_string()
                    } else {
                        serde_json::to_string_pretty(&jobs).unwrap_or_else(|_| "jobs".to_string())
                    }
                }
            }
        }
        "delegate_task" => {
            // Needs GatewayState for the child agent loop; fetch it from the
            // global gateway state accessor used by the message handler.
            // Boxed to break the async recursion (loop -> tool -> child loop).
            match crate::gateway::state_ref() {
                Some(state) => {
                    let state = state.clone();
                    let user_id = user_id.to_string();
                    let args = args.clone();
                    Box::pin(async move {
                        crate::gateway::delegation::delegate_task(&state, &user_id, &args).await
                    })
                    .await
                }
                None => "Error: gateway state unavailable for delegation.".to_string(),
            }
        }
        "list_delegations" => {
            match crate::gateway::delegation::list_delegations(db, user_id) {
                Ok(list) if list.is_empty() => "No delegations yet.".to_string(),
                Ok(list) => serde_json::to_string_pretty(&list)
                    .unwrap_or_else(|_| format!("{} delegations", list.len())),
                Err(e) => format!("Error: {}", e),
            }
        }
        "write_file" => {
            let vm_mode = std::env::var("VM_MODE").unwrap_or_else(|_| "shared".to_string());
            let vm_enabled = std::env::var("VM_ENABLED")
                .map(|v| v == "true" || v == "1")
                .unwrap_or(false);
            if vm_enabled && vm_mode == "vm" {
                let path = args["path"].as_str().unwrap_or("");
                let content = args["content"].as_str().unwrap_or("");
                match crate::tools::vm_tools::dispatch_vm_tool(
                    "vm_file_transfer",
                    &serde_json::json!({"path": path, "content": content, "direction": "to_vm"}),
                )
                .await
                {
                    Some(result) => result,
                    None => "Error: VM not running. Start a VM first with vm_start.".to_string(),
                }
            } else {
                let path = args["path"].as_str().unwrap_or("");
                let content = args["content"].as_str().unwrap_or("");
                match crate::tools::write_file::write_file(path, content).await {
                    Ok(_) => format!("File written: {}", path),
                    Err(e) => format!("Error: {}", e),
                }
            } // end else (shared mode)
        }
        "edit_file" => {
            let vm_mode = std::env::var("VM_MODE").unwrap_or_else(|_| "shared".to_string());
            let vm_enabled = std::env::var("VM_ENABLED")
                .map(|v| v == "true" || v == "1")
                .unwrap_or(false);
            if vm_enabled && vm_mode == "vm" {
                let path = args["path"].as_str().unwrap_or("");
                let old_text = args["old_text"]
                    .as_str()
                    .or_else(|| args["old_string"].as_str())
                    .unwrap_or("");
                let new_text = args["new_text"]
                    .as_str()
                    .or_else(|| args["new_string"].as_str())
                    .unwrap_or("");
                let cmd = format!(
                    "sed -i 's/{}/{}/g' {}",
                    old_text.replace('/', "\\/"),
                    new_text.replace('/', "\\/"),
                    path
                );
                match crate::tools::vm_tools::dispatch_vm_tool(
                    "vm_shell",
                    &serde_json::json!({"command": cmd}),
                )
                .await
                {
                    Some(result) => result,
                    None => "Error: VM not running. Start a VM first with vm_start.".to_string(),
                }
            } else {
                let path = args["path"].as_str().unwrap_or("");
                let old_text = args["old_text"]
                    .as_str()
                    .or_else(|| args["old_string"].as_str())
                    .unwrap_or("");
                let new_text = args["new_text"]
                    .as_str()
                    .or_else(|| args["new_string"].as_str())
                    .unwrap_or("");
                match crate::tools::edit_file::edit_file(path, old_text, new_text).await {
                    Ok(_) => format!("File edited: {}", path),
                    Err(e) => format!("Error: {}", e),
                }
            } // end else (shared mode)
        }
        "read_file" => {
            let vm_mode = std::env::var("VM_MODE").unwrap_or_else(|_| "shared".to_string());
            let vm_enabled = std::env::var("VM_ENABLED")
                .map(|v| v == "true" || v == "1")
                .unwrap_or(false);
            if vm_enabled && vm_mode == "vm" {
                let path = args["path"].as_str().unwrap_or("");
                match crate::tools::vm_tools::dispatch_vm_tool(
                    "vm_file_read",
                    &serde_json::json!({"path": path}),
                )
                .await
                {
                    Some(result) => result,
                    None => "Error: VM not running. Start a VM first with vm_start.".to_string(),
                }
            } else {
                let path = args["path"].as_str().unwrap_or("");
                crate::tools::read_file::run(path).await.unwrap_or_else(|e| format!("Error reading file: {e}"))
            } // end else (shared mode)
        }
        "get_context" => match db.load_context(user_id) {
            Ok(ctx) => serde_json::to_string_pretty(&ctx)
                .unwrap_or_else(|_| "Failed to serialize".to_string()),
            Err(e) => format!("Error: {}", e),
        },
        "set_context" => {
            let key = args["key"].as_str().unwrap_or("");
            let value = args
                .get("value")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            match db.merge_context_from_agent(user_id, serde_json::json!({key: value})) {
                Ok(_) => format!("Context key '{}' set", key),
                Err(e) => format!("Error: {}", e),
            }
        }
        "delete_context" => {
            let key = args["key"].as_str().unwrap_or("");
            match db.merge_context_from_agent(user_id, serde_json::json!({key: null})) {
                Ok(_) => format!("Context key '{}' deleted", key),
                Err(e) => format!("Error: {}", e),
            }
        }
        "agent_next" => crate::tools::agent_control::run(
            db,
            user_id,
            crate::tools::agent_control::AgentControlSignal::Next,
        )
        .await
        .unwrap_or_else(|e| format!("Error: {}", e)),
        "agent_complete" => crate::tools::agent_control::run(
            db,
            user_id,
            crate::tools::agent_control::AgentControlSignal::Complete,
        )
        .await
        .unwrap_or_else(|e| format!("Error: {}", e)),
        "agent_set_path" => {
            let path = args["path"].as_str().unwrap_or("");
            crate::tools::agent_control::run(
                db,
                user_id,
                crate::tools::agent_control::AgentControlSignal::Path(path.to_string()),
            )
            .await
            .unwrap_or_else(|e| format!("Error: {}", e))
        }
        "agent_feedback" => {
            let message = args["message"].as_str().unwrap_or("");
            crate::tools::agent_control::run(
                db,
                user_id,
                crate::tools::agent_control::AgentControlSignal::Feedback(message.to_string()),
            )
            .await
            .unwrap_or_else(|e| format!("Error: {}", e))
        }
        "discord_upload_file" => {
            let settings_upload_ch = db
                .load_context(user_id)
                .ok()
                .and_then(|c| c.settings.upload_channel_id.clone())
                .filter(|s| !s.is_empty());
            let fallback_ch = ctx_data
                .as_ref()
                .and_then(|c| c.get("channel_id"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or(settings_upload_ch)
                .unwrap_or_default();
            let channel_id = args["channel_id"]
                .as_str()
                .filter(|s| !s.is_empty())
                .unwrap_or(&fallback_ch);
            let filename = args["filename"].as_str().unwrap_or("file");
            let base64_content = args["base64_content"].as_str().unwrap_or("");
            // Decode base64 to temp file, then upload
            use base64::Engine;
            match base64::engine::general_purpose::STANDARD.decode(base64_content) {
                Ok(bytes) => {
                    let tmp_path = format!("/tmp/{}", filename);
                    if let Err(e) = std::fs::write(&tmp_path, &bytes) {
                        format!("Error writing temp file: {}", e)
                    } else {
                        match crate::tools::discord_upload::upload_file(
                            channel_id,
                            &tmp_path,
                            filename,
                            args.get("message").and_then(|v| v.as_str()),
                        )
                        .await
                        {
                            Ok(result) => {
                                let _ = std::fs::remove_file(&tmp_path);
                                result
                            }
                            Err(e) => {
                                let _ = std::fs::remove_file(&tmp_path);
                                format!("Error: {}", e)
                            }
                        }
                    }
                }
                Err(e) => format!("Error decoding base64: {}", e),
            }
        }
        "discord_send_message" => {
            let settings_upload_ch = db
                .load_context(user_id)
                .ok()
                .and_then(|c| c.settings.upload_channel_id.clone())
                .filter(|s| !s.is_empty());
            let fallback_ch = ctx_data
                .as_ref()
                .and_then(|c| c.get("channel_id"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or(settings_upload_ch)
                .unwrap_or_default();
            let channel_id = args["channel_id"]
                .as_str()
                .filter(|s| !s.is_empty())
                .unwrap_or(&fallback_ch);
            let message = args["message"].as_str().unwrap_or("");
            match crate::tools::discord_send_message::send_message(channel_id, message).await {
                Ok(_) => "Message sent".to_string(),
                Err(e) => format!("Error: {}", e),
            }
        }
        "discord_send_embed" => {
            let fallback_ch = ctx_data
                .as_ref()
                .and_then(|c| c.get("channel_id"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let channel_id = args["channel_id"]
                .as_str()
                .filter(|s| !s.is_empty())
                .unwrap_or(fallback_ch);
            let title = args.get("title").and_then(|v| v.as_str());
            let description = args.get("description").and_then(|v| v.as_str());
            let url = args.get("url").and_then(|v| v.as_str());
            let color = args
                .get("color")
                .and_then(|v| crate::tools::discord_send_embed::parse_color(v));
            let footer = args.get("footer").and_then(|v| v.as_str());
            let author = args.get("author").and_then(|v| v.as_str());
            let thumbnail = args.get("thumbnail").and_then(|v| v.as_str());
            let image = args.get("image").and_then(|v| v.as_str());
            let fields = args
                .get("fields")
                .map(|v| crate::tools::discord_send_embed::parse_fields(v))
                .unwrap_or_default();
            match crate::tools::discord_send_embed::send_embed(
                user_id,
                channel_id,
                title,
                description,
                url,
                color,
                footer,
                author,
                thumbnail,
                image,
                fields,
            )
            .await
            {
                Ok(_) => "Embed sent".to_string(),
                Err(e) => format!("Error: {}", e),
            }
        }
        "learn_fact" => {
            let fact = args["fact"].as_str().unwrap_or("");
            match crate::db::memory_profiles::learn_fact(db, user_id, fact) {
                Ok(_) => format!("Learned: {}", fact),
                Err(e) => format!("Error: {}", e),
            }
        }
        "learn_preference" => {
            let key = args["key"].as_str().unwrap_or("");
            let value = args.get("value").cloned().unwrap_or(serde_json::Value::Null);
            match crate::db::memory_profiles::update_memory(db, user_id, |memory| {
                crate::db::memory::update_preference(memory, key, &value);
            }) {
                Ok(_) => format!("Preference '{}' = '{}'", key, value),
                Err(e) => format!("Error: {}", e),
            }
        }
        "learn_topic" => {
            let topic = args["topic"].as_str().unwrap_or("");
            match crate::db::memory_profiles::learn_topic(db, user_id, topic) {
                Ok(_) => format!("Topic tracked: {}", topic),
                Err(e) => format!("Error: {}", e),
            }
        }
        "rag_search" => {
            let query = args["query"].as_str().unwrap_or("");
            let limit = args["limit"].as_u64().unwrap_or(5) as usize;
            match crate::tools::vector::vector_search(db, user_id, query, limit).await {
                Ok(results) => {
                    if results.is_empty() {
                        "No relevant documents found.".to_string()
                    } else {
                        let mut output = String::new();
                        for (i, chunk) in results.iter().enumerate() {
                            output.push_str(&format!(
                                "--- Result {} (similarity: {:.3}) ---\n{}\n\n",
                                i + 1,
                                chunk.similarity,
                                chunk.content
                            ));
                        }
                        output
                    }
                }
                Err(e) => format!("Error: {}", e),
            }
        }
        "rag_ingest" => {
            let filename = args["filename"].as_str().unwrap_or("untitled");
            let content = args["content"].as_str().unwrap_or("");
            let file_type = args["file_type"].as_str().unwrap_or("txt");
            match crate::tools::vector::ingest_with_embeddings(db, user_id, filename, content, file_type).await {
                Ok(doc_id) => format!("Document ingested successfully. ID: {}", doc_id),
                Err(e) => format!("Error: {}", e),
            }
        }
        "rag_list" => {
            match crate::tools::rag_ingest::list_documents(db, user_id).await {
                Ok(docs) => {
                    if docs.is_empty() {
                        "No documents in knowledge base.".to_string()
                    } else {
                        let mut output = String::from("Documents in knowledge base:\n");
                        for doc in &docs {
                            output.push_str(&format!(
                                "- {} ({}) - {} chunks - ID: {}\n",
                                doc["filename"], doc["file_type"], doc["chunk_count"], doc["id"]
                            ));
                        }
                        output
                    }
                }
                Err(e) => format!("Error: {}", e),
            }
        }
        "rag_delete" => {
            let document_id = args["document_id"].as_str().unwrap_or("");
            match crate::tools::rag_ingest::delete_document(db, user_id, document_id).await {
                Ok(_) => format!("Document {} deleted.", document_id),
                Err(e) => format!("Error: {}", e),
            }
        }
        "understand_image" => {
            let result = crate::tools::understand_image::run(&args).await;
            // Store image content_parts alongside the result
            // We return the text, but the caller needs to handle content_parts separately
            // Use a special marker to indicate this tool returned image data
            return serde_json::json!({
                "text": result.text,
                "content_parts": result.content_parts.iter().map(|cp| {
                    serde_json::to_value(cp).unwrap_or_default()
                }).collect::<Vec<_>>()
            })
            .to_string();
        }
        "send_screenshot" => {
            let fallback_ch = ctx_data
                .as_ref()
                .and_then(|c| c.get("channel_id"))
                .and_then(|v| v.as_str())
                .unwrap_or("web");
            let channel_id = args["channel_id"]
                .as_str()
                .filter(|s| !s.is_empty())
                .unwrap_or(fallback_ch);
            let caption = args["caption"].as_str();
            let vm_name = args["vm_name"].as_str().unwrap_or("praxis-vm");
            if channel_id == "web" || channel_id.is_empty() {
                match crate::tools::web_interactive::send_screenshot_to_web(
                    user_id, caption, vm_name,
                )
                .await
                {
                    Ok(result) => result,
                    Err(e) => format!("Error: {}", e),
                }
            } else {
                match crate::tools::discord_interactive::send_screenshot_to_discord(
                    channel_id, caption, vm_name,
                )
                .await
                {
                    Ok(result) => result,
                    Err(e) => format!("Error: {}", e),
                }
            }
        }
        "ask_questions" => {
            // Get timeout from args, then context, then default to 120
            let default_timeout = ctx_data
                .as_ref()
                .and_then(|c| c.get("question_timeout_secs"))
                .and_then(|v| v.as_u64())
                .unwrap_or(120);
            let timeout = args["timeout_secs"].as_u64().unwrap_or(default_timeout);

            // Detect if this is a web user (no Discord pairing)
            let paired_discord_user_id = db
                .get_pairing_by_internal_user(user_id)
                .ok()
                .flatten()
                .map(|p| p.discord_user_id)
                .unwrap_or_default();

            let fallback_ch = ctx_data
                .as_ref()
                .and_then(|c| c.get("channel_id"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let channel_id = args["channel_id"]
                .as_str()
                .filter(|s| !s.is_empty())
                .unwrap_or(fallback_ch);

            let is_web = channel_id == "web" || paired_discord_user_id.is_empty();

            let questions_raw = args["questions"].as_array();
            let Some(arr) = questions_raw else {
                return "Error: 'questions' field is required and must be an array.".to_string();
            };
            if arr.is_empty() {
                return "Error: 'questions' array must contain at least one question.".to_string();
            }
            let mut questions: Vec<(String, String, Vec<String>)> = Vec::new();
            for (i, q) in arr.iter().enumerate() {
                if q.get("options").is_some() {
                    return format!("Error: Question {}: use 'suggestions' with plain strings, not 'options' with objects.", i + 1);
                }
                let label = q["label"].as_str().filter(|s| !s.is_empty());
                let Some(label) = label else {
                    return format!("Error: Question {} is missing a non-empty 'label' field.", i + 1);
                };
                let text = q["question"].as_str().filter(|s| !s.is_empty());
                let Some(text) = text else {
                    return format!("Error: Question {} (label: '{}') is missing a non-empty 'question' field.", i + 1, label);
                };
                let suggestions: Vec<String> = q["suggestions"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                questions.push((label.to_string(), text.to_string(), suggestions));
            }
            tracing::info!(timeout_secs = timeout, is_web, "ask_questions: waiting for responses");

            if is_web {
                match crate::tools::web_interactive::ask_questions_web(
                    user_id,
                    &questions,
                    timeout,
                )
                .await
                {
                    Ok(result) => result,
                    Err(e) => format!("Error: {}", e),
                }
            } else {
                if channel_id.is_empty() {
                    return "Error: No channel_id provided and no originating channel found. Please specify a channel_id.".to_string();
                }
                if paired_discord_user_id.is_empty() {
                    return "Error: No Discord user pairing found.".to_string();
                }
                match crate::tools::discord_interactive::ask_questions(
                    channel_id,
                    &questions,
                    timeout,
                    &paired_discord_user_id,
                )
                .await
                {
                    Ok(result) => result,
                    Err(e) => format!("Error: {}", e),
                }
            }
        }
        "cron_add" => {
            let name = args["name"].as_str().unwrap_or("");
            let schedule = args["schedule"].as_str().unwrap_or("");
            let prompt = args["prompt"].as_str().unwrap_or("");
            let template = args["template"].as_str().unwrap_or("agent.poml");
            let timezone = args["timezone"].as_str().unwrap_or("UTC");
            let description = args["description"].as_str().map(|s| s.to_string());
            let enabled = args["enabled"].as_bool().unwrap_or(true);

            if name.is_empty() || schedule.is_empty() || prompt.is_empty() {
                return "Error: name, schedule, and prompt are required.".to_string();
            }

            if !crate::gateway::cron_scheduler::CronScheduler::validate_schedule(schedule) {
                return format!("Error: Invalid cron expression '{}'. Use 6 fields: sec min hour day month weekday. Example: '0 0 9 * * *'", schedule);
            }

            let job_id = uuid::Uuid::new_v4().to_string()[..8].to_string();

            let job = crate::db::cron_jobs::CronJob {
                id: job_id.clone(),
                name: name.to_string(),
                description,
                schedule: schedule.to_string(),
                timezone: timezone.to_string(),
                user_id: user_id.to_string(),
                channel_id: None,
                template: template.to_string(),
                prompt: prompt.to_string(),
                context_overrides: None,
                enabled,
                trigger_type: "cron".to_string(),
                webhook_secret: None,
                event_type: None,
                last_run: None,
                next_run: None,
                run_count: 0,
                last_error: None,
            };

            match db.create_cron_job(&job) {
                Ok(_) => format!("Cron job created. ID: {} Name: '{}' Schedule: '{}'", job_id, name, schedule),
                Err(e) => format!("Error creating cron job: {}", e),
            }
        }
        "cron_delete" => {
            let job_id = args["job_id"].as_str().unwrap_or("");
            if job_id.is_empty() {
                return "Error: job_id is required.".to_string();
            }
            match db.delete_cron_job(job_id) {
                Ok(_) => format!("Cron job {} deleted.", job_id),
                Err(e) => format!("Error deleting cron job: {}", e),
            }
        }
        "cron_list" => {
            match db.list_cron_jobs(user_id) {
                Ok(jobs) => {
                    if jobs.is_empty() {
                        "No cron jobs found.".to_string()
                    } else {
                        let mut output = String::from("Cron jobs:\n");
                        for job in &jobs {
                            let status = if job.enabled { "enabled" } else { "disabled" };
                            let runs = format!("runs: {}", job.run_count);
                            let last = job.last_run.as_deref().unwrap_or("never");
                            let err = job.last_error.as_deref().unwrap_or("");
                            output.push_str(&format!(
                                "- [{}] {} ({}) {} schedule='{}' last={} {} prompt='{}'\n",
                                job.id, job.name, status, runs, job.schedule, last,
                                if err.is_empty() { String::new() } else { format!("error='{}'", err) },
                                job.prompt
                            ));
                        }
                        output
                    }
                }
                Err(e) => format!("Error listing cron jobs: {}", e),
            }
        }
        "cron_toggle" => {
            let job_id = args["job_id"].as_str().unwrap_or("");
            let enabled = args["enabled"].as_bool().unwrap_or(true);
            if job_id.is_empty() {
                return "Error: job_id is required.".to_string();
            }
            match db.toggle_cron_job(job_id, enabled) {
                Ok(_) => format!("Cron job {} {}.", job_id, if enabled { "enabled" } else { "disabled" }),
                Err(e) => format!("Error toggling cron job: {}", e),
            }
        }
        "cron_run" => {
            let job_id = args["job_id"].as_str().unwrap_or("");
            if job_id.is_empty() {
                return "Error: job_id is required.".to_string();
            }
            match db.get_cron_job(job_id) {
                Ok(Some(job)) => {
                    format!(
                        "Cron job '{}' triggered manually. ID: {} Prompt: '{}' Template: {}",
                        job.name, job.id, job.prompt, job.template
                    )
                }
                Ok(None) => format!("Error: Cron job {} not found.", job_id),
                Err(e) => format!("Error fetching cron job: {}", e),
            }
        }
        "update_template" => crate::tools::update_template::run(db, &args).await
            .unwrap_or_else(|e| format!("Error: {}", e)),
        _ => {
            // Check VM tools first
            if tc.function.name.starts_with("vm_") {
                if let Some(result) =
                    crate::tools::vm_tools::dispatch_vm_tool(&tc.function.name, &args).await
                {
                    return result;
                }
            }
            match plugins
                .execute_tool_for_task(
                    user_id,
                    &tc.id,
                    &tc.function.name,
                    &args,
                    ctx_data.as_ref(),
                    Some(&plugin_secrets),
                )
                .await
            {
                Ok(result) => result,
                Err(e) => format!("Plugin tool {} failed: {}", tc.function.name, e),
            }
        }
    }
}

/// Validate tool parameters against the tool definition
pub fn validate_tool_params(
    tool_name: &str,
    args: &serde_json::Value,
    tool_defs: &[crate::gateway::llm::provider::ToolDefinition],
) -> Result<(), String> {
    let args = crate::tools::tool_output::execution_args(args).map_err(|e| e.to_string())?;
    let tool_def = tool_defs.iter().find(|t| t.function.name == tool_name);
    if let Some(def) = tool_def {
        let params = &def.function.parameters;
        if let Some(required) = params.get("required").and_then(|r| r.as_array()) {
            let mut missing = Vec::new();
            for req in required {
                if let Some(key) = req.as_str() {
                    if args.get(key).is_none() || args.get(key).unwrap().is_null() {
                        missing.push(key.to_string());
                    }
                }
            }
            if !missing.is_empty() {
                return Err(format!(
                    "Missing required parameters: {}. Provide these in the arguments.",
                    missing.join(", ")
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "skill_dispatch_tests.rs"]
mod skill_dispatch_tests;

#[cfg(test)]
#[path = "backend_dispatch_tests.rs"]
mod backend_dispatch_tests;

#[cfg(test)]
mod agent_tests {
    use super::*;

    #[test]
    fn test_agent_loop_config_default() {
        let config = AgentLoopConfig::default();
        assert_eq!(config.max_turns, 10);
        assert_eq!(config.max_tool_calls, 5);
        assert!(config.tags_enabled);
        assert!(config.sm_file.is_none());
        assert!(!config.feedback_enabled);
    }

    #[test]
    fn test_get_tool_definitions() {
        // Tool definitions now come from the database
        // This test verifies the structure is compatible
        let dir = tempfile::TempDir::new().unwrap();
        let db = crate::db::Database::new(dir.path()).unwrap();
        crate::db::tools::init_default_tools(&db).unwrap();
        let tools = crate::db::tools::to_tool_definitions(&db).unwrap();
        assert!(!tools.is_empty());
        let names: Vec<&str> = tools.iter().map(|t| t.function.name.as_str()).collect();
        assert!(names.contains(&"execute_terminal"));
        assert!(names.contains(&"write_file"));
        assert!(names.contains(&"edit_file"));
        assert!(names.contains(&"read_file"));
    }

    #[test]
    fn test_agent_loop_result_debug() {
        let result = AgentLoopResult {
            response: "test".to_string(),
            response_message_id: None,
            turns_used: 1,
            tag_execution: None,
            completed: false,
            advanced: false,
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
            generation_ms: 0,
        };
        let debug = format!("{:?}", result);
        assert!(debug.contains("test"));
        assert!(debug.contains("turns_used: 1"));
    }

    #[test]
    fn test_tool_history_recording() {
        // Test that tool_history is properly structured in custom_data
        let mut custom_data = serde_json::json!({});

        // Simulate adding a tool call to history
        if let Some(obj) = custom_data.as_object_mut() {
            if !obj.contains_key("tool_history") {
                obj.insert("tool_history".to_string(), serde_json::json!([]));
            }
            if let Some(history) = obj.get_mut("tool_history").and_then(|v| v.as_array_mut()) {
                history.push(serde_json::json!({
                    "iteration": 1,
                    "tool": "execute_terminal",
                    "args": {"command": "ls -la"},
                    "result": "total 248...",
                    "timestamp": "2026-05-06 10:30:00",
                    "duration_ms": 150
                }));
            }
        }

        // Verify structure
        let history = custom_data["tool_history"].as_array().unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0]["iteration"], 1);
        assert_eq!(history[0]["tool"], "execute_terminal");
        assert_eq!(history[0]["args"]["command"], "ls -la");
        assert!(history[0]["timestamp"].as_str().is_some());
        assert!(history[0]["duration_ms"].as_i64().is_some());
    }

    #[test]
    fn test_tool_history_max_entries() {
        // Test that tool_history is limited to 50 entries
        let mut custom_data = serde_json::json!({});
        let obj = custom_data.as_object_mut().unwrap();
        obj.insert("tool_history".to_string(), serde_json::json!([]));

        // Add 55 entries
        let history = obj.get_mut("tool_history").unwrap().as_array_mut().unwrap();
        for i in 0..55 {
            history.push(serde_json::json!({
                "iteration": i,
                "tool": "test_tool",
                "args": {},
                "result": "ok",
                "timestamp": "2026-05-06 10:30:00",
                "duration_ms": 100
            }));
        }

        // Simulate trimming (as done in agent_loop)
        if history.len() > 50 {
            let drain_count = history.len() - 50;
            history.drain(0..drain_count);
        }

        assert_eq!(history.len(), 50);
        // Verify oldest entries were removed
        assert_eq!(history[0]["iteration"], 5);
        assert_eq!(history[49]["iteration"], 54);
    }

    #[test]
    fn test_used_tools_object() {
        // Test that used_tools is properly structured
        let mut custom_data = serde_json::json!({});
        let obj = custom_data.as_object_mut().unwrap();
        obj.insert("tool_history".to_string(), serde_json::json!([]));

        let history = obj.get_mut("tool_history").unwrap().as_array_mut().unwrap();
        history.push(serde_json::json!({
            "iteration": 1,
            "tool": "execute_terminal",
            "args": {"command": "ls -la"},
            "result": "total 248...",
            "timestamp": "2026-05-06 10:30:00",
            "duration_ms": 150
        }));

        let history_arr = obj.get("tool_history").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        obj.insert("used_tools".to_string(), serde_json::json!({
            "last_call": "execute_terminal",
            "last_result": "total 248...",
            "last_args": {"command": "ls -la"},
            "count": 1,
            "history": history_arr
        }));

        // Verify used_tools structure
        let used = &custom_data["used_tools"];
        assert_eq!(used["last_call"], "execute_terminal");
        assert_eq!(used["last_result"], "total 248...");
        assert_eq!(used["last_args"]["command"], "ls -la");
        assert_eq!(used["count"], 1);
        let hist = used["history"].as_array().unwrap();
        assert_eq!(hist.len(), 1);
        assert_eq!(hist[0]["tool"], "execute_terminal");
    }

    #[test]
    fn test_tool_stream_event_payloads() {
        // tool_call payload: tool name + call id + safe args preview.
        let call_payload = serde_json::json!({
            "tool": "write_file",
            "call_id": "call_1",
            "args_preview": {"path": "/tmp/x.txt", "content_bytes": 42},
        });
        assert_eq!(call_payload["tool"], "write_file");
        assert_eq!(call_payload["args_preview"]["path"], "/tmp/x.txt");
        assert!(call_payload["args_preview"]["content_bytes"].is_u64());
        // The preview must NOT contain the file content itself.
        assert!(call_payload["args_preview"].get("content").is_none());

        // tool_result payload: tool name + duration + truncated result.
        let result_payload = serde_json::json!({
            "tool": "execute_terminal",
            "call_id": "call_2",
            "duration_ms": 150,
            "result": "total 248...",
        });
        assert_eq!(result_payload["tool"], "execute_terminal");
        assert_eq!(result_payload["duration_ms"], 150);
        assert_eq!(result_payload["result"], "total 248...");

        // understand_image results stream text only (image goes as chat_image).
        let image_tool_result = serde_json::json!({
            "text": "Image loaded: /tmp/x.png. describe",
            "content_parts": [{"image_url": {"url": "data:image/png;base64,AAAA"}}],
        });
        let text_only = image_tool_result.get("text").and_then(|t| t.as_str()).unwrap_or_default();
        assert!(text_only.contains("Image loaded"));
        assert!(image_tool_result.get("content_parts").is_some());
    }

    #[test]
    fn test_chat_image_event_extracts_data_url() {
        // understand_image returns content_parts with a data URL; the chat
        // event must carry that URL so the browser can render it inline.
        let data_url = "data:image/png;base64,AAAA";
        let part = serde_json::json!({"image_url": {"url": data_url, "detail": "high"}});
        let extracted = part
            .get("image_url")
            .and_then(|iu| iu.get("url"))
            .and_then(|u| u.as_str())
            .unwrap_or("");
        assert_eq!(extracted, data_url);
        assert!(extracted.starts_with("data:image/"));
    }
}

include!("capability_dispatch_tests.rs");
