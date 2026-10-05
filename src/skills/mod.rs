use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::io::Read;

mod index;
pub use index::{lookup_skill, SkillIndex};
pub mod discovery;

pub fn validate_name(name: &str) -> anyhow::Result<()> {
    anyhow::ensure!(!name.is_empty() && name.len() <= 64 && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'), "Invalid skill name (1..64 letters, digits, _ or -)");
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    pub name: String,
    pub description: String,
    /// Required non-empty string inputs, injected as top-level POML variables.
    #[serde(default)]
    pub required_parameters: Vec<String>,
    /// Omit metadata from model discovery. Exact-name dependencies remain usable.
    #[serde(default)]
    pub skill_hidden: bool,
    /// Only a human context selection may activate this skill, never use_skill.
    #[serde(default)]
    pub user_only: bool,
    /// Optional semantic version from the manifest, shown in /skill listings.
    #[serde(default)]
    pub version: Option<String>,
    #[serde(skip)]
    pub folder: String,
}

impl Skill {
    /// Read a bounded manifest only. Never read/render instruction bodies here.
    pub fn from_dir(root: &Path, folder: &Path) -> anyhow::Result<Self> {
        let root = root.canonicalize()?;
        let folder = if folder.is_absolute() { folder.to_path_buf() } else { std::env::current_dir()?.join(folder) };
        let relative = folder.strip_prefix(&root)?;
        let mut checked = root.clone();
        for component in relative.components() {
            anyhow::ensure!(matches!(component, std::path::Component::Normal(_)), "Invalid skill folder");
            checked.push(component);
            let meta = std::fs::symlink_metadata(&checked)?;
            anyhow::ensure!(meta.is_dir() && !meta.file_type().is_symlink(), "Skill folders must be local directories, not symlinks");
        }
        for name in ["skill.json", "skill.poml"] {
            let meta = std::fs::symlink_metadata(checked.join(name))?;
            anyhow::ensure!(meta.is_file() && !meta.file_type().is_symlink(), "Skill files must be regular local files");
        }
        let mut bytes = Vec::new();
        std::fs::File::open(checked.join("skill.json"))?.take(16385).read_to_end(&mut bytes)?;
        anyhow::ensure!(bytes.len() <= 16384, "Skill manifest exceeds 16 KiB");
        let mut skill: Skill = serde_json::from_slice(&bytes)?;
        validate_name(&skill.name)?;
        anyhow::ensure!(!skill.description.trim().is_empty() && skill.description.chars().count() <= 1024, "Skill description must contain 1..1024 characters");
        anyhow::ensure!(skill.required_parameters.len() <= 16, "At most 16 required skill parameters");
        for key in &skill.required_parameters { validate_name(key)?; }
        if let Some(version) = &skill.version {
            let v = version.trim();
            anyhow::ensure!(!v.is_empty() && v.chars().count() <= 32, "Skill version must contain 1..32 characters");
            skill.version = Some(v.to_string());
        }
        skill.folder = checked.to_string_lossy().to_string();
        Ok(skill)
    }

    /// Automatic activation maps only the documented task-input aliases.
    /// Other required inputs must already exist in the shared context.
    pub fn parameters_for_task(&self, context: &serde_json::Value, task: &str) -> anyhow::Result<serde_json::Value> {
        let mut parameters = context.clone();
        anyhow::ensure!(parameters.is_object(), "Skill context must be an object");
        for key in ["code", "error", "user_request"] {
            parameters[key] = serde_json::json!(task);
        }
        self.validate_parameters(&parameters)?;
        Ok(parameters)
    }

    pub fn validate_parameters(&self, parameters: &serde_json::Value) -> anyhow::Result<()> {
        anyhow::ensure!(
            parameters.is_object(),
            "Skill parameters must be a JSON object"
        );
        for key in &self.required_parameters {
            anyhow::ensure!(
                parameters
                    .get(key)
                    .and_then(|v| v.as_str())
                    .is_some_and(|s| !s.trim().is_empty()),
                "Skill '{}' requires a non-empty string parameter '{}'",
                self.name,
                key
            );
        }
        Ok(())
    }
}

pub struct SkillRegistry {
    skills: HashMap<String, Skill>,
}

impl SkillRegistry {
    pub fn new() -> Self {
        Self {
            skills: HashMap::new(),
        }
    }

    pub fn load_from_dir(&mut self, dir: &Path) -> anyhow::Result<()> {
        if !dir.exists() {
            return Ok(());
        }

        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                let skill_json = path.join("skill.json");
                let skill_poml = path.join("skill.poml");

                if skill_json.exists() && skill_poml.exists() {
                    // A broken optional skill must not hide unrelated valid skills.
                    let loaded = (|| -> anyhow::Result<Skill> {
                        Skill::from_dir(dir, &path)
                    })();
                    match loaded {
                        Ok(skill) => {
                            self.skills.insert(skill.name.clone(), skill);
                        }
                        Err(error) => {
                            tracing::warn!(path = %skill_json.display(), %error, "Skipping invalid skill")
                        }
                    }
                }
            }
        }

        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<&Skill> {
        self.skills.get(name)
    }

    pub fn list(&self) -> Vec<&Skill> {
        let mut skills: Vec<_> = self.skills.values().collect();
        skills.sort_by(|a, b| a.name.cmp(&b.name));
        skills
    }

    /// Return skills as JSON array for POML context
    pub fn to_context_array(&self) -> serde_json::Value {
        let skills: Vec<serde_json::Value> = self
            .list()
            .into_iter()
            .filter(|s| !s.skill_hidden)
            .take(index::MAX_RESULTS)
            .map(|s| {
                serde_json::json!({
                    "name": s.name,
                    "description": s.description,
                    "required_parameters": s.required_parameters,
                    "user_only": s.user_only
                })
            })
            .collect();
        serde_json::json!(skills)
    }
}

