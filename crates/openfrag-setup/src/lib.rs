//! Headless setup and compatibility contracts for the local openfrag application.

#![allow(clippy::missing_errors_doc)]

use serde::Serialize;
use std::{
    ffi::OsString,
    fs::{self, OpenOptions},
    io::Read,
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    sync::mpsc,
    time::{Duration, Instant},
};

const GSI_FILE: &str = "gamestate_integration_openfrag.cfg";
const TOKEN_FILE: &str = "gsi-token";
const LOCAL_STEAM_ID_FILE: &str = "local-steam-id";
const GSI_URI: &str = "http://127.0.0.1:7130/gsi/router";
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckId {
    DataDirectory,
    DemoImport,
    GsiConfig,
    Capture,
    ManualFlag,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Ready,
    Warning,
    Blocked,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Check {
    pub id: CheckId,
    pub status: CheckStatus,
    pub summary: String,
    pub action: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RecorderInstall {
    Native(PathBuf),
    Flatpak,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionKind {
    Wayland,
    X11,
    Other,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[allow(clippy::struct_excessive_bools)]
pub struct HostFacts {
    pub data_directory_writable: bool,
    pub cs2_cfg_directory: Option<PathBuf>,
    pub recorder: Option<RecorderInstall>,
    pub ffmpeg_available: bool,
    pub audio_source_available: bool,
    pub session: SessionKind,
    pub global_shortcuts_portal: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DoctorReport {
    pub checks: Vec<Check>,
}

impl DoctorReport {
    #[must_use]
    pub fn check(&self, id: CheckId) -> Option<&Check> {
        self.checks.iter().find(|check| check.id == id)
    }

    #[must_use]
    pub fn ready_for_demo_import(&self) -> bool {
        self.check(CheckId::DemoImport)
            .is_some_and(|check| check.status == CheckStatus::Ready)
    }
}

#[must_use]
pub fn evaluate(facts: &HostFacts) -> DoctorReport {
    let data = if facts.data_directory_writable {
        ready(CheckId::DataDirectory, "Local data directory is writable")
    } else {
        blocked(
            CheckId::DataDirectory,
            "Local data directory is not writable",
            "Choose a writable local data directory and run the Doctor again.",
        )
    };
    let demo = if facts.data_directory_writable {
        ready(CheckId::DemoImport, "Local Demo import is available")
    } else {
        blocked(
            CheckId::DemoImport,
            "Local Demo import needs writable storage",
            "Fix the local data directory; Steam sign-in and automatic download are not required.",
        )
    };
    let gsi = if !facts.data_directory_writable {
        blocked(
            CheckId::GsiConfig,
            "Game State Integration needs writable local storage",
            "Fix the local data directory before installing the GSI configuration.",
        )
    } else if facts.cs2_cfg_directory.is_some() {
        ready(CheckId::GsiConfig, "CS2 configuration directory was found")
    } else {
        blocked(
            CheckId::GsiConfig,
            "CS2 configuration directory was not found",
            "Choose the local CS2 game/csgo/cfg directory and run setup again.",
        )
    };
    let capture = capture_check(facts);
    let manual_flag = if capture.status == CheckStatus::Blocked {
        blocked(
            CheckId::ManualFlag,
            "Manual Flag needs a working replay buffer",
            "Fix gpu-screen-recorder capture before testing Manual Flag.",
        )
    } else if facts.global_shortcuts_portal {
        ready(CheckId::ManualFlag, "Global shortcut portal is available")
    } else {
        let session = match facts.session {
            SessionKind::Wayland => "Wayland",
            SessionKind::X11 => "X11",
            SessionKind::Other => "this desktop session",
        };
        blocked(
            CheckId::ManualFlag,
            "Global shortcut portal is unavailable",
            &format!(
                "Enable the XDG Global Shortcuts portal for {session}; openfrag will not install a privileged input hook."
            ),
        )
    };
    DoctorReport {
        checks: vec![data, demo, gsi, capture, manual_flag],
    }
}

fn capture_check(facts: &HostFacts) -> Check {
    if !facts.data_directory_writable {
        return blocked(
            CheckId::Capture,
            "Replay capture needs writable local storage",
            "Fix the local data directory before testing capture.",
        );
    }
    if facts.recorder.is_none() {
        return blocked(
            CheckId::Capture,
            "gpu-screen-recorder was not found",
            "Install gpu-screen-recorder from a trusted package source, then run the Doctor again.",
        );
    }
    let mut missing = Vec::new();
    if !facts.ffmpeg_available {
        missing.push("FFmpeg for trim and export");
    }
    if !facts.audio_source_available {
        missing.push("an audio source");
    }
    if missing.is_empty() {
        ready(
            CheckId::Capture,
            "Replay capture dependencies are available",
        )
    } else {
        Check {
            id: CheckId::Capture,
            status: CheckStatus::Warning,
            summary: "Replay capture is available with limitations".into(),
            action: format!(
                "Configure {} before the capture test.",
                missing.join(" and ")
            ),
        }
    }
}

fn ready(id: CheckId, summary: &str) -> Check {
    Check {
        id,
        status: CheckStatus::Ready,
        summary: summary.into(),
        action: String::new(),
    }
}

fn blocked(id: CheckId, summary: &str, action: &str) -> Check {
    Check {
        id,
        status: CheckStatus::Blocked,
        summary: summary.into(),
        action: action.into(),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GsiInstallation {
    pub config_path: PathBuf,
    pub token_path: PathBuf,
    pub local_steam_id_path: PathBuf,
}

#[derive(Debug)]
pub enum InstallError {
    InvalidToken,
    InvalidSteamId,
    InvalidDirectory(&'static str),
    UnsafeTarget(&'static str),
    Io(std::io::Error),
}

impl From<std::io::Error> for InstallError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

pub fn install_gsi(
    cs2_cfg_directory: &Path,
    data_directory: &Path,
    token: &str,
    local_steam_id: &str,
) -> Result<GsiInstallation, InstallError> {
    if token.len() < 8
        || token.len() > 256
        || !token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        return Err(InstallError::InvalidToken);
    }
    if local_steam_id.len() != 17
        || !local_steam_id.bytes().all(|byte| byte.is_ascii_digit())
        || local_steam_id.parse::<u64>().is_err()
    {
        return Err(InstallError::InvalidSteamId);
    }
    require_real_directory(cs2_cfg_directory, "CS2 cfg directory")?;
    require_real_directory(data_directory, "data directory")?;
    let config_path = cs2_cfg_directory.join(GSI_FILE);
    let token_path = data_directory.join(TOKEN_FILE);
    let local_steam_id_path = data_directory.join(LOCAL_STEAM_ID_FILE);
    require_safe_target(&config_path, "GSI config")?;
    require_safe_target(&token_path, "GSI token")?;
    require_safe_target(&local_steam_id_path, "local Steam ID")?;

    let config = gsi_config(token);
    atomic_write_private(&token_path, format!("{token}\n").as_bytes())?;
    atomic_write_private(
        &local_steam_id_path,
        format!("{local_steam_id}\n").as_bytes(),
    )?;
    atomic_write_private(&config_path, config.as_bytes())?;
    Ok(GsiInstallation {
        config_path,
        token_path,
        local_steam_id_path,
    })
}

fn gsi_config(token: &str) -> String {
    format!(
        "\"openfrag v1\"\n{{\n  \"uri\" \"{GSI_URI}\"\n  \"timeout\" \"5.0\"\n  \"buffer\" \"0.1\"\n  \"throttle\" \"0.1\"\n  \"heartbeat\" \"10.0\"\n  \"auth\" {{ \"token\" \"{token}\" }}\n  \"data\"\n  {{\n    \"provider\" \"1\"\n    \"map\" \"1\"\n    \"round\" \"1\"\n    \"player_id\" \"1\"\n    \"player_state\" \"1\"\n    \"player_match_stats\" \"1\"\n  }}\n}}\n"
    )
}

fn require_real_directory(path: &Path, name: &'static str) -> Result<(), InstallError> {
    let metadata = fs::symlink_metadata(path).map_err(InstallError::Io)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(InstallError::InvalidDirectory(name));
    }
    Ok(())
}

fn require_safe_target(path: &Path, name: &'static str) -> Result<(), InstallError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err(InstallError::UnsafeTarget(name))
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(InstallError::Io(error)),
    }
}

fn atomic_write_private(path: &Path, bytes: &[u8]) -> Result<(), InstallError> {
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = path.with_extension(format!("openfrag-{}-{sequence}.tmp", std::process::id()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        Ok::<(), std::io::Error>(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.map_err(InstallError::Io)
}

/// A diagnostic result produced by headless discovery, separate from the accepted setup API.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Discovery {
    Available,
    Missing(String),
    Unknown(String),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PortalBinding {
    Unapproved,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscoveredHost {
    pub data_directory: Discovery,
    pub cs2_cfg_candidates: Vec<PathBuf>,
    pub native_recorder: Discovery,
    pub flatpak_recorder: Discovery,
    pub ffmpeg: Discovery,
    pub audio: Discovery,
    pub session: SessionKind,
    pub portal_service: Discovery,
    pub portal_binding: PortalBinding,
}
/// Injected boundary for safe, headless host discovery.
pub trait HostProbe {
    fn discover(&self) -> DiscoveredHost;
}
pub trait ProbeEnvironment {
    fn var(&self, name: &str) -> Option<OsString>;
    fn home(&self) -> Option<PathBuf>;
}
pub trait ProbeFilesystem {
    fn exists(&self, path: &Path) -> bool;
    fn writable(&self, path: &Path) -> Result<bool, String>;
    fn executable(&self, name: &str) -> bool;
}
pub trait ProbeCommands {
    fn run(
        &self,
        program: &str,
        args: &[&str],
        limit: usize,
        timeout: Duration,
    ) -> Result<(bool, Vec<u8>, Vec<u8>), String>;
}
pub struct SystemProbe<E, F, C> {
    environment: E,
    filesystem: F,
    commands: C,
}
impl<E, F, C> SystemProbe<E, F, C> {
    #[must_use]
    pub fn new(environment: E, filesystem: F, commands: C) -> Self {
        Self {
            environment,
            filesystem,
            commands,
        }
    }
}
impl<E: ProbeEnvironment, F: ProbeFilesystem, C: ProbeCommands> HostProbe for SystemProbe<E, F, C> {
    fn discover(&self) -> DiscoveredHost {
        let home = self.environment.home();
        let data = self
            .environment
            .var("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| home.as_ref().map(|value| value.join(".local/share")));
        let data_directory = match data.as_deref() {
            Some(path) => self.filesystem.writable(path).map_or_else(
                |error| Discovery::Unknown(format!("cannot verify {}: {error}", path.display())),
                |ok| {
                    if ok {
                        Discovery::Available
                    } else {
                        Discovery::Missing(format!("{} is not writable", path.display()))
                    }
                },
            ),
            None => Discovery::Missing("HOME and XDG_DATA_HOME are unavailable".into()),
        };
        let mut cs2_cfg_candidates = Vec::new();
        if let Some(data) = data {
            cs2_cfg_candidates.push(
                data.join("Steam/steamapps/common/Counter-Strike Global Offensive/game/csgo/cfg"),
            );
        }
        if let Some(home) = home {
            cs2_cfg_candidates.push(home.join(
                ".steam/steam/steamapps/common/Counter-Strike Global Offensive/game/csgo/cfg",
            ));
            cs2_cfg_candidates.push(home.join(".var/app/com.valvesoftware.Steam/data/Steam/steamapps/common/Counter-Strike Global Offensive/game/csgo/cfg"));
        }
        cs2_cfg_candidates.retain(|path| self.filesystem.exists(path));
        cs2_cfg_candidates.sort();
        cs2_cfg_candidates.dedup();
        let native_recorder = available_executable(&self.filesystem, "gpu-screen-recorder");
        let ffmpeg = available_executable(&self.filesystem, "ffmpeg");
        let flatpak_recorder = command_discovery(
            &self.commands,
            "flatpak",
            &[
                "info",
                "--show-application",
                "com.dec05eba.gpu_screen_recorder",
            ],
            "gpu-screen-recorder Flatpak",
        );
        let audio = if matches!(
            self.commands
                .run("pw-cli", &["info", "0"], 8192, Duration::from_secs(1)),
            Ok((true, _, _))
        ) || matches!(
            self.commands
                .run("pactl", &["info"], 8192, Duration::from_secs(1)),
            Ok((true, _, _))
        ) {
            Discovery::Available
        } else {
            Discovery::Missing("no PipeWire or PulseAudio source is available".into())
        };
        let session = match self.environment.var("XDG_SESSION_TYPE").as_deref() {
            Some(value) if value == "wayland" => SessionKind::Wayland,
            Some(value) if value == "x11" => SessionKind::X11,
            _ => SessionKind::Other,
        };
        let portal_service = match self.commands.run(
            "busctl",
            &["--user", "--no-legend", "list"],
            65536,
            Duration::from_secs(1),
        ) {
            Ok((true, output, _))
                if String::from_utf8_lossy(&output).lines().any(|line| {
                    line.split_whitespace().next() == Some("org.freedesktop.portal.Desktop")
                }) =>
            {
                Discovery::Available
            }
            Ok((true, _, _)) => {
                Discovery::Missing("XDG GlobalShortcuts portal service is not registered".into())
            }
            Ok(_) => Discovery::Unknown("cannot list user D-Bus names without activation".into()),
            Err(error) => Discovery::Unknown(format!(
                "cannot inspect user D-Bus without activation: {error}"
            )),
        };
        DiscoveredHost {
            data_directory,
            cs2_cfg_candidates,
            native_recorder,
            flatpak_recorder,
            ffmpeg,
            audio,
            session,
            portal_service,
            portal_binding: PortalBinding::Unapproved,
        }
    }
}
impl From<&DiscoveredHost> for HostFacts {
    fn from(discovered: &DiscoveredHost) -> Self {
        Self {
            data_directory_writable: discovered.data_directory == Discovery::Available,
            cs2_cfg_directory: discovered.cs2_cfg_candidates.first().cloned(),
            recorder: if discovered.native_recorder == Discovery::Available {
                Some(RecorderInstall::Native(PathBuf::from(
                    "gpu-screen-recorder",
                )))
            } else if discovered.flatpak_recorder == Discovery::Available {
                Some(RecorderInstall::Flatpak)
            } else {
                None
            },
            ffmpeg_available: discovered.ffmpeg == Discovery::Available,
            audio_source_available: discovered.audio == Discovery::Available,
            session: discovered.session,
            global_shortcuts_portal: false,
        }
    }
}
fn available_executable(filesystem: &impl ProbeFilesystem, name: &str) -> Discovery {
    if filesystem.executable(name) {
        Discovery::Available
    } else {
        Discovery::Missing(format!("{name} is not available on PATH"))
    }
}
fn command_discovery(
    commands: &impl ProbeCommands,
    program: &str,
    args: &[&str],
    label: &str,
) -> Discovery {
    match commands.run(program, args, 8192, Duration::from_secs(1)) {
        Ok((true, _, _)) => Discovery::Available,
        Ok(_) => Discovery::Missing(format!("{label} is not installed")),
        Err(error) => Discovery::Unknown(format!("cannot inspect {label}: {error}")),
    }
}

#[derive(Default)]
pub struct StdProbeEnvironment;
impl ProbeEnvironment for StdProbeEnvironment {
    fn var(&self, name: &str) -> Option<OsString> {
        std::env::var_os(name)
    }
    fn home(&self) -> Option<PathBuf> {
        std::env::var_os("HOME").map(PathBuf::from)
    }
}
#[derive(Default)]
pub struct StdProbeFilesystem;
impl ProbeFilesystem for StdProbeFilesystem {
    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }
    fn writable(&self, path: &Path) -> Result<bool, String> {
        let metadata =
            fs::metadata(path).map_err(|error| format!("{}: {error}", path.display()))?;
        if !metadata.is_dir() {
            return Ok(false);
        }
        let probe = path.join(format!(".openfrag-probe-{}", std::process::id()));
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&probe)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        fs::remove_file(probe).map_err(|error| error.to_string())?;
        Ok(true)
    }
    fn executable(&self, name: &str) -> bool {
        std::env::var_os("PATH").is_some_and(|paths| {
            std::env::split_paths(&paths).any(|path| path.join(name).is_file())
        })
    }
}
#[derive(Default)]
pub struct StdProbeCommands;
impl ProbeCommands for StdProbeCommands {
    fn run(
        &self,
        program: &str,
        args: &[&str],
        limit: usize,
        timeout: Duration,
    ) -> Result<(bool, Vec<u8>, Vec<u8>), String> {
        let mut child = std::process::Command::new(program)
            .args(args)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|error| error.to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "stdout unavailable".to_owned())?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| "stderr unavailable".to_owned())?;
        let (sender, receiver) = mpsc::channel();
        let stdout_sender = sender.clone();
        std::thread::spawn(move || {
            let _ = stdout_sender.send((
                "stdout",
                bounded_read(stdout, limit).map_err(|error| format!("stdout: {error}")),
            ));
        });
        let stderr_sender = sender.clone();
        std::thread::spawn(move || {
            let _ = stderr_sender.send((
                "stderr",
                bounded_read(stderr, limit).map_err(|error| format!("stderr: {error}")),
            ));
        });
        drop(sender);
        let deadline = Instant::now() + timeout;
        while child
            .try_wait()
            .map_err(|error| error.to_string())?
            .is_none()
        {
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{program} timed out"));
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let mut out = Vec::new();
        let mut err = Vec::new();
        for _ in 0..2 {
            let (name, bytes) = receiver
                .recv_timeout(Duration::from_secs(1))
                .map_err(|_| format!("{program} pipe drain timed out"))?;
            let bytes = bytes?;
            if name == "stdout" {
                out = bytes;
            } else {
                err = bytes;
            }
        }
        let status = child.wait().map_err(|error| error.to_string())?;
        Ok((status.success(), out, err))
    }
}
fn bounded_read(reader: impl Read, limit: usize) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::with_capacity(limit.min(8192));
    reader
        .take(u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() > limit {
        return Err(format!("output exceeded {limit} bytes"));
    }
    Ok(bytes)
}

#[cfg(test)]
mod discovery_tests {
    use super::*;
    struct Env;
    impl ProbeEnvironment for Env {
        fn var(&self, _: &str) -> Option<OsString> {
            None
        }
        fn home(&self) -> Option<PathBuf> {
            Some(PathBuf::from("/home/test"))
        }
    }
    struct Fs;
    impl ProbeFilesystem for Fs {
        fn exists(&self, _: &Path) -> bool {
            false
        }
        fn writable(&self, _: &Path) -> Result<bool, String> {
            Err("NotFound".into())
        }
        fn executable(&self, _: &str) -> bool {
            false
        }
    }
    struct Commands;
    impl ProbeCommands for Commands {
        fn run(
            &self,
            _: &str,
            _: &[&str],
            _: usize,
            _: Duration,
        ) -> Result<(bool, Vec<u8>, Vec<u8>), String> {
            Ok((false, vec![], vec![]))
        }
    }
    #[test]
    fn missing_candidates_and_unapproved_portal_do_not_become_ready_facts() {
        let discovered = SystemProbe::new(Env, Fs, Commands).discover();
        assert!(discovered.cs2_cfg_candidates.is_empty());
        assert!(matches!(discovered.data_directory, Discovery::Unknown(_)));
        let facts = HostFacts::from(&discovered);
        assert!(facts.cs2_cfg_directory.is_none());
        assert!(!facts.global_shortcuts_portal);
    }
    #[test]
    fn bounded_read_rejects_stdout_and_stderr_sized_buffers() {
        assert!(bounded_read(&b"123"[..], 2).is_err());
        assert!(bounded_read(&b"456"[..], 2).is_err());
    }
}
