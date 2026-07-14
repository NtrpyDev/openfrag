//! Headless setup and compatibility contracts for the local openfrag application.

#![allow(clippy::missing_errors_doc)]

use serde::Serialize;
use std::{
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
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
