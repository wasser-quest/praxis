//! Management readiness is independent of inference setup. Validate a planned
//! task without changing context/history, binding receipts or waking features.
use super::GatewayState;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct SetupError {
    pub code: &'static str,
    pub message: String,
    pub operator_action: &'static str,
    pub retryable: bool,
}
impl std::fmt::Display for SetupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Inference setup error [{}]: {}. {}",
            self.code, self.message, self.operator_action
        )
    }
}
impl std::error::Error for SetupError {}

#[derive(Serialize)]
pub struct Readiness {
    pub ready: bool,
    pub error: Option<SetupError>,
}

/// Configuration availability only, never a claim that an endpoint/model is
/// reachable. Explicit provider login keeps its existing endpoint probe.
pub fn readiness(state: &GatewayState) -> Readiness {
    let error = check_provider(state, None).err();
    Readiness {
        ready: error.is_none(),
        error,
    }
}

fn check_provider(state: &GatewayState, provider: Option<&str>) -> Result<(), SetupError> {
    state.llm.validate_request_configuration(provider).map_err(|error| SetupError {
        code:"provider_not_configured",
        message:error.to_string(),
        operator_action:"Configure the selected provider with /login or /v1/providers/login, or select a configured provider in context; then start a new task. Management remains available. This setup error is not retryable",
        retryable:false,
    })
}

pub(crate) fn check_task(
    state: &GatewayState,
    user: &str,
    input: &str,
    channel: Option<&str>,
    workflow_default: Option<&str>,
) -> anyhow::Result<()> {
    super::task_control::check_registry(user, &state.plugins)?;
    let root = std::path::Path::new(&state.config.root_dir);
    let mut original = state.db.load_context(user)?;
    if original.settings.sm_file.is_none() && original.sm_file.is_none() {
        original.sm_file = workflow_default.map(str::to_owned);
    }
    super::prompt::reset_completion(&mut original, root)?;
    let candidate = super::workflow_actions::plan(root, &original, input, &state.plugins, channel)?;
    let workflow = crate::sm::load_file_in(
        &root.join("contexts"),
        super::prompt::workflow_name(&candidate),
    )
    .map_err(|error| anyhow::anyhow!("Workflow routing failed: {error}"))?;
    let workspace = state.config.workspace_root()?;
    if super::task_control::cancellation(user).is_some() {
        super::task_control::pin_workspace(user, &workspace)?;
    }
    workflow.workspace.check(&workspace)?;
    super::workflow_preflight::validate(
        &state.db,
        &state.plugins,
        super::prompt::workflow_name(&candidate),
        &workflow,
        &candidate,
    )?;
    check_provider(state, candidate.settings.provider.as_deref())?;
    Ok(())
}
