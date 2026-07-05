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
