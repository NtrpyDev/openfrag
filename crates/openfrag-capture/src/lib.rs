//! A testable supervisor for a `gpu-screen-recorder` capture process.
#![allow(clippy::missing_errors_doc, clippy::struct_excessive_bools)]

use std::collections::VecDeque;
use std::ffi::OsString;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// How the recorder is launched. Arguments are always passed directly, never to a shell.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Launch {
    Native {
        program: OsString,
        args: Vec<OsString>,
    },
    Flatpak {
        app_id: OsString,
        args: Vec<OsString>,
    },
}

impl Launch {
    #[must_use]
    pub fn argv(&self) -> Vec<OsString> {
        match self {
            Self::Native { program, args } => std::iter::once(program.clone())
                .chain(args.clone())
                .collect(),
            Self::Flatpak { app_id, args } => [
                OsString::from("flatpak"),
                OsString::from("run"),
                app_id.clone(),
            ]
            .into_iter()
            .chain(args.clone())
            .collect(),
        }
    }
}

/// Installation choice for the separately installed replay recorder.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RecorderInstall {
    Native { program: OsString },
    Flatpak { app_id: OsString },
}

/// Direct arguments for a replay-buffer recorder invocation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReplayConfig {
    pub capture_target: OsString,
    pub output_directory: PathBuf,
    pub video_codec: OsString,
    pub audio_codec: OsString,
    pub buffer_seconds: u32,
}

impl ReplayConfig {
    #[must_use]
    pub fn sixty_second(capture_target: OsString, output_directory: PathBuf) -> Self {
        Self {
            capture_target,
            output_directory,
            video_codec: "h264".into(),
            audio_codec: "aac".into(),
            buffer_seconds: 60,
        }
    }
}

