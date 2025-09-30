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
