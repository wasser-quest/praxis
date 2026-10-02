//! End-to-end handler regressions: real renderer, synthetic provider, temporary files/DB.
//! No Discord, media provider, credentials or live services are used.
use super::*;
use crate::gateway::llm::{
    error::{ErrorKind, ProviderError},
    provider::{ChatResponse, FunctionCall, LLMProvider, ToolCall},
    resilience::ResilienceConfig,
    LLMRouter,
};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

#[tokio::test]
#[ignore = "Requires the real Microsoft POML CLI; Decision HTTP and main model are scripted"]
async fn decision_routing_rerenders_before_chat_and_agent_requests() {
    use wiremock::{Mock,MockServer,ResponseTemplate,matchers::method};
    std::fs::create_dir_all("decisions").unwrap();
    let server=MockServer::start().await;
    Mock::given(method("POST")).respond_with(|req:&wiremock::Request| {
        let data:serde_json::Value=req.body_json().unwrap();
        let label=if data["contexts"][0].as_str().unwrap().contains("CACHE_EVIDENCE_314") {"bug"} else {"design"};
        ResponseTemplate::new(200).set_body_json(serde_json::json!({"results":[{"decision":{"category":label},"fields":{"category":{"value":label,"probability":0.99}}}]}))
    }).expect(4).mount(&server).await;
    let profile_file=tempfile::Builder::new().prefix("decision-test-").suffix(".json").tempfile_in("decisions").unwrap();
    std::fs::write(profile_file.path(),serde_json::to_vec(&serde_json::json!({
        "endpoint":format!("{}/v1/decision",server.uri()),"model":"scripted-router","instructions":"Synthetic classifier.",
        "schema":{"category":{"type":"enum","choices":["design","bug"]}},"state_field":"category",
        "state_map":{"design":"code_architect","bug":"debugger"}
    })).unwrap()).unwrap();
    for turns in [1,3] {
        let dir=tempfile::tempdir().unwrap();let file=dir.path().join("evidence.txt");std::fs::write(&file,"CACHE_EVIDENCE_314").unwrap();
        let (state,user,requests)=fixture(dir.path(),vec![
            Step::Reply(reply(None,vec![call("evidence","read_file",serde_json::json!({"path":file}))])),
            Step::Reply(reply(Some("Verified."),vec![])),
        ]);
        let mut ctx=state.db.load_context(&user).unwrap();ctx.settings.sm_file=Some("20-tasks".into());
        ctx.settings.decision_profile=Some(profile_file.path().file_stem().unwrap().to_str().unwrap().into());
        ctx.settings.max_llm_turns=Some(turns);ctx.settings.max_tool_calls=Some(4);ctx.settings.compaction_enabled=false;
        state.db.save_context(&ctx).unwrap();
        let result=handle_message(&state,&user,"Inspect the provided evidence.",None).await.unwrap();
        assert!(result.contains("Verified."),"{result}");
        let requests=requests.lock().unwrap();assert_eq!(requests.len(),2);
        assert!(requests[0].messages[0].content.as_deref().unwrap().contains("[STATE_ACTIVE:code_architect]"),"first request, turns={turns}");
        assert!(requests[1].messages[0].content.as_deref().unwrap().contains("[STATE_ACTIVE:debugger]"),"after tool, turns={turns}");
        assert_eq!(state.db.load_context(&user).unwrap().active_state.as_deref(),Some("debugger"));
    }
}

#[cfg(unix)]
#[tokio::test]
#[ignore = "Requires real POML; model and postcondition processes are offline"]
async fn patch_tools_rollback_then_commit_across_all_three_loops() {
    use crate::tools::apply_patch::hash;
    for flow in ["message", "agent", "standalone"] {
        let source=tempfile::Builder::new().prefix("patch-source-").tempfile_in(".").unwrap();
        let expected=tempfile::Builder::new().prefix("patch-expected-").tempfile_in(".").unwrap();
        std::fs::write(source.path(),"before").unwrap(); std::fs::write(expected.path(),"good").unwrap();
        let path=source.path().file_name().unwrap().to_str().unwrap();
        let check=serde_json::json!({"program":"/usr/bin/cmp","args":["-s",path,expected.path().file_name().unwrap().to_str().unwrap()],"timeout_secs":5,"resources":[path,expected.path().file_name().unwrap().to_str().unwrap()]});
        let workflow=tempfile::Builder::new().prefix("patch-workflow-").suffix(".sm").tempfile_in("contexts").unwrap();
        std::fs::write(workflow.path(),format!("[state working]\nsettings.activated_tools = [\"inspect_file\", \"apply_patch\", \"agent_complete\"]\n[checks]\ntests = {check}\n[guards]\n_complete = [tests]\n")).unwrap();
        let edit=|content:&str|serde_json::json!({"edits":[{"path":path,"expected_sha256":hash(b"before"),"content":content}],"checks":["tests"]});
        let dir=tempfile::tempdir().unwrap();
        let (state,user,requests)=fixture(dir.path(),vec![
            Step::Reply(reply(None,vec![call("inspect","inspect_file",serde_json::json!({"path":path}))])),
            Step::Reply(reply(None,vec![call("bad-patch","apply_patch",edit("bad"))])),
            Step::Reply(reply(None,vec![call("good-patch","apply_patch",edit("good"))])),
            Step::Reply(reply(Some("Verified patch completed."),vec![])),
        ]);
        state.db.merge_context(&user,serde_json::json!({"settings.sm_file":workflow.path().file_stem().unwrap().to_str().unwrap(),"settings.max_llm_turns":if flow=="message" {1} else {8},"settings.max_tool_calls":8,"settings.compaction_enabled":false})).unwrap();
        let response=match flow {
            "message"=>handle_message(&state,&user,"Apply a verified edit.",Some("web")).await.unwrap(),
            "agent"=>crate::gateway::agent_loop::run_agent_loop(&state,&user,"Apply a verified edit.",Default::default(),None).await.unwrap().response,
            _=>{let tools=crate::db::tools::to_tool_definitions(&state.db).unwrap(); state.llm.get().chat_with_tools(&state.db,&user,vec![],tools,Some(5),None,None,None).await.unwrap().response},
        };
        assert!(response.contains("Verified patch completed"),"{flow}: {response}");
        assert_eq!(std::fs::read(source.path()).unwrap(),b"good");
        let requests=requests.lock().unwrap(); assert_eq!(requests.len(),4,"{flow}");
        let output=|turn:usize,id:&str|->serde_json::Value {
            let message=requests[turn].messages.iter().find(|m|m.tool_call_id.as_deref()==Some(id)).unwrap();
            serde_json::from_str(message.content.as_deref().unwrap()).unwrap()
        };
        assert_eq!(output(1,"inspect")["sha256"],hash(b"before"));
        assert_eq!(output(2,"bad-patch")["receipt"]["outcome"],"rolled_back");
        assert_eq!(output(2,"bad-patch")["checks"][0]["receipt"]["verified"],false);
        assert_eq!(output(3,"good-patch")["receipt"]["outcome"],"committed");
        assert_eq!(output(3,"good-patch")["checks"][0]["receipt"]["verified"],true);
        assert_eq!(output(3,"good-patch")["checks"][0]["receipt"]["resources"]["sha256"].as_str().unwrap().len(),64);
        for name in ["inspect_file","apply_patch"] { assert!(requests[0].tools.as_ref().unwrap().iter().any(|t|t.function.name==name),"{flow}: {name}"); }
        // Fail visibly if a runtime/fixture leaves generated shipped-path files.
        workflow.close().unwrap();
        source.close().unwrap();
        expected.close().unwrap();
    }
}

