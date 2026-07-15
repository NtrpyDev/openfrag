//! Daemon-owned discovery and mutation boundary for first-run setup.

use crate::api::{
    ApiError, SetupAction, SetupActionRequest, SetupCandidate, SetupCheck, SetupResponse,
};
use crate::capture_runtime::CaptureRuntime;
use crate::live_runtime::{ManualFlagStatusHandle, ManualFlagWorkerStatus};
use openfrag_capture::{FfprobeMediaProbe, SaveProvenance, StdClock, StdFilesystem, StdProcess};
use openfrag_setup::{
    CaptureConfiguration, CaptureRecorder, discover_cs2_cfg_directories, install_actionable_gsi,
    read_capture_configuration, validate_cs2_cfg_directory, write_capture_configuration,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs,
    hash::{Hash, Hasher},
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

static STATE_TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);

pub trait SetupHost: Send + Sync + 'static {
    fn cs2_cfg_candidates(&self) -> Vec<PathBuf>;
    fn install_gsi(&self, cfg: &Path) -> Result<(), String>;
    fn gsi_installed(&self, cfg: &Path) -> bool;
    fn gsi_active(&self) -> bool;
    fn identity_verified(&self) -> bool;
    fn executable(&self, name: &str) -> Option<PathBuf>;
    fn validate_executable(&self, path: &Path) -> bool;
    fn flatpak_recorder_available(&self) -> bool;
    fn capture_configured(&self) -> bool;
    fn configure_capture(&self, selection: &CaptureSelection) -> Result<(), String>;
    fn run_test_capture(&self) -> Result<(), String>;
    fn manual_flag_available(&self) -> bool;
}

#[derive(Clone, Debug)]
pub struct CaptureSelection {
    pub recorder: String,
    pub recorder_path: Option<PathBuf>,
    pub target: String,
    pub output_directory: PathBuf,
    pub ffprobe_path: PathBuf,
}

pub struct SystemSetupHost {
    home: PathBuf,
    config_directory: PathBuf,
    data_directory: PathBuf,
    xdg_data_home: Option<PathBuf>,
    gsi_active: bool,
    manual_flag_status: Option<ManualFlagStatusHandle>,
}

impl SystemSetupHost {
    #[must_use]
    pub fn new(
        home: PathBuf,
        data_directory: PathBuf,
        config_directory: PathBuf,
        xdg_data_home: Option<PathBuf>,
        gsi_active: bool,
    ) -> Self {
        Self {
            home,
            config_directory,
            data_directory,
            xdg_data_home,
            gsi_active,
            manual_flag_status: None,
        }
    }

    #[must_use]
    pub fn with_manual_flag_status(mut self, status: ManualFlagStatusHandle) -> Self {
        self.manual_flag_status = Some(status);
        self
    }
}

impl SetupHost for SystemSetupHost {
    fn cs2_cfg_candidates(&self) -> Vec<PathBuf> {
        discover_cs2_cfg_directories(&self.home, self.xdg_data_home.as_deref())
    }

    fn install_gsi(&self, cfg: &Path) -> Result<(), String> {
        install_actionable_gsi(cfg, &self.config_directory)
            .map(|_| ())
            .map_err(|error| format!("cannot install loopback GSI config: {error:?}"))
    }

    fn gsi_installed(&self, cfg: &Path) -> bool {
        let path = cfg.join("gamestate_integration_openfrag.cfg");
        fs::read_to_string(path).is_ok_and(|contents| {
            contents.contains("http://127.0.0.1:7130/gsi/router") && !contents.contains("0.0.0.0")
        })
    }

    fn gsi_active(&self) -> bool {
        self.gsi_active
    }

    fn identity_verified(&self) -> bool {
        fs::read_to_string(self.data_directory.join("local-steam-id")).is_ok_and(|value| {
            let value = value.trim();
            value.len() == 17 && value.bytes().all(|byte| byte.is_ascii_digit())
        })
    }

    fn executable(&self, name: &str) -> Option<PathBuf> {
        std::env::var_os("PATH")
            .into_iter()
            .flat_map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
            .map(|directory| directory.join(name))
            .find(|path| self.validate_executable(path))
    }

