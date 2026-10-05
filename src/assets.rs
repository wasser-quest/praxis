//! Public runtime assets shared by interactive onboarding and offline repair.
//! No configuration, credentials, databases, provider calls or activation changes.
use anyhow::Context as _;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use praxis_plugin_api::web::PackagedAsset as Asset;

macro_rules! asset {
    ($path:literal) => {
        Asset {
            path: $path,
            bytes: include_bytes!(concat!("../", $path)),
            executable: false,
            dashboard: false,
        }
    };
    ($path:literal, executable) => {
        Asset {
            path: $path,
            bytes: include_bytes!(concat!("../", $path)),
            executable: true,
            dashboard: false,
        }
    };
}

// Deliberate allow-list: never recursively bundle arbitrary user files/secrets.
// A coverage regression guards all shipped POML/JSON/SM/skill dependencies.
const BUNDLED_ASSETS: &[Asset] = &[
    asset!("contexts/standard.sm"),
    asset!("contexts/20-tasks.sm"),
    asset!("contexts/verified-coding.sm"),
    asset!("contexts/verified-capabilities.sm"),
    asset!("contexts/verified-implementation.sm"),
    asset!("contexts/branching-coding.sm"),
    asset!("contexts/standard-verified.sm"),
    asset!("contexts/language-learning.sm"),
    asset!("templates/graph-coding.poml"),
    asset!("templates/language-flow.poml"),
    asset!("templates/verified-implementation.poml"),
    asset!("templates/20-tasks.poml"),
    asset!("workflows/tts-api.json"),
    asset!("workflows/tts-qwen3-api.json"),
    asset!("integrations/comfyui_audio/__init__.py"),
    asset!("integrations/comfyui_audio/node.py"),
    asset!("integrations/comfyui_audio/worker.py"),
    asset!("integrations/comfyui_audio/protocol.py"),
    asset!("integrations/comfyui_audio/warm_worker.py"),
    asset!("integrations/comfyui_audio/segments.py"),
    asset!("integrations/comfyui_audio/audio.py"),
    asset!("integrations/comfyui_audio/download_qwen_model.py"),
    asset!("integrations/comfyui_audio/qwen-requirements.txt"),
    asset!("integrations/comfyui_audio/README.md"),
    asset!("templates/standard.poml"),
    asset!("templates/system.poml"),
    asset!("templates/language_learning.poml"),
    asset!("templates/daily_quiz.poml"),
    asset!("templates/personas/code.poml"),
    asset!("templates/personas/code_architect.poml"),
    asset!("templates/personas/senior_dev.poml"),
    asset!("templates/personas/expert_programmer.poml"),
    asset!("templates/personas/debugger.poml"),
    asset!("templates/personas/review.poml"),
    asset!("templates/personas/language_teacher.poml"),
    asset!("templates/personas/research.poml"),
    asset!("templates/user.poml"),
    asset!("templates/compaction.poml"),
    asset!("templates/blueprints/standard.json"),
    asset!("templates/blueprints/language_instructor.json"),
    asset!("templates/blueprints/code_assistant.json"),
    asset!("templates/blueprints/researcher.json"),
    asset!("templates/shared/blueprint.poml"),
    asset!("templates/shared/runtime.poml"),
    asset!("templates/shared/runtime_minimal.poml"),
    asset!("templates/shared/output_format.poml"),
    asset!("templates/shared/state_base.poml"),
    asset!("templates/shared/roles.poml"),
    asset!("templates/discovery/skills.poml"),
    asset!("templates/shared/task_inputs.poml"),
    asset!("templates/tasks/plan.poml"),
    asset!("templates/tasks/code.poml"),
    asset!("templates/tasks/test.poml"),
    asset!("templates/tasks/review.poml"),
    asset!("templates/tasks/feedback.poml"),
    asset!("templates/tasks/done.poml"),
    asset!("templates/tasks/daily_quiz.poml"),
    asset!("templates/tasks/transcript_check.poml"),
    asset!("templates/states/standard/standard.poml"),
    asset!("templates/states/standard/standard.json"),
    asset!("templates/states/teach/teach.json"),
    asset!("templates/states/teach/teach.poml"),
    asset!("templates/states/code/code.json"),
    asset!("templates/states/code/code.poml"),
    asset!("templates/states/code_architect/code_architect.json"),
    asset!("templates/states/code_architect/code_architect.poml"),
    asset!("templates/states/senior_dev/senior_dev.json"),
    asset!("templates/states/senior_dev/senior_dev.poml"),
    asset!("templates/states/expert_programmer/expert_programmer.json"),
    asset!("templates/states/expert_programmer/expert_programmer.poml"),
    asset!("templates/states/debugger/debugger.json"),
    asset!("templates/states/debugger/debugger.poml"),
    asset!("templates/states/review/review.json"),
    asset!("templates/states/review/review.poml"),
    asset!("templates/states/research/research.json"),
    asset!("templates/states/research/research.poml"),
    asset!("skills/code_review/skill.json"),
    asset!("skills/code_review/skill.poml"),
    asset!("skills/debug/skill.json"),
    asset!("skills/debug/skill.poml"),
    asset!("skills/tmux/skill.json"),
    asset!("skills/tmux/skill.poml"),
    asset!("skills/tmux/scripts/create_session.sh", executable),
    asset!("skills/tmux/scripts/kill_session.sh", executable),
    asset!("skills/tmux/scripts/list_sessions.sh", executable),
    asset!("skills/poml_templates/skill.json"),
    asset!("skills/poml_templates/skill.poml"),
    asset!("skills/poml_templates/reference.md"),
    asset!("skills/poml_templates/examples/defaults.json"),
    asset!("skills/poml_templates/examples/footer.poml"),
    asset!("skills/poml_templates/examples/starter.poml"),
    asset!("skills/poml_templates/scripts/validate.py", executable),
    asset!("skills/skill_creator/skill.json"),
    asset!("skills/skill_creator/skill.poml"),
    asset!("skills/skill_creator/references/authoring.md"),
    asset!("static/index.html"),
    asset!("static/style.css"),
    asset!("static/app.js"),
    asset!("static/extensions.js"),
    asset!("static/workflow-dashboard.js"),
    asset!("static/chat-audio.js"),
    asset!("static/logo.svg"),
    asset!("static/logo.png"),
    asset!("static/favicon.ico"),
    asset!("static/favicon.png"),
    asset!("static/apple-touch-icon.png"),
    asset!("examples/poml-test-context.json"),
    asset!("examples/long-horizon.env"),
    asset!("examples/plugins/verified-rust/plugin.json"),
    asset!("docs/DECISION_IR.md"),
    asset!("docs/WORKFLOW_GRAPHS.md"),
    asset!("docs/SNAKE_IR_TEST.md"),
    asset!("docs/VERIFIED_CODING_ROLES.md"),
    asset!("docs/LANGUAGE_LEARNING_FLOW.md"),
    asset!("docs/PLUGIN_FIRST_PLAN.md"),
    asset!("docs/INSTALLATION_PRESETS.md"),
    asset!("docs/VM_PLUGIN.md"),
    asset!("docs/PLUGIN_PROCESS_PROTOCOL.md"),
    asset!("docs/PLUGIN_WEB_CONTRIBUTIONS.md"),
    asset!("docs/BRANDING.md"),
    asset!("docs/SKILLS.md"),
    asset!("docs/POML_WORKFLOWS.md"),
    asset!("docs/CONTEXT_VARIABLES.md"),
    asset!("docs/LLM_RESILIENCE.md"),
    asset!("docs/MEDIA_PLUGINS.md"),
    asset!("docs/COMFYUI.md"),
    asset!("docs/COMFYUI_QWEN3.md"),
    asset!("docs/VOSK_REMOTE.md"),
    asset!("docs/TOOL_DISCOVERY.md"),
    asset!("docs/TOOL_PACKAGES.md"),
    asset!("docs/TOOL_OUTPUTS.md"),
    asset!("docs/MEMORY_PROFILES.md"),
    asset!("plugins/vm/plugin.json"),
    asset!("plugins/brave_search/plugin.json"),
    asset!("plugins/brave_search/search.py", executable),
    asset!("plugins/brave_search/README.md"),
];

