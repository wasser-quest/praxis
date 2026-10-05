//! Trusted plugin definitions describe effects; model input and handler output
//! never supply verification authority. Compensation is explicit, not atomic.
use super::{Plugin, PluginHandler, PluginTool};
use crate::gateway::{
    action_contracts::{self, CheckContract, CheckEvidence, Outcome},
    resource_snapshots::ResourceSnapshot,
    task_control,
};
use praxis_plugin_api::process::TerminalResult;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{collections::HashMap, path::Path, time::Duration};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EffectClass {
    ReadOnly,
    Verification,
    WorkspaceWrite,
    ExternalWrite,
}
impl EffectClass {
    fn mutates(self) -> bool {
        matches!(self, Self::WorkspaceWrite | Self::ExternalWrite)
    }
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Idempotency {
    NonIdempotent,
    Idempotent,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ActionContract {
    pub effect: EffectClass,
    pub idempotency: Idempotency,
    pub timeout_secs: u64,
    #[serde(default)]
    pub preconditions: Vec<CheckContract>,
    pub postconditions: Vec<CheckContract>,
    #[serde(default)]
    pub compensation: Option<Compensation>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Compensation {
    pub handler: PluginHandler,
    pub postconditions: Vec<CheckContract>,
    pub timeout_secs: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConditionReceipt {
    pub phase: &'static str,
    pub index: usize,
    pub outcome: Outcome,
    pub exit_code: Option<i32>,
    pub resources: Option<ResourceSnapshot>,
    pub workspace_revision: Option<String>,
    pub output: Option<TerminalResult>,
}
#[derive(Debug, Clone, Serialize)]
pub struct ActionReceipt {
    pub id: String,
    pub task_id: String,
    pub call_id: String,
    pub action: String,
    pub contract_sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub registry_revision: Option<String>,
    pub effect: EffectClass,
    pub idempotency: Idempotency,
    pub revision: u64,
    pub workspace_revision: Option<String>,
    pub attempted: bool,
    pub outcome: &'static str,
    pub failure: Option<&'static str>,
    pub verified: bool,
    /// The package that defined the check policy, as `<id>@<version>`. The
    /// kernel ran the checks and observed their exit codes and resources.
    pub verified_by: String,
    pub compensation_attempted: bool,
    pub compensation_verified: bool,
    pub conditions: Vec<ConditionReceipt>,
}

pub(crate) fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
fn validate_handler(handler: &PluginHandler) -> anyhow::Result<()> {
    match handler {
        PluginHandler::Service(adapter) => crate::runtime::features::validate_adapter(adapter)?,
        PluginHandler::Verification(_) | PluginHandler::SourceEdit(_) => {}
        PluginHandler::Executable { .. } => anyhow::bail!("Executable protocol v1 supplies raw tools; use a service adapter for action contracts"),
        PluginHandler::Script { path, interpreter } => anyhow::ensure!(
            !path.is_empty()
                && !path.contains('\0')
                && !interpreter.is_empty()
                && !interpreter.contains('\0'),
            "Invalid capability script"
        ),
        PluginHandler::Http { url, method } => {
            let url =
                reqwest::Url::parse(url).map_err(|_| anyhow::anyhow!("Invalid capability URL"))?;
            anyhow::ensure!(
                matches!(url.scheme(), "http" | "https")
                    && url.username().is_empty()
                    && url.password().is_none()
                    && url.fragment().is_none(),
                "Invalid capability URL"
            );
            anyhow::ensure!(
                matches!(method.to_uppercase().as_str(), "GET" | "POST"),
                "Capability HTTP supports GET or POST"
            );
        }
        PluginHandler::Builtin { .. } => {
            anyhow::bail!("Builtin capabilities require a semantic adapter")
        }
    }
    Ok(())
}
fn validate_checks(checks: &[CheckContract], required: bool) -> anyhow::Result<()> {
    anyhow::ensure!(
        checks.len() <= 8 && (!required || !checks.is_empty()),
        "Conditions require 1..8 checks (preconditions may be empty)"
    );
    for check in checks {
        check.validate()?;
    }
    Ok(())
}
pub(crate) fn validate_tool(plugin: &str, tool: &PluginTool) -> anyhow::Result<()> {
    if let PluginHandler::Executable { path, timeout_secs } = &tool.handler {
        anyhow::ensure!(!path.is_empty() && !path.contains('\0') && (1..=600).contains(timeout_secs), "Invalid executable handler");
        anyhow::ensure!(tool.contract.is_none(), "Executable protocol v1 supplies raw tools; use a service adapter for action contracts");
    }
    if let PluginHandler::Service(adapter) = &tool.handler {
        anyhow::ensure!(
            identifier(plugin) && identifier(&tool.name),
            "Native service names must be identifiers"
        );
        crate::runtime::features::validate_adapter(adapter)?;
    }
    let Some(contract) = &tool.contract else {
        anyhow::ensure!(
            !matches!(
                &tool.handler,
                PluginHandler::Verification(_) | PluginHandler::SourceEdit(_)
            ),
            "Native adapters require an action contract"
        );
        return Ok(());
    };
    if matches!(&tool.handler, PluginHandler::Verification(_)) {
        anyhow::ensure!(
            contract.effect == EffectClass::Verification,
            "Native verification requires effect=verification"
        );
    }
    if matches!(&tool.handler, PluginHandler::SourceEdit(_)) {
        anyhow::ensure!(
            contract.effect == EffectClass::WorkspaceWrite
                && contract.idempotency == Idempotency::NonIdempotent
                && contract.compensation.is_none(),
            "Native source edits require workspace_write, non_idempotent and native rollback"
        );
        let fields = ["path", "expected_sha256", "content"];
        anyhow::ensure!(
            tool.parameters["properties"]
                .as_object()
                .is_some_and(|props| props.len() == fields.len()
                    && fields
                        .iter()
                        .all(|key| props.get(*key).is_some_and(|s| s["type"] == "string")))
                && tool.parameters["required"]
                    .as_array()
                    .is_some_and(|required| required.len() == fields.len()
                        && fields
                            .iter()
                            .all(|key| required.contains(&Value::String((*key).into())))),
            "Native source edit input requires exactly path, expected_sha256 and content strings"
        );
    }
    anyhow::ensure!(
        identifier(plugin) && identifier(&tool.name),
        "Capability names must be identifiers"
    );
    anyhow::ensure!(
        (1..=300).contains(&contract.timeout_secs),
        "Capability timeout_secs must be 1..300"
    );
    validate_handler(&tool.handler)?;
    validate_checks(&contract.preconditions, false)?;
    validate_checks(&contract.postconditions, true)?;
    if let Some(cleanup) = &contract.compensation {
        anyhow::ensure!(
            !matches!(
                &cleanup.handler,
                PluginHandler::Verification(_)
                    | PluginHandler::SourceEdit(_)
                    | PluginHandler::Service(_)
            ),
            "Native adapters cannot perform compensation"
        );
        anyhow::ensure!(
            contract.effect.mutates() && (1..=300).contains(&cleanup.timeout_secs),
            "Compensation requires a mutating effect and timeout 1..300"
        );
        validate_handler(&cleanup.handler)?;
        validate_checks(&cleanup.postconditions, true)?;
    }
    validate_schema(&tool.parameters)
}

// A deliberately small, fail-closed schema subset. No silently ignored JSON
// Schema features; extend this with adapters rather than accepting anything.
fn validate_schema(schema: &Value) -> anyhow::Result<()> {
    let object = schema
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("Capability schema must be an object"))?;
    anyhow::ensure!(
        object.keys().all(|k| matches!(
            k.as_str(),
            "type" | "properties" | "required" | "additionalProperties" | "description"
        )) && schema["type"] == "object"
            && schema["additionalProperties"] == false,
        "Capability schema requires a closed object"
    );
    let properties = schema["properties"]
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("Capability properties required"))?;
    anyhow::ensure!(properties.len() <= 128, "Too many capability properties");
    for definition in properties.values() {
        let spec = definition
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("Invalid property schema"))?;
        let kind = definition["type"].as_str().unwrap_or("");
        anyhow::ensure!(
            matches!(kind, "string" | "boolean" | "integer" | "number"),
            "Only flat primitive capability inputs are supported"
        );
        anyhow::ensure!(
            spec.keys()
                .all(|k| matches!(k.as_str(), "type" | "description" | "enum")
                    || (kind == "string" && matches!(k.as_str(), "minLength" | "maxLength"))
                    || (matches!(kind, "integer" | "number")
                        && matches!(k.as_str(), "minimum" | "maximum"))),
            "Unsupported capability schema keyword"
        );
        for key in ["minLength", "maxLength"] {
            if let Some(v) = spec.get(key) {
                anyhow::ensure!(v.as_u64().is_some(), "Invalid string bound");
            }
        }
        for key in ["minimum", "maximum"] {
            if let Some(v) = spec.get(key) {
                anyhow::ensure!(
                    v.as_f64().is_some_and(f64::is_finite),
                    "Invalid numeric bound"
                );
            }
        }
        if let Some(values) = spec.get("enum") {
            let values = values
                .as_array()
                .ok_or_else(|| anyhow::anyhow!("Invalid enum"))?;
            anyhow::ensure!(
                !values.is_empty()
                    && values.len() <= 128
                    && values.iter().all(|v| primitive_type(kind, v)),
                "Invalid enum values"
            );
        }
    }
    if let Some(required) = object.get("required") {
        let required = required
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("Invalid required fields"))?;
        anyhow::ensure!(
            required
                .iter()
                .all(|v| v.as_str().is_some_and(|k| properties.contains_key(k))),
            "Required field not declared"
        );
    }
    Ok(())
}
fn primitive_type(kind: &str, value: &Value) -> bool {
    match kind {
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "integer" => value.is_i64() || value.is_u64(),
        "number" => value.is_number(),
        _ => false,
    }
}
fn validate_input(schema: &Value, args: &Value) -> anyhow::Result<()> {
    anyhow::ensure!(
        serde_json::to_vec(args)?.len() <= 1024 * 1024,
        "Capability input too large"
    );
    let object = args
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("Capability input must be an object"))?;
    let properties = schema["properties"]
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("Invalid schema"))?;
    if let Some(required) = schema["required"].as_array() {
        anyhow::ensure!(
            required
                .iter()
                .all(|v| v.as_str().is_some_and(|k| object.contains_key(k))),
            "Required capability input missing"
        );
    }
    for (key, value) in object {
        let spec = properties
            .get(key)
            .ok_or_else(|| anyhow::anyhow!("Unknown capability input field"))?;
        anyhow::ensure!(
            primitive_type(spec["type"].as_str().unwrap_or(""), value),
            "Invalid capability input type"
        );
        if let Some(values) = spec["enum"].as_array() {
            anyhow::ensure!(values.contains(value), "Capability input outside enum");
        }
        if let Some(s) = value.as_str() {
            let n = s.chars().count() as u64;
            anyhow::ensure!(
                spec["minLength"].as_u64().is_none_or(|min| n >= min)
                    && spec["maxLength"].as_u64().is_none_or(|max| n <= max),
                "Capability string outside bounds"
            );
        }
        if let Some(n) = value.as_f64() {
            anyhow::ensure!(
                spec["minimum"].as_f64().is_none_or(|min| n >= min)
                    && spec["maximum"].as_f64().is_none_or(|max| n <= max),
                "Capability number outside bounds"
            );
        }
    }
    Ok(())
}