/// Builds a shell-free recorder command. No path or argument is interpreted by a shell.
#[must_use]
pub fn replay_launch(install: RecorderInstall, config: ReplayConfig) -> Launch {
    let args = vec![
        "-w".into(),
        config.capture_target,
        "-r".into(),
        config.buffer_seconds.to_string().into(),
        "-c".into(),
        config.video_codec,
        "-ac".into(),
        config.audio_codec,
        "-o".into(),
        config.output_directory.into_os_string(),
    ];
    match install {
        RecorderInstall::Native { program } => Launch::Native { program, args },
        RecorderInstall::Flatpak { app_id } => Launch::Flatpak { app_id, args },
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SaveProvenance {
    ManualFlag,
    AutoRoundEnd,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Signal {
    User1,
    Interrupt,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MediaInfo {
    pub duration_ms: u64,
    pub video_streams: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SaveAcknowledgement {
    pub path: PathBuf,
    pub recorder_request_id: String,
    pub provenance: SaveProvenance,
    pub requested_at_ms: u64,
    pub media: MediaInfo,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SaveDisposition {
    Signalled,
    Coalesced,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SaveRequestOutcome {
    pub recorder_request_id: String,
    pub disposition: SaveDisposition,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Error {
    Process(String),
    Filesystem(String),
    Media(String),
    NotRunning,
    NoSaveInFlight,
    UnsafeOutputPath,
    SymlinkOutput,
    InvalidOutput,
    OutputNotFound,
}

pub trait Process {
    fn spawn(&mut self, argv: &[OsString]) -> Result<(), String>;
    fn try_wait(&mut self) -> Result<Option<i32>, String>;
    fn signal(&mut self, signal: Signal) -> Result<(), String>;
    /// Returns stderr bytes accumulated since the previous call.
    fn drain_stderr(&mut self) -> Result<Vec<u8>, String>;
}

pub trait Filesystem {
    fn outputs_since(&self, directory: &Path, since_ms: u64) -> Result<Vec<PathBuf>, String>;
    fn is_symlink(&self, path: &Path) -> Result<bool, String>;
    fn is_regular_file(&self, path: &Path) -> Result<bool, String>;
    fn is_contained(&self, directory: &Path, path: &Path) -> Result<bool, String>;
}

pub trait Clock {
    fn now_ms(&self) -> u64;
    fn wall_ms(&self) -> u64 {
        self.now_ms()
    }
}
pub trait MediaProbe {
    fn verify(&self, path: &Path) -> Result<MediaInfo, String>;
}

pub struct Config {
    pub launch: Launch,
    pub output_directory: PathBuf,
    pub max_stderr_bytes: usize,
    pub stable_after: Duration,
}

impl Config {
    #[must_use]
    pub fn new(launch: Launch, output_directory: PathBuf) -> Self {
        Self {
            launch,
            output_directory,
            max_stderr_bytes: 16 * 1024,
            stable_after: Duration::from_secs(30),
        }
    }
}

struct PendingSave {
    recorder_request_id: String,
    provenance: SaveProvenance,
    since_wall_ms: u64,
    requested_at_ms: u64,
}

/// Coordinates one recorder child and one durable save acknowledgement at a time.
pub struct Supervisor<P, F, C, M> {
    process: P,
    filesystem: F,
    clock: C,
    probe: M,
    config: Config,
    running: bool,
    shutting_down: bool,
    started_ms: u64,
    stable: bool,
    in_flight: Option<PendingSave>,
    queued: Option<PendingSave>,
    next_restart_ms: Option<u64>,
    failures: usize,
    stderr: VecDeque<u8>,
}

impl<P: Process, F: Filesystem, C: Clock, M: MediaProbe> Supervisor<P, F, C, M> {
    #[must_use]
    pub fn new(process: P, filesystem: F, clock: C, probe: M, config: Config) -> Self {
        Self {
            process,
            filesystem,
            clock,
            probe,
            config,
            running: false,
            shutting_down: false,
            started_ms: 0,
            stable: false,
            in_flight: None,
            queued: None,
            next_restart_ms: None,
            failures: 0,
            stderr: VecDeque::new(),
        }
    }

    pub fn start(&mut self) -> Result<(), Error> {
        self.spawn()
    }
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.running
    }
    #[must_use]
    pub fn next_restart_ms(&self) -> Option<u64> {
        self.next_restart_ms
    }
    #[must_use]
    pub fn has_save_in_flight(&self) -> bool {
        self.in_flight.is_some()
    }
    #[must_use]
    pub fn stderr_tail(&self) -> Vec<u8> {
        self.stderr.iter().copied().collect()
    }

    pub fn request_save(
        &mut self,
        recorder_request_id: &str,
        provenance: SaveProvenance,
    ) -> Result<SaveRequestOutcome, Error> {
        if !self.running {
            return Err(Error::NotRunning);
        }
        if self.in_flight.is_some() {
            let queued = self.queued.get_or_insert_with(|| PendingSave {
                recorder_request_id: recorder_request_id.to_owned(),
                provenance,
                since_wall_ms: 0,
                requested_at_ms: 0,
            });
            if provenance == SaveProvenance::ManualFlag {
                queued.provenance = SaveProvenance::ManualFlag;
            }
            return Ok(SaveRequestOutcome {
                recorder_request_id: queued.recorder_request_id.clone(),
                disposition: SaveDisposition::Coalesced,
            });
        }
        self.signal_save(recorder_request_id.to_owned(), provenance)?;
        Ok(SaveRequestOutcome {
            recorder_request_id: recorder_request_id.to_owned(),
            disposition: SaveDisposition::Signalled,
        })
    }

    fn signal_save(
        &mut self,
        recorder_request_id: String,
        provenance: SaveProvenance,
    ) -> Result<(), Error> {
        self.process.signal(Signal::User1).map_err(Error::Process)?;
        self.in_flight = Some(PendingSave {
            recorder_request_id,
            provenance,
            since_wall_ms: self.clock.wall_ms(),
            requested_at_ms: self.clock.now_ms(),
        });
        Ok(())
    }

    /// Discover and verify a newly emitted file before acknowledging the save.
    pub fn discover_save(&mut self) -> Result<Option<SaveAcknowledgement>, Error> {
        let Some(in_flight) = &self.in_flight else {
            return Err(Error::NoSaveInFlight);
        };
        let candidates = self
            .filesystem
            .outputs_since(&self.config.output_directory, in_flight.since_wall_ms)
            .map_err(Error::Filesystem)?;
        let Some(path) = candidates.into_iter().next() else {
            return Ok(None);
        };
        self.acknowledge(path).map(Some)
    }

    pub fn acknowledge(&mut self, path: PathBuf) -> Result<SaveAcknowledgement, Error> {
        let pending = self.in_flight.as_ref().ok_or(Error::NoSaveInFlight)?;
        self.validate_output(&path)?;
        let media = self.probe.verify(&path).map_err(Error::Media)?;
        let acknowledgement = SaveAcknowledgement {
            path,
            recorder_request_id: pending.recorder_request_id.clone(),
            provenance: pending.provenance,
            requested_at_ms: pending.requested_at_ms,
            media,
        };
        self.in_flight = None;
        if let Some(next) = self.queued.take() {
            self.signal_save(next.recorder_request_id, next.provenance)?;
        }
        Ok(acknowledgement)
    }

    /// Polls exit/stderr state and starts a due restart. Returns the detected exit code.
    pub fn poll(&mut self) -> Result<Option<i32>, Error> {
        self.append_stderr()?;
        if self.running {
            if !self.stable
                && self.clock.now_ms().saturating_sub(self.started_ms)
                    >= duration_ms(self.config.stable_after)
            {
                self.failures = 0;
                self.stable = true;
            }
            if let Some(code) = self.process.try_wait().map_err(Error::Process)? {
                self.running = false;
                if !self.shutting_down {
                    let delay = restart_delay(self.failures);
                    self.failures = self.failures.saturating_add(1);
                    self.next_restart_ms =
                        Some(self.clock.now_ms().saturating_add(duration_ms(delay)));
                }
                return Ok(Some(code));
            }
        }
        if !self.shutting_down
            && !self.running
            && self
                .next_restart_ms
                .is_some_and(|at| self.clock.now_ms() >= at)
        {
            self.spawn()?;
        }
        Ok(None)
    }

    pub fn shutdown(&mut self) -> Result<(), Error> {
        self.shutting_down = true;
        self.next_restart_ms = None;
        if self.running {
            self.process
                .signal(Signal::Interrupt)
                .map_err(Error::Process)?;
        }
        Ok(())
    }

    fn spawn(&mut self) -> Result<(), Error> {
        self.process
            .spawn(&self.config.launch.argv())
            .map_err(Error::Process)?;
        self.running = true;
        self.shutting_down = false;
        self.stable = false;
        self.started_ms = self.clock.now_ms();
        self.next_restart_ms = None;
        Ok(())
    }
    fn append_stderr(&mut self) -> Result<(), Error> {
        for byte in self.process.drain_stderr().map_err(Error::Process)? {
            self.stderr.push_back(byte);
            if self.stderr.len() > self.config.max_stderr_bytes {
                self.stderr.pop_front();
            }
        }
        Ok(())
    }
    fn validate_output(&self, path: &Path) -> Result<(), Error> {
        if !path.starts_with(&self.config.output_directory)
            || path.components().any(|c| matches!(c, Component::ParentDir))
        {
            return Err(Error::UnsafeOutputPath);
        }
        if self
            .filesystem
            .is_symlink(path)
            .map_err(Error::Filesystem)?
        {
            return Err(Error::SymlinkOutput);
        }
        if !self
            .filesystem
            .is_regular_file(path)
            .map_err(Error::Filesystem)?
        {
            return Err(Error::InvalidOutput);
        }
        if !self
            .filesystem
            .is_contained(&self.config.output_directory, path)
            .map_err(Error::Filesystem)?
        {
            return Err(Error::UnsafeOutputPath);
        }
        Ok(())
    }
}

#[must_use]
pub fn restart_delay(failures: usize) -> Duration {
    Duration::from_secs([1, 2, 4, 8, 16, 30][failures.min(5)])
}

fn duration_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// A monotonic clock with an epoch-based projection for filesystem timestamps.
pub struct StdClock {
    started: Instant,
    started_wall_ms: u64,
}
impl Default for StdClock {
    fn default() -> Self {
        Self {
            started: Instant::now(),
            started_wall_ms: system_time_ms(SystemTime::now()).unwrap_or(0),
        }
    }
}
impl Clock for StdClock {
    fn now_ms(&self) -> u64 {
        duration_ms(self.started.elapsed())
    }
    fn wall_ms(&self) -> u64 {
        self.started_wall_ms.saturating_add(self.now_ms())
    }
}

/// A concrete filesystem adapter that rejects symlinks and escapes from its output directory.
#[derive(Default)]
pub struct StdFilesystem;
impl Filesystem for StdFilesystem {
    fn outputs_since(&self, directory: &Path, since_ms: u64) -> Result<Vec<PathBuf>, String> {
        let mut outputs = std::fs::read_dir(directory)
            .map_err(|error| {
                format!(
                    "cannot read output directory {}: {error}",
                    directory.display()
                )
            })?
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let path = entry.path();
                let metadata = std::fs::symlink_metadata(&path).ok()?;
                if metadata.file_type().is_symlink() || !metadata.is_file() {
                    return None;
                }
                let modified_ms = system_time_ms(metadata.modified().ok()?)?;
                (modified_ms >= since_ms).then_some((modified_ms, path))
            })
            .collect::<Vec<_>>();
        outputs.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
        Ok(outputs.into_iter().map(|(_, path)| path).collect())
    }
    fn is_symlink(&self, path: &Path) -> Result<bool, String> {
        Ok(std::fs::symlink_metadata(path)
            .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?
            .file_type()
            .is_symlink())
    }
    fn is_regular_file(&self, path: &Path) -> Result<bool, String> {
        Ok(std::fs::symlink_metadata(path)
            .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?
            .is_file())
    }
    fn is_contained(&self, directory: &Path, path: &Path) -> Result<bool, String> {
        let root = std::fs::canonicalize(directory).map_err(|error| {
            format!(
                "cannot canonicalize output directory {}: {error}",
                directory.display()
            )
        })?;
        let candidate = std::fs::canonicalize(path)
            .map_err(|error| format!("cannot canonicalize output {}: {error}", path.display()))?;
        Ok(candidate.starts_with(root))
    }
}

fn system_time_ms(time: SystemTime) -> Option<u64> {
    u64::try_from(time.duration_since(UNIX_EPOCH).ok()?.as_millis()).ok()
}

/// A bounded, direct-argv ffprobe adapter for validating saved media.
pub struct FfprobeMediaProbe {
    pub program: PathBuf,
    pub timeout: Duration,
    pub max_output_bytes: usize,
}
impl Default for FfprobeMediaProbe {
    fn default() -> Self {
        Self {
            program: "ffprobe".into(),
            timeout: Duration::from_secs(2),
            max_output_bytes: 64 * 1024,
        }
    }
}
impl MediaProbe for FfprobeMediaProbe {
    fn verify(&self, path: &Path) -> Result<MediaInfo, String> {
        let mut child = std::process::Command::new(&self.program)
            .args([
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_entries",
                "stream=codec_type:format=duration",
                "-of",
                "json",
            ])
            .arg(path)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|error| format!("cannot start ffprobe {}: {error}", self.program.display()))?;
        let deadline = Instant::now() + self.timeout;
        loop {
            if child
                .try_wait()
                .map_err(|error| format!("cannot poll ffprobe: {error}"))?
                .is_some()
            {
                break;
            }
            if Instant::now() >= deadline {
                child
                    .kill()
                    .map_err(|error| format!("cannot stop timed-out ffprobe: {error}"))?;
                let _ = child.wait();
                return Err(format!("ffprobe exceeded {} ms", duration_ms(self.timeout)));
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let stdout = read_limited(
            child
                .stdout
                .take()
                .ok_or_else(|| "ffprobe stdout was not piped".to_owned())?,
            self.max_output_bytes,
            "stdout",
        )?;
        let stderr = read_limited(
            child
                .stderr
                .take()
                .ok_or_else(|| "ffprobe stderr was not piped".to_owned())?,
            self.max_output_bytes,
            "stderr",
        )?;
        let status = child
            .wait()
            .map_err(|error| format!("cannot reap ffprobe: {error}"))?;
        if !status.success() {
            return Err(format!(
                "ffprobe failed with {status}: {}",
                String::from_utf8_lossy(&stderr)
            ));
        }
        parse_probe_json(&stdout)
    }
}

fn read_limited(reader: impl Read, limit: usize, name: &str) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::with_capacity(limit.min(8192));
    reader
        .take(u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| format!("cannot read ffprobe {name}: {error}"))?;
    if bytes.len() > limit {
        return Err(format!("ffprobe {name} exceeded {limit} bytes"));
    }
    Ok(bytes)
}

fn parse_probe_json(bytes: &[u8]) -> Result<MediaInfo, String> {
    let value: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|error| format!("ffprobe returned invalid JSON: {error}"))?;
    let streams = value
        .get("streams")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "ffprobe JSON lacks streams array".to_owned())?;
    if streams.len() != 1
        || streams[0]
            .get("codec_type")
            .and_then(serde_json::Value::as_str)
            != Some("video")
    {
        return Err("ffprobe JSON must contain exactly one video stream".to_owned());
    }
    let duration = value
        .get("format")
        .and_then(|format| format.get("duration"))
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "ffprobe JSON lacks a string duration".to_owned())?;
    let duration_ms = parse_duration_ms(duration)?;
    Ok(MediaInfo {
        duration_ms: duration_ms as u64,
        video_streams: 1,
    })
}

fn parse_duration_ms(duration: &str) -> Result<u64, String> {
    let (whole, fraction) = duration.split_once('.').unwrap_or((duration, ""));
    if whole.is_empty()
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err("ffprobe duration must be an unsigned decimal".to_owned());
    }
    let seconds = whole
        .parse::<u64>()
        .map_err(|_| "ffprobe duration is too large".to_owned())?;
    let mut milliseconds = 0_u64;
    for (index, byte) in fraction.bytes().take(3).enumerate() {
        milliseconds = milliseconds.saturating_add(u64::from(byte - b'0') * [100, 10, 1][index]);
    }
    let result = seconds
        .checked_mul(1000)
        .and_then(|value| value.checked_add(milliseconds))
        .ok_or_else(|| "ffprobe duration is too large".to_owned())?;
    if result == 0 {
        return Err("ffprobe duration must be positive".to_owned());
    }
    Ok(result)
}

/// A concrete adapter using `std::process::Command`; it does not invoke a shell.
#[derive(Default)]
pub struct StdProcess {
    child: Option<std::process::Child>,
    stderr: Option<Receiver<Vec<u8>>>,
}
impl Process for StdProcess {
    fn spawn(&mut self, argv: &[OsString]) -> Result<(), String> {
        let (program, command_args) = argv.split_first().ok_or_else(|| "empty argv".to_owned())?;
        let mut child = std::process::Command::new(program)
            .args(command_args)
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| e.to_string())?;
        let (sender, receiver) = mpsc::channel();
        if let Some(mut stderr) = child.stderr.take() {
            std::thread::spawn(move || {
                loop {
                    let mut chunk = [0; 4096];
                    match stderr.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(length) if sender.send(chunk[..length].to_vec()).is_err() => break,
                        Ok(_) => {}
                    }
                }
            });
        }
        self.child = Some(child);
        self.stderr = Some(receiver);
        Ok(())
    }
    fn try_wait(&mut self) -> Result<Option<i32>, String> {
        let Some(child) = &mut self.child else {
            return Ok(None);
        };
        child
            .try_wait()
            .map(|status| status.map(|s| s.code().unwrap_or(-1)))
            .map_err(|e| e.to_string())
    }
    fn signal(&mut self, signal: Signal) -> Result<(), String> {
        let child = self.child.as_ref().ok_or_else(|| "no child".to_owned())?;
        let signal = match signal {
            Signal::User1 => nix::sys::signal::Signal::SIGUSR1,
            Signal::Interrupt => nix::sys::signal::Signal::SIGINT,
        };
        let pid = i32::try_from(child.id()).map_err(|_| "child pid does not fit i32".to_owned())?;
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), signal).map_err(|e| e.to_string())
    }
    fn drain_stderr(&mut self) -> Result<Vec<u8>, String> {
        Ok(self
            .stderr
            .as_ref()
            .map_or_else(Vec::new, |receiver| receiver.try_iter().flatten().collect()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[derive(Default)]
    struct FakeProcess {
        spawns: Vec<Vec<OsString>>,
        signals: Vec<Signal>,
        exits: Vec<Option<i32>>,
        stderr: Vec<u8>,
    }
    impl Process for FakeProcess {
        fn spawn(&mut self, argv: &[OsString]) -> Result<(), String> {
            self.spawns.push(argv.to_vec());
            Ok(())
        }
        fn try_wait(&mut self) -> Result<Option<i32>, String> {
            Ok(if self.exits.is_empty() {
                None
            } else {
                self.exits.remove(0)
            })
        }
        fn signal(&mut self, signal: Signal) -> Result<(), String> {
            self.signals.push(signal);
            Ok(())
        }
        fn drain_stderr(&mut self) -> Result<Vec<u8>, String> {
            Ok(std::mem::take(&mut self.stderr))
        }
    }
    #[derive(Default)]
    struct FakeFs {
        outputs: Vec<PathBuf>,
        symlink: bool,
        regular: bool,
    }
    impl Filesystem for FakeFs {
        fn outputs_since(&self, _: &Path, _: u64) -> Result<Vec<PathBuf>, String> {
            Ok(self.outputs.clone())
        }
        fn is_symlink(&self, _: &Path) -> Result<bool, String> {
            Ok(self.symlink)
        }
        fn is_regular_file(&self, _: &Path) -> Result<bool, String> {
            Ok(self.regular)
        }
        fn is_contained(&self, _: &Path, _: &Path) -> Result<bool, String> {
            Ok(true)
        }
    }
    struct FakeClock(Cell<u64>);
    impl Clock for FakeClock {
        fn now_ms(&self) -> u64 {
            self.0.get()
        }
    }
    struct FakeProbe;
    impl MediaProbe for FakeProbe {
        fn verify(&self, _: &Path) -> Result<MediaInfo, String> {
            Ok(MediaInfo {
                duration_ms: 100,
                video_streams: 1,
            })
        }
    }
    fn supervisor(clock: u64) -> Supervisor<FakeProcess, FakeFs, FakeClock, FakeProbe> {
        let mut config = Config::new(
            Launch::Native {
                program: "gpu-screen-recorder".into(),
                args: vec!["-w".into(), "screen".into()],
            },
            "/clips".into(),
        );
        config.max_stderr_bytes = 3;
        config.stable_after = Duration::from_secs(10);
        Supervisor::new(
            FakeProcess::default(),
            FakeFs {
                outputs: vec![],
                symlink: false,
                regular: true,
            },
            FakeClock(Cell::new(clock)),
            FakeProbe,
            config,
        )
    }

    #[test]
    fn argv_is_direct_for_native_and_flatpak() {
        assert_eq!(
            Launch::Native {
                program: "recorder".into(),
                args: vec![";rm".into()]
            }
            .argv(),
            vec![OsString::from("recorder"), OsString::from(";rm")]
        );
        assert_eq!(
            Launch::Flatpak {
                app_id: "com.example.Recorder".into(),
                args: vec!["--arg".into()]
            }
            .argv(),
            vec!["flatpak", "run", "com.example.Recorder", "--arg"]
                .into_iter()
                .map(OsString::from)
                .collect::<Vec<_>>()
        );
    }
    #[test]
    fn replay_builder_keeps_recorder_arguments_direct() {
        let launch = replay_launch(
            RecorderInstall::Flatpak {
                app_id: "com.dec05eba.gpu_screen_recorder".into(),
            },
            ReplayConfig::sixty_second("screen".into(), "/clips;not-a-shell".into()),
        );
        assert_eq!(
            launch.argv(),
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
                "/clips;not-a-shell"
            ]
            .into_iter()
            .map(OsString::from)
            .collect::<Vec<_>>()
        );
    }
    #[test]
    fn std_filesystem_selects_newest_regular_output_and_rejects_symlinks() {
        let root = temporary_directory("filesystem");
        let older = root.join("older.mp4");
        let newer = root.join("newer.mp4");
        std::fs::write(&older, b"old").unwrap();
        std::thread::sleep(Duration::from_millis(15));
        let since = system_time_ms(SystemTime::now()).unwrap();
        std::thread::sleep(Duration::from_millis(15));
        std::fs::write(&newer, b"new").unwrap();
        let filesystem = StdFilesystem;
        assert_eq!(
            filesystem.outputs_since(&root, since).unwrap(),
            vec![newer.clone()]
        );
        #[cfg(unix)]
        {
            let link = root.join("link.mp4");
            std::os::unix::fs::symlink(&newer, &link).unwrap();
            assert!(filesystem.is_symlink(&link).unwrap());
            let outside = root.with_extension("outside");
            std::fs::write(&outside, b"outside").unwrap();
            assert!(!filesystem.is_contained(&root, &outside).unwrap());
            std::fs::remove_file(outside).unwrap();
        }
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn fake_ffprobe_is_strict_and_uses_a_temp_media_path() {
        let root = temporary_directory("ffprobe");
        let media = root.join("clip.mp4");
        std::fs::write(&media, b"not real media; fake probe owns validation").unwrap();
        let probe_program = root.join("fake-ffprobe");
        std::fs::write(&probe_program, "#!/usr/bin/env sh\nprintf '%s' '{\"streams\":[{\"codec_type\":\"video\"}],\"format\":{\"duration\":\"1.25\"}}'\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = std::fs::metadata(&probe_program).unwrap().permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&probe_program, permissions).unwrap();
        }
        let probe = FfprobeMediaProbe {
            program: probe_program,
            timeout: Duration::from_secs(1),
            max_output_bytes: 1024,
        };
        assert_eq!(
            probe.verify(&media).unwrap(),
            MediaInfo {
                duration_ms: 1250,
                video_streams: 1
            }
        );
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn only_one_follow_up_is_coalesced_and_provenance_survives() {
        let mut s = supervisor(5);
        s.start().unwrap();
        assert_eq!(
            s.request_save("manual-first", SaveProvenance::ManualFlag),
            Ok(SaveRequestOutcome {
                recorder_request_id: "manual-first".into(),
                disposition: SaveDisposition::Signalled,
            })
        );
        assert_eq!(
            s.request_save("auto-follow-up", SaveProvenance::AutoRoundEnd),
            Ok(SaveRequestOutcome {
                recorder_request_id: "auto-follow-up".into(),
                disposition: SaveDisposition::Coalesced,
            })
        );
        assert_eq!(
            s.request_save("manual-coalesced", SaveProvenance::ManualFlag),
            Ok(SaveRequestOutcome {
                recorder_request_id: "auto-follow-up".into(),
                disposition: SaveDisposition::Coalesced,
            })
        );
        let ack = s.acknowledge("/clips/one.mp4".into()).unwrap();
        assert_eq!(ack.recorder_request_id, "manual-first");
        assert_eq!(ack.provenance, SaveProvenance::ManualFlag);
        assert_eq!(s.process.signals, vec![Signal::User1, Signal::User1]);
        let follow_up = s.acknowledge("/clips/two.mp4".into()).unwrap();
        assert_eq!(follow_up.recorder_request_id, "auto-follow-up");
        assert_eq!(follow_up.provenance, SaveProvenance::ManualFlag);
    }
    #[test]
    fn rejects_escape_symlink_and_unverified_media() {
        let mut s = supervisor(0);
        s.start().unwrap();
        s.request_save("manual", SaveProvenance::ManualFlag)
            .unwrap();
        assert_eq!(
            s.acknowledge("/clips/../outside.mp4".into()),
            Err(Error::UnsafeOutputPath)
        );
        s.filesystem.symlink = true;
        assert_eq!(
            s.acknowledge("/clips/link.mp4".into()),
            Err(Error::SymlinkOutput)
        );
    }
    #[test]
    fn discovers_only_after_a_save_and_verifies_before_acknowledging() {
        let mut s = supervisor(0);
        s.start().unwrap();
        assert_eq!(s.discover_save(), Err(Error::NoSaveInFlight));
        s.request_save("auto", SaveProvenance::AutoRoundEnd)
            .unwrap();
        s.filesystem.outputs = vec!["/clips/final.mp4".into()];
        assert_eq!(
            s.discover_save().unwrap().unwrap().path,
            PathBuf::from("/clips/final.mp4")
        );
    }
    #[test]
    fn stderr_is_bounded_and_exit_uses_capped_backoff_then_stable_reset() {
        let mut s = supervisor(0);
        s.start().unwrap();
        s.process.stderr = b"abcdef".to_vec();
        s.process.exits = vec![Some(9)];
        assert_eq!(s.poll(), Ok(Some(9)));
        assert_eq!(s.stderr_tail(), b"def");
        assert_eq!(s.next_restart_ms(), Some(1000));
        s.clock.0.set(1000);
        s.poll().unwrap();
        s.process.exits = vec![Some(1)];
        s.poll().unwrap();
        assert_eq!(s.next_restart_ms(), Some(3000));
        s.clock.0.set(3000);
        s.poll().unwrap();
        s.clock.0.set(13_000);
        s.poll().unwrap();
        s.process.exits = vec![Some(1)];
        s.poll().unwrap();
        assert_eq!(s.next_restart_ms(), Some(14_000));
        assert_eq!(restart_delay(99), Duration::from_secs(30));
    }
    #[test]
    fn shutdown_sends_interrupt_and_never_restarts() {
        let mut s = supervisor(0);
        s.start().unwrap();
        s.shutdown().unwrap();
        s.process.exits = vec![Some(0)];
        assert_eq!(s.poll(), Ok(Some(0)));
        assert_eq!(s.process.signals, vec![Signal::Interrupt]);
        assert_eq!(s.next_restart_ms(), None);
    }

    fn temporary_directory(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "openfrag-capture-{name}-{}",
            system_time_ms(SystemTime::now()).unwrap()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }
}