#[cfg(unix)]
#[tokio::test]
#[ignore = "Requires real POML; external writer and model are offline fixtures"]
async fn resource_contract_agent_rejects_external_mutation_then_rechecks() {
    let source = tempfile::Builder::new().prefix("resource-source-").tempfile_in(".").unwrap();
    std::fs::write(source.path(), "before").unwrap();
    let path = source.path().file_name().unwrap().to_str().unwrap();
    let check = serde_json::json!({"program":"/bin/true","timeout_secs":5,"resources":[path]});
    let workflow = tempfile::Builder::new().prefix("resource-workflow-").suffix(".sm").tempfile_in("contexts").unwrap();
    std::fs::write(workflow.path(), format!("[state working]\nsettings.activated_tools = [\"run_check\", \"agent_complete\"]\n[checks]\ntests = {check}\n[guards]\n_complete = [tests]\n")).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let (state, user, requests) = fixture(dir.path(), vec![
        Step::Reply(reply(None, vec![call("before-write", "run_check", serde_json::json!({"name":"tests"}))])),
        Step::ExternalWrite(source.path().to_path_buf(), "external".into(), reply(Some("Finished. [[AGENT:COMPLETE]]"), vec![])),
        Step::Reply(reply(None, vec![call("after-write", "run_check", serde_json::json!({"name":"tests"}))])),
        Step::Reply(reply(Some("Verified completion. [[AGENT:COMPLETE]]"), vec![])),
    ]);
    state.db.merge_context(&user, serde_json::json!({
        "settings.sm_file":workflow.path().file_stem().unwrap().to_str().unwrap(),
        "settings.max_llm_turns":8,"settings.max_tool_calls":4,"settings.compaction_enabled":false
    })).unwrap();
    let result = crate::gateway::agent_loop::run_agent_loop(&state, &user, "Verify before completing.", Default::default(), None).await.unwrap();
    assert!(result.completed);
    assert!(result.response.contains("Verified completion"));
    assert_eq!(requests.lock().unwrap().len(), 4);
    let history = state.db.get_messages(&user, 100).unwrap();
    assert!(history.iter().any(|m| m.role == "system" && m.content.contains("Execution guard rejected")));
    let receipt = |id: &str| -> serde_json::Value {
        let message = history.iter().find(|m| m.tool_call_id.as_deref() == Some(id)).unwrap();
        serde_json::from_str::<serde_json::Value>(&message.content).unwrap()["receipt"].clone()
    };
    assert_eq!(receipt("before-write")["verified"], true);
    assert_eq!(receipt("after-write")["verified"], true);
    assert_ne!(receipt("before-write")["resources"]["sha256"], receipt("after-write")["resources"]["sha256"]);
    workflow.close().unwrap();
    source.close().unwrap();
}

