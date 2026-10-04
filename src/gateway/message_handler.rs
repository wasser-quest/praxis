use crate::gateway::llm::provider::{ChatMessage, ChatRequest};
use crate::gateway::GatewayState;
use crate::tools::registry::build_tool_definitions_for_user;
use crate::voice::tts;

pub async fn handle_message(
    state: &GatewayState,
    user_id: &str,
    content: &str,
    channel_id: Option<&str>,
) -> anyhow::Result<String> {
    let _task = crate::gateway::task_control::begin(user_id)?;
    handle_message_inner(state, user_id, content, channel_id).await
}

pub(crate) async fn handle_message_inner(
    state: &GatewayState,
    user_id: &str,
    content: &str,
    channel_id: Option<&str>,
) -> anyhow::Result<String> {
    // A prior agent_complete must not stop this independent task after one tool.
    crate::gateway::prompt::reset_task_completion_in(&state.db, std::path::Path::new(&state.config.root_dir), user_id)?;
    // Route before deciding the path and before either prompt is rendered.
    let ctx = crate::gateway::prompt::prepare_runtime(state, user_id, content, None, channel_id)?;

    // GPU-Router-Prewarm (pgpu §12.4): LLM-Slot VOR dem Turn wecken (await —
    //Wake + State-Check sind <1 s). Nachts (20:00-Sleep = auto_rent aus)
    // ist force-wake der einzige Weg hoch; X-Router-Wait allein würde sofort
    // mit 503 auto_rent_off abgewiesen. Danach hält X-Router-Wait die LLM-
    // Anfrage selbst bis healthy (Restart ~1-2 min) — die Nachricht WARTET
    // also auf die geladene Box und wird danach beantwortet.
    if crate::gpu_router::configured() {
        let _ = crate::gpu_router::ensure_awake(crate::gpu_router::SLOT_LLM).await;
    }
    // Media-Slot früh wecken — die Box wärmt, während das LLM noch
    // generiert, statt beim Sprechen auf einen kalten Slot (503) zu laufen.
    // Nur wenn die Antwort in DIESEM Kanal wirklich gesprochen wird — sonst
    // mietet ein Web-Chat ohne web_chat_tts eine Media-Box für nichts.
    // No-op ohne GPU_ROUTER_URL; ensure_awake dedupliziert (30 s).
    if reply_tts_enabled(&ctx.settings, channel_id)
        && matches!(
            ctx.settings.voice_tts_type.as_str(),
            "comfyui_qwen3" | "comfyui_xtts"
        )
    {
        crate::gpu_router::ensure_awake_background(crate::gpu_router::SLOT_MEDIA);
    }

    let max_turns = ctx.settings.max_llm_turns.unwrap_or(1);

    // Use agent loop when max_turns > 1
    if max_turns > 1 {
        return handle_message_agent_loop(state, user_id, content, channel_id, &ctx, max_turns)
            .await;
    }

    // One user-facing turn can require several tool-only LLM responses. Keep
    // this chat path bounded without mistaking a tool response for completion.
    // Snapshot the budget: a tool cannot grow its own budget during this task.
    let tool_limit = ctx.settings.max_tool_calls.unwrap_or(5).clamp(0, 128) as usize;
    let mut tool_calls_used = 0usize;
    let mut current_tool_ids = std::collections::HashSet::new();
    let mut finalizing = tool_limit == 0;
    // Persist raw input, then route BEFORE either current-request template.
    let input_message_id = state.db.add_message(user_id, &crate::db::messages::Message::user(content.to_string()))?;
    super::action_contracts::record_user_message(user_id, input_message_id)?;
    crate::gateway::compaction::before_request(state, user_id).await?;
    let mut ctx = crate::gateway::decision_routing::prepare(state, user_id, content, None, channel_id).await?;
    let rendered_user = crate::gateway::prompt::render_user(state, &ctx, content).await?;
    let system_prompt = crate::gateway::prompt::render_system(state, &ctx, content).await?;

    let mut messages = Vec::new();
    messages.push(ChatMessage {
        reasoning_content: None,
role: "system".to_string(),
        content: Some(system_prompt),
        content_parts: None,
        tool_calls: None,
        tool_call_id: None,
        tool_name: None,
    });

    let token_budget = ctx.settings.history_token_limit.unwrap_or(crate::db::messages::DEFAULT_HISTORY_TOKENS);
    let (history, _tokens) = state
        .db
        .get_messages_with_token_budget(user_id, token_budget)?;
    for msg in &history {
        let tool_calls = msg.tool_calls.as_ref().map(|tcs| {
            tcs.iter()
                .map(|tc| crate::gateway::llm::provider::ToolCall {
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
            content: if msg.content.is_empty() {
                None
            } else {
                Some(msg.content.clone())
            },
            content_parts: None,
            tool_calls,
            tool_call_id: msg.tool_call_id.clone(),
            tool_name: msg.tool_name.clone(),
        });
    }

    // Render only the CURRENT user turn; stored history stays raw.
    if let Some(last) = messages.last_mut() {
        if last.role == "user" {
            last.content = Some(rendered_user);
        }
    }

    let tool_defs = build_tool_definitions_for_user(&ctx.settings, Some(&state.plugins.tool_definitions()), Some(&state.db), user_id);

    // Clone tool names and definitions for validation before moving tool_defs into request
    let mut tool_names: Vec<String> = tool_defs.iter().map(|t| t.function.name.clone()).collect();
    let mut tools_for_validation = tool_defs.clone();

    let request = ChatRequest {
        messages,
        tools: if finalizing || tool_defs.is_empty() {
            None
        } else {
            Some(tool_defs)
        },
        temperature: Some(0.7),
        max_tokens: Some(state.llm.get().task_output_tokens()),
        model: ctx.settings.model.clone(),
        vision_provider: ctx.settings.vision_provider.clone().or_else(|| state.config.vision_provider.clone()),
        vision_model: ctx.settings.vision_model.clone().or_else(|| state.config.vision_model.clone()),
        thinking: crate::gateway::llm::provider::ThinkingMode::from_setting(&ctx.settings.thinking_mode),
    };

    let provider = if ctx.settings.use_freerouter {
        Some("free_router")
    } else {
        ctx.settings.provider.as_deref()
    };

    let mut response = super::telemetry::chat(state, &ctx, request, provider)
        .await?;

    // Track cumulative usage across tool-call continuations.
    let mut cumulative_prompt_tokens: u32 = 0;
    let mut cumulative_completion_tokens: u32 = 0;
    let mut cumulative_total_tokens: u32 = 0;
    if let Some(ref usage) = response.usage {
        cumulative_prompt_tokens = cumulative_prompt_tokens.saturating_add(usage.prompt_tokens);
        cumulative_completion_tokens = cumulative_completion_tokens.saturating_add(usage.completion_tokens);
        cumulative_total_tokens = cumulative_total_tokens.saturating_add(usage.total_tokens);
    }

    while let Some(tool_calls) = &response.tool_calls {
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

        let mut results = Vec::new();
        for tc in tool_calls {
            if crate::gateway::task_control::cancellation(user_id).is_some_and(|token| token.is_cancelled()) {
                let mut msg = crate::db::messages::Message::tool("Error: Task cancelled; tool not executed".into(), tc.id.clone());
                msg.tool_name = Some(tc.function.name.clone());
                state.db.add_message(user_id, &msg)?;
                continue;
            }
            if tool_calls_used >= tool_limit {
                let mut msg = crate::db::messages::Message::tool(
                    "Error: Maximum tool-call limit reached; tool not executed".into(),
                    tc.id.clone(),
                );
                msg.tool_name = Some(tc.function.name.clone());
                state.db.add_message(user_id, &msg)?;
                continue;
            }
            // Invalid calls also consume budget; otherwise malformed/unknown
            // calls could keep a recovery loop running indefinitely.
            tool_calls_used += 1;
            // Validate tool exists before executing
            if !tool_names.contains(&tc.function.name)
                || !crate::tools::discovery::enabled(&state.db, &state.plugins, &tc.function.name)
            {
                let result = format!("Error: Unknown tool '{}'. Check the tool name and try again.", tc.function.name);
                tracing::warn!(user_id = %user_id, tool = %tc.function.name, "Unknown tool called");
                
                // Add error as tool result so LLM can recover
                let msg = crate::db::messages::Message::tool(result.clone(), tc.id.clone());
                let mut msg = msg;
                msg.tool_name = Some(tc.function.name.clone());
                state.db.add_message(user_id, &msg)?;
                results.push((tc.id.clone(), result));
                continue;
            }

            // Validate required parameters
            let args: serde_json::Value = serde_json::from_str(&tc.function.arguments).unwrap_or_default();
            if let Err(e) = crate::gateway::agent_loop::validate_tool_params(&tc.function.name, &args, &tools_for_validation) {
                let result = format!("Error: {}", e);
                tracing::warn!(user_id = %user_id, tool = %tc.function.name, error = %e, "Invalid tool parameters");
                
                let msg = crate::db::messages::Message::tool(result.clone(), tc.id.clone());
                let mut msg = msg;
                msg.tool_name = Some(tc.function.name.clone());
                state.db.add_message(user_id, &msg)?;
                results.push((tc.id.clone(), result));
                continue;
            }

            let result = execute_tool_call_in(std::path::Path::new(&state.config.root_dir), &state.db, user_id, tc, &state.plugins).await;
            let mut image_content_parts: Option<Vec<serde_json::Value>> = None;
            let mut final_result = result.clone();

            // Check if understand_image returned image data
            if tc.function.name == "understand_image" {
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&result) {
                    if let Some(parts) = parsed.get("content_parts").and_then(|v| v.as_array()) {
                        if !parts.is_empty() {
                            final_result = parsed
                                .get("text")
                                .and_then(|v| v.as_str())
                                .unwrap_or(&result)
                                .to_string();
                            image_content_parts = Some(parts.clone());
                        }
                    }
                }
            }

            let stored_result = crate::gateway::tool_results::prepare_for_call(&state.db, user_id, tc, &final_result)?;
            let msg = if let Some(parts) = image_content_parts {
                crate::db::messages::Message::tool_with_image(
                    stored_result.clone(),
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
            results.push((tc.id.clone(), final_result));
        }

        if crate::gateway::task_control::cancellation(user_id).is_some_and(|token| token.is_cancelled()) {
            return Err(crate::gateway::llm::error::ProviderError::new(
                crate::gateway::llm::error::ErrorKind::Cancelled,
            ).into());
        }
        if finalizing {
            anyhow::bail!("Maximum tool-call limit reached ({tool_limit}); provider requested more tools instead of a final answer. Tool results are saved; the task was not silently completed.");
        }

        // Context tools must affect the very next LLM request, not be overwritten
        // by the pre-tool snapshot at the end of this handler.
        crate::gateway::compaction::before_request(state, user_id).await?;
        ctx = crate::gateway::decision_routing::prepare(state, user_id, content, None, channel_id).await?;
        let mut followup_messages = Vec::new();
        followup_messages.push(ChatMessage {
            reasoning_content: None,
role: "system".to_string(),
            content: Some(crate::gateway::prompt::render_system(state, &ctx, content).await?),
            content_parts: None,
            tool_calls: None,
            tool_call_id: None,
            tool_name: None,
        });
        let (history, _tokens) = state
            .db
            .get_messages_with_token_budget(user_id, ctx.settings.history_token_limit.unwrap_or(crate::db::messages::DEFAULT_HISTORY_TOKENS))?;

        let image_msg_indices = crate::gateway::prompt::history_image_indices(
            &history, &current_tool_ids, state.llm.get().history_image_messages(),
        );
        if history.iter().enumerate().any(|(i, m)|
            m.content_parts.as_ref().is_some_and(|p| !p.is_empty()) && !image_msg_indices.contains(&i)
        ) {
            followup_messages.push(ChatMessage {
    reasoning_content: None,
                role: "system".into(),
                content: Some(crate::gateway::prompt::OMITTED_HISTORY_IMAGES_NOTICE.into()),
                content_parts: None, tool_calls: None, tool_call_id: None, tool_name: None,
            });
        }

        for (msg_idx, msg) in history.iter().enumerate() {
            let tool_calls = msg.tool_calls.as_ref().map(|tcs| {
                tcs.iter()
                    .map(|tc| crate::gateway::llm::provider::ToolCall {
                        id: tc.id.clone(),
                        function: crate::gateway::llm::provider::FunctionCall {
                            name: tc.function.name.clone(),
                            arguments: tc.function.arguments.clone(),
                        },
                    })
                    .collect()
            });
            followup_messages.push(ChatMessage {
    reasoning_content: None,
                role: msg.role.clone(),
                content: if msg.content.is_empty() {
                    None
                } else {
                    Some(msg.content.clone())
                },
                content_parts: if image_msg_indices.contains(&msg_idx) {
                    msg.content_parts.as_ref().map(|parts| {
                        parts
                            .iter()
                            .filter_map(|v| serde_json::from_value(v.clone()).ok())
                            .collect()
                    })
                } else {
                    None
                },
                tool_calls,
                tool_call_id: msg.tool_call_id.clone(),
                tool_name: msg.tool_name.clone(),
            });
        }

        // Keep tools available after use_skill/read_file/etc. Refresh definitions
        // as well as context, so newly disabled tools cannot run next round.
        let tool_defs = build_tool_definitions_for_user(&ctx.settings, Some(&state.plugins.tool_definitions()), Some(&state.db), user_id);
        tool_names = tool_defs.iter().map(|t| t.function.name.clone()).collect();
        tools_for_validation = tool_defs.clone();
        finalizing = tool_calls_used >= tool_limit;
        if finalizing {
            followup_messages.push(ChatMessage {
    reasoning_content: None,
                role: "system".into(),
                content: Some("The tool-call budget for this request is exhausted. No more tools may run. Give an honest final status using the saved tool results, explicitly stating any unfinished work. Do not claim unexecuted actions succeeded.".into()),
                content_parts: None,
                tool_calls: None,
                tool_call_id: None,
                tool_name: None,
            });
        }
        let followup_request = ChatRequest {
            messages: followup_messages,
            tools: if finalizing || tool_defs.is_empty() { None } else { Some(tool_defs) },
            temperature: Some(0.7),
            max_tokens: Some(state.llm.get().task_output_tokens()),
            model: ctx.settings.model.clone(),
            vision_provider: ctx.settings.vision_provider.clone().or_else(|| state.config.vision_provider.clone()),
            vision_model: ctx.settings.vision_model.clone().or_else(|| state.config.vision_model.clone()),
            thinking: crate::gateway::llm::provider::ThinkingMode::from_setting(&ctx.settings.thinking_mode),
        };

        response = super::telemetry::chat(state, &ctx, followup_request, ctx.settings.provider.as_deref())
            .await?;
        if let Some(ref usage) = response.usage {
            cumulative_prompt_tokens = cumulative_prompt_tokens.saturating_add(usage.prompt_tokens);
            cumulative_completion_tokens = cumulative_completion_tokens.saturating_add(usage.completion_tokens);
            cumulative_total_tokens = cumulative_total_tokens.saturating_add(usage.total_tokens);
        }
    }

    let mut reply = response.content.unwrap_or_default();
    if let Err(error) = super::action_contracts::require(user_id, "_complete") {
        reply.push_str(&format!("\n\nTask incomplete — runtime verification required: {error}"));
    }

    let mut msg = crate::db::messages::Message::assistant(reply.clone());
    if cumulative_total_tokens > 0 {
        msg.prompt_tokens = Some(cumulative_prompt_tokens);
        msg.completion_tokens = Some(cumulative_completion_tokens);
        msg.total_tokens = Some(cumulative_total_tokens);
    }
    let message_id = state.db.add_message(user_id, &msg)?;
    crate::dashboard::stream::assistant_saved(user_id, message_id, &reply);
    // Emit usage event for TUI/dashboard token display.
    if cumulative_total_tokens > 0 {
        let usage_data = serde_json::json!({
            "prompt_tokens": cumulative_prompt_tokens,
            "completion_tokens": cumulative_completion_tokens,
            "total_tokens": cumulative_total_tokens,
        });
        crate::dashboard::stream::send(user_id, "usage", &usage_data.to_string());
    }

    let mut updated_ctx = state.db.load_context(user_id)?;
    state.db.increment_turn(&mut updated_ctx);
    state.db.save_context(&updated_ctx)?;

    if reply_tts_enabled(&updated_ctx.settings, channel_id) {
        spawn_tts(
            reply.clone(),
            &updated_ctx.settings,
            &state.secrets,
            user_id,
            &state.db,
            Some(message_id),
            channel_id,
        );
    }

    Ok(reply)
}

fn stream_response(user_id: &str, reply: &str) {
    // No-op: real streaming is handled by streaming_chat in LLMRouter
    // This function is kept for backward compatibility
    _ = user_id;
    _ = reply;
}

async fn handle_message_agent_loop(
    state: &GatewayState,
    user_id: &str,
    content: &str,
    channel_id: Option<&str>,
    ctx: &crate::db::contexts::Context,
    max_turns: i32,
) -> anyhow::Result<String> {
    let user_id_owned = user_id.to_string();
    let secrets = state.secrets.clone();

    // Inject channel_id into custom_data so tools can use it.
    // Always overwrite with the CURRENT channel so replies/tools follow the user
    // (previously it was written once and never updated, so later messages from
    // other channels kept the stale value and outputs landed in random channels).
    // Voice input keeps the last real text channel for text tools and records the
    // voice channel separately under voice_channel_id.
    let mut ctx = ctx.clone();
    if ctx.custom_data.is_null() {
        ctx.custom_data = serde_json::json!({});
    }
    if let Some(obj) = ctx.custom_data.as_object_mut() {
        let is_voice = channel_id.map_or(false, |ch| ch.starts_with("voice:"));
        let text_channel: Option<String> = if is_voice {
            // STT/voice input: text tools should target the last text channel the
            // user actually wrote in; fall back to a non-voice feedback channel.
            obj.get("user_channel_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| {
                    ctx.settings
                        .feedback_channel_id
                        .clone()
                        .filter(|s| !s.starts_with("voice:"))
                })
        } else {
            channel_id
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .or_else(|| {
                    ctx.settings
                        .feedback_channel_id
                        .clone()
                        .filter(|s| !s.starts_with("voice:"))
                })
        };
        if let Some(ch) = text_channel {
            if !ch.is_empty() {
                obj.insert("user_channel_id".to_string(), serde_json::json!(ch));
                obj.insert("channel_id".to_string(), serde_json::json!(ch));
            }
        }
        if is_voice {
            if let Some(ch) = channel_id {
                obj.insert("voice_channel_id".to_string(), serde_json::json!(ch));
            }
        }
        // STT language confidence for the transcript-check rule (see voice::last_stt_confidence)
        if let Some(c) = crate::voice::last_stt_confidence() {
            let threshold = ctx.settings.stt_low_confidence_threshold;
            obj.insert("stt_confidence".to_string(), serde_json::json!(c));
            if c < threshold {
                obj.insert("stt_low_confidence".to_string(), serde_json::json!(true));
            } else {
                obj.remove("stt_low_confidence");
            }
        }
    }
    state.db.save_context(&ctx)?;

    let config = crate::gateway::agent_loop::AgentLoopConfig {
        max_turns,
        max_tool_calls: ctx.settings.max_tool_calls.unwrap_or(5),
        tags_enabled: ctx.settings.tags_enabled,
        sm_file: ctx.settings.sm_file.clone().or(ctx.sm_file.clone()),
        feedback_enabled: ctx.settings.feedback_enabled,
        message_on_toolcalling: ctx.settings.message_on_toolcalling,
        tool_history_limit: ctx.settings.tool_history_limit,
    };

    let (feedback_tx, mut feedback_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    // Spawn feedback routing task (always for web stream)
    let is_web = channel_id.map_or(true, |ch| ch == "web" || ch.is_empty());
    let uid = user_id_owned.clone();
    let session_id = ctx.session_id.clone();
    let feedback_key = format!("{uid}:::{session_id}");
    let tts_secrets = secrets;
    let tts_db = state.db.clone();
    let tts_channel_id = channel_id.map(str::to_owned);

    tokio::spawn(async move {
        while let Some(msg) = feedback_rx.recv().await {
            if is_web {
                crate::dashboard::stream::send(&uid, "feedback", &msg);
            }
            // A context command/tool can disable speech during the agent loop.
            let Ok(current_ctx) = tts_db.load_context(&uid) else { continue };
            if current_ctx.session_id != session_id { continue; }
            let tts_settings = &current_ctx.settings;
            if !super::feedback::LIMITER.allow(&feedback_key, tts_settings, std::time::Instant::now()) { continue; }
            let ch = tts_settings.feedback_channel_id.as_ref().or(tts_channel_id.as_ref());
            let tts_for_feedback = tts_settings.use_tts
                || tts_channel_id.as_deref().is_some_and(|ch| ch.starts_with("voice:"));
            let mut handled = false;
            for mode in &tts_settings.feedback_mode {
                match mode.as_str() {
                    "tts" => {
                        // Explicit feedback TTS may override Discord use_tts,
                        // but spawn_tts never overrides the web OFF setting.
                        spawn_tts(msg.clone(), tts_settings, &tts_secrets, &uid, &tts_db, None, tts_channel_id.as_deref());
                        handled = true;
                    }
                    "dm" => {
                        crate::event_channel::broadcast_agent_feedback(&uid, &msg);
                        crate::dashboard::stream::send(&uid, "feedback", &msg);
                        handled = true;
                    }
                    "text" => {
                        let sent_to_channel = ch.as_ref().map_or(false, |ch_id| {
                            if ch_id.parse::<u64>().ok().filter(|&id| id > 0).is_some() {
                                crate::event_channel::broadcast_channel_message(
                                    &uid, ch_id, &msg,
                                );
                                true
                            } else {
                                false
                            }
                        });
                        if !sent_to_channel {
                            crate::event_channel::broadcast_agent_feedback(&uid, &msg);
                        }
                        crate::dashboard::stream::send(&uid, "feedback", &msg);
                        handled = true;
                    }
                    _ => {}
                }
            }
            if !handled && tts_for_feedback {
                spawn_tts(msg.clone(), tts_settings, &tts_secrets, &uid, &tts_db, None, tts_channel_id.as_deref());
            }
        }
    });

    let result = crate::gateway::agent_loop::run_agent_loop_in_task(
        state,
        user_id,
        content,
        config,
        Some(feedback_tx),
    )
    .await?;

    let reply = result.response.clone();

    let mut updated_ctx = state.db.load_context(user_id)?;
    state.db.increment_turn(&mut updated_ctx);
    state.db.save_context(&updated_ctx)?;

    // Web speech has its own permission; Discord use_tts cannot enable it.
    if reply_tts_enabled(&updated_ctx.settings, channel_id) {
        spawn_tts(
            reply.clone(),
            &updated_ctx.settings,
            &state.secrets,
            user_id,
            &state.db,
            result.response_message_id,
            channel_id,
        );
    }

    Ok(reply)
}

#[cfg(test)]
async fn execute_tool_call(db: &crate::db::Database, user_id: &str, tc: &crate::gateway::llm::provider::ToolCall, plugins: &crate::plugins::PluginRegistry) -> String {
    execute_tool_call_in(std::path::Path::new("."), db, user_id, tc, plugins).await
}

#[cfg(all(test, unix))]
#[path = "plugin_dispatch_tests.rs"]
mod plugin_dispatch_tests;

async fn execute_tool_call_in(
    root: &std::path::Path,
    db: &crate::db::Database,
    user_id: &str,
    tc: &crate::gateway::llm::provider::ToolCall,
    plugins: &crate::plugins::PluginRegistry,
) -> String {
    super::tool_dispatch::DispatchContext::new(root, db, user_id, plugins,
        super::tool_dispatch::DispatchMode::Chat).execute(tc).await
}

fn reply_tts_enabled(settings: &crate::db::contexts::ContextSettings, channel_id: Option<&str>) -> bool {
    if channel_id == Some("web") {
        settings.web_chat_tts
    } else {
        settings.use_tts || channel_id.is_some_and(|ch| ch.starts_with("voice:"))
    }
}

fn spawn_tts(
    text: String,
    settings: &crate::db::contexts::ContextSettings,
    secrets: &crate::db::secrets::Secrets,
    user_id: &str,
    db: &crate::db::Database,
    message_id: Option<i64>,
    channel_id: Option<&str>,
) {
    // Tool-only turns, empty tag output and whitespace are not speech. Return
    // before spawning a task or constructing/contacting any TTS provider.
    if text.trim().is_empty() {
        return;
    }
    let is_web = channel_id == Some("web");
    // Also guard explicit feedback and stale settings snapshots before making
    // a provider call. A failed context lookup must not enable web synthesis.
    if is_web && !db.load_context(user_id).is_ok_and(|ctx| ctx.settings.web_chat_tts) {
        return;
    }
    let tts_type = settings.voice_tts_type.clone();
    // Use the caller's settings snapshot (including workflow overrides), with
    // compatibility for earlier custom_data configurations and environment defaults.
    // Never reinterpret other providers' voice IDs/local clone paths.
    let comfyui_config = (tts_type == "comfyui_xtts").then(|| {
        db.load_context(user_id).and_then(|ctx| {
            crate::comfyui::config::ComfyUiConfig::from_settings(settings, Some(&ctx.custom_data))
        })
    });
    let comfyui_qwen3_config = (tts_type == "comfyui_qwen3").then(|| {
        db.load_context(user_id).and_then(|ctx| {
            crate::comfyui::qwen3::Qwen3TtsConfig::from_settings(settings, Some(&ctx.custom_data))
        })
    });
    let rvc_on = settings.rvc_on;
    let rvc_server = settings.rvc_server.clone();
    let rvc_model_path = settings.rvc_model_path.clone();
    let rvc_index_path = settings.rvc_index_path.clone();
    let elevenlabs_api_key = secrets.elevenlabs_api_key.clone();
    let elevenlabs_voice_id = settings.voice_elevenlabs_voice_id.clone();
    let elevenlabs_tts_model = settings.elevenlabs_tts_model.clone();
    let elevenlabs_stability = settings.elevenlabs_stability;
    let elevenlabs_similarity_boost = settings.elevenlabs_similarity_boost;
    let elevenlabs_style = settings.elevenlabs_style;
    let elevenlabs_speed = settings.elevenlabs_speed;
    let elevenlabs_tts_language = settings.elevenlabs_tts_language.clone();
    let minimax_api_key = secrets.minimax_api_key.clone();
    let minimax_voice_id = settings.minimax_voice_id.clone();
    let minimax_model = settings
        .minimax_tts_model
        .clone()
        .unwrap_or_else(|| "speech-02-hd".to_string());
    let mimo_api_key = secrets.mimo_api_key.clone();
    let mimo_tts_type = settings
        .mimo_tts_type
        .clone()
        .unwrap_or_else(|| "builtin".to_string());
    let mimo_voice = settings.mimo_voice_id.clone();
    // TTS base URL/model overrides: context variable first, then environment.
    let mimo_tts_base_url = settings.mimo_tts_base_url.clone().or_else(|| {
        std::env::var("MIMO_TTS_API_BASE")
            .ok()
            .filter(|v| !v.trim().is_empty())
    });
    let mimo_tts_model_override = settings.mimo_tts_model.clone();
    let qwen_tts_server = settings.qwen_tts_server.clone();
    let qwen_tts_model = settings.qwen_tts_model.clone();
    let qwen_tts_speaker = settings.qwen_tts_speaker.clone();
    let qwen_tts_language = settings.qwen_tts_language.clone();
    let qwen_voice_clone_audio_path = settings.qwen_voice_clone_audio_path.clone();
    let qwen_voice_clone_enabled = settings.qwen_voice_clone_enabled;
    let qwen_voice_clone_prompt = settings.qwen_voice_clone_prompt.clone();
    let audio_output_path = settings.voice_audio_output_path.clone();
    let user_id = user_id.to_string();
    let tts_db = db.clone();

    tracing::trace!(user_id = %user_id, tts_type = %tts_type, "TTS: Spawning task");

    tokio::task::spawn(async move {
        // The task may not have been scheduled until after an OFF edit.
        if is_web && !tts_db.load_context(&user_id).is_ok_and(|ctx| ctx.settings.web_chat_tts) {
            return;
        }
        let audio_bytes = match tts_type.as_str() {
            "windows_sapi" => {
                let tts_engine = tts::windows_sapi::WindowsSAPI::new();
                match tts_engine.speak_to_bytes(&text) {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        tracing::warn!("TTS FAILED: windows_sapi: {}", e);
                        return;
                    }
                }
            }
            "elevenlabs" => {
                let api_key = elevenlabs_api_key.unwrap_or_default();
                let voice_id = elevenlabs_voice_id.unwrap_or_default();
                if api_key.is_empty() || voice_id.is_empty() {
                    tracing::warn!("TTS FAILED: elevenlabs: API key or voice_id not set");
                    return;
                }
                let tts_engine = tts::elevenlabs::ElevenLabsTTS::new(api_key, voice_id);
                let voice_settings = tts::elevenlabs::ElevenLabsVoiceSettings {
                    stability: elevenlabs_stability,
                    similarity_boost: elevenlabs_similarity_boost,
                    style: elevenlabs_style,
                    speed: elevenlabs_speed,
                    language: elevenlabs_tts_language,
                };
                match tts_engine
                    .speak_with_settings(&text, &elevenlabs_tts_model, &voice_settings)
                    .await
                {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        tracing::warn!("TTS FAILED: elevenlabs: {}", e);
                        return;
                    }
                }
            }
            "minimax" => {
                let api_key = minimax_api_key.unwrap_or_default();
                let voice_id = minimax_voice_id.unwrap_or_default();
                let tts_engine =
                    tts::minimax_tts::MiniMaxTTS::new(api_key, voice_id, minimax_model);
                match tts_engine.speak(&text).await {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        tracing::warn!("TTS FAILED: minimax: {}", e);
                        return;
                    }
                }
            }
            "mimo_tts" => {
                let api_key = mimo_api_key.unwrap_or_default();
                let model = mimo_tts_model_override.unwrap_or_else(|| {
                    match mimo_tts_type.as_str() {
                        "voicedesign" => "mimo-v2.5-tts-voicedesign",
                        "voiceclone" => "mimo-v2.5-tts-voiceclone",
                        _ => "mimo-v2.5-tts",
                    }
                    .to_string()
                });
                let tts_client = tts::mimo_tts::MiMoTTS::new(api_key, model, mimo_tts_base_url);
                let voice = mimo_voice.as_deref().unwrap_or("mimo_default");
                match tts_client.speak_builtin(&text, voice, None).await {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        tracing::warn!("TTS FAILED: mimo_tts: {}", e);
                        return;
                    }
                }
            }
            "comfyui_xtts" => {
                let config = match comfyui_config {
                    Some(Ok(config)) => config,
                    Some(Err(error)) => {
                        tracing::warn!("TTS FAILED: comfyui_xtts configuration: {error}");
                        return;
                    }
                    None => return,
                };
                // Reply synthesis intentionally outlives its generating task,
                // just like the existing TTS backends. TaskGuard drop on normal
                // completion must not cancel the final spoken reply.
                match crate::voice::comfyui_xtts::speak(&config, &text, None).await {
                    Ok(bytes) => bytes,
                    Err(error) => {
                        tracing::warn!("TTS FAILED: comfyui_xtts: {error:#}");
                        return;
                    }
                }
            }
            "comfyui_qwen3" => {
                let config = match comfyui_qwen3_config {
                    Some(Ok(config)) => config,
                    Some(Err(error)) => {
                        tracing::warn!("TTS FAILED: comfyui_qwen3 configuration: {error}");
                        return;
                    }
                    None => return,
                };
                // One combined WAV; reuse delivery below, with no provider fallback.
                match crate::voice::comfyui_qwen3::speak(&config, &text, None).await {
                    Ok(bytes) => bytes,
                    Err(error) => {
                        tracing::warn!("TTS FAILED: comfyui_qwen3: {error:#}");
                        return;
                    }
                }
            }
            "qwen_tts" => {
                let server_url = qwen_tts_server.unwrap_or_default();
                if server_url.is_empty() {
                    tracing::warn!("TTS FAILED: qwen_tts: server_url not set");
                    return;
                }
                let language = qwen_tts_language.unwrap_or_else(|| "English".to_string());
                let speaker = qwen_tts_speaker.clone();
                let tts_client = tts::qwen_tts::QwenTTSClient::new(server_url, language, speaker);
                let use_clone = qwen_voice_clone_enabled || qwen_voice_clone_audio_path.as_ref().map_or(false, |p| !p.is_empty());
                if use_clone {
                    if let Some(ref clone_path) = qwen_voice_clone_audio_path {
                        if !clone_path.is_empty() {
                            match tts_client.speak_voice_clone(&text, clone_path, qwen_voice_clone_prompt.as_deref()).await {
                                Ok(bytes) => bytes,
                                Err(e) => {
                                    tracing::warn!("TTS FAILED: qwen_tts voice_clone: {}", e);
                                    return;
                                }
                            }
                        } else {
                            tracing::warn!("TTS FAILED: qwen_tts: voice_clone_enabled but no audio path set");
                            return;
                        }
                    } else {
                        tracing::warn!("TTS FAILED: qwen_tts: voice_clone_enabled but no audio path set");
                        return;
                    }
                } else {
                    match tts_client.speak(&text).await {
                        Ok(bytes) => bytes,
                        Err(e) => {
                            tracing::warn!("TTS FAILED: qwen_tts: {}", e);
                            return;
                        }
                    }
                }
            }
            other => {
                tracing::warn!("TTS FAILED: Unknown type: {}", other);
                return;
            }
        };

        tracing::trace!(user_id = %user_id, "TTS: Audio received ({} bytes)", audio_bytes.len());

        // Cost ledger: record ElevenLabs character usage per user.
        if tts_type == "elevenlabs" {
            let chars = text.chars().count();
            if let Err(e) = crate::db::memory::record_media_spend(&tts_db, &user_id, chars) {
                tracing::warn!("media_spend record failed: {}", e);
            }
        }

        if let Some(ref path) = audio_output_path {
            let folder = tts::ensure_audio_folder(path, "generated_tts");
            let _ = tts::save_audio_file(
                &audio_bytes,
                folder.to_str().unwrap_or(path),
                "01_tts_original",
            );
        }

        let final_audio = if rvc_on {
            match tts::convert_through_rvc(audio_bytes, rvc_server, rvc_model_path, rvc_index_path)
                .await
            {
                Ok(converted) => {
                    if let Some(ref path) = audio_output_path {
                        let folder = tts::ensure_audio_folder(path, "rvc");
                        let _ = tts::save_audio_file(
                            &converted,
                            folder.to_str().unwrap_or(path),
                            "02_rvc_converted",
                        );
                    }
                    converted
                }
                Err(e) => {
                    tracing::warn!("TTS RVC FAILED: {}", e);
                    return;
                }
            }
        } else {
            audio_bytes
        };

        if let Some(ref path) = audio_output_path {
            let folder = tts::ensure_audio_folder(path, "final_output");
            let _ = tts::save_audio_file(&final_audio, folder.to_str().unwrap_or(path), "03_final");
        }

        if final_audio.is_empty() {
            tracing::warn!(user_id = %user_id, "TTS returned empty audio");
            return;
        }

        // Persist first: SSE is only a notification, not the sole copy of the
        // audio. History/reconnect can recover it even with zero subscribers.
        let mime = tts_audio_mime(&final_audio);
        let stored = match message_id {
            Some(id) => match tts_db.save_message_audio(&user_id, id, mime, &final_audio) {
                Ok(saved) => saved,
                Err(e) => {
                    tracing::warn!(user_id = %user_id, "Saving reply audio failed: {e}");
                    false
                }
            },
            None => false, // Transient feedback has no persisted reply row.
        };
        // Never resurrect a deleted reply (or a changed storage session) as an
        // anonymous audio clip. Inline fallback is only for transient feedback.
        // Synthesis can finish after the user disables web TTS. Keep the paid-
        // for bytes available for manual replay, but do not announce/autoplay.
        let web_chat_tts = tts_db.load_context(&user_id)
            .is_ok_and(|ctx| ctx.settings.web_chat_tts);
        if web_chat_tts && (stored || message_id.is_none())
            && crate::dashboard::stream::has_subscriber(&user_id)
        {
            let payload = if stored {
                serde_json::json!({ "message_id": message_id, "mime": mime })
            } else {
                use base64::Engine;
                serde_json::json!({
                    "audio": format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(&final_audio)),
                    "mime": mime,
                })
            };
            crate::dashboard::stream::send(&user_id, "chat_tts", &payload.to_string());
        } else if stored {
            tracing::info!(user_id = %user_id, message_id, "Reply audio saved for dashboard replay");
        }

        if !is_web || web_chat_tts {
            crate::event_channel::broadcast_voice_tts(&user_id, final_audio);
        }
    });
}

fn tts_audio_mime(audio: &[u8]) -> &'static str {
    if audio.starts_with(b"RIFF") && audio.get(8..12) == Some(b"WAVE") {
        "audio/wav"
    } else if audio.starts_with(b"OggS") {
        "audio/ogg"
    } else if audio.starts_with(b"fLaC") {
        "audio/flac"
    } else {
        "audio/mpeg"
    }
}

#[cfg(test)]
#[path = "skill_dispatch_tests.rs"]
mod skill_dispatch_tests;

#[cfg(test)]
#[path = "backend_dispatch_tests.rs"]
mod backend_dispatch_tests;

#[cfg(test)]
#[path = "message_tool_loop_tests.rs"]
mod message_tool_loop_tests;

#[cfg(test)]
#[path = "local_model_live_tests.rs"]
mod local_model_live_tests;

#[cfg(test)]
#[path = "message_audio_tests.rs"]
mod message_audio_tests;

#[cfg(test)]
#[path = "comfyui_tts_tests.rs"]
mod comfyui_tts_tests;

#[cfg(test)]
mod gateway_tests {
    #[test]
    fn test_message_handler_compiles() {
        assert!(true);
    }
}

include!("capability_dispatch_tests.rs");