type Failure = &'static str;
fn truncate(text: &mut String) -> bool {
    let mut end = 65536.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let lost = end < text.len();
    text.truncate(end);
    lost
}
pub(crate) async fn conditions(
    checks: &[CheckContract],
    phase: &'static str,
    root: &Path,
    cancel: &CancellationToken,
    receipt: &mut ActionReceipt,
) -> Result<Vec<CheckEvidence>, Failure> {
    let mut evidence = Vec::new();
    for (index, check) in checks.iter().enumerate() {
        let (outcome, output, snapshot) = match action_contracts::execute(check, root, cancel).await
        {
            Ok((outcome, mut output, snapshot)) => {
                output.stdout_truncated |= truncate(&mut output.stdout);
                output.stderr_truncated |= truncate(&mut output.stderr);
                (outcome, Some(output), snapshot)
            }
            Err(outcome) => (outcome, None, CheckEvidence::default()),
        };
        receipt.conditions.push(ConditionReceipt {
            phase,
            index,
            outcome,
            exit_code: output.as_ref().map(|o| o.exit_code),
            resources: snapshot.resources.clone(),
            workspace_revision: snapshot.workspace_revision.clone(),
            output,
        });
        if outcome != Outcome::Passed {
            return Err(match outcome {
                Outcome::Cancelled => "cancelled",
                Outcome::TimedOut => "timed_out",
                _ => "condition_failed",
            });
        }
        evidence.push(snapshot);
    }
    if cancel.is_cancelled() {
        return Err("cancelled");
    }
    if !checks
        .iter()
        .zip(&evidence)
        .all(|(check, snapshot)| snapshot.current(check, root))
    {
        return Err("evidence_stale");
    }
    Ok(evidence)
}