// First-party compatibility packages shipped with the full distribution.
const COMPATIBILITY_ASSETS: &[Asset] = &[
    asset!("plugins/comfyui/.gitignore"),
    asset!("plugins/comfyui/README.md"),
    asset!("plugins/comfyui/common.py", executable),
    asset!("plugins/comfyui/examples/img2img.api.json"),
    asset!("plugins/comfyui/examples/txt2img.api.json"),
    asset!("plugins/comfyui/nodes.py", executable),
    asset!("plugins/comfyui/plugin.json"),
    asset!("plugins/comfyui/result.py", executable),
    asset!("plugins/comfyui/run.py", executable),
    asset!("plugins/comfyui/workflow.py", executable),
    asset!("plugins/elevenlabs_tts/README.md"),
    asset!("plugins/elevenlabs_tts/generate.py", executable),
    asset!("plugins/elevenlabs_tts/media_common.py", executable),
    asset!("plugins/elevenlabs_tts/plugin.json"),
    asset!("plugins/mimo_understand/mimo_common.py", executable),
    asset!("plugins/mimo_understand/plugin.json"),
    asset!("plugins/mimo_understand/understand_audio.py", executable),
    asset!("plugins/mimo_understand/understand_image.py", executable),
    asset!("plugins/mimo_understand/understand_video.py", executable),
    asset!("plugins/openrouter_image/README.md"),
    asset!("plugins/openrouter_image/generate.py", executable),
    asset!("plugins/openrouter_image/media_common.py", executable),
    asset!("plugins/openrouter_image/plugin.json"),
    asset!("plugins/sosse/get_document.py", executable),
    asset!("plugins/sosse/plugin.json"),
    asset!("plugins/sosse/search.py", executable),
    asset!("plugins/spotify/get_album.py", executable),
    asset!("plugins/spotify/get_artist.py", executable),
    asset!("plugins/spotify/get_playlist.py", executable),
    asset!("plugins/spotify/get_recommendations.py", executable),
    asset!("plugins/spotify/get_track.py", executable),
    asset!("plugins/spotify/plugin.json"),
    asset!("plugins/spotify/search.py", executable),
    asset!("plugins/spotify/spotify_common.py", executable),
    asset!("plugins/system_info/plugin.json"),
    asset!("plugins/system_info/process_list.py", executable),
    asset!("plugins/system_info/system_info.py", executable),
];

