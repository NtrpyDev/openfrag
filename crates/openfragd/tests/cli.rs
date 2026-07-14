use std::process::Command;

#[cfg(unix)]
fn fixture_executable(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, "not launched by setup").unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
}

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
        .args(["--steam-id", "76561198000000001"])
        .output()
        .expect("run GSI setup");

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("UTF-8 setup output");
    let config = std::fs::read_to_string(cfg.join("gamestate_integration_openfrag.cfg"))
        .expect("GSI config");
    let token = std::fs::read_to_string(data.join("gsi-token")).expect("GSI token");
    let steam_id = std::fs::read_to_string(data.join("local-steam-id")).expect("local Steam ID");
    assert!(config.contains(token.trim()));
    assert!(!stdout.contains(token.trim()));
    assert_eq!(steam_id, "76561198000000001\n");
    assert!(stdout.contains("Game State Integration configured"));
}

#[cfg(unix)]
#[test]
fn setup_capture_persists_an_enabled_native_replay_configuration_without_launching_it() {
    let temp = tempfile::tempdir().expect("temporary capture setup root");
    let data = temp.path().join("data");
    let clips = temp.path().join("clips");
    let recorder = temp.path().join("gpu-screen-recorder");
    let ffprobe = temp.path().join("ffprobe");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::create_dir_all(&clips).unwrap();
    fixture_executable(&recorder);
    fixture_executable(&ffprobe);

    let output = Command::new(env!("CARGO_BIN_EXE_openfragd"))
        .args(["setup-capture", "--data-dir"])
        .arg(&data)
        .args(["--recorder", "native", "--recorder-path"])
        .arg(&recorder)
        .args(["--capture-target", "DP-1", "--output-dir"])
        .arg(&clips)
        .arg("--ffprobe-path")
        .arg(&ffprobe)
        .args(["--enabled", "true"])
        .output()
        .expect("run capture setup");

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let configuration = openfrag_setup::read_capture_configuration(&data).unwrap();
    assert!(configuration.enabled());
    assert_eq!(
        configuration.recorder(),
        &openfrag_setup::CaptureRecorder::Native(recorder)
    );
    assert_eq!(configuration.capture_target(), "DP-1");
    assert_eq!(configuration.output_directory(), clips);
    assert_eq!(configuration.ffprobe_path(), ffprobe);
    assert_eq!(configuration.replay_seconds(), 60);
}

#[cfg(unix)]
#[test]
fn setup_capture_persists_an_explicitly_disabled_flatpak_configuration() {
    let temp = tempfile::tempdir().expect("temporary Flatpak capture setup root");
    let data = temp.path().join("data");
    let clips = temp.path().join("clips");
    let ffprobe = temp.path().join("ffprobe");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::create_dir_all(&clips).unwrap();
    fixture_executable(&ffprobe);

    let output = Command::new(env!("CARGO_BIN_EXE_openfragd"))
        .args(["setup-capture", "--data-dir"])
        .arg(&data)
        .args(["--recorder", "flatpak", "--capture-target", "screen"])
        .arg("--output-dir")
        .arg(&clips)
        .arg("--ffprobe-path")
        .arg(&ffprobe)
        .args(["--enabled", "false"])
        .output()
        .expect("run Flatpak capture setup");

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let configuration = openfrag_setup::read_capture_configuration(&data).unwrap();
    assert!(!configuration.enabled());
    assert_eq!(
        configuration.recorder(),
        &openfrag_setup::CaptureRecorder::Flatpak
    );
    assert_eq!(configuration.capture_target(), "screen");
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

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).expect("Doctor JSON");
    let checks = report["checks"].as_array().expect("checks array");
    assert!(
        checks
            .iter()
            .any(|check| { check["id"] == "demo_import" && check["status"] == "ready" })
    );
    assert!(
        checks
            .iter()
            .any(|check| { check["id"] == "capture" && check["status"] == "blocked" })
    );
    assert_eq!(report["diagnostics"]["ffprobe"]["status"], "missing");
}

#[cfg(unix)]
#[test]
fn doctor_reports_ffprobe_from_isolated_path_without_desktop_activation() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().expect("temporary Doctor root");
    let bin = temp.path().join("bin");
    let cfg = temp.path().join("cfg");
    let data = temp.path().join("data");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::create_dir_all(&data).unwrap();
    let ffprobe = bin.join("ffprobe");
    std::fs::write(&ffprobe, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&ffprobe, std::fs::Permissions::from_mode(0o700)).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_openfragd"))
        .args(["doctor", "--json", "--cs2-cfg-dir"])
        .arg(&cfg)
        .arg("--data-dir")
        .arg(&data)
        .env("PATH", &bin)
        .env("HOME", temp.path())
        .env("XDG_DATA_HOME", &data)
        .output()
        .expect("run isolated Compatibility Doctor");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["diagnostics"]["ffprobe"]["status"], "available");
}