pub(crate) async fn execute(
    plugin: &Plugin,
    tool: &PluginTool,
    user: &str,
    call: &str,
    args: &Value,
    context: Option<&Value>,
    secrets: Option<&HashMap<String, String>>,
) -> anyhow::Result<String> {
    execute_with_service(plugin, tool, user, call, args, context, secrets, None).await
}

pub(crate) async fn execute_with_service(
    plugin: &Plugin,
    tool: &PluginTool,
    user: &str,
    call: &str,
    args: &Value,
    context: Option<&Value>,
    secrets: Option<&HashMap<String, String>>,
    service: Option<&crate::runtime::features::ServiceInvocation>,
) -> anyhow::Result<String> {
    validate_tool(&plugin.name, tool)?;
    validate_input(&tool.parameters, args)?;
    anyhow::ensure!(
        !matches!(&tool.handler, PluginHandler::Service(_)) || service.is_some(),
        "Native service requires a host-issued invocation context"
    );
    let contract = tool
        .contract
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Missing capability contract"))?;
    if matches!(&tool.handler, PluginHandler::SourceEdit(_)) {
        return crate::tools::apply_patch::source_edit::run(plugin, tool, user, call, args).await;
    }
    let cancel = task_control::cancellation(user)
        .ok_or_else(|| anyhow::anyhow!("Capability requires an active task"))?;
    anyhow::ensure!(!cancel.is_cancelled(), "Task cancelled");
    let root = action_contracts::action_root(user)?;
    let admission_deadline = async {
        if let Some(service) = service {
            tokio::time::sleep_until(service.context.deadline()).await;
        } else {
            std::future::pending::<()>().await;
        }
    };
    let _operation = tokio::select! {
        biased;
        _ = cancel.cancelled() => anyhow::bail!("Task cancelled"),
        _ = admission_deadline => anyhow::bail!("Native service invocation timed out before execution"),
        lock = crate::tools::apply_patch::FILE_OPERATIONS.lock() => lock,
    };
    let workspace = crate::tools::apply_patch::journal::ready(&root)?;
    if contract.effect.mutates() {
        task_control::invalidate_workspace(&root)?;
    }
    let (ticket, mut receipt) = start_receipt(plugin, tool, user, call)?;
    let scoped: HashMap<String, String> = plugin
        .secrets
        .iter()
        .filter_map(|key| {
            secrets
                .and_then(|values| values.get(key))
                .map(|v| (key.clone(), v.clone()))
        })
        .collect();
    let mut result = Value::Null;
    let lifecycle = async {
        conditions(
            &contract.preconditions,
            "precondition",
            &root,
            &cancel,
            &mut receipt,
        )
        .await?;
        if let Some(service) = service {
            service.ready().map_err(|error| crate::runtime::features::failure(&error))?;
        }
        if contract.effect.mutates() {
            if let Some(workspace) = &workspace {
                workspace
                    .advance_revision()
                    .map_err(|_| "workspace_error")?;
            }
        }
        receipt.workspace_revision = crate::tools::apply_patch::journal::workspace_revision(&root)
            .map_err(|_| "workspace_error")?;
        receipt.attempted = true;
        result = handler(&tool.handler, args, context, &scoped, &root, service).await?;
        let evidence = conditions(
            &contract.postconditions,
            "postcondition",
            &root,
            &cancel,
            &mut receipt,
        )
        .await?;
        if cancel.is_cancelled() {
            return Err("cancelled");
        }
        receipt.outcome = "committed";
        receipt.verified = true;
        action_contracts::publish_action(
            user,
            &ticket,
            &receipt,
            &contract.postconditions,
            &evidence,
        )
        .map_err(|_| "evidence_stale")?;
        Ok::<_, Failure>(())
    };
    let execution = tokio::select! { biased; _ = cancel.cancelled() => Err("cancelled"), result = tokio::time::timeout(Duration::from_secs(contract.timeout_secs), lifecycle) => result.unwrap_or(Err("timed_out")) };
    if let Err(failure) = execution {
        action_contracts::revoke_action(user, &ticket, &receipt.action);
        receipt.verified = false;
        receipt.failure = Some(failure);
        receipt.outcome = if receipt.attempted {
            "failed"
        } else {
            "precondition_failed"
        };
        if receipt.attempted {
            if let Some(cleanup) = &contract.compensation {
                receipt.compensation_attempted = true;
                let payload = serde_json::json!({"action_id":receipt.id,"input":args,"result":result,"failure":failure});
                // /stop cancels the requested action, not already-needed cleanup.
                let cleanup_token = CancellationToken::new();
                let compensation = async {
                    handler(&cleanup.handler, &payload, context, &scoped, &root, None).await?;
                    conditions(
                        &cleanup.postconditions,
                        "compensation",
                        &root,
                        &cleanup_token,
                        &mut receipt,
                    )
                    .await?;
                    Ok::<_, Failure>(())
                };
                let ok =
                    tokio::time::timeout(Duration::from_secs(cleanup.timeout_secs), compensation)
                        .await
                        .is_ok_and(|r| r.is_ok());
                let uncertain_external = contract.effect == EffectClass::ExternalWrite
                    && matches!(failure, "handler_failed" | "timed_out" | "cancelled");
                receipt.compensation_verified = ok && !uncertain_external;
                receipt.outcome = if receipt.compensation_verified {
                    "compensated"
                } else {
                    "compensation_failed"
                };
            }
        }
    }
    Ok(serde_json::json!({"receipt":receipt,"result":result}).to_string())
}

