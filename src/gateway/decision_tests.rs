use super::{decision_client, decision_profiles::{self, DecisionProfile}};
use serde_json::json;
use wiremock::{Mock, MockServer, ResponseTemplate, matchers::{method,path}};

fn profile(endpoint: String) -> DecisionProfile {
    serde_json::from_value(json!({
        "endpoint":endpoint,"model":"test-router","instructions":"Classify the next task.",
        "schema":{"category":{"type":"enum","choices":["A","B"]}},
        "state_field":"category","state_map":{"A":"standard","B":"debugger"},
        "timeout_ms":500,"minimum_probability":0.8,"reevaluate":"every_step"
    })).unwrap()
}

#[test]
fn decision_profiles_roundtrip_and_validate_before_replacing() {
    let dir=tempfile::tempdir().unwrap();
    let p=profile("http://127.0.0.1:11440/v1/decision".into());
    let text=serde_json::to_string(&p).unwrap();
    decision_profiles::save(dir.path(),"routing",&text).unwrap();
    assert_eq!(decision_profiles::load(dir.path(),"routing").unwrap().model,"test-router");
    assert_eq!(decision_profiles::list(dir.path()).unwrap(),vec!["routing"]);
    assert!(decision_profiles::save(dir.path(),"../escape",&text).is_err());
    assert!(decision_profiles::save(dir.path(),"routing","{}").is_err());
    assert_eq!(decision_profiles::load(dir.path(),"routing").unwrap().model,"test-router");
    let mut bad=p.clone();bad.endpoint="http://user:secret@host/v1/decision".into();assert!(bad.validate().is_err());
    bad=p.clone();bad.state_map.remove("A");assert!(bad.validate().is_err());
    bad=p.clone();bad.timeout_ms=0;assert!(bad.validate().is_err());
    bad=p.clone();bad.minimum_probability=1.1;assert!(bad.validate().is_err());
}

#[tokio::test]
async fn decision_client_uses_playground_protocol_and_validates_complete_results() {
    let server=MockServer::start().await;
    let p=profile(format!("{}/v1/decision",server.uri()));
    Mock::given(method("POST")).and(path("/v1/decision"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"results":[
            {"decision":{"category":"B"},"fields":{"category":{"value":"B","probability":0.91}}}
        ],"timings":{"total_ms":5}}))).expect(1).mount(&server).await;
    let result=decision_client::decide(&p,vec!["synthetic task".into()],&tokio_util::sync::CancellationToken::new()).await.unwrap();
    assert_eq!(result[0].label,"B");assert_eq!(result[0].probability,0.91);
    let sent=server.received_requests().await.unwrap()[0].body_json::<serde_json::Value>().unwrap();
    assert_eq!(sent["contexts"],json!(["synthetic task"]));assert_eq!(sent["model"],"test-router");
    assert_eq!(sent["schema"],p.schema);assert_eq!(sent["cache_prompt"],true);
    for bad in [json!({"results":[]}),json!({"results":[{"decision":{"category":"not-declared"},"fields":{"category":{"value":"not-declared","probability":0.99}}}]}),
        json!({"results":[{"decision":{"category":"A"},"fields":{"category":{"value":"B","probability":0.99}}}]}),
        json!({"results":[{"decision":{"category":"A"},"fields":{"category":{"value":"A","probability":2}}}]})] {
        assert!(decision_client::validate_results(&p,&bad,1).is_err());
    }
}