enum Step {
    Reply(ChatResponse),
    ExternalWrite(std::path::PathBuf, String, ChatResponse),
    Unavailable,
    ReadLastOutput,
    Cancel(ChatResponse),
}
struct ScriptedProvider {
    user: String,
    steps: Mutex<VecDeque<Step>>,
    requests: Arc<Mutex<Vec<ChatRequest>>>,
}
#[async_trait::async_trait]
impl LLMProvider for ScriptedProvider {
    fn name(&self) -> &str {
        "offline-tool-chain"
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn chat(&self, request: ChatRequest) -> anyhow::Result<ChatResponse> {
        self.requests.lock().unwrap().push(request.clone());
        match self
            .steps
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected extra generation")
        {
            Step::Reply(response) => Ok(response),
            Step::ExternalWrite(path, content, response) => {
                std::fs::write(path, content).unwrap();
                Ok(response)
            }
            Step::Unavailable => Err(ProviderError::new(ErrorKind::Unavailable).into()),
            Step::ReadLastOutput => {
                let text = request.messages.iter().rev().find(|m| m.role == "tool").unwrap().content.as_deref().unwrap();
                let id = serde_json::from_str::<serde_json::Value>(text).ok()
                    .and_then(|v| v.get("output_id").or_else(|| v.pointer("/_praxis_tool_output/output_id")).and_then(serde_json::Value::as_str).map(String::from))
                    .unwrap_or_else(|| text.rsplit("output_id=").next().unwrap().split_whitespace().next().unwrap().to_string());
                Ok(reply(None, vec![call("reread", "read_tool_result", serde_json::json!({"output_id":id,"json_pointer":"/stdout"}))]))
            }
            Step::Cancel(response) => {
                crate::gateway::task_control::cancel(&self.user);
                Ok(response)
            }
        }
    }
}
fn call(id: &str, name: &str, args: serde_json::Value) -> ToolCall {
    ToolCall {
        id: id.into(),
        function: FunctionCall {
            name: name.into(),
            arguments: args.to_string(),
        },
    }
}
fn reply(content: Option<&str>, calls: Vec<ToolCall>) -> ChatResponse {
    ChatResponse {
        reasoning_content: None,
        content: content.map(String::from),
        tool_calls: (!calls.is_empty()).then_some(calls),
        finish_reason: None,
        usage: None,
    }
}
fn fixture(
    dir: &std::path::Path,
    steps: Vec<Step>,
) -> (GatewayState, String, Arc<Mutex<Vec<ChatRequest>>>) {
    let db = crate::db::Database::new(dir).unwrap();
    crate::db::tools::init_default_tools(&db).unwrap();
    let user = format!("tool-chain-{}", uuid::Uuid::new_v4());
    db.save_context(&db.load_context(&user).unwrap()).unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let router = LLMRouter::with_providers(
        vec![Box::new(ScriptedProvider {
            user: user.clone(),
            steps: Mutex::new(steps.into()),
            requests: requests.clone(),
        })],
        "offline-tool-chain".into(),
        vec![],
        ResilienceConfig {
            max_attempts: 2,
            max_output_tokens: 12_345,
            history_image_messages: 0,
            initial_backoff_ms: 1,
            max_backoff_ms: 1,
            ..Default::default()
        },
    );
    let state = GatewayState {
        db,
        config: crate::config::Config::from_env(),
        secrets: Default::default(),
        llm: crate::gateway::LlmHandle::new(router),
        plugins: Arc::new(crate::plugins::PluginRegistry::new()),
        event_tx: tokio::sync::broadcast::channel(16).0,
        start_time: std::time::Instant::now(),
    };
    (state, user, requests)
}
fn assert_paired_history(state: &GatewayState, user: &str, expected_calls: usize) {
    let history = state.db.get_messages(user, 100).unwrap();
    let calls: Vec<_> = history
        .iter()
        .filter_map(|m| m.tool_calls.as_ref())
        .flatten()
        .collect();
    assert_eq!(calls.len(), expected_calls);
    for call in calls {
        let results: Vec<_> = history
            .iter()
            .filter(|m| m.tool_call_id.as_deref() == Some(&call.id))
            .collect();
        assert_eq!(
            results.len(),
            1,
            "each requested tool needs exactly one durable receipt"
        );
        assert_eq!(
            results[0].tool_name.as_deref(),
            Some(call.function.name.as_str())
        );
    }
    assert!(
        !history.iter().any(|m| m.role == "assistant"
            && m.tool_calls.is_none()
            && m.content.trim().is_empty()),
        "never persist an empty final answer"
    );
    assert!(crate::gateway::task_control::cancellation(user).is_none());
}

#[cfg(unix)]
#[tokio::test]
#[ignore = "Requires the real Microsoft POML CLI; checks and model are offline"]
async fn contract_agent_rejects_unverified_completion_then_recovers() {
    std::fs::create_dir_all("contexts").unwrap();
    let workflow_file = tempfile::Builder::new().prefix("contract-test-").suffix(".sm").tempfile_in("contexts").unwrap();
    std::fs::write(workflow_file.path(), r#"
@steps [working, done]
[state working]
settings.activated_tools = ["run_check", "agent_complete", "set_context", "get_context"]
[state done]
settings.activated_tools = ["agent_complete"]
[checks]
tests = {"program":"/bin/true","timeout_secs":5}
[guards]
done = [tests]
_complete = [tests]
"#).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let (state, user, requests) = fixture(dir.path(), vec![
        Step::Reply(reply(None, vec![call("early-complete", "agent_complete", serde_json::json!({}))])),
        Step::Reply(reply(None, vec![call("forged-context", "set_context", serde_json::json!({"key":"settings.done","value":true}))])),
        Step::Reply(reply(Some("Finished. [[AGENT:COMPLETE]]"), vec![])),
        Step::Reply(reply(None, vec![call("actual-check", "run_check", serde_json::json!({"name":"tests"}))])),
        Step::Reply(reply(Some("Verified completion. [[AGENT:COMPLETE]]"), vec![])),
    ]);
    state.db.merge_context(&user, serde_json::json!({
        "settings.sm_file":workflow_file.path().file_stem().unwrap().to_str().unwrap(),
        "settings.max_llm_turns":8, "settings.max_tool_calls":10, "settings.compaction_enabled":false
    })).unwrap();
    let result = crate::gateway::agent_loop::run_agent_loop(&state, &user, "Verify before completing.", Default::default(), None).await.unwrap();
    assert!(result.completed);
    assert!(result.response.contains("Verified completion"));
    assert_eq!(requests.lock().unwrap().len(), 5);
    let history = state.db.get_messages(&user, 100).unwrap();
    for id in ["early-complete", "forged-context"] {
        assert!(history.iter().find(|m| m.tool_call_id.as_deref() == Some(id)).unwrap().content.contains("requires current verified checks"));
    }
    assert!(history.iter().any(|m| m.role == "system" && m.content.contains("Execution guard rejected")));
    assert!(history.iter().find(|m| m.tool_call_id.as_deref() == Some("actual-check")).unwrap().content.contains("\"verified\":true"));
}