    fn validate_executable(&self, path: &Path) -> bool {
        use std::os::unix::fs::PermissionsExt as _;
        path.is_absolute()
            && !path.components().any(|component| {
                matches!(
                    component,
                    std::path::Component::CurDir | std::path::Component::ParentDir
                )
            })
            && !has_symlink_component(path)
            && fs::symlink_metadata(path).is_ok_and(|metadata| {
                metadata.is_file()
                    && !metadata.file_type().is_symlink()
                    && metadata.permissions().mode() & 0o111 != 0
            })
    }

    fn flatpak_recorder_available(&self) -> bool {
        let installations = [
            self.home
                .join(".local/share/flatpak/app/com.dec05eba.gpu_screen_recorder"),
            PathBuf::from("/var/lib/flatpak/app/com.dec05eba.gpu_screen_recorder"),
        ];
        installations.iter().any(|path| path.is_dir())
    }

    fn capture_configured(&self) -> bool {
        read_capture_configuration(&self.data_directory).is_ok()
    }

    fn configure_capture(&self, selection: &CaptureSelection) -> Result<(), String> {
        let recorder = match selection.recorder.as_str() {
            "native" => CaptureRecorder::Native(
                selection
                    .recorder_path
                    .clone()
                    .or_else(|| self.executable("gpu-screen-recorder"))
                    .ok_or_else(|| "native gpu-screen-recorder was not found".to_owned())?,
            ),
            "flatpak" if self.flatpak_recorder_available() => CaptureRecorder::Flatpak,
            "flatpak" => return Err("the gpu-screen-recorder Flatpak was not found".into()),
            _ => return Err("choose the native or Flatpak recorder".into()),
        };
        let configuration = CaptureConfiguration::new(
            true,
            recorder,
            selection.target.clone(),
            selection.output_directory.clone(),
            selection.ffprobe_path.clone(),
        )
        .map_err(|error| format!("invalid capture configuration: {error:?}"))?;
        write_capture_configuration(&self.data_directory, &configuration)
            .map(|_| ())
            .map_err(|error| format!("cannot save capture configuration: {error:?}"))
    }

    fn run_test_capture(&self) -> Result<(), String> {
        let configuration = read_capture_configuration(&self.data_directory)
            .map_err(|error| format!("capture is not configured: {error:?}"))?;
        let probe = FfprobeMediaProbe {
            program: configuration.ffprobe_path().to_path_buf(),
            timeout: Duration::from_secs(2),
            max_output_bytes: 64 * 1024,
        };
        let mut runtime = CaptureRuntime::from_data_directory(
            &self.data_directory,
            StdProcess::default(),
            StdFilesystem,
            StdClock::default(),
            probe,
        );
        runtime
            .start()
            .map_err(|error| format!("cannot start bounded capture test: {error:?}"))?;
        std::thread::sleep(Duration::from_millis(500));
        if let Err(error) = runtime.request_save("setup-test", SaveProvenance::ManualFlag) {
            let _ = runtime.shutdown();
            return Err(format!("cannot request test replay: {error:?}"));
        }
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            if Instant::now() >= deadline {
                let _ = runtime.shutdown();
                return Err("bounded capture test exceeded 8 seconds".into());
            }
            let acknowledgement = match runtime.discover_save() {
                Ok(value) => value,
                Err(error) => {
                    let _ = runtime.shutdown();
                    return Err(format!("cannot verify test replay: {error:?}"));
                }
            };
            if let Some(acknowledgement) = acknowledgement {
                runtime
                    .shutdown()
                    .map_err(|error| format!("cannot stop test recorder: {error:?}"))?;
                let _ = fs::remove_file(acknowledgement.path);
                return Ok(());
            }
            let exit = match runtime.poll() {
                Ok(value) => value,
                Err(error) => {
                    let _ = runtime.shutdown();
                    return Err(format!("test recorder failed: {error:?}"));
                }
            };
            if let Some(code) = exit {
                let _ = runtime.shutdown();
                return Err(format!("test recorder exited with code {code}"));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn manual_flag_available(&self) -> bool {
        self.manual_flag_status
            .as_ref()
            .is_some_and(|status| status.status() == ManualFlagWorkerStatus::Ready)
    }
}

#[derive(Clone, Debug, Default, Deserialize, Hash, Serialize)]
struct PersistedSetup {
    revision: u64,
    skipped: BTreeSet<String>,
    selected_cfg: Option<PathBuf>,
    background_enabled: Option<bool>,
    test_capture_passed: bool,
    ffprobe_path: Option<PathBuf>,
}

pub struct SetupRuntime {
    host: Arc<dyn SetupHost>,
    state_path: PathBuf,
    state: Mutex<PersistedSetup>,
}

impl SetupRuntime {
    pub fn new(host: Arc<dyn SetupHost>, state_path: PathBuf) -> Result<Self, ApiError> {
        let state = match fs::symlink_metadata(&state_path) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                return Err(ApiError::Unavailable(
                    "setup state must be a private regular file".into(),
                ));
            }
            Ok(metadata) if metadata.permissions().mode() & 0o077 != 0 => {
                return Err(ApiError::Unavailable(
                    "setup state permissions must be private".into(),
                ));
            }
            Ok(_) => serde_json::from_slice(&fs::read(&state_path).map_err(|error| {
                ApiError::Unavailable(format!("cannot read setup state: {error}"))
            })?)
            .map_err(|error| ApiError::Unavailable(format!("invalid setup state: {error}")))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => PersistedSetup::default(),
            Err(error) => {
                return Err(ApiError::Unavailable(format!(
                    "cannot inspect setup state: {error}"
                )));
            }
        };
        Ok(Self {
            host,
            state_path,
            state: Mutex::new(state),
        })
    }

