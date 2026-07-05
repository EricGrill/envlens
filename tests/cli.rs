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
fn check_json_is_valid_and_stable() {
    let output = cmd()
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env("SOURCE_DATE_EPOCH", "0")
        .args(["check", "--json", "tests/fixtures/basic"])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).expect("valid json");

    assert_eq!(json["version"], 1);
    assert_eq!(json["generated_at"], "1970-01-01T00:00:00Z");
    assert_eq!(json["root"], "tests/fixtures/basic");
    assert_eq!(json["profile"], "default");
    assert!(json["summary"]["errors"].as_u64().unwrap_or(0) > 0);

    let process_keys: Vec<_> = json["variables"]
        .as_array()
        .expect("variables array")
        .iter()
        .filter(|variable| {
            variable["occurrences"]
                .as_array()
                .expect("occurrences array")
                .iter()
                .any(|occurrence| occurrence["source_id"] == "process")
        })
        .map(|variable| variable["key"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(process_keys, vec!["SOURCE_DATE_EPOCH"]);

    let snapshot = serde_json::to_string_pretty(&json).expect("json formats");
    insta::assert_snapshot!("check_json_is_valid_and_stable", snapshot);
}

#[test]
fn check_exit_1_on_errors() {
    cmd()
        .args(["check", "tests/fixtures/basic"])
        .assert()
        .code(1);

    cmd()
        .args(["check", "tests/fixtures/compose"])
        .assert()
        .success();
}

#[test]
fn determinism_byte_identical() {
    let first = cmd()
        .env("SOURCE_DATE_EPOCH", "0")
        .args(["check", "--json", "tests/fixtures/basic"])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let second = cmd()
        .env("SOURCE_DATE_EPOCH", "0")
        .args(["check", "--json", "tests/fixtures/basic"])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();

    assert_eq!(first, second);
}

#[test]
fn no_values_truly_value_free() {
    let output = cmd()
        .env("SOURCE_DATE_EPOCH", "0")
        .args(["check", "--json", "--no-values", "tests/fixtures/basic"])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8_lossy(&output);
    for raw in ["5001", "secret123", "sk_live_"] {
        assert!(!stdout.contains(raw), "output leaked {raw}: {stdout}");
    }

    let json: Value = serde_json::from_slice(&output).expect("valid json");
    assert_no_value_keys(&json);
}

#[test]
fn planted_secret_never_unmasked() {
    let json_output = cmd()
        .env("SOURCE_DATE_EPOCH", "0")
        .args(["check", "--json", "tests/fixtures/basic"])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let json_stdout = String::from_utf8_lossy(&json_output);

    let human_output = cmd()
        .args(["check", "tests/fixtures/basic"])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let human_stdout = String::from_utf8_lossy(&human_output);

    for raw in ["envlensFakeHistoricalSecret", "secret123"] {
        assert!(
            !json_stdout.contains(raw),
            "json leaked raw secret {raw}: {json_stdout}"
        );
        assert!(
            !human_stdout.contains(raw),
            "human output leaked raw secret {raw}: {human_stdout}"
        );
    }
}

#[test]
fn empty_dir_exit_0() {
    let output = cmd()
        .env("SOURCE_DATE_EPOCH", "0")
        .args(["check", "--json", "tests/fixtures/empty"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: Value = serde_json::from_slice(&output).expect("valid json");
    let sources: Vec<_> = json["sources"]
        .as_array()
        .expect("sources array")
        .iter()
        .map(|source| source["id"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(sources, vec!["process"]);
}

#[test]
fn check_human_output_readable() {
    let output = cmd()
        .args(["check", "--no-color", "tests/fixtures/basic"])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8_lossy(&output);
    insta::assert_snapshot!("check_human_output_readable", stdout);
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
fn unknown_profile_source_and_panic_do_not_leak_secret_like_input() {
    let raw = "envlensFakeHistoricalSecret";

    for args in [
        vec!["check", "--profile", raw, "tests/fixtures/empty"],
        vec!["check", "--source", raw, "tests/fixtures/empty"],
    ] {
        let output = cmd()
            .args(args)
            .assert()
            .code(2)
            .get_output()
            .stderr
            .clone();
        let stderr = String::from_utf8_lossy(&output);
        assert!(!stderr.contains(raw), "stderr leaked raw secret: {stderr}");
    }

    let output = cmd()
        .env("ENVLENS_TEST_PANIC", raw)
        .assert()
        .code(4)
        .get_output()
        .stderr
        .clone();
    let stderr = String::from_utf8_lossy(&output);
    assert!(stderr.contains("internal error:"));
    assert!(
        !stderr.contains(raw),
        "panic stderr leaked raw secret: {stderr}"
    );
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

fn assert_no_value_keys(value: &Value) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                assert_ne!(key, "value");
                assert_ne!(key, "effective");
                assert_ne!(key, "raw");
                assert_no_value_keys(child);
            }
        }
        Value::Array(items) => {
            for item in items {
                assert_no_value_keys(item);
            }
        }
        _ => {}
    }
}