#[tokio::test]
async fn decision_router_applies_real_state_and_preserves_history_on_failure() {
    for (status,probability,label,expected) in [(200,0.95,"B","debugger"),(200,0.2,"B","standard"),(200,0.99,"invalid","standard"),(503,0.99,"B","standard")] {
        let root=tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("contexts")).unwrap();std::fs::create_dir(root.path().join("templates")).unwrap();
        std::fs::write(root.path().join("contexts/test.sm"),"@steps [standard]\n[state standard]\nsettings.system_template = \"standard\"\n[state debugger]\nsettings.system_template = \"debugger\"\nsm_data.role = \"debugger\"\n").unwrap();
        for name in ["standard","debugger"] {std::fs::write(root.path().join(format!("templates/{name}.poml")),format!("<poml>{name}</poml>")).unwrap();}
        let server=MockServer::start().await;
        Mock::given(method("POST")).respond_with(ResponseTemplate::new(status).set_body_json(json!({"results":[{"decision":{"category":label},"fields":{"category":{"value":label,"probability":probability}}}]}))).expect(1).mount(&server).await;
        let p=profile(format!("{}/v1/decision",server.uri()));
        decision_profiles::save(&root.path().join("decisions"),"routing",&serde_json::to_string(&p).unwrap()).unwrap();
        let db=crate::db::Database::new(&root.path().join("data")).unwrap();
        let mut ctx=crate::db::contexts::Context {user_id:format!("decision-step-{}",uuid::Uuid::new_v4()),active_state:Some("standard".into()),..Default::default()};
        ctx.settings.sm_file=Some("test".into());ctx.settings.decision_profile=Some("routing".into());db.save_context(&ctx).unwrap();
        db.add_message(&ctx.user_id,&crate::db::messages::Message::user("Find the error".into())).unwrap();
        let state=super::GatewayState {db:db.clone(),config:crate::config::Config::from_env(),secrets:Default::default(),
            llm:super::LlmHandle::new(super::llm::LLMRouter::with_providers(vec![],"unused".into(),vec![],Default::default())),
            plugins:std::sync::Arc::new(crate::plugins::PluginRegistry::new()),event_tx:tokio::sync::broadcast::channel(16).0,start_time:std::time::Instant::now()};
        let guard=super::task_control::begin(&ctx.user_id).unwrap();
        let result=super::decision_routing::route_in(root.path(),&state,ctx.clone(),"Find the error",None).await.unwrap();
        assert_eq!(result.active_state.as_deref(),Some(expected));
        assert_eq!(db.load_context(&ctx.user_id).unwrap().active_state.as_deref(),Some(expected));
        if expected=="debugger" {assert_eq!(result.settings.system_template.as_deref(),Some("debugger"));assert_eq!(result.sm_data["role"],"debugger");}
        assert_eq!(db.get_messages(&ctx.user_id,100).unwrap().len(),1);
        drop(guard);
    }
}

#[test]
fn decision_context_cas_is_session_scoped_and_rejects_stale_snapshots() {
    let dir=tempfile::tempdir().unwrap();let db=crate::db::Database::new(dir.path()).unwrap();
    let mut ctx=crate::db::contexts::Context {user_id:"decision-cas".into(),..Default::default()};
    db.save_context(&ctx).unwrap();
    ctx.session_id="other".into();db.save_context(&ctx).unwrap();
    let snapshot=db.load_context(&ctx.user_id).unwrap();
    let mut next=snapshot.clone();next.active_state=Some("debugger".into());
    assert!(db.compare_and_save_context(&snapshot,&next).unwrap());
    assert_eq!(db.load_context(&ctx.user_id).unwrap().active_state.as_deref(),Some("debugger"));
    assert!(db.load_session_context(&ctx.user_id,"default").unwrap().active_state.is_none());
    assert!(!db.compare_and_save_context(&snapshot,&ctx).unwrap());
    db.save_context(&crate::db::contexts::Context {user_id:ctx.user_id.clone(),..Default::default()}).unwrap();
    assert!(!db.compare_and_save_context(&next,&snapshot).unwrap());
}

#[test]
fn decision_profile_selection_is_not_model_writable() {
    let dir=tempfile::tempdir().unwrap();let db=crate::db::Database::new(dir.path()).unwrap();
    let ctx=crate::db::contexts::Context {user_id:"decision-policy".into(),..Default::default()};db.save_context(&ctx).unwrap();
    for update in [json!({"settings.decision_profile":"task-router"}),json!({"settings":{"decision_profile":"task-router"}})] {
        assert!(db.merge_context_from_agent(&ctx.user_id,update).is_err());
        assert!(db.load_context(&ctx.user_id).unwrap().settings.decision_profile.is_none());
    }
}

#[tokio::test]
async fn decision_client_cancellation_and_timeout_do_not_retry() {
    let server=MockServer::start().await;
    let mut p=profile(format!("{}/v1/decision",server.uri()));p.timeout_ms=50;
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(200).set_delay(std::time::Duration::from_secs(1)))
        .expect(1).mount(&server).await;
    assert!(decision_client::decide(&p,vec!["synthetic".into()],&tokio_util::sync::CancellationToken::new()).await.is_err());
    let cancel=tokio_util::sync::CancellationToken::new();cancel.cancel();
    assert!(decision_client::decide(&p,vec!["synthetic".into()],&cancel).await.is_err());
}

fn ollama_profile(endpoint: String) -> DecisionProfile {
    serde_json::from_value(json!({
        "backend":"ollama", "endpoint":endpoint, "model":"nimble:latest",
        "instructions":"Classify the next task.",
        "schema":{"category":{"type":"enum","choices":["A","B"]}},
        "state_field":"category", "state_map":{"A":"standard","B":"debugger"},
        "criteria":{"A":"Ordinary conversation","B":"Diagnose a software failure"},
        "timeout_ms":1000, "minimum_probability":0.8
    })).unwrap()
}