    pub fn response(&self) -> Result<SetupResponse, ApiError> {
        let state = self
            .state
            .lock()
            .map_err(|_| ApiError::Unavailable("setup state lock is unavailable".into()))?;
        Ok(self.response_for(&state))
    }

    pub fn act(&self, request: SetupActionRequest) -> Result<SetupResponse, ApiError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| ApiError::Unavailable("setup state lock is unavailable".into()))?;
        let current = self.response_for(&state);
        if request.expected_fingerprint != current.fingerprint {
            return Err(ApiError::Conflict(
                "setup changed; reload before applying this action".into(),
            ));
        }
        self.apply_action(&mut state, request)?;
        state.revision = state.revision.saturating_add(1);
        self.persist(&state)?;
        Ok(self.response_for(&state))
    }

    fn apply_action(
        &self,
        state: &mut PersistedSetup,
        request: SetupActionRequest,
    ) -> Result<(), ApiError> {
        match request.action.as_str() {
            "install_gsi" => {
                if !request.consent {
                    return Err(ApiError::Invalid(
                        "installing the CS2 cfg requires explicit consent".into(),
                    ));
                }
                let cfg = request
                    .path
                    .ok_or_else(|| ApiError::Invalid("select a CS2 cfg directory".into()))?;
                validate_cs2_cfg_directory(&cfg).map_err(|error| {
                    ApiError::Invalid(format!("invalid CS2 cfg directory: {error:?}"))
                })?;
                self.host.install_gsi(&cfg).map_err(ApiError::Unavailable)?;
                state.selected_cfg = Some(cfg);
                state.skipped.remove("gsi");
            }
            "skip" => {
                let step = request
                    .step
                    .ok_or_else(|| ApiError::Invalid("select a setup step to defer".into()))?;
                if !OPTIONAL_STEPS.contains(&step.as_str()) {
                    return Err(ApiError::Invalid(format!("{step} cannot be deferred")));
                }
                state.skipped.insert(step);
            }
            "set_background" => {
                state.background_enabled = Some(request.consent);
            }
            "check_identity" => {
                if !self.host.identity_verified() {
                    return Err(ApiError::Unavailable(
                        "No verified local identity is available from GSI or a Demo yet".into(),
                    ));
                }
                state.skipped.remove("local_identity");
            }
            "configure_capture" => {
                self.apply_capture_configuration(state, request)?;
            }
            "set_ffprobe" => {
                let path = request
                    .ffprobe_path
                    .ok_or_else(|| ApiError::Invalid("choose the ffprobe executable".into()))?;
                if !self.host.validate_executable(&path) {
                    return Err(ApiError::Invalid(
                        "ffprobe must be an absolute executable file".into(),
                    ));
                }
                state.ffprobe_path = Some(path);
                state.skipped.remove("ffprobe");
            }
            "run_test_capture" => {
                if !request.consent {
                    return Err(ApiError::Invalid(
                        "the bounded capture test requires explicit consent".into(),
                    ));
                }
                self.host
                    .run_test_capture()
                    .map_err(ApiError::Unavailable)?;
                state.test_capture_passed = true;
                state.skipped.remove("test_capture");
            }
            "request_manual_flag" => {
                if !request.consent {
                    return Err(ApiError::Invalid(
                        "Manual Flag approval requires explicit consent".into(),
                    ));
                }
                if !self.host.manual_flag_available() {
                    return Err(ApiError::Unavailable(
                        "Global Shortcuts portal approval is unavailable in this session".into(),
                    ));
                }
            }
            other => return Err(ApiError::Invalid(format!("unknown setup action: {other}"))),
        }
        Ok(())
    }

    fn apply_capture_configuration(
        &self,
        state: &mut PersistedSetup,
        request: SetupActionRequest,
    ) -> Result<(), ApiError> {
        if !request.consent {
            return Err(ApiError::Invalid(
                "saving capture paths requires explicit consent".into(),
            ));
        }
        let selection = CaptureSelection {
            recorder: request
                .recorder
                .ok_or_else(|| ApiError::Invalid("choose a recorder".into()))?,
            recorder_path: request.path,
            target: request
                .capture_target
                .ok_or_else(|| ApiError::Invalid("enter an explicit capture target".into()))?,
            output_directory: request
                .output_directory
                .ok_or_else(|| ApiError::Invalid("choose a capture output directory".into()))?,
            ffprobe_path: request
                .ffprobe_path
                .ok_or_else(|| ApiError::Invalid("choose the ffprobe executable".into()))?,
        };
        self.host
            .configure_capture(&selection)
            .map_err(ApiError::Invalid)?;
        state.skipped.remove("capture");
        state.skipped.remove("ffprobe");
        state.test_capture_passed = false;
        state.ffprobe_path = Some(selection.ffprobe_path);
        Ok(())
    }

    fn response_for(&self, state: &PersistedSetup) -> SetupResponse {
        let candidates = self.host.cs2_cfg_candidates();
        let gsi_installed = self.host.gsi_active()
            || state
                .selected_cfg
                .as_deref()
                .is_some_and(|cfg| self.host.gsi_installed(cfg))
            || candidates.iter().any(|cfg| self.host.gsi_installed(cfg));
        let discovery = SetupDiscovery {
            candidates,
            gsi_installed: gsi_installed.into(),
            identity_verified: self.host.identity_verified().into(),
            native_recorder: self.host.executable("gpu-screen-recorder"),
            flatpak_recorder: self.host.flatpak_recorder_available().into(),
            ffprobe: state
                .ffprobe_path
                .clone()
                .filter(|path| self.host.validate_executable(path))
                .or_else(|| self.host.executable("ffprobe")),
            capture_configured: self.host.capture_configured().into(),
        };
        let checks = self.checks_for(state, &discovery);
        let complete = checks.iter().all(|check| {
            matches!(
                check.status.as_str(),
                "ready" | "not_tested" | "unavailable"
            )
        });
        let mut fingerprint = std::collections::hash_map::DefaultHasher::new();
        state.hash(&mut fingerprint);
        discovery.hash(&mut fingerprint);
        SetupResponse {
            fingerprint: format!("setup-{:016x}", fingerprint.finish()),
            complete,
            checks,
            cs2_cfg_candidates: discovery
                .candidates
                .into_iter()
                .map(|path| SetupCandidate {
                    path: path.display().to_string(),
                    source: "steam_library".into(),
                })
                .collect(),
        }
    }

    fn checks_for(&self, state: &PersistedSetup, discovery: &SetupDiscovery) -> Vec<SetupCheck> {
        let mut checks = initial_checks(state, discovery);
        checks.extend(capture_checks(state, discovery));
        checks.push(check(
            "demo_import",
            "ready",
            "A local Demo can be selected from the dashboard",
            vec![],
        ));
        checks.push(optional_check(
            state,
            "manual_flag",
            if self.host.manual_flag_available() {
                "needs_action"
            } else {
                "unavailable"
            },
            "Approve Manual Flag through the desktop Global Shortcuts portal",
            vec![
                action("request_manual_flag", "Request portal approval", true),
                action("skip", "Approve later", false),
            ],
        ));
        checks.push(background_check(state));
        checks
    }

    fn persist(&self, state: &PersistedSetup) -> Result<(), ApiError> {
        let parent = self
            .state_path
            .parent()
            .ok_or_else(|| ApiError::Unavailable("setup state path has no parent".into()))?;
        fs::create_dir_all(parent).map_err(|error| {
            ApiError::Unavailable(format!("cannot create setup state directory: {error}"))
        })?;
        if has_symlink_component(parent) {
            return Err(ApiError::Unavailable(
                "setup state directory cannot contain symlinks".into(),
            ));
        }
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700)).map_err(|error| {
            ApiError::Unavailable(format!("cannot protect setup state directory: {error}"))
        })?;
        let temporary = self.state_path.with_extension(format!(
            "{}-{}.tmp",
            std::process::id(),
            STATE_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let bytes = serde_json::to_vec_pretty(state).map_err(|error| {
            ApiError::Unavailable(format!("cannot encode setup state: {error}"))
        })?;
        let write_result = (|| {
            let mut file = fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(0o600)
                .open(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            fs::rename(&temporary, &self.state_path)?;
            fs::set_permissions(&self.state_path, fs::Permissions::from_mode(0o600))
        })();
        if write_result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        write_result
            .map_err(|error| ApiError::Unavailable(format!("cannot persist setup state: {error}")))
    }
}

#[derive(Hash)]
struct SetupDiscovery {
    candidates: Vec<PathBuf>,
    gsi_installed: Observed,
    identity_verified: Observed,
    native_recorder: Option<PathBuf>,
    flatpak_recorder: Observed,
    ffprobe: Option<PathBuf>,
    capture_configured: Observed,
}

#[derive(Clone, Copy, Hash)]
enum Observed {
    Yes,
    No,
}

impl Observed {
    const fn yes(self) -> bool {
        matches!(self, Self::Yes)
    }
}

impl From<bool> for Observed {
    fn from(value: bool) -> Self {
        if value { Self::Yes } else { Self::No }
    }
}

fn initial_checks(state: &PersistedSetup, discovery: &SetupDiscovery) -> Vec<SetupCheck> {
    let mut checks = vec![check(
        "storage",
        "ready",
        "Private local storage is ready",
        vec![],
    )];
    checks.push(if discovery.gsi_installed.yes() {
        check("gsi", "ready", "Private loopback GSI is installed", vec![])
    } else if state.skipped.contains("gsi") {
        deferred("gsi", "Live GSI setup was deferred")
    } else {
        check(
            "gsi",
            "needs_action",
            if discovery.candidates.is_empty() {
                "Choose the exact CS2 game/csgo/cfg directory"
            } else {
                "Install the loopback GSI config in a discovered CS2 directory"
            },
            vec![
                action("install_gsi", "Install GSI config", true),
                action("skip", "Do this later", false),
            ],
        )
    });
    checks.push(optional_check(
        state,
        "local_identity",
        if discovery.identity_verified.yes() {
            "ready"
        } else {
            "needs_action"
        },
        "Identity is verified from GSI or an imported Demo, never free-form input",
        vec![
            action("check_identity", "Check verified evidence", false),
            action("skip", "Verify later", false),
        ],
    ));
    checks
}

fn capture_checks(state: &PersistedSetup, discovery: &SetupDiscovery) -> Vec<SetupCheck> {
    let recorder_summary = match (
        discovery.native_recorder.as_ref(),
        discovery.flatpak_recorder.yes(),
    ) {
        (Some(path), true) => {
            format!("Choose native {} or the Flatpak recorder", path.display())
        }
        (Some(path), false) => format!("Native recorder found at {}", path.display()),
        (None, true) => "Flatpak recorder found".into(),
        (None, false) => "gpu-screen-recorder was not found".into(),
    };
    vec![
        optional_check(
            state,
            "capture",
            if discovery.capture_configured.yes() {
                "ready"
            } else if discovery.native_recorder.is_some() || discovery.flatpak_recorder.yes() {
                "needs_action"
            } else {
                "unavailable"
            },
            if discovery.capture_configured.yes() {
                "Replay capture paths are configured"
            } else {
                &recorder_summary
            },
            vec![
                action("configure_capture", "Configure replay capture", true),
                action("skip", "Use Demo import without capture", false),
            ],
        ),
        optional_check(
            state,
            "ffprobe",
            if discovery.ffprobe.is_some() {
                "ready"
            } else {
                "unavailable"
            },
            discovery
                .ffprobe
                .as_ref()
                .map_or("ffprobe was not found".into(), |path| {
                    format!("ffprobe found at {}", path.display())
                })
                .as_str(),
            vec![
                action("set_ffprobe", "Use this ffprobe path", false),
                action("skip", "Configure later", false),
            ],
        ),
        optional_check(
            state,
            "test_capture",
            if state.test_capture_passed {
                "ready"
            } else {
                "not_tested"
            },
            if state.test_capture_passed {
                "Bounded replay test passed"
            } else {
                "No bounded replay test has run"
            },
            vec![
                action("run_test_capture", "Run bounded test", true),
                action("skip", "Test later", false),
            ],
        ),
    ]
}

fn background_check(state: &PersistedSetup) -> SetupCheck {
    if let Some(enabled) = state.background_enabled {
        check(
            "background",
            "ready",
            if enabled {
                "Background start is enabled; capture still requires an explicit session"
            } else {
                "Background start is disabled"
            },
            vec![action("set_background", "Change preference", false)],
        )
    } else {
        check(
            "background",
            "needs_action",
            "Choose whether openfrag starts in the background",
            vec![
                action("set_background", "Enable background start", true),
                action("set_background", "Keep disabled", false),
            ],
        )
    }
}

const OPTIONAL_STEPS: &[&str] = &[
    "gsi",
    "local_identity",
    "capture",
    "ffprobe",
    "test_capture",
    "manual_flag",
];

fn has_symlink_component(path: &Path) -> bool {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        if fs::symlink_metadata(&current).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
            return true;
        }
    }
    false
}

