//! Private, validated persistence for the replay capture configuration.

use std::{
    ffi::{OsStr, OsString},
    fs::{self, OpenOptions},
    io::Write,
    os::unix::{
        ffi::{OsStrExt, OsStringExt},
        fs::{OpenOptionsExt, PermissionsExt},
    },
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

const CONFIG_FILE: &str = "capture.conf";
const FORMAT_HEADER: &str = "OPENFRAG_CAPTURE_V1";
const REPLAY_SECONDS: u16 = 60;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CaptureRecorder {
    Native(PathBuf),
    Flatpak,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CaptureConfiguration {
    enabled: bool,
    recorder: CaptureRecorder,
    capture_target: String,
    output_directory: PathBuf,
    ffprobe_path: PathBuf,
}

impl CaptureConfiguration {
    pub fn new(
        enabled: bool,
        recorder: CaptureRecorder,
        capture_target: impl Into<String>,
        output_directory: PathBuf,
        ffprobe_path: PathBuf,
    ) -> Result<Self, CaptureConfigError> {
        let configuration = Self {
            enabled,
            recorder,
            capture_target: capture_target.into(),
            output_directory,
            ffprobe_path,
        };
        configuration.validate()?;
        Ok(configuration)
    }

    #[must_use]
    pub const fn enabled(&self) -> bool {
        self.enabled
    }

    #[must_use]
    pub const fn recorder(&self) -> &CaptureRecorder {
        &self.recorder
    }

    #[must_use]
    pub fn capture_target(&self) -> &str {
        &self.capture_target
    }

    #[must_use]
    pub fn output_directory(&self) -> &Path {
        &self.output_directory
    }

    #[must_use]
    pub const fn replay_seconds(&self) -> u16 {
        REPLAY_SECONDS
    }

    #[must_use]
    pub fn ffprobe_path(&self) -> &Path {
        &self.ffprobe_path
    }

    fn validate(&self) -> Result<(), CaptureConfigError> {
        validate_capture_target(&self.capture_target)?;
        require_real_directory(&self.output_directory, "capture output directory")?;
        require_executable(&self.ffprobe_path, "ffprobe executable")?;
        if let CaptureRecorder::Native(path) = &self.recorder {
            require_executable(path, "gpu-screen-recorder executable")?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CaptureConfigInstallation {
    pub path: PathBuf,
}

#[derive(Debug)]
pub enum CaptureConfigError {
    InvalidValue(&'static str),
    InvalidPath(&'static str),
    UnsafePath(&'static str),
    UnsafeTarget,
    InvalidFormat,
    Io(std::io::Error),
}

impl From<std::io::Error> for CaptureConfigError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

pub fn write_capture_configuration(
    data_directory: &Path,
    configuration: &CaptureConfiguration,
) -> Result<CaptureConfigInstallation, CaptureConfigError> {
    require_real_directory(data_directory, "data directory")?;
    configuration.validate()?;
    let path = data_directory.join(CONFIG_FILE);
    require_safe_target(&path)?;
    atomic_write_private(&path, &encode(configuration))?;
    Ok(CaptureConfigInstallation { path })
}

pub fn read_capture_configuration(
    data_directory: &Path,
) -> Result<CaptureConfiguration, CaptureConfigError> {
    require_real_directory(data_directory, "data directory")?;
    let path = data_directory.join(CONFIG_FILE);
    require_existing_safe_file(&path)?;
    decode(&fs::read(path)?)
}

fn validate_capture_target(value: &str) -> Result<(), CaptureConfigError> {
    let mut bytes = value.bytes();
    if value.len() > 128
        || !bytes
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
    {
        return Err(CaptureConfigError::InvalidValue("capture target"));
    }
    if !bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':'))
    {
        return Err(CaptureConfigError::InvalidValue("capture target"));
    }
    Ok(())
}

fn require_real_directory(path: &Path, name: &'static str) -> Result<(), CaptureConfigError> {
    require_absolute_clean_path(path, name)?;
    require_no_symlink_components(path, name)?;
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() {
        return Err(CaptureConfigError::InvalidPath(name));
    }
    Ok(())
}

fn require_executable(path: &Path, name: &'static str) -> Result<(), CaptureConfigError> {
    require_absolute_clean_path(path, name)?;
    require_no_symlink_components(path, name)?;
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
        return Err(CaptureConfigError::InvalidPath(name));
    }
    Ok(())
}

fn require_absolute_clean_path(path: &Path, name: &'static str) -> Result<(), CaptureConfigError> {
    if !path.is_absolute()
        || path.as_os_str().as_bytes().len() > 4096
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
        || path.as_os_str().as_bytes().iter().any(u8::is_ascii_control)
    {
        return Err(CaptureConfigError::UnsafePath(name));
    }
    Ok(())
}

fn require_no_symlink_components(
    path: &Path,
    name: &'static str,
) -> Result<(), CaptureConfigError> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        if fs::symlink_metadata(&current)?.file_type().is_symlink() {
            return Err(CaptureConfigError::UnsafePath(name));
        }
    }
    Ok(())
}

fn require_safe_target(path: &Path) -> Result<(), CaptureConfigError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err(CaptureConfigError::UnsafeTarget)
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(CaptureConfigError::Io(error)),
    }
}

fn require_existing_safe_file(path: &Path) -> Result<(), CaptureConfigError> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(CaptureConfigError::UnsafeTarget);
    }
    Ok(())
}

