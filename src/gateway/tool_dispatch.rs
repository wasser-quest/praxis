//! One execution boundary for chat, agent/WebSocket and lowered Decision IR.
//! Ingress owns task charging and result archival; this layer resolves owners,
//! rechecks flags/cancellation and preserves the native compatibility adapters.
use super::llm::provider::ToolCall;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum DispatchMode {
    Chat,
    Agent,
}

/// Constructed by ingress from the authenticated caller and installation root,
/// never from model arguments. The pinned workspace remains in the task ledger.
pub(crate) struct DispatchContext<'a> {
    root: &'a std::path::Path,
    db: &'a crate::db::Database,
    user_id: &'a str,
    plugins: &'a crate::plugins::PluginRegistry,
    mode: DispatchMode,
}
impl<'a> DispatchContext<'a> {
    pub(crate) fn new(
        root: &'a std::path::Path,
        db: &'a crate::db::Database,
        user_id: &'a str,
        plugins: &'a crate::plugins::PluginRegistry,
        mode: DispatchMode,
    ) -> Self {
        Self {
            root,
            db,
            user_id,
            plugins,
            mode,
        }
    }
    pub(crate) async fn execute(&self, call: &ToolCall) -> String {
        dispatch(
            self.root,
            self.db,
            self.user_id,
            call,
            self.plugins,
            self.mode,
        )
        .await
    }
}

async fn dispatch(
    root: &std::path::Path,
    db: &crate::db::Database,
    user_id: &str,
    tc: &ToolCall,
    plugins: &crate::plugins::PluginRegistry,
    mode: DispatchMode,
) -> String {
    dispatch_at_depth(root, db, user_id, tc, plugins, mode, 0).await
}

/// One dispatch iteration: the caller's result text, or a call that the
/// `execute_decision` operation lowered and the loop must run next.
enum Step {
    Text(String),
    Lower(ToolCall),
}

