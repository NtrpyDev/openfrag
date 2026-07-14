use std::process::Command;

#[test]
fn version_command_identifies_the_v1_daemon() {
    let output = Command::new(env!("CARGO_BIN_EXE_openfragd"))
        .arg("--version")
        .output()
        .expect("run openfragd");

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).expect("UTF-8 version output"),
        "openfragd 1.0.0\n"
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn setup_gsi_writes_a_private_local_configuration_without_printing_the_token() {
    let temp = tempfile::tempdir().expect("temporary setup root");
    let cfg = temp.path().join("cfg");
    let data = temp.path().join("data");
    std::fs::create_dir_all(&cfg).expect("cfg directory");
    std::fs::create_dir_all(&data).expect("data directory");

    let output = Command::new(env!("CARGO_BIN_EXE_openfragd"))
        .args(["setup-gsi", "--cs2-cfg-dir"])
        .arg(&cfg)
        .arg("--data-dir")
        .arg(&data)
        .output()
        .expect("run GSI setup");

    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let stdout = String::from_utf8(output.stdout).expect("UTF-8 setup output");
    let config = std::fs::read_to_string(cfg.join("gamestate_integration_openfrag.cfg"))
        .expect("GSI config");
    let token = std::fs::read_to_string(data.join("gsi-token")).expect("GSI token");
    assert!(config.contains(token.trim()));
    assert!(!stdout.contains(token.trim()));
    assert!(stdout.contains("Game State Integration configured"));
}

#[test]
fn doctor_reports_json_without_requiring_capture_for_demo_import() {
    let temp = tempfile::tempdir().expect("temporary setup root");
    let cfg = temp.path().join("cfg");
    let data = temp.path().join("data");
    std::fs::create_dir_all(&cfg).expect("cfg directory");
    std::fs::create_dir_all(&data).expect("data directory");

    let output = Command::new(env!("CARGO_BIN_EXE_openfragd"))
        .args(["doctor", "--json", "--cs2-cfg-dir"])
        .arg(&cfg)
        .arg("--data-dir")
        .arg(&data)
        .env("PATH", "")
        .output()
        .expect("run Compatibility Doctor");

    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).expect("Doctor JSON");
    let checks = report["checks"].as_array().expect("checks array");
    assert!(checks.iter().any(|check| {
        check["id"] == "demo_import" && check["status"] == "ready"
    }));
    assert!(checks.iter().any(|check| {
        check["id"] == "capture" && check["status"] == "blocked"
    }));
}
