use praxis_legacy_file_ops::{edit_file, read_file};
use serde_json::json;
use std::io::Write;
use std::process::{Command, Stdio};

#[tokio::test]
async fn read_preserves_full_text_and_refuses_oversize_or_nontext_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("source");
    let text = format!("\n {}尾部\n", "code\n".repeat(12_000));
    std::fs::write(&path, &text).unwrap();
    assert_eq!(read_file::run(path.to_str().unwrap()).await.unwrap(), text);
    assert!(read_file::run(dir.path().to_str().unwrap()).await.is_err());
    std::fs::write(&path, [0xff, 0xfe]).unwrap();
    assert!(read_file::run(path.to_str().unwrap()).await.is_err());
    std::fs::File::create(&path)
        .unwrap()
        .set_len(8 * 1024 * 1024 + 1)
        .unwrap();
    assert!(read_file::run(path.to_str().unwrap()).await.is_err());
}

#[tokio::test]
async fn edit_replaces_first_match_and_missing_match_preserves_original() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("source");
    std::fs::write(&path, "hello hello\n").unwrap();
    edit_file::edit_file(path.to_str().unwrap(), "hello", "世界")
        .await
        .unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "世界 hello\n");
    assert!(
        edit_file::edit_file(path.to_str().unwrap(), "absent", "changed")
            .await
            .is_err()
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "世界 hello\n");
}

fn call(
    dir: &std::path::Path,
    tool: &str,
    arguments: serde_json::Value,
    version: u32,
) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_praxis-legacy-file-ops"))
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let request = json!({"protocol_version":version,"tool":tool,"arguments":arguments,"context":{},"secrets":{}});
    child
        .stdin
        .take()
        .unwrap()
        .write_all(request.to_string().as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn installed_binary_keeps_relative_paths_output_whitespace_and_legacy_aliases() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("source"), "\n hello hello \n").unwrap();
    let output = call(dir.path(), "read_file", json!({"path":"source"}), 1);
    assert!(output.status.success(), "{:?}", output);
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        result,
        json!({"protocol_version":1,"result":"\n hello hello \n"})
    );
    for (old, new) in [("old_text", "new_text"), ("old_string", "new_string")] {
        let output = call(
            dir.path(),
            "edit_file",
            json!({"path":"source",old:"hello",new:"rust"}),
            1,
        );
        assert!(output.status.success());
        let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(result["result"], "File edited: source");
    }
    assert_eq!(
        std::fs::read_to_string(dir.path().join("source")).unwrap(),
        "\n rust rust \n"
    );
    let output = call(dir.path(), "read_file", json!({"path":"missing"}), 1);
    assert!(output.status.success());
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(result["result"]
        .as_str()
        .unwrap()
        .starts_with("Error reading file:"));
}

#[test]
fn invalid_protocol_and_unknown_operation_do_not_modify_source() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("source");
    std::fs::write(&path, "hello").unwrap();
    for (tool, version) in [("edit_file", 2), ("write_file", 1)] {
        let output = call(
            dir.path(),
            tool,
            json!({"path":"source","old_string":"hello","new_string":"changed"}),
            version,
        );
        assert!(!output.status.success());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello");
    }
}
