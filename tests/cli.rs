use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;

fn cmd() -> Command {
    let mut command = match Command::cargo_bin("envlens") {
        Ok(command) => command,
        Err(err) => panic!("binary exists: {err}"),
    };
    command.env_clear();
    command
}

#[test]
fn version_flag() {
    cmd()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains("envlens 0.1.0"));
}

#[test]
fn unknown_flag_exits_2() {
    cmd().arg("--bogus").assert().code(2);
}

#[test]
fn unknown_profile_exits_2() {
    cmd()
        .args(["--profile", "missing", "check", "tests/fixtures/basic"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("unknown profile"));
}

#[test]
fn source_not_in_project_exits_2() {
    cmd()
        .args(["check", "--source", "not-a-source", "tests/fixtures/basic"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("unknown source"));
}

#[test]
fn missing_dir_exits_3() {
    cmd()
        .args(["check", "/definitely/not/envlens"])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("root is unreadable"));
}

#[test]
fn source_filter_reaches_output() {
    let output = cmd()
        .args([
            "check",
            "--json",
            "--source",
            ".env",
            "tests/fixtures/basic",
        ])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).expect("valid json");

    let env_local = json["sources"]
        .as_array()
        .expect("sources array")
        .iter()
        .find(|source| source["id"] == ".env.local")
        .expect(".env.local source");
    assert_eq!(env_local["enabled"], false);

    let port = json["variables"]
        .as_array()
        .expect("variables array")
        .iter()
        .find(|var| var["key"] == "PORT")
        .expect("PORT variable");
    assert_eq!(port["effective"]["source_id"], ".env");
    assert_eq!(port["effective"]["value"], "3000");
}

#[test]
fn malformed_secret_never_leaks_json_or_human() {
    let raw = "envlensFakeHistoricalSecret";

    for args in [
        vec!["check", "--json", "tests/fixtures/malformed-secret"],
        vec![
            "check",
            "--json",
            "--no-values",
            "tests/fixtures/malformed-secret",
        ],
    ] {
        let output = cmd()
            .args(args)
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let stdout = String::from_utf8_lossy(&output);
        assert!(!stdout.contains(raw), "json leaked raw secret: {stdout}");
    }

    let output = cmd()
        .args(["check", "tests/fixtures/malformed-secret"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8_lossy(&output);
    assert!(
        !stdout.contains(raw),
        "human output leaked raw secret: {stdout}"
    );
}

#[test]
fn subcommand_path_wins_over_parent_path() {
    cmd()
        .args(["/definitely/not/envlens", "check", "tests/fixtures/empty"])
        .assert()
        .success();
}

#[test]
fn check_strict_threshold_fails_on_warnings() {
    cmd()
        .args(["check", "tests/fixtures/malformed-secret"])
        .assert()
        .success();

    cmd()
        .args(["check", "--strict", "tests/fixtures/malformed-secret"])
        .assert()
        .code(1);
}

#[test]
fn bare_tui_and_report_stubs_exit_4() {
    cmd()
        .arg("tests/fixtures/empty")
        .assert()
        .code(4)
        .stderr(predicate::str::contains("TUI not yet implemented"));

    cmd()
        .args(["report", "--format", "json", "tests/fixtures/empty"])
        .assert()
        .code(4)
        .stderr(predicate::str::contains("report not yet implemented"));
}

#[test]
fn bare_tui_validates_profile_source_and_root_before_stub() {
    cmd()
        .args(["--profile", "missing", "tests/fixtures/empty"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("unknown profile"));

    cmd()
        .args(["--source", "not-a-source", "tests/fixtures/empty"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("unknown source"));

    cmd()
        .arg("/definitely/not/envlens")
        .assert()
        .code(3)
        .stderr(predicate::str::contains("root is unreadable"));
}

#[test]
fn panic_hook_exits_4() {
    let output = cmd()
        .env("ENVLENS_TEST_PANIC", "1")
        .assert()
        .code(4)
        .get_output()
        .stderr
        .clone();
    let stderr = String::from_utf8_lossy(&output);
    assert!(stderr.contains("internal error:"));
    assert!(stderr.contains("forced envlens test panic"));
}