#[tokio::test]
#[cfg(unix)]
#[ignore = "Requires Node and POML_CLI; synthetic provider and temporary shell/file fixtures only"]
async fn tool_output_model_controls_all_three_loops_and_default_is_full_without_reexecution() {
    for flow in ["message", "agent", "standalone"] {
        for select_tail in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let source = dir.path().join("source.txt");
            let effect = dir.path().join("effects.txt");
            let body = format!("BEGIN_SENTINEL\n{}END_SENTINEL\n", "middle payload αβγ\n".repeat(3000));
            std::fs::write(&source, &body).unwrap();
            let mut args = serde_json::json!({"command":format!("printf once >> '{}'; cat '{}'; printf warning >&2", effect.display(), source.display())});
            if select_tail { args["_output"] = serde_json::json!({"view":"tail","line_count":1,"json_pointer":"/stdout"}); }
            let (state, user, requests) = fixture(dir.path(), vec![
                Step::Reply(reply(None, vec![call("execute-once", "execute_terminal", args)])),
                Step::ReadLastOutput,
                Step::Reply(reply(Some("Read requested output without rerunning."), vec![])),
            ]);
            state.db.merge_context(&user, serde_json::json!({"sm_data.role":"code","settings.tool_result_limit":1,"settings.history_with_toolcalls":false})).unwrap();
            if flow == "agent" {
                let config = crate::gateway::agent_loop::AgentLoopConfig { max_turns:4, ..Default::default() };
                crate::gateway::agent_loop::run_agent_loop(&state, &user, "offline output test", config, None).await.unwrap();
            } else if flow == "standalone" {
                let tools = crate::db::tools::to_tool_definitions(&state.db).unwrap();
                state.llm.get().chat_with_tools(&state.db, &user, vec![], tools, Some(4), None, None, None).await.unwrap();
            } else {
                handle_message(&state, &user, "offline output test", Some("web")).await.unwrap();
            }
            assert_eq!(std::fs::read_to_string(&effect).unwrap(), "once");
            let requests = requests.lock().unwrap();
            assert_eq!(requests.len(), 3, "flow={flow}, select_tail={select_tail}");
            let definition = requests[0].tools.as_ref().unwrap().iter().find(|t| t.function.name == "execute_terminal").unwrap();
            assert_eq!(definition.function.parameters["properties"]["_output"]["type"], "object");
            let first = requests[1].messages.iter().find(|m| m.tool_call_id.as_deref() == Some("execute-once")).unwrap().content.as_deref().unwrap();
            if select_tail {
                let page: serde_json::Value = serde_json::from_str(first).unwrap();
                assert_eq!(page["text"], "3002: END_SENTINEL\n");
                assert!(!first.contains("BEGIN_SENTINEL"));
            } else {
                assert!(first.len() > 32000);
                assert!(first.contains("BEGIN_SENTINEL") && first.contains("END_SENTINEL") && first.contains("warning"));
            }
            let reread = requests[2].messages.iter().find(|m| m.tool_call_id.as_deref() == Some("reread")).unwrap().content.as_deref().unwrap();
            let reread: serde_json::Value = serde_json::from_str(reread).unwrap();
            assert_eq!(reread["text"], body, "full default must also apply to the model's reader call");
            assert!(reread["next"].is_null());
        }
    }
}

#[tokio::test]
#[ignore = "Requires Node and POML_CLI; provider and all inputs are synthetic"]
async fn tool_chain_skill_then_reference_then_action_reaches_final_answer() {
    for max_turns in [None, Some(5)] {
        for with_history in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let output = dir.path().join("synthetic-artifact.txt");
            let reference = std::env::current_dir()
                .unwrap()
                .join("skills/code_review/references/STYLE_GUIDE.md");
            let steps = vec![
                Step::Reply(reply(
                    None,
                    vec![call(
                        "skill",
                        "use_skill",
                        serde_json::json!({"name":"code_review","parameters":{"user_request":"offline synthetic inspection"}}),
                    )],
                )),
                Step::Reply(reply(
                    Some(""),
                    vec![call(
                        "reference",
                        "read_file",
                        serde_json::json!({"path":reference}),
                    )],
                )),
                Step::Reply(reply(
                    Some(" \t"),
                    vec![call(
                        "action",
                        "write_file",
                        serde_json::json!({"path":output,"content":"synthetic artifact"}),
                    )],
                )),
                Step::Reply(reply(Some("Synthetic task finished."), vec![])),
            ];
            let (state, user, requests) = fixture(dir.path(), steps);
            state.db.merge_context(&user, serde_json::json!({"settings.max_llm_turns":max_turns,"settings.history_with_toolcalls":with_history})).unwrap();
            let result = handle_message(&state, &user, "offline test", Some("web"))
                .await
                .unwrap();
            assert_eq!(result, "Synthetic task finished.");
            assert_eq!(
                std::fs::read_to_string(output).unwrap(),
                "synthetic artifact"
            );
            assert_paired_history(&state, &user, 3);
            let requests = requests.lock().unwrap();
            assert_eq!(requests.len(), 4);
            for (i, request) in requests.iter().enumerate() {
                assert_eq!(request.max_tokens, Some(12_345), "configured output bound lost at continuation {i}");
                assert!(
                    request
                        .tools
                        .as_ref()
                        .unwrap()
                        .iter()
                        .any(|t| t.function.name == "read_file"),
                    "tools lost at continuation {i}"
                );
                assert_eq!(
                    request.messages.iter().filter(|m| m.role == "tool").count(),
                    i,
                    "current tool results lost at continuation {i}"
                );
            }
            assert!(requests[1].messages.iter().any(|m| m.role == "tool"
                && m.content
                    .as_deref()
                    .unwrap_or("")
                    .contains("references/format-cheatsheet.md")));
            assert!(requests[2]
                .messages
                .iter()
                .any(|m| m.tool_call_id.as_deref() == Some("reference")
                    && !m.content.as_deref().unwrap_or("").starts_with("Error:")));
        }
    }
}

#[tokio::test]
#[cfg(unix)]
#[ignore = "Requires Node and POML_CLI; only temporary local side effects"]
async fn tool_chain_retry_never_reexecutes_previous_rounds() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("effects");
    let (state, user, requests) = fixture(
        dir.path(),
        vec![
            Step::Reply(reply(
                None,
                vec![call(
                    "a",
                    "execute_terminal",
                    serde_json::json!({"command":format!("printf a >> '{}'",path.display())}),
                )],
            )),
            Step::Reply(reply(
                None,
                vec![call(
                    "b",
                    "execute_terminal",
                    serde_json::json!({"command":format!("printf b >> '{}'",path.display())}),
                )],
            )),
            Step::Unavailable,
            Step::Reply(reply(Some("Done."), vec![])),
        ],
    );
    // Shell execution is deliberately a coding-state capability, not a
    // permission granted implicitly by the synthetic provider's tool call.
    state.db.merge_context(&user, serde_json::json!({"sm_data.role":"code"})).unwrap();
    assert_eq!(
        handle_message(&state, &user, "offline test", Some("web"))
            .await
            .unwrap(),
        "Done."
    );
    assert_eq!(std::fs::read_to_string(path).unwrap(), "ab");
    assert_paired_history(&state, &user, 2);
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 4);
    assert_eq!(
        serde_json::to_value(&requests[2]).unwrap(),
        serde_json::to_value(&requests[3]).unwrap()
    );
}

