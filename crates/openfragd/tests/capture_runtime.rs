#[path = "../src/capture_runtime.rs"]
mod capture_runtime;

use capture_runtime::{CaptureRuntime, RuntimeError, RuntimeStatus, UnavailableReason};
use openfrag_capture::{
    Clock, Filesystem, MediaInfo, MediaProbe, Process, SaveDisposition, SaveProvenance, Signal,
};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

#[derive(Default)]
struct ProcessLog {
    spawns: Vec<Vec<OsString>>,
    signals: Vec<Signal>,
}

struct FakeProcess(Arc<Mutex<ProcessLog>>);

impl Process for FakeProcess {
    fn spawn(&mut self, argv: &[OsString]) -> Result<(), String> {
        self.0.lock().expect("process log").spawns.push(argv.into());
        Ok(())
    }

    fn try_wait(&mut self) -> Result<Option<i32>, String> {
        Ok(None)
    }

    fn signal(&mut self, signal: Signal) -> Result<(), String> {
        self.0.lock().expect("process log").signals.push(signal);
        Ok(())
    }

    fn drain_stderr(&mut self) -> Result<Vec<u8>, String> {
        Ok(Vec::new())
    }
}

struct FakeFilesystem {
    output: Option<PathBuf>,
}

impl Filesystem for FakeFilesystem {
    fn outputs_since(&self, _: &Path, _: u64) -> Result<Vec<PathBuf>, String> {
        Ok(self.output.iter().cloned().collect())
    }

    fn is_symlink(&self, _: &Path) -> Result<bool, String> {
        Ok(false)
    }

    fn is_regular_file(&self, _: &Path) -> Result<bool, String> {
        Ok(true)
    }

    fn is_contained(&self, _: &Path, _: &Path) -> Result<bool, String> {
        Ok(true)
    }
}

struct FakeClock;

impl Clock for FakeClock {
    fn now_ms(&self) -> u64 {
        12_000
    }
}

struct FakeProbe;

impl MediaProbe for FakeProbe {
    fn verify(&self, _: &Path) -> Result<MediaInfo, String> {
        Ok(MediaInfo {
            duration_ms: 60_000,
            video_streams: 1,
        })
    }
}

fn runtime(
    config_path: &Path,
    log: Arc<Mutex<ProcessLog>>,
    output: Option<PathBuf>,
) -> CaptureRuntime<FakeProcess, FakeFilesystem, FakeClock, FakeProbe> {
    CaptureRuntime::from_private_config(
        config_path,
        FakeProcess(log),
        FakeFilesystem { output },
        FakeClock,
        FakeProbe,
    )
}

fn write_private(path: &Path, json: &str) {
    std::fs::write(path, json).expect("write capture config");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .expect("private permissions");
    }
}

#[test]
fn missing_and_disabled_configs_never_reach_the_supervisor() {
    let directory = tempfile::tempdir().expect("temp directory");
    let log = Arc::new(Mutex::new(ProcessLog::default()));
    let mut missing = runtime(&directory.path().join("missing.json"), log.clone(), None);
    assert_eq!(
        missing.status(),
        RuntimeStatus::Unavailable(UnavailableReason::Missing)
    );
    assert_eq!(
        missing.start(),
        Err(RuntimeError::Unavailable(UnavailableReason::Missing))
    );

    let path = directory.path().join("capture.json");
    write_private(
        &path,
        r#"{"enabled":false,"recorder":{"kind":"native","program":"gpu-screen-recorder"},"capture_target":"screen","output_directory":"/tmp/openfrag-clips"}"#,
    );
    let mut disabled = runtime(&path, log.clone(), None);
    assert_eq!(disabled.status(), RuntimeStatus::Disabled);
    assert_eq!(
        disabled.request_save(SaveProvenance::ManualFlag),
        Err(RuntimeError::Disabled)
    );
    assert!(log.lock().expect("process log").spawns.is_empty());
}

#[test]
fn insecure_or_invalid_config_is_explicitly_unavailable() {
    let directory = tempfile::tempdir().expect("temp directory");
    let path = directory.path().join("capture.json");
    std::fs::write(&path, b"{}").expect("write config");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))
            .expect("public permissions");
        assert!(matches!(
            runtime(&path, Arc::new(Mutex::new(ProcessLog::default())), None).status(),
            RuntimeStatus::Unavailable(UnavailableReason::Unsafe(_))
        ));
    }
    write_private(
        &path,
        r#"{"enabled":true,"recorder":{"kind":"native","program":""},"capture_target":"screen","output_directory":"relative"}"#,
    );
    assert!(matches!(
        runtime(&path, Arc::new(Mutex::new(ProcessLog::default())), None).status(),
        RuntimeStatus::Unavailable(UnavailableReason::Invalid(_))
    ));
}

#[test]
fn enabled_runtime_delegates_lifecycle_to_fakes_only() {
    let directory = tempfile::tempdir().expect("temp directory");
    let path = directory.path().join("capture.json");
    write_private(
        &path,
        r#"{"enabled":true,"recorder":{"kind":"flatpak","app_id":"com.dec05eba.gpu_screen_recorder"},"capture_target":"screen","output_directory":"/captures"}"#,
    );
    let log = Arc::new(Mutex::new(ProcessLog::default()));
    let mut runtime = runtime(&path, log.clone(), Some("/captures/clip.mp4".into()));
    assert_eq!(runtime.status(), RuntimeStatus::Ready);
    runtime.start().expect("start fake supervisor");
    assert_eq!(runtime.status(), RuntimeStatus::Running);
    assert_eq!(
        runtime
            .request_save(SaveProvenance::ManualFlag)
            .expect("fake save"),
        SaveDisposition::Signalled
    );
    let acknowledgement = runtime
        .discover_save()
        .expect("discover fake save")
        .expect("acknowledgement");
    assert_eq!(acknowledgement.path, PathBuf::from("/captures/clip.mp4"));
    assert_eq!(acknowledgement.provenance, SaveProvenance::ManualFlag);
    assert_eq!(runtime.poll(), Ok(None));
    runtime.shutdown().expect("shutdown fake supervisor");
    assert!(runtime.stderr_tail().expect("stderr tail").is_empty());

    let log = log.lock().expect("process log");
    assert_eq!(
        log.spawns,
        vec![
            vec![
                "flatpak",
                "run",
                "com.dec05eba.gpu_screen_recorder",
                "-w",
                "screen",
                "-r",
                "60",
                "-c",
                "h264",
                "-ac",
                "aac",
                "-o",
                "/captures",
            ]
            .into_iter()
            .map(OsString::from)
            .collect::<Vec<_>>()
        ]
    );
    assert_eq!(log.signals, vec![Signal::User1, Signal::Interrupt]);
}
