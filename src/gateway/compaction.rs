//! Pre-request compaction shared by chat and agent turns. Commit only after a
//! complete successful summary; failed inference never deletes conversation rows.
use super::{GatewayState, llm::provider::{ChatMessage, ChatRequest, ThinkingMode}};
use crate::db::messages::Message;

pub fn threshold(settings: &crate::db::contexts::ContextSettings) -> usize {
    settings.compaction_token_limit.unwrap_or(8_000)
        .min(settings.history_token_limit.unwrap_or(crate::db::messages::DEFAULT_HISTORY_TOKENS) / 2).max(1)
}

pub async fn before_request(state: &GatewayState, user: &str) -> anyhow::Result<()> {
    let ctx = state.db.load_context(user)?;
    if !ctx.settings.compaction_enabled { return Ok(()); }
    let (messages,tokens) = state.db.get_messages_with_token_budget(user,usize::MAX)?;
    let threshold = threshold(&ctx.settings);
    if tokens <= threshold { return Ok(()); }
    let Some(last_user) = messages.iter().rposition(|m|m.role == "user").filter(|i|*i>0) else { return Ok(()) };
    if !super::task_control::claim_compaction(user) { return Ok(()); }
    let prefix = &messages[..last_user];
    compaction_event(state, &ctx, "running", tokens)?;
    crate::dashboard::stream::send(user,"feedback","Compacting older complete turns; keeping the current request and tool chain.");
    match summarize(state,user,prefix,&ctx.settings.compaction_summary,ctx.settings.compaction_template.as_deref()).await {
        Ok(summary) => {
            state.db.commit_compaction(user,&ctx.session_id,&ctx.settings.compaction_summary,&summary,prefix)?;
            compaction_event(state, &ctx, "completed", tokens)?;
            crate::dashboard::stream::send(user,"feedback","Compaction saved: goal, key insights and handoff. Current turn and saved tool outputs preserved.");
        }
        Err(error) => {
            compaction_event(state, &ctx, "failed", tokens)?;
            tracing::warn!(user_id=user,error=%error,"Compaction failed; history unchanged");
            crate::dashboard::stream::send(user,"feedback","Auto-compaction failed; no history was deleted. Safe request budgeting remains active.");
        }
    }
    Ok(())
}

fn compaction_event(state: &GatewayState, ctx: &crate::db::contexts::Context, status: &str, before: usize) -> anyhow::Result<()> {
    let limits = super::telemetry::limits(&state.db, ctx)?;
    let payload = serde_json::json!({"status":status,"tokens_before_estimate":before,"limits":limits});
    state.db.record_execution_event(ctx, "compaction", &payload)?;
    crate::dashboard::stream::send(&ctx.user_id, "compaction", &payload.to_string());
    Ok(())
}

pub async fn summarize(state:&GatewayState,user:&str,messages:&[Message],previous:&str,template:Option<&str>) -> anyhow::Result<String> {
    let ctx=state.db.load_context(user)?;
    let path=super::templates::resolve_template(&std::path::Path::new(&state.config.root_dir).join("templates"),template.unwrap_or("compaction"))?;
    let mut transcript=String::new();
    for m in messages {
        let content = if m.role=="tool" && m.content.chars().count()>2000 {
            let head:String=m.content.chars().take(700).collect();
            let tail:String=m.content.chars().rev().take(1000).collect::<Vec<_>>().into_iter().rev().collect();
            format!("{head}\n[Tool body excerpt only; full snapshot remains stored. Preserve output_id references below.]\n{tail}")
        } else {m.content.clone()};
        transcript.push_str(&format!("\n[message {} {} tool={:?}]\n{}\n",m.id.unwrap_or(0),m.role,m.tool_name,content));
        if let Some(calls)=&m.tool_calls { transcript.push_str(&serde_json::to_string(calls)?); }
    }
    // Bound each request independently. Carry the rolling handoff into each
    // chunk; never summarize just the newest suffix and erase unseen history.
    let chars:Vec<char>=transcript.chars().collect();
    anyhow::ensure!(chars.len() <= 16*6000,"Compaction backlog too large for one bounded task; history unchanged");
    let mut summary=previous.to_string();
    for chunk in chars.chunks(6000) {
        let text:String=chunk.iter().collect();
        let prompt=super::poml::render_strict(&path.to_string_lossy(),&serde_json::json!({
            "conversation_text":text,"previous_summary":summary
        })).await?;
        let request=ChatRequest {
            messages:vec![ChatMessage {role:"user".into(),content:Some(prompt),reasoning_content:None,
                content_parts:None,tool_calls:None,tool_call_id:None,tool_name:None}],
            tools:None,temperature:Some(0.1),max_tokens:Some(1024),model:ctx.settings.model.clone(),
            vision_provider:None,vision_model:None,thinking:Some(ThinkingMode::Off),
        };
        let cancel=super::task_control::cancellation(user).unwrap_or_default();
        let response=state.llm.get().chat_controlled(request,ctx.settings.provider.as_deref(),None,&cancel).await?;
        anyhow::ensure!(response.tool_calls.is_none(),"Compaction must not execute tools");
        summary=response.content.unwrap_or_default();
        anyhow::ensure!(!summary.trim().is_empty() && !summary.contains(super::task_control::TEMPLATE_OMITTED_MARKER),"Compaction did not produce a valid handoff");
    }
    anyhow::ensure!(!summary.trim().is_empty(),"No compaction summary");
    Ok(summary)
}