#[tokio::test]
#[ignore = "Requires Node and POML_CLI; no real provider calls"]
async fn tool_chain_budget_blocks_excess_calls_and_refuses_tool_only_finalization() {
    let dir = tempfile::tempdir().unwrap();
    let paths: Vec<_> = (0..3)
        .map(|i| dir.path().join(format!("effect-{i}")))
        .collect();
    let calls: Vec<_> = paths
        .iter()
        .enumerate()
        .map(|(i, p)| {
            call(
                &format!("call-{i}"),
                "write_file",
                serde_json::json!({"path":p,"content":"effect"}),
            )
        })
        .collect();
    let (state, user, requests) = fixture(
        dir.path(),
        vec![
            Step::Reply(reply(None, calls[..2].to_vec())),
            Step::Reply(reply(None, calls[2..].to_vec())),
        ],
    );
    state
        .db
        .merge_context(&user, serde_json::json!({"settings.max_tool_calls":1}))
        .unwrap();
    let error = handle_message(&state, &user, "offline test", Some("web"))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("tool-call limit"), "{error}");
    assert!(paths[0].exists());
    assert!(!paths[1].exists() && !paths[2].exists());
    assert_paired_history(&state, &user, 3);
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests[1].tools.is_none());
}

#[tokio::test]
#[ignore = "Requires Node and POML_CLI; no real provider calls"]
async fn tool_chain_zero_budget_and_true_empty_response_are_not_silent_successes() {
    for max in [0, -1] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("must-not-exist");
        let (state, user, requests) = fixture(
            dir.path(),
            vec![Step::Reply(reply(
                None,
                vec![call(
                    "blocked",
                    "write_file",
                    serde_json::json!({"path":path,"content":"effect"}),
                )],
            ))],
        );
        state
            .db
            .merge_context(&user, serde_json::json!({"settings.max_tool_calls":max}))
            .unwrap();
        assert!(handle_message(&state, &user, "offline test", Some("web"))
            .await
            .is_err());
        assert!(!path.exists());
        assert!(requests.lock().unwrap()[0].tools.is_none());
        assert_paired_history(&state, &user, 1);
    }
    let dir = tempfile::tempdir().unwrap();
    let (state, user, requests) =
        fixture(dir.path(), vec![Step::Reply(reply(Some(" \n\t"), vec![]))]);
    let error = handle_message(&state, &user, "offline test", Some("web"))
        .await
        .unwrap_err();
    assert_eq!(
        error
            .downcast_ref::<crate::gateway::llm::error::CallFailure>()
            .unwrap()
            .terminal
            .kind,
        ErrorKind::InvalidResponse
    );
    assert_eq!(requests.lock().unwrap().len(), 1);
    assert_paired_history(&state, &user, 0);
}

#[tokio::test]
#[ignore = "Requires Node and POML_CLI; cancellation and files are local mocks"]
async fn tool_chain_cancellation_persists_receipts_without_further_effects() {
    let dir = tempfile::tempdir().unwrap();
    let paths: Vec<_> = (0..3)
        .map(|i| dir.path().join(format!("effect-{i}")))
        .collect();
    let calls: Vec<_> = paths
        .iter()
        .enumerate()
        .map(|(i, p)| {
            call(
                &format!("call-{i}"),
                "write_file",
                serde_json::json!({"path":p,"content":"effect"}),
            )
        })
        .collect();
    let (state, user, requests) = fixture(
        dir.path(),
        vec![
            Step::Reply(reply(None, calls[..1].to_vec())),
            Step::Cancel(reply(None, calls[1..].to_vec())),
        ],
    );
    let error = handle_message(&state, &user, "offline test", Some("web"))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("cancelled"), "{error}");
    assert!(paths[0].exists());
    assert!(!paths[1].exists() && !paths[2].exists());
    assert_paired_history(&state, &user, 3);
    assert_eq!(requests.lock().unwrap().len(), 2);
}

#[tokio::test]
#[ignore = "Requires Node and POML_CLI; no real provider calls"]
async fn tool_chain_agent_limit_never_returns_a_previous_tasks_answer() {
    let dir = tempfile::tempdir().unwrap();
    let (state, user, _) = fixture(
        dir.path(),
        vec![
            Step::Reply(reply(
                None,
                vec![call(
                    "one",
                    "get_context",
                    serde_json::json!({"key":"mode"}),
                )],
            )),
            Step::Reply(reply(
                None,
                vec![call(
                    "two",
                    "get_context",
                    serde_json::json!({"key":"turn"}),
                )],
            )),
        ],
    );
    state
        .db
        .merge_context(&user, serde_json::json!({"settings.max_llm_turns":2}))
        .unwrap();
    state
        .db
        .add_message(
            &user,
            &crate::db::messages::Message::assistant("Old unrelated answer".into()),
        )
        .unwrap();
    let error = handle_message(&state, &user, "offline test", Some("web"))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("turn limit"), "{error}");
    assert_paired_history(&state, &user, 2);
}