fn encode(configuration: &CaptureConfiguration) -> Vec<u8> {
    let (recorder, recorder_path) = match &configuration.recorder {
        CaptureRecorder::Native(path) => ("native", encode_os(path.as_os_str())),
        CaptureRecorder::Flatpak => ("flatpak", String::new()),
    };
    format!(
        "{FORMAT_HEADER}\nenabled={}\nrecorder={recorder}\nrecorder_path={recorder_path}\ncapture_target={}\noutput_directory={}\nreplay_seconds={REPLAY_SECONDS}\nffprobe_path={}\n",
        if configuration.enabled { "true" } else { "false" },
        configuration.capture_target,
        encode_os(configuration.output_directory.as_os_str()),
        encode_os(configuration.ffprobe_path.as_os_str()),
    )
    .into_bytes()
}

fn decode(bytes: &[u8]) -> Result<CaptureConfiguration, CaptureConfigError> {
    let text = std::str::from_utf8(bytes).map_err(|_| CaptureConfigError::InvalidFormat)?;
    let mut lines = text.lines();
    if lines.next() != Some(FORMAT_HEADER) {
        return Err(CaptureConfigError::InvalidFormat);
    }
    let enabled = match field(&mut lines, "enabled")? {
        "true" => true,
        "false" => false,
        _ => return Err(CaptureConfigError::InvalidFormat),
    };
    let recorder_kind = field(&mut lines, "recorder")?;
    let recorder_path = field(&mut lines, "recorder_path")?;
    let capture_target = field(&mut lines, "capture_target")?.to_owned();
    let output_directory = decode_path(field(&mut lines, "output_directory")?)?;
    if field(&mut lines, "replay_seconds")? != REPLAY_SECONDS.to_string() {
        return Err(CaptureConfigError::InvalidFormat);
    }
    let ffprobe_path = decode_path(field(&mut lines, "ffprobe_path")?)?;
    if lines.next().is_some() {
        return Err(CaptureConfigError::InvalidFormat);
    }
    let recorder = match (recorder_kind, recorder_path.is_empty()) {
        ("flatpak", true) => CaptureRecorder::Flatpak,
        ("native", false) => CaptureRecorder::Native(decode_path(recorder_path)?),
        _ => return Err(CaptureConfigError::InvalidFormat),
    };
    CaptureConfiguration::new(
        enabled,
        recorder,
        capture_target,
        output_directory,
        ffprobe_path,
    )
}

fn field<'a>(
    lines: &mut impl Iterator<Item = &'a str>,
    expected: &str,
) -> Result<&'a str, CaptureConfigError> {
    let line = lines.next().ok_or(CaptureConfigError::InvalidFormat)?;
    line.strip_prefix(expected)
        .and_then(|value| value.strip_prefix('='))
        .ok_or(CaptureConfigError::InvalidFormat)
}

fn encode_os(value: &OsStr) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(value.as_bytes().len() * 2);
    for byte in value.as_bytes() {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn decode_path(value: &str) -> Result<PathBuf, CaptureConfigError> {
    if !value.len().is_multiple_of(2) || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(CaptureConfigError::InvalidFormat);
    }
    let bytes = value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = hex_value(pair[0])?;
            let low = hex_value(pair[1])?;
            Some((high << 4) | low)
        })
        .collect::<Option<Vec<_>>>()
        .ok_or(CaptureConfigError::InvalidFormat)?;
    Ok(PathBuf::from(OsString::from_vec(bytes)))
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn atomic_write_private(path: &Path, bytes: &[u8]) -> Result<(), CaptureConfigError> {
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = path.with_extension(format!(
        "openfrag-capture-{}-{sequence}.tmp",
        std::process::id()
    ));
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
    result.map_err(CaptureConfigError::Io)
}
