use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use openfrag_clips::{
    ArtifactProvenance, CancellationToken, Clip, ClipId, ClipOrigin, ClipRepository,
    DerivativeProfile, DerivativeReviewError, DerivativeReviewRequest, DiscordCompatibilityError,
    DiscordEncodeProfile, FfmpegTranscoder, FfprobeMediaProbe, MAX_CAPTURED_PROCESS_BYTES,
    MediaInfo, MediaProbe, MediaProbeError, RepositoryError, ReviewError, ReviewMetadata,
    ReviewState, TranscodeError, TranscodeRequest, Transcoder, TrimRange,
    prepare_derivative_and_review,
};

#[derive(Default)]
struct MemoryRepository {
    committed: Vec<Clip>,
    fail: bool,
}

impl ClipRepository for MemoryRepository {
    fn compare_and_swap(
        &mut self,
        expected_revision: u64,
        updated: &Clip,
    ) -> Result<(), RepositoryError> {
        assert_eq!(expected_revision, 0);
        if self.fail {
            Err(RepositoryError::new("injected repository failure"))
        } else {
            self.committed.push(updated.clone());
            Ok(())
        }
    }
}

struct UnavailableTranscoder;

impl Transcoder for UnavailableTranscoder {
    fn transcode(
        &mut self,
        _request: &TranscodeRequest<'_>,
        _cancellation: &CancellationToken,
    ) -> Result<(), TranscodeError> {
        Err(TranscodeError::Failed {
            status: Some(99),
            stderr: "transcoder must not be needed".into(),
            stderr_truncated: false,
        })
    }
}

struct ValidProbe;

impl MediaProbe for ValidProbe {
    fn probe(
        &mut self,
        _path: &Path,
        _cancellation: &CancellationToken,
    ) -> Result<MediaInfo, MediaProbeError> {
        MediaInfo::new(42_000, true)
    }
}

fn temporary_directory(test_name: &str) -> PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "openfrag-clips-{test_name}-{}-{unique}",
        std::process::id()
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
}

#[cfg(unix)]
fn write_executable(path: &Path, body: &str) {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;

    let mut file = std::fs::File::create(path).unwrap();
    file.write_all(body.as_bytes()).unwrap();
    file.sync_all().unwrap();
    drop(file);
    let mut permissions = std::fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(path, permissions).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(10));
}

fn provisional_clip(source_path: PathBuf) -> Clip {
    Clip::provisional(
        ClipId::new("clip-7").unwrap(),
        ArtifactProvenance::new("source-7", source_path, "source-sha", 42_000, 17).unwrap(),
        "session-7",
        ClipOrigin::manual_flag("manual-7", 5_000, BTreeSet::new()).unwrap(),
        "Round win",
    )
    .unwrap()
}

fn review_metadata() -> ReviewMetadata {
    ReviewMetadata {
        title: "Round win".into(),
        note: "Local derivative".into(),
        tags: BTreeSet::from(["mirage".into()]),
        favorite: true,
    }
}