#[tokio::test]
#[ignore = "Requires Node and POML_CLI; no real provider calls"]
async fn tool_chain_budget_allows_one_honest_finalization_and_counts_invalid_calls() {
    for valid in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let tool = if valid {
            call("last", "get_context", serde_json::json!({"key":"mode"}))
        } else {
            call("last", "nonexistent_tool", serde_json::json!({}))
        };
        let (state, user, requests) = fixture(
            dir.path(),
            vec![
                Step::Reply(reply(None, vec![tool])),
                Step::Reply(reply(
                    Some("Partial status: no artifact was generated."),
                    vec![],
                )),
            ],
        );
        state
            .db
            .merge_context(&user, serde_json::json!({"settings.max_tool_calls":1}))
            .unwrap();
        assert_eq!(
            handle_message(&state, &user, "offline test", Some("web"))
                .await
                .unwrap(),
            "Partial status: no artifact was generated."
        );
        assert_paired_history(&state, &user, 1);
        assert_eq!(state.db.load_context(&user).unwrap().turn, 1);
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert!(requests[1].tools.is_none());
        assert!(requests[1]
            .messages
            .iter()
            .any(|m| m.role == "system" && m.content.as_deref().unwrap_or("").contains("budget")));
    }
}

#[tokio::test]
#[ignore = "Requires Node and POML_CLI; synthetic provider and temporary files only"]
async fn tool_chain_new_task_resets_stale_completion_without_losing_settings() {
    for max_turns in [None, Some(5)] {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("fresh-task.txt");
        let image = dir.path().join("current.png");
        image::RgbaImage::new(1, 1).save(&image).unwrap();
        let (state, user, requests) = fixture(dir.path(), vec![
            Step::Reply(reply(None, vec![
                call("check", "get_context", serde_json::json!({"key":"settings.done"})),
                call("discover-image", "search_tools", serde_json::json!({"query":"understand_image", "limit":1})),
            ])),
            Step::Reply(reply(Some("Continuing the current task."), vec![
                call("write", "write_file", serde_json::json!({"path":output,"content":"fresh task"})),
                call("current-image", "understand_image", serde_json::json!({"path":image,"prompt":"Inspect this synthetic test pixel."})),
            ])),
            Step::Reply(reply(Some("Fresh task finished."), vec![])),
        ]);
        state.db.merge_context(&user, serde_json::json!({
            "settings.done":true, "settings.max_llm_turns":max_turns,
            "settings.max_tool_calls":5, "custom_data.preserve_me":"unchanged"
        })).unwrap();
        let mut old_attachment = crate::db::messages::Message::user("Earlier image at /synthetic/old.png".into());
        old_attachment.content_parts = Some(vec![serde_json::json!({"type":"image_url","image_url":{"url":"data:image/png;base64,OLD_SYNTHETIC"}})]);
        state.db.add_message(&user, &old_attachment).unwrap();
        let result = handle_message(&state, &user, "new independent task", Some("web")).await.unwrap();
        assert_eq!(result, "Fresh task finished.", "old done flag stopped the new tool chain");
        assert_eq!(std::fs::read_to_string(output).unwrap(), "fresh task");
        let ctx = state.db.load_context(&user).unwrap();
        assert!(!ctx.settings.done);
        assert_eq!(ctx.settings.max_llm_turns, max_turns);
        assert_eq!(ctx.settings.max_tool_calls, Some(5));
        assert_eq!(ctx.custom_data["preserve_me"], "unchanged");
        assert_eq!(requests.lock().unwrap().len(), 3);
        for request in requests.lock().unwrap().iter() {
            assert!(!serde_json::to_string(request).unwrap().contains("OLD_SYNTHETIC"), "previous task images must not be retransmitted");
        }
        assert!(requests.lock().unwrap()[2].messages.iter().any(|m|
            m.tool_call_id.as_deref() == Some("current-image") && m.content_parts.as_ref().is_some_and(|p| !p.is_empty())
        ), "newly read tool images must still reach the model");
        let history = state.db.get_messages(&user, 100).unwrap();
        assert!(history.iter().any(|m| m.content == old_attachment.content && m.content_parts == old_attachment.content_parts), "saved images must remain intact");
        let check = history.iter().find(|m| m.tool_call_id.as_deref() == Some("check")).unwrap();
        let observed: serde_json::Value = serde_json::from_str(&check.content).unwrap();
        assert_eq!(observed["settings"]["done"], false, "completion must reset BEFORE the first tool call");
        assert!(!requests.lock().unwrap()[0].tools.as_ref().unwrap().iter().any(|t| t.function.name == "understand_image"));
        assert!(requests.lock().unwrap()[1].tools.as_ref().unwrap().iter().any(|t| t.function.name == "understand_image"));
        assert_paired_history(&state, &user, 4);
    }
}

#[tokio::test]
#[ignore = "Requires Node and POML_CLI; no live provider calls"]
async fn tool_chain_completion_stops_current_task_but_not_next_direct_agent_run() {
    let dir = tempfile::tempdir().unwrap();
    let (state, user, requests) = fixture(dir.path(), vec![
        Step::Reply(reply(None, vec![call("complete", "agent_complete", serde_json::json!({}))])),
        Step::Reply(reply(None, vec![call("next-task", "get_context", serde_json::json!({"key":"settings.done"}))])),
        Step::Reply(reply(Some("Second task completed normally."), vec![])),
    ]);
    state.db.merge_context(&user, serde_json::json!({"settings.max_llm_turns":5})).unwrap();
    let first = crate::gateway::agent_loop::run_agent_loop(
        &state, &user, "complete this task", Default::default(), None,
    ).await.unwrap();
    assert!(first.completed);
    assert!(state.db.load_context(&user).unwrap().settings.done);
    assert_eq!(requests.lock().unwrap().len(), 1, "current completion must still stop generation");
    let second = crate::gateway::agent_loop::run_agent_loop(
        &state, &user, "start another task", Default::default(), None,
    ).await.unwrap();
    assert_eq!(second.response, "Second task completed normally.");
    assert_eq!(second.turns_used, 2);
    assert_eq!(requests.lock().unwrap().len(), 3);
    assert!(!state.db.load_context(&user).unwrap().settings.done);
    assert_paired_history(&state, &user, 2);
}