fn action(id: &str, label: &str, requires_consent: bool) -> SetupAction {
    SetupAction {
        id: id.into(),
        label: label.into(),
        requires_consent,
    }
}

fn check(id: &str, status: &str, summary: &str, actions: Vec<SetupAction>) -> SetupCheck {
    SetupCheck {
        id: id.into(),
        status: status.into(),
        summary: summary.into(),
        actions,
    }
}

fn deferred(id: &str, summary: &str) -> SetupCheck {
    check(
        id,
        "not_tested",
        summary,
        vec![action("skip", "Deferred", false)],
    )
}

fn optional_check(
    state: &PersistedSetup,
    id: &str,
    status: &str,
    summary: &str,
    actions: Vec<SetupAction>,
) -> SetupCheck {
    if state.skipped.contains(id) {
        deferred(id, &format!("{summary}. Deferred by choice"))
    } else {
        check(id, status, summary, actions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct FakeHost {
        cfg: PathBuf,
        installs: AtomicUsize,
    }

    impl SetupHost for FakeHost {
        fn cs2_cfg_candidates(&self) -> Vec<PathBuf> {
            vec![self.cfg.clone()]
        }
        fn install_gsi(&self, _: &Path) -> Result<(), String> {
            self.installs.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn gsi_installed(&self, _: &Path) -> bool {
            self.installs.load(Ordering::SeqCst) > 0
        }
        fn gsi_active(&self) -> bool {
            false
        }
        fn identity_verified(&self) -> bool {
            false
        }
        fn executable(&self, _: &str) -> Option<PathBuf> {
            None
        }
        fn validate_executable(&self, _: &Path) -> bool {
            false
        }
        fn flatpak_recorder_available(&self) -> bool {
            false
        }
        fn capture_configured(&self) -> bool {
            false
        }
        fn configure_capture(&self, _: &CaptureSelection) -> Result<(), String> {
            Ok(())
        }
        fn run_test_capture(&self) -> Result<(), String> {
            Ok(())
        }
        fn manual_flag_available(&self) -> bool {
            false
        }
    }

    #[test]
    fn injected_host_is_not_called_without_consent_and_stale_actions_are_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let cfg = temp.path().join("game/csgo/cfg");
        fs::create_dir_all(&cfg).unwrap();
        fs::write(cfg.parent().unwrap().join("gameinfo.gi"), "gameinfo").unwrap();
        let host = Arc::new(FakeHost {
            cfg: cfg.clone(),
            installs: AtomicUsize::new(0),
        });
        let runtime =
            SetupRuntime::new(host.clone(), temp.path().join("state/setup.json")).unwrap();
        let fingerprint = runtime.response().unwrap().fingerprint;
        let request = |consent| SetupActionRequest {
            action: "install_gsi".into(),
            expected_fingerprint: fingerprint.clone(),
            consent,
            step: None,
            path: Some(cfg.clone()),
            recorder: None,
            capture_target: None,
            output_directory: None,
            ffprobe_path: None,
        };
        assert!(matches!(
            runtime.act(request(false)),
            Err(ApiError::Invalid(_))
        ));
        assert_eq!(host.installs.load(Ordering::SeqCst), 0);
        runtime.act(request(true)).unwrap();
        assert_eq!(host.installs.load(Ordering::SeqCst), 1);
        assert!(matches!(
            runtime.act(request(true)),
            Err(ApiError::Conflict(_))
        ));
    }
}