pub(crate) fn start_receipt(
    plugin: &Plugin,
    tool: &PluginTool,
    user: &str,
    call: &str,
) -> anyhow::Result<(action_contracts::ActionTicket, ActionReceipt)> {
    crate::gateway::task_control::check_capability(user, plugin, tool)?;
    let contract = tool
        .contract
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Missing capability contract"))?;
    let key = format!("{}/{}", plugin.name, tool.name);
    let fingerprint = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&(
            plugin.name.as_str(),
            plugin.version.as_str(),
            tool
        ))?)
    );
    let ticket = action_contracts::start_action(user, call, &key, &fingerprint)?;
    let receipt = ActionReceipt {
        id: uuid::Uuid::new_v4().to_string(),
        task_id: ticket.task_id.clone(),
        call_id: call.into(),
        action: key,
        contract_sha256: fingerprint,
        registry_revision: crate::gateway::task_control::registry_revision(user),
        effect: contract.effect,
        idempotency: contract.idempotency,
        revision: ticket.revision,
        workspace_revision: None,
        attempted: false,
        outcome: "precondition_failed",
        failure: None,
        verified: false,
        verified_by: format!("{}@{}", plugin.name, plugin.version),
        compensation_attempted: false,
        compensation_verified: false,
        conditions: Vec::new(),
    };
    Ok((ticket, receipt))
}

