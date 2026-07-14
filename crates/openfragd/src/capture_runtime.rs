use openfrag_capture::{
    Clock, Config, Error as SupervisorError, Filesystem, MediaProbe, RecorderInstall, ReplayConfig,
    SaveAcknowledgement, SaveDisposition, SaveProvenance, Supervisor, replay_launch,
};
use serde::Deserialize;
use std::{ffi::OsString, fs, path::Path, path::PathBuf};

const MAX_CONFIG_BYTES: u64 = 64 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UnavailableReason {
    Missing,
    Unsafe(String),
    Invalid(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RuntimeStatus {
    Disabled,
    Unavailable(UnavailableReason),
    Ready,
    Running,
    RestartPending { at_ms: u64 },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RuntimeError {
    Disabled,
    Unavailable(UnavailableReason),
    Supervisor(SupervisorError),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DiskConfig {
    enabled: bool,
    recorder: RecorderConfig,
    capture_target: String,
    output_directory: PathBuf,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum RecorderConfig {
    Native { program: String },
    Flatpak { app_id: String },
}

enum Gate<P, F, C, M> {
    Disabled,
    Unavailable(UnavailableReason),
    Ready(Supervisor<P, F, C, M>),
}

/// Gated daemon runtime for one replay-buffer supervisor.
pub struct CaptureRuntime<P, F, C, M> {
    gate: Gate<P, F, C, M>,
}

impl<P, F, C, M> CaptureRuntime<P, F, C, M>
where
    P: openfrag_capture::Process,
    F: Filesystem,
    C: Clock,
    M: MediaProbe,
{
    #[must_use]
    pub fn from_private_config(
        config_path: &Path,
        process: P,
        filesystem: F,
        clock: C,
        probe: M,
    ) -> Self {
        let disk = match read_private_config(config_path) {
            Ok(config) => config,
            Err(reason) => {
                return Self {
                    gate: Gate::Unavailable(reason),
                };
            }
        };
        if !disk.enabled {
            return Self {
                gate: Gate::Disabled,
            };
        }
        let replay = ReplayConfig::sixty_second(
            OsString::from(disk.capture_target),
            disk.output_directory.clone(),
        );
        let install = match disk.recorder {
            RecorderConfig::Native { program } => RecorderInstall::Native {
                program: OsString::from(program),
            },
            RecorderConfig::Flatpak { app_id } => RecorderInstall::Flatpak {
                app_id: OsString::from(app_id),
            },
        };
        let config = Config::new(replay_launch(install, replay), disk.output_directory);
        Self {
            gate: Gate::Ready(Supervisor::new(process, filesystem, clock, probe, config)),
        }
    }

    #[must_use]
    pub fn status(&self) -> RuntimeStatus {
        match &self.gate {
            Gate::Disabled => RuntimeStatus::Disabled,
            Gate::Unavailable(reason) => RuntimeStatus::Unavailable(reason.clone()),
            Gate::Ready(supervisor) if supervisor.is_running() => RuntimeStatus::Running,
            Gate::Ready(supervisor) => supervisor
                .next_restart_ms()
                .map_or(RuntimeStatus::Ready, |at_ms| {
                    RuntimeStatus::RestartPending { at_ms }
                }),
        }
    }

    pub fn start(&mut self) -> Result<(), RuntimeError> {
        self.supervisor_mut()?
            .start()
            .map_err(RuntimeError::Supervisor)
    }

    pub fn poll(&mut self) -> Result<Option<i32>, RuntimeError> {
        self.supervisor_mut()?
            .poll()
            .map_err(RuntimeError::Supervisor)
    }

    pub fn request_save(
        &mut self,
        provenance: SaveProvenance,
    ) -> Result<SaveDisposition, RuntimeError> {
        self.supervisor_mut()?
            .request_save(provenance)
            .map_err(RuntimeError::Supervisor)
    }

    pub fn discover_save(&mut self) -> Result<Option<SaveAcknowledgement>, RuntimeError> {
        self.supervisor_mut()?
            .discover_save()
            .map_err(RuntimeError::Supervisor)
    }

    pub fn shutdown(&mut self) -> Result<(), RuntimeError> {
        self.supervisor_mut()?
            .shutdown()
            .map_err(RuntimeError::Supervisor)
    }

    pub fn stderr_tail(&self) -> Result<Vec<u8>, RuntimeError> {
        self.supervisor().map(Supervisor::stderr_tail)
    }

    fn supervisor(&self) -> Result<&Supervisor<P, F, C, M>, RuntimeError> {
        match &self.gate {
            Gate::Disabled => Err(RuntimeError::Disabled),
            Gate::Unavailable(reason) => Err(RuntimeError::Unavailable(reason.clone())),
            Gate::Ready(supervisor) => Ok(supervisor),
        }
    }

    fn supervisor_mut(&mut self) -> Result<&mut Supervisor<P, F, C, M>, RuntimeError> {
        match &mut self.gate {
            Gate::Disabled => Err(RuntimeError::Disabled),
            Gate::Unavailable(reason) => Err(RuntimeError::Unavailable(reason.clone())),
            Gate::Ready(supervisor) => Ok(supervisor),
        }
    }
}

fn read_private_config(path: &Path) -> Result<DiskConfig, UnavailableReason> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            UnavailableReason::Missing
        } else {
            UnavailableReason::Unsafe(format!("cannot inspect capture config: {error}"))
        }
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(UnavailableReason::Unsafe(
            "capture config must be a regular file".into(),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(UnavailableReason::Unsafe(
                "capture config must not be accessible by group or other users".into(),
            ));
        }
    }
    if metadata.len() > MAX_CONFIG_BYTES {
        return Err(UnavailableReason::Unsafe(
            "capture config exceeds 64 KiB".into(),
        ));
    }
    let bytes = fs::read(path).map_err(|error| {
        UnavailableReason::Unsafe(format!("cannot read capture config: {error}"))
    })?;
    let config: DiskConfig = serde_json::from_slice(&bytes).map_err(|error| {
        UnavailableReason::Invalid(format!("capture config JSON is invalid: {error}"))
    })?;
    validate_config(&config)?;
    Ok(config)
}

fn validate_config(config: &DiskConfig) -> Result<(), UnavailableReason> {
    if config.capture_target.trim().is_empty() {
        return Err(UnavailableReason::Invalid(
            "capture_target must not be empty".into(),
        ));
    }
    if !config.output_directory.is_absolute() {
        return Err(UnavailableReason::Invalid(
            "output_directory must be absolute".into(),
        ));
    }
    let identifier = match &config.recorder {
        RecorderConfig::Native { program } => program,
        RecorderConfig::Flatpak { app_id } => app_id,
    };
    if identifier.trim().is_empty() {
        return Err(UnavailableReason::Invalid(
            "recorder identifier must not be empty".into(),
        ));
    }
    Ok(())
}
