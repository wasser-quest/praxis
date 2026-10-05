pub async fn edit_file(path: &str, old_text: &str, new_text: &str) -> anyhow::Result<()> {
    let content = std::fs::read_to_string(path)?;
    if !content.contains(old_text) {
        anyhow::bail!("Text not found in file");
    }
    let new_content = content.replacen(old_text, new_text, 1);
    std::fs::write(path, new_content)?;
    Ok(())
}

#[cfg(test)]
mod tool_tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[tokio::test]
    async fn test_edit_file() {
        let tmp = NamedTempFile::new().unwrap();
        let path = tmp.path().to_str().unwrap();
        std::fs::write(path, "hello world").unwrap();
        edit_file(path, "world", "rust").await.unwrap();
        let content = std::fs::read_to_string(path).unwrap();
        assert_eq!(content, "hello rust");
    }

    #[tokio::test]
    async fn test_edit_file_not_found() {
        let tmp = NamedTempFile::new().unwrap();
        let path = tmp.path().to_str().unwrap();
        std::fs::write(path, "hello").unwrap();
        assert!(edit_file(path, "missing", "new").await.is_err());
    }
}
