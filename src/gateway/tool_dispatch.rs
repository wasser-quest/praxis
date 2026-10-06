//! One execution boundary for chat, agent/WebSocket and lowered Decision IR.
//! Ingress owns task charging and result archival; this layer resolves owners,
//! rechecks flags/cancellation and preserves the native compatibility adapters.
use super::llm::provider::ToolCall;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DispatchMode {
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
                                mode,
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
