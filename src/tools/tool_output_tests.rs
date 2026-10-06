use crate::{db::Database, gateway::tool_results, tools::tool_output};
use serde_json::{json, Value};

fn fixture() -> (tempfile::TempDir, Database) {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::new(dir.path()).unwrap();
    crate::db::tools::init_default_tools(&db).unwrap();
    db.save_context(&db.load_context("alice").unwrap()).unwrap();
    (dir, db)
}
fn save(db: &Database, user: &str, text: &str, limit: usize) -> String {
    let mut ctx = db.load_context(user).unwrap();
    ctx.settings.tool_result_limit = Some(limit); // legacy settings must not clip results
    db.save_context(&ctx).unwrap();
    let response = tool_results::prepare(db, user, "synthetic_plugin", "call", text).unwrap();
    serde_json::from_str::<Value>(&response).ok().and_then(|v| v.pointer("/_praxis_tool_output/output_id").and_then(Value::as_str).map(String::from))
        .unwrap_or_else(|| response.split("output_id=").nth(1).unwrap().split_whitespace().next().unwrap().to_string())
}
fn read(db: &Database, user: &str, args: Value) -> Value {
    serde_json::from_str(&tool_output::run(db, user, &args).unwrap()).unwrap()
}

#[test]
fn tool_output_original_and_full_reads_survive_restart() {
    let (dir, db) = fixture();
    let text = "😀αβ\n日本語\n".repeat(1000);
    let id = save(&db, "alice", &text, 7);
    let stored = db.get_tool_output("alice", &id).unwrap().unwrap();
    assert_eq!(stored.content, text);
    assert_eq!(stored.source_bytes, text.len());
    drop(db);
    let db = Database::new(dir.path()).unwrap();
    let mut args = json!({"output_id":id,"view":"full","max_chars":257});
    let mut assembled = String::new();
    loop {
        let page = read(&db, "alice", args);
        assert!(page["text"].as_str().unwrap().chars().count() <= 257);
        assembled.push_str(page["text"].as_str().unwrap());
        assert_eq!(page["storage_truncated"], false);
        if page["next"].is_null() { break; }
        args = page["next"].clone();
    }
    assert_eq!(assembled, text);
}

#[test]
fn tool_output_lines_tail_search_and_json_field_are_selectable() {
    let (_dir, db) = fixture();
    let id = save(&db, "alice", "zero\nERROR first\n二\nERROR last\nend\n", 0);
    let lines = read(&db, "alice", json!({"output_id":id,"view":"lines","start_line":2,"line_count":2}));
    assert_eq!(lines["text"], "2: ERROR first\n3: 二\n");
    let tail = read(&db, "alice", json!({"output_id":id,"view":"tail","line_count":2}));
    assert_eq!(tail["text"], "4: ERROR last\n5: end\n");
    let hits = read(&db, "alice", json!({"output_id":id,"view":"search","query":"ERROR","line_count":1}));
    assert_eq!(hits["text"], "2: ERROR first\n");
    let rest = read(&db, "alice", hits["next"].clone());
    assert_eq!(rest["text"], "4: ERROR last\n");
    assert!(rest["next"].is_null());
    let id = save(&db, "alice", &json!({"stdout":"one\ntwo\nthree\n","stderr":"warning", "rows":[{"value":42}]}).to_string(), 10);
    assert_eq!(read(&db, "alice", json!({"output_id":id,"json_pointer":"/stdout","view":"tail","line_count":1}))["text"], "3: three\n");
    assert_eq!(read(&db, "alice", json!({"output_id":id,"json_pointer":"/rows/0/value"}))["text"], "42");
    assert!(tool_output::run(&db, "alice", &json!({"output_id":id,"json_pointer":"/missing"})).is_err());
}

#[test]
fn tool_output_long_single_line_search_pages_do_not_lose_later_matches() {
    let (_dir, db) = fixture();
    let text = format!("hit {}\nhit last\n", "x".repeat(20_000));
    let id = save(&db, "alice", &text, 10);
    let mut args = json!({"output_id":id,"view":"search","query":"hit","line_count":1,"max_chars":1000});
    let mut all = String::new();
    loop {
        let page = read(&db, "alice", args);
        all.push_str(page["text"].as_str().unwrap());
        if page["next"].is_null() { break; }
        args = page["next"].clone();
    }
    assert_eq!(all, format!("1: hit {}\n2: hit last\n", "x".repeat(20_000)));
}

