use super::*;
use serde_json::json;
use tempfile::TempDir;

fn fixture() -> (TempDir, crate::db::Database, std::path::PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("skills");
    std::fs::create_dir(&root).unwrap();
    let db = crate::db::Database::new(&temp.path().join("data")).unwrap();
    crate::db::tools::init_default_tools(&db).unwrap();
    (temp, db, root)
}

fn put(root: &Path, folder: &str, name: &str, hidden: bool, user_only: bool) {
    let dir = root.join(folder);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("skill.json"),
        json!({
            "name": name, "description": "Memory palace Gedächtnispalast Unicode 日本語",
            "required_parameters": ["user_request"], "skill_hidden": hidden, "user_only": user_only
        })
        .to_string(),
    )
    .unwrap();
    // Intentionally not renderable: metadata discovery must never read/render this.
    std::fs::write(
        dir.join("skill.poml"),
        "BODY_MUST_NOT_BE_LOADED {{missing}}".repeat(20000),
    )
    .unwrap();
}

#[test]
fn skill_index_is_persistent_bounded_and_metadata_only() {
    let (_temp, db, root) = fixture();
    for i in 0..75 {
        put(
            &root,
            &format!("group/s{i:03}"),
            &format!("s{i:03}"),
            false,
            false,
        );
    }
    let mut index = SkillIndex::open(&db.data_dir(), &root).unwrap();
    assert_eq!(index.rebuild().unwrap().indexed, 75);
    let result = index.search("memory", 5, false).unwrap();
    assert_eq!(result.skills.len(), 5);
    assert!(result.has_more);
    let text = serde_json::to_string(&result).unwrap();
    assert!(!text.contains("BODY_MUST_NOT_BE_LOADED") && !text.contains("skill.poml"));
    drop(index);
    // Unindexed additions do not cause per-query scans. One-folder refresh is explicit.
    put(&root, "later", "later", false, false);
    let mut index = SkillIndex::open(&db.data_dir(), &root).unwrap();
    assert_eq!(index.search("later", 5, false).unwrap().skills.len(), 0);
    index.refresh("later").unwrap();
    assert_eq!(
        index.search("later", 5, false).unwrap().skills[0].name,
        "later"
    );
    assert!(index.search("memory", 1000, false).is_err());
    assert!(index.search(&"x".repeat(513), 5, false).is_err());
    assert!(index.search("\" OR * NOT", 5, false).is_ok());
    assert!(!index.search("日本語", 5, false).unwrap().skills.is_empty());
}

#[tokio::test]
async fn skill_visibility_and_user_only_are_distinct_and_rechecked_live() {
    let (_temp, db, root) = fixture();
    put(&root, "normal", "normal", false, false);
    put(&root, "hidden", "hidden", true, false);
    put(&root, "manual", "manual", false, true);
    let mut index = SkillIndex::open(&db.data_dir(), &root).unwrap();
    index.rebuild().unwrap();
    let result = index.search("memory", 10, false).unwrap();
    assert_eq!(result.skills.len(), 2);
    assert!(result
        .skills
        .iter()
        .any(|s| s.name == "manual" && s.user_only));
    assert!(!result.skills.iter().any(|s| s.name == "hidden"));
    assert_eq!(index.search("memory", 10, true).unwrap().skills.len(), 3);
    // Exact-name resolution may load a hidden dependency, but never user_only from a tool.
    assert!(lookup_skill(&db, &root, "hidden").is_ok());
    let args = json!({"name":"manual", "parameters":{"user_request":"task"}});
    let error = crate::tools::use_skill::run_in(&db, &args, &root)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("user-only"), "{error}");
    put(&root, "normal", "normal", true, true);
    assert!(!index
        .search("normal", 10, false)
        .unwrap()
        .skills
        .iter()
        .any(|s| s.name == "normal"));
    let error =
        crate::tools::use_skill::run_in(&db, &json!({"name":"normal","parameters":{}}), &root)
            .await
            .unwrap_err()
            .to_string();
    assert!(error.contains("user-only"), "{error}");
}

#[test]
fn skill_index_registered_policy_cannot_be_shadowed_by_a_new_folder() {
    let (_temp, db, root) = fixture();
    put(&root, "group/manual", "manual", false, true);
    let mut index = SkillIndex::open(&db.data_dir(), &root).unwrap();
    index.rebuild().unwrap();
    put(&root, "manual", "manual", false, false);
    assert!(lookup_skill(&db, &root, "manual").unwrap().user_only);
    assert!(index.refresh("manual").is_err());
    assert!(index.lookup("manual").unwrap().user_only);
}

