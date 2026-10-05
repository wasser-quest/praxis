pub mod anthropic;
pub mod codex;
pub mod embeddings;
pub mod error;
pub(super) mod http;
pub mod resilience;
mod routing;
pub mod llamacpp;
pub mod mimo;
pub mod minimax;
pub mod ollama;
pub mod openai;
pub mod openrouter;
pub mod free_router;
pub mod provider;
mod tests;
#[cfg(test)]
mod resilience_tests;
#[cfg(test)]
mod http_tests;
#[cfg(test)]
mod task_tests;

use provider::{ChatRequest, ChatResponse, LLMProvider};

pub struct LLMRouter {
    providers: Vec<Box<dyn LLMProvider>>,
    default_provider: String,
    fallback_providers: Vec<String>,
    policy: resilience::ResilienceConfig,
    gates: std::collections::HashMap<String, std::sync::Arc<resilience::ProviderGate>>,
}

impl LLMRouter {
    pub fn new(config: &crate::config::Config, secrets: &crate::db::secrets::Secrets) -> Self {
        let mut providers: Vec<Box<dyn LLMProvider>> = Vec::new();

        if let Some(key) = secrets.openai_api_key.as_ref().filter(|key| !key.trim().is_empty()) {
            providers.push(Box::new(openai::OpenAIProvider::new(
                key.clone(),
                config.openai_model.clone(),
                config.openai_api_base.clone(),
            )));
        }

        if let Some(auth) = codex::CodexAuth::from_secrets(secrets) {
            // Refreshed tokens go back into the live store (and disk when the
            // gateway holds the master key) so restarts do not log the user out.
            let imported = auth.clone();
            let on_refresh: codex::OnRefresh = Box::new(move |previous: &codex::CodexAuth, auth: &codex::CodexAuth| {
                if crate::db::secrets::update_runtime_secrets(|secrets| {
                    // A retired router must not undo logout or replace a new
                    // account. Its original snapshot may precede a shared refresh.
                    let current = codex::CodexAuth::from_secrets(secrets);
                    if current.as_ref() != Some(previous) && current.as_ref() != Some(&imported) { return false; }
                    auth.store(secrets);
                    true
                }).is_err() {
                    tracing::warn!("Codex tokens refreshed in memory but could not be persisted");
                }
            });
            providers.push(Box::new(codex::CodexProvider::new(auth, config.codex_model.clone(), Some(on_refresh))));
        }

        if let Some(key) = secrets.anthropic_api_key.as_ref().filter(|key| !key.trim().is_empty()) {
            providers.push(Box::new(anthropic::AnthropicProvider::new(
                key.clone(),
                config.anthropic_model.clone(),
                config.anthropic_api_base.clone(),
            )));
        }

        providers.push(Box::new(ollama::OllamaProvider::new(
            config.ollama_api_base.clone(),
            config.ollama_model.clone(),
            secrets.ollama_api_key.clone(),
        )));

        providers.push(Box::new(llamacpp::LlamaCppProvider::new(
            secrets.llamacpp_api_key.clone(),
            config.llamacpp_model.clone(),
            config.llamacpp_api_base.clone(),
        )));

        // pgpu free router: uses the same GPU_ROUTER_URL/TOKEN as the GPU router.
        // The free router endpoint is /free/v1 on the pgpu dashboard port.
        if crate::gpu_router::configured() {
            let token = std::env::var("GPU_ROUTER_TOKEN").unwrap_or_default();
            providers.push(Box::new(free_router::FreeRouterProvider::new(
                if token.is_empty() { None } else { Some(token) },
                config.llamacpp_model.clone(), // reuse llamacpp model as default
                std::env::var("GPU_ROUTER_URL").unwrap_or_default(),
            )));
        }

        if let Some(key) = secrets.minimax_api_key.as_ref().filter(|key| !key.trim().is_empty()) {
            providers.push(Box::new(minimax::MiniMaxProvider::new(
                key.clone(),
                config.minimax_model.clone(),
                config.minimax_api_base.clone(),
                config.minimax_api_mode.clone(),
            )));
        }

        if let Some(key) = secrets.mimo_api_key.as_ref().filter(|key| !key.trim().is_empty()) {
            providers.push(Box::new(mimo::MiMoProvider::new(
                key.clone(),
                config.mimo_model.clone(),
                config.mimo_api_base.clone(),
                config.mimo_api_mode.clone(),
            )));
        }

        if let Some(key) = secrets.openrouter_api_key.as_ref().filter(|key| !key.trim().is_empty()) {
            providers.push(Box::new(openrouter::OpenRouterProvider::new(
                key.clone(),
                config.openrouter_model.clone(),
                config.openrouter_api_base.clone(),
            )));
        }

        let mut router = Self::with_providers(
            providers, config.use_provider.clone(), config.llm_fallback_providers.clone(), config.llm_resilience.clone(),
        );
        // Aliases pointing at the same endpoint/account must not multiply the
        // quota. Keys stay in this local map only and are never logged.
        let mut accounts = std::collections::HashMap::new();
        for (name, base, key) in [
            ("openai", &config.openai_api_base, secrets.openai_api_key.as_deref()),
            ("anthropic", &config.anthropic_api_base, secrets.anthropic_api_key.as_deref()),
            ("ollama", &config.ollama_api_base, secrets.ollama_api_key.as_deref()),
            ("llamacpp", &config.llamacpp_api_base, secrets.llamacpp_api_key.as_deref()),
            ("free_router", &std::env::var("GPU_ROUTER_URL").unwrap_or_default(), std::env::var("GPU_ROUTER_TOKEN").ok().filter(|s|!s.is_empty()).as_deref()),
            ("minimax", &config.minimax_api_base, secrets.minimax_api_key.as_deref()),
            ("mimo", &config.mimo_api_base, secrets.mimo_api_key.as_deref()),
            ("openrouter", &config.openrouter_api_base, secrets.openrouter_api_key.as_deref()),
        ] {
            if let Some(gate) = router.gates.get_mut(name) {
                let endpoint = reqwest::Url::parse(base).map(|url| url.origin().ascii_serialization()).unwrap_or_else(|_| base.clone());
                *gate = accounts.entry((endpoint, key.unwrap_or("").to_string())).or_insert_with(|| gate.clone()).clone();
            }
        }
        router
    }