#[test]
fn tool_output_scope_rejects_other_users_sessions_and_disabled_reader() {
    let (_dir, db) = fixture();
    let id = save(&db, "alice", "private result", 0);
    assert!(tool_output::run(&db, "bob", &json!({"output_id":id})).is_err());
    let mut ctx = db.load_context("alice").unwrap();
    ctx.session_id = "other".into(); db.save_context(&ctx).unwrap();
    assert!(tool_output::run(&db, "alice", &json!({"output_id":id})).is_err());
    ctx.session_id.clear(); db.save_context(&ctx).unwrap();
    assert_eq!(read(&db, "alice", json!({"output_id":id}))["text"], "private result");
    crate::db::tools::set_plugin_tool_enabled(&db, "read_tool_result", false).unwrap();
    assert!(tool_output::run(&db, "alice", &json!({"output_id":id})).is_err());
    crate::db::tools::init_default_tools(&db).unwrap();
    // Startup never re-enables a tool the operator disabled.
    assert!(!crate::db::tools::tool_enabled(&db, "read_tool_result").unwrap());
}

#[test]
fn tool_output_compaction_keeps_cache_but_explicit_clear_removes_it() {
    let (_dir, db) = fixture();
    let id = save(&db, "alice", "original", 0);
    db.retain_chat_messages("alice", &[]).unwrap();
    assert!(db.get_tool_output("alice", &id).unwrap().is_some());
    db.clear_messages("alice").unwrap();
    assert!(db.get_tool_output("alice", &id).unwrap().is_none());
    let id = save(&db, "alice", "new", 0);
    db.clear_session_messages("alice", "default").unwrap();
    assert!(db.get_tool_output("alice", &id).unwrap().is_none());
    let id = save(&db, "alice", "newer", 0);
    db.delete_context("alice").unwrap();
    assert!(db.get_tool_output("alice", &id).unwrap().is_none());
}

#[test]
fn tool_output_delete_one_session_and_literal_user_preserves_other_owners() {
    let (_dir, db) = fixture();
    let mut ctx = db.load_context("a_%").unwrap();
    ctx.session_id = "s".into(); db.save_context(&ctx).unwrap();
    let original = save(&db, "a_%", "private", 0);
    let other = save(&db, "alice", "other private", 0);
    db.delete_session("a_%", "s").unwrap();
    assert!(db.get_tool_output("a_%", &original).unwrap().is_none());
    assert!(db.get_tool_output("alice", &other).unwrap().is_some());
    db.delete_context("a_%").unwrap();
    assert!(db.get_tool_output("alice", &other).unwrap().is_some());
}