#[derive(Debug, Default)]
pub struct InstallReport {
    pub created: Vec<&'static str>,
    pub preserved: Vec<&'static str>,
    pub updated: Vec<&'static str>,
    pub backup_dir: Option<PathBuf>,
}

/// Fill missing bundled assets. If `overwrite` is true, replaces ALL existing bundled assets.
/// If `update_dashboard` is true, also updates dashboard files (with backup).
/// Existing prompt/SM/skill/configuration files are replaced when `overwrite` is true.
/// Each file is atomic; this is not a cross-file transaction.
pub fn install(directory: &Path, update_dashboard: bool, overwrite: bool) -> anyhow::Result<InstallReport> {
    install_selected(directory, BUNDLED_ASSETS.iter().collect(), update_dashboard, overwrite)
}

/// Restore the source distribution's plugin set, without changing existing
/// manifests, operator configuration, credentials, flags or service state.
pub fn install_compatibility(directory: &Path, update_dashboard: bool) -> anyhow::Result<InstallReport> {
    let assets = BUNDLED_ASSETS.iter().chain(COMPATIBILITY_ASSETS);
    #[cfg(feature = "vm")]
    let assets = assets.chain(praxis_vm_web::ASSETS);
    install_selected(directory, assets.collect(), update_dashboard, false)
}