    pub fn provider_names(&self) -> Vec<String> {
        let mut names: Vec<_> = self.providers
            .iter()
            .map(|p| p.name().to_string())
            .collect();
        names.sort();
        names
    }

    /// Provider metadata without constructing HTTP clients or starting workers.
    pub(crate) fn configured_provider_names(secrets: &crate::db::secrets::Secrets) -> Vec<String> {
        let mut names: Vec<_> = super::providers::PROVIDERS.iter()
            .filter(|spec| matches!(spec.kind, super::providers::AuthKind::Endpoint)
                || super::providers::has_credential(secrets, spec.name))
            .map(|spec| spec.name.to_owned()).collect();
        names.sort();
        names
    }

    pub fn default_provider(&self) -> &str {
        &self.default_provider
    }

    /// Multi-turn tool loop: calls LLM, executes tools, feeds results back.
    /// Loops until no more tool calls or max_iterations reached.
    pub async fn chat_with_tools(
        &self,
        db: &crate::db::Database,
        user_id: &str,
        mut messages: Vec<provider::ChatMessage>,
        tools: Vec<provider::ToolDefinition>,
        max_iterations: Option<i32>,
        feedback_tx: Option<tokio::sync::mpsc::UnboundedSender<String>>,
        provider: Option<&str>,
        model: Option<&str>,
    ) -> anyhow::Result<ChatWithToolsResult> {
        let _task = crate::gateway::task_control::begin(user_id)?;
        let context = db.load_context(user_id)?;
        let workflow = crate::sm::load_file(crate::gateway::prompt::workflow_name(&context))
            .map_err(|e| anyhow::anyhow!("Cannot load verification policy: {e}"))?;
        crate::gateway::action_contracts::bind(user_id, crate::gateway::prompt::workflow_name(&context), &workflow, std::path::Path::new("."))?;
        let mut tools = tools;
        for tool in &mut tools { crate::tools::tool_output::augment_definition(tool); }
        if !tools.is_empty()
            && crate::db::tools::get(db, "read_tool_result").is_ok_and(|t| t.is_enabled)
            && crate::tools::packages::tool_package_enabled(&db.data_dir(), "read_tool_result").unwrap_or(false)
        {
            if !tools.iter().any(|t| t.function.name == "read_tool_result") {
                if let Some(reader) = crate::db::tools::to_tool_definitions(db)?.into_iter().find(|t| t.function.name == "read_tool_result") {
                    tools.push(reader);
                }
            }
            messages.push(provider::ChatMessage { role: "system".into(), content: Some(crate::tools::tool_output::INSTRUCTIONS.into()),
                reasoning_content: None, content_parts: None, tool_calls: None, tool_call_id: None, tool_name: None });
        }
        let cancel = crate::gateway::task_control::cancellation(user_id).unwrap_or_default();
        let max_iterations = max_iterations.unwrap_or(200);
        let mut iteration = 0;
        let mut tool_call_records: Vec<ToolCallRecord> = Vec::new();
        let mut feedback_messages: Vec<String> = Vec::new();
        let mut last_assistant_response = String::new();
        let mut agent_signal = AgentSignalFromTool::None;

        loop {
            iteration += 1;
            if iteration > max_iterations {
                anyhow::bail!("Max tool iterations exceeded ({})", max_iterations);
            }

            let request = ChatRequest {
                messages: messages.clone(),
                tools: if tools.is_empty() {
                    None
                } else {
                    Some(tools.clone())
                },
                temperature: Some(0.7),
                max_tokens: Some(self.task_output_tokens()),
                model: model.map(|m| m.to_string()),
                vision_provider: None,
                vision_model: None,
                thinking: None,
            };

            let response = self.chat_controlled(request, provider, None, &cancel).await?;

            // Add assistant message to history
            let assistant_content = response.content.clone().unwrap_or_default();
            if !assistant_content.is_empty() {
                last_assistant_response = assistant_content.clone();
            }

            messages.push(provider::ChatMessage {
    reasoning_content: None,
                role: "assistant".to_string(),
                content: response.content.clone(),
                content_parts: None,
                tool_calls: response.tool_calls.clone(),
                tool_call_id: None,
                tool_name: None,
            });

            // If no tool calls, we're done
            let tool_calls = match response.tool_calls {
                Some(calls) if !calls.is_empty() => calls,
                _ => {
                    crate::gateway::action_contracts::require(user_id, "_complete")?;
                    break;
                }
            };

            // Execute each tool call
            for tool_call in &tool_calls {
                if cancel.is_cancelled() {
                    return Err(error::ProviderError::new(error::ErrorKind::Cancelled).into());
                }
                let args: serde_json::Value =
                    serde_json::from_str(&tool_call.function.arguments).unwrap_or_default();

                let args = match crate::tools::tool_output::execution_args(&args) {
                    Ok(args) => args,
                    Err(error) => {
                        let result = format!("Error: {error}; tool not executed");
                        tool_call_records.push(ToolCallRecord { name: tool_call.function.name.clone(), arguments: tool_call.function.arguments.clone(), result: result.clone() });
                        messages.push(provider::ChatMessage { role: "tool".into(), content: Some(result), reasoning_content: None,
                            content_parts: None, tool_calls: None, tool_call_id: Some(tool_call.id.clone()), tool_name: Some(tool_call.function.name.clone()) });
                        continue;
                    }
                };
                tracing::info!(tool = %tool_call.function.name, args_bytes = tool_call.function.arguments.len(), "Executing tool call");

                // Handle agent control signals specially
                crate::gateway::action_contracts::before_tool(user_id, &tool_call.function.name)?;
                let result_str = match tool_call.function.name.as_str() {
                    "apply_patch" => crate::tools::apply_patch::run(user_id, &tool_call.id, &args).await
                        .unwrap_or_else(|e| format!("Error: {e}")),
                    "inspect_file" => crate::tools::apply_patch::inspect(user_id, &args).await
                        .unwrap_or_else(|e| format!("Error: {e}")),
                    "run_check" => crate::gateway::action_contracts::run(user_id, &tool_call.id, &args).await
                        .unwrap_or_else(|e| format!("Error: {e}")),
                    "read_tool_result" => crate::tools::tool_output::run(db, user_id, &args)
                        .unwrap_or_else(|e| format!("Error: {e}")),
                    "use_skill" => crate::tools::use_skill::run(db, &args).await
                        .unwrap_or_else(|e| format!("Error: {}", e)),
                    "update_template" => crate::tools::update_template::run(db, &args).await
                        .unwrap_or_else(|e| format!("Error: {}", e)),
                    "agent_complete" => {
                        match crate::tools::agent_control::run(
                            db,
                            user_id,
                            crate::tools::agent_control::AgentControlSignal::Complete,
                        )
                        .await {
                            Ok(result) => { agent_signal = AgentSignalFromTool::Done; result }
                            Err(error) => format!("Error: {error}"),
                        }
                    }
                    "agent_next" | "agent_back" => {
                        let root = crate::gateway::state_ref().map(|state| std::path::PathBuf::from(&state.config.root_dir))
                            .or_else(|| std::env::var_os("ROOT_DIR").map(std::path::PathBuf::from)).unwrap_or_else(|| std::path::PathBuf::from("."));
                        match crate::tools::agent_control::navigate(db, &root, user_id, tool_call.function.name == "agent_back", &args).await {
                            Ok(result) => { agent_signal = AgentSignalFromTool::Next; result }
                            Err(error) => format!("Error: {error}"),
                        }
                    }
                    "agent_set_path" => {
                        let path = args
                            .get("path")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        agent_signal = AgentSignalFromTool::Path(path.clone());
                        crate::tools::agent_control::run(
                            db,
                            user_id,
                            crate::tools::agent_control::AgentControlSignal::Path(path),
                        )
                        .await
                        .unwrap_or_else(|e| format!("Error: {}", e))
                    }
                    "agent_feedback" => {
                        let msg = args
                            .get("message")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        if !msg.is_empty() {
                            if let Some(ref tx) = feedback_tx {
                                let _ = tx.send(msg.clone());
                            }
                            feedback_messages.push(msg);
                        }
                        "Feedback sent successfully.".to_string()
                    }
                    "execute_terminal" => {
                        let command = args["command"].as_str().unwrap_or("");
                        match crate::tools::execute_terminal::execute_terminal(command, None).await
                        {
                            Ok(result) => result.render(),
                            Err(e) => format!("Error: {}", e),
                        }
                    }
                    "write_file" => {
                        let path = args["path"].as_str().unwrap_or("");
                        let content = args["content"].as_str().unwrap_or("");
                        match crate::tools::write_file::write_file(path, content).await {
                            Ok(_) => format!("File written: {}", path),
                            Err(e) => format!("Error: {}", e),
                        }
                    }
                    #[cfg(feature = "legacy_file_ops")]
                    "edit_file" | "read_file" => praxis_legacy_file_ops::execute(&tool_call.function.name, &args)
                        .await.unwrap_or_else(|e| format!("Error: {e}")),
                    "memory_profile_create" => {
                        crate::tools::memory::profile_create(db, user_id, &args)
                            .unwrap_or_else(|e| format!("Error: {e}"))
                    }
                    "memory_profile_load" => crate::tools::memory::profile_load(db, user_id, &args)
                        .unwrap_or_else(|e| format!("Error: {e}")),
                    "memory_profile_list" => crate::tools::memory::profile_list(db, user_id)
                        .unwrap_or_else(|e| format!("Error: {e}")),
                    "memory_get" => crate::tools::memory::get(db, user_id, &args)
                        .unwrap_or_else(|e| format!("Error: {e}")),
                    "memory_set" => crate::tools::memory::set(db, user_id, &args)
                        .unwrap_or_else(|e| format!("Error: {e}")),
                    "learn_fact" => {
                        let fact = args["fact"].as_str().unwrap_or("");
                        match crate::db::memory_profiles::learn_fact(db, user_id, fact) {
                            Ok(_) => format!("Learned: {}", fact),
                            Err(e) => format!("Error: {e}"),
                        }
                    }
                    "learn_preference" => {
                        let key = args["key"].as_str().unwrap_or("");
                        let value = args.get("value").cloned().unwrap_or(serde_json::json!(""));
                        match crate::db::memory_profiles::update_memory(db, user_id, |memory| {
                            crate::db::memory::update_preference(memory, key, &value);
                        }) {
                            Ok(_) => format!("Preference saved: {}", key),
                            Err(e) => format!("Error: {e}"),
                        }
                    }
                    "learn_topic" => {
                        let topic = args["topic"].as_str().unwrap_or("");
                        match crate::db::memory_profiles::learn_topic(db, user_id, topic) {
                            Ok(_) => format!("Topic tracked: {}", topic),
                            Err(e) => format!("Error: {e}"),
                        }
                    }
                    "ask_questions" => {
                        // Get timeout from args, then context, then default to 120
                        let ctx_data = db
                            .load_context(user_id)
                            .ok()
                            .map(|ctx| ctx.custom_data)
                            .filter(|v| !v.is_null());
                        let fallback_ch = ctx_data
                            .as_ref()
                            .and_then(|c| c.get("channel_id"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let channel_id = args["channel_id"]
                            .as_str()
                            .filter(|s| !s.is_empty())
                            .unwrap_or(fallback_ch);
                        let discord_pairing = db
                            .get_pairing_by_internal_user(user_id)
                            .ok()
                            .flatten();
                        let is_web = channel_id == "web" || channel_id.is_empty() || discord_pairing.is_none();
                        if channel_id.is_empty() && discord_pairing.is_some() {
                            "Error: No channel_id provided and no originating channel found. Please specify a channel_id.".to_string()
                        } else {
                            let default_timeout = ctx_data
                                .as_ref()
                                .and_then(|c| c.get("question_timeout_secs"))
                                .and_then(|v| v.as_u64())
                                .unwrap_or(120);
                            let timeout = args["timeout_secs"].as_u64().unwrap_or(default_timeout);
                            let paired_discord_user_id = discord_pairing
                                .map(|p| p.discord_user_id)
                                .unwrap_or_default();
                            let questions_raw = args["questions"].as_array();
                            let mut questions: Vec<(String, String, Vec<String>)> = Vec::new();
                            let mut format_error = None;
                            if let Some(arr) = questions_raw {
                                for (i, q) in arr.iter().enumerate() {
                                    // Check for invalid 'options' field
                                    if q.get("options").is_some() {
                                        format_error = Some(format!("Error: Question {}: use 'suggestions' with plain strings, not 'options' with objects. Example: \"suggestions\": [\"yes\", \"no\", \"maybe\"]", i + 1));
                                        break;
                                    }
                                    let label = q["label"].as_str().filter(|s| !s.is_empty());
                                    let Some(label) = label else {
                                        format_error = Some(format!("Error: Question {} is missing a non-empty 'label' field.", i + 1));
                                        break;
                                    };
                                    let text = q["question"].as_str().filter(|s| !s.is_empty());
                                    let Some(text) = text else {
                                        format_error = Some(format!("Error: Question {} (label: '{}') is missing a non-empty 'question' field.", i + 1, label));
                                        break;
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
                            }
                            if let Some(error) = format_error {
                                error
                            } else if questions_raw.is_none() {
                                "Error: 'questions' field is required and must be an array.".to_string()
                            } else if questions.is_empty() {
                                "Error: 'questions' array must contain at least one question.".to_string()
                            } else if is_web {
                                tracing::info!(timeout_secs = timeout, "ask_questions: web mode");
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
                                tracing::info!(timeout_secs = timeout, "ask_questions: waiting for responses");
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
                            "Error: name, schedule, and prompt are required.".to_string()
                        } else if !crate::gateway::cron_scheduler::CronScheduler::validate_schedule(schedule) {
                            format!("Error: Invalid cron expression '{}'.", schedule)
                        } else {
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
                                Ok(_) => format!("Cron job created. ID: {} Name: '{}'", job_id, name),
                                Err(e) => format!("Error: {}", e),
                            }
                        }
                    }
                    "cron_delete" => {
                        let job_id = args["job_id"].as_str().unwrap_or("");
                        if job_id.is_empty() {
                            "Error: job_id required.".to_string()
                        } else {
                            match db.delete_cron_job(job_id) {
                                Ok(_) => format!("Cron job {} deleted.", job_id),
                                Err(e) => format!("Error: {}", e),
                            }
                        }
                    }
                    "cron_list" => {
                        match db.list_cron_jobs(user_id) {
                            Ok(jobs) if jobs.is_empty() => "No cron jobs.".to_string(),
                            Ok(jobs) => {
                                let mut out = String::from("Cron jobs:\n");
                                for j in &jobs {
                                    let s = if j.enabled { "on" } else { "off" };
                                    out.push_str(&format!("- [{}] {} ({}) runs={} schedule='{}' prompt='{}'\n", j.id, j.name, s, j.run_count, j.schedule, j.prompt));
                                }
                                out
                            }
                            Err(e) => format!("Error: {}", e),
                        }
                    }
                    "cron_toggle" => {
                        let job_id = args["job_id"].as_str().unwrap_or("");
                        let enabled = args["enabled"].as_bool().unwrap_or(true);
                        if job_id.is_empty() {
                            "Error: job_id required.".to_string()
                        } else {
                            match db.toggle_cron_job(job_id, enabled) {
                                Ok(_) => format!("Job {} {}.", job_id, if enabled { "enabled" } else { "disabled" }),
                                Err(e) => format!("Error: {}", e),
                            }
                        }
                    }
                    "cron_run" => {
                        let job_id = args["job_id"].as_str().unwrap_or("");
                        if job_id.is_empty() {
                            "Error: job_id required.".to_string()
                        } else {
                            match db.get_cron_job(job_id) {
                                Ok(Some(j)) => format!("Triggered '{}'. Prompt: '{}' Template: {}", j.name, j.prompt, j.template),
                                Ok(None) => format!("Job {} not found.", job_id),
                                Err(e) => format!("Error: {}", e),
                            }
                        }
                    }
                    other => format!("Unknown tool: {}", other),
                };

                // Check if this was an image tool that returned content_parts
                let image_result = if tool_call.function.name == "understand_image" {
                    Some(crate::tools::understand_image::run(&args).await)
                } else {
                    None
                };

                let (final_result_str, content_parts) = if let Some(img) = image_result {
                    (img.text, Some(img.content_parts))
                } else {
                    (result_str.clone(), None)
                };

                tool_call_records.push(ToolCallRecord {
                    name: tool_call.function.name.clone(),
                    arguments: tool_call.function.arguments.clone(),
                    result: final_result_str.clone(),
                });

                let delivered = crate::gateway::tool_results::prepare_for_call(db, user_id, tool_call, &final_result_str)?;
                // Add the MODEL-selected view, never the silently clipped original.
                messages.push(provider::ChatMessage {
    reasoning_content: None,
                    role: "tool".to_string(),
                    content: Some(delivered),
                    content_parts,
                    tool_calls: None,
                    tool_call_id: Some(tool_call.id.clone()),
                    tool_name: Some(tool_call.function.name.clone()),
                });
            }

            if agent_signal == AgentSignalFromTool::Done {
                crate::gateway::action_contracts::require(user_id, "_complete")?;
                break;
            }
        }

        Ok(ChatWithToolsResult {
            response: last_assistant_response,
            tool_calls: tool_call_records,
            feedback_messages,
            agent_signal,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum AgentSignalFromTool {
    None,
    Done,
    Next,
    Path(String),
}

#[derive(Debug)]
pub struct ToolCallRecord {
    pub name: String,
    pub arguments: String,
    pub result: String,
}

#[derive(Debug)]
pub struct ChatWithToolsResult {
    pub response: String,
    pub tool_calls: Vec<ToolCallRecord>,
    pub feedback_messages: Vec<String>,
    pub agent_signal: AgentSignalFromTool,
}

#[cfg(test)]
mod skill_tool_loop_tests;

#[cfg(test)]
mod gateway_tests {
    use super::*;

    #[test]
    fn test_llm_router_creation() {
        let config = crate::config::Config::from_env();
        let secrets = crate::db::secrets::Secrets::default();
        let _router = LLMRouter::new(&config, &secrets);
    }
}
