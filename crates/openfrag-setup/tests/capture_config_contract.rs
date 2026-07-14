use openfrag_setup::{
    CaptureConfiguration, CaptureRecorder, read_capture_configuration, write_capture_configuration,
};
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    path::Path,
};

fn executable(path: &Path) {
    fs::write(path, "fixture executable").unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn native_capture_configuration_is_private_fixed_and_idempotent() {
    let temporary = tempfile::tempdir().unwrap();
    let data = temporary.path().join("data");
    let clips = temporary.path().join("clips");
    let recorder = temporary.path().join("gpu-screen-recorder");
    let ffprobe = temporary.path().join("ffprobe");
    fs::create_dir_all(&data).unwrap();
    fs::create_dir_all(&clips).unwrap();
    executable(&recorder);
    executable(&ffprobe);

    let configuration = CaptureConfiguration::new(
        true,
        CaptureRecorder::Native(recorder.clone()),
        "DP-1",
        clips.clone(),
        ffprobe.clone(),
    )
    .unwrap();
    assert_eq!(configuration.replay_seconds(), 60);

    let first = write_capture_configuration(&data, &configuration).unwrap();
    let first_bytes = fs::read(&first.path).unwrap();
    let second = write_capture_configuration(&data, &configuration).unwrap();

    assert_eq!(first, second);
    assert_eq!(fs::read(&second.path).unwrap(), first_bytes);
    assert_eq!(
        fs::metadata(&second.path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(read_capture_configuration(&data).unwrap(), configuration);
}

#[test]
fn flatpak_capture_configuration_round_trips_an_explicit_disabled_state() {
    let temporary = tempfile::tempdir().unwrap();
    let data = temporary.path().join("data");
    let clips = temporary.path().join("clips");
    let ffprobe = temporary.path().join("ffprobe");
    fs::create_dir_all(&data).unwrap();
    fs::create_dir_all(&clips).unwrap();
    executable(&ffprobe);

    let configuration =
        CaptureConfiguration::new(false, CaptureRecorder::Flatpak, "screen", clips, ffprobe)
            .unwrap();
    write_capture_configuration(&data, &configuration).unwrap();

    let loaded = read_capture_configuration(&data).unwrap();
    assert!(!loaded.enabled());
    assert_eq!(loaded.recorder(), &CaptureRecorder::Flatpak);
    assert_eq!(loaded.capture_target(), "screen");
    assert_eq!(loaded.replay_seconds(), 60);
}

#[test]
fn traversal_control_characters_and_non_executables_are_rejected() {
    let temporary = tempfile::tempdir().unwrap();
    let clips = temporary.path().join("clips");
    let ffprobe = temporary.path().join("ffprobe");
    fs::create_dir_all(&clips).unwrap();
    executable(&ffprobe);

    assert!(
        CaptureConfiguration::new(
            true,
            CaptureRecorder::Flatpak,
            "../screen",
            clips.clone(),
            ffprobe.clone(),
        )
        .is_err()
    );
    assert!(
        CaptureConfiguration::new(
            true,
            CaptureRecorder::Flatpak,
            "screen\nnext=value",
            clips.clone(),
            ffprobe.clone(),
        )
        .is_err()
    );

    let relative_output = Path::new("clips").to_path_buf();
    assert!(
        CaptureConfiguration::new(
            true,
            CaptureRecorder::Flatpak,
            "screen",
            relative_output,
            ffprobe.clone(),
        )
        .is_err()
    );

    let non_executable = temporary.path().join("not-executable");
    fs::write(&non_executable, "fixture").unwrap();
    assert!(
        CaptureConfiguration::new(
            true,
            CaptureRecorder::Native(non_executable),
            "screen",
            clips,
            ffprobe,
        )
        .is_err()
    );
}

#[test]
fn symlinked_paths_and_configuration_targets_are_rejected_without_mutation() {
    let temporary = tempfile::tempdir().unwrap();
    let data = temporary.path().join("data");
    let real_clips = temporary.path().join("real-clips");
    let linked_clips = temporary.path().join("linked-clips");
    let ffprobe = temporary.path().join("ffprobe");
    fs::create_dir_all(&data).unwrap();
    fs::create_dir_all(&real_clips).unwrap();
    executable(&ffprobe);
    symlink(&real_clips, &linked_clips).unwrap();

    assert!(
        CaptureConfiguration::new(
            true,
            CaptureRecorder::Flatpak,
            "screen",
            linked_clips,
            ffprobe.clone(),
        )
        .is_err()
    );

    let safe = CaptureConfiguration::new(
        true,
        CaptureRecorder::Flatpak,
        "screen",
        real_clips,
        ffprobe,
    )
    .unwrap();
    let outside = temporary.path().join("outside");
    fs::write(&outside, "do not replace").unwrap();
    symlink(&outside, data.join("capture.conf")).unwrap();

    assert!(write_capture_configuration(&data, &safe).is_err());
    assert_eq!(fs::read_to_string(outside).unwrap(), "do not replace");
}
