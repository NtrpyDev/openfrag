//! Durable local Demo import primitives. This crate owns orchestration, not parser output.

use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

pub const MAX_DEMO_BYTES: u64 = 2_147_483_648;
pub const CHUNK_BYTES: usize = 8 * 1024 * 1024;
const DEMO_MAGIC: &[u8] = b"PBDEMS2\0";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    AwaitingImport,
    Validating,
    Hashing,
    Deduplicating,
    Copying,
    Parsing,
    Rating,
    LinkingClips,
    Ready,
    Error(ErrorCode),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ErrorCode {
    Io,
    Corrupt,
    Unsupported,
    Size,
    Parse,
    Analysis,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Progress {
    pub processed_bytes: u64,
    pub total_bytes: Option<u64>,
    pub indeterminate: bool,
    pub work_done: u64,
    pub heartbeat: u64,
}
impl Progress {
    pub fn fraction_millionths(&self) -> Option<u64> {
        self.total_bytes
            .filter(|&t| t > 0)
            .map(|t| (self.processed_bytes.min(t) * 1_000_000) / t)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CalculationIdentity {
    pub source_sha256: String,
    pub parser_commit: String,
    pub parser_build: String,
    pub generated_proto_build: String,
    pub requested_schema_hash: String,
    pub metric_definition_version: String,
    pub formula_version: String,
    pub evidence_semantics_epoch: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attempt {
    pub id: u64,
    pub state: State,
    pub retries: u8,
    pub next_retry_after: Option<Duration>,
    pub progress: Progress,
    pub lease_owner: Option<String>,
    pub lease_expires_at: Option<SystemTime>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DedupDecision {
    Copy,
    Ready,
    Resume,
    Parse,
    Rate,
    PriorFailure,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParserCapability {
    Unavailable {
        reason: String,
    },
    Pinned {
        commit: String,
        build: String,
        schema_hash: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedOutput {
    pub demo_metadata: String,
    pub participants: Vec<String>,
    pub rounds: u64,
    pub events: u64,
    pub receipts: Vec<String>,
    pub suspicious_empty: bool,
}

/// The pinned LaihoE parser is not vendored in this crate yet. Production callers must
/// surface `Unavailable` and never turn it into an empty Match.
pub fn parser_capability() -> ParserCapability {
    ParserCapability::Unavailable { reason: "demoparser commit ba39cc44cd5abfd7f34df2b3c0a7dd3630048311 requires vendored generated protos and is not yet integrated".into() }
}

pub trait ImportStore {
    fn save_attempt(&mut self, attempt: &Attempt) -> Result<(), ErrorCode>;
    fn load_attempt(&self, id: u64) -> Option<Attempt>;
}

pub trait ParserAdapter {
    type Parsed;
    type Error: std::fmt::Display;
    fn parse(
        &self,
        demo: &Path,
        progress: &mut dyn FnMut(u64),
    ) -> Result<Self::Parsed, Self::Error>;
    fn rate(&self, parsed: &Self::Parsed) -> Result<(), Self::Error>;
}

pub fn validate(path: &Path, free_bytes: u64) -> Result<u64, ErrorCode> {
    if path.is_symlink() {
        return Err(ErrorCode::Corrupt);
    }
    let meta = fs::metadata(path).map_err(|_| ErrorCode::Io)?;
    if !meta.is_file() {
        return Err(ErrorCode::Corrupt);
    }
    if meta.len() > MAX_DEMO_BYTES {
        return Err(ErrorCode::Size);
    }
    if free_bytes < meta.len() + meta.len() / 10 {
        return Err(ErrorCode::Io);
    }
    if meta.len() < DEMO_MAGIC.len() as u64 {
        return Err(ErrorCode::Corrupt);
    }
    let mut f = fs::File::open(path).map_err(|_| ErrorCode::Io)?;
    let mut magic = [0u8; DEMO_MAGIC.len()];
    f.read_exact(&mut magic).map_err(|_| ErrorCode::Corrupt)?;
    if magic != DEMO_MAGIC {
        return Err(ErrorCode::Unsupported);
    }
    Ok(meta.len())
}

pub fn hash_file(
    src: &Path,
    total: u64,
    progress: &mut dyn FnMut(Progress),
) -> Result<String, ErrorCode> {
    let before = fs::metadata(src).map_err(|_| ErrorCode::Io)?;
    let mut input = fs::File::open(src).map_err(|_| ErrorCode::Io)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; CHUNK_BYTES];
    let mut done = 0;
    loop {
        let n = input.read(&mut buf).map_err(|_| ErrorCode::Io)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        done += n as u64;
        progress(Progress {
            processed_bytes: done,
            total_bytes: Some(total),
            indeterminate: false,
            work_done: 0,
            heartbeat: done,
        });
    }
    let after = fs::metadata(src).map_err(|_| ErrorCode::Io)?;
    if before.len() != after.len()
        || before.modified().ok() != after.modified().ok()
        || done != total
    {
        return Err(ErrorCode::Io);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

pub fn copy_content_addressed(
    src: &Path,
    dst: &Path,
    expected_hash: &str,
    total: u64,
    progress: &mut dyn FnMut(Progress),
) -> Result<PathBuf, ErrorCode> {
    if dst.exists() {
        return Ok(dst.to_path_buf());
    }
    let parent = dst.parent().ok_or(ErrorCode::Io)?;
    fs::create_dir_all(parent).map_err(|_| ErrorCode::Io)?;
    let tmp = parent.join(format!(
        ".staging-{}-{:x}",
        std::process::id(),
        Sha256::digest(
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
                .to_le_bytes()
        )
    ));
    let mut out = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .map_err(|_| ErrorCode::Io)?;
    let mut input = fs::File::open(src).map_err(|_| ErrorCode::Io)?;
    let mut buf = vec![0u8; CHUNK_BYTES];
    let mut done = 0;
    let mut h = Sha256::new();
    let result = (|| {
        loop {
            let n = input.read(&mut buf).map_err(|_| ErrorCode::Io)?;
            if n == 0 {
                break;
            }
            h.update(&buf[..n]);
            out.write_all(&buf[..n]).map_err(|_| ErrorCode::Io)?;
            done += n as u64;
            progress(Progress {
                processed_bytes: done,
                total_bytes: Some(total),
                indeterminate: false,
                work_done: 0,
                heartbeat: done,
            });
        }
        out.sync_all().map_err(|_| ErrorCode::Io)?;
        if done != total || format!("{:x}", h.finalize()) != expected_hash {
            return Err(ErrorCode::Corrupt);
        }
        fs::rename(&tmp, dst).map_err(|_| ErrorCode::Io)?;
        if let Some(p) = dst.parent() {
            fs::File::open(p)
                .and_then(|f| f.sync_all())
                .map_err(|_| ErrorCode::Io)?;
        }
        Ok(dst.to_path_buf())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

pub fn hash_and_copy(
    src: &Path,
    dst: &Path,
    total: u64,
    progress: &mut dyn FnMut(Progress),
) -> Result<String, ErrorCode> {
    let mut input = fs::File::open(src).map_err(|_| ErrorCode::Io)?;
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent).map_err(|_| ErrorCode::Io)?;
    }
    let tmp = dst.with_extension(format!("staging-{}", std::process::id()));
    let mut output = fs::File::create(&tmp).map_err(|_| ErrorCode::Io)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; CHUNK_BYTES];
    let mut done = 0;
    let result = (|| {
        loop {
            let n = input.read(&mut buf).map_err(|_| ErrorCode::Io)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            output.write_all(&buf[..n]).map_err(|_| ErrorCode::Io)?;
            done += n as u64;
            progress(Progress {
                processed_bytes: done,
                total_bytes: Some(total),
                indeterminate: false,
                work_done: 0,
                heartbeat: done,
            });
        }
        output.sync_all().map_err(|_| ErrorCode::Io)?;
        fs::rename(&tmp, dst).map_err(|_| ErrorCode::Io)?;
        Ok(format!("{:x}", hasher.finalize()))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

pub fn retry_delay(automatic_retry: u8) -> Option<Duration> {
    match automatic_retry {
        0 => Some(Duration::from_secs(2)),
        1 => Some(Duration::from_secs(10)),
        2 => Some(Duration::from_secs(60)),
        _ => None,
    }
}

pub fn lease_available(attempt: &Attempt, now: SystemTime, owner: &str) -> bool {
    attempt
        .lease_owner
        .as_deref()
        .map(|x| x == owner)
        .unwrap_or(true)
        || attempt.lease_expires_at.map(|x| x <= now).unwrap_or(true)
}

pub fn choose_dedup(
    artifact_exists: bool,
    run_succeeded: bool,
    run_running: bool,
    parser_changed: bool,
    formula_changed: bool,
    prior_failed: bool,
) -> DedupDecision {
    if !artifact_exists {
        DedupDecision::Copy
    } else if run_succeeded && !parser_changed && !formula_changed {
        DedupDecision::Ready
    } else if run_running {
        DedupDecision::Resume
    } else if parser_changed {
        DedupDecision::Parse
    } else if formula_changed {
        DedupDecision::Rate
    } else if prior_failed {
        DedupDecision::PriorFailure
    } else {
        DedupDecision::Parse
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    struct Fake;
    impl ParserAdapter for Fake {
        type Parsed = u8;
        type Error = &'static str;
        fn parse(&self, _: &Path, p: &mut dyn FnMut(u64)) -> Result<u8, Self::Error> {
            p(1);
            Ok(1)
        }
        fn rate(&self, _: &u8) -> Result<(), Self::Error> {
            Ok(())
        }
    }
    #[test]
    fn validates_limit_and_headroom() {
        let d = tempdir().unwrap();
        let p = d.path().join("x.dem");
        fs::write(&p, DEMO_MAGIC).unwrap();
        assert_eq!(validate(&p, 7), Err(ErrorCode::Io));
        assert_eq!(validate(&p, 8), Ok(8));
    }
    #[test]
    fn hashes_and_copies_atomically() {
        let d = tempdir().unwrap();
        let s = d.path().join("a.dem");
        let t = d.path().join("store/a.dem");
        fs::write(&s, [DEMO_MAGIC, b"demo bytes"].concat()).unwrap();
        let mut seen = Vec::new();
        let h = hash_and_copy(&s, &t, 18, &mut |p| seen.push(p)).unwrap();
        assert!(t.is_file());
        assert_eq!(seen.last().unwrap().fraction_millionths(), Some(1_000_000));
        assert_eq!(h.len(), 64);
    }
    #[test]
    fn retries_are_exact() {
        assert_eq!(retry_delay(0), Some(Duration::from_secs(2)));
        assert_eq!(retry_delay(1), Some(Duration::from_secs(10)));
        assert_eq!(retry_delay(2), Some(Duration::from_secs(60)));
        assert_eq!(retry_delay(3), None);
    }
    #[test]
    fn dedup_branches_are_deterministic() {
        assert_eq!(
            choose_dedup(false, false, false, false, false, false),
            DedupDecision::Copy
        );
        assert_eq!(
            choose_dedup(true, true, false, false, false, false),
            DedupDecision::Ready
        );
        assert_eq!(
            choose_dedup(true, false, true, false, false, false),
            DedupDecision::Resume
        );
        assert_eq!(
            choose_dedup(true, false, false, true, false, false),
            DedupDecision::Parse
        );
        assert_eq!(
            choose_dedup(true, false, false, false, true, false),
            DedupDecision::Rate
        );
    }
    #[test]
    fn fake_adapter_is_not_production_parser() {
        let d = tempdir().unwrap();
        let p = d.path().join("x");
        fs::write(&p, DEMO_MAGIC).unwrap();
        let f = Fake;
        let mut n = 0;
        let parsed = f.parse(&p, &mut |x| n = x).unwrap();
        f.rate(&parsed).unwrap();
        assert_eq!(n, 1);
    }
}
