//! Trusted workflow setup checks, before any model or effect is attempted.
use anyhow::{ensure, Context as _};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, fs, path::Path};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Requirements {
    #[serde(default)]
    pub required_files: Vec<String>,
    #[serde(default)]
    pub required_directories: Vec<String>,
}

impl Requirements {
    pub fn is_empty(&self) -> bool {
        self.required_files.is_empty() && self.required_directories.is_empty()
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        ensure!(
            self.required_files.len() + self.required_directories.len() <= 64,
            "Workspace preflight accepts at most 64 required paths"
        );
        let mut seen = HashSet::new();
        for value in self.required_files.iter().chain(&self.required_directories) {
            ensure!(
                !value.is_empty()
                    && value.len() <= 4096
                    && !value.contains(['\\', '\0'])
                    && !Path::new(value).is_absolute()
                    && value.split('/').all(|part| !part.is_empty()
                        && part != "."
                        && part != ".."
                        && !part.contains(':'))
                    && seen.insert(value),
                "Workspace requirements must be unique normalized relative paths: {value:?}"
            );
        }
        Ok(())
    }

    pub fn check(&self, root: &Path) -> anyhow::Result<()> {
        // A workflow that declares nothing needs no prepared project: a bare
        // `praxis` run can just chat instead of failing a setup check.
        if self.is_empty() {
            return Ok(());
        }
        self.validate()?;
        let mut failures = Vec::new();
        for (paths, directory) in [
            (&self.required_files, false),
            (&self.required_directories, true),
        ] {
            for value in paths {
                let mut current = root.to_path_buf();
                let mut metadata = None;
                let mut problem = None;
                for part in value.split('/') {
                    current.push(part);
                    match fs::symlink_metadata(&current) {
                        Ok(found) if found.file_type().is_symlink() => {
                            problem = Some("contains a symlink");
                            break;
                        }
                        Ok(found) => metadata = Some(found),
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                            problem = Some("is missing");
                            break;
                        }
                        Err(error) => {
                            return Err(error).with_context(|| {
                                format!(
                                    "Cannot inspect workspace requirement {value:?} at {}",
                                    root.display()
                                )
                            })
                        }
                    }
                }
                if problem.is_none() {
                    if !metadata.is_some_and(|m| if directory { m.is_dir() } else { m.is_file() }) {
                        problem = Some(if directory {
                            "must be a directory"
                        } else {
                            "must be a regular file"
                        });
                    }
                }
                if let Some(problem) = problem {
                    failures.push(format!("{value:?} {problem}"));
                }
            }
        }
        ensure!(failures.is_empty(),
            "Workspace preflight failed at '{}': {}. Point WORKSPACE_DIR (or praxis run --workspace-dir) at the prepared project and start a new task; nothing was changed.",
            root.display(), failures.join("; "));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_workflow_that_declares_nothing_never_touches_the_filesystem() {
        // The confusing case was a bare run failing a check it never asked for.
        let nowhere = std::path::Path::new("/definitely-not-a-project-9f2c1");
        assert!(Requirements::default().check(nowhere).is_ok());
    }

    #[test]
    fn a_failure_names_the_missing_paths_and_the_fix_in_one_line() {
        let root = tempfile::tempdir().unwrap();
        let error = Requirements { required_files: vec!["Cargo.toml".into()], ..Default::default() }
            .check(root.path()).unwrap_err().to_string();
        assert!(error.contains("\"Cargo.toml\" is missing"), "{error}");
        assert!(error.contains("WORKSPACE_DIR"), "{error}");
        assert_eq!(error.matches('\n').count(), 0, "the preflight message stays one line: {error}");
    }

    #[test]
    fn required_paths_check_types_without_creating_or_searching_for_files() {
        let root = tempfile::tempdir().unwrap();
        let requirements = Requirements {
            required_files: vec!["Cargo.toml".into()],
            required_directories: vec!["src".into()],
        };
        assert!(requirements.check(root.path()).is_err());
        assert!(!root.path().join("src").exists());
        fs::create_dir(root.path().join("Cargo.toml")).unwrap();
        fs::write(root.path().join("src"), "file, not directory").unwrap();
        let error = requirements.check(root.path()).unwrap_err().to_string();
        assert!(
            error.contains("regular file") && error.contains("directory"),
            "{error}"
        );
        fs::remove_dir(root.path().join("Cargo.toml")).unwrap();
        fs::remove_file(root.path().join("src")).unwrap();
        fs::write(root.path().join("Cargo.toml"), "[package]").unwrap();
        fs::create_dir(root.path().join("src")).unwrap();
        requirements.check(root.path()).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn requirements_never_follow_symlinks_even_into_a_prepared_project() {
        let root = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        fs::write(other.path().join("Cargo.toml"), "[package]").unwrap();
        std::os::unix::fs::symlink(other.path(), root.path().join("project")).unwrap();
        let requirements = Requirements {
            required_files: vec!["project/Cargo.toml".into()],
            ..Default::default()
        };
        assert!(requirements
            .check(root.path())
            .unwrap_err()
            .to_string()
            .contains("symlink"));
    }
}
