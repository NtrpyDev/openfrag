use openfrag_setup::{
    CheckId, CheckStatus, HostFacts, RecorderInstall, SessionKind, evaluate, install_gsi,
};
use std::{fs, os::unix::fs::PermissionsExt};

#[test]
fn demo_import_remains_ready_when_capture_is_unavailable() {
    let report = evaluate(&HostFacts {
        data_directory_writable: true,
        cs2_cfg_directory: Some("/steam/cs2/cfg".into()),
        recorder: None,
        ffmpeg_available: false,
        audio_source_available: false,
        session: SessionKind::Wayland,
        global_shortcuts_portal: false,
    });

    assert!(report.ready_for_demo_import());
    assert_eq!(report.check(CheckId::DemoImport).status, CheckStatus::Ready);
    assert_eq!(report.check(CheckId::Capture).status, CheckStatus::Blocked);
    assert_eq!(
        report.check(CheckId::ManualFlag).status,
        CheckStatus::Blocked
    );
    assert!(report.check(CheckId::Capture).action.contains("gpu-screen-recorder"));
}

#[test]
fn capture_requires_the_recorder_but_audio_is_an_actionable_warning() {
    let report = evaluate(&HostFacts {
        data_directory_writable: true,
        cs2_cfg_directory: Some("/steam/cs2/cfg".into()),
        recorder: Some(RecorderInstall::Native("/usr/bin/gpu-screen-recorder".into())),
        ffmpeg_available: true,
        audio_source_available: false,
        session: SessionKind::X11,
        global_shortcuts_portal: true,
    });

    assert_eq!(report.check(CheckId::Capture).status, CheckStatus::Warning);
    assert!(report.check(CheckId::Capture).action.contains("audio"));
    assert_eq!(report.check(CheckId::GsiConfig).status, CheckStatus::Ready);
    assert_eq!(report.check(CheckId::ManualFlag).status, CheckStatus::Ready);
}

#[test]
fn gsi_install_is_private_loopback_only_and_rerunnable() {
    let temp = tempfile::tempdir().unwrap();
    let cfg = temp.path().join("cfg");
    let data = temp.path().join("data");
    fs::create_dir_all(&cfg).unwrap();
    fs::create_dir_all(&data).unwrap();

    let first = install_gsi(&cfg, &data, "safe-token_123").unwrap();
    let config = fs::read_to_string(&first.config_path).unwrap();
    assert!(config.contains("http://127.0.0.1:7130/gsi/router"));
    assert!(config.contains("\"token\" \"safe-token_123\""));
    assert!(!config.contains("steamcommunity"));
    assert!(!config.contains("https://"));
    assert_eq!(
        fs::metadata(&first.config_path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(&first.token_path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );

    let second = install_gsi(&cfg, &data, "replacement-token").unwrap();
    assert_eq!(first, second);
    assert!(fs::read_to_string(second.config_path)
        .unwrap()
        .contains("replacement-token"));
    assert_eq!(
        fs::read_to_string(second.token_path).unwrap(),
        "replacement-token\n"
    );
}

#[test]
fn unsafe_tokens_and_symlink_targets_are_rejected_without_mutation() {
    let temp = tempfile::tempdir().unwrap();
    let cfg = temp.path().join("cfg");
    let data = temp.path().join("data");
    fs::create_dir_all(&cfg).unwrap();
    fs::create_dir_all(&data).unwrap();
    assert!(install_gsi(&cfg, &data, "bad\n\"token").is_err());
    assert!(!cfg.join("gamestate_integration_openfrag.cfg").exists());

    let outside = temp.path().join("outside");
    fs::write(&outside, "do not replace").unwrap();
    std::os::unix::fs::symlink(&outside, cfg.join("gamestate_integration_openfrag.cfg"))
        .unwrap();
    assert!(install_gsi(&cfg, &data, "safe-token").is_err());
    assert_eq!(fs::read_to_string(outside).unwrap(), "do not replace");
}