#[tokio::test]
#[ignore = "Requires Node and POML_CLI; local synthetic LLM and TTS only"]
async fn audio_web_tts_off_blocks_discord_flag_and_explicit_feedback_on_both_paths() {
    use wiremock::{matchers::{method, path}, Mock, MockServer, ResponseTemplate};
    let server = MockServer::start().await;
    Mock::given(method("POST")).and(path("/tts"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0).mount(&server).await;
    for max_turns in [1, 4] {
        let dir = tempfile::tempdir().unwrap();
        let (state, user, _) = fixture(dir.path(), vec![
            Step::Reply(reply(Some("Checking the context."), vec![call(
                "context", "get_context", serde_json::json!({}),
            )])),
            Step::Reply(reply(Some("Silent web reply."), vec![])),
        ]);
        state.db.merge_context(&user, serde_json::json!({
            "settings.max_llm_turns": max_turns,
            "settings.use_tts": true,
            "settings.web_chat_tts": false,
            "settings.feedback_mode": ["tts"],
            "settings.message_on_toolcalling": true,
            "settings.voice_tts_type": "qwen_tts",
            "settings.qwen_tts_server": server.uri(),
        })).unwrap();
        let response = handle_message(&state, &user, "offline audio gate test", Some("web")).await.unwrap();
        assert_eq!(response, "Silent web reply.");
    }
    // Allow detached feedback/final synthesis tasks to reach the local mock.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    server.verify().await;
}

#[tokio::test]
#[ignore = "Requires Node and POML_CLI; synthetic provider and temporary memory only"]
async fn tool_chain_discovery_loads_memory_only_for_current_task_on_both_paths() {
    use crate::db::memory_profiles;

    for max_turns in [1, 10] {
        let dir = tempfile::tempdir().unwrap();
        let profile = "discovery-progress";
        let (state, user, requests) = fixture(dir.path(), vec![
            Step::Reply(reply(None, vec![call("not-discovered", "memory_set", serde_json::json!({"key":"xp","value":999}))])),
            Step::Reply(reply(None, vec![call("discover", "search_tools", serde_json::json!({"query":"memory"}))])),
            Step::Reply(reply(None, vec![call("missing", "memory_get", serde_json::json!({"key":"xp"}))])),
            Step::Reply(reply(None, vec![call("create", "memory_profile_create", serde_json::json!({"name":profile}))])),
            Step::Reply(reply(None, vec![call("load", "memory_profile_load", serde_json::json!({"name":profile}))])),
            Step::Reply(reply(None, vec![call("before", "memory_get", serde_json::json!({"key":"xp"}))])),
            Step::Reply(reply(None, vec![call("save", "memory_set", serde_json::json!({"key":"xp","value":42,"expected_value":null,"expected_profile":profile}))])),
            Step::Reply(reply(None, vec![call("read", "memory_get", serde_json::json!({"key":"xp"}))])),
            Step::Reply(reply(Some("Progress saved."), vec![])),
            Step::Reply(reply(Some("Fresh task."), vec![])),
        ]);
        // Include the complete profile lifecycle in both paths' fixed budgets.
        state.db.merge_context(&user, serde_json::json!({"settings.max_llm_turns":max_turns,"settings.max_tool_calls":8})).unwrap();
        assert_eq!(handle_message(&state, &user, "save synthetic learning progress", Some("web")).await.unwrap(), "Progress saved.");
        assert_paired_history(&state, &user, 8);
        let receipts = state.db.get_messages(&user, 100).unwrap();
        let receipt = |id: &str| -> serde_json::Value {
            let message = receipts.iter().find(|m| m.tool_call_id.as_deref() == Some(id)).unwrap();
            serde_json::from_str(&message.content)
                .unwrap_or_else(|error| panic!("{id} receipt failed: {error}: {}", message.content))
        };
        let discovered = receipt("discover");
        for name in ["memory_get", "memory_set", "memory_profile_create", "memory_profile_load", "memory_profile_list"] {
            assert!(discovered["tools"].as_array().unwrap().iter().any(|t| t["name"] == name));
        }
        let missing = receipt("missing");
        assert_eq!(missing["profile"], "states/standard/standard");
        assert_eq!(missing["profile_exists"], false);
        assert_eq!(missing["exists"], false, "the rejected write must not persist");
        assert_eq!(receipt("create")["created"], true);
        assert_eq!(receipt("load")["loaded"], true);
        let before = receipt("before");
        assert_eq!(before["profile"], profile);
        assert_eq!(before["profile_exists"], true);
        assert_eq!(before["exists"], false);
        assert_eq!(before["value"], serde_json::Value::Null);
        let saved = receipt("save");
        assert_eq!(saved["saved"], true, "{saved}");
        assert_eq!(saved["profile"], before["profile"]);
        let after = receipt("read");
        assert_eq!(after["profile"], profile);
        assert_eq!(after["exists"], true);
        assert_eq!(after["value"], 42);

        // Reopen the database: a scripted final answer alone proves nothing.
        // Routed persona memory must never fall back to the legacy standard bucket.
        let reopened = crate::db::Database::new(dir.path()).unwrap();
        let view = memory_profiles::snapshot(&reopened, &reopened.load_context(&user).unwrap()).unwrap();
        assert_eq!(view.profile, profile);
        assert!(view.loaded && view.exists);
        assert_eq!(view.memory.custom_variables["xp"], 42);
        assert!(crate::db::memory::load_memory(&reopened, &user).unwrap().custom_variables.is_empty());
        assert!(memory_profiles::read_named(&reopened, "other-user", profile).unwrap().is_none());
        assert!(crate::gateway::task_control::selected_tools(&user).is_empty());
        assert_eq!(handle_message(&state, &user, "a new task", Some("web")).await.unwrap(), "Fresh task.");
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 10);
        for index in [0, 1, 9] {
            let tools = requests[index].tools.as_ref().unwrap();
            assert!(tools.len() <= 13); // existing core plus read_tool_result
            assert!(!tools.iter().any(|t| t.function.name.starts_with("memory_")));
        }
        let catalog = crate::tools::discovery::catalog(&state.db, &state.plugins).unwrap();
        for expected in catalog.iter().filter(|t| t.function.name.starts_with("memory_")) {
            let offered = requests[2].tools.as_ref().unwrap().iter()
                .find(|t| t.function.name == expected.function.name).unwrap();
            assert_eq!(offered.function.parameters, expected.function.parameters);
        }
        assert!(requests[1].messages.iter().any(|m| m.tool_call_id.as_deref() == Some("not-discovered") && m.content.as_deref().unwrap_or("").starts_with("Error: Unknown tool")));
    }
}

