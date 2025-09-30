use assert_cmd::Command;
use assert_fs::prelude::*;
use predicates::prelude::*;

#[test]
fn directory_with_ext_filter_is_sorted_and_filtered() -> anyhow::Result<()> {
    let temp = assert_fs::TempDir::new()?;
    temp.child("dir/a.txt").write_str("A\n")?;
    temp.child("dir/b.md").write_str("B\n")?;
    temp.child("dir/sub/c.txt").write_str("C\n")?;

    let mut cmd = Command::cargo_bin("printfiles")?;
    cmd.current_dir(temp.path()).args(["dir", "--ext", "txt"]);

    let expected = "===dir/a.txt===\nA\n===end of 'dir/a.txt'===\n===dir/sub/c.txt===\nC\n===end of 'dir/sub/c.txt'===\n";

    cmd.assert()
        .success()
        .stdout(expected)
        .stderr(predicate::str::is_empty());

    temp.close()?;
    Ok(())
}

#[test]
fn glob_patterns_collect_multiple_files() -> anyhow::Result<()> {
    let temp = assert_fs::TempDir::new()?;
    temp.child("src/lib.rs").write_str("lib\n")?;
    temp.child("src/bin/main.rs").write_str("main\n")?;
    temp.child("docs/readme.md").write_str("doc\n")?;

    let mut cmd = Command::cargo_bin("printfiles")?;
    cmd.current_dir(temp.path())
        .args(["src/**/*.rs", "docs/*.md"]);

    let stdout = cmd.assert().success().get_output().stdout.clone();
    let text = String::from_utf8(stdout)?;

    let expected = "===docs/readme.md===\ndoc\n===end of 'docs/readme.md'===\n===src/bin/main.rs===\nmain\n===end of 'src/bin/main.rs'===\n===src/lib.rs===\nlib\n===end of 'src/lib.rs'===\n";
    assert_eq!(text, expected);

    Ok(())
}

#[test]
fn exit_code_is_two_when_no_matches() -> anyhow::Result<()> {
    let temp = assert_fs::TempDir::new()?;

    let mut cmd = Command::cargo_bin("printfiles")?;
    cmd.current_dir(temp.path()).arg("missing/**/*.rs");

    cmd.assert()
        .code(2)
        .stderr(predicate::str::contains("未匹配到任何文件"));

    Ok(())
}

#[test]
fn max_size_skips_large_files() -> anyhow::Result<()> {
    let temp = assert_fs::TempDir::new()?;
    let content = "x".repeat(32);
    temp.child("files/big.txt").write_str(&content)?;

    let mut cmd = Command::cargo_bin("printfiles")?;
    cmd.current_dir(temp.path())
        .args(["files", "--max-size", "10"]);

    let output = cmd.assert().success().get_output().clone();

    let stdout = String::from_utf8(output.stdout.clone())?;
    assert_eq!(
        stdout,
        "===files/big.txt===\n(skipped: file exceeds max size)\n===end of 'files/big.txt'===\n"
    );

    let stderr = String::from_utf8(output.stderr)?;
    assert!(stderr.contains("提示: 跳过"));
    assert!(stderr.contains("max_size=10"));

    Ok(())
}

#[test]
fn binary_skip_writes_placeholder() -> anyhow::Result<()> {
    let temp = assert_fs::TempDir::new()?;
    let bytes = [0u8, 1, 2, 3];
    temp.child("bin/image.bin").write_binary(&bytes)?;

    let mut cmd = Command::cargo_bin("printfiles")?;
    cmd.current_dir(temp.path()).arg("bin");

    let output = cmd.assert().success().get_output().clone();
    let stdout = String::from_utf8(output.stdout.clone())?;
    assert_eq!(
        stdout,
        "===bin/image.bin===\n(skipped binary file)\n===end of 'bin/image.bin'===\n"
    );
    let stderr = String::from_utf8(output.stderr)?;
    assert!(stderr.contains("二进制文件按 Skip 处理"));

    Ok(())
}

#[test]
fn binary_hex_outputs_encoded_bytes() -> anyhow::Result<()> {
    let temp = assert_fs::TempDir::new()?;
    let bytes = [0u8, 1, 2, 255];
    temp.child("bin/data.bin").write_binary(&bytes)?;

    let mut cmd = Command::cargo_bin("printfiles")?;
    cmd.current_dir(temp.path())
        .args(["bin/data.bin", "--binary", "hex"]);

    let output = cmd.assert().success().get_output().clone();
    let stdout = String::from_utf8(output.stdout.clone())?;
    assert!(stdout.contains("000102ff"));
    let stderr = String::from_utf8(output.stderr)?;
    assert!(stderr.contains("二进制文件按 Hex 处理"));

    Ok(())
}

#[test]
fn sort_by_size_changes_order() -> anyhow::Result<()> {
    let temp = assert_fs::TempDir::new()?;
    temp.child("logs/a.log").write_str("short\n")?;
    temp.child("logs/b.log").write_str("a bit longer\n")?;
    temp.child("logs/c.log")
        .write_str("longest line of them all\n")?;

    let mut cmd = Command::cargo_bin("printfiles")?;
    cmd.current_dir(temp.path())
        .args(["logs/*.log", "--sort", "size"]);

    let stdout = cmd.assert().success().get_output().stdout.clone();
    let text = String::from_utf8(stdout)?;

    let expected_order = ["logs/a.log", "logs/b.log", "logs/c.log"];
    let mut found = Vec::new();
    for line in text.lines() {
        if let Some(stripped) = line.strip_prefix("===") {
            if let Some(name) = stripped.strip_suffix("===") {
                if name.ends_with(".log") {
                    found.push(name.to_string());
                }
            }
        }
    }
    assert_eq!(found, expected_order);

    Ok(())
}

#[test]
fn quiet_suppresses_warnings() -> anyhow::Result<()> {
    let temp = assert_fs::TempDir::new()?;
    temp.child("files/big.log").write_str(&"x".repeat(32))?;

    let mut cmd = Command::cargo_bin("printfiles")?;
    cmd.current_dir(temp.path())
        .args(["files", "--max-size", "10", "--quiet"]);

    let output = cmd.assert().success().get_output().clone();
    assert!(output.stderr.is_empty());

    let stdout = String::from_utf8(output.stdout)?;
    assert!(stdout.contains("(skipped: file exceeds max size)"));

    Ok(())
}

#[test]
fn relative_from_rebases_output() -> anyhow::Result<()> {
    let temp = assert_fs::TempDir::new()?;
    temp.child("workspace/src/lib.rs").write_str("lib\n")?;
    temp.child("workspace/docs/readme.md").write_str("doc\n")?;

    let workspace = temp.child("workspace");

    let mut cmd = Command::cargo_bin("printfiles")?;
    cmd.current_dir(workspace.path())
        .args(["src/**/*.rs", "docs/*.md", "--relative-from", "src"]);

    let stdout = cmd.assert().success().get_output().stdout.clone();
    let text = String::from_utf8(stdout)?;

    let expected = "===docs/readme.md===\ndoc\n===end of 'docs/readme.md'===\n===lib.rs===\nlib\n===end of 'lib.rs'===\n";
    assert_eq!(text, expected);

    Ok(())
}