pub async fn execute_skill(skill: &Skill, context: &serde_json::Value) -> anyhow::Result<String> {
    skill.validate_parameters(context)?;
    let poml_path = format!("{}/skill.poml", skill.folder);
    tracing::info!("Rendering skill: {} from {}", skill.name, poml_path);
    // Never present unrendered fallback markup as a successfully loaded skill.
    let mut parameters = context.clone();
    parameters["skill_dir"] = serde_json::json!(skill.folder);
    crate::runtime::engine::render_strict(&poml_path, &parameters).await
}

pub async fn execute_skill_by_name(
    registry: &SkillRegistry,
    name: &str,
    context: &serde_json::Value,
) -> anyhow::Result<String> {
    let skill = registry
        .get(name)
        .ok_or_else(|| anyhow::anyhow!("Skill not found: {}", name))?;
    execute_skill(skill, context).await
}

#[cfg(test)]
mod discovery_tests;

#[cfg(test)]
mod skill_tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn create_test_skill(dir: &Path, name: &str) {
        let skill_dir = dir.join(name);
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("skill.json"),
            format!(r#"{{"name": "{}", "description": "Test skill"}}"#, name),
        )
        .unwrap();
        fs::write(
            skill_dir.join("skill.poml"),
            "<poml><p>Hello {{name}}</p></poml>",
        )
        .unwrap();
    }

    #[test]
    fn test_load_skills() {
        let dir = TempDir::new().unwrap();
        create_test_skill(dir.path(), "test");
        let mut registry = SkillRegistry::new();
        registry.load_from_dir(dir.path()).unwrap();
        assert!(registry.get("test").is_some());
    }

    #[test]
    fn test_skill_missing_poml() {
        let dir = TempDir::new().unwrap();
        let skill_dir = dir.path().join("bad");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("skill.json"),
            r#"{"name":"bad","description":"x"}"#,
        )
        .unwrap();
        let mut registry = SkillRegistry::new();
        registry.load_from_dir(dir.path()).unwrap();
        assert!(registry.get("bad").is_none());
    }

    #[test]
    fn test_to_context_array() {
        let dir = TempDir::new().unwrap();
        create_test_skill(dir.path(), "a");
        create_test_skill(dir.path(), "b");
        let mut registry = SkillRegistry::new();
        registry.load_from_dir(dir.path()).unwrap();
        let arr = registry.to_context_array();
        assert!(arr.is_array());
        assert_eq!(arr.as_array().unwrap().len(), 2);
    }

    #[test]
    fn test_invalid_manifest_does_not_hide_valid_skills() {
        let dir = TempDir::new().unwrap();
        create_test_skill(dir.path(), "valid");
        create_test_skill(dir.path(), "broken");
        fs::write(dir.path().join("broken/skill.json"), "not JSON").unwrap();
        let mut registry = SkillRegistry::new();
        registry.load_from_dir(dir.path()).unwrap();
        assert!(registry.get("valid").is_some());
        assert!(registry.get("broken").is_none());
    }

    #[test]
    fn test_required_parameters_are_advertised_and_checked() {
        let skill: Skill = serde_json::from_value(serde_json::json!({
            "name": "debug", "description": "Debug errors", "required_parameters": ["error"]
        }))
        .unwrap();
        for context in [
            serde_json::json!({}),
            serde_json::json!([]),
            serde_json::json!({"error": null}),
            serde_json::json!({"error": " "}),
            serde_json::json!({"error": 42}),
        ] {
            assert!(skill.validate_parameters(&context).is_err());
        }
        assert!(skill
            .validate_parameters(&serde_json::json!({"error": "test"}))
            .is_ok());
        let mut registry = SkillRegistry::new();
        registry.skills.insert(skill.name.clone(), skill);
        assert_eq!(
            registry.to_context_array()[0]["required_parameters"],
            serde_json::json!(["error"])
        );
    }

    #[test]
    fn backend_active_skill_maps_task_aliases_without_inventing_inputs() {
        for key in ["code", "error", "user_request"] {
            let skill: Skill = serde_json::from_value(serde_json::json!({"name":"test", "description":"Test", "required_parameters":[key]})).unwrap();
            let parameters = skill.parameters_for_task(&serde_json::json!({"user_prompt":"old"}), "current task").unwrap();
            assert_eq!(parameters[key], "current task");
        }
        let skill: Skill = serde_json::from_value(serde_json::json!({"name":"test", "description":"Test", "required_parameters":["unprovided"]})).unwrap();
        assert!(skill.parameters_for_task(&serde_json::json!({}), "task").is_err());
    }

    #[test]
    fn test_empty_registry() {
        let registry = SkillRegistry::new();
        assert!(registry.list().is_empty());
        assert!(registry.to_context_array().as_array().unwrap().is_empty());
    }
}
