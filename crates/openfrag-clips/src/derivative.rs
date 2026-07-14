use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use crate::{
    ArtifactProvenance, Clip, ClipRepository, DerivativeProvenance, ModelError, RepositoryError,
    ReviewEdit, ReviewError, ReviewState, TrimRange, review_clip, sanitize_export_filename,
};

#[derive(Clone, Debug, Default)]
pub struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TranscodeError {
    Cancelled,
    TimedOut,
    Spawn(String),
    Failed {
        status: Option<i32>,
        stderr: String,
        stderr_truncated: bool,
    },
    TargetTooSmall,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MediaProbeError {
    Cancelled,
    TimedOut,
    Spawn(String),
    Failed {
        status: Option<i32>,
        stderr: String,
        stderr_truncated: bool,
    },
    MalformedOutput,
    NonPositiveDuration,
    MissingVideoStream,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MediaInfo {
    duration_ms: u64,
}

impl MediaInfo {
    pub const fn new(duration_ms: u64, has_video_stream: bool) -> Result<Self, MediaProbeError> {
        if duration_ms == 0 {
            Err(MediaProbeError::NonPositiveDuration)
        } else if !has_video_stream {
            Err(MediaProbeError::MissingVideoStream)
        } else {
            Ok(Self { duration_ms })
        }
    }

    pub const fn duration_ms(&self) -> u64 {
        self.duration_ms
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscordEncodeProfile {
    max_bytes: u64,
    max_width: u16,
    max_height: u16,
    max_frames_per_second: u8,
    audio_bitrate_bps: u64,
    min_video_bitrate_bps: u64,
    max_video_bitrate_bps: u64,
}

impl Default for DiscordEncodeProfile {
    fn default() -> Self {
        Self {
            max_bytes: 10 * 1024 * 1024,
            max_width: 1920,
            max_height: 1080,
            max_frames_per_second: 60,
            audio_bitrate_bps: 128_000,
            min_video_bitrate_bps: 400_000,
            max_video_bitrate_bps: 8_000_000,
        }
    }
}

impl DiscordEncodeProfile {
    pub const fn max_bytes(&self) -> u64 {
        self.max_bytes
    }

    pub const fn max_width(&self) -> u16 {
        self.max_width
    }

    pub const fn max_height(&self) -> u16 {
        self.max_height
    }

    pub const fn max_frames_per_second(&self) -> u8 {
        self.max_frames_per_second
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DerivativeProfile {
    Review,
    Discord(DiscordEncodeProfile),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TranscodeRequest<'a> {
    source: &'a Path,
    destination: &'a Path,
    trim: TrimRange,
    profile: &'a DerivativeProfile,
}

impl TranscodeRequest<'_> {
    pub fn source(&self) -> &Path {
        self.source
    }

    pub fn destination(&self) -> &Path {
        self.destination
    }

    pub const fn trim(&self) -> TrimRange {
        self.trim
    }

    pub const fn profile(&self) -> &DerivativeProfile {
        self.profile
    }
}

pub trait Transcoder {
    fn transcode(
        &mut self,
        request: &TranscodeRequest<'_>,
        cancellation: &CancellationToken,
    ) -> Result<(), TranscodeError>;
}

pub const MAX_CAPTURED_PROCESS_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug)]
pub struct FfmpegTranscoder {
    executable: PathBuf,
    timeout: Duration,
}

impl FfmpegTranscoder {
    pub fn new(executable: PathBuf, timeout: Duration) -> Self {
        Self {
            executable,
            timeout,
        }
    }
}

impl Transcoder for FfmpegTranscoder {
    fn transcode(
        &mut self,
        request: &TranscodeRequest<'_>,
        cancellation: &CancellationToken,
    ) -> Result<(), TranscodeError> {
        let mut command = Command::new(&self.executable);
        command.args(["-nostdin", "-hide_banner", "-loglevel", "error", "-n", "-i"]);
        command.arg(request.source());
        command.args(["-ss", &format_milliseconds(request.trim().start_ms())]);
        command.args([
            "-t",
            &format_milliseconds(request.trim().end_ms() - request.trim().start_ms()),
        ]);
        match request.profile() {
            DerivativeProfile::Review => {
                command.args(["-map", "0", "-c", "copy"]);
            }
            DerivativeProfile::Discord(profile) => {
                let duration_ms = request.trim().end_ms() - request.trim().start_ms();
                let video_bitrate = discord_video_bitrate(profile, duration_ms)?;
                let filter = format!(
                    "scale={}:{}:force_original_aspect_ratio=decrease:force_divisible_by=2,fps={}",
                    profile.max_width, profile.max_height, profile.max_frames_per_second
                );
                command
                    .args(["-map", "0:v:0", "-map", "0:a?", "-vf"])
                    .arg(filter)
                    .args(["-c:v", "libx264", "-pix_fmt", "yuv420p"])
                    .args(["-b:v", &video_bitrate.to_string()])
                    .args(["-maxrate", &video_bitrate.to_string()])
                    .args(["-bufsize", &video_bitrate.saturating_mul(2).to_string()])
                    .args([
                        "-c:a",
                        "aac",
                        "-b:a",
                        &profile.audio_bitrate_bps.to_string(),
                    ])
                    .args(["-movflags", "+faststart", "-f", "mp4"]);
            }
        }
        command.arg(request.destination());
        match run_process(&mut command, self.timeout, cancellation) {
            Ok(_) => Ok(()),
            Err(ProcessFailure::Cancelled) => Err(TranscodeError::Cancelled),
            Err(ProcessFailure::TimedOut) => Err(TranscodeError::TimedOut),
            Err(ProcessFailure::Io(error)) => Err(TranscodeError::Spawn(error)),
            Err(ProcessFailure::Failed {
                status,
                stderr,
                stderr_truncated,
            }) => Err(TranscodeError::Failed {
                status,
                stderr,
                stderr_truncated,
            }),
        }
    }
}

#[derive(Clone, Debug)]
pub struct FfprobeMediaProbe {
    executable: PathBuf,
    timeout: Duration,
}

impl FfprobeMediaProbe {
    pub fn new(executable: PathBuf, timeout: Duration) -> Self {
        Self {
            executable,
            timeout,
        }
    }
}

impl MediaProbe for FfprobeMediaProbe {
    fn probe(
        &mut self,
        path: &Path,
        cancellation: &CancellationToken,
    ) -> Result<MediaInfo, MediaProbeError> {
        let mut command = Command::new(&self.executable);
        command.args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=codec_type",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1",
        ]);
        command.arg(path);
        let output = match run_process(&mut command, self.timeout, cancellation) {
            Ok(output) => output,
            Err(ProcessFailure::Cancelled) => return Err(MediaProbeError::Cancelled),
            Err(ProcessFailure::TimedOut) => return Err(MediaProbeError::TimedOut),
            Err(ProcessFailure::Io(error)) => return Err(MediaProbeError::Spawn(error)),
            Err(ProcessFailure::Failed {
                status,
                stderr,
                stderr_truncated,
            }) => {
                return Err(MediaProbeError::Failed {
                    status,
                    stderr,
                    stderr_truncated,
                });
            }
        };
        if output.stdout_truncated {
            return Err(MediaProbeError::MalformedOutput);
        }
        let has_video = output
            .stdout
            .lines()
            .any(|line| line.trim() == "codec_type=video");
        if !has_video {
            return Err(MediaProbeError::MissingVideoStream);
        }
        let duration = output
            .stdout
            .lines()
            .find_map(|line| line.trim().strip_prefix("duration="))
            .ok_or(MediaProbeError::MalformedOutput)?;
        let duration_ms = parse_seconds_as_milliseconds(duration)?;
        MediaInfo::new(duration_ms, true)
    }
}

pub trait MediaProbe {
    fn probe(
        &mut self,
        path: &Path,
        cancellation: &CancellationToken,
    ) -> Result<MediaInfo, MediaProbeError>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReviewMetadata {
    pub title: String,
    pub note: String,
    pub tags: BTreeSet<String>,
    pub favorite: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DerivativeReviewRequest {
    pub artifact_id: String,
    pub destination_directory: PathBuf,
    pub trim: Option<TrimRange>,
    pub profile: DerivativeProfile,
    pub metadata: ReviewMetadata,
    pub reviewed_at_ms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DerivativeReviewError {
    Cancelled,
    TrimOutsideSource,
    Transcode(TranscodeError),
    Probe(MediaProbeError),
    File(String),
    Model(ModelError),
    Review(ReviewError),
    RepositoryCleanup {
        repository: RepositoryError,
        cleanup: String,
    },
    NameSpaceExhausted,
    OutputTooLarge {
        actual_bytes: u64,
        max_bytes: u64,
    },
}

pub fn prepare_derivative_and_review(
    repository: &mut impl ClipRepository,
    transcoder: &mut impl Transcoder,
    probe: &mut impl MediaProbe,
    clip: &Clip,
    request: DerivativeReviewRequest,
    cancellation: &CancellationToken,
) -> Result<Clip, DerivativeReviewError> {
    if cancellation.is_cancelled() {
        return Err(DerivativeReviewError::Cancelled);
    }
    if matches!(clip.review_state(), ReviewState::Rejected { .. }) {
        return Err(DerivativeReviewError::Review(
            ReviewError::InvalidTransition,
        ));
    }
    let trim = match request.trim {
        Some(trim) => trim,
        None => TrimRange::new(0, clip.source_capture().duration_ms())
            .map_err(DerivativeReviewError::Model)?,
    };
    if trim.end_ms() > clip.source_capture().duration_ms() {
        return Err(DerivativeReviewError::TrimOutsideSource);
    }

    fs::create_dir_all(&request.destination_directory)
        .map_err(|error| DerivativeReviewError::File(error.to_string()))?;
    let filename = sanitize_export_filename(&request.metadata.title, clip.id());
    let staging_path = staging_path(&request.destination_directory, &filename);
    let creation_result = create_staging_derivative(
        transcoder,
        clip.source_capture().path(),
        &staging_path,
        trim,
        request.trim.is_some(),
        &request.profile,
        cancellation,
    );
    if let Err(error) = creation_result {
        let _ = fs::remove_file(&staging_path);
        return Err(error);
    }
    let media = verify_staging_derivative(probe, &staging_path, &request.profile, cancellation)?;
    let destination_path =
        match promote_without_overwrite(&staging_path, &request.destination_directory, &filename) {
            Ok(path) => path,
            Err(error) => {
                let _ = fs::remove_file(&staging_path);
                return Err(error);
            }
        };
    let (sha256, bytes) = hash_and_size(&destination_path).map_err(|error| {
        let _ = fs::remove_file(&destination_path);
        DerivativeReviewError::File(error.to_string())
    })?;
    let artifact = ArtifactProvenance::new(
        request.artifact_id,
        destination_path.clone(),
        sha256,
        media.duration_ms(),
        bytes,
    )
    .map_err(|error| {
        let _ = fs::remove_file(&destination_path);
        DerivativeReviewError::Model(error)
    })?;
    let derivative = DerivativeProvenance::new(artifact, clip.source_capture().artifact_id(), trim);
    let reviewed = review_clip(
        repository,
        clip,
        ReviewEdit {
            title: request.metadata.title,
            note: request.metadata.note,
            tags: request.metadata.tags,
            favorite: request.metadata.favorite,
            derivative: Some(derivative),
        },
        request.reviewed_at_ms,
    );
    match reviewed {
        Ok(reviewed) => Ok(reviewed),
        Err(ReviewError::Repository(repository)) => match fs::remove_file(&destination_path) {
            Ok(()) => Err(DerivativeReviewError::Review(ReviewError::Repository(
                repository,
            ))),
            Err(cleanup) => Err(DerivativeReviewError::RepositoryCleanup {
                repository,
                cleanup: cleanup.to_string(),
            }),
        },
        Err(error) => {
            let _ = fs::remove_file(&destination_path);
            Err(DerivativeReviewError::Review(error))
        }
    }
}

fn verify_staging_derivative(
    probe: &mut impl MediaProbe,
    staging_path: &Path,
    profile: &DerivativeProfile,
    cancellation: &CancellationToken,
) -> Result<MediaInfo, DerivativeReviewError> {
    if cancellation.is_cancelled() {
        let _ = fs::remove_file(staging_path);
        return Err(DerivativeReviewError::Cancelled);
    }
    let media = probe.probe(staging_path, cancellation).map_err(|error| {
        let _ = fs::remove_file(staging_path);
        DerivativeReviewError::Probe(error)
    })?;
    if let DerivativeProfile::Discord(discord) = profile {
        let actual_bytes = fs::metadata(staging_path)
            .map_err(|error| {
                let _ = fs::remove_file(staging_path);
                DerivativeReviewError::File(error.to_string())
            })?
            .len();
        if actual_bytes > discord.max_bytes {
            let _ = fs::remove_file(staging_path);
            return Err(DerivativeReviewError::OutputTooLarge {
                actual_bytes,
                max_bytes: discord.max_bytes,
            });
        }
    }
    Ok(media)
}

fn create_staging_derivative(
    transcoder: &mut impl Transcoder,
    source: &Path,
    staging_path: &Path,
    trim: TrimRange,
    trim_requested: bool,
    profile: &DerivativeProfile,
    cancellation: &CancellationToken,
) -> Result<(), DerivativeReviewError> {
    if !trim_requested && *profile == DerivativeProfile::Review {
        copy_to_new_file(source, staging_path)
            .map_err(|error| DerivativeReviewError::File(error.to_string()))
    } else {
        transcoder
            .transcode(
                &TranscodeRequest {
                    source,
                    destination: staging_path,
                    trim,
                    profile,
                },
                cancellation,
            )
            .map_err(DerivativeReviewError::Transcode)
    }
}

fn staging_path(directory: &Path, filename: &str) -> PathBuf {
    static NEXT_STAGING_ID: AtomicU64 = AtomicU64::new(0);
    let id = NEXT_STAGING_ID.fetch_add(1, Ordering::Relaxed);
    directory.join(format!(".{filename}.stage-{}-{id}", std::process::id()))
}

fn copy_to_new_file(source: &Path, destination: &Path) -> io::Result<()> {
    let mut source_file = fs::File::open(source)?;
    let mut destination_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    io::copy(&mut source_file, &mut destination_file)?;
    destination_file.sync_all()
}

fn promote_without_overwrite(
    staging_path: &Path,
    directory: &Path,
    filename: &str,
) -> Result<PathBuf, DerivativeReviewError> {
    let stem = filename.strip_suffix(".mp4").unwrap_or(filename);
    for collision_index in 1_u32..=u32::MAX {
        let candidate = if collision_index == 1 {
            filename.to_owned()
        } else {
            format!("{stem}-{collision_index}.mp4")
        };
        let destination = directory.join(candidate);
        match fs::hard_link(staging_path, &destination) {
            Ok(()) => {
                if let Err(error) = fs::remove_file(staging_path) {
                    let _ = fs::remove_file(&destination);
                    return Err(DerivativeReviewError::File(error.to_string()));
                }
                return Ok(destination);
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(DerivativeReviewError::File(error.to_string())),
        }
    }
    Err(DerivativeReviewError::NameSpaceExhausted)
}

fn hash_and_size(path: &Path) -> io::Result<(String, u64)> {
    let mut file = fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 16 * 1024];
    let mut bytes = 0_u64;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
        bytes = bytes.saturating_add(read as u64);
    }
    Ok((format!("{:x}", hash.finalize()), bytes))
}

fn format_milliseconds(milliseconds: u64) -> String {
    format!("{}.{:03}", milliseconds / 1_000, milliseconds % 1_000)
}

fn discord_video_bitrate(
    profile: &DiscordEncodeProfile,
    duration_ms: u64,
) -> Result<u64, TranscodeError> {
    let total_bitrate = profile
        .max_bytes
        .saturating_mul(8)
        .saturating_mul(1_000)
        .checked_div(duration_ms)
        .unwrap_or(0);
    let available = total_bitrate
        .saturating_mul(95)
        .checked_div(100)
        .unwrap_or(0)
        .saturating_sub(profile.audio_bitrate_bps);
    if available < profile.min_video_bitrate_bps {
        Err(TranscodeError::TargetTooSmall)
    } else {
        Ok(available.min(profile.max_video_bitrate_bps))
    }
}

fn parse_seconds_as_milliseconds(value: &str) -> Result<u64, MediaProbeError> {
    let (seconds, fraction) = value
        .split_once('.')
        .map_or((value, ""), |(seconds, fraction)| (seconds, fraction));
    let seconds = seconds
        .parse::<u64>()
        .map_err(|_| MediaProbeError::MalformedOutput)?;
    if !fraction.chars().all(|character| character.is_ascii_digit()) {
        return Err(MediaProbeError::MalformedOutput);
    }
    let mut fraction_ms = 0_u64;
    let mut digits = 0_u8;
    for character in fraction.chars().take(3) {
        let digit = character
            .to_digit(10)
            .ok_or(MediaProbeError::MalformedOutput)?;
        fraction_ms = fraction_ms
            .saturating_mul(10)
            .saturating_add(u64::from(digit));
        digits += 1;
    }
    while digits < 3 {
        fraction_ms = fraction_ms.saturating_mul(10);
        digits += 1;
    }
    let duration_ms = seconds
        .checked_mul(1_000)
        .and_then(|milliseconds| milliseconds.checked_add(fraction_ms))
        .ok_or(MediaProbeError::MalformedOutput)?;
    if duration_ms == 0 {
        Err(MediaProbeError::NonPositiveDuration)
    } else {
        Ok(duration_ms)
    }
}

struct ProcessOutput {
    stdout: String,
    stdout_truncated: bool,
}

enum ProcessFailure {
    Cancelled,
    TimedOut,
    Io(String),
    Failed {
        status: Option<i32>,
        stderr: String,
        stderr_truncated: bool,
    },
}

fn run_process(
    command: &mut Command,
    timeout: Duration,
    cancellation: &CancellationToken,
) -> Result<ProcessOutput, ProcessFailure> {
    if cancellation.is_cancelled() {
        return Err(ProcessFailure::Cancelled);
    }
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| ProcessFailure::Io(error.to_string()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ProcessFailure::Io("child stdout was not captured".into()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| ProcessFailure::Io("child stderr was not captured".into()))?;
    let stdout_reader = thread::spawn(move || read_bounded(stdout));
    let stderr_reader = thread::spawn(move || read_bounded(stderr));
    let started = Instant::now();
    let process_result = loop {
        if cancellation.is_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            break Err(ProcessFailure::Cancelled);
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            break Err(ProcessFailure::TimedOut);
        }
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => thread::sleep(Duration::from_millis(5)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(ProcessFailure::Io(error.to_string()));
            }
        }
    };
    let (stdout, stdout_truncated) = join_reader(stdout_reader)?;
    let (stderr, stderr_truncated) = join_reader(stderr_reader)?;
    let stdout = String::from_utf8_lossy(&stdout).into_owned();
    let stderr = String::from_utf8_lossy(&stderr).into_owned();
    match process_result {
        Ok(status) if status.success() => Ok(ProcessOutput {
            stdout,
            stdout_truncated,
        }),
        Ok(status) => Err(ProcessFailure::Failed {
            status: status.code(),
            stderr,
            stderr_truncated,
        }),
        Err(error) => Err(error),
    }
}

fn read_bounded(mut reader: impl Read) -> io::Result<(Vec<u8>, bool)> {
    let mut captured = Vec::new();
    let mut buffer = [0_u8; 4 * 1024];
    let mut truncated = false;
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        let remaining = MAX_CAPTURED_PROCESS_BYTES.saturating_sub(captured.len());
        let retained = remaining.min(read);
        captured.extend_from_slice(&buffer[..retained]);
        truncated |= retained < read;
    }
    Ok((captured, truncated))
}

fn join_reader(
    reader: thread::JoinHandle<io::Result<(Vec<u8>, bool)>>,
) -> Result<(Vec<u8>, bool), ProcessFailure> {
    reader
        .join()
        .map_err(|_| ProcessFailure::Io("process output reader panicked".into()))?
        .map_err(|error| ProcessFailure::Io(error.to_string()))
}
