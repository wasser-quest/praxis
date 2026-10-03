//! Trusted template-entry routing, evaluated on a candidate before persistence.
//! Only a static leading source header is authority. Rendered prompts, user text,
//! quoted examples and tool/model output are NEVER scanned for these actions.
use crate::{db::contexts::Context,plugins::PluginRegistry};
use std::{collections::HashSet,io::Read,path::Path};
use anyhow::{ensure,Context as _};

fn workflow_directive(path:&Path,prefix:&str)->anyhow::Result<Option<String>> {
    let mut bytes=vec![];
    std::fs::File::open(path)?.take(8192).read_to_end(&mut bytes)?;
    let source=String::from_utf8_lossy(&bytes);
    let source=source.trim_start_matches('\u{feff}').trim_start();
    let Some(header)=source.strip_prefix("<!-- praxis:on-enter") else {return Ok(None);};
    let end=header.find("-->").context("Template entry header must close within 8 KiB")?;
    let header=header[..end].replace("{{settings.tag_prefix}}",prefix);
    ensure!(!header.contains("{{"),"Template entry directives must be static; only settings.tag_prefix is substituted");
    let parsed=crate::tags::parse_tags_with_prefix(&header,prefix)?;
    ensure!(parsed.cleaned_response.trim().is_empty() && parsed.actions.len()==1,"Entry header currently expects exactly one workflow-selection tag");
    let action=&parsed.actions[0];
    ensure!(action.tag=="sm","Unsupported template entry action; no action was executed");
    let name=action.value.as_deref().filter(|s|!s.is_empty()).context("sm tag needs a workflow name")?;
    Ok(Some(name.strip_suffix(".sm").unwrap_or(name).to_string()))
}

/// Plan a bounded fixed point: SM → template → optional workflow selection.
/// No DB/network/write operations; errors leave the caller's context untouched.
pub fn plan(root:&Path,original:&Context,input:&str,plugins:&PluginRegistry,channel:Option<&str>)->anyhow::Result<Context> {
    // A MANUAL system_template choice sticks and disables workflow routing.
    // A template that some state of the active workflow selects is the
    // workflow's own output, so routing must keep running; otherwise the first
    // state switch would freeze the context in that state forever.
    let skip_routing = match original.settings.system_template.as_deref() {
        None | Some("standard") => false,
        Some(template) => {
            let workflow = crate::sm::load_file_in(&root.join("contexts"), super::prompt::workflow_name(original)).ok();
            !workflow.is_some_and(|sm| sm.is_graph() || sm.states.values().any(|s| s.variables.get("settings.system_template").is_some_and(|t| t == template)))
        }
    };

    let mut candidate=original.clone();let mut seen=HashSet::new();
    for _ in 0..4 {
        if skip_routing {
            return Ok(candidate);
        }
        let old_workflow=super::prompt::workflow_name(&candidate).to_string();
        super::prompt::route_context_once(root,&mut candidate,input,plugins,channel)?;
        let workflow=super::prompt::workflow_name(&candidate).to_string();
        let template=candidate.settings.system_template.as_deref().unwrap_or("standard").to_string();
        ensure!(seen.insert((workflow.clone(),candidate.active_state.clone(),template.clone())),"Workflow/template entry cycle detected; no changes committed");
        if workflow!=old_workflow {
            candidate.active_state=None;candidate.settings.active_state=None;
            candidate.settings.decision_profile=None;candidate.settings.system_template=None;continue;
        }
        let path=super::templates::resolve_template(&root.join("templates"),&template)?;
        let Some(target)=workflow_directive(&path,&candidate.settings.tag_prefix)? else {return Ok(candidate);};
        if target==workflow.strip_suffix(".sm").unwrap_or(&workflow) {return Ok(candidate);}
        crate::sm::load_file_in(&root.join("contexts"),&target).map_err(|e|anyhow::anyhow!("Invalid workflow target: {e}"))?;
        candidate.sm_file=Some(target.clone());candidate.settings.sm_file=Some(target);
        candidate.active_state=None;candidate.settings.active_state=None;
        candidate.settings.decision_profile=None;candidate.settings.system_template=None;
    }
    anyhow::bail!("Workflow/template entry exceeded four routing steps; no changes committed")
}