#[test]
fn no_trim_review_copies_and_verifies_an_immutable_source_before_commit() {
    let root = temporary_directory("no-trim");
    let source_path = root.join("source.mp4");
    let source_bytes = b"source clip bytes";
    std::fs::write(&source_path, source_bytes).unwrap();
    let destination_directory = root.join("derivatives");
    std::fs::create_dir(&destination_directory).unwrap();
    let clip = provisional_clip(source_path.clone());
    let original = clip.clone();
    let mut repository = MemoryRepository::default();

    let reviewed = prepare_derivative_and_review(
        &mut repository,
        &mut UnavailableTranscoder,
        &mut ValidProbe,
        &clip,
        DerivativeReviewRequest {
            artifact_id: "review-derivative".into(),
            destination_directory: destination_directory.clone(),
            trim: None,
            profile: DerivativeProfile::Review,
            metadata: review_metadata(),
            reviewed_at_ms: 90_000,
        },
        &CancellationToken::new(),
    )
    .unwrap();

    assert_eq!(clip, original);
    assert_eq!(std::fs::read(&source_path).unwrap(), source_bytes);
    assert!(matches!(
        reviewed.review_state(),
        ReviewState::Reviewed {
            reviewed_at_ms: 90_000
        }
    ));
    let derivative = reviewed.derivative().unwrap();
    assert_eq!(derivative.source_artifact_id(), "source-7");
    assert_eq!(derivative.trim().start_ms(), 0);
    assert_eq!(derivative.trim().end_ms(), 42_000);
    assert_eq!(derivative.artifact().duration_ms(), 42_000);
    assert_eq!(
        derivative.artifact().sha256(),
        "6cef93364d8161935d80c6596c4acf2e5eedb99b0739f1a74aa101a5cf7f7a1c"
    );
    assert_eq!(reviewed.source_capture().sha256(), "source-sha");
    assert_eq!(
        derivative.artifact().path(),
        destination_directory.join("round-win-clip-7.mp4")
    );
    assert_eq!(
        std::fs::read(derivative.artifact().path()).unwrap(),
        source_bytes
    );
    assert_eq!(repository.committed, vec![reviewed]);
    assert!(
        std::fs::read_dir(&destination_directory)
            .unwrap()
            .all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains("stage"))
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn trimmed_discord_derivative_uses_direct_deterministic_ffmpeg_arguments() {
    let root = temporary_directory("trim-discord");
    let source_path = root.join("source.mp4");
    let source_bytes = b"immutable source bytes";
    std::fs::write(&source_path, source_bytes).unwrap();
    let destination_directory = root.join("derivatives");
    std::fs::create_dir(&destination_directory).unwrap();
    let argument_log = root.join("ffmpeg-arguments.txt");
    let ffmpeg = root.join("fake-ffmpeg");
    write_executable(
        &ffmpeg,
        &format!(
            "#!/bin/sh\n: > '{}'\nfor argument in \"$@\"; do printf '%s\\n' \"$argument\" >> '{}'; last=$argument; done\nprintf 'encoded video bytes' > \"$last\"\n",
            argument_log.display(),
            argument_log.display()
        ),
    );
    let ffprobe = root.join("fake-ffprobe");
    write_executable(
        &ffprobe,
        "#!/bin/sh\nprintf '%s\\n' '{\"streams\":[{\"codec_type\":\"video\",\"codec_name\":\"h264\",\"pix_fmt\":\"yuv420p\",\"width\":1920,\"height\":1080,\"avg_frame_rate\":\"60/1\"},{\"codec_type\":\"audio\",\"codec_name\":\"aac\"}],\"format\":{\"duration\":\"30.000000\"}}'\n",
    );
    let clip = provisional_clip(source_path.clone());
    let original = clip.clone();
    let mut repository = MemoryRepository::default();
    let mut transcoder = FfmpegTranscoder::new(ffmpeg, std::time::Duration::from_secs(2));
    let mut probe = FfprobeMediaProbe::new(ffprobe, std::time::Duration::from_secs(2));

    let reviewed = prepare_derivative_and_review(
        &mut repository,
        &mut transcoder,
        &mut probe,
        &clip,
        DerivativeReviewRequest {
            artifact_id: "discord-derivative".into(),
            destination_directory,
            trim: Some(TrimRange::new(5_000, 35_000).unwrap()),
            profile: DerivativeProfile::Discord(DiscordEncodeProfile::default()),
            metadata: review_metadata(),
            reviewed_at_ms: 90_000,
        },
        &CancellationToken::new(),
    )
    .unwrap();

    assert_eq!(clip, original);
    assert_eq!(std::fs::read(&source_path).unwrap(), source_bytes);
    assert_eq!(
        std::fs::read(reviewed.derivative().unwrap().artifact().path()).unwrap(),
        b"encoded video bytes"
    );
    let arguments: Vec<String> = std::fs::read_to_string(argument_log)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    assert!(arguments.windows(2).any(|pair| pair == ["-ss", "5.000"]));
    assert!(arguments.windows(2).any(|pair| pair == ["-t", "30.000"]));
    assert!(arguments.windows(2).any(|pair| pair == ["-c:v", "libx264"]));
    assert!(
        arguments
            .windows(2)
            .any(|pair| pair == ["-pix_fmt", "yuv420p"])
    );
    assert!(arguments.windows(2).any(|pair| pair == ["-c:a", "aac"]));
    assert!(arguments.windows(2).any(|pair| pair[0] == "-vf"
        && pair[1].contains("1920:1080")
        && pair[1].contains("fps=60")));
    assert!(arguments.windows(2).any(|pair| {
        pair[0] == "-b:v"
            && pair[1]
                .parse::<u64>()
                .is_ok_and(|bitrate| bitrate <= 8_000_000)
    }));
    assert!(
        arguments
            .last()
            .is_some_and(|path| path.contains(".stage-"))
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn trimmed_review_reencodes_for_accurate_cuts_instead_of_stream_copying() {
    let root = temporary_directory("trim-review");
    let source_path = root.join("source.mp4");
    let source_bytes = b"immutable source bytes";
    std::fs::write(&source_path, source_bytes).unwrap();
    let destination_directory = root.join("derivatives");
    std::fs::create_dir(&destination_directory).unwrap();
    let argument_log = root.join("ffmpeg-arguments.txt");
    let ffmpeg = root.join("fake-ffmpeg");
    write_executable(
        &ffmpeg,
        &format!(
            "#!/bin/sh\n: > '{}'\nfor argument in \"$@\"; do printf '%s\\n' \"$argument\" >> '{}'; last=$argument; done\nprintf 'accurately trimmed bytes' > \"$last\"\n",
            argument_log.display(),
            argument_log.display()
        ),
    );
    let clip = provisional_clip(source_path.clone());
    let mut repository = MemoryRepository::default();

    let reviewed = prepare_derivative_and_review(
        &mut repository,
        &mut FfmpegTranscoder::new(ffmpeg, std::time::Duration::from_secs(2)),
        &mut ValidProbe,
        &clip,
        DerivativeReviewRequest {
            artifact_id: "review-trim".into(),
            destination_directory,
            trim: Some(TrimRange::new(5_000, 35_000).unwrap()),
            profile: DerivativeProfile::Review,
            metadata: review_metadata(),
            reviewed_at_ms: 90_000,
        },
        &CancellationToken::new(),
    )
    .unwrap();

    let arguments: Vec<String> = std::fs::read_to_string(argument_log)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    assert!(arguments.windows(2).any(|pair| pair == ["-ss", "5.000"]));
    assert!(arguments.windows(2).any(|pair| pair == ["-t", "30.000"]));
    assert!(arguments.windows(2).any(|pair| pair == ["-c:v", "libx264"]));
    assert!(arguments.windows(2).any(|pair| pair == ["-c:a", "aac"]));
    assert!(!arguments.windows(2).any(|pair| pair == ["-c", "copy"]));
    assert_eq!(
        std::fs::read(reviewed.derivative().unwrap().artifact().path()).unwrap(),
        b"accurately trimmed bytes"
    );
    assert_eq!(std::fs::read(&source_path).unwrap(), source_bytes);
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn malformed_ffprobe_output_prevents_commit_and_removes_staging_file() {
    let root = temporary_directory("malformed-probe");
    let source_path = root.join("source.mp4");
    let source_bytes = b"immutable source bytes";
    std::fs::write(&source_path, source_bytes).unwrap();
    let destination_directory = root.join("derivatives");
    std::fs::create_dir(&destination_directory).unwrap();
    let ffmpeg = root.join("fake-ffmpeg");
    write_executable(
        &ffmpeg,
        "#!/bin/sh\nfor argument in \"$@\"; do last=$argument; done\nprintf 'partial derivative' > \"$last\"\n",
    );
    let ffprobe = root.join("fake-ffprobe");
    write_executable(
        &ffprobe,
        "#!/bin/sh\nprintf '%s\\n' '{\"streams\":[{\"codec_type\":\"video\"}],\"format\":{\"duration\":\"30.000junk\"}}'\n",
    );
    let clip = provisional_clip(source_path.clone());
    let mut repository = MemoryRepository::default();

    let result = prepare_derivative_and_review(
        &mut repository,
        &mut FfmpegTranscoder::new(ffmpeg, std::time::Duration::from_secs(2)),
        &mut FfprobeMediaProbe::new(ffprobe, std::time::Duration::from_secs(2)),
        &clip,
        DerivativeReviewRequest {
            artifact_id: "bad-probe".into(),
            destination_directory: destination_directory.clone(),
            trim: Some(TrimRange::new(5_000, 35_000).unwrap()),
            profile: DerivativeProfile::Review,
            metadata: review_metadata(),
            reviewed_at_ms: 90_000,
        },
        &CancellationToken::new(),
    );

    assert_eq!(
        result,
        Err(DerivativeReviewError::Probe(
            MediaProbeError::MalformedOutput
        ))
    );
    assert!(repository.committed.is_empty());
    assert_eq!(std::fs::read(&source_path).unwrap(), source_bytes);
    assert_eq!(
        std::fs::read_dir(&destination_directory).unwrap().count(),
        0
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn ffprobe_rejects_positive_duration_without_a_video_stream() {
    let root = temporary_directory("probe-no-video");
    let ffprobe = root.join("fake-ffprobe");
    write_executable(
        &ffprobe,
        "#!/bin/sh\nprintf '%s\\n' '{\"streams\":[{\"codec_type\":\"audio\",\"codec_name\":\"aac\"}],\"format\":{\"duration\":\"30.000000\"}}'\n",
    );

    let result = FfprobeMediaProbe::new(ffprobe, std::time::Duration::from_secs(2))
        .probe(Path::new("unused.mp4"), &CancellationToken::new());

    assert_eq!(result, Err(MediaProbeError::MissingVideoStream));
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn ffprobe_rejects_zero_duration_json() {
    let root = temporary_directory("probe-zero-duration");
    let ffprobe = root.join("fake-ffprobe");
    write_executable(
        &ffprobe,
        "#!/bin/sh\nprintf '%s\\n' '{\"streams\":[{\"codec_type\":\"video\",\"codec_name\":\"h264\",\"pix_fmt\":\"yuv420p\",\"width\":1920,\"height\":1080,\"avg_frame_rate\":\"60/1\"}],\"format\":{\"duration\":\"0.000000\"}}'\n",
    );

    let result = FfprobeMediaProbe::new(ffprobe, std::time::Duration::from_secs(2))
        .probe(Path::new("unused.mp4"), &CancellationToken::new());

    assert_eq!(result, Err(MediaProbeError::NonPositiveDuration));
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
enum ExpectedDiscordRejection {
    Codec,
    PixelFormat,
    Dimensions,
    FrameRate,
    AudioCodec,
}

#[cfg(unix)]
fn assert_incompatible_discord_fixture(
    name: &str,
    fixture: &str,
    expected: ExpectedDiscordRejection,
) {
    let root = temporary_directory(name);
    let source_path = root.join("source.mp4");
    let source_bytes = b"immutable source bytes";
    std::fs::write(&source_path, source_bytes).unwrap();
    let destination_directory = root.join("derivatives");
    std::fs::create_dir(&destination_directory).unwrap();
    let ffmpeg = root.join("fake-ffmpeg");
    write_executable(
        &ffmpeg,
        "#!/bin/sh\nfor argument in \"$@\"; do last=$argument; done\nprintf 'encoded bytes' > \"$last\"\n",
    );
    let ffprobe = root.join("fake-ffprobe");
    write_executable(
        &ffprobe,
        &format!("#!/bin/sh\nprintf '%s\\n' '{fixture}'\n"),
    );
    let clip = provisional_clip(source_path.clone());
    let mut repository = MemoryRepository::default();

    let result = prepare_derivative_and_review(
        &mut repository,
        &mut FfmpegTranscoder::new(ffmpeg, std::time::Duration::from_secs(2)),
        &mut FfprobeMediaProbe::new(ffprobe, std::time::Duration::from_secs(2)),
        &clip,
        DerivativeReviewRequest {
            artifact_id: format!("incompatible-{name}"),
            destination_directory: destination_directory.clone(),
            trim: Some(TrimRange::new(5_000, 35_000).unwrap()),
            profile: DerivativeProfile::Discord(DiscordEncodeProfile::default()),
            metadata: review_metadata(),
            reviewed_at_ms: 90_000,
        },
        &CancellationToken::new(),
    );

    let Err(DerivativeReviewError::DiscordIncompatible(actual)) = result else {
        panic!("expected incompatible Discord media for {name}");
    };
    assert!(matches!(
        (expected, actual),
        (
            ExpectedDiscordRejection::Codec,
            DiscordCompatibilityError::VideoCodec { .. }
        ) | (
            ExpectedDiscordRejection::PixelFormat,
            DiscordCompatibilityError::PixelFormat { .. }
        ) | (
            ExpectedDiscordRejection::Dimensions,
            DiscordCompatibilityError::Dimensions { .. }
        ) | (
            ExpectedDiscordRejection::FrameRate,
            DiscordCompatibilityError::FrameRate { .. }
        ) | (
            ExpectedDiscordRejection::AudioCodec,
            DiscordCompatibilityError::AudioCodec { .. }
        )
    ));
    assert!(repository.committed.is_empty());
    assert_eq!(std::fs::read(&source_path).unwrap(), source_bytes);
    assert_eq!(
        std::fs::read_dir(&destination_directory).unwrap().count(),
        0
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn discord_profile_rejects_each_incompatible_ffprobe_json_fixture() {
    let fixtures = [
        (
            "codec",
            "{\"streams\":[{\"codec_type\":\"video\",\"codec_name\":\"vp9\",\"pix_fmt\":\"yuv420p\",\"width\":1920,\"height\":1080,\"avg_frame_rate\":\"60/1\"}],\"format\":{\"duration\":\"30.000000\"}}",
            ExpectedDiscordRejection::Codec,
        ),
        (
            "pixel-format",
            "{\"streams\":[{\"codec_type\":\"video\",\"codec_name\":\"h264\",\"pix_fmt\":\"yuv444p\",\"width\":1920,\"height\":1080,\"avg_frame_rate\":\"60/1\"}],\"format\":{\"duration\":\"30.000000\"}}",
            ExpectedDiscordRejection::PixelFormat,
        ),
        (
            "width",
            "{\"streams\":[{\"codec_type\":\"video\",\"codec_name\":\"h264\",\"pix_fmt\":\"yuv420p\",\"width\":1921,\"height\":1080,\"avg_frame_rate\":\"60/1\"}],\"format\":{\"duration\":\"30.000000\"}}",
            ExpectedDiscordRejection::Dimensions,
        ),
        (
            "height",
            "{\"streams\":[{\"codec_type\":\"video\",\"codec_name\":\"h264\",\"pix_fmt\":\"yuv420p\",\"width\":1920,\"height\":1081,\"avg_frame_rate\":\"60/1\"}],\"format\":{\"duration\":\"30.000000\"}}",
            ExpectedDiscordRejection::Dimensions,
        ),
        (
            "frame-rate",
            "{\"streams\":[{\"codec_type\":\"video\",\"codec_name\":\"h264\",\"pix_fmt\":\"yuv420p\",\"width\":1920,\"height\":1080,\"avg_frame_rate\":\"61/1\"}],\"format\":{\"duration\":\"30.000000\"}}",
            ExpectedDiscordRejection::FrameRate,
        ),
        (
            "audio-codec",
            "{\"streams\":[{\"codec_type\":\"video\",\"codec_name\":\"h264\",\"pix_fmt\":\"yuv420p\",\"width\":1920,\"height\":1080,\"avg_frame_rate\":\"60/1\"},{\"codec_type\":\"audio\",\"codec_name\":\"opus\"}],\"format\":{\"duration\":\"30.000000\"}}",
            ExpectedDiscordRejection::AudioCodec,
        ),
    ];

    for (name, fixture, expected) in fixtures {
        assert_incompatible_discord_fixture(name, fixture, expected);
    }
}

#[cfg(unix)]
#[test]
fn discord_profile_rejects_verified_output_above_ten_mib_and_cleans_it() {
    let root = temporary_directory("discord-size");
    let source_path = root.join("source.mp4");
    let source_bytes = b"immutable source bytes";
    std::fs::write(&source_path, source_bytes).unwrap();
    let destination_directory = root.join("derivatives");
    std::fs::create_dir(&destination_directory).unwrap();
    let ffmpeg = root.join("fake-ffmpeg");
    write_executable(
        &ffmpeg,
        "#!/bin/sh\nfor argument in \"$@\"; do last=$argument; done\ntruncate -s 10485761 \"$last\"\n",
    );
    let ffprobe = root.join("fake-ffprobe");
    write_executable(
        &ffprobe,
        "#!/bin/sh\nprintf '%s\\n' '{\"streams\":[{\"codec_type\":\"video\",\"codec_name\":\"h264\",\"pix_fmt\":\"yuv420p\",\"width\":1920,\"height\":1080,\"avg_frame_rate\":\"60/1\"},{\"codec_type\":\"audio\",\"codec_name\":\"aac\"}],\"format\":{\"duration\":\"30.000000\"}}'\n",
    );
    let clip = provisional_clip(source_path.clone());
    let mut repository = MemoryRepository::default();

    let result = prepare_derivative_and_review(
        &mut repository,
        &mut FfmpegTranscoder::new(ffmpeg, std::time::Duration::from_secs(2)),
        &mut FfprobeMediaProbe::new(ffprobe, std::time::Duration::from_secs(2)),
        &clip,
        DerivativeReviewRequest {
            artifact_id: "oversize-discord".into(),
            destination_directory: destination_directory.clone(),
            trim: Some(TrimRange::new(5_000, 35_000).unwrap()),
            profile: DerivativeProfile::Discord(DiscordEncodeProfile::default()),
            metadata: review_metadata(),
            reviewed_at_ms: 90_000,
        },
        &CancellationToken::new(),
    );

    assert_eq!(
        result,
        Err(DerivativeReviewError::OutputTooLarge {
            actual_bytes: 10_485_761,
            max_bytes: 10_485_760,
        })
    );
    assert!(repository.committed.is_empty());
    assert_eq!(std::fs::read(&source_path).unwrap(), source_bytes);
    assert_eq!(
        std::fs::read_dir(&destination_directory).unwrap().count(),
        0
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn ordinary_review_copy_may_exceed_the_discord_size_ceiling() {
    let root = temporary_directory("large-review");
    let source_path = root.join("source.mp4");
    let source_file = std::fs::File::create(&source_path).unwrap();
    source_file.set_len(10_485_761).unwrap();
    source_file.sync_all().unwrap();
    let destination_directory = root.join("derivatives");
    std::fs::create_dir(&destination_directory).unwrap();
    let clip = provisional_clip(source_path.clone());
    let mut repository = MemoryRepository::default();

    let reviewed = prepare_derivative_and_review(
        &mut repository,
        &mut UnavailableTranscoder,
        &mut ValidProbe,
        &clip,
        DerivativeReviewRequest {
            artifact_id: "large-review".into(),
            destination_directory,
            trim: None,
            profile: DerivativeProfile::Review,
            metadata: review_metadata(),
            reviewed_at_ms: 90_000,
        },
        &CancellationToken::new(),
    )
    .unwrap();

    assert_eq!(
        reviewed.derivative().unwrap().artifact().bytes(),
        10_485_761
    );
    assert_eq!(std::fs::metadata(&source_path).unwrap().len(), 10_485_761);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn trim_outside_source_is_rejected_before_file_or_repository_changes() {
    let root = temporary_directory("invalid-trim");
    let source_path = root.join("source.mp4");
    let source_bytes = b"immutable source bytes";
    std::fs::write(&source_path, source_bytes).unwrap();
    let destination_directory = root.join("derivatives");
    let clip = provisional_clip(source_path.clone());
    let mut repository = MemoryRepository::default();

    let result = prepare_derivative_and_review(
        &mut repository,
        &mut UnavailableTranscoder,
        &mut ValidProbe,
        &clip,
        DerivativeReviewRequest {
            artifact_id: "invalid-trim".into(),
            destination_directory: destination_directory.clone(),
            trim: Some(TrimRange::new(5_000, 50_000).unwrap()),
            profile: DerivativeProfile::Review,
            metadata: review_metadata(),
            reviewed_at_ms: 90_000,
        },
        &CancellationToken::new(),
    );

    assert_eq!(result, Err(DerivativeReviewError::TrimOutsideSource));
    assert!(repository.committed.is_empty());
    assert_eq!(std::fs::read(&source_path).unwrap(), source_bytes);
    assert!(!destination_directory.exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn timed_out_transcode_is_killed_and_partial_staging_is_removed() {
    let root = temporary_directory("timeout");
    let source_path = root.join("source.mp4");
    let source_bytes = b"immutable source bytes";
    std::fs::write(&source_path, source_bytes).unwrap();
    let destination_directory = root.join("derivatives");
    std::fs::create_dir(&destination_directory).unwrap();
    let ffmpeg = root.join("fake-ffmpeg");
    write_executable(
        &ffmpeg,
        "#!/bin/sh\nfor argument in \"$@\"; do last=$argument; done\nprintf 'partial derivative' > \"$last\"\nwhile :; do :; done\n",
    );
    let clip = provisional_clip(source_path.clone());
    let mut repository = MemoryRepository::default();
    let started = std::time::Instant::now();

    let result = prepare_derivative_and_review(
        &mut repository,
        &mut FfmpegTranscoder::new(ffmpeg, std::time::Duration::from_millis(25)),
        &mut ValidProbe,
        &clip,
        DerivativeReviewRequest {
            artifact_id: "timeout".into(),
            destination_directory: destination_directory.clone(),
            trim: Some(TrimRange::new(5_000, 35_000).unwrap()),
            profile: DerivativeProfile::Review,
            metadata: review_metadata(),
            reviewed_at_ms: 90_000,
        },
        &CancellationToken::new(),
    );

    assert_eq!(
        result,
        Err(DerivativeReviewError::Transcode(TranscodeError::TimedOut))
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(1));
    assert!(repository.committed.is_empty());
    assert_eq!(std::fs::read(&source_path).unwrap(), source_bytes);
    assert_eq!(
        std::fs::read_dir(&destination_directory).unwrap().count(),
        0
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn failed_transcode_bounds_stderr_and_removes_partial_staging() {
    let root = temporary_directory("failure");
    let source_path = root.join("source.mp4");
    let source_bytes = b"immutable source bytes";
    std::fs::write(&source_path, source_bytes).unwrap();
    let destination_directory = root.join("derivatives");
    std::fs::create_dir(&destination_directory).unwrap();
    let ffmpeg = root.join("fake-ffmpeg");
    write_executable(
        &ffmpeg,
        "#!/bin/sh\nfor argument in \"$@\"; do last=$argument; done\nprintf 'partial derivative' > \"$last\"\ni=0; while [ \"$i\" -lt 20000 ]; do printf x >&2; i=$((i + 1)); done\nexit 7\n",
    );
    let clip = provisional_clip(source_path.clone());
    let mut repository = MemoryRepository::default();

    let result = prepare_derivative_and_review(
        &mut repository,
        &mut FfmpegTranscoder::new(ffmpeg, std::time::Duration::from_secs(2)),
        &mut ValidProbe,
        &clip,
        DerivativeReviewRequest {
            artifact_id: "failed".into(),
            destination_directory: destination_directory.clone(),
            trim: Some(TrimRange::new(5_000, 35_000).unwrap()),
            profile: DerivativeProfile::Review,
            metadata: review_metadata(),
            reviewed_at_ms: 90_000,
        },
        &CancellationToken::new(),
    );

    let Err(DerivativeReviewError::Transcode(TranscodeError::Failed {
        status,
        stderr,
        stderr_truncated,
    })) = result
    else {
        panic!("expected a failed transcode");
    };
    assert_eq!(status, Some(7));
    assert_eq!(stderr.len(), MAX_CAPTURED_PROCESS_BYTES);
    assert!(stderr_truncated);
    assert!(repository.committed.is_empty());
    assert_eq!(std::fs::read(&source_path).unwrap(), source_bytes);
    assert_eq!(
        std::fs::read_dir(&destination_directory).unwrap().count(),
        0
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn cancellation_kills_transcode_and_removes_partial_staging() {
    let root = temporary_directory("cancelled");
    let source_path = root.join("source.mp4");
    let source_bytes = b"immutable source bytes";
    std::fs::write(&source_path, source_bytes).unwrap();
    let destination_directory = root.join("derivatives");
    std::fs::create_dir(&destination_directory).unwrap();
    let ffmpeg = root.join("fake-ffmpeg");
    write_executable(
        &ffmpeg,
        "#!/bin/sh\nfor argument in \"$@\"; do last=$argument; done\nprintf 'partial derivative' > \"$last\"\nwhile :; do :; done\n",
    );
    let clip = provisional_clip(source_path.clone());
    let mut repository = MemoryRepository::default();
    let cancellation = CancellationToken::new();
    let cancellation_sender = cancellation.clone();
    let canceller = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(20));
        cancellation_sender.cancel();
    });

    let result = prepare_derivative_and_review(
        &mut repository,
        &mut FfmpegTranscoder::new(ffmpeg, std::time::Duration::from_secs(2)),
        &mut ValidProbe,
        &clip,
        DerivativeReviewRequest {
            artifact_id: "cancelled".into(),
            destination_directory: destination_directory.clone(),
            trim: Some(TrimRange::new(5_000, 35_000).unwrap()),
            profile: DerivativeProfile::Review,
            metadata: review_metadata(),
            reviewed_at_ms: 90_000,
        },
        &cancellation,
    );
    canceller.join().unwrap();

    assert_eq!(
        result,
        Err(DerivativeReviewError::Transcode(TranscodeError::Cancelled))
    );
    assert!(repository.committed.is_empty());
    assert_eq!(std::fs::read(&source_path).unwrap(), source_bytes);
    assert_eq!(
        std::fs::read_dir(&destination_directory).unwrap().count(),
        0
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn derivative_promotion_preserves_an_existing_collision() {
    let root = temporary_directory("collision");
    let source_path = root.join("source.mp4");
    let source_bytes = b"immutable source bytes";
    std::fs::write(&source_path, source_bytes).unwrap();
    let destination_directory = root.join("derivatives");
    std::fs::create_dir(&destination_directory).unwrap();
    let occupied = destination_directory.join("round-win-clip-7.mp4");
    std::fs::write(&occupied, b"existing derivative").unwrap();
    let clip = provisional_clip(source_path.clone());
    let mut repository = MemoryRepository::default();

    let reviewed = prepare_derivative_and_review(
        &mut repository,
        &mut UnavailableTranscoder,
        &mut ValidProbe,
        &clip,
        DerivativeReviewRequest {
            artifact_id: "collision".into(),
            destination_directory: destination_directory.clone(),
            trim: None,
            profile: DerivativeProfile::Review,
            metadata: review_metadata(),
            reviewed_at_ms: 90_000,
        },
        &CancellationToken::new(),
    )
    .unwrap();

    assert_eq!(std::fs::read(&occupied).unwrap(), b"existing derivative");
    assert_eq!(
        reviewed.derivative().unwrap().artifact().path(),
        destination_directory.join("round-win-clip-7-2.mp4")
    );
    assert_eq!(
        std::fs::read(reviewed.derivative().unwrap().artifact().path()).unwrap(),
        source_bytes
    );
    assert_eq!(std::fs::read(&source_path).unwrap(), source_bytes);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn repository_failure_removes_promoted_derivative_and_preserves_public_state() {
    let root = temporary_directory("repository-failure");
    let source_path = root.join("source.mp4");
    let source_bytes = b"immutable source bytes";
    std::fs::write(&source_path, source_bytes).unwrap();
    let destination_directory = root.join("derivatives");
    std::fs::create_dir(&destination_directory).unwrap();
    let clip = provisional_clip(source_path.clone());
    let original = clip.clone();
    let mut repository = MemoryRepository {
        committed: Vec::new(),
        fail: true,
    };

    let result = prepare_derivative_and_review(
        &mut repository,
        &mut UnavailableTranscoder,
        &mut ValidProbe,
        &clip,
        DerivativeReviewRequest {
            artifact_id: "repository-failure".into(),
            destination_directory: destination_directory.clone(),
            trim: None,
            profile: DerivativeProfile::Review,
            metadata: review_metadata(),
            reviewed_at_ms: 90_000,
        },
        &CancellationToken::new(),
    );

    assert_eq!(
        result,
        Err(DerivativeReviewError::Review(ReviewError::Repository(
            RepositoryError::new("injected repository failure")
        )))
    );
    assert_eq!(clip, original);
    assert!(repository.committed.is_empty());
    assert_eq!(std::fs::read(&source_path).unwrap(), source_bytes);
    assert_eq!(
        std::fs::read_dir(&destination_directory).unwrap().count(),
        0
    );
    std::fs::remove_dir_all(root).unwrap();
}
