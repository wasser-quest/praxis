use crate::db::Database;
use serde_json::Value;
use std::path::{Path, PathBuf};

fn destination(root: &Path, name: &str) -> anyhow::Result<PathBuf> {
    let parts: Vec<_> = name.split('/').collect();
    anyhow::ensure!(parts.iter().all(|s| !s.is_empty() && s.bytes().all(
        |b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
    )), "Template name must contain only letters, numbers, '_' or '-' separated by '/', without .poml");
    std::fs::create_dir_all(root)?;
    let mut parent = root.canonicalize()?;
    for part in &parts[..parts.len() - 1] {
        parent.push(part);
        if let Ok(meta) = std::fs::symlink_metadata(&parent) {
            anyhow::ensure!(
                !meta.file_type().is_symlink(),
                "Template directories must not be symlinks"
            );
        }
        std::fs::create_dir_all(&parent)?;
    }
    let path = parent.join(format!("{}.poml", parts[parts.len() - 1]));
    if let Ok(meta) = std::fs::symlink_metadata(&path) {
        anyhow::ensure!(
            !meta.file_type().is_symlink(),
            "Template destination must not be a symlink"
        );
    }
    Ok(path)
}

/// Render a temporary sibling so relative includes/data imports resolve exactly
/// as they will after saving. Only replace the destination after strict success.
pub async fn save_validated(
    root: &Path,
    name: &str,
    content: &str,
    context: &Value,
) -> anyhow::Result<String> {
    anyhow::ensure!(
        !content.trim().is_empty(),
        "Template content must not be empty"
    );
    anyhow::ensure!(
        context.is_object(),
        "Template validation context must be a JSON object"
    );
    let path = destination(root, name)?;
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Missing template parent"))?;
    let temporary = tempfile::Builder::new()
        .prefix(".validate-")
        .suffix(".poml")
        .tempfile_in(parent)?;
    std::fs::write(temporary.path(), content)?;
    let temp_path = temporary
        .path()
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Template path is not UTF-8"))?;
    let rendered =
        crate::runtime::engine::render_strict_candidate(temp_path, context, Some(&path)).await?;
    temporary.persist(&path).map_err(|e| e.error)?;
    Ok(rendered)
}

pub async fn run(db: &Database, args: &Value) -> anyhow::Result<String> {
    anyhow::ensure!(
        crate::db::tools::get(db, "update_template")?.is_enabled,
        "update_template is disabled"
    );
    let name = args
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("Template name is required"))?;
    let content = args
        .get("content")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("Template content is required"))?;
    // Synthetic context only: validation must not silently expose stored user data.
    let mut validation = crate::db::contexts::Context {
        user_id: "validation".into(), ..Default::default()
    };
    validation.settings.path = ".".into();
    let default_context = crate::gateway::prompt::base_context(&validation, "Validation test")?;
    let context = args.get("context").unwrap_or(&default_context);
    let rendered = save_validated(Path::new("templates"), name, content, context).await?;
    db.save_template(name, content, None, false).map_err(|e| {
        anyhow::anyhow!("Validated template saved to disk, but database update failed: {e}")
    })?;
    Ok(format!(
        "Template '{name}' updated and strictly validated. Preview: {}",
        crate::util::truncate_chars(&rendered, 500)
    ))
}

#[cfg(test)]
mod validated_template_tests {
    use super::*;

    #[test]
    fn rejects_unsafe_names() {
        let root = tempfile::tempdir().unwrap();
        for name in [
            "",
            "../outside",
            "/tmp/outside",
            "a/../../outside",
            "a//b",
            "a/",
            "a.poml",
            "C:\\outside",
            "a\\b",
        ] {
            assert!(destination(root.path(), name).is_err(), "{name}");
        }
        assert!(destination(root.path(), "tasks/my_template-1")
            .unwrap()
            .starts_with(root.path()));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_escape() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("linked")).unwrap();
        assert!(destination(root.path(), "linked/new/file").is_err());
        assert!(!outside.path().join("new").exists());
        std::os::unix::fs::symlink(outside.path().join("target"), root.path().join("file.poml"))
            .unwrap();
        assert!(destination(root.path(), "file").is_err());
    }

    #[tokio::test]
    #[ignore = "Requires Node and POML_CLI pointing to Microsoft's JavaScript CLI"]
    async fn real_poml_rejects_prospective_include_cycles() {
        assert!(std::env::var("POML_CLI").is_ok());
        let root = tempfile::tempdir().unwrap();
        let old = "<poml><p>Original prompt</p></poml>";
        let path = root.path().join("task.poml");
        std::fs::write(&path, old).unwrap();
        std::fs::write(
            root.path().join("indirect.poml"),
            "<poml><include src=\"task.poml\" /></poml>",
        )
        .unwrap();
        for target in ["task.poml", "indirect.poml"] {
            let cyclic = format!("<poml><include src=\"{target}\" /></poml>");
            assert!(
                save_validated(root.path(), "task", &cyclic, &serde_json::json!({}))
                    .await
                    .is_err()
            );
            assert_eq!(std::fs::read_to_string(&path).unwrap(), old);
        }
        // Literal self-source imports are not include cycles, but must show the
        // prospective source rather than accidentally validating the old file.
        let literal = "<poml><let name=\"source\" src=\"task.poml\" type=\"string\" /><p>{{source}}</p></poml>";
        let rendered = save_validated(root.path(), "task", literal, &serde_json::json!({}))
            .await
            .unwrap();
        assert!(rendered.contains("<let name="), "{rendered}");
        assert!(!rendered.contains("Original prompt"), "{rendered}");
    }

    #[tokio::test]
    #[ignore = "Requires Node and POML_CLI pointing to Microsoft's JavaScript CLI"]
    async fn real_poml_preserves_old_file_on_failure_and_resolves_relative_includes() {
        assert!(std::env::var("POML_CLI").is_ok());
        let root = tempfile::tempdir().unwrap();
        let old = "<poml><p>Original prompt</p></poml>";
        let path = root.path().join("task.poml");
        std::fs::write(&path, old).unwrap();
        let broken = "<poml><p>{{undefined_variable}}</p></poml>";
        assert!(
            save_validated(root.path(), "task", broken, &serde_json::json!({}))
                .await
                .is_err()
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), old);
        assert!(
            save_validated(root.path(), "new", broken, &serde_json::json!({}))
                .await
                .is_err()
        );
        assert!(!root.path().join("new.poml").exists());
        std::fs::write(
            root.path().join("snippet.poml"),
            "<poml><p>Hello {{name}}</p></poml>",
        )
        .unwrap();
        let valid = "<poml><include src=\"snippet.poml\" /></poml>";
        let rendered = save_validated(
            root.path(),
            "task",
            valid,
            &serde_json::json!({"name": "世界"}),
        )
        .await
        .unwrap();
        assert!(rendered.contains("Hello 世界"), "{rendered}");
        assert_eq!(std::fs::read_to_string(path).unwrap(), valid);
        assert!(!std::fs::read_dir(root.path()).unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".validate-")));
    }
}
