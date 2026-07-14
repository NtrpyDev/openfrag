use openfrag_capture::{
    Clock, Config, Error as SupervisorError, Filesystem, MediaProbe, RecorderInstall, ReplayConfig,
    SaveAcknowledgement, SaveDisposition, SaveProvenance, Supervisor, replay_launch,
};
use openfrag_setup::{
    CaptureConfigError, CaptureRecorder, read_capture_configuration,
};
use std::{ffi::OsString, path::Path};

const FLATPAK_APP_ID: &str = "com.dec05eba.gpu_screen_recorder";

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

enum Gate<P, F, C, M> {
    Disabled,
    Unavailable(UnavailableReason),
    Ready(Supervisor<P, F, C, M>),
}

/// Gated daemon runtime for one replay-buffer supervisor.
pub struct CaptureRuntime<P, F, C, M> {
    gate: Gate<P, F, C, M>,
    available_from_ms: Option<u64>,
}

impl<P, F, C, M> CaptureRuntime<P, F, C, M>
where
    P: openfrag_capture::Process,
    F: Filesystem,
    C: Clock,
    M: MediaProbe,
{
    #[must_use]
    pub fn from_data_directory(
        data_directory: &Path,
        process: P,
        filesystem: F,
        clock: C,
        probe: M,
    ) -> Self {
        let configuration = match read_capture_configuration(data_directory) {
            Ok(config) => config,
            Err(error) => {
                return Self {
                    gate: Gate::Unavailable(unavailable_reason(error)),
                    available_from_ms: None,
                };
            }
        };
        if !configuration.enabled() {
            return Self {
                gate: Gate::Disabled,
                available_from_ms: None,
            };
        }
        let replay = ReplayConfig::sixty_second(
            OsString::from(configuration.capture_target()),
            configuration.output_directory().to_path_buf(),
        );
        let install = match configuration.recorder() {
            CaptureRecorder::Native(program) => RecorderInstall::Native {
                program: program.as_os_str().to_owned(),
            },
            CaptureRecorder::Flatpak => RecorderInstall::Flatpak {
                app_id: OsString::from(FLATPAK_APP_ID),
            },
        };
        let config = Config::new(
            replay_launch(install, replay),
            configuration.output_directory().to_path_buf(),
        );
        Self {
            gate: Gate::Ready(Supervisor::new(process, filesystem, clock, probe, config)),
            available_from_ms: None,
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
        self.start_at(0)
    }

    pub fn start_at(&mut self, available_from_ms: u64) -> Result<(), RuntimeError> {
        self.supervisor_mut()?
            .start()
            .map_err(RuntimeError::Supervisor)?;
        self.available_from_ms = Some(available_from_ms);
        Ok(())
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

impl<P, F, C, M> crate::live_runtime::CaptureRuntimePort for CaptureRuntime<P, F, C, M>
where
    P: openfrag_capture::Process + Send,
    F: Filesystem + Send,
    C: Clock + Send,
    M: MediaProbe + Send,
{
    fn readiness(&self) -> Result<(), String> {
        match self.status() {
            RuntimeStatus::Running if self.available_from_ms.is_some() => Ok(()),
            status => Err(format!("capture runtime is not running: {status:?}")),
        }
    }

    fn available_from_ms(&self) -> u64 {
        self.available_from_ms.unwrap_or(0)
    }

    fn request_save(&mut self, provenance: SaveProvenance) -> Result<SaveDisposition, String> {
        CaptureRuntime::request_save(self, provenance).map_err(|error| format!("{error:?}"))
    }
}

fn unavailable_reason(error: CaptureConfigError) -> UnavailableReason {
    match error {
        CaptureConfigError::Io(error) if error.kind() == std::io::ErrorKind::NotFound => {
            UnavailableReason::Missing
        }
        CaptureConfigError::InvalidValue(_)
        | CaptureConfigError::InvalidPath(_)
        | CaptureConfigError::InvalidFormat => UnavailableReason::Invalid(format!("{error:?}")),
        CaptureConfigError::UnsafePath(_)
        | CaptureConfigError::UnsafeTarget
        | CaptureConfigError::Io(_) => UnavailableReason::Unsafe(format!("{error:?}")),
    }
}