#[tokio::test]
#[ignore = "Requires Node and real POML_CLI; isolated DB and scripted provider"]
async fn small_model_agent_cannot_refill_its_turn_budget() {
    let dir = tempfile::tempdir().unwrap();
    let (state, user, requests) = fixture(dir.path(), vec![
        Step::Reply(reply(None, vec![call("extend", "set_context", serde_json::json!({"key":"settings.max_llm_turns","value":100}))])),
        Step::Reply(reply(None, vec![call("read", "get_context", serde_json::json!({}))])),
        Step::Reply(reply(Some("This third generation must never run"), vec![])),
    ]);
    state.db.merge_context(&user, serde_json::json!({"settings.max_llm_turns":2})).unwrap();
    assert!(handle_message(&state, &user, "synthetic task", Some("web")).await.is_err());
    assert_eq!(requests.lock().unwrap().len(),2);
}

#[tokio::test]
#[ignore = "Requires Node and real POML_CLI; actual SM/DB/tools with scripted provider"]
async fn twenty_tasks_rerenders_real_state_between_tools_in_chat_and_agent() {
    for turns in [1,8] {
        let dir=tempfile::tempdir().unwrap();
        let path=dir.path().join("observations.txt");
        std::fs::write(&path,"A new failure requires diagnosis before implementation.").unwrap();
        let (state,user,requests)=fixture(dir.path(),vec![
            Step::Reply(reply(None,vec![call("plan","set_context",serde_json::json!({"key":"active_state","value":"code_architect"}))])),
            Step::Reply(reply(None,vec![call("read","read_file",serde_json::json!({"path":path}))])),
            Step::Reply(reply(None,vec![call("diagnose","set_context",serde_json::json!({"key":"active_state","value":"debugger"}))])),
            Step::Reply(reply(None,vec![call("implement","set_context",serde_json::json!({"key":"active_state","value":"expert_programmer"}))])),
            Step::Reply(reply(Some("A verified handoff, not a claimed execution."),vec![])),
        ]);
        state.db.merge_context(&user,serde_json::json!({"settings.sm_file":"20-tasks","settings.max_llm_turns":turns,"settings.max_tool_calls":8})).unwrap();
        handle_message(&state,&user,"Read the observations and propose a checked solution.",Some("web")).await.unwrap();
        let requests=requests.lock().unwrap();
        assert_eq!(requests.len(),5);
        for (request,expected) in requests.iter().zip(["standard","code_architect","code_architect","debugger","expert_programmer"]) {
            let system=request.messages[0].content.as_deref().unwrap();
            assert!(system.contains(&format!("[STATE_ACTIVE:{expected}]")),"wrong rendered state for turns={turns}: {expected}");
            assert!(system.contains("NACH jedem Werkzeugergebnis"));
            assert!(!system.contains("set_context(\"sm_data.role\""));
        }
        let ctx=state.db.load_context(&user).unwrap();
        assert_eq!(ctx.active_state.as_deref(),Some("expert_programmer"));
        assert_eq!(ctx.settings.active_state,ctx.active_state);
        assert_eq!(ctx.sm_data["role"],"expert_programmer");
        assert_paired_history(&state,&user,4);
    }
}

#[tokio::test]
#[ignore = "Requires Node and real POML_CLI; isolated DB and scripted provider"]
async fn small_model_compaction_runs_before_chat_and_failure_keeps_history() {
    for failure in [false,true] {
        let dir=tempfile::tempdir().unwrap();
        let mut steps=if failure {vec![Step::Unavailable,Step::Unavailable]} else {vec![Step::Reply(reply(Some("## Big idea\nBuild a reliable local assistant.\n## Key insights\nPreserve verified receipts.\n## Handoff\nNext: test tools."),vec![]))]};
        steps.push(Step::Reply(reply(Some("final answer"),vec![])));
        let (state,user,requests)=fixture(dir.path(),steps);
        state.db.add_message(&user,&crate::db::messages::Message::user("Build a reliable local assistant; never repeat side effects.".into())).unwrap();
        state.db.add_message(&user,&crate::db::messages::Message::assistant("Verified receipt saved.".into())).unwrap();
        state.db.merge_context(&user,serde_json::json!({"settings.compaction_enabled":true,"settings.compaction_token_limit":1,"settings.compaction_summary":"Earlier insight: private self hosting","settings.model":"synthetic-model"})).unwrap();
        assert_eq!(handle_message(&state,&user,"What next?",Some("web")).await.unwrap(),"final answer");
        let history=state.db.get_messages(&user,100).unwrap();
        assert_eq!(history.len(),if failure {4}else{2});
        let requests=requests.lock().unwrap();
        let first=&requests[0];
        assert!(first.tools.is_none());
        assert_eq!(first.model.as_deref(),Some("synthetic-model"));
        assert_eq!(first.thinking,Some(crate::gateway::llm::provider::ThinkingMode::Off));
        let prompt=first.messages[0].content.as_deref().unwrap();
        assert!(prompt.contains("Earlier insight: private self hosting") && prompt.contains("never repeat side effects"));
        let actual=requests.last().unwrap();
        assert!(actual.messages[0].content.as_deref().unwrap().contains(if failure {"Earlier insight"} else {"Next: test tools"}));
    }
}

#[test]
fn tool_chain_empty_tts_returns_before_spawning_any_task() {
    // No Tokio runtime: trying to spawn even a doomed TTS task must fail this test.
    let dir = tempfile::tempdir().unwrap();
    let db = crate::db::Database::new(dir.path()).unwrap();
    for text in ["", " \n\t", "\u{a0}\u{2003}"] {
        spawn_tts(
            text.into(),
            &Default::default(),
            &Default::default(),
            "no-tts-for-empty-text",
            &db,
            None,
            Some("web"),
        );
    }
}