fn install_selected(directory: &Path, assets: Vec<&Asset>, update_dashboard: bool, overwrite: bool) -> anyhow::Result<InstallReport> {
    fs::create_dir_all(directory)
        .with_context(|| format!("Cannot create asset directory {}", directory.display()))?;
    let root = directory.canonicalize()?;
    // Preflight every destination before writing files. Do not follow asset
    // symlinks, including dangling links, into unrelated directories.
    for asset in &assets {
        check_destination(&root, Path::new(asset.path))?;
    }
    let mut report = InstallReport::default();
    for asset in &assets {
        let destination = root.join(asset.path);
        let existing = match fs::symlink_metadata(&destination) {
            Ok(meta) => {
                anyhow::ensure!(
                    meta.is_file() && !meta.file_type().is_symlink(),
                    "Asset destination is not a regular file: {}",
                    destination.display()
                );
                true
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => return Err(error.into()),
        };
        let update = existing && (update_dashboard && (asset.path.starts_with("static/") || asset.dashboard) || overwrite);
        if existing && !update {
            report.preserved.push(asset.path);
            continue;
        }
        if update {
            let previous = fs::read(&destination)?;
            if previous.as_slice() == asset.bytes {
                report.preserved.push(asset.path);
                continue;
            }
            if report.backup_dir.is_none() {
                let relative =
                    PathBuf::from(".praxis-asset-backups").join(uuid::Uuid::new_v4().to_string());
                check_destination(&root, &relative.join("placeholder"))?;
                fs::create_dir_all(root.join(&relative))?;
                report.backup_dir = Some(root.join(relative));
            }
            let backup = report
                .backup_dir
                .as_ref()
                .expect("backup directory initialized")
                .join(asset.path);
            fs::create_dir_all(backup.parent().context("Asset backup has no parent")?)?;
            write_atomic(&backup, &previous, asset.executable, false)?;
        }
        check_destination(&root, Path::new(asset.path))?;
        fs::create_dir_all(destination.parent().context("Asset has no parent")?)?;
        write_atomic(&destination, asset.bytes, asset.executable, update)
            .with_context(|| format!("Cannot install {}", destination.display()))?;
        if update {
            report.updated.push(asset.path);
        } else {
            report.created.push(asset.path);
        }
    }
    Ok(report)
}

fn check_destination(root: &Path, relative: &Path) -> anyhow::Result<()> {
    let mut path = root.to_path_buf();
    let components: Vec<_> = relative.components().collect();
    for (index, component) in components.iter().enumerate() {
        anyhow::ensure!(
            matches!(component, std::path::Component::Normal(_)),
            "Invalid bundled asset path"
        );
        path.push(component.as_os_str());
        match fs::symlink_metadata(&path) {
            Ok(meta) => {
                anyhow::ensure!(
                    !meta.file_type().is_symlink(),
                    "Refusing symlink asset destination: {}",
                    path.display()
                );
                let leaf = index + 1 == components.len();
                anyhow::ensure!(
                    if leaf { meta.is_file() } else { meta.is_dir() },
                    "Invalid asset destination: {}",
                    path.display()
                );
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn write_atomic(path: &Path, bytes: &[u8], executable: bool, replace: bool) -> anyhow::Result<()> {
    let mut temporary =
        tempfile::NamedTempFile::new_in(path.parent().context("Asset has no parent")?)?;
    temporary.write_all(bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temporary
            .as_file()
            .set_permissions(fs::Permissions::from_mode(if executable {
                0o755
            } else {
                0o644
            }))?;
    }
    #[cfg(not(unix))]
    let _ = executable;
    temporary.as_file().sync_all()?;
    if replace {
        temporary.persist(path)?;
    } else {
        temporary.persist_noclobber(path)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "assets_tests.rs"]
mod tests;