fn systemone_answer(probability: f64, confidence: f64) -> serde_json::Value {
    json!({"model":"nimble:latest", "answers":{"category":{
        "type":"choice", "choice":"B", "probabilities":{"A":1.0-probability,"B":probability},
        "confidence":confidence}}, "usage":{"input_tokens":25,"output_tokens":1}})
}

#[tokio::test]
async fn decision_ollama_systemone_uses_model_state_and_described_choices() {
    let server=MockServer::start().await;
    let p=ollama_profile(format!("{}/v1/systemone",server.uri()));
    Mock::given(method("POST")).and(path("/v1/systemone"))
        .respond_with(ResponseTemplate::new(200).set_body_json(systemone_answer(0.91,0.15)))
        .expect(1).mount(&server).await;
    let result=decision_client::decide(&p,vec!["quoted evidence".into()],&Default::default()).await.unwrap();
    assert_eq!(result[0].label,"B");
    assert_eq!(result[0].probability,0.91);
    assert_eq!(result[0].confidence,Some(0.15));
    assert_eq!(result[0].usage.as_ref().unwrap().input_tokens,25);
    let sent=server.received_requests().await.unwrap()[0].body_json::<serde_json::Value>().unwrap();
    assert_eq!(sent["model"],"nimble:latest");
    assert_eq!(sent["state"],"quoted evidence");
    assert_eq!(sent["questions"]["category"]["type"],"choice");
    assert_eq!(sent["questions"]["category"]["criteria"],json!(p.criteria));
    assert!(sent.get("contexts").is_none() && sent.get("tools").is_none() && sent.get("stream").is_none());
}

#[test]
fn decision_ollama_rejects_incomplete_or_inconsistent_probability_evidence() {
    let p=ollama_profile("http://127.0.0.1:1/v1/systemone".into());
    assert!(decision_client::validate_systemone(&p,&systemone_answer(0.91,0.15)).is_ok());
    for (pointer,value) in [
        ("/model",json!("different-model")),
        ("/answers/category/type",json!("score")),
        ("/answers/category/choice",json!("not-declared")),
        ("/answers/category/probabilities/B",json!(null)),
        ("/answers/category/probabilities/B",json!(1.5)),
        ("/answers/category/probabilities/A",json!(0.5)),
        ("/answers/category/probabilities/unknown",json!(0.01)),
        ("/answers/category/confidence",json!(-1)),
    ] {
        let mut answer=systemone_answer(0.91,0.15);
        if let Some(slot)=answer.pointer_mut(pointer) {*slot=value;}
        else {answer["answers"]["category"]["probabilities"]["unknown"]=value;}
        assert!(decision_client::validate_systemone(&p,&answer).is_err(),"accepted {pointer}");
    }
    assert!(decision_client::validate_systemone(&p,&systemone_answer(0.2,0.99)).is_err(),"Selected choice must be a maximum");
    let mut missing=systemone_answer(0.91,0.15);
    missing.as_object_mut().unwrap().remove("usage");
    assert!(decision_client::validate_systemone(&p,&missing).unwrap().usage.is_none());
}

#[test]
fn decision_ollama_profile_enforces_systemone_limits_and_keeps_native_defaults() {
    use super::decision_profiles::DecisionBackend;
    assert_eq!(profile("http://127.0.0.1:1/v1/decision".into()).backend,DecisionBackend::Native);
    let p=ollama_profile("http://127.0.0.1:1/v1/systemone".into());
    assert!(p.validate().is_ok());
    let mut extended=p.clone();extended.timeout_ms=60000;assert!(extended.validate().is_ok());
    let mut bad=p.clone();bad.criteria.remove("A");assert!(bad.validate().is_err());
    bad=p.clone();bad.endpoint="http://127.0.0.1:1/api/chat".into();assert!(bad.validate().is_err());
    bad=p.clone();bad.schema["category"]["choices"]=json!(["B"]);bad.state_map.remove("A");bad.criteria.remove("A");assert!(bad.validate().is_err());
}

#[tokio::test]
async fn decision_ollama_cancellation_and_timeout_never_retry() {
    let server=MockServer::start().await;
    let mut p=ollama_profile(format!("{}/v1/systemone",server.uri()));p.timeout_ms=50;
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(200)
        .set_body_json(systemone_answer(0.91,0.15)).set_delay(std::time::Duration::from_secs(1)))
        .expect(1).mount(&server).await;
    assert!(decision_client::decide(&p,vec!["synthetic".into()],&Default::default()).await.is_err());
    let cancel=tokio_util::sync::CancellationToken::new();cancel.cancel();
    assert!(decision_client::decide(&p,vec!["synthetic".into()],&cancel).await.is_err());
}
