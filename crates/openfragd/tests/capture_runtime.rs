use openfrag_capture::{Clock, Filesystem, MediaInfo, MediaProbe, Process, SaveProvenance, Signal};
use openfrag_setup::{CaptureConfiguration, CaptureRecorder, write_capture_configuration};
use openfragd::{
    capture_runtime::{CaptureRuntime, RuntimeError, RuntimeStatus, UnavailableReason},
    live_runtime::CaptureRuntimePort,
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
    data_directory: &Path,
    log: Arc<Mutex<ProcessLog>>,
    output: Option<PathBuf>,
) -> CaptureRuntime<FakeProcess, FakeFilesystem, FakeClock, FakeProbe> {
    CaptureRuntime::from_data_directory(
        data_directory,
        FakeProcess(log),
        FakeFilesystem { output },
        FakeClock,
        FakeProbe,
    )
}

fn assert_live_port_ready(port: &impl CaptureRuntimePort) {
    assert_eq!(port.readiness(), Ok(()));
    assert_eq!(port.available_from_ms(), 12_000);
}

fn make_executable(path: &Path) {
    std::fs::write(path, b"fake executable").expect("write executable");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .expect("executable permissions");
    }
}

fn write_configuration(data_directory: &Path, enabled: bool, recorder: CaptureRecorder) {
    let output = data_directory.join("captures");
    std::fs::create_dir_all(&output).expect("capture directory");
    let ffprobe = data_directory.join("ffprobe");
    make_executable(&ffprobe);
    let configuration = CaptureConfiguration::new(enabled, recorder, "screen", output, ffprobe)
        .expect("valid capture configuration");
    write_capture_configuration(data_directory, &configuration).expect("write capture config");
}

#[test]
fn missing_and_disabled_configs_never_reach_the_supervisor() {
    let directory = tempfile::tempdir().expect("temp directory");
    let log = Arc::new(Mutex::new(ProcessLog::default()));
    let mut missing = runtime(directory.path(), log.clone(), None);
    assert_eq!(
        missing.status(),
        RuntimeStatus::Unavailable(UnavailableReason::Missing)
    );
    assert_eq!(
        missing.start(),
        Err(RuntimeError::Unavailable(UnavailableReason::Missing))
    );

    let recorder = directory.path().join("gpu-screen-recorder");
    make_executable(&recorder);
    write_configuration(directory.path(), false, CaptureRecorder::Native(recorder));
    let mut disabled = runtime(directory.path(), log.clone(), None);
    assert_eq!(disabled.status(), RuntimeStatus::Disabled);
    assert_eq!(
        disabled.request_save("manual-save", SaveProvenance::ManualFlag),
        Err(RuntimeError::Disabled)
    );
    assert!(log.lock().expect("process log").spawns.is_empty());
}

#[test]
fn insecure_or_invalid_config_is_explicitly_unavailable() {
    let directory = tempfile::tempdir().expect("temp directory");
    let path = directory.path().join("capture.conf");
    std::fs::write(&path, b"{}").expect("write config");
    assert!(matches!(
        runtime(
            directory.path(),
            Arc::new(Mutex::new(ProcessLog::default())),
            None
        )
        .status(),
        RuntimeStatus::Unavailable(UnavailableReason::Invalid(_))
    ));

    std::fs::remove_file(&path).expect("remove malformed config");
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("missing", &path).expect("symlink capture config");
        assert!(matches!(
            runtime(
                directory.path(),
                Arc::new(Mutex::new(ProcessLog::default())),
                None
            )
            .status(),
            RuntimeStatus::Unavailable(UnavailableReason::Unsafe(_))
        ));
    }
}

#[test]
fn enabled_runtime_delegates_lifecycle_to_fakes_only() {
    let directory = tempfile::tempdir().expect("temp directory");
    write_configuration(directory.path(), true, CaptureRecorder::Flatpak);
    let output = directory.path().join("captures/clip.mp4");
    let log = Arc::new(Mutex::new(ProcessLog::default()));
    let mut runtime = runtime(directory.path(), log.clone(), Some(output.clone()));
    assert_eq!(runtime.status(), RuntimeStatus::Ready);
    runtime.start_at(12_000).expect("start fake supervisor");
    assert_eq!(runtime.status(), RuntimeStatus::Running);
    assert_live_port_ready(&runtime);
    assert_eq!(
        CaptureRuntimePort::request_save(&mut runtime, "manual-save", SaveProvenance::ManualFlag,)
            .expect("fake save")
            .recorder_request_id,
        "manual-save"
    );
    let acknowledgement = CaptureRuntimePort::discover_save(&mut runtime)
        .expect("discover fake save")
        .expect("acknowledgement");
    assert_eq!(acknowledgement.path, output);
    assert_eq!(acknowledgement.recorder_request_id, "manual-save");
    assert_eq!(acknowledgement.provenance, SaveProvenance::ManualFlag);
    assert_eq!(CaptureRuntimePort::poll(&mut runtime), Ok(None));
    CaptureRuntimePort::shutdown(&mut runtime).expect("shutdown fake supervisor");
    assert!(runtime.stderr_tail().expect("stderr tail").is_empty());

    let log = log.lock().expect("process log");
    let mut expected = [
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
    ]
    .into_iter()
    .map(OsString::from)
    .collect::<Vec<_>>();
    expected.push(directory.path().join("captures").into_os_string());
    assert_eq!(log.spawns, vec![expected]);
    assert_eq!(log.signals, vec![Signal::User1, Signal::Interrupt]);
}