#[test]
fn skill_index_handles_deletion_duplicates_and_path_escape() {
    let (_temp, db, root) = fixture();
    put(&root, "a", "a", false, false);
    let mut index = SkillIndex::open(&db.data_dir(), &root).unwrap();
    index.rebuild().unwrap();
    put(&root, "duplicate", "a", false, false);
    assert!(index.rebuild().is_err()); // transaction preserves previous valid index
    assert!(index.lookup("a").is_ok());
    assert!(index.refresh("../outside").is_err());
    assert!(index.lookup("../a").is_err());
    std::fs::remove_file(root.join("a/skill.json")).unwrap();
    assert!(index.lookup("a").is_err());
    assert!(index.search("memory", 10, false).unwrap().skills.is_empty());
}

#[tokio::test]
async fn skill_prompt_is_constant_size_and_does_not_scan_directory() {
    let (_temp, db, root) = fixture();
    let ctx = crate::db::contexts::Context {
        user_id: "alice".into(),
        ..Default::default()
    };
    // A non-directory would fail the old eager registry scan.
    std::fs::remove_dir(&root).unwrap();
    std::fs::write(&root, "not a directory").unwrap();
    let value = crate::gateway::prompt::build_context(
        &db,
        &ctx,
        "task",
        &crate::plugins::PluginRegistry::new(),
        0,
        root.parent().unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(value["skills"], json!([]));
    assert_eq!(value["active_skill_instructions"], "");
}

#[tokio::test]
#[ignore = "Requires Node and POML_CLI"]
async fn skill_discovery_poml_controls_queries_without_loading_bodies() {
    let (_temp, db, root) = fixture();
    put(&root, "normal", "normal", false, false);
    put(&root, "hidden", "hidden", true, false);
    put(&root, "manual", "manual", false, true);
    let install = root.parent().unwrap();
    std::fs::create_dir_all(install.join("templates/discovery")).unwrap();
    let policy = install.join("templates/discovery/skills.md");
    std::fs::write(
        &policy,
        include_str!("../../templates/discovery/skills.md"),
    )
    .unwrap();
    let mut value = json!({"skills":[], "custom_data":{}, "user_prompt":"palace"});
    discovery::enrich(&db, &mut value, install).await.unwrap();
    assert_eq!(value["skills"], json!([]));
    assert!(!db.data_dir().join("skill-index.sqlite").exists());
    assert!(value["skill_discovery_instructions"]
        .as_str()
        .unwrap()
        .contains("search_skills"));
    std::fs::write(&policy, r#"<poml><p>{{JSON.stringify({instructions:'CUSTOM_POLICY', names:['hidden'], queries:user_prompt === 'palace' ? ['memory'] : [], limit:2})}}</p></poml>"#).unwrap();
    discovery::enrich(&db, &mut value, install).await.unwrap();
    assert_eq!(value["skill_discovery_instructions"], "CUSTOM_POLICY");
    let results = value["skills"].as_array().unwrap();
    assert_eq!(results.len(), 2);
    assert!(results.iter().any(|s| s["user_only"] == true));
    assert!(!value.to_string().contains("BODY_MUST_NOT_BE_LOADED"));
    assert!(!results.iter().any(|s| s["name"] == "hidden"));
    std::fs::write(&policy, "<poml><p>not a JSON plan</p></poml>").unwrap();
    assert!(discovery::enrich(&db, &mut value, install).await.is_err());
}

#[test]
fn skill_discovery_plan_limits_are_enforced() {
    for input in [
        json!({"limit":0}),
        json!({"limit":21}),
        json!({"names":["../escape"]}),
        json!({"include_hidden":true}),
        json!({"instructions":"x".repeat(4097)}),
    ] {
        assert!(discovery::DiscoveryPlan::parse(&input.to_string()).is_err());
    }
}

#[test]
fn skill_manifest_defaults_and_size_limits() {
    let (_temp, _db, root) = fixture();
    put(&root, "a", "a", false, false);
    let mut manifest = json!({"name":"a","description":"Test","required_parameters":[]});
    std::fs::write(root.join("a/skill.json"), manifest.to_string()).unwrap();
    let skill = Skill::from_dir(&root, &root.join("a")).unwrap();
    assert!(!skill.skill_hidden && !skill.user_only);
    manifest["description"] = json!("x".repeat(1025));
    std::fs::write(root.join("a/skill.json"), manifest.to_string()).unwrap();
    assert!(Skill::from_dir(&root, &root.join("a")).is_err());
}
