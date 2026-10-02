//! Trusted, editable Decision request profiles. Selecting a profile is policy,
//! not permission: it may only map finite labels to states of the active workflow.
use anyhow::{ensure, Context as _};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, path::{Path,PathBuf}};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DecisionProfile {
    #[serde(default)]
    pub backend: DecisionBackend,
    pub endpoint: String,
    pub model: String,
    pub instructions: String,
    /// Optional POML view of decision_input; never an action source.
    #[serde(default)]
    pub input_template: Option<String>,
    pub schema: Value,
    pub state_field: String,
    pub state_map: BTreeMap<String,String>,
    /// Optional System One descriptions; otherwise state targets describe choices.
    #[serde(default)]
    pub criteria: BTreeMap<String,String>,
    #[serde(default="default_mode")]
    pub mode: String,
    #[serde(default="default_cache")]
    pub cache_prompt: bool,
    #[serde(default="default_timeout")]
    pub timeout_ms: u64,
    #[serde(default="default_probability")]
    pub minimum_probability: f64,
    #[serde(default="default_context_limit")]
    pub max_context_chars: usize,
    #[serde(default)]
    pub reevaluate: Reevaluate,
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all="snake_case")]
pub enum DecisionBackend { #[default] Native, Ollama }
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all="snake_case")]
pub enum Reevaluate { TaskEntry, #[default] EveryStep }
fn default_mode()->String { "auto".into() }
fn default_cache()->bool { true }
fn default_timeout()->u64 { 2000 }
fn default_probability()->f64 { 0.8 }
fn default_context_limit()->usize { 12000 }

impl DecisionProfile {
    pub fn choices(&self)->anyhow::Result<Vec<&str>> {
        let field=&self.schema[&self.state_field];
        ensure!(field["type"]=="enum", "State field must use the Decision enum schema");
        let values=field["choices"].as_array().context("State field needs choices")?;
        ensure!(!values.is_empty() && values.len()<=64, "Use 1–64 routing choices");
        let mut choices=Vec::new();
        for value in values {
            let label=value.as_str().context("Routing choices must be strings")?;
            ensure!(!label.is_empty() && label.len()<=128 && !label.chars().any(char::is_control), "Invalid routing label");
            ensure!(!choices.contains(&label), "Duplicate routing label");
            choices.push(label);
        }
        Ok(choices)
    }
    pub fn validate(&self)->anyhow::Result<()> {
        let url=reqwest::Url::parse(&self.endpoint).context("Invalid Decision endpoint URL")?;
        ensure!(matches!(url.scheme(),"http"|"https") && url.host_str().is_some(), "Decision endpoint must be HTTP(S)");
        ensure!(url.username().is_empty() && url.password().is_none() && url.query().is_none() && url.fragment().is_none(), "Do not embed credentials, query strings or fragments in a Decision endpoint");
        ensure!(!self.model.trim().is_empty() && self.model.len()<=256, "A served model alias is required");
        ensure!(!self.instructions.trim().is_empty() && self.instructions.len()<=32000,"Instructions must be 1–32000 bytes");
        ensure!(self.schema.is_object() && self.schema.to_string().len()<=32000,"Schema must be an object of at most 32000 bytes");
        let maximum_timeout=if self.backend==DecisionBackend::Ollama {300000} else {10000};
        ensure!(self.timeout_ms>=50 && self.timeout_ms<=maximum_timeout,"Decision timeout outside backend limits");
        ensure!(self.max_context_chars>=256 && self.max_context_chars<=32000,"Context bound must be 256–32000 characters");
        ensure!(self.minimum_probability.is_finite() && (0.0..=1.0).contains(&self.minimum_probability),"Probability threshold must be in [0,1]");
        ensure!(!self.mode.is_empty() && self.mode.len()<=32 && !self.mode.chars().any(char::is_control),"Invalid Decision mode");
        let choices=self.choices()?;
        if self.backend==DecisionBackend::Ollama {
            ensure!(url.path().ends_with("/v1/systemone"),"Ollama Decision endpoint must end in /v1/systemone");
            ensure!((2..=26).contains(&choices.len()),"System One choice questions require 2–26 choices");
            if !self.criteria.is_empty() {
                ensure!(self.criteria.len()==choices.len(),"Describe every System One choice exactly once");
                for label in &choices {
                    let description=self.criteria.get(*label).context("Missing System One choice description")?;
                    ensure!(!description.trim().is_empty() && description.len()<=4096,"Invalid System One choice description");
                }
            }
        }
        ensure!(self.state_map.len()==choices.len(),"Map every routing choice exactly once");
        for label in choices {
            let state=self.state_map.get(label).context("Missing routing choice in state_map")?;
            ensure!(!state.is_empty() && state!="_default" && state.len()<=128 && !state.chars().any(char::is_control),"Invalid state target");
        }
        Ok(())
    }
}

fn filename(name:&str)->anyhow::Result<String> {
    let name=name.strip_suffix(".json").unwrap_or(name);
    ensure!(!name.is_empty() && name.len()<=64 && name.as_bytes()[0].is_ascii_alphanumeric() && name.bytes().all(|c|c.is_ascii_alphanumeric()||c==b'_'||c==b'-'),"Decision profile names use letters, digits, _ and - only");
    Ok(format!("{name}.json"))
}
pub fn resolve(root:&Path,name:&str)->anyhow::Result<PathBuf> {
    let root=root.canonicalize()?;
    let path=root.join(filename(name)?).canonicalize()?;
    ensure!(path.starts_with(&root) && path.is_file(),"Decision profile escapes its directory");
    ensure!(path.metadata()?.len()<=65536,"Decision profile exceeds 64 KiB");
    Ok(path)
}
pub fn load(root:&Path,name:&str)->anyhow::Result<DecisionProfile> {
    let text=std::fs::read_to_string(resolve(root,name)?)?;
    let profile:DecisionProfile=serde_json::from_str(&text)?;
    profile.validate()?;
    Ok(profile)
}
pub fn save(root:&Path,name:&str,text:&str)->anyhow::Result<()> {
    let file=filename(name)?;
    ensure!(text.len()<=65536,"Decision profile exceeds 64 KiB");
    let profile:DecisionProfile=serde_json::from_str(text)?;
    profile.validate()?;
    std::fs::create_dir_all(root)?;
    let target=root.join(file);
    ensure!(!std::fs::symlink_metadata(&target).is_ok_and(|m|m.file_type().is_symlink()),"Refusing to replace a symlink");
    let mut tmp=tempfile::NamedTempFile::new_in(root)?;
    use std::io::Write;
    tmp.write_all(text.as_bytes())?;tmp.as_file().sync_all()?;
    tmp.persist(target).map_err(|e|e.error)?;
    Ok(())
}
pub fn list(root:&Path)->anyhow::Result<Vec<String>> {
    if !root.exists() { return Ok(vec![]); }
    let mut names=vec![];
    for entry in std::fs::read_dir(root)? {
        let entry=entry?;let path=entry.path();
        if path.extension().and_then(|x|x.to_str())!=Some("json") || !entry.file_type()?.is_file() {continue;}
        if let Some(name)=path.file_stem().and_then(|x|x.to_str()) {
            if filename(name).is_ok() {names.push(name.to_string());}
        }
    }
    names.sort();Ok(names)
}