#[test]
fn tool_output_expiry_quotas_and_capture_loss_are_explicit() {
    let (_dir, db) = fixture();
    let old = save(&db, "alice", "old", 0);
    db.conn().execute("UPDATE tool_outputs SET created_at=0", []).unwrap();
    assert!(db.get_tool_output("alice", &old).unwrap().is_none());
    db.prune_tool_outputs().unwrap();
    assert_eq!(db.conn().query_row("SELECT count(*) FROM tool_outputs", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
    let text = "😀".repeat(crate::db::tool_outputs::MAX_OUTPUT_BYTES / 4 + 1);
    let id = save(&db, "alice", &text, 10);
    let page = read(&db, "alice", json!({"output_id":id,"max_chars":10}));
    assert_eq!(page["storage_truncated"], true);
    assert_eq!(page["source_bytes"], text.len());
    assert!(page["retained_bytes"].as_u64().unwrap() < page["source_bytes"].as_u64().unwrap());
    for _ in 0..crate::db::tool_outputs::MAX_USER_OUTPUTS { save(&db, "alice", "small", 0); }
    assert!(db.get_tool_output("alice", &id).unwrap().is_none());
    let count: usize = db.conn().query_row("SELECT count(*) FROM tool_outputs WHERE owner_id='alice'", [], |r| r.get(0)).unwrap();
    assert_eq!(count, crate::db::tool_outputs::MAX_USER_OUTPUTS);
}

#[test]
fn tool_output_invalid_queries_do_not_fall_back_to_full_disclosure() {
    let (_dir, db) = fixture();
    let id = save(&db, "alice", "a\0b\n😀\n", 1);
    for extra in [json!({"max_chars":0}), json!({"max_chars":32001}), json!({"offset":-1}),
                  json!({"offset":999}), json!({"line_count":0}), json!({"start_line":0}),
                  json!({"view":"invalid"}), json!({"view":"search"}), json!({"view":"search","query":""}),
                  json!({"json_pointer":"invalid"}), json!({"user_id":"bob"})] {
        let mut args = extra; args["output_id"] = json!(id);
        assert!(tool_output::run(&db, "alice", &args).is_err(), "{args}");
    }
    assert_eq!(read(&db, "alice", json!({"output_id":id}))["text"], "a\0b\n😀\n");
    let blank = save(&db, "alice", "", 0);
    let page = read(&db, "alice", json!({"output_id":blank}));
    assert_eq!(page["text"], ""); assert!(page["next"].is_null());
}

#[test]
fn tool_output_default_is_full_even_with_legacy_zero_limit_and_over_32000_chars() {
    let (_dir, db) = fixture();
    let text = format!("{}END_SENTINEL", "😀abc\n".repeat(12_000));
    for legacy_limit in [0, 1, 2000] {
        db.merge_context("alice", json!({"settings.tool_result_limit":legacy_limit})).unwrap();
        let delivered = tool_results::prepare(&db, "alice", "plugin", "id", &text).unwrap();
        assert!(delivered.starts_with(&text));
        assert!(!delivered.contains("Preview:"));
        let id = delivered.split("output_id=").nth(1).unwrap().split_whitespace().next().unwrap();
        let page = read(&db, "alice", json!({"output_id":id}));
        assert_eq!(page["text"], text);
        assert!(page["next"].is_null());
    }
}

#[test]
fn tool_output_direct_parameter_selects_model_view_and_is_not_forwarded() {
    use crate::gateway::llm::provider::{ToolCall, FunctionCall};
    let (_dir, db) = fixture();
    let text = "start\nselected α\nprivate tail\n";
    let args = json!({"command":"synthetic","_output":{"view":"lines","start_line":2,"line_count":1}});
    assert_eq!(tool_output::execution_args(&args).unwrap(), json!({"command":"synthetic"}));
    let call = ToolCall { id:"call".into(), function:FunctionCall {name:"execute_terminal".into(),arguments:args.to_string()} };
    let selected: Value = serde_json::from_str(&tool_results::prepare_for_call(&db, "alice", &call, text).unwrap()).unwrap();
    assert_eq!(selected["text"], "2: selected α\n");
    assert!(!selected.to_string().contains("private tail"));
    assert_eq!(read(&db, "alice", json!({"output_id":selected["output_id"]}))["text"], text);
    let mut call = call;
    call.function.arguments = json!({"_output":{"json_pointer":"/not-json"}}).to_string();
    let failed: Value = serde_json::from_str(&tool_results::prepare_for_call(&db, "alice", &call, text).unwrap()).unwrap();
    assert_eq!(failed["tool_executed"], true);
    assert!(!failed.to_string().contains("private tail"));
    assert!(failed["output_id"].as_str().unwrap().starts_with("out_"));
    for selection in [json!(false), json!({"view":"unknown"}), json!({"max_chars":0}), json!({"output_id":"out_00000000000000000000000000000000"})] {
        assert!(tool_output::execution_args(&json!({"command":"must-not-run","_output":selection})).is_err());
    }
    let plugins = crate::plugins::load_all_plugins(std::path::Path::new("plugins"));
    let catalog = crate::tools::discovery::catalog(&db, &plugins).unwrap();
    assert!(catalog.iter().any(|t| t.function.name == "brave_web_search"));
    for tool in catalog.iter().filter(|t| t.function.name != "read_tool_result") {
        assert_eq!(tool.function.parameters["properties"]["_output"]["type"], "object", "{}", tool.function.name);
        assert!(!tool.function.parameters["required"].as_array().is_some_and(|v| v.contains(&json!("_output"))));
    }
}

#[tokio::test]
#[cfg(feature = "legacy_file_ops")]
async fn tool_output_file_capture_does_not_lose_the_old_10000_char_tail() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("large.txt");
    let text = format!("{}FILE_END", "large\n".repeat(10_000));
    std::fs::write(&path, &text).unwrap();
    assert_eq!(crate::tools::read_file::run(path.to_str().unwrap()).await.unwrap(), text);
    let file = std::fs::File::create(&path).unwrap();
    file.set_len(crate::db::tool_outputs::MAX_OUTPUT_BYTES as u64 + 1).unwrap();
    assert!(crate::tools::read_file::run(path.to_str().unwrap()).await.is_err());
}

#[test]
fn tool_output_reader_pages_are_not_retruncated_or_recursively_archived() {
    let (_dir, db) = fixture();
    let id = save(&db, "alice", &"x".repeat(9000), 1);
    let page = tool_output::run(&db, "alice", &json!({"output_id":id,"max_chars":8000})).unwrap();
    let before: usize = db.conn().query_row("SELECT count(*) FROM tool_outputs", [], |r| r.get(0)).unwrap();
    assert_eq!(tool_results::prepare(&db, "alice", "read_tool_result", "read", &page).unwrap(), page);
    let after: usize = db.conn().query_row("SELECT count(*) FROM tool_outputs", [], |r| r.get(0)).unwrap();
    assert_eq!(before, after);
    let defs = crate::tools::discovery::definitions(&db, &crate::plugins::PluginRegistry::new(), "alice").unwrap();
    assert!(defs.iter().any(|t| t.function.name == "read_tool_result"));
}