/// The dispatch loop. A lowered Decision-IR instruction becomes its next
/// iteration — never a nested dispatch — so the instruction and its resolved
/// call share one execution chain (`docs/PLUGINIZATION_HANDOFF.md` §6C).
/// `depth` counts lowerings already run for this call.
async fn dispatch_at_depth(
    root: &std::path::Path,
    db: &crate::db::Database,
    user_id: &str,
    tc: &ToolCall,
    plugins: &crate::plugins::PluginRegistry,
    mode: DispatchMode,
    depth: u8,
) -> String {
    let mut call = tc.clone();
    let mut depth = depth;
    loop {
        // One iteration of the dispatch body, in this future rather than a
        // nested one: an iteration's frames must not stack on the next's.
        let step = 'step: {
            let tc = &call;

            if let Err(error) = super::task_control::check_registry(user_id, plugins) {
                break 'step Step::Text(format!("Error: {error}; tool not executed"));
            }
            tracing::info!(tool = %tc.function.name, args_bytes = tc.function.arguments.len(), "execute_tool_call: dispatching");

            let args: serde_json::Value = match serde_json::from_str(&tc.function.arguments) {
                Ok(v) => v,
                Err(e) => break 'step Step::Text(format!("Error parsing arguments: {}", e)),
            };

            let args = match crate::tools::tool_output::execution_args(&args) {
                Ok(args) => args,
                Err(error) => break 'step Step::Text(format!("Error: {error}; tool not executed")),
            };
            let owner = match crate::tools::catalog::owner(plugins, &tc.function.name) {
                Ok(owner) => owner,
                Err(error) => break 'step Step::Text(format!("Error: {error}; tool not executed")),
            };
            if let Err(error) = crate::tools::catalog::require_enabled(db, &owner, &tc.function.name) {
                break 'step Step::Text(format!("Error: {error}; tool not executed"));
            }
            if super::task_control::cancellation(user_id).is_some_and(|token| token.is_cancelled()) {
                break 'step Step::Text("Error: task cancelled; tool not executed".into());
            }
            if !matches!(&owner, crate::tools::catalog::ToolOwner::Plugin { tool, .. } if tool.contract.is_some())
            {
                if let Err(error) = super::action_contracts::before_tool(user_id, &tc.function.name) {
                    break 'step Step::Text(format!("Error: {error}; tool not executed"));
                }
            }
            let ctx_data = db
                .load_context(user_id)
                .ok()
                .map(|ctx| ctx.custom_data)
                .filter(|v| !v.is_null());

            let all_secrets = crate::db::secrets::get_secrets();
            let plugin_secrets = plugins.secrets_for_tool(&tc.function.name, &all_secrets);

            if matches!(&owner, crate::tools::catalog::ToolOwner::Plugin { .. }) {
                // A `builtin` handler runs a host-owned operation under this
                // dispatcher's host-issued identity and workspace root; the package
                // only declares the tool (name, schema, ownership). A lowered call
                // goes back to the loop above as its next iteration.
                if let crate::tools::catalog::ToolOwner::Plugin { tool, .. } = &owner {
                    if let crate::plugins::PluginHandler::Builtin { name: operation } = &tool.handler {
                        break 'step match Box::pin(crate::tools::builtin_operations::execute(
                            &crate::tools::builtin_operations::BuiltinContext {
                                db,
                                plugins,
                                user: user_id,
                                call: &tc.id,
                                root,
                                depth,
                            },
                            operation,
                            &args,
                        ))
                        .await
                        {
                            Ok(crate::tools::builtin_operations::BuiltinStep::Text(text)) => Step::Text(text),
                            Ok(crate::tools::builtin_operations::BuiltinStep::Lower(resolved)) => {
                                Step::Lower(resolved)
                            }
                            Err(error) => Step::Text(format!("Error: {error}")),
                        };
                    }
                }
                break 'step Step::Text(Box::pin(plugins
                    .execute_tool_with_host(
                        db,
                        user_id,
                        &tc.id,
                        &tc.function.name,
                        &args,
                        ctx_data.as_ref(),
                        Some(&plugin_secrets),
                    ))
                    .await
                    .unwrap_or_else(|e| format!("Error: Plugin tool {} failed: {}", tc.function.name, e)));
            }

            Step::Text(match tc.function.name.as_str() {
                "apply_patch" => crate::tools::apply_patch::run(user_id, &tc.id, &args)
                    .await
                    .unwrap_or_else(|e| format!("Error: {e}")),
                "inspect_file" => crate::tools::apply_patch::inspect(user_id, &args)
                    .await
                    .unwrap_or_else(|e| format!("Error: {e}")),
                "search_skills" => crate::tools::search_skills::run(db, &args)
                    .await
                    .unwrap_or_else(|e| format!("Error: {e}")),
                "use_skill" => crate::tools::use_skill::run(db, &args)
                    .await
                    .unwrap_or_else(|e| format!("Error: {}", e)),
                #[cfg(feature = "shell")]
                "execute_terminal" => {
                    // Execute on the backend selected by the host at registration.
                    if mode == DispatchMode::Agent && crate::runtime::vm::guest_backend(plugins) {
                        let command = args["command"].as_str().unwrap_or("");
                        match invoke_guest(plugins, db, user_id, &tc.id,
                            "vm_shell",
                            &serde_json::json!({"command": command}),
                        )
                        .await
                        {
                            Ok(result) => result,
                            Err(error) => format!("Error: {error}"),
                        }
                    } else {
                        let command = args["command"].as_str().unwrap_or("");
                        match crate::tools::execute_terminal::execute_terminal(command, None).await {
                            Ok(result) => result.render(),
                            Err(e) => format!("Error: {}", e),
                        }
                    } // end else (shared mode)
                }
                #[cfg(feature = "shell")]
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
                #[cfg(feature = "shell")]
                "background_status" => match args["job_id"].as_str() {
                    Some(id) => match crate::tools::execute_terminal::job_status(id)
                        .filter(|job| job.owner_user_id.as_deref() == Some(user_id))
                    {
                        Some(job) => {
                            serde_json::to_string_pretty(&job).unwrap_or_else(|_| format!("{}", id))
                        }
                        None => format!("Unknown job id: {}. Use no job_id to list your jobs.", id),
                    },
                    None => {
                        let jobs: Vec<_> = crate::tools::execute_terminal::list_jobs()
                            .into_iter()
                            .filter(|job| job.owner_user_id.as_deref() == Some(user_id))
                            .collect();
                        if jobs.is_empty() {
                            "No background jobs.".to_string()
                        } else {
                            serde_json::to_string_pretty(&jobs).unwrap_or_else(|_| "jobs".to_string())
                        }
                    }
                },
                "write_file" => {
                    if mode == DispatchMode::Agent && crate::runtime::vm::guest_backend(plugins) {
                        let path = args["path"].as_str().unwrap_or("");
                        let content = args["content"].as_str().unwrap_or("");
                        match invoke_guest(plugins, db, user_id, &tc.id,
                            "vm_file_transfer",
                            &serde_json::json!({"path": path, "content": content, "direction": "to_vm"}),
                        )
                        .await
                        {
                            Ok(result) => result,
                            Err(error) => format!("Error: {error}"),
                        }
                    } else if args.get("expected_absent").is_some() || args.get("expected_sha256").is_some() {
                        // Versioned contract: single-file transactional write with a
                        // precondition. Never falls back to the raw write.
                        crate::tools::apply_patch::write_checked(user_id, &tc.id, &args)
                            .await
                            .unwrap_or_else(|error| format!("Error: {error}"))
                    } else {
                        let path = args["path"].as_str().unwrap_or("");
                        let content = args["content"].as_str().unwrap_or("");
                        match crate::tools::write_file::write_file(path, content).await {
                            Ok(_) => format!("File written: {}", path),
                            Err(e) => format!("Error: {}", e),
                        }
                    } // end else (shared mode)
                }
                #[cfg(feature = "legacy_file_ops")]
                "edit_file" => {
                    if mode == DispatchMode::Agent && crate::runtime::vm::guest_backend(plugins) {
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
                        match invoke_guest(plugins, db, user_id, &tc.id,
                            "vm_shell",
                            &serde_json::json!({"command": cmd}),
                        )
                        .await
                        {
                            Ok(result) => result,
                            Err(error) => format!("Error: {error}"),
                        }
                    } else {
                        praxis_legacy_file_ops::execute(&tc.function.name, &args)
                            .await
                            .unwrap_or_else(|error| format!("Error: {error}"))
                    } // end else (shared mode)
                }
                #[cfg(feature = "legacy_file_ops")]
                "read_file" => {
                    if mode == DispatchMode::Agent && crate::runtime::vm::guest_backend(plugins) {
                        let path = args["path"].as_str().unwrap_or("");
                        match invoke_guest(plugins, db, user_id, &tc.id,
                            "vm_file_read",
                            &serde_json::json!({"path": path}),
                        )
                        .await
                        {
                            Ok(result) => result,
                            Err(error) => format!("Error: {error}"),
                        }
                    } else {
                        praxis_legacy_file_ops::execute(&tc.function.name, &args)
                            .await
                            .unwrap_or_else(|error| format!("Error: {error}"))
                    } // end else (shared mode)
                }
                "discord_upload_file" => {
                    let settings_upload_ch = if mode == DispatchMode::Agent {
                        db.load_context(user_id)
                            .ok()
                            .and_then(|c| c.settings.upload_channel_id.clone())
                            .filter(|s| !s.is_empty())
                    } else {
                        None
                    };
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
                    let settings_upload_ch = if mode == DispatchMode::Agent {
                        db.load_context(user_id)
                            .ok()
                            .and_then(|c| c.settings.upload_channel_id.clone())
                            .filter(|s| !s.is_empty())
                    } else {
                        None
                    };
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
                #[cfg(feature = "vision")]
                "understand_image" => {
                    let result = crate::tools::understand_image::run(&args).await;
                    // Store image content_parts alongside the result
                    // We return the text, but the caller needs to handle content_parts separately
                    // Use a special marker to indicate this tool returned image data
                    break 'step Step::Text(serde_json::json!({
                        "text": result.text,
                        "content_parts": result.content_parts.iter().map(|cp| {
                            serde_json::to_value(cp).unwrap_or_default()
                        }).collect::<Vec<_>>()
                    })
                    .to_string());
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
                            plugins, user_id, caption, vm_name,
                        )
                        .await
                        {
                            Ok(result) => result,
                            Err(e) => format!("Error: {}", e),
                        }
                    } else {
                        match crate::tools::discord_interactive::send_screenshot_to_discord(
                            plugins, user_id, channel_id, caption, vm_name,
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
                        break 'step Step::Text("Error: 'questions' field is required and must be an array.".to_string());
                    };
                    if arr.is_empty() {
                        break 'step Step::Text("Error: 'questions' array must contain at least one question.".to_string());
                    }
                    let mut questions: Vec<(String, String, Vec<String>)> = Vec::new();
                    for (i, q) in arr.iter().enumerate() {
                        if q.get("options").is_some() {
                            break 'step Step::Text(format!("Error: Question {}: use 'suggestions' with plain strings, not 'options' with objects.", i + 1));
                        }
                        let label = q["label"].as_str().filter(|s| !s.is_empty());
                        let Some(label) = label else {
                            break 'step Step::Text(format!(
                                "Error: Question {} is missing a non-empty 'label' field.",
                                i + 1
                            ));
                        };
                        let text = q["question"].as_str().filter(|s| !s.is_empty());
                        let Some(text) = text else {
                            break 'step Step::Text(format!(
                                "Error: Question {} (label: '{}') is missing a non-empty 'question' field.",
                                i + 1,
                                label
                            ));
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
                    tracing::info!(
                        timeout_secs = timeout,
                        is_web,
                        "ask_questions: waiting for responses"
                    );

                    if is_web {
                        match crate::tools::web_interactive::ask_questions_web(user_id, &questions, timeout)
                            .await
                        {
                            Ok(result) => result,
                            Err(e) => format!("Error: {}", e),
                        }
                    } else {
                        if channel_id.is_empty() {
                            break 'step Step::Text("Error: No channel_id provided and no originating channel found. Please specify a channel_id.".to_string());
                        }
                        if paired_discord_user_id.is_empty() {
                            break 'step Step::Text("Error: No Discord user pairing found.".to_string());
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
                "update_template" => crate::tools::update_template::run(db, &args)
                    .await
                    .unwrap_or_else(|e| format!("Error: {}", e)),
                _ => {
                    format!("Error: No native adapter for '{}'", tc.function.name)
                }
            })
        };
        match step {
            Step::Text(text) => return text,
            Step::Lower(resolved) => {
                // Belt and braces: declared mappings cannot chain lowerings
                // today, and the operation itself refuses deeper ones.
                if depth >= crate::tools::builtin_operations::MAX_LOWER_DEPTH {
                    return "Error: Decision IR lowering is nested too deeply; decision not executed".into();
                }
                depth += 1;
                call = resolved;
            }
        }
    }
}

async fn invoke_guest(plugins: &crate::plugins::PluginRegistry, db: &crate::db::Database, user: &str, call: &str, name: &str, args: &serde_json::Value) -> anyhow::Result<String> {
    let owner = crate::tools::catalog::owner(plugins, name)?;
    crate::tools::catalog::require_enabled(db, &owner, name)?;
    let secrets = plugins.secrets_for_tool(name, &crate::db::secrets::get_secrets());
    plugins.execute_tool_with_host(db, user, call, name, args, None, Some(&secrets)).await
}