async fn handler(
    handler: &PluginHandler,
    args: &Value,
    context: Option<&Value>,
    secrets: &HashMap<String, String>,
    root: &Path,
    service: Option<&crate::runtime::features::ServiceInvocation>,
) -> Result<Value, Failure> {
    match handler {
        PluginHandler::Service(_) => service
            .ok_or("handler_failed")?
            .invoke(args)
            .await
            .map_err(|error| crate::runtime::features::failure(&error)),
        // The action's configured postconditions are the operation. Execute them
        // exactly once in the shared lifecycle; only their fresh receipts prove
        // success. This acknowledgement carries no verification authority.
        PluginHandler::Verification(_) => Ok(serde_json::json!({"requested":true})),
        PluginHandler::SourceEdit(_) => Err("handler_failed"), // native transaction dispatch only
        PluginHandler::Http { url, method } => {
            let client = crate::branding::client_builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|_| "handler_failed")?;
            let request = if method.eq_ignore_ascii_case("POST") {
                client.post(url).json(args)
            } else {
                client
                    .get(url)
                    .query(args.as_object().ok_or("handler_failed")?)
            };
            let mut response = request.send().await.map_err(|_| "handler_failed")?;
            if !response.status().is_success() {
                return Err("handler_failed");
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|_| "handler_failed")? {
                if bytes.len() + chunk.len() > 1024 * 1024 {
                    return Err("handler_failed");
                }
                bytes.extend_from_slice(&chunk);
            }
            serde_json::from_slice(&bytes).map_err(|_| "handler_failed")
        }
        PluginHandler::Script { path, interpreter } => {
            let mut command = tokio::process::Command::new(interpreter);
            command
                .arg(path)
                .current_dir(root)
                .env(
                    "PLUGIN_ARGS",
                    serde_json::to_string(args).map_err(|_| "handler_failed")?,
                )
                .env(
                    "PLUGIN_CONTEXT",
                    context.map(Value::to_string).unwrap_or_else(|| "{}".into()),
                )
                .env(
                    "PLUGIN_SECRETS",
                    serde_json::to_string(secrets).map_err(|_| "handler_failed")?,
                )
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .kill_on_drop(true);
            #[cfg(unix)]
            {
                use std::os::unix::process::CommandExt;
                command.as_std_mut().process_group(0);
            }
            let mut child = command.spawn().map_err(|_| "handler_failed")?;
            struct Group(Option<u32>);
            impl Drop for Group {
                fn drop(&mut self) {
                    #[cfg(unix)]
                    if let Some(id) = self.0 {
                        unsafe {
                            libc::kill(-(id as i32), libc::SIGKILL);
                        }
                    }
                }
            }
            let group = Group(child.id());
            let stdout = child.stdout.take().ok_or("handler_failed")?;
            let stderr = child.stderr.take().ok_or("handler_failed")?;
            let (out, err, status) = tokio::join!(
                praxis_plugin_api::process::capture(stdout),
                praxis_plugin_api::process::capture(stderr),
                child.wait()
            );
            drop(group);
            let (out, truncated) = out.map_err(|_| "handler_failed")?;
            let (_, err_truncated) = err.map_err(|_| "handler_failed")?;
            if !status.map_err(|_| "handler_failed")?.success()
                || truncated
                || err_truncated
                || out.len() > 1024 * 1024
            {
                return Err("handler_failed");
            }
            Ok(serde_json::from_str(out.trim())
                .unwrap_or_else(|_| Value::String(out.trim().into())))
        }
        PluginHandler::Builtin { .. } | PluginHandler::Executable { .. } => Err("handler_failed"),
    }
}
